//! The AI change ledger (reference `ai-changes.ts`): writes the AI proposed, held in memory until
//! applied, rejected, or undone. A change belongs to the notebook it was proposed for and never
//! applies to another. Applying edits one page as one step: an open page's buffer in one
//! transaction (one undo step, then saved), or a closed page through `page_writes`, refused when
//! the page changed after it was read. A change that no longer matches its target is refused,
//! never forced.

use super::live::{self, LiveNotebook};
use crate::{
    notebook_session::OpenPage, page_persistence::PagePersistence, page_writes, query_index::ask,
    task_edit::CaptureTo,
};
use gpui::{App, AppContext as _, AsyncApp, Context, Entity, Global, Task, WeakEntity};
use quire_core::{
    markdown::page_name,
    source_text::SourceText,
    tasks::{Action, Edit, TaskState, capture as capture_edit, created_stamp, find_current},
};
use serde_json::{Value, json};

pub const STALE: &str = "This block changed since the AI proposed it. Ask again.";
const GONE: &str = "That AI change is gone. Ask the AI again if you still want it.";
const NOT_READY: &str =
    "A page could not be read as Markdown. Rebuild Quire with its bundled grammars.";
/// Calls from a terminal session have no in-app run to group their changes.
pub const TERMINAL_RUN: &str = "terminal";

/// A page a change names: its identity, path, and title when proposed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub page: String,
    pub path: String,
    pub title: String,
}

/// What a write tool proposes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// Adds a block at the end of a page; without a page, today's daily page, started if missing.
    Append {
        target: Option<Target>,
        title: String,
        after: String,
    },
    Create {
        title: String,
        after: String,
    },
    /// Replaces a block's text, found by its ID or, for a block without one, by its key.
    Replace {
        target: Target,
        id: Option<String>,
        key: Option<String>,
        before: String,
        after: String,
    },
    TaskState {
        target: Target,
        id: Option<String>,
        key: String,
        raw: String,
        state: TaskState,
        before: String,
        after: String,
    },
    Capture {
        title: String,
        text: String,
        project: Option<Target>,
        after: String,
    },
}

impl Request {
    pub fn tool(&self) -> &'static str {
        match self {
            Self::Append { .. } => "append_block",
            Self::Create { .. } => "create_page",
            Self::Replace { .. } => "replace_block",
            Self::TaskState { .. } => "set_task_state",
            Self::Capture { .. } => "capture_task",
        }
    }

    /// The page title the change is about.
    pub fn title(&self) -> &str {
        match self {
            Self::Append { title, .. }
            | Self::Create { title, .. }
            | Self::Capture { title, .. } => title,
            Self::Replace { target, .. } | Self::TaskState { target, .. } => &target.title,
        }
    }

    /// The text before and after the change, as the change list shows it.
    pub fn texts(&self) -> (&str, &str) {
        match self {
            Self::Append { after, .. }
            | Self::Create { after, .. }
            | Self::Capture { after, .. } => ("", after),
            Self::Replace { before, after, .. } | Self::TaskState { before, after, .. } => {
                (before, after)
            }
        }
    }

    /// The change as the reference `ChangeRequest` reports it to the model.
    pub fn to_json(&self) -> Value {
        let tool = self.tool();
        match self {
            Self::Append {
                target,
                title,
                after,
            } => {
                let mut change = json!({ "tool": tool });
                if let Some(target) = target {
                    change["path"] = target.path.clone().into();
                }
                change["title"] = title.clone().into();
                change["before"] = "".into();
                change["after"] = after.clone().into();
                change
            }
            Self::Create { title, after } => {
                json!({ "tool": tool, "title": title, "before": "", "after": after })
            }
            Self::Replace {
                target,
                id,
                key,
                before,
                after,
            } => {
                let mut change =
                    json!({ "tool": tool, "path": target.path, "title": target.title });
                match id {
                    Some(id) => change["blockId"] = id.clone().into(),
                    None => change["blockKey"] = key.clone().into(),
                }
                change["before"] = before.clone().into();
                change["after"] = after.clone().into();
                change
            }
            Self::TaskState {
                target,
                key,
                raw,
                state,
                before,
                after,
                ..
            } => json!({
                "tool": tool,
                "path": target.path,
                "title": target.title,
                "key": key,
                "raw": raw,
                "state": state.as_str(),
                "before": before,
                "after": after,
            }),
            Self::Capture {
                title,
                text,
                project,
                after,
            } => {
                let mut change = json!({ "tool": tool, "title": title, "text": text });
                if let Some(project) = project {
                    change["project"] = project.path.clone().into();
                }
                change["before"] = "".into();
                change["after"] = after.clone().into();
                change
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Staged,
    Applied,
    Rejected,
    Undone,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Applied => "applied",
            Self::Rejected => "rejected",
            Self::Undone => "undone",
        }
    }
}

/// What applying did, so undo can tell whether the page moved on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    pub page: String,
    pub title: String,
    /// Whether applying created the page, which undo cannot remove.
    pub created: bool,
    before: String,
    after: String,
    /// The undo step applying made in an open page's buffer.
    step: Option<Step>,
}

/// One transaction in an open page's buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Step {
    buffer: WeakEntity<language::Buffer>,
    transaction: text::TransactionId,
}

#[derive(Clone, Debug)]
pub struct Change {
    pub id: String,
    pub run: String,
    /// The notebook it was proposed for.
    pub notebook: String,
    pub status: Status,
    pub request: Request,
    pub applied: Option<Applied>,
    busy: bool,
}

#[derive(Default)]
pub struct AiChanges {
    changes: Vec<Change>,
    next: usize,
}

pub struct Ledger(pub Entity<AiChanges>);

impl Global for Ledger {}

pub fn init(cx: &mut App) {
    if !cx.has_global::<Ledger>() {
        let ledger = cx.new(|_| AiChanges::default());
        cx.set_global(Ledger(ledger));
    }
}

pub fn ledger(cx: &App) -> Entity<AiChanges> {
    cx.global::<Ledger>().0.clone()
}

impl AiChanges {
    pub fn stage(
        &mut self,
        run: &str,
        notebook: &str,
        request: Request,
        cx: &mut Context<Self>,
    ) -> String {
        self.next += 1;
        let id = format!("change-{}", self.next);
        self.changes.push(Change {
            id: id.clone(),
            run: run.to_owned(),
            notebook: notebook.to_owned(),
            status: Status::Staged,
            request,
            applied: None,
            busy: false,
        });
        cx.notify();
        id
    }

    /// The changes for `notebook`, oldest first, optionally of one run.
    pub fn list(&self, notebook: &str, run: Option<&str>) -> Vec<Change> {
        self.changes
            .iter()
            .filter(|change| change.notebook == notebook && run.is_none_or(|run| change.run == run))
            .cloned()
            .collect()
    }

    fn find(&mut self, id: &str, notebook: &str) -> Result<&mut Change, String> {
        self.changes
            .iter_mut()
            .find(|change| change.id == id && change.notebook == notebook)
            .ok_or_else(|| GONE.to_owned())
    }

    pub fn reject(&mut self, id: &str, notebook: &str, cx: &mut Context<Self>) {
        if let Ok(change) = self.find(id, notebook)
            && change.status == Status::Staged
            && !change.busy
        {
            change.status = Status::Rejected;
            cx.notify();
        }
    }
}

fn current(cx: &App) -> Result<LiveNotebook, String> {
    live::current(cx).ok_or_else(|| GONE.to_owned())
}

/// The changes of the open notebook, optionally of one run.
pub fn list(run: Option<&str>, cx: &App) -> Vec<Change> {
    let Some(notebook) = live::current(cx) else {
        return Vec::new();
    };
    cx.try_global::<Ledger>()
        .map(|ledger| ledger.0.read(cx).list(&notebook.identity, run))
        .unwrap_or_default()
}

pub fn reject(id: &str, cx: &mut App) {
    if let Ok(notebook) = current(cx) {
        ledger(cx).update(cx, |ledger, cx| ledger.reject(id, &notebook.identity, cx));
    }
}

/// Applies one staged change as one edit; fails when its target moved on.
pub fn apply(id: &str, cx: &mut App) -> Task<Result<(), String>> {
    let id = id.to_owned();
    let start = (|| {
        let notebook = current(cx)?;
        let request = ledger(cx).update(cx, |ledger, cx| {
            let change = ledger.find(&id, &notebook.identity)?;
            if change.status != Status::Staged || change.busy {
                return Err(format!("This change is already {}.", change.status.label()));
            }
            change.busy = true;
            cx.notify();
            Ok(change.request.clone())
        })?;
        Ok((notebook, request))
    })();
    let (notebook, request) = match start {
        Ok(started) => started,
        Err(error) => return Task::ready(Err(error)),
    };
    cx.spawn(async move |cx| {
        let applied = run(&request, &notebook, cx).await;
        cx.update(|cx| {
            ledger(cx).update(cx, |ledger, cx| {
                if let Ok(change) = ledger.find(&id, &notebook.identity) {
                    change.busy = false;
                    if let Ok(applied) = &applied {
                        change.status = Status::Applied;
                        change.applied = Some(applied.clone());
                    }
                }
                cx.notify();
            })
        });
        applied.map(|_| ())
    })
}

/// Applies every staged change of `run`, one after another, and makes the changes to each open
/// page one undo step, as accepting one proposal is; reports the ones that failed.
pub fn apply_all(run: &str, cx: &mut App) -> Task<Vec<(String, String)>> {
    let staged: Vec<String> = list(Some(run), cx)
        .into_iter()
        .filter(|change| change.status == Status::Staged)
        .map(|change| change.id)
        .collect();
    cx.spawn(async move |cx| {
        let mut failed = Vec::new();
        let mut steps: Vec<Step> = Vec::new();
        for id in staged {
            match cx.update(|cx| apply(&id, cx)).await {
                Ok(()) => steps.extend(cx.update(|cx| {
                    list(None, cx)
                        .into_iter()
                        .find(|change| change.id == id)
                        .and_then(|change| change.applied?.step)
                })),
                Err(error) => failed.push((id, error)),
            }
        }
        cx.update(|cx| group_steps(steps, cx));
        failed
    })
}

/// Folds each later step into the first one made in the same buffer.
fn group_steps(steps: Vec<Step>, cx: &mut App) {
    let mut firsts: Vec<&Step> = Vec::new();
    for step in &steps {
        match firsts.iter().find(|first| first.buffer == step.buffer) {
            Some(first) => {
                if let Some(buffer) = step.buffer.upgrade() {
                    buffer.update(cx, |buffer, _| {
                        buffer.merge_transactions(step.transaction, first.transaction)
                    });
                }
            }
            None => firsts.push(step),
        }
    }
}

/// Reverses an applied change while its page is exactly as applying left it.
pub fn undo(id: &str, cx: &mut App) -> Task<Result<(), String>> {
    let id = id.to_owned();
    let start = (|| {
        let notebook = current(cx)?;
        let applied = ledger(cx).update(cx, |ledger, cx| {
            let change = ledger.find(&id, &notebook.identity)?;
            let Some(applied) = change.applied.clone().filter(|_| change.status == Status::Applied && !change.busy) else {
                return Ok(None);
            };
            if applied.created {
                return Err(format!(
                    "This change created [[{}]], and Undo cannot remove pages. Delete the page from its page menu if you do not want it.",
                    applied.title
                ));
            }
            change.busy = true;
            cx.notify();
            Ok(Some(applied))
        })?;
        Ok((notebook, applied))
    })();
    let (notebook, applied) = match start {
        Ok((notebook, Some(applied))) => (notebook, applied),
        Ok((_, None)) => return Task::ready(Ok(())),
        Err(error) => return Task::ready(Err(error)),
    };
    cx.spawn(async move |cx| {
        let moved = format!(
            "[[{}]] changed after this AI change. Undo it in the page with Command/Ctrl-Z instead.",
            applied.title
        );
        let reverted = async {
            let observed = observe(&notebook, &applied.page, cx)
                .await?
                .filter(|observed| observed.text == applied.after)
                .ok_or_else(|| moved.clone())?;
            let edit = replace_all(&observed.text, &applied.before);
            commit(&notebook, observed, vec![edit], &moved, cx).await
        }
        .await;
        cx.update(|cx| {
            ledger(cx).update(cx, |ledger, cx| {
                if let Ok(change) = ledger.find(&id, &notebook.identity) {
                    change.busy = false;
                    if reverted.is_ok() {
                        change.status = Status::Undone;
                    }
                }
                cx.notify();
            })
        });
        reverted.map(|_| ())
    })
}

/// One edit turning `from` into `to`, touching only what differs.
fn replace_all(from: &str, to: &str) -> Edit {
    let prefix = from
        .char_indices()
        .zip(to.chars())
        .find(|((_, a), b)| a != b)
        .map_or(from.len().min(to.len()), |((at, _), _)| at);
    let prefix = (0..=prefix)
        .rev()
        .find(|at| from.is_char_boundary(*at) && to.is_char_boundary(*at))
        .unwrap_or(0);
    let suffix = from[prefix..]
        .chars()
        .rev()
        .zip(to[prefix..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum::<usize>();
    Edit {
        range: prefix..from.len() - suffix,
        replacement: to[prefix..to.len() - suffix].to_owned(),
    }
}

/// A page's current text as one change reads it: from an open page's buffer at a known
/// version, or from its stored head.
struct Observed {
    text: String,
    open: Option<(WeakEntity<PagePersistence>, language::BufferSnapshot)>,
    source: Option<(page_writes::PageSource, SourceText)>,
}

fn open_buffer(
    pages: &[Entity<OpenPage>],
    page: &str,
    cx: &App,
) -> Option<Entity<PagePersistence>> {
    pages
        .iter()
        .find(|open| open.read(cx).id() == page)
        .and_then(|open| open.read(cx).persistence().cloned())
}

async fn observe(
    notebook: &LiveNotebook,
    page: &str,
    cx: &mut AsyncApp,
) -> Result<Option<Observed>, String> {
    let open = cx.update(|cx| {
        open_buffer(&notebook.open_pages, page, cx).map(|persistence| {
            let snapshot = persistence.read(cx).buffer().read(cx).snapshot();
            (persistence.downgrade(), snapshot)
        })
    });
    if let Some((persistence, snapshot)) = open {
        return Ok(Some(Observed {
            text: snapshot.text(),
            open: Some((persistence, snapshot)),
            source: None,
        }));
    }
    let source =
        page_writes::read(page.to_owned(), &notebook.open_pages, &notebook.storage, cx).await?;
    Ok(source.map(|source| {
        let (original, text) = SourceText::prepare(source.stored.clone());
        Observed {
            text,
            open: None,
            source: Some((source, original)),
        }
    }))
}

/// The page's text after a change, and the undo step it made in an open page's buffer.
struct Committed {
    text: String,
    step: Option<Step>,
}

/// Applies `edits` to the observed text as one step, refusing with `stale` when the page
/// changed since it was observed.
async fn commit(
    notebook: &LiveNotebook,
    observed: Observed,
    mut edits: Vec<Edit>,
    stale: &str,
    cx: &mut AsyncApp,
) -> Result<Committed, String> {
    edits.sort_by_key(|edit| edit.range.start);
    if let Some((persistence, snapshot)) = observed.open {
        return cx.update(|cx| {
            let persistence = persistence.upgrade().ok_or_else(|| stale.to_owned())?;
            let buffer = persistence.read(cx).buffer().clone();
            if buffer.read(cx).version() != *snapshot.version() {
                return Err(stale.to_owned());
            }
            let (text, transaction) = buffer.update(cx, |buffer, cx| {
                buffer.finalize_last_transaction();
                buffer.start_transaction();
                buffer.edit(
                    edits.into_iter().map(|edit| (edit.range, edit.replacement)),
                    None,
                    cx,
                );
                let transaction = buffer.end_transaction(cx);
                buffer.finalize_last_transaction();
                (buffer.text(), transaction)
            });
            persistence.update(cx, |persistence, cx| persistence.save(cx));
            Ok(Committed {
                text,
                step: transaction.map(|transaction| Step {
                    buffer: buffer.downgrade(),
                    transaction,
                }),
            })
        });
    }
    let Some((source, original)) = observed.source else {
        return Err(stale.to_owned());
    };
    let stored = original.apply(edits).map_err(|error| {
        format!(
            "Could not change {}: {error}. Review the page and ask again.",
            source.path
        )
    })?;
    let text = SourceText::prepare(stored.to_string()).1;
    let written = cx.update(|cx| {
        page_writes::apply(
            vec![source.write(stored)],
            notebook.storage.clone(),
            STALE,
            cx,
        )
    });
    match written.await {
        Ok(_) => Ok(Committed { text, step: None }),
        Err(error) if error == STALE => Err(stale.to_owned()),
        Err(error) => Err(error),
    }
}

fn utf16_byte(text: &str, units: u32) -> usize {
    super::read::byte_at(text, units)
}

async fn analysis(text: &str, cx: &mut AsyncApp) -> Result<quire_store::index::PageIndex, String> {
    let work = cx.update(|cx| crate::page_index::analyze(text.to_owned(), cx));
    cx.background_spawn(work)
        .await
        .ok_or_else(|| NOT_READY.to_owned())
}

fn applied(page: String, title: &str, before: String, committed: Committed) -> Applied {
    Applied {
        page,
        title: title.to_owned(),
        created: false,
        before,
        after: committed.text,
        step: committed.step,
    }
}

/// Creates a page with `text` at `folder`, as New page does.
async fn create(
    notebook: &LiveNotebook,
    title: &str,
    folder: &str,
    text: &str,
) -> Result<Applied, String> {
    let page = crate::navigation::create_page_with(
        notebook.storage.clone(),
        notebook.index.clone(),
        title.to_owned(),
        folder.to_owned(),
        text.to_owned(),
    )
    .await?;
    Ok(Applied {
        page,
        title: title.to_owned(),
        created: true,
        before: String::new(),
        after: text.to_owned(),
        step: None,
    })
}

async fn append(
    notebook: &LiveNotebook,
    page: &str,
    title: &str,
    block: &str,
    cx: &mut AsyncApp,
) -> Result<Applied, String> {
    let observed = observe(notebook, page, cx).await?.ok_or_else(|| {
        format!("[[{title}]] no longer exists. Ask the AI again to choose a page.")
    })?;
    let end = observed.text.len();
    let gap = if observed.text.is_empty() || observed.text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let before = observed.text.clone();
    let edit = Edit {
        range: end..end,
        replacement: format!("{gap}{block}\n"),
    };
    let committed = commit(notebook, observed, vec![edit], STALE, cx).await?;
    Ok(applied(page.to_owned(), title, before, committed))
}

fn daily(notebook: &LiveNotebook) -> Result<(String, String, String), String> {
    match notebook.daily.clone()? {
        CaptureTo::Today {
            date,
            title,
            folder,
        } => Ok((date, title, folder)),
        CaptureTo::Inbox => Err("Today's daily page has no layout. Fix the daily layout in Daily notes, then ask again.".into()),
    }
}

async fn run(
    request: &Request,
    notebook: &LiveNotebook,
    cx: &mut AsyncApp,
) -> Result<Applied, String> {
    match request {
        Request::Append {
            target: Some(target),
            after,
            ..
        } => append(notebook, &target.page, &target.title, after, cx).await,
        Request::Append {
            target: None,
            title,
            after,
        } => {
            let (date, layout_title, folder) = daily(notebook)?;
            let found = ask(notebook
                .index
                .with_generation(move |index| index.daily_page(&date)))
            .await?
            .1;
            match found {
                Some(page) => {
                    append(notebook, &page.page.to_lossy_string(), title, after, cx).await
                }
                None => create(notebook, &layout_title, &folder, &format!("{after}\n")).await,
            }
        }
        Request::Create { title, after } => {
            let wanted = title.clone();
            if ask(notebook.index.find_page(wanted)).await?.is_some() {
                return Err(format!(
                    "A page named \"{title}\" now exists. Ask the AI again to add to it instead."
                ));
            }
            create(notebook, title, "", after).await
        }
        Request::Replace {
            target,
            id,
            key,
            before,
            after,
        } => {
            let page = match id {
                Some(id) => ask(notebook.index.resolve_block(id.clone()))
                    .await?
                    .map(|page| page.page.to_lossy_string())
                    .ok_or_else(|| STALE.to_owned())?,
                None => target.page.clone(),
            };
            let observed = observe(notebook, &page, cx)
                .await?
                .ok_or_else(|| STALE.to_owned())?;
            let index = analysis(&observed.text, cx).await?;
            let block = index
                .blocks
                .iter()
                .find(|block| match (id, key) {
                    (Some(id), _) => block.id.as_deref() == Some(id.as_str()),
                    (None, Some(key)) => {
                        let line = quire_core::ai::tools::line_at(
                            &observed.text,
                            block.content_from as usize,
                        );
                        quire_core::ai::tools::block_key(line, &block.text) == *key
                    }
                    (None, None) => false,
                })
                .filter(|block| block.text == *before)
                .ok_or_else(|| STALE.to_owned())?;
            let from = utf16_byte(&observed.text, block.content_from);
            let edit = Edit {
                range: from..from + before.len(),
                replacement: after.clone(),
            };
            let previous = observed.text.clone();
            let committed = commit(notebook, observed, vec![edit], STALE, cx).await?;
            Ok(applied(page, &target.title, previous, committed))
        }
        Request::TaskState {
            target,
            id,
            key,
            raw,
            state,
            ..
        } => {
            let observed = observe(notebook, &target.page, cx)
                .await?
                .ok_or_else(|| STALE.to_owned())?;
            let today = notebook.today.clone()?;
            let work = cx.update(|cx| crate::tasks::Analysis::detached(&observed.text, cx));
            let found = cx
                .background_spawn(work)
                .await
                .ok_or_else(|| NOT_READY.to_owned())?;
            let edits = cx.update(|cx| {
                let days = cx
                    .global::<crate::tasks::TaskContext>()
                    .days()
                    .ok_or_else(|| crate::tasks::NO_TIME_ZONE.to_owned())?;
                found.with_page("", |page| {
                    let current = page.tasks(&days);
                    find_current(&current, id.as_deref(), key, raw)
                        .map(|task| task.changes(Action::Set(*state), &today))
                        .map_err(|_| STALE.to_owned())
                })
            })?;
            let previous = observed.text.clone();
            let committed = commit(notebook, observed, edits, STALE, cx).await?;
            Ok(applied(
                target.page.clone(),
                &target.title,
                previous,
                committed,
            ))
        }
        Request::Capture { text, project, .. } => {
            capture(notebook, text, project.as_ref(), cx).await
        }
    }
}

/// The reference `captureTask`, as Tasks capture does it: one task on the Inbox page or
/// today's daily page, created when missing, linked to its project, with a ➕ date off today's
/// page.
async fn capture(
    notebook: &LiveNotebook,
    value: &str,
    project: Option<&Target>,
    cx: &mut AsyncApp,
) -> Result<Applied, String> {
    let to = notebook.capture.clone()?;
    let (found, target, folder) = match &to {
        CaptureTo::Inbox => (
            ask(notebook.index.find_page(crate::task_edit::INBOX.into())).await?,
            crate::task_edit::INBOX.to_owned(),
            String::new(),
        ),
        CaptureTo::Today {
            date,
            title,
            folder,
        } => {
            let date = date.clone();
            let found = ask(notebook
                .index
                .with_generation(move |index| index.daily_page(&date)))
            .await?
            .1;
            (found, title.clone(), folder.clone())
        }
    };
    let link = match project {
        Some(project) => {
            let named = ask(notebook.index.find_page(project.title.clone())).await?;
            Some(
                if named.is_some_and(|named| named.page.to_lossy_string() == project.page) {
                    project.title.clone()
                } else {
                    page_name(&project.path).to_owned()
                },
            )
        }
        None => None,
    };
    let work = cx.update(|cx| crate::tasks::Analysis::detached(value, cx));
    let tags = cx
        .background_spawn(work)
        .await
        .ok_or_else(|| NOT_READY.to_owned())?
        .tags;
    let today = notebook.today.clone()?;
    let title = found
        .as_ref()
        .map_or(target.clone(), |found| found.title.clone());
    let stamp = match to {
        CaptureTo::Today { .. } => String::new(),
        CaptureTo::Inbox => cx.update(|cx| {
            cx.global::<crate::tasks::TaskContext>()
                .days()
                .map(|days| created_stamp(&title, &today, &days))
                .ok_or_else(|| crate::tasks::NO_TIME_ZONE.to_owned())
        })?,
    };
    let Some(found) = found else {
        let text = capture_edit("", value, &tags, link.as_deref(), &stamp)?.replacement;
        return create(notebook, &target, &folder, &text).await;
    };
    let page = found.page.to_lossy_string();
    let observed = observe(notebook, &page, cx)
        .await?
        .ok_or_else(|| "The capture page changed while capturing. Ask again.".to_owned())?;
    let edit = capture_edit(&observed.text, value, &tags, link.as_deref(), &stamp)?;
    let previous = observed.text.clone();
    let committed = commit(notebook, observed, vec![edit], STALE, cx).await?;
    Ok(applied(page, &title, previous, committed))
}

#[cfg(test)]
#[path = "changes_tests.rs"]
mod tests;
