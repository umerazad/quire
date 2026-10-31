//! Outline editing in Write view: list continuation, indentation, block movement, merging,
//! multi-line paste, numbered lists, and folding of block subtrees. Each command is one
//! undoable source transaction applied through the Zed editor; commands that do not apply
//! propagate to the next key binding, so ordinary editing keeps its Zed behavior.

use crate::markdown_blocks::Block;
use editor::{Editor, SelectionEffects};
use gpui::{
    Action, App, Context, Entity, Focusable as _, Global, KeyBinding, SharedString, Window, actions,
};
use multi_buffer::{MultiBuffer, MultiBufferOffset, ToOffset, ViewEditIntent};
use std::ops::Range;

pub(crate) mod fold_store;
pub(crate) mod folds;
mod fresh_ids;
mod lists;
mod model;
pub mod zoom;

pub use lists::Style;
use model::{Edit, Outcome, Plan};

actions!(
    quire_outline,
    [
        /// Continues, splits, or ends the list item at the caret.
        SplitBlock,
        /// Nests the list item and its children under the previous sibling.
        IndentBlock,
        /// Moves the list item and its children out one level.
        OutdentBlock,
        /// Vim `>>` or visual `>`: nests the list items on the lines, with their children.
        IndentLine,
        /// Vim `<<` or visual `<`: moves the list items on the lines out one level.
        OutdentLine,
        /// Swaps the list item and its children with the previous sibling.
        MoveBlockUp,
        /// Swaps the list item and its children with the next sibling.
        MoveBlockDown,
        /// Backspace at a block's text start merges it into the block above.
        DeleteBackward,
        /// Delete at a block's text end pulls in the next block.
        DeleteForward,
        /// Cmd-Backspace deletes back to the list item's text start, keeping its marker.
        DeleteToTextStart,
        /// Vim `J`: joins the next block's text to this one.
        JoinBlocks,
        /// Vim `o`: opens a sibling block below and starts Insert mode.
        OpenBelow,
        /// Vim `O`: opens a sibling block above and starts Insert mode.
        OpenAbove,
        /// Pastes several lines into a list item as blocks.
        PasteBlocks,
        /// Collapses the children of the block at the cursor.
        FoldBlock,
        /// Expands the children of the block at the cursor.
        UnfoldBlock,
        /// Collapses or expands the children of the block at the cursor.
        ToggleBlockFold,
        /// Collapses every block with children.
        FoldAllBlocks,
        /// Expands every collapsed block.
        UnfoldAllBlocks,
        /// Numbers the list at the cursor, or turns a numbered list back into bullets.
        ToggleNumberedList,
        /// Numbers the list items at the cursor or in the selection.
        NumberList,
        /// Turns the list items at the cursor or in the selection into bullets.
        BulletList,
        /// Selects the text of the block a zoom or an embed shows, with its children.
        SelectBlock,
        /// Shows only the list item at the caret and its children.
        ZoomIn,
        /// Shows the whole page again after a zoom.
        ZoomOut
    ]
);

const WRITE: &str = "Editor && QuireWrite && !QuireControl";
const WRITE_INPUT: &str = "Editor && QuireWrite && !QuireControl && !VimControl && vim_mode != replace && vim_mode != waiting && vim_mode != literal";
const WRITE_NORMAL: &str =
    "Editor && QuireWrite && !QuireControl && vim_mode == normal && !VimCount";
const WRITE_VISUAL: &str = "Editor && QuireWrite && !QuireControl && vim_mode == visual";

/// The last outline command refusal, shown by the notebook window until the next command.
#[derive(Default)]
pub struct Notice(Option<SharedString>);
impl Global for Notice {}

pub fn notice(cx: &App) -> Option<SharedString> {
    cx.try_global::<Notice>()
        .and_then(|notice| notice.0.clone())
}

/// Shows a notice composed at run time, such as an error that names or quotes its input.
pub(crate) fn show_notice(message: impl Into<SharedString>, cx: &mut App) {
    let message = message.into();
    if notice(cx).as_ref() != Some(&message) {
        cx.set_global(Notice(Some(message)));
    }
}

pub(crate) fn set_notice(message: Option<&'static str>, cx: &mut App) {
    if notice(cx).as_deref() != message {
        cx.set_global(Notice(message.map(SharedString::new_static)));
    }
}

enum Handled {
    Done,
    Record,
    Insert(Page, Plan),
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, window, cx| {
        if let Some(window) = window {
            follow_edits(editor, window, cx);
            folds::follow(window, cx);
        }
        cx.subscribe_self(|editor, event: &editor::EditorEvent, cx| {
            let progress = crate::code_format::formatting(editor)
                && notice(cx).as_deref() == Some(crate::code_format::FORMATTING);
            if !progress
                && matches!(
                    event,
                    editor::EditorEvent::Edited { .. }
                        | editor::EditorEvent::SelectionsChanged { local: true }
                )
            {
                set_notice(None, cx);
            }
        })
        .detach();
        register(editor, cx, |editor, _: &SplitBlock, window, cx| {
            split(editor, window, cx)
        });
        register(editor, cx, |editor, _: &IndentBlock, window, cx| {
            indent(editor, false, false, window, cx)
        });
        register(editor, cx, |editor, _: &OutdentBlock, window, cx| {
            indent(editor, true, false, window, cx)
        });
        register(editor, cx, |editor, _: &IndentLine, window, cx| {
            indent(editor, false, true, window, cx)
        });
        register(editor, cx, |editor, _: &OutdentLine, window, cx| {
            indent(editor, true, true, window, cx)
        });
        register(editor, cx, |editor, _: &MoveBlockUp, window, cx| {
            move_block(editor, true, window, cx)
        });
        register(editor, cx, |editor, _: &MoveBlockDown, window, cx| {
            move_block(editor, false, window, cx)
        });
        register(editor, cx, |editor, _: &DeleteBackward, window, cx| {
            merge(editor, false, window, cx)
        });
        register(editor, cx, |editor, _: &DeleteForward, window, cx| {
            merge(editor, true, window, cx)
        });
        register(editor, cx, |editor, _: &DeleteToTextStart, window, cx| {
            delete_to_text_start(editor, window, cx)
        });
        register(editor, cx, |editor, _: &JoinBlocks, window, cx| {
            join(editor, window, cx)
        });
        register(editor, cx, |editor, _: &OpenBelow, _, cx| {
            open_line(editor, true, cx)
        });
        register(editor, cx, |editor, _: &OpenAbove, _, cx| {
            open_line(editor, false, cx)
        });
        register(editor, cx, |editor, _: &PasteBlocks, window, cx| {
            paste(editor, window, cx)
        });
        register(editor, cx, |editor, _: &FoldBlock, window, cx| {
            fold(editor, Some(true), window, cx)
        });
        register(editor, cx, |editor, _: &UnfoldBlock, window, cx| {
            fold(editor, Some(false), window, cx)
        });
        register(editor, cx, |editor, _: &ToggleBlockFold, window, cx| {
            fold(editor, None, window, cx)
        });
        register(editor, cx, |editor, _: &FoldAllBlocks, window, cx| {
            fold_all(editor, true, window, cx)
        });
        register(editor, cx, |editor, _: &UnfoldAllBlocks, window, cx| {
            fold_all(editor, false, window, cx)
        });
        register(editor, cx, |editor, _: &ToggleNumberedList, window, cx| {
            number_list(editor, None, window, cx)
        });
        register(editor, cx, |editor, _: &NumberList, window, cx| {
            number_list(editor, Some(Style::Numbered), window, cx)
        });
        register(editor, cx, |editor, _: &BulletList, window, cx| {
            number_list(editor, Some(Style::Bullet), window, cx)
        });
        register(editor, cx, |editor, _: &SelectBlock, window, cx| {
            select_block(editor, window, cx)
        });
    })
    .detach();
    bind_keys(cx);
}

fn bind_keys(cx: &mut App) {
    let mac = cfg!(target_os = "macos");
    let platform = |mac_key: &'static str, other: &'static str| if mac { mac_key } else { other };
    cx.bind_keys([
        KeyBinding::new("enter", SplitBlock, Some(WRITE_INPUT)),
        KeyBinding::new("tab", IndentBlock, Some(WRITE_INPUT)),
        KeyBinding::new("shift-tab", OutdentBlock, Some(WRITE_INPUT)),
        KeyBinding::new("backspace", DeleteBackward, Some(WRITE_INPUT)),
        KeyBinding::new("delete", DeleteForward, Some(WRITE_INPUT)),
        KeyBinding::new(platform("cmd-v", "ctrl-v"), PasteBlocks, Some(WRITE_INPUT)),
        KeyBinding::new(
            platform("cmd-a", "ctrl-a"),
            SelectBlock,
            Some(platform(
                "Editor && QuireWrite && QuireExcerpt && !QuireControl",
                "Editor && QuireWrite && QuireExcerpt && !QuireControl && !VimControl",
            )),
        ),
        KeyBinding::new(platform("cmd-.", "ctrl-."), ZoomIn, Some(WRITE)),
        KeyBinding::new(platform("cmd-shift-.", "ctrl-shift-."), ZoomOut, None),
        KeyBinding::new("alt-up", MoveBlockUp, Some(WRITE)),
        KeyBinding::new("alt-down", MoveBlockDown, Some(WRITE)),
        KeyBinding::new("alt-shift-up", MoveBlockUp, Some(WRITE)),
        KeyBinding::new("alt-shift-down", MoveBlockDown, Some(WRITE)),
        KeyBinding::new(
            platform("cmd-shift-up", "ctrl-shift-up"),
            MoveBlockUp,
            Some(WRITE),
        ),
        KeyBinding::new(
            platform("cmd-shift-down", "ctrl-shift-down"),
            MoveBlockDown,
            Some(WRITE),
        ),
        KeyBinding::new(
            platform("cmd-alt-[", "ctrl-shift-["),
            FoldBlock,
            Some(WRITE_INPUT),
        ),
        KeyBinding::new(
            platform("cmd-alt-]", "ctrl-shift-]"),
            UnfoldBlock,
            Some(WRITE_INPUT),
        ),
        KeyBinding::new(
            platform("cmd-shift-7", "ctrl-shift-7"),
            ToggleNumberedList,
            Some(WRITE),
        ),
        KeyBinding::new("o", OpenBelow, Some(WRITE_NORMAL)),
        KeyBinding::new("shift-o", OpenAbove, Some(WRITE_NORMAL)),
        KeyBinding::new("shift-j", JoinBlocks, Some(WRITE_NORMAL)),
        KeyBinding::new("z a", ToggleBlockFold, Some(WRITE_NORMAL)),
        KeyBinding::new("z c", FoldBlock, Some(WRITE_NORMAL)),
        KeyBinding::new("z o", UnfoldBlock, Some(WRITE_NORMAL)),
        KeyBinding::new("z shift-m", FoldAllBlocks, Some(WRITE_NORMAL)),
        KeyBinding::new("z shift-r", UnfoldAllBlocks, Some(WRITE_NORMAL)),
        KeyBinding::new(
            platform("cmd-k cmd-0", "ctrl-k ctrl-0"),
            FoldAllBlocks,
            Some(WRITE),
        ),
        KeyBinding::new(
            platform("cmd-k cmd-j", "ctrl-k ctrl-j"),
            UnfoldAllBlocks,
            Some(WRITE),
        ),
        KeyBinding::new(
            ">",
            IndentLine,
            Some("Editor && QuireWrite && vim_operator == >"),
        ),
        KeyBinding::new(
            "<",
            OutdentLine,
            Some("Editor && QuireWrite && vim_operator == <"),
        ),
        KeyBinding::new(">", IndentLine, Some(WRITE_VISUAL)),
        KeyBinding::new("<", OutdentLine, Some(WRITE_VISUAL)),
    ]);
    // Zed binds deleting to the line start only on macOS; elsewhere Ctrl-Backspace deletes a word.
    if mac {
        cx.bind_keys([KeyBinding::new(
            "cmd-backspace",
            DeleteToTextStart,
            Some(WRITE_INPUT),
        )]);
    }
}

/// Registers a Write view command on an editor. A command returning `None` propagates, so the
/// next binding for its key runs. Commands an embedded editor declines bubble to its host, which
/// leaves them alone so they never edit the page around the embed.
fn register<A: Action>(
    editor: &mut Editor,
    cx: &mut Context<Editor>,
    run: fn(&mut Editor, &A, &mut Window, &mut Context<Editor>) -> Option<Handled>,
) {
    let handle = cx.entity().downgrade();
    editor
        .register_action(move |action: &A, window, cx| {
            let Some(editor) = handle.upgrade() else {
                return cx.propagate();
            };
            if !editor.focus_handle(cx).is_focused(window) {
                return cx.propagate();
            }
            let handled = editor.update(cx, |editor, cx| run(editor, action, window, cx));
            match handled {
                None => cx.propagate(),
                Some(Handled::Done) => {}
                Some(Handled::Record) => {
                    vim::record_host_command(&editor, cx);
                    vim::end_host_visual(&editor, window, cx);
                }
                Some(Handled::Insert(page, plan)) => {
                    vim::begin_host_insert(&editor, window, cx);
                    editor.update(cx, |editor, cx| apply(editor, &page, plan, window, cx));
                }
            }
        })
        .detach();
}

/// The page a Write view shows, in source offsets. A view of one block's subtree, a zoom or a
/// block embed, names that block as `zoom` and may change only the source it `shown`.
struct Page {
    buffer: Entity<language::Buffer>,
    text: String,
    blocks: Vec<Block>,
    head: usize,
    selection: Range<usize>,
    single: bool,
    zoom: Option<String>,
    shown: Range<usize>,
}

pub const OUTSIDE_VIEW: &str =
    "That edit reaches outside the shown block. Zoom out or open its page to change the rest.";

impl Page {
    fn caret(&self) -> Option<usize> {
        (self.single && self.selection.is_empty()).then_some(self.head)
    }

    fn contains(&self, edits: &[Edit]) -> bool {
        edits
            .iter()
            .all(|(range, _)| self.shown.start <= range.start && range.end <= self.shown.end)
    }

    fn zoom(&self) -> Option<&str> {
        self.zoom.as_deref()
    }
}

/// The source buffer a Write view edits: its only buffer, whether the view shows the whole page
/// or one excerpt of it.
pub(crate) fn source_buffer(editor: &Editor, cx: &App) -> Option<Entity<language::Buffer>> {
    let multi = editor.buffer().read(cx);
    multi.as_singleton().or_else(|| {
        let mut buffers = multi.all_buffers_iter();
        buffers.next().filter(|_| buffers.next().is_none())
    })
}

/// A view offset as an offset in the source buffer.
fn to_source(editor: &Editor, offset: MultiBufferOffset, cx: &App) -> Option<usize> {
    let multi = editor.buffer().read(cx).snapshot(cx);
    multi
        .point_to_buffer_offset(offset)
        .map(|(_, offset)| offset.0)
}

/// A source offset as an anchor in the editor's view, if the view shows it.
fn view_anchor(
    editor: &Editor,
    buffer: &Entity<language::Buffer>,
    offset: usize,
    cx: &App,
) -> Option<multi_buffer::Anchor> {
    let anchor = buffer.read(cx).snapshot().anchor_before(offset);
    editor
        .buffer()
        .read(cx)
        .snapshot(cx)
        .anchor_in_excerpt(anchor)
}

fn view_range(
    editor: &Editor,
    buffer: &Entity<language::Buffer>,
    range: Range<usize>,
    cx: &App,
) -> Option<Range<MultiBufferOffset>> {
    let multi = editor.buffer().read(cx).snapshot(cx);
    let start = view_anchor(editor, buffer, range.start, cx)?.to_offset(&multi);
    let end = view_anchor(editor, buffer, range.end, cx)?.to_offset(&multi);
    Some(start..end)
}

/// The page under a Write view: a whole page, or one block's subtree in a zoom or an embed.
fn page(editor: &mut Editor, cx: &mut Context<Editor>) -> Option<Page> {
    if !crate::editor_projection::enabled(editor) {
        return None;
    }
    let buffer = source_buffer(editor, cx)?;
    let snapshot = buffer.read(cx).snapshot();
    let whole = editor.buffer().read(cx).as_singleton().is_some();
    let zoom = if whole {
        None
    } else {
        Some(crate::semantic_excerpt::root(editor, cx)?)
    };
    let text = snapshot.text();
    let shown = if whole {
        0..snapshot.len()
    } else {
        let multi = editor.buffer().read(cx).snapshot(cx);
        let excerpt = multi.excerpts().next()?;
        let start = text::ToOffset::to_offset(&excerpt.context.start, &snapshot);
        model::line_start(&text, start)..text::ToOffset::to_offset(&excerpt.context.end, &snapshot)
    };
    let blocks = crate::markdown_blocks::current_blocks(&snapshot)?;
    let multi = editor.buffer().read(cx).snapshot(cx);
    let newest = *editor.selections.newest_anchor();
    let offset = |anchor: multi_buffer::Anchor| to_source(editor, anchor.to_offset(&multi), cx);
    let start = offset(newest.start)?;
    let end = offset(newest.end)?;
    let head = offset(newest.head())?;
    Some(Page {
        text,
        blocks,
        head,
        selection: start..end,
        single: editor.selections.count() == 1,
        buffer,
        zoom,
        shown,
    })
}

const LITERAL: &[&str] = &[
    "fenced_code_block",
    "indented_code_block",
    "code_span",
    "pipe_table",
    "html_block",
    "thematic_break",
    "link_reference_definition",
    "atx_heading",
    "setext_heading",
];

/// Whether the character before the position is in code, a table, raw HTML, or a heading, which
/// keep their text, so a caret right after a closing backtick counts as inside. At a line start
/// the line itself decides.
fn literal(buffer: &Entity<language::Buffer>, position: usize, cx: &App) -> bool {
    let snapshot = buffer.read(cx).snapshot();
    let before = match snapshot.reversed_chars_at(position).next() {
        Some('\n') | None => position,
        Some(_) => position - 1,
    };
    snapshot.syntax_layers().any(|layer| {
        let mut node = layer.node().descendant_for_byte_range(before, position);
        while let Some(current) = node {
            if LITERAL.contains(&current.kind()) {
                return true;
            }
            node = current.parent();
        }
        false
    })
}

/// Applies source edits that follow up an earlier edit, folding them into that edit's
/// transaction `into` while it is still the latest, so one undo reverts both.
fn amend(
    buffer: &Entity<language::Buffer>,
    edits: &[Edit],
    into: Option<text::TransactionId>,
    cx: &mut App,
) {
    let latest =
        |buffer: &language::Buffer| buffer.peek_undo_stack().map(|entry| entry.transaction_id());
    let into = into.filter(|into| latest(buffer.read(cx)) == Some(*into));
    source_edit(buffer, edits, cx);
    buffer.update(cx, |buffer, _| {
        if let (Some(into), Some(added)) = (into, latest(buffer))
            && added != into
        {
            buffer.merge_transactions(added, into);
        }
    });
}

/// Applies source edits that complete an earlier edit after other edits may have followed it,
/// such as a fresh ID the index asked for, folding them into that edit's transaction `into`
/// while its history still holds it. Undo reverts edits by their own operations, so the
/// later edits stay intact when undo reaches `into`.
fn amend_later(
    buffer: &Entity<language::Buffer>,
    edits: &[Edit],
    into: Option<text::TransactionId>,
    cx: &mut App,
) {
    let into = into.filter(|into| buffer.read(cx).get_transaction(*into).is_some());
    let Some(into) = into else {
        return source_edit(buffer, edits, cx);
    };
    buffer.update(cx, |buffer, _| {
        buffer.finalize_last_transaction();
    });
    source_edit(buffer, edits, cx);
    buffer.update(cx, |buffer, _| {
        if let Some(added) = buffer.peek_undo_stack().map(|entry| entry.transaction_id())
            && added != into
        {
            buffer.merge_transactions(added, into);
        }
    });
}

/// Edits the source buffer directly. Every view of the page shares its history, so the edit
/// joins the running editor transaction and no view's Write filter rewrites it.
fn source_edit(buffer: &Entity<language::Buffer>, edits: &[Edit], cx: &mut App) {
    if edits.is_empty() {
        return;
    }
    buffer.update(cx, |buffer, cx| {
        buffer.edit(
            edits
                .iter()
                .map(|(range, text)| (range.clone(), text.as_str())),
            None,
            cx,
        )
    });
}

pub(crate) fn structural_edit(buffer: &Entity<MultiBuffer>, edits: &[Edit], cx: &mut App) {
    if edits.is_empty() {
        return;
    }
    let _intent = buffer
        .read(cx)
        .scoped_edit_intent(ViewEditIntent::Structural);
    buffer.update(cx, |buffer, cx| {
        buffer.edit(
            edits.iter().map(|(range, text)| {
                (
                    MultiBufferOffset(range.start)..MultiBufferOffset(range.end),
                    text.as_str(),
                )
            }),
            None,
            cx,
        )
    });
}

fn changed_ranges(edits: &[Edit]) -> Vec<Range<usize>> {
    let mut delta = 0isize;
    edits
        .iter()
        .map(|(range, text)| {
            let start = range.start.saturating_add_signed(delta);
            delta += text.len() as isize - range.len() as isize;
            start..start + text.len()
        })
        .collect()
}

fn apply(
    editor: &mut Editor,
    page: &Page,
    plan: Plan,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    apply_then(editor, page, plan, |_, _, _| None, window, cx)
}

/// Follows edits that are not outline commands, such as pasting, typing a numbered line, Vim
/// `dd` or `yyp`, or undo, in the focused view: copied block IDs become fresh ones, and in
/// Write view the numbered runs an edit touched keep counting. Both land right after the edit,
/// so the buffer groups them with it into one undo step.
fn follow_edits(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    let Some(buffer) = source_buffer(editor, cx) else {
        return;
    };
    let mut seen = buffer.read(cx).version();
    cx.subscribe_in(&buffer, window, move |editor, buffer, event, window, cx| {
        let language::BufferEvent::Edited { source } = event else {
            return;
        };
        let since = std::mem::replace(&mut seen, buffer.read(cx).version());
        if !source.is_local() || !editor.focus_handle(cx).is_focused(window) {
            return;
        }
        let touched = |cx: &App| -> Vec<Range<usize>> {
            buffer
                .read(cx)
                .snapshot()
                .edits_since::<usize>(&since)
                .map(|edit| edit.new)
                .collect()
        };
        let inserted = touched(cx);
        if inserted.is_empty() {
            return;
        }
        let into = buffer
            .read(cx)
            .peek_undo_stack()
            .map(|entry| entry.transaction_id());
        fresh_ids::follow(editor, buffer, &inserted, into, cx);
        if crate::editor_projection::enabled(editor) {
            let touched = touched(cx);
            if numbered(&buffer.read(cx).text(), &touched) {
                let renumbered = renumber_touched(buffer, &touched, cx);
                amend(buffer, &renumbered, into, cx);
            }
        }
        seen = buffer.read(cx).version();
    })
    .detach();
}

/// Whether a numbered list item sits on or right after a line an edit touched.
fn numbered(text: &str, touched: &[Range<usize>]) -> bool {
    touched.iter().any(|range| {
        let from = model::line_start(text, range.start);
        let to = model::line_end(text, range.end);
        let to = model::line_end(text, (to + 1).min(text.len()));
        text[from..to].lines().any(|line| {
            model::list_mark(line.trim_start())
                .is_some_and(|mark| mark.starts_with(|c: char| c.is_ascii_digit()))
        })
    })
}

/// Applies the plan, then each plan `then` computes from the edited page and the caret until it
/// returns None, as one undo step. Numbered runs the edits touched keep counting. A plan
/// reaching outside the block a zoom or an embed shows is refused.
fn apply_then(
    editor: &mut Editor,
    page: &Page,
    plan: Plan,
    mut then: impl FnMut(&str, &[Block], usize) -> Option<Plan>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if !page.contains(&plan.edits) {
        set_notice(Some(OUTSIDE_VIEW), cx);
        return;
    }
    set_notice(None, cx);
    let kept = folds::keep(editor, page, cx);
    let source = page.buffer.clone();
    let edited = |source: &Entity<language::Buffer>, cx: &App| {
        let snapshot = source.read(cx).snapshot();
        let blocks = crate::markdown_blocks::current_blocks(&snapshot);
        (snapshot.text(), blocks)
    };
    editor.finalize_last_transaction(cx);
    editor.transact(window, cx, |editor, window, cx| {
        let mut touched = changed_ranges(&plan.edits);
        let mut cursor = plan.cursor;
        let mut shown_end = model::map_position(&plan.edits, page.shown.end, true);
        source_edit(&source, &plan.edits, cx);
        loop {
            let (text, blocks) = edited(&source, cx);
            let Some(next) = blocks
                .and_then(|blocks| then(&text, &blocks, cursor))
                .filter(|next| {
                    next.edits
                        .iter()
                        .all(|(range, _)| page.shown.start <= range.start && range.end <= shown_end)
                })
            else {
                break;
            };
            shown_end = model::map_position(&next.edits, shown_end, true);
            touched = touched
                .into_iter()
                .map(|range| {
                    model::map_position(&next.edits, range.start, false)
                        ..model::map_position(&next.edits, range.end, true)
                })
                .chain(changed_ranges(&next.edits))
                .collect();
            source_edit(&source, &next.edits, cx);
            cursor = next.cursor;
        }
        let renumbered = renumber_touched(&source, &touched, cx);
        let cursor = model::map_position(&renumbered, cursor, true);
        source_edit(&source, &renumbered, cx);
        if let Some(caret) = view_anchor(editor, &source, cursor, cx) {
            editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                selections.select_anchor_ranges([caret..caret])
            });
        }
    });
    editor.finalize_last_transaction(cx);
    folds::reconcile(editor, &source, kept, window, cx);
}

/// The renumbering that keeps the numbered runs overlapping `touched` counting.
fn renumber_touched(
    source: &Entity<language::Buffer>,
    touched: &[Range<usize>],
    cx: &App,
) -> Vec<Edit> {
    if touched.is_empty() {
        return Vec::new();
    }
    let snapshot = source.read(cx).snapshot();
    let Some(blocks) = crate::markdown_blocks::current_blocks(&snapshot) else {
        return Vec::new();
    };
    lists::renumber_changes(&snapshot.text(), &blocks, |from, to| {
        touched
            .iter()
            .any(|range| range.start <= to && range.end >= from)
    })
}

fn finish(
    editor: &mut Editor,
    page: &Page,
    outcome: Option<Outcome>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    match outcome? {
        Outcome::Apply(plan) => apply(editor, page, plan, window, cx),
        Outcome::Refuse(message) => set_notice(Some(message), cx),
    }
    Some(Handled::Done)
}

fn split(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = page.caret()?;
    if !editor.context_menu_visible()
        && let Some(at) = model::step_out_of_link(&page.text, head)
    {
        let anchor = view_anchor(editor, &page.buffer, at, cx)?;
        editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
            selections.select_anchor_ranges([anchor..anchor])
        });
        return Some(Handled::Done);
    }
    if literal(&page.buffer, head, cx) {
        return None;
    }
    let folds = folds::collapsed_source(editor, cx);
    let outcome = model::split(&page.text, &page.blocks, head, page.zoom(), |at| {
        folds.iter().any(|fold| fold.start == at)
    })
    .or_else(|| model::newline_after_id(&page.text, &page.blocks, head));
    finish(editor, &page, outcome, window, cx)
}

fn indent(
    editor: &mut Editor,
    outdent: bool,
    line: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    if line {
        return indent_lines(editor, &page, outdent, window, cx);
    }
    let head = page.caret()?;
    if literal(&page.buffer, head, cx) {
        return None;
    }
    let zoom = page.zoom.clone();
    if let Some(bullet) = model::bare_bullet(&page.text, head) {
        let mut pending = true;
        let then = |text: &str, blocks: &[Block], cursor| {
            std::mem::take(&mut pending).then_some(())?;
            match model::indent(text, blocks, cursor, outdent, zoom.as_deref()) {
                Some(Outcome::Apply(next)) => Some(next),
                _ => None,
            }
        };
        apply_then(editor, &page, bullet, then, window, cx);
        return Some(Handled::Done);
    }
    let outcome = model::indent(&page.text, &page.blocks, head, outdent, page.zoom());
    finish(editor, &page, outcome, window, cx)
}

/// Vim `>>` and `<<`, with a count of lines, and `>` and `<` in a visual mode, with a count of
/// levels: the list items starting on the lines move with their children. Lines without list
/// items keep Vim's indent.
fn indent_lines(
    editor: &mut Editor,
    page: &Page,
    outdent: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    if !page.single {
        return None;
    }
    let mut lines = page.selection.clone();
    if lines.end > lines.start && model::line_start(&page.text, lines.end) == lines.end {
        lines.end -= 1;
    }
    let first = model::list_line(&page.text, &page.blocks, lines.start)?.1;
    if literal(&page.buffer, first.text_start, cx) {
        return None;
    }
    let count = vim::take_host_count(cx).unwrap_or(1);
    let mut levels = 1;
    if lines.is_empty() {
        lines.end = (1..count).fold(lines.end, |at, _| {
            (model::line_end(&page.text, at) + 1).min(page.text.len())
        });
    } else {
        levels = count;
    }
    let rows = model::row_of(&page.text, lines.start)..model::row_of(&page.text, lines.end);
    let zoom = page.zoom.clone();
    match model::indent_lines(&page.text, &page.blocks, lines, outdent, page.zoom())? {
        Outcome::Apply(plan) => {
            let mut left = levels - 1;
            let then = |text: &str, blocks: &[Block], _| {
                left = left.checked_sub(1)?;
                let lines = model::row_start(text, rows.start)..model::row_start(text, rows.end);
                match model::indent_lines(text, blocks, lines, outdent, zoom.as_deref()) {
                    Some(Outcome::Apply(next)) => Some(next),
                    _ => None,
                }
            };
            apply_then(editor, page, plan, then, window, cx);
            Some(Handled::Record)
        }
        Outcome::Refuse(message) => {
            set_notice(Some(message), cx);
            Some(Handled::Done)
        }
    }
}

fn move_block(
    editor: &mut Editor,
    up: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let outcome = model::move_block(&page.text, &page.blocks, page.head, up);
    finish(editor, &page, outcome, window, cx)
}

fn merge(
    editor: &mut Editor,
    forward: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = page.caret()?;
    if !forward && let Some(plan) = model::delete_empty_link(&page.text, head) {
        apply(editor, &page, plan, window, cx);
        return Some(Handled::Done);
    }
    let outcome = if forward {
        model::merge_forward(&page.text, &page.blocks, head)
    } else {
        model::merge_backward(&page.text, &page.blocks, head)
    }?;
    let line_from = model::line_start(&page.text, head);
    let line_to = model::line_end(&page.text, head);
    let hidden = folds::collapsed_source(editor, cx).into_iter().any(|fold| {
        if forward {
            head <= fold.start && fold.start <= line_to
        } else {
            line_from
                .checked_sub(1)
                .is_some_and(|above| fold.start < above && above <= fold.end)
        }
    });
    if hidden {
        return Some(Handled::Done);
    }
    finish(editor, &page, Some(outcome), window, cx)
}

fn delete_to_text_start(
    editor: &mut Editor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let plan = model::delete_to_text_start(&page.text, &page.blocks, page.caret()?)?;
    apply(editor, &page, plan, window, cx);
    Some(Handled::Done)
}

fn join(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let outcome = model::join(&page.text, &page.blocks, page.head);
    finish(editor, &page, outcome, window, cx).map(|_| Handled::Record)
}

fn open_line(editor: &mut Editor, below: bool, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let folds = folds::collapsed_source(editor, cx);
    let above_root = !below
        && page.zoom.is_some()
        && model::list_line(&page.text, &page.blocks, page.head)
            .is_some_and(|(block, _)| block.id == page.zoom);
    if above_root {
        set_notice(Some(OUTSIDE_VIEW), cx);
        return Some(Handled::Done);
    }
    let plan = model::open_line(
        &page.text,
        &page.blocks,
        page.head,
        below,
        page.zoom(),
        |at| folds.iter().any(|fold| fold.start == at),
    )?;
    if !page.contains(&plan.edits) {
        set_notice(Some(OUTSIDE_VIEW), cx);
        return Some(Handled::Done);
    }
    Some(Handled::Insert(page, plan))
}

fn paste(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = page.caret()?;
    let pasted = cx.read_from_clipboard()?.text()?;
    let plan = model::paste(&page.text, &page.blocks, head, &pasted)?;
    apply(editor, &page, plan, window, cx);
    Some(Handled::Done)
}

/// The block whose children a fold command collapses: the list item on the cursor's line, or
/// its nearest ancestor with children.
fn fold_target(page: &Page) -> Option<&Block> {
    let mut block = page.blocks.iter().rfind(|block| {
        block.kind == "list"
            && model::line_start(&page.text, block.from) <= page.head
            && page.head <= block.subtree_to
    })?;
    while block.subtree_to <= block.content_to {
        let parent = block.parent.as_deref()?;
        block = page.blocks.iter().find(|item| item.key == parent)?;
    }
    Some(block)
}

fn fold(
    editor: &mut Editor,
    collapse: Option<bool>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let collapsed = folds::collapsed_source(editor, cx);
    let folded = model::list_line(&page.text, &page.blocks, page.head)
        .map(|(block, _)| block)
        .filter(|block| collapsed.iter().any(|fold| fold.start == block.content_to));
    let target = fold_target(&page);
    match (collapse, folded, target) {
        (Some(false) | None, Some(block), _) => {
            let range = view_range(editor, &page.buffer, block.content_to..block.subtree_to, cx)?;
            folds::set(editor, range, false, window, cx);
        }
        (Some(true) | None, _, Some(block)) => {
            let range = view_range(editor, &page.buffer, block.content_to..block.subtree_to, cx)?;
            let end = model::bounds(&page.text, block).text_end;
            let end = view_anchor(editor, &page.buffer, end, cx);
            folds::set(editor, range, true, window, cx);
            if let Some(end) = end.filter(|_| page.head > block.content_to) {
                editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                    selections.select_anchor_ranges([end..end])
                });
            }
        }
        _ => {}
    }
    Some(Handled::Done)
}

fn fold_all(
    editor: &mut Editor,
    collapse: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    folds::set_all(editor, &page, collapse, window, cx);
    Some(Handled::Done)
}

fn number_list(
    editor: &mut Editor,
    style: Option<Style>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let Some(edits) = lists::numbered_list_changes(
        &page.text,
        &page.blocks,
        page.selection.start,
        page.selection.end,
        style,
    ) else {
        set_notice(Some(lists::NO_LIST), cx);
        return Some(Handled::Done);
    };
    let cursor = model::map_position(&edits, page.head, true);
    apply(editor, &page, Plan { edits, cursor }, window, cx);
    Some(Handled::Done)
}

/// Select all in a zoom or a block embed: the shown block's text and its children, past its
/// marker and checkbox.
fn select_block(
    editor: &mut Editor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let zoom = page.zoom()?;
    let block = page
        .blocks
        .iter()
        .find(|block| block.id.as_deref() == Some(zoom))?;
    let text_start = model::bounds(&page.text, block).text_start;
    let start = view_anchor(editor, &page.buffer, text_start, cx)?;
    let end = view_anchor(editor, &page.buffer, block.subtree_to, cx)?;
    editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
        selections.select_anchor_ranges([start..end])
    });
    Some(Handled::Done)
}

#[cfg(test)]
#[path = "outline/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "outline/editor_parity_tests.rs"]
mod editor_parity_tests;
