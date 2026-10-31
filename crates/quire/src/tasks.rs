//! Task commands in page editors: Cmd-Enter toggles done and Cmd-Shift-Enter cycles Inbox, Next,
//! Waiting, Someday (Ctrl on other platforms), at the caret or across a selection, and the
//! Write view's checkbox toggles through the same command. Each command is one undoable source
//! transaction through the Zed editor, so Write, Source, and every view of the page's shared
//! buffer see it. Semantics live in `quire_core::tasks`; this module supplies the native
//! Markdown analysis and the clock.

use crate::markdown_blocks::Block;
use editor::{Editor, SelectionEffects};
use gpui::{App, Context, Entity, Global, KeyBinding, Window, actions};
use language::{Buffer, BufferSnapshot};
use multi_buffer::{MultiBufferOffset, ToOffset};
use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_core::tasks::{DailyNames, Page, PageBlock, Shortcut, Task, TaskEdit};
use std::{ops::Range, rc::Rc};

actions!(
    quire_tasks,
    [
        /// Completes the task at the caret, reopens a done one, or makes the line a task.
        ToggleTask,
        /// Moves the task at the caret to its next state: Inbox, Next, Waiting, Someday.
        CycleTask
    ]
);

const CONTEXT: &str = "Editor && !QuireControl";

/// What task commands read besides the page: the clock, the time zone, the daily layout that
/// names daily pages, and each buffer's page title.
/// Reads the page title of a buffer, when known.
pub type PageTitle = Rc<dyn Fn(&Entity<Buffer>, &App) -> Option<String>>;

pub struct TaskContext {
    pub now: Rc<dyn Fn() -> i64>,
    pub calendar: Option<DailyCalendar>,
    pub layout: DailyLayout,
    /// The page title of a buffer, when known. A task made on today's daily page needs no
    /// created stamp.
    pub title: PageTitle,
}
impl Global for TaskContext {}

pub(crate) const NO_TIME_ZONE: &str =
    "Tasks need the system time zone to date them. Set a valid time zone and reopen the notebook.";

impl TaskContext {
    pub(crate) fn today(&self) -> Option<String> {
        self.calendar.as_ref()?.date_at((self.now)()).ok()
    }

    pub(crate) fn days(&self) -> Option<DailyNames> {
        Some(DailyNames::new(self.layout.clone(), self.calendar.clone()?))
    }
}

pub fn init(cx: &mut App) {
    if !cx.has_global::<TaskContext>() {
        cx.set_global(TaskContext {
            now: Rc::new(crate::navigation::now_millis),
            calendar: DailyCalendar::system().ok(),
            layout: DailyLayout::default(),
            title: Rc::new(|_, _| None),
        });
    }
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        for shortcut in [Shortcut::Toggle, Shortcut::Cycle] {
            let handle = handle.clone();
            let run = move |window: &mut Window, cx: &mut App| {
                let handled = handle.upgrade().is_some_and(|editor| {
                    editor.update(cx, |editor, cx| run(editor, shortcut, None, window, cx))
                });
                if !handled {
                    cx.propagate();
                }
            };
            match shortcut {
                Shortcut::Toggle => editor
                    .register_action(move |_: &ToggleTask, window, cx| run(window, cx))
                    .detach(),
                Shortcut::Cycle => editor
                    .register_action(move |_: &CycleTask, window, cx| run(window, cx))
                    .detach(),
            }
        }
    })
    .detach();
    let platform = |mac: &'static str, other: &'static str| {
        if cfg!(target_os = "macos") {
            mac
        } else {
            other
        }
    };
    cx.bind_keys([
        KeyBinding::new(
            platform("cmd-enter", "ctrl-enter"),
            ToggleTask,
            Some(CONTEXT),
        ),
        KeyBinding::new(
            platform("cmd-shift-enter", "ctrl-shift-enter"),
            CycleTask,
            Some(CONTEXT),
        ),
    ]);
}

/// The page's native analysis, as task semantics read it.
pub(crate) struct Analysis {
    text: String,
    blocks: Vec<Block>,
    references: Vec<quire_core::markdown::Reference>,
    pub(crate) tags: Vec<quire_core::markdown::TagToken>,
    syntax: Vec<Range<usize>>,
    code: Vec<Range<usize>>,
}

impl Analysis {
    /// Analyzes text outside any buffer. Parsing happens in the returned future, which belongs
    /// on the background executor.
    pub(crate) fn detached(
        text: &str,
        cx: &mut App,
    ) -> impl std::future::Future<Output = Option<Self>> + Send + use<> {
        let snapshot = crate::markdown::detached_snapshot(text, cx);
        async move { Self::read(&snapshot.await) }
    }

    pub(crate) fn read(snapshot: &BufferSnapshot) -> Option<Self> {
        let text = snapshot.text();
        let blocks = crate::markdown_blocks::current_blocks(snapshot)?;
        let inline = crate::page_analysis::analyze_inline(snapshot)?;
        let syntax = crate::page_analysis::task_syntax(snapshot, &text)?;
        Some(Self {
            references: inline.references.to_vec(),
            tags: inline.tags.to_vec(),
            text,
            blocks,
            syntax: syntax.tags,
            code: syntax.code_blocks,
        })
    }

    pub(crate) fn with_page<R>(&self, title: &str, run: impl FnOnce(&Page) -> R) -> R {
        let blocks: Vec<PageBlock> = self
            .blocks
            .iter()
            .map(|block| PageBlock {
                key: &block.key,
                id: block.id.as_deref(),
                kind: block.kind,
                from: block.from,
                content: (block.content_from, block.content_to),
                parent: block.parent.as_deref(),
            })
            .collect();
        run(&Page {
            text: &self.text,
            title,
            blocks: &blocks,
            tags: &self.tags,
            references: &self.references,
            syntax_ranges: &self.syntax,
            code_ranges: &self.code,
        })
    }
}

const NOT_READY: &str = "This page is still being read. Try the task command again in a moment.";
const OUTSIDE: &str = "This task reaches outside the embedded block. Open its page to change it.";

/// Runs a task command on the Markdown page under the newest selection, which may be a whole
/// page or an embedded excerpt of one. `at` (a view offset) replaces the selection, for the
/// checkbox. Returns false when no Markdown page is there, so the key propagates.
fn run(
    editor: &mut Editor,
    shortcut: Shortcut,
    at: Option<usize>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> bool {
    let multi = editor.buffer().read(cx).snapshot(cx);
    let newest = editor.selections.newest_anchor();
    let (tail, head) = match at {
        Some(at) => (at, at),
        None => (
            newest.tail().to_offset(&multi).0,
            newest.head().to_offset(&multi).0,
        ),
    };
    let source = |offset: usize| {
        editor
            .buffer()
            .read(cx)
            .point_to_buffer_offset(MultiBufferOffset(offset), cx)
    };
    let Some((buffer, head)) = source(head) else {
        return false;
    };
    let head = head.0;
    let tail = source(tail)
        .filter(|(other, _)| *other == buffer)
        .map_or(head, |(_, offset)| offset.0);
    let snapshot = buffer.read(cx).snapshot();
    if snapshot
        .language()
        .is_none_or(|language| language.name().as_ref() != "Markdown")
    {
        return false;
    }
    let context = cx.global::<TaskContext>();
    let (Some(today), Some(days)) = (context.today(), context.days()) else {
        crate::outline::set_notice(Some(NO_TIME_ZONE), cx);
        return true;
    };
    let title = (context.title)(&buffer, cx).unwrap_or_default();
    let Some(analysis) = Analysis::read(&snapshot) else {
        crate::outline::set_notice(Some(NOT_READY), cx);
        return true;
    };
    let planned = analysis.with_page(&title, |page| {
        let tasks: Vec<Task> = page.tasks(&days);
        if tail == head {
            quire_core::tasks::task_at(page, &tasks, &days, &today, head, (tail, head), shortcut)
        } else {
            quire_core::tasks::tasks_in(
                page,
                &tasks,
                &days,
                &today,
                tail.min(head)..tail.max(head),
                shortcut,
            )
            .map(|edits| TaskEdit {
                edits,
                selection: None,
            })
        }
    });
    match planned {
        Ok(plan) => apply(editor, &buffer, plan, at.is_none(), window, cx),
        Err(message) => crate::outline::set_notice(Some(message), cx),
    }
    true
}

/// A source offset as an offset in the editor's view of it, if the view shows it.
fn view_offset(editor: &Editor, buffer: &Entity<Buffer>, offset: usize, cx: &App) -> Option<usize> {
    let multi = editor.buffer().read(cx).snapshot(cx);
    let anchor = buffer.read(cx).snapshot().anchor_before(offset);
    Some(multi.anchor_in_excerpt(anchor)?.to_offset(&multi).0)
}

fn apply(
    editor: &mut Editor,
    buffer: &Entity<Buffer>,
    plan: TaskEdit,
    place_caret: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let edits: Option<Vec<_>> = plan
        .edits
        .into_iter()
        .filter(|edit| !(edit.range.is_empty() && edit.replacement.is_empty()))
        .map(|edit| {
            let start = view_offset(editor, buffer, edit.range.start, cx)?;
            let end = view_offset(editor, buffer, edit.range.end, cx)?;
            Some((start..end, edit.replacement))
        })
        .collect();
    let Some(edits) = edits else {
        crate::outline::set_notice(Some(OUTSIDE), cx);
        return;
    };
    crate::outline::set_notice(None, cx);
    let view = editor.buffer().clone();
    editor.finalize_last_transaction(cx);
    editor.transact(window, cx, |editor, window, cx| {
        crate::outline::structural_edit(&view, &edits, cx);
        let caret = plan
            .selection
            .filter(|_| place_caret)
            .and_then(|(tail, head)| {
                Some((
                    view_offset(editor, buffer, tail, cx)?,
                    view_offset(editor, buffer, head, cx)?,
                ))
            });
        if let Some((tail, head)) = caret {
            editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                selections.select_ranges([MultiBufferOffset(tail)..MultiBufferOffset(head)])
            });
        }
    });
    editor.finalize_last_transaction(cx);
}

/// Changes a listed task on the page this editor shows, as one undo step in this editor.
/// Returns false when the editor does not show the task's line, so the caller writes the page
/// another way.
pub(crate) fn change_listed(
    editor: &mut Editor,
    task: &crate::task_edit::TaskRef,
    action: quire_core::tasks::Action,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Result<bool, String> {
    let Some(buffer) = editor.buffer().read(cx).all_buffers_iter().next() else {
        return Ok(false);
    };
    let snapshot = buffer.read(cx).snapshot();
    let context = cx.global::<TaskContext>();
    let (Some(today), Some(days)) = (context.today(), context.days()) else {
        return Err(NO_TIME_ZONE.into());
    };
    let analysis = Analysis::read(&snapshot).ok_or_else(|| NOT_READY.to_owned())?;
    let edits = analysis.with_page("", |page| {
        let tasks = page.tasks(&days);
        quire_core::tasks::find_current(&tasks, task.id.as_deref(), &task.key, &task.raw)
            .map(|found| found.changes(action, &today))
            .map_err(str::to_owned)
    })?;
    if edits.iter().any(|edit| {
        [edit.range.start, edit.range.end]
            .iter()
            .any(|offset| view_offset(editor, &buffer, *offset, cx).is_none())
    }) {
        return Ok(false);
    }
    apply(
        editor,
        &buffer,
        TaskEdit {
            edits,
            selection: None,
        },
        false,
        window,
        cx,
    );
    Ok(true)
}

/// The checkbox click: toggles the task whose marker starts at view offset `marker_start`.
pub fn toggle_at(
    editor: &mut Editor,
    marker_start: usize,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    run(editor, Shortcut::Toggle, Some(marker_start), window, cx);
}

#[cfg(test)]
mod tests;
