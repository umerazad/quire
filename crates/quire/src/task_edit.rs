//! Task changes made from task lists rather than inside a page editor: state changes, ticking,
//! and dates for one or several tasks across pages, and capture into the Inbox page. Each reads
//! every touched page fresh, plans with the NR18 task semantics, and writes all pages or none
//! through `page_writes`, so a task that changed since it was listed is refused, never
//! overwritten.

use crate::notebook_backend::NotebookBackend;
use crate::{
    notebook_session::OpenPage,
    page_writes::{self, PageWrite},
    query_index::ask,
    tasks::{Analysis, TaskContext},
};
use gpui::{App, AppContext as _, AsyncApp, Entity, Task};
use quire_core::{
    markdown::page_name,
    source_text::SourceText,
    tasks::{
        Action, DailyNames, DateField, TaskState, capture as capture_task, created_stamp,
        find_current,
    },
};
use quire_store::index::{IndexClient, TaskItem};
use std::sync::Arc;

pub const STALE_TASKS: &str = "changed since it was listed. Review the list and try again.";
const NOT_READY: &str =
    "A page could not be read as Markdown. Rebuild Quire with its bundled grammars.";
pub const INBOX: &str = "Inbox";

/// A listed task: enough to find it again in a fresh reading of its page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskRef {
    pub page: String,
    pub title: String,
    pub id: Option<String>,
    pub key: String,
    pub raw: String,
}

impl From<&TaskItem> for TaskRef {
    fn from(task: &TaskItem) -> Self {
        Self {
            page: task.source.page.to_lossy_string(),
            title: task.source.title.clone(),
            id: task.id.clone(),
            key: task.key.clone(),
            raw: task.raw.clone(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TaskChange {
    State(Action),
    /// Sets, replaces, or with no date removes one date stamp.
    Date(DateField, Option<String>),
}

fn clock(cx: &App) -> Result<(String, DailyNames), String> {
    let context = cx.global::<TaskContext>();
    context
        .today()
        .zip(context.days())
        .ok_or_else(|| crate::tasks::NO_TIME_ZONE.to_owned())
}

/// Plans one page's edits for its listed tasks; None when one of them changed since listed.
async fn plan_page(
    source: page_writes::PageSource,
    tasks: &[&TaskRef],
    change: &TaskChange,
    cx: &mut AsyncApp,
) -> Result<Option<PageWrite>, String> {
    let (original, normalized) = SourceText::prepare(source.stored.clone());
    let work = cx.update(|cx| Analysis::detached(&normalized, cx));
    let analysis = cx
        .background_spawn(work)
        .await
        .ok_or_else(|| NOT_READY.to_owned())?;
    let edits = cx.update(|cx| {
        let (today, days) = clock(cx)?;
        analysis.with_page("", |page| -> Result<_, String> {
            let current = page.tasks(&days);
            let mut edits = Vec::new();
            for task in tasks {
                let Ok(found) = find_current(&current, task.id.as_deref(), &task.key, &task.raw)
                else {
                    return Ok(None);
                };
                match change {
                    TaskChange::State(action) => edits.extend(found.changes(*action, &today)),
                    TaskChange::Date(field, date) => {
                        edits.extend(found.set_date(*field, date.as_deref(), &days)?)
                    }
                }
            }
            edits.sort_by_key(|edit| edit.range.start);
            Ok(Some(edits))
        })
    })?;
    let Some(edits) = edits else {
        return Ok(None);
    };
    let text = original.apply(edits).map_err(|error| {
        format!(
            "Could not change tasks in {}: {error}. Review the page and try again.",
            source.path
        )
    })?;
    Ok(Some(source.write(text)))
}

/// Applies `change` to every listed task, writing every touched page or none. Refuses, naming
/// the pages, when any task changed since it was listed. Returns how many tasks changed.
pub fn update(
    tasks: Vec<TaskRef>,
    change: TaskChange,
    open_pages: Vec<Entity<OpenPage>>,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    cx.spawn(async move |cx| {
        let mut pages: Vec<(&str, Vec<&TaskRef>)> = Vec::new();
        for task in &tasks {
            match pages.iter_mut().find(|(page, _)| *page == task.page) {
                Some((_, group)) => group.push(task),
                None => pages.push((&task.page, vec![task])),
            }
        }
        let mut writes = Vec::new();
        let mut changed = Vec::new();
        for (page, group) in &pages {
            let source = page_writes::read((*page).to_owned(), &open_pages, &storage, cx).await?;
            let planned = match source {
                Some(source) => plan_page(source, group, &change, cx).await?,
                None => None,
            };
            match planned {
                Some(write) => writes.push(write),
                None => changed.push(group[0].title.as_str()),
            }
        }
        if !changed.is_empty() {
            return Err(stale(&changed));
        }
        let titles: Vec<&str> = pages
            .iter()
            .map(|(_, group)| group[0].title.as_str())
            .collect();
        let stale_all = stale(&titles);
        let written = cx.update(|cx| page_writes::apply(writes, storage, STALE_TASKS, cx));
        match written.await {
            Ok(_) => Ok(tasks.len()),
            Err(error) if error == STALE_TASKS => Err(stale_all),
            Err(error) => Err(error),
        }
    })
}

/// Names the pages whose tasks changed since they were listed.
fn stale(titles: &[&str]) -> String {
    match titles {
        [one] => format!("A task on {one} {STALE_TASKS}"),
        many => format!("Tasks on {} {STALE_TASKS}", many.join(", ")),
    }
}

/// Where a captured task went and the list it belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Captured {
    pub page: String,
    pub state: TaskState,
}

/// A project to link a captured task to: the page and how it is titled and named.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CaptureProject {
    pub page: String,
    pub title: String,
    pub path: String,
}

/// Where quick capture appends: the Inbox page, or today's daily page, found by its date and
/// created at its layout path when missing, as Settings > Tasks says.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureTo {
    Inbox,
    Today {
        date: String,
        title: String,
        folder: String,
    },
}

/// The reference `captureTask`: appends `- [ ] text #inbox [[Project]] ➕ [[today]]` to the Inbox
/// page, replacing a new page's lone empty bullet, or creates the page with it. A task captured
/// to today's page needs no ➕ date.
pub fn capture(
    value: String,
    project: Option<CaptureProject>,
    to: CaptureTo,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<Captured, String>> {
    cx.spawn(async move |cx| {
        let (inbox, target, folder) = match &to {
            CaptureTo::Inbox => (
                ask(index.find_page(INBOX.into())).await?,
                INBOX.to_owned(),
                String::new(),
            ),
            CaptureTo::Today {
                date,
                title,
                folder,
            } => {
                let date = date.clone();
                let found =
                    ask(index.with_generation(move |index| index.daily_page(&date))).await?;
                (found.1, title.clone(), folder.clone())
            }
        };
        let today = matches!(to, CaptureTo::Today { .. });
        let link = match &project {
            Some(project) => {
                let found = ask(index.find_page(project.title.clone())).await?;
                Some(
                    if found.is_some_and(|found| found.page.to_lossy_string() == project.page) {
                        project.title.clone()
                    } else {
                        page_name(&project.path).to_owned()
                    },
                )
            }
            None => None,
        };
        let work = cx.update(|cx| Analysis::detached(&value, cx));
        let value_tags = cx
            .background_spawn(work)
            .await
            .ok_or_else(|| NOT_READY.to_owned())?
            .tags;
        let state = value_tags
            .iter()
            .filter_map(|tag| TaskState::parse(&tag.text.strip_prefix('#')?.to_lowercase()))
            .next()
            .unwrap_or(TaskState::Inbox);
        let stamp = |title: &str, cx: &mut AsyncApp| -> Result<String, String> {
            if today {
                return Ok(String::new());
            }
            cx.update(|cx| {
                let (today, days) = clock(cx)?;
                Ok(created_stamp(title, &today, &days))
            })
        };
        let source = match &inbox {
            Some(found) => {
                page_writes::read(found.page.to_lossy_string(), &open_pages, &storage, cx).await?
            }
            None => None,
        };
        let Some(source) = source else {
            let text = capture_task(
                "",
                &value,
                &value_tags,
                link.as_deref(),
                &stamp(&target, cx)?,
            )?
            .replacement;
            let page =
                crate::navigation::create_page_with(storage, index, target, folder, text).await?;
            return Ok(Captured { page, state });
        };
        let title = inbox.map_or(target, |found| found.title);
        let (original, normalized) = SourceText::prepare(source.stored.clone());
        let edit = capture_task(
            &normalized,
            &value,
            &value_tags,
            link.as_deref(),
            &stamp(&title, cx)?,
        )?;
        let text: Arc<str> = original.apply(vec![edit]).map_err(|error| {
            format!("Could not add the task to {title}: {error}. Open the page and add it there.")
        })?;
        let page = source.page.clone();
        let written = cx.update(|cx| {
            page_writes::apply(
                vec![source.write(text)],
                storage,
                "The capture page changed while capturing. Capture the task again.",
                cx,
            )
        });
        written.await?;
        Ok(Captured { page, state })
    })
}
