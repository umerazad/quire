//! The page's pointer menu (NR16.2), installed as each page editor's custom Zed context menu
//! and opened by a right-click or Shift-F10: the clipboard, inline marks on a selection, the
//! list item's style and task, and its block reference. Every item dispatches an editor action
//! from the editor, so the menu and the keys run the same code.

use crate::navigation::pointer_menu::{self, Entry, Item};
use editor::{DisplayPoint, Editor, MultiBufferOffset, SelectionEffects};
use gpui::{App, Context, Entity, Focusable, KeyBinding, Window, actions};
use language::Buffer;
use multi_buffer::ToOffset;
use std::ops::Range;
use ui::ContextMenu;

actions!(
    quire_page,
    [
        /// Wraps the selection in `**`, or takes the marks off.
        ToggleBold,
        /// Wraps the selection in `_`, or takes the marks off.
        ToggleItalic,
        /// Wraps the selection in `~~`, or takes the marks off.
        ToggleStrikethrough,
        /// Wraps the selection in backticks, or takes them off.
        ToggleCode,
        /// Wraps the selection in `[[` and `]]`, or takes them off.
        ToggleLinkToPage,
        /// Copies `((id))` for the list item at the caret, giving it an ID when it has none.
        CopyBlockReference,
        /// Copies `{{embed: ((id))}}` for the list item at the caret.
        CopyBlockEmbed,
    ]
);

pub const NOT_IN_BLOCK: &str = "Place the caret in a list item, then copy its reference or embed.";
pub const READ_ONLY: &str =
    "This page is read-only and the item has no ID to copy. Reopen the page, then try again.";

/// The reference `inlineMarks`: Italic uses `_` so it never reads as half of a `**` pair.
const MARKS: [(&str, &str, &str); 5] = [
    ("Bold", "**", "**"),
    ("Italic", "_", "_"),
    ("Strikethrough", "~~", "~~"),
    ("Code", "`", "`"),
    ("Link to page", "[[", "]]"),
];

#[derive(Debug, PartialEq, Eq)]
pub struct MarkEdit {
    pub changes: Vec<(Range<usize>, String)>,
    pub selection: Range<usize>,
}

/// The reference `toggleMark`: wraps `from..to` in a mark, or takes the mark off when the
/// selection is already wrapped, inside or just outside it. An empty selection gets an empty
/// pair with the caret between.
pub fn toggle_mark(doc: &str, from: usize, to: usize, open: &str, close: &str) -> MarkEdit {
    let inner = &doc[from..to];
    let width = open.len() + close.len();
    if inner.len() >= width && inner.starts_with(open) && inner.ends_with(close) {
        return MarkEdit {
            changes: vec![(
                from..to,
                inner[open.len()..inner.len() - close.len()].into(),
            )],
            selection: from..to - width,
        };
    }
    let before = from
        .checked_sub(open.len())
        .and_then(|start| doc.get(start..from));
    let after = doc.get(to..to + close.len());
    if before == Some(open) && after == Some(close) && (from < to || open != close) {
        return MarkEdit {
            changes: vec![
                (from - open.len()..from, String::new()),
                (to..to + close.len(), String::new()),
            ],
            selection: from - open.len()..to - open.len(),
        };
    }
    MarkEdit {
        changes: vec![(from..from, open.into()), (to..to, close.into())],
        selection: from + open.len()..to + open.len(),
    }
}

/// Runs `act` on the editor when it receives `A`.
fn register<A: gpui::Action>(
    editor: &mut Editor,
    cx: &mut Context<Editor>,
    act: impl Fn(&mut Editor, &mut Window, &mut Context<Editor>) + 'static,
) {
    let handle = cx.entity().downgrade();
    editor
        .register_action(move |_: &A, window, cx| {
            if let Some(editor) = handle.upgrade() {
                editor.update(cx, |editor, cx| act(editor, window, cx));
            }
        })
        .detach();
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        register::<ToggleBold>(editor, cx, |e, w, cx| apply_mark(e, 0, w, cx));
        register::<ToggleItalic>(editor, cx, |e, w, cx| apply_mark(e, 1, w, cx));
        register::<ToggleStrikethrough>(editor, cx, |e, w, cx| apply_mark(e, 2, w, cx));
        register::<ToggleCode>(editor, cx, |e, w, cx| apply_mark(e, 3, w, cx));
        register::<ToggleLinkToPage>(editor, cx, |e, w, cx| apply_mark(e, 4, w, cx));
        register::<CopyBlockReference>(editor, cx, |e, _, cx| copy_block(e, false, cx));
        register::<CopyBlockEmbed>(editor, cx, |e, _, cx| copy_block(e, true, cx));
    })
    .detach();
    let bold = if cfg!(target_os = "macos") {
        "cmd-b"
    } else {
        "ctrl-b"
    };
    cx.bind_keys([
        // Vim's Ctrl-B scrolls a page up outside Insert mode.
        KeyBinding::new(
            bold,
            ToggleBold,
            Some(if cfg!(target_os = "macos") {
                "Editor && !QuireControl"
            } else {
                "Editor && !QuireControl && !VimControl"
            }),
        ),
        KeyBinding::new(
            "shift-f10",
            editor::actions::OpenContextMenu,
            Some("Editor && !QuireControl"),
        ),
    ]);
}

/// Makes `editor` open the page menu on a right-click or Shift-F10.
pub fn install(editor: &mut Editor) {
    editor
        .set_custom_context_menu(|editor, point, window, cx| Some(menu(editor, point, window, cx)));
}

fn apply_mark(editor: &mut Editor, mark: usize, window: &mut Window, cx: &mut Context<Editor>) {
    if editor.read_only(cx) {
        return;
    }
    let (_, open, close) = MARKS[mark];
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let selection = editor
        .selections
        .newest::<MultiBufferOffset>(&editor.display_snapshot(cx));
    let edit = toggle_mark(
        &snapshot.text(),
        selection.start.0,
        selection.end.0,
        open,
        close,
    );
    editor.transact(window, cx, |editor, window, cx| {
        editor.edit(
            edit.changes.into_iter().map(|(range, text)| {
                (
                    MultiBufferOffset(range.start)..MultiBufferOffset(range.end),
                    text,
                )
            }),
            cx,
        );
        editor.change_selections(SelectionEffects::default(), window, cx, |selections| {
            selections.select_ranges([
                MultiBufferOffset(edit.selection.start)..MultiBufferOffset(edit.selection.end)
            ])
        });
    });
}

/// The list item at the caret, with the buffer it lives in.
fn caret_block(
    editor: &Editor,
    cx: &App,
) -> Option<(Entity<Buffer>, crate::markdown_blocks::Block)> {
    let multi = editor.buffer().read(cx);
    let snapshot = multi.snapshot(cx);
    let head = editor
        .selections
        .newest_anchor()
        .head()
        .to_offset(&snapshot);
    let (buffer, at) = snapshot.point_to_buffer_offset(head)?;
    let entity = multi.buffer(buffer.remote_id())?;
    let block = crate::markdown_blocks::current_blocks(buffer)?
        .into_iter()
        .find(|block| block.kind == "list" && block.from <= at.0 && at.0 <= block.content_to)?;
    Some((entity, block))
}

fn copy_block(editor: &mut Editor, embed: bool, cx: &mut Context<Editor>) {
    let Some((buffer, block)) = caret_block(editor, cx) else {
        crate::outline::show_notice(NOT_IN_BLOCK, cx);
        return;
    };
    let id = match block.id {
        Some(id) => id,
        None if editor.read_only(cx) => {
            crate::outline::show_notice(READ_ONLY, cx);
            return;
        }
        None => {
            let id = crate::references::new_block_id();
            let at = block.content_to;
            let gap = if block.content_from == at { "" } else { " " };
            buffer.update(cx, |buffer, cx| {
                buffer.edit([(at..at, format!("{gap}^{id}"))], None, cx)
            });
            id
        }
    };
    let text = if embed {
        format!("{{{{embed: (({id}))}}}}")
    } else {
        format!("(({id}))")
    };
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(text));
}

/// The menu's entries for the caret and selection, as the reference `pageMenu` builds them.
fn entries(editor: &Editor, cx: &mut App) -> Vec<Entry> {
    let selection = editor
        .selections
        .newest::<MultiBufferOffset>(&editor.display_snapshot(cx));
    let empty = selection.is_empty();
    let editable = !editor.read_only(cx);
    let block = caret_block(editor, cx);
    let lists = editable
        && block.is_some()
        && crate::editor_projection::enabled(editor)
        && editor.buffer().read(cx).as_singleton().is_some();
    let clipboard = vec![
        Item::action("Cut", editor::actions::Cut)
            .disabled(empty || !editable)
            .done(),
        Item::action("Copy", editor::actions::Copy)
            .disabled(empty)
            .done(),
        Item::action("Paste", editor::actions::Paste)
            .disabled(!editable)
            .done(),
        Item::action("Select all", editor::actions::SelectAll).done(),
    ];
    let marks: Vec<Entry> = if !empty && editable {
        let actions: [Box<dyn gpui::Action>; 5] = [
            Box::new(ToggleBold),
            Box::new(ToggleItalic),
            Box::new(ToggleStrikethrough),
            Box::new(ToggleCode),
            Box::new(ToggleLinkToPage),
        ];
        std::iter::once(Entry::Separator)
            .chain(
                MARKS
                    .iter()
                    .zip(actions)
                    .map(|((label, _, _), action)| Item::boxed(*label, action).done()),
            )
            .collect()
    } else {
        Vec::new()
    };
    let block_items = block
        .as_ref()
        .map(|(buffer, block)| {
            let snapshot = buffer.read(cx).snapshot();
            let numbered = snapshot
                .text_for_range(block.from..block.content_to)
                .collect::<String>()
                .trim_start()
                .starts_with(|c: char| c.is_ascii_digit());
            let text = crate::markdown_blocks::block_text(&snapshot, block);
            let task = crate::task_marker::read(&text).map(|marker| marker.checked);
            let style = if lists {
                vec![
                    Entry::Separator,
                    Item::action("Bullet list", crate::outline::BulletList)
                        .checked(!numbered)
                        .done(),
                    Item::action("Numbered list", crate::outline::NumberList)
                        .checked(numbered)
                        .done(),
                ]
            } else {
                vec![Entry::Separator]
            };
            let task = editable.then(|| {
                let label = match task {
                    None => "Turn into task",
                    Some(true) => "Reopen task",
                    Some(false) => "Mark task done",
                };
                Item::action(label, crate::tasks::ToggleTask).done()
            });
            let copy = !editable && block.id.is_none();
            [
                style,
                task.into_iter().collect(),
                vec![
                    Entry::Separator,
                    Item::action("Copy block reference", CopyBlockReference)
                        .disabled(copy)
                        .done(),
                    Item::action("Copy block embed", CopyBlockEmbed)
                        .disabled(copy)
                        .done(),
                ],
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    [clipboard, marks, block_items]
        .into_iter()
        .flatten()
        .collect()
}

fn menu(
    editor: &mut Editor,
    point: DisplayPoint,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Entity<ContextMenu> {
    // A right-click outside the selection moves the caret there first; inside it keeps it.
    let display = editor.display_snapshot(cx);
    let selection = editor.selections.newest_display(&display);
    let (start, end) = if selection.start <= selection.end {
        (selection.start, selection.end)
    } else {
        (selection.end, selection.start)
    };
    if point < start || point > end {
        editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
            selections.select_display_ranges([point..point])
        });
    }
    let entries = pointer_menu::tidy(entries(editor, cx));
    #[cfg(test)]
    let summary = pointer_menu::summary(&entries);
    let menu = pointer_menu::build(entries, Some(editor.focus_handle(cx)), window, cx);
    #[cfg(test)]
    cx.set_global(LastMenu(summary, menu.downgrade()));
    menu
}

/// The last page menu built and its entries as text, for tests.
#[cfg(test)]
pub struct LastMenu(pub Vec<String>, pub gpui::WeakEntity<ContextMenu>);
#[cfg(test)]
impl gpui::Global for LastMenu {}

#[cfg(test)]
mod tests {
    use super::*;

    /// Applies the edit and returns the text with what ends up selected (rt
    /// `tests/page-menu.test.ts` `apply`).
    fn apply(doc: &str, from: usize, to: usize, open: &str, close: &str) -> (String, String) {
        let edit = toggle_mark(doc, from, to, open, close);
        let mut changes = edit.changes;
        changes.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
        let text = changes
            .into_iter()
            .fold(doc.to_owned(), |mut text, (range, insert)| {
                text.replace_range(range, &insert);
                text
            });
        let selected = text[edit.selection].to_owned();
        (text, selected)
    }

    #[test]
    fn wraps_a_selection_in_a_mark_and_keeps_the_words_selected() {
        assert_eq!(
            apply("say hello now", 4, 9, "**", "**"),
            ("say **hello** now".into(), "hello".into())
        );
        assert_eq!(
            apply("say hello now", 4, 9, "[[", "]]"),
            ("say [[hello]] now".into(), "hello".into())
        );
    }

    #[test]
    fn takes_a_mark_off_whether_it_is_selected_or_just_outside() {
        assert_eq!(
            apply("say **hello** now", 4, 13, "**", "**"),
            ("say hello now".into(), "hello".into())
        );
        assert_eq!(
            apply("say **hello** now", 6, 11, "**", "**"),
            ("say hello now".into(), "hello".into())
        );
        assert_eq!(
            apply("a `code` b", 3, 7, "`", "`"),
            ("a code b".into(), "code".into())
        );
    }

    #[test]
    fn leaves_an_empty_pair_with_the_caret_between_for_an_empty_selection() {
        assert_eq!(toggle_mark("ab", 1, 1, "_", "_").selection, 2..2);
        assert_eq!(apply("ab", 1, 1, "_", "_").0, "a__b");
        // An empty selection between a pair of identical marks gets another pair.
        assert_eq!(apply("a__b", 2, 2, "_", "_").0, "a____b");
        // At the start of the text there is nothing before to take off.
        assert_eq!(apply("**x", 0, 0, "**", "**").0, "******x");
    }
}
