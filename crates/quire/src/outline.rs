//! Outline editing in Write view: list continuation, indentation, block movement, merging,
//! multi-line paste, numbered lists, and folding of block subtrees. Each command is one
//! undoable source transaction applied through the Zed editor; commands that do not apply
//! propagate to the next key binding, so ordinary editing keeps its Zed behavior.

use crate::markdown_blocks::Block;
use editor::{Editor, FoldPlaceholder, SelectionEffects, display_map::Crease};
use gpui::{Action, App, Context, Entity, Global, KeyBinding, SharedString, Window, actions};
use multi_buffer::{MultiBuffer, MultiBufferOffset, ToOffset, ViewEditIntent};
use std::{any::TypeId, ops::Range};

mod lists;
mod model;

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
        /// Vim `>>`: nests the list item on the cursor's line.
        IndentLine,
        /// Vim `<<`: moves the list item on the cursor's line out one level.
        OutdentLine,
        /// Swaps the list item and its children with the previous sibling.
        MoveBlockUp,
        /// Swaps the list item and its children with the next sibling.
        MoveBlockDown,
        /// Backspace at a block's text start merges it into the block above.
        DeleteBackward,
        /// Delete at a block's text end pulls in the next block.
        DeleteForward,
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
        /// Numbers the list at the cursor, or turns a numbered list back into bullets.
        ToggleNumberedList,
        /// Numbers the list items at the cursor or in the selection.
        NumberList,
        /// Turns the list items at the cursor or in the selection into bullets.
        BulletList
    ]
);

const WRITE: &str = "Editor && QuireWrite && !QuireControl";
const WRITE_INPUT: &str = "Editor && QuireWrite && !QuireControl && !VimControl && vim_mode != replace && vim_mode != waiting && vim_mode != literal";
const WRITE_NORMAL: &str =
    "Editor && QuireWrite && !QuireControl && vim_mode == normal && !VimCount";

/// The last outline command refusal, shown by the notebook window until the next command.
#[derive(Default)]
pub struct Notice(Option<SharedString>);
impl Global for Notice {}

pub fn notice(cx: &App) -> Option<SharedString> {
    cx.try_global::<Notice>()
        .and_then(|notice| notice.0.clone())
}

pub(crate) fn set_notice(message: Option<&'static str>, cx: &mut App) {
    if notice(cx).as_deref() != message {
        cx.set_global(Notice(message.map(SharedString::new_static)));
    }
}

/// Shows a message built at run time, such as a formatter error that quotes its input.
pub(crate) fn show_notice(message: impl Into<SharedString>, cx: &mut App) {
    cx.set_global(Notice(Some(message.into())));
}

/// Marks folds that collapse a block's children, apart from the folds hiding Write syntax.
struct OutlineFold;

enum Handled {
    Done,
    Record,
    Insert(Plan),
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        cx.subscribe_self(|_, event: &editor::EditorEvent, cx| {
            if matches!(
                event,
                editor::EditorEvent::Edited { .. }
                    | editor::EditorEvent::SelectionsChanged { local: true }
            ) {
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
        register(editor, cx, |editor, _: &ToggleNumberedList, window, cx| {
            number_list(editor, None, window, cx)
        });
        register(editor, cx, |editor, _: &NumberList, window, cx| {
            number_list(editor, Some(Style::Numbered), window, cx)
        });
        register(editor, cx, |editor, _: &BulletList, window, cx| {
            number_list(editor, Some(Style::Bullet), window, cx)
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
    ]);
}

/// Registers a Write view command on an editor. A command returning `None` propagates, so the
/// next binding for its key runs.
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
            let handled = editor.update(cx, |editor, cx| run(editor, action, window, cx));
            match handled {
                None => cx.propagate(),
                Some(Handled::Done) => {}
                Some(Handled::Record) => vim::record_host_command(&editor, cx),
                Some(Handled::Insert(plan)) => {
                    vim::begin_host_insert(&editor, window, cx);
                    editor.update(cx, |editor, cx| apply(editor, plan, window, cx));
                }
            }
        })
        .detach();
}

struct Page {
    buffer: Entity<language::Buffer>,
    text: String,
    blocks: Vec<Block>,
    head: usize,
    selection: Range<usize>,
    single: bool,
}

impl Page {
    fn caret(&self) -> Option<usize> {
        (self.single && self.selection.is_empty()).then_some(self.head)
    }
}

/// The page in a Write view of one whole page. Embedded excerpts keep ordinary editing.
fn page(editor: &mut Editor, cx: &mut Context<Editor>) -> Option<Page> {
    if !crate::editor_projection::enabled(editor) {
        return None;
    }
    let buffer = editor.buffer().read(cx).as_singleton()?;
    let snapshot = buffer.read(cx).snapshot();
    let blocks = crate::markdown_blocks::current_blocks(&snapshot)?;
    let newest = editor
        .selections
        .newest_anchor()
        .map(|anchor| anchor.to_offset(&editor.buffer().read(cx).snapshot(cx)));
    Some(Page {
        text: snapshot.text(),
        blocks,
        head: newest.head().0,
        selection: newest.start.0..newest.end.0,
        single: editor.selections.count() == 1,
        buffer,
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

/// Whether the position is inside code, a table, raw HTML, or a heading, which keep their text.
fn literal(buffer: &Entity<language::Buffer>, position: usize, cx: &App) -> bool {
    let snapshot = buffer.read(cx).snapshot();
    snapshot.syntax_layers().any(|layer| {
        let mut node = layer.node().descendant_for_byte_range(position, position);
        while let Some(current) = node {
            if LITERAL.contains(&current.kind()) {
                return true;
            }
            node = current.parent();
        }
        false
    })
}

fn outline_folds(editor: &mut Editor, cx: &mut Context<Editor>) -> Vec<Range<usize>> {
    let snapshot = editor.display_snapshot(cx);
    let buffer = snapshot.buffer_snapshot();
    snapshot
        .folds_in_range(MultiBufferOffset(0)..buffer.len())
        .filter(|fold| fold.placeholder.type_tag == Some(TypeId::of::<OutlineFold>()))
        .map(|fold| fold.range.start.to_offset(buffer).0..fold.range.end.to_offset(buffer).0)
        .collect()
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

fn apply(editor: &mut Editor, plan: Plan, window: &mut Window, cx: &mut Context<Editor>) {
    apply_then(editor, plan, |_, _, _| None, window, cx)
}

/// Applies the plan, then the plan `then` computes from the edited page and the caret, as one
/// undo step. Numbered runs the edits touched keep counting.
fn apply_then(
    editor: &mut Editor,
    plan: Plan,
    then: impl FnOnce(&str, &[Block], usize) -> Option<Plan>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    set_notice(None, cx);
    let buffer = editor.buffer().clone();
    let Some(source) = buffer.read(cx).as_singleton() else {
        return;
    };
    let edited = |source: &Entity<language::Buffer>, cx: &App| {
        let snapshot = source.read(cx).snapshot();
        let blocks = crate::markdown_blocks::current_blocks(&snapshot);
        (snapshot.text(), blocks)
    };
    editor.finalize_last_transaction(cx);
    editor.transact(window, cx, |editor, window, cx| {
        let mut touched = changed_ranges(&plan.edits);
        let mut cursor = plan.cursor;
        structural_edit(&buffer, &plan.edits, cx);
        let (text, blocks) = edited(&source, cx);
        if let Some(next) = blocks.and_then(|blocks| then(&text, &blocks, cursor)) {
            touched = touched
                .into_iter()
                .map(|range| {
                    model::map_position(&next.edits, range.start, false)
                        ..model::map_position(&next.edits, range.end, true)
                })
                .chain(changed_ranges(&next.edits))
                .collect();
            structural_edit(&buffer, &next.edits, cx);
            cursor = next.cursor;
        }
        let (text, blocks) = edited(&source, cx);
        let renumbered = blocks
            .filter(|_| !touched.is_empty())
            .map(|blocks| {
                lists::renumber_changes(&text, &blocks, |from, to| {
                    touched
                        .iter()
                        .any(|range| range.start <= to && range.end >= from)
                })
            })
            .unwrap_or_default();
        let cursor = model::map_position(&renumbered, cursor, true);
        structural_edit(&buffer, &renumbered, cx);
        editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
            selections.select_ranges([MultiBufferOffset(cursor)..MultiBufferOffset(cursor)])
        });
    });
    editor.finalize_last_transaction(cx);
}

fn finish(
    editor: &mut Editor,
    outcome: Option<Outcome>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    match outcome? {
        Outcome::Apply(plan) => apply(editor, plan, window, cx),
        Outcome::Refuse(message) => set_notice(Some(message), cx),
    }
    Some(Handled::Done)
}

fn split(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = page.caret()?;
    if literal(&page.buffer, head, cx) {
        return None;
    }
    let folds = outline_folds(editor, cx);
    let outcome = model::split(&page.text, &page.blocks, head, None, |at| {
        folds.iter().any(|fold| fold.start == at)
    })
    .or_else(|| model::newline_after_id(&page.text, &page.blocks, head));
    finish(editor, outcome, window, cx)
}

fn indent(
    editor: &mut Editor,
    outdent: bool,
    line: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = if line {
        if !page.single
            || model::line_start(&page.text, page.selection.start)
                != model::line_start(&page.text, page.selection.end)
        {
            return None;
        }
        model::list_line(&page.text, &page.blocks, page.selection.start)?
            .1
            .text_start
    } else {
        page.caret()?
    };
    if literal(&page.buffer, head, cx) {
        return None;
    }
    if !line && let Some(bullet) = model::bare_bullet(&page.text, head) {
        let then = |text: &str, blocks: &[Block], cursor| match model::indent(
            text, blocks, cursor, outdent, None,
        ) {
            Some(Outcome::Apply(next)) => Some(next),
            _ => None,
        };
        apply_then(editor, bullet, then, window, cx);
        return Some(Handled::Done);
    }
    let outcome = model::indent(&page.text, &page.blocks, head, outdent, None);
    finish(editor, outcome, window, cx).map(|handled| if line { Handled::Record } else { handled })
}

fn move_block(
    editor: &mut Editor,
    up: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let outcome = model::move_block(&page.text, &page.blocks, page.head, up);
    finish(editor, outcome, window, cx)
}

fn merge(
    editor: &mut Editor,
    forward: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = page.caret()?;
    let outcome = if forward {
        model::merge_forward(&page.text, &page.blocks, head)
    } else {
        model::merge_backward(&page.text, &page.blocks, head)
    }?;
    let line_from = model::line_start(&page.text, head);
    let line_to = model::line_end(&page.text, head);
    let hidden = outline_folds(editor, cx).into_iter().any(|fold| {
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
    finish(editor, Some(outcome), window, cx)
}

fn join(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let outcome = model::join(&page.text, &page.blocks, page.head);
    finish(editor, outcome, window, cx).map(|_| Handled::Record)
}

fn open_line(editor: &mut Editor, below: bool, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let folds = outline_folds(editor, cx);
    model::open_line(&page.text, &page.blocks, page.head, below, None, |at| {
        folds.iter().any(|fold| fold.start == at)
    })
    .map(Handled::Insert)
}

fn paste(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) -> Option<Handled> {
    let page = page(editor, cx)?;
    let head = page.caret()?;
    let pasted = cx.read_from_clipboard()?.text()?;
    let plan = model::paste(&page.text, &page.blocks, head, &pasted)?;
    apply(editor, plan, window, cx);
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
    let folds = outline_folds(editor, cx);
    let folded = model::list_line(&page.text, &page.blocks, page.head)
        .map(|(block, _)| block)
        .filter(|block| folds.iter().any(|fold| fold.start == block.content_to));
    let target = fold_target(&page);
    match (collapse, folded, target) {
        (Some(false) | None, Some(block), _) => {
            let range = MultiBufferOffset(block.content_to)..MultiBufferOffset(block.subtree_to);
            editor.remove_folds_with_type(&[range], TypeId::of::<OutlineFold>(), true, cx);
        }
        (Some(true) | None, _, Some(block)) => {
            let range = MultiBufferOffset(block.content_to)..MultiBufferOffset(block.subtree_to);
            let placeholder = FoldPlaceholder {
                type_tag: Some(TypeId::of::<OutlineFold>()),
                merge_adjacent: false,
                ..editor.default_fold_placeholder(cx)
            };
            let end = model::bounds(&page.text, block).text_end;
            editor.fold_creases(vec![Crease::simple(range, placeholder)], true, window, cx);
            if page.head > block.content_to {
                editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
                    selections.select_ranges([MultiBufferOffset(end)..MultiBufferOffset(end)])
                });
            }
        }
        _ => {}
    }
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
    apply(editor, Plan { edits, cursor }, window, cx);
    Some(Handled::Done)
}

#[cfg(test)]
#[path = "outline/tests.rs"]
mod tests;
