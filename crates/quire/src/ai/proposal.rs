//! The AI proposal (reference `src/editor/ai-proposal.ts`): `:ai`, the prompt, and the page menu
//! resolve a range, start a transform, and show its answer in a block under the range. The
//! proposal stays in that block, never in the page, until Accept applies it as one undo step;
//! Reject or Escape drops it and ends the run. Finished proposals show as Zed diff hunks over the
//! text they would replace, as page history does.

use super::{
    process::{self, Host, Outcome, Request, Transcript, Update},
    settings::{self, AiSettings},
};
use ::settings::Settings as _;
use buffer_diff::BufferDiff;
use editor::{
    Addon, Editor, EditorMode, SelectionEffects, SizingBehavior,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId, RenderBlock},
};
use futures::{StreamExt as _, channel::oneshot};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, KeyContext, SharedString, Task, WeakEntity,
    Window, prelude::*,
};
use language::{Buffer, BufferSnapshot, Capability};
use multi_buffer::{MultiBuffer, ToOffset as _};
use quire_core::ai::{
    AiRange, Backend, Command, Placement, Verb,
    backend::{AI_OFF, CANCELED, PROMPT_LIMIT, PROMPT_TOO_LONG},
    commands::{
        build_draft_prompt, build_prompt, clean_response, context_summary, drop_foreign_markers,
        is_blank_line, markers, parse_command, place_below, place_here, restore_markers,
    },
};
use std::{
    collections::HashSet,
    ops::Range,
    sync::Arc,
    time::{Duration, Instant},
};
use text::{Anchor, ToOffset as _};
use ui::{
    Button, ButtonCommon as _, ButtonStyle, Clickable as _, Color, Label, LabelCommon as _,
    LabelSize, h_flex, v_flex,
};

pub const CONTEXT: &str = "QuireProposal";
pub const STALE: &str =
    "The text changed while the proposal was written. Reject it and run :ai again.";
pub const NO_RANGE: &str =
    "Place the cursor on a block or a blank line, or select text, then run :ai again.";
pub const READ_ONLY: &str = "This page is read-only. Reopen it to edit, then run :ai again.";
pub const NO_PANE: &str = "Questions about the notebook open the answer pane, which Quire Native does not have yet. Select text or place the cursor on a block and give an instruction, such as :ai tighten.";
/// How much text above a blank line goes with a draft request.
const DRAFT_CONTEXT: usize = 6000;

/// What a transform was asked to do, kept for Retry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invocation {
    /// `:ai` arguments: a verb and extra words, or a free prompt.
    pub args: String,
    /// Explicit 0-based buffer rows, inclusive, from an Ex range or a prompt opened on a selection.
    pub rows: Option<Range<u32>>,
    pub verb: Option<Verb>,
}

/// The text a proposal would change, as anchors that follow edits made meanwhile.
#[derive(Clone)]
struct Target {
    buffer: Entity<Buffer>,
    range: Range<Anchor>,
    original: String,
    placement: Placement,
}

enum Phase {
    Writing {
        text: String,
        activity: String,
    },
    Checking {
        text: String,
    },
    Ready {
        text: String,
        taken: HashSet<String>,
        diff: Entity<Editor>,
    },
    Failed {
        message: String,
        retry: bool,
    },
}

/// The block under the range: the backend and elapsed time, the context sent, the streamed
/// text, and then the change as diff hunks with Accept and Reject.
pub struct ProposalView {
    host: WeakEntity<Editor>,
    target: Target,
    invocation: Invocation,
    backend: Backend,
    context: Vec<String>,
    started: Instant,
    finished: Option<Duration>,
    phase: Phase,
    focus: FocusHandle,
    _cancel: Option<oneshot::Sender<()>>,
    _run: Option<Task<()>>,
    _ticker: Option<Task<()>>,
}

/// The proposal an editor shows, and its block.
struct Proposals {
    view: Entity<ProposalView>,
    block: CustomBlockId,
}

impl Addon for Proposals {
    fn extend_key_context(&self, context: &mut KeyContext, _: &App) {
        context.add(CONTEXT);
    }

    fn to_any(&self) -> &dyn std::any::Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The proposal shown in `editor`, for tests and the page menu.
#[cfg(test)]
pub fn current(editor: &Editor) -> Option<Entity<ProposalView>> {
    editor
        .addon::<Proposals>()
        .map(|proposals| proposals.view.clone())
}

/// The buffer the caret is in, its offset there, and the newest selection in that buffer.
fn caret(editor: &Editor, cx: &App) -> Option<(Entity<Buffer>, usize, Range<usize>)> {
    let multi = editor.buffer().read(cx);
    let snapshot = multi.snapshot(cx);
    let selection = editor.selections.newest_anchor();
    let head = selection.head().to_offset(&snapshot);
    let (buffer, at) = snapshot.point_to_buffer_offset(head)?;
    let entity = multi.buffer(buffer.remote_id())?;
    let (start, end) = (
        selection.start.to_offset(&snapshot),
        selection.end.to_offset(&snapshot),
    );
    let selected = snapshot
        .range_to_buffer_ranges(start.min(end)..start.max(end))
        .into_iter()
        .find(|(found, _, _)| found.remote_id() == buffer.remote_id())
        .map(|(_, range, _)| range.start.0..range.end.0)
        .unwrap_or(at.0..at.0);
    Some((entity, at.0, selected))
}

fn offsets(range: &Range<Anchor>, snapshot: &text::BufferSnapshot) -> Range<usize> {
    range.start.to_offset(snapshot)..range.end.to_offset(snapshot)
}

fn line_start(text: &str, at: usize) -> usize {
    text[..at].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(text: &str, at: usize) -> usize {
    text[at..].find('\n').map_or(text.len(), |index| at + index)
}

/// The byte offset where 0-based `row` starts, clamped to the text.
fn row_start(text: &str, row: u32) -> usize {
    text.split_inclusive('\n')
        .take(row as usize)
        .map(str::len)
        .sum::<usize>()
        .min(text.len())
}

/// The range a command applies to (reference `resolveRange`): the explicit rows or selection,
/// or else the block under the caret with its children, widened to whole lines and blocks.
pub fn resolve_range(
    snapshot: &BufferSnapshot,
    head: usize,
    selection: Range<usize>,
    rows: Option<Range<u32>>,
) -> Option<Range<usize>> {
    let text = snapshot.text();
    let blocks = crate::markdown_blocks::current_blocks(snapshot).unwrap_or_default();
    let (from, mut to) = if let Some(rows) = rows {
        let first = rows.start.min(rows.end);
        let last = rows.start.max(rows.end);
        let from = row_start(&text, first);
        (from, line_end(&text, row_start(&text, last)))
    } else if !selection.is_empty() {
        (
            line_start(&text, selection.start),
            line_end(&text, selection.end),
        )
    } else {
        let block = blocks
            .iter()
            .rev()
            .find(|block| head >= line_start(&text, block.from) && head <= block.content_to)?;
        (
            line_start(&text, block.from),
            line_end(&text, block.from.max(block.subtree_to.saturating_sub(1))),
        )
    };
    for block in &blocks {
        if block.from >= from && block.from <= to && block.content_to > to {
            to = line_end(&text, block.content_to);
        }
    }
    (!text[from..to].trim().is_empty()).then_some(from..to)
}

/// The page title, the parent blocks' text, and the block IDs inside `range`.
fn range_context(
    editor: &Editor,
    buffer: &Entity<Buffer>,
    range: &Range<usize>,
    cx: &App,
) -> AiRange {
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let path = crate::references::host(editor)
        .and_then(|host| host.read(cx).notebook().page_path(buffer, cx))
        .unwrap_or_default();
    let title = quire_core::markdown::page_title(&path, &text).to_lossy_string();
    let blocks = crate::markdown_blocks::current_blocks(&snapshot).unwrap_or_default();
    let inside: Vec<_> = blocks
        .iter()
        .filter(|block| block.from >= range.start && block.from <= range.end)
        .collect();
    let mut ancestors = Vec::new();
    let mut parent = inside.first().and_then(|block| block.parent.clone());
    while let Some(key) = parent {
        let Some(block) = blocks.iter().find(|block| block.key == key) else {
            break;
        };
        ancestors.insert(0, crate::markdown_blocks::block_text(&snapshot, block));
        parent = block.parent.clone();
    }
    AiRange {
        path,
        title: if title.is_empty() {
            "Untitled".into()
        } else {
            title
        },
        text: text[range.clone()].to_owned(),
        ancestors,
        block_ids: inside.iter().filter_map(|block| block.id.clone()).collect(),
    }
}

/// Where transcripts of this editor's runs go, with the page they belong to.
fn transcript(editor: &Editor, cx: &App) -> Option<std::path::PathBuf> {
    crate::references::host(editor).and_then(|host| host.read(cx).notebook().transcripts(cx))
}

/// Runs `:ai` (or the prompt, the page menu, or a verb) in `editor`. Problems that stop the
/// command before it starts are shown as a notice, as the reference reports them to `onError`.
pub fn start(editor: &Entity<Editor>, invocation: Invocation, window: &mut Window, cx: &mut App) {
    editor.update(cx, |editor, cx| start_in(editor, invocation, window, cx));
}

/// [`start`] from inside an update of `editor`.
pub fn start_in(
    editor: &mut Editor,
    invocation: Invocation,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if let Err(message) = begin(editor, invocation, window, cx) {
        crate::outline::show_notice(message, cx);
    }
}

/// What the prompt describes: the kind of text a command will use, its label, the rows of a
/// selection, and where the prompt goes.
pub struct PromptTarget {
    pub kind: super::prompt::TargetKind,
    pub label: String,
    pub rows: Option<Range<u32>>,
    pub at: multi_buffer::Anchor,
}

fn row_of(text: &str, offset: usize) -> u32 {
    text[..offset].matches('\n').count() as u32
}

fn lines_label(kind: &str, count: u32) -> String {
    format!("{kind} · {count} line{}", if count == 1 { "" } else { "s" })
}

/// The text a typed instruction would apply to (reference `promptTarget`). `rows` are the lines
/// of a Visual selection recorded before Visual mode ended.
pub fn prompt_target(editor: &Editor, rows: Option<Range<u32>>, cx: &App) -> Option<PromptTarget> {
    use super::prompt::TargetKind;
    let (buffer, head, selection) = caret(editor, cx)?;
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let line = line_start(&text, head)..line_end(&text, head);
    let blank = is_blank_line(&text[line.clone()])
        && selection.start >= line.start
        && selection.end <= line.end + 1;
    let selected = rows.or_else(|| {
        (!selection.is_empty() && !blank)
            .then(|| row_of(&text, selection.start)..row_of(&text, selection.end))
    });
    let (kind, label, rows, at) = if let Some(rows) = selected {
        let end = line_end(&text, row_start(&text, rows.end));
        let label = lines_label("Selection", rows.end - rows.start + 1);
        (TargetKind::Selection, label, Some(rows), end)
    } else if blank {
        let label = "Blank line · writes here, using the text above".to_owned();
        (TargetKind::Blank, label, None, line.end)
    } else {
        let range = resolve_range(&snapshot, head, selection, None)?;
        let count = row_of(&text, range.end) - row_of(&text, range.start) + 1;
        (
            TargetKind::Block,
            lines_label("Block", count),
            None,
            range.end,
        )
    };
    let at = editor
        .buffer()
        .read(cx)
        .snapshot(cx)
        .anchor_in_buffer(snapshot.anchor_after(at))?;
    Some(PromptTarget {
        kind,
        label,
        rows,
        at,
    })
}

fn begin(
    editor: &mut Editor,
    invocation: Invocation,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Result<(), String> {
    let (buffer, head, selection) = caret(editor, cx).ok_or(NO_RANGE)?;
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let explicit = invocation.rows.is_some() || !selection.is_empty();
    let page = range_context(editor, &buffer, &(head..head), cx).title;
    let command = match invocation.verb {
        Some(verb) => parse_command(
            &format!("{} {}", verb.key(), invocation.args),
            true,
            None,
            false,
        ),
        None => parse_command(&invocation.args, explicit, Some(&page), false),
    };
    let Command::Run {
        verb,
        prompt,
        placement,
    } = command
    else {
        return Err(NO_PANE.into());
    };
    if placement == Placement::Answer {
        return Err(NO_PANE.into());
    }
    let line = line_start(&text, head)..line_end(&text, head);
    let blank = invocation.rows.is_none()
        && is_blank_line(&text[line.clone()])
        && selection.start >= line.start
        && selection.end <= line.end + 1;
    if blank && verb.is_some_and(|verb| verb != Verb::Continue) {
        return Err(format!(
            ":ai {} needs text to work on. Put the cursor on a block or select lines, or on a blank line type what to write, such as :ai list three ideas.",
            verb.map(Verb::key).unwrap_or_default()
        ));
    }
    let range = if blank {
        line.clone()
    } else {
        resolve_range(&snapshot, head, selection, invocation.rows.clone()).ok_or(NO_RANGE)?
    };
    if editor.read_only(cx) || buffer.read(cx).read_only() {
        return Err(READ_ONLY.into());
    }
    if !settings::is_on(cx) {
        return Err(if settings::is_tested(cx) {
            AI_OFF.into()
        } else {
            settings::UNTESTED.into()
        });
    }
    let placement = if blank { Placement::Here } else { placement };
    let mut context = range_context(editor, &buffer, &range, cx);
    let (request_prompt, summary) = if blank {
        let before_start = line.start.saturating_sub(DRAFT_CONTEXT);
        let before_start = (before_start..line.start)
            .find(|index| text.is_char_boundary(*index))
            .unwrap_or(line.start);
        let before = &text[before_start..line.start];
        context.ancestors.clear();
        context.block_ids.clear();
        let lines = before
            .trim_end()
            .split('\n')
            .filter(|_| !before.trim().is_empty())
            .count();
        let summary = vec![
            format!("Page: {}", context.title),
            if lines == 0 {
                "No text above the cursor".into()
            } else {
                format!(
                    "Text above the cursor: {lines} line{}",
                    if lines == 1 { "" } else { "s" }
                )
            },
        ];
        (
            build_draft_prompt(
                &prompt,
                &context.title,
                before,
                verb == Some(Verb::Continue),
            ),
            summary,
        )
    } else {
        (
            build_prompt(verb, &prompt, placement, Some(&context)),
            context_summary(&context),
        )
    };
    if request_prompt.len() > PROMPT_LIMIT {
        return Err(PROMPT_TOO_LONG.into());
    }
    let settings = AiSettings::get_global(cx).clone();
    let target = Target {
        range: snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end),
        original: text[range.clone()].to_owned(),
        buffer: buffer.clone(),
        placement,
    };
    let request = Request {
        backend: settings.backend,
        binary: settings.binary(settings.backend).to_owned(),
        model: settings.model.clone(),
        prompt: request_prompt,
        timeout: process::TRANSFORM_TIMEOUT,
        transcript: transcript(editor, cx).map(|folder| Transcript {
            folder,
            id: format!("ai-{}", uuid::Uuid::new_v4().simple()),
            page: context.path.clone(),
        }),
    };
    dismiss(editor, cx);
    let host = cx.entity().downgrade();
    let view = cx.new(|cx| {
        ProposalView::new(
            Proposed {
                host,
                target: target.clone(),
                invocation,
                backend: settings.backend,
                context: summary,
            },
            request,
            window,
            cx,
        )
    });
    let multi = editor.buffer().read(cx).snapshot(cx);
    let anchor = multi
        .anchor_range_in_buffer(target.range.clone())
        .map(|range| range.end)
        .ok_or(NO_RANGE)?;
    let rows = view.read(cx).rows(cx);
    let block = editor.insert_blocks(
        [BlockProperties {
            placement: BlockPlacement::Below(anchor),
            height: Some(rows),
            style: BlockStyle::Flex,
            render: render(view.clone()),
            priority: 0,
        }],
        None,
        cx,
    )[0];
    cx.observe(&view, |editor, view, cx| resize(editor, &view, cx))
        .detach();
    editor.register_addon(Proposals { view, block });
    cx.notify();
    Ok(())
}

fn render(view: Entity<ProposalView>) -> RenderBlock {
    Arc::new(move |block| {
        gpui::div()
            .key_context("QuireControl")
            .ml(block.anchor_x)
            .w_full()
            .child(view.clone())
            .into_any_element()
    })
}

fn resize(editor: &mut Editor, view: &Entity<ProposalView>, cx: &mut Context<Editor>) {
    let Some(proposals) = editor.addon::<Proposals>() else {
        return;
    };
    if proposals.view != *view {
        return;
    }
    let block = proposals.block;
    let rows = view.read(cx).rows(cx);
    editor.resize_blocks([(block, rows)].into_iter().collect(), None, cx);
}

/// Removes `editor`'s proposal, ending its run.
pub fn dismiss(editor: &mut Editor, cx: &mut Context<Editor>) -> bool {
    let Some(proposals) = editor.addon::<Proposals>() else {
        return false;
    };
    let (block, view) = (proposals.block, proposals.view.clone());
    // Ends the run now; the block's renderer may keep the view alive until the next draw.
    view.update(cx, |view, _| view._cancel = None);
    editor.remove_blocks([block].into_iter().collect(), None, cx);
    editor.unregister_addon::<Proposals>();
    cx.notify();
    true
}

/// What a proposal shows besides the run: where it goes and what was sent.
struct Proposed {
    host: WeakEntity<Editor>,
    target: Target,
    invocation: Invocation,
    backend: Backend,
    context: Vec<String>,
}

impl ProposalView {
    fn new(
        proposed: Proposed,
        request: Request,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let Proposed {
            host,
            target,
            invocation,
            backend,
            context,
        } = proposed;
        let (cancel, canceled) = oneshot::channel::<()>();
        let (updates, mut received) = futures::channel::mpsc::unbounded();
        let runner = cx.global::<Host>().clone();
        let running = cx.background_spawn(async move {
            runner
                .run(
                    request,
                    async {
                        canceled.await.ok();
                    },
                    move |update| {
                        updates.unbounded_send(update).ok();
                    },
                )
                .await
        });
        let run = cx.spawn_in(window, async move |this, cx| {
            while let Some(update) = received.next().await {
                if this
                    .update(cx, |this, cx| this.progress(update, cx))
                    .is_err()
                {
                    return;
                }
            }
            let outcome = running.await;
            this.update_in(cx, |this, window, cx| this.finish(outcome, window, cx))
                .ok();
        });
        let ticker = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(100))
                    .await;
                let writing = this.update(cx, |this, cx| {
                    cx.notify();
                    this.finished.is_none()
                });
                if !matches!(writing, Ok(true)) {
                    return;
                }
            }
        });
        Self {
            host,
            target,
            invocation,
            backend,
            context,
            started: Instant::now(),
            finished: None,
            phase: Phase::Writing {
                text: String::new(),
                activity: "Writing".into(),
            },
            focus: cx.focus_handle(),
            _cancel: Some(cancel),
            _run: Some(run),
            _ticker: Some(ticker),
        }
    }

    fn progress(&mut self, update: Update, cx: &mut Context<Self>) {
        if let Phase::Writing { text, activity } = &mut self.phase {
            match update {
                Update::Token(token) => text.push_str(&token),
                Update::Tool { name, detail } => {
                    *activity = format!("{name} {detail}").trim().to_owned()
                }
            }
            cx.notify();
        }
    }

    fn finish(&mut self, outcome: Outcome, window: &mut Window, cx: &mut Context<Self>) {
        self.finished = Some(self.started.elapsed());
        self._cancel = None;
        match outcome {
            Outcome::Done(text) => self.check(clean_response(&text), window, cx),
            Outcome::Failed { message, retry } => self.phase = Phase::Failed { message, retry },
            Outcome::Canceled => {
                self.phase = Phase::Failed {
                    message: CANCELED.into(),
                    retry: true,
                }
            }
        }
        cx.notify();
    }

    /// Finds which block IDs the model wrote that already name a block elsewhere, in this page
    /// or anywhere the index knows, before offering the proposal.
    fn check(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        let own: HashSet<String> = markers(&self.target.original).into_iter().collect();
        let candidates: Vec<String> = markers(&text)
            .into_iter()
            .filter(|id| !own.contains(id))
            .collect();
        let snapshot = self.target.buffer.read(cx).snapshot();
        let range = offsets(&self.target.range, &snapshot);
        let full = snapshot.text();
        let outside = format!("{}\n{}", &full[..range.start], &full[range.end..]);
        let here: HashSet<String> = markers(&outside).into_iter().collect();
        let index = self.host.upgrade().and_then(|host| {
            crate::references::host(host.read(cx)).and_then(|host| host.read(cx).index().cloned())
        });
        self.phase = Phase::Checking { text: text.clone() };
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            let mut taken: HashSet<String> = candidates
                .iter()
                .filter(|id| here.contains(*id))
                .cloned()
                .collect();
            if let Some(index) = index {
                let unknown: Vec<String> = candidates
                    .iter()
                    .filter(|id| !taken.contains(*id))
                    .cloned()
                    .collect();
                for id in unknown {
                    if let Ok(Some(_)) =
                        crate::query_index::ask(index.resolve_block(id.clone())).await
                    {
                        taken.insert(id);
                    }
                }
            }
            this.update_in(cx, |this, window, cx| this.ready(text, taken, window, cx))
                .ok();
        }));
    }

    fn ready(
        &mut self,
        text: String,
        taken: HashSet<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((_, applied, base)) = self.edit(&text, &taken, cx) else {
            return;
        };
        let buffer = crate::markdown::buffer(applied, cx);
        buffer.update(cx, |buffer, cx| {
            buffer.set_capability(Capability::ReadOnly, cx)
        });
        let snapshot = buffer.read(cx).text_snapshot();
        let diff = cx.new(|cx| BufferDiff::new(&snapshot, None, None, cx));
        let update = diff.update(cx, |diff, cx| {
            diff.set_base_text(Some(base.into()), snapshot, cx)
        });
        self._run = Some(cx.spawn_in(window, async move |this, cx| {
            update.await;
            this.update_in(cx, |this, window, cx| {
                let multibuffer = cx.new(|cx| {
                    let mut multibuffer = MultiBuffer::singleton(buffer, cx);
                    multibuffer.add_diff(diff, cx);
                    multibuffer
                });
                let diff = cx.new(|cx| {
                    let mut editor = Editor::new(
                        EditorMode::Full {
                            scale_ui_elements_with_buffer_font_size: true,
                            show_active_line_background: false,
                            sizing_behavior: SizingBehavior::SizeByContent,
                        },
                        multibuffer,
                        None,
                        window,
                        cx,
                    );
                    editor.set_read_only(true);
                    editor.set_expand_all_diff_hunks(cx);
                    editor
                });
                this.phase = Phase::Ready { text, taken, diff };
                cx.notify();
            })
            .ok();
        }));
    }

    /// The edit Accept makes: its buffer range, the text it writes, and the text it replaces,
    /// with the markers lost because their lines changed.
    fn edit(
        &self,
        proposal: &str,
        taken: &HashSet<String>,
        cx: &App,
    ) -> Option<(Range<usize>, String, String)> {
        let snapshot = self.target.buffer.read(cx).snapshot();
        let range = offsets(&self.target.range, &snapshot);
        let original = &self.target.original;
        let own = if self.target.placement == Placement::Replace {
            original.as_str()
        } else {
            ""
        };
        let (proposed, _) = drop_foreign_markers(proposal, own, |id| taken.contains(id));
        Some(match self.target.placement {
            Placement::Insert => (
                range.end..range.end,
                format!("\n{}", place_below(original, &proposed)),
                String::new(),
            ),
            Placement::Here => {
                let mut insert = place_here(original, &proposed);
                if range.end == snapshot.len() && range.start > 0 && original.is_empty() {
                    insert.push('\n');
                }
                (range, insert, original.clone())
            }
            Placement::Replace | Placement::Answer => (
                range,
                restore_markers(original, &proposed).text,
                original.clone(),
            ),
        })
    }

    fn lost(&self, proposal: &str, taken: &HashSet<String>) -> Vec<String> {
        if self.target.placement != Placement::Replace {
            return Vec::new();
        }
        let (proposed, _) =
            drop_foreign_markers(proposal, &self.target.original, |id| taken.contains(id));
        restore_markers(&self.target.original, &proposed).lost
    }

    pub fn is_ready(&self) -> bool {
        matches!(self.phase, Phase::Ready { .. })
    }

    #[cfg(test)]
    pub fn is_writing(&self) -> bool {
        matches!(self.phase, Phase::Writing { .. } | Phase::Checking { .. })
    }

    /// The text shown in the block, for tests: the streamed or final proposal, or the failure.
    #[cfg(test)]
    pub fn shown_text(&self) -> String {
        match &self.phase {
            Phase::Writing { text, .. } | Phase::Checking { text } | Phase::Ready { text, .. } => {
                text.clone()
            }
            Phase::Failed { message, .. } => message.clone(),
        }
    }

    #[cfg(test)]
    pub fn context(&self) -> &[String] {
        &self.context
    }

    #[cfg(test)]
    pub fn diff_editor(&self) -> Option<&Entity<Editor>> {
        match &self.phase {
            Phase::Ready { diff, .. } => Some(diff),
            _ => None,
        }
    }

    /// Whether the caret of the host editor sits inside the proposal's range.
    fn caret_inside(&self, editor: &Editor, cx: &App) -> bool {
        let Some((buffer, head, _)) = caret(editor, cx) else {
            return false;
        };
        let snapshot = buffer.read(cx).snapshot();
        let range = offsets(&self.target.range, &snapshot);
        buffer == self.target.buffer && head >= range.start && head <= range.end
    }

    /// The rows the block needs in the host editor.
    fn rows(&self, cx: &App) -> u32 {
        let body = match &self.phase {
            Phase::Writing { text, .. } | Phase::Checking { text } => {
                text.split('\n').count().max(1) as u32
            }
            Phase::Ready { diff, .. } => {
                let multi = diff.read(cx).buffer().read(cx).snapshot(cx);
                multi.max_point().row + 1
            }
            Phase::Failed { .. } => 1,
        };
        body + 4
    }

    fn status(&self) -> String {
        let elapsed = self.finished.unwrap_or_else(|| self.started.elapsed());
        let label = match &self.phase {
            Phase::Writing { activity, .. } => activity.as_str(),
            Phase::Checking { .. } => "Checking block IDs",
            Phase::Ready { .. } => "Ready",
            Phase::Failed { .. } => "Stopped",
        };
        format!("{label} · {:.1} s", elapsed.as_secs_f64())
    }
}

impl Focusable for ProposalView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Accepts `editor`'s finished proposal: refuses when the text changed meanwhile, and otherwise
/// applies it as one undo step with the block IDs of unchanged lines kept.
pub fn accept(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> bool {
    let Some(view) = editor.read(cx).addon::<Proposals>().map(|p| p.view.clone()) else {
        return false;
    };
    let (text, taken) = match &view.read(cx).phase {
        Phase::Ready { text, taken, .. } => (text.clone(), taken.clone()),
        _ => return false,
    };
    let (target, edit, lost) = {
        let proposal = view.read(cx);
        (
            proposal.target.clone(),
            proposal.edit(&text, &taken, cx),
            proposal.lost(&text, &taken),
        )
    };
    let snapshot = target.buffer.read(cx).snapshot();
    let current: String = snapshot
        .text_for_range(offsets(&target.range, &snapshot))
        .collect();
    if current != target.original {
        crate::outline::show_notice(STALE, cx);
        return true;
    }
    let Some((range, insert, _)) = edit else {
        return false;
    };
    editor.update(cx, |editor, cx| {
        let multi = editor.buffer().clone();
        let Some(anchors) = multi.read(cx).snapshot(cx).anchor_range_in_buffer(
            snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end),
        ) else {
            return;
        };
        dismiss(editor, cx);
        editor.finalize_last_transaction(cx);
        editor.transact(window, cx, |editor, window, cx| {
            let _intent = multi
                .read(cx)
                .scoped_edit_intent(multi_buffer::ViewEditIntent::Structural);
            multi.update(cx, |multi, cx| {
                multi.edit([(anchors.clone(), insert.as_str())], None, cx)
            });
            let end = anchors.end.to_offset(&multi.read(cx).snapshot(cx));
            editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                selections.select_ranges([end..end])
            });
        });
        editor.finalize_last_transaction(cx);
        window.focus(&editor.focus_handle(cx), cx);
    });
    if !lost.is_empty() {
        let count = lost.len();
        let message = format!(
            "{count} block marker{} ({}) did not survive because their lines changed. References to {} now show as missing; undo to restore.",
            if count == 1 { "" } else { "s" },
            lost.iter()
                .map(|id| format!("^{id}"))
                .collect::<Vec<_>>()
                .join(", "),
            if count == 1 { "it" } else { "them" }
        );
        // After the edit's own events, which clear notices.
        cx.defer(move |cx| crate::outline::show_notice(message, cx));
    }
    true
}

/// Rejects `editor`'s proposal, or cancels the run still writing it, leaving no trace.
pub fn reject(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> bool {
    editor.update(cx, |editor, cx| {
        let removed = dismiss(editor, cx);
        if removed {
            window.focus(&editor.focus_handle(cx), cx);
        }
        removed
    })
}

fn retry(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    let Some(invocation) = editor
        .read(cx)
        .addon::<Proposals>()
        .map(|proposals| proposals.view.read(cx).invocation.clone())
    else {
        return;
    };
    reject(editor, window, cx);
    start(editor, invocation, window, cx);
}

/// Enter accepts while the caret is inside the range of a finished proposal; anywhere else it
/// keeps its usual meaning.
pub fn accept_key(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> bool {
    let inside = editor
        .read(cx)
        .addon::<Proposals>()
        .is_some_and(|proposals| {
            let view = proposals.view.read(cx);
            view.is_ready() && view.caret_inside(editor.read(cx), cx)
        });
    inside && accept(editor, window, cx)
}

impl Render for ProposalView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use theme::ActiveTheme as _;
        let colors = cx.theme().colors();
        let host = self.host.clone();
        let button = |id: &'static str, label: &'static str, primary: bool| {
            Button::new(id, label).style(if primary {
                ButtonStyle::Filled
            } else {
                ButtonStyle::Subtle
            })
        };
        let act = |run: fn(&Entity<Editor>, &mut Window, &mut App) -> bool| {
            let host = host.clone();
            move |_: &gpui::ClickEvent, window: &mut Window, cx: &mut App| {
                if let Some(host) = host.upgrade() {
                    run(&host, window, cx);
                }
            }
        };
        let accept_label = match self.target.placement {
            Placement::Insert => "Insert below",
            Placement::Here => "Insert",
            Placement::Replace | Placement::Answer => "Accept",
        };
        let actions = match &self.phase {
            Phase::Writing { .. } | Phase::Checking { .. } => {
                h_flex().child(button("ai-cancel", "Cancel", false).on_click(act(reject)))
            }
            Phase::Ready { .. } => h_flex()
                .gap_2()
                .child(button("ai-accept", accept_label, true).on_click(act(accept)))
                .child(button("ai-reject", "Reject", false).on_click(act(reject))),
            Phase::Failed {
                retry: can_retry, ..
            } => h_flex()
                .gap_2()
                .when(*can_retry, |row| {
                    let host = host.clone();
                    row.child(
                        button("ai-retry", "Retry", true).on_click(move |_, window, cx| {
                            if let Some(host) = host.upgrade() {
                                retry(&host, window, cx);
                            }
                        }),
                    )
                })
                .child(button("ai-close", "Close", false).on_click(act(reject))),
        };
        let body = match &self.phase {
            Phase::Writing { text, .. } | Phase::Checking { text } => gpui::div()
                .child(Label::new(SharedString::from(text.clone())).size(LabelSize::Small))
                .into_any_element(),
            Phase::Ready { diff, .. } => diff.clone().into_any_element(),
            Phase::Failed { message, .. } => Label::new(SharedString::from(message.clone()))
                .color(Color::Error)
                .into_any_element(),
        };
        v_flex()
            .id("ai-proposal")
            .debug_selector(|| "quire-ai-proposal".into())
            .track_focus(&self.focus)
            .w_full()
            .pl_2()
            .gap_1()
            .border_l_2()
            .border_color(colors.border_focused)
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(self.backend.name()).size(LabelSize::Small))
                    .child(
                        Label::new(self.status())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
            .child(
                Label::new(format!("Context sent: {}", self.context.join(" · ")))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(body)
            .child(actions)
    }
}

#[cfg(test)]
#[path = "proposal_tests.rs"]
pub(crate) mod tests;
