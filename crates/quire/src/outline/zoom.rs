//! Zoom: a Write view of one list item and its children. The view is an excerpt of the page's
//! buffer bound to the item's ID, so it shares the page's text and undo history, follows the
//! item as the page changes, and cannot edit outside it. Outline commands treat the item as
//! the root: Enter at its end makes a child and outdenting never lifts a block out of it.

use crate::page_analysis::PageAnalysis;
use crate::semantic_excerpt::SemanticExcerpt;
use editor::{Editor, SelectionEffects};
use gpui::{App, AppContext as _, Entity, Subscription, Window};
use multi_buffer::ToOffset as _;
use std::{cell::Cell, rc::Rc};

pub const NOT_IN_BLOCK: &str = "Place the caret in a list item, then zoom into it.";
pub const READ_ONLY: &str =
    "This page is read-only and the item has no ID to zoom into. Reopen the page, then try again.";
pub const SOURCE: &str = "Zoom works in Write view. Switch to Write, then zoom into the item.";

pub struct Zoom {
    pub editor: Entity<Editor>,
    model: Entity<SemanticExcerpt>,
    _caret: Subscription,
}

/// The list item holding the caret of `editor`, in source offsets, with the caret.
fn caret_item(
    editor: &Editor,
    cx: &App,
) -> Option<(
    Entity<language::Buffer>,
    crate::markdown_blocks::Block,
    usize,
)> {
    let buffer = super::source_buffer(editor, cx)?;
    let multi = editor.buffer().read(cx).snapshot(cx);
    let head = editor.selections.newest_anchor().head().to_offset(&multi);
    let (_, head) = multi.point_to_buffer_offset(head)?;
    let head = head.0;
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let block = crate::markdown_blocks::current_blocks(&snapshot)?
        .into_iter()
        .rfind(|block| {
            block.kind == "list"
                && super::model::line_start(&text, block.from) <= head
                && head <= block.content_to
        })?;
    Some((buffer, block, head))
}

impl Zoom {
    /// Zooms into the list item at the caret of `from`, a page editor or another zoom, giving
    /// the item an ID when it has none. The ID is its own undo step in `from`.
    pub fn open(
        from: &Entity<Editor>,
        analysis: &Entity<PageAnalysis>,
        window: &mut Window,
        cx: &mut App,
    ) -> Result<Self, &'static str> {
        if !crate::editor_projection::enabled(from.read(cx)) {
            return Err(SOURCE);
        }
        let (buffer, block, head) = caret_item(from.read(cx), cx).ok_or(NOT_IN_BLOCK)?;
        let id = match block.id {
            Some(id) => id,
            None if from.read(cx).read_only(cx) => return Err(READ_ONLY),
            None => {
                let id = crate::references::new_block_id();
                let at = block.content_to;
                let gap = if block.content_from == at { "" } else { " " };
                from.update(cx, |editor, cx| {
                    editor.finalize_last_transaction(cx);
                    editor.transact(window, cx, |_, _, cx| {
                        buffer.update(cx, |buffer, cx| {
                            buffer.edit([(at..at, format!("{gap}^{id}"))], None, cx)
                        });
                    });
                    editor.finalize_last_transaction(cx);
                });
                id
            }
        };
        let caret = buffer.read(cx).snapshot().anchor_before(head);
        let model = cx.new(|cx| SemanticExcerpt::new(analysis.clone(), id, cx));
        let excerpt = model.read(cx).buffer.clone();
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_multibuffer(excerpt, None, window, cx);
            crate::page_menu::install(&mut editor);
            editor
        });
        crate::semantic_excerpt::attach(&editor, &model, window, cx);
        if let Some(host) = crate::references::host(from.read(cx)) {
            crate::references::install(&editor, &host, window, cx);
        }
        crate::editor_projection::bind(&editor, window, cx).map_err(|_| SOURCE)?;
        let pending = Rc::new(Cell::new(Some(caret)));
        let place = {
            let pending = pending.clone();
            move |editor: &mut Editor, window: &mut Window, cx: &mut gpui::Context<Editor>| {
                if crate::semantic_excerpt::root(editor, cx).is_none() {
                    return;
                }
                let Some(caret) = pending.take() else {
                    return;
                };
                let multi = editor.buffer().read(cx).snapshot(cx);
                if let Some(caret) = multi.anchor_in_excerpt(caret) {
                    editor.change_selections(SelectionEffects::default(), window, cx, |s| {
                        s.select_anchor_ranges([caret..caret])
                    });
                }
            }
        };
        let caret = editor.update(cx, |editor, cx| {
            place(editor, window, cx);
            cx.observe_in(&model, window, move |editor, _, window, cx| {
                place(editor, window, cx)
            })
        });
        Ok(Self {
            editor,
            model,
            _caret: caret,
        })
    }

    /// Why the zoomed item cannot be shown, when it cannot.
    pub fn message(&self, cx: &App) -> Option<String> {
        self.model.read(cx).message()
    }

    /// The zoom's caret as a source offset, to keep the place when zooming out.
    pub fn caret(&self, cx: &App) -> Option<usize> {
        let editor = self.editor.read(cx);
        let multi = editor.buffer().read(cx).snapshot(cx);
        let head = editor.selections.newest_anchor().head().to_offset(&multi);
        multi.point_to_buffer_offset(head).map(|(_, at)| at.0)
    }
}
