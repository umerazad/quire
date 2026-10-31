//! The five write tools (reference `notebook-tools.ts` write handlers and `answerCall`): each
//! checks its target against the live notebook and proposes a change, which waits for review,
//! or with AI writes set to Apply, applies at once.

use super::{
    bridge::Call,
    changes::{self, Request, TERMINAL_RUN, Target},
    host::message,
    live::LiveNotebook,
    read::{Failure, analyze, find, page_text, query, require, resolve, utf16_prefix},
};
use crate::task_edit::CaptureTo;
use gpui::{App, AsyncApp, Task};
use quire_core::{
    ai::{
        tidy,
        tools::{AWAITING, Writes, as_block, block_key, line_at, without_marker},
    },
    markdown::safe_page_path,
    tasks::TaskState,
};
use quire_store::index::PageRef;
use serde_json::{Value, json};

/// The notice a change staged from a terminal raises.
pub const STAGED: &str = "An AI session in a terminal proposed a change. Open AI changes from the page menu to apply or reject it.";

fn text_of(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn target(page: &PageRef) -> Target {
    Target {
        page: page.page.to_lossy_string(),
        path: page.path.clone(),
        title: page.title.clone(),
    }
}

fn today(notebook: &LiveNotebook) -> Result<String, Failure> {
    notebook.today.clone().map_err(Failure::Internal)
}

async fn append_block(
    input: &Value,
    notebook: &LiveNotebook,
) -> Result<(String, Request), Failure> {
    let page = text_of(input, "page").unwrap_or_default();
    let today = today(notebook)?;
    let is_today = page.trim().eq_ignore_ascii_case("today");
    let lookup = page.clone();
    let date = today.clone();
    let found = query(notebook, move |index| find(index, &lookup, &date)).await?;
    if found.is_none() && !is_today {
        return Err(Failure::Tool(format!(
            "No page named \"{page}\". Use create_page to make it, or list_pages to find its title."
        )));
    }
    let title = found.as_ref().map_or(today, |page| page.title.clone());
    Ok((
        format!("Append to [[{title}]]"),
        Request::Append {
            target: found.as_ref().map(target),
            title,
            after: as_block(&text_of(input, "text").unwrap_or_default()),
        },
    ))
}

async fn create_page(input: &Value, notebook: &LiveNotebook) -> Result<(String, Request), Failure> {
    let title = text_of(input, "title")
        .unwrap_or_default()
        .trim()
        .to_owned();
    let wanted = title.clone();
    let exists = query(notebook, move |index| {
        Ok(Ok(index.find_page(&wanted)?.is_some()))
    })
    .await?;
    if exists {
        return Err(Failure::Tool(format!(
            "A page named \"{title}\" already exists. Use append_block to add to it."
        )));
    }
    safe_page_path(&title).map_err(|error| Failure::Tool(error.to_string()))?;
    Ok((
        format!("Create [[{title}]]"),
        Request::Create {
            title,
            after: text_of(input, "text").unwrap_or_default(),
        },
    ))
}

async fn replace_block(
    input: &Value,
    notebook: &LiveNotebook,
    cx: &mut AsyncApp,
) -> Result<(String, Request), Failure> {
    let key = text_of(input, "key");
    let id = text_of(input, "id").map(|id| id.strip_prefix('^').unwrap_or(&id).to_owned());
    let (page, id) = match (&key, id) {
        (Some(_), _) => {
            let Some(page) = text_of(input, "page") else {
                return Err(Failure::Tool(
                    "Pass the page with a block key: the page read_page returned it for.".into(),
                ));
            };
            let date = today(notebook)?;
            (
                query(notebook, move |index| require(index, &page, &date)).await?,
                None,
            )
        }
        (None, Some(id)) => {
            let (page, id) = resolve(notebook, id).await?;
            (page, Some(id))
        }
        (None, None) => {
            return Err(Failure::Tool(
                "Pass the block's id, or its key and page from read_page.".into(),
            ));
        }
    };
    let text = page_text(notebook, &page, cx)
        .await?
        .ok_or_else(|| Failure::Tool(changes::STALE.into()))?;
    let analysis = analyze(&text, cx).await?;
    let block = analysis.blocks.iter().find(|block| match (&id, &key) {
        (Some(id), _) => block.id.as_deref() == Some(id.as_str()),
        (None, Some(key)) => {
            block_key(line_at(&text, block.content_from as usize), &block.text) == *key
        }
        (None, None) => false,
    });
    let Some(block) = block else {
        return Err(Failure::Tool(match (&id, &key) {
            (None, Some(key)) => format!(
                "Block {key} changed or moved since it was read. Call read_page on {} again and use the new key.",
                page.title
            ),
            _ => format!(
                "Block {} is missing. Restore its source or update this reference.",
                id.clone().unwrap_or_default()
            ),
        }));
    };
    let written = text_of(input, "text").unwrap_or_default();
    let after = match &id {
        Some(id) => without_marker(&written, id),
        None => &written,
    }
    .trim()
    .to_owned();
    if after.contains('\n') && block.kind == "list" {
        return Err(Failure::Tool(
            "Block text for a list item must be one line. Append child blocks with append_block instead."
                .into(),
        ));
    }
    let named = match (&id, &key) {
        (Some(id), _) => format!("^{id}"),
        (None, Some(key)) => format!(
            "line {}",
            key.get(1..)
                .and_then(|rest| rest.split('-').next())
                .unwrap_or_default()
        ),
        (None, None) => String::new(),
    };
    Ok((
        format!("Replace block {named} on [[{}]]", page.title),
        Request::Replace {
            target: target(&page),
            id,
            key: key.clone(),
            before: block.text.clone(),
            after,
        },
    ))
}

async fn set_task_state(
    input: &Value,
    notebook: &LiveNotebook,
) -> Result<(String, Request), Failure> {
    let key = text_of(input, "key").unwrap_or_default();
    let state = text_of(input, "state")
        .as_deref()
        .and_then(TaskState::parse)
        .ok_or_else(|| Failure::Internal("the task state is unknown".into()))?;
    let task = query(notebook, move |index| {
        Ok(index
            .tasks()?
            .into_iter()
            .find(|task| task.key == key)
            .ok_or_else(|| {
                "No task has that key, or it changed. Call tasks again for current keys.".to_owned()
            }))
    })
    .await?;
    Ok((
        format!(
            "Mark \"{}\" {}",
            utf16_prefix(&task.text, 60),
            state.as_str()
        ),
        Request::TaskState {
            target: target(&task.source),
            id: task.id.clone(),
            key: task.key.clone(),
            raw: task.raw.clone(),
            state,
            before: format!("{} ({})", task.text, task.state.as_str()),
            after: format!("{} ({})", task.text, state.as_str()),
        },
    ))
}

async fn capture_task(
    input: &Value,
    notebook: &LiveNotebook,
) -> Result<(String, Request), Failure> {
    let project = match text_of(input, "project").filter(|project| !project.is_empty()) {
        Some(name) => {
            let wanted = name.clone();
            let found = query(notebook, move |index| Ok(Ok(index.find_page(&wanted)?))).await?;
            Some(found.map(|page| target(&page)).ok_or_else(|| {
                Failure::Tool(format!(
                    "No project page named \"{name}\". Use list_pages to find it, or leave project out."
                ))
            })?)
        }
        None => None,
    };
    let text = text_of(input, "text").unwrap_or_default().trim().to_owned();
    if text.contains(['\r', '\n']) {
        return Err(Failure::Tool(
            "Capture one task at a time, on one line.".into(),
        ));
    }
    let title = match notebook.capture {
        Ok(CaptureTo::Today { .. }) => "today's daily page",
        _ => "Inbox",
    };
    Ok((
        format!("Capture task \"{}\"", utf16_prefix(&text, 60)),
        Request::Capture {
            title: title.into(),
            after: format!("- [ ] {text}"),
            text,
            project,
        },
    ))
}

async fn propose(
    call: &Call,
    notebook: &LiveNotebook,
    cx: &mut AsyncApp,
) -> Result<(String, Request), Failure> {
    let input = &call.input;
    match call.name.as_str() {
        "append_block" => append_block(input, notebook).await,
        "create_page" => create_page(input, notebook).await,
        "replace_block" => replace_block(input, notebook, cx).await,
        "set_task_state" => set_task_state(input, notebook).await,
        "capture_task" => capture_task(input, notebook).await,
        other => Err(Failure::Internal(format!("{other} is not a write tool"))),
    }
}

/// Why Tidy may not make `request` on the day's page `day`, or None when it may.
async fn tidy_refusal(
    request: &Request,
    day: &str,
    notebook: &LiveNotebook,
    cx: &mut AsyncApp,
) -> Result<Option<String>, Failure> {
    let Request::Replace {
        target,
        before,
        after,
        ..
    } = request
    else {
        return Ok(Some(tidy::TIDY_ONLY.into()));
    };
    if target.page != day {
        return Ok(Some(tidy::OTHER_PAGE.into()));
    }
    let named = tidy::link_targets(after);
    let existing = query(notebook, move |index| {
        let found: Result<Vec<String>, _> = named
            .into_iter()
            .filter_map(|title| match index.find_page(&title) {
                Ok(Some(_)) => Some(Ok(title)),
                Ok(None) => None,
                Err(error) => Some(Err(error)),
            })
            .collect();
        Ok(Ok(found?))
    })
    .await?;
    let daily = cx.update(|cx| {
        let tasks = cx.global::<crate::tasks::TaskContext>();
        let (layout, calendar) = (tasks.layout.clone(), tasks.calendar.clone());
        move |title: &str| {
            calendar
                .as_ref()
                .and_then(|calendar| quire_core::daily::daily_date(title, &layout, calendar))
                .is_some()
        }
    });
    Ok(tidy::check_change(
        before,
        after,
        |title| existing.iter().any(|found| found == title),
        daily,
    ))
}

/// Proposes the call's change and stages it; with AI writes set to Apply, applies it at once.
/// A Tidy run only stages `replace_block` changes to its day that pass Tidy's limits, and they
/// always wait for review.
pub fn answer(call: Call, notebook: LiveNotebook, cx: &mut App) -> Task<Result<Value, String>> {
    if call.writes == Writes::Off {
        return Task::ready(Err(quire_core::ai::tools::WRITES_OFF.into()));
    }
    let day = super::host::tidy_day(call.run.as_deref(), cx);
    if day.is_some() && call.name != "replace_block" {
        return Task::ready(Err(tidy::TIDY_ONLY.into()));
    }
    cx.spawn(async move |cx| {
        let (summary, request) = propose(&call, &notebook, cx)
            .await
            .map_err(|failure| message(&call.name, failure))?;
        if let Some(day) = &day
            && let Some(refusal) = tidy_refusal(&request, day, &notebook, cx)
                .await
                .map_err(|failure| message(&call.name, failure))?
        {
            return Err(refusal);
        }
        let change = request.to_json();
        let run = call.run.as_deref().unwrap_or(TERMINAL_RUN).to_owned();
        let id = cx.update(|cx| {
            changes::ledger(cx).update(cx, |ledger, cx| {
                ledger.stage(&run, &notebook.identity, request, cx)
            })
        });
        let mut result = json!({ "summary": summary, "change": change, "change_id": id });
        if call.writes != Writes::Apply || day.is_some() {
            result["status"] = "awaiting review".into();
            result["note"] = AWAITING.into();
            if run == TERMINAL_RUN {
                cx.update(|cx| crate::outline::show_notice(STAGED, cx));
            }
            return Ok(result);
        }
        match cx.update(|cx| changes::apply(&id, cx)).await {
            Ok(()) => {
                result["status"] = "applied".into();
                Ok(result)
            }
            Err(error) => {
                cx.update(|cx| changes::reject(&id, cx));
                Err(error)
            }
        }
    })
}
