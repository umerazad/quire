//! Copying or moving blocks through the clipboard, as `rt` does. Pasting into a Write view gives
//! every block ID the text carries a fresh one and points the references inside it at the
//! copies, so references to the originals stay where they were. Text this notebook cut moves
//! instead: while no open page still has its IDs, it keeps them, and every reference to the
//! moved blocks follows. Source view pastes text as written, as `rt`'s source mode does.

use super::Handled;
use editor::{ClipboardSelection, Editor};
use gpui::{
    Action, App, ClipboardEntry, ClipboardItem, Context, EntityId, Focusable as _, Global, Window,
};
use quire_core::markdown::{Addresses, fresh_ids, page_name, renamed_ids};
use std::collections::HashMap;

/// What a page editor last put on the clipboard with Copy or Cut, and where it came from.
struct Origin {
    notebook: EntityId,
    path: Option<String>,
    text: String,
    cut: bool,
}

struct Clipped(Option<Origin>);
impl Global for Clipped {}

/// Notes where each Copy and Cut in this editor came from, then lets the editor do it.
pub(super) fn record(editor: &mut Editor, cx: &mut Context<Editor>) {
    remember::<editor::actions::Copy>(editor, false, cx);
    remember::<editor::actions::Cut>(editor, true, cx);
}

fn remember<A: Action>(editor: &mut Editor, cut: bool, cx: &mut Context<Editor>) {
    let handle = cx.entity().downgrade();
    editor
        .register_action(move |_: &A, window, cx| {
            cx.propagate();
            let Some(editor) = handle.upgrade() else {
                return;
            };
            if !editor.focus_handle(cx).is_focused(window) {
                return;
            }
            let Some((notebook, path)) = place(editor.read(cx), cx) else {
                return;
            };
            cx.defer(move |cx| {
                let text = cx.read_from_clipboard().and_then(|item| item.text());
                cx.set_global(Clipped(text.map(|text| Origin {
                    notebook,
                    path,
                    text,
                    cut,
                })));
            });
        })
        .detach();
}

/// The notebook an editor's page belongs to, and the page's path when the notebook has it open.
fn place(editor: &Editor, cx: &App) -> Option<(EntityId, Option<String>)> {
    let host = crate::references::host(editor)?;
    let buffer = super::source_buffer(editor, cx)?;
    let path = host.read(cx).notebook().page_path(&buffer, cx);
    Some((host.entity_id(), path))
}

/// Pastes the clipboard with its blocks copied when it carries block IDs and does not move
/// them; otherwise the editor pastes it as it is.
pub(super) fn paste(
    editor: &mut Editor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Handled> {
    let item = cx.read_from_clipboard()?;
    if item
        .entries()
        .iter()
        .any(|entry| matches!(entry, ClipboardEntry::Image(_)))
    {
        return None;
    }
    let copy = fresh_copy(editor, &item.text()?, cx)?;
    editor.paste_item(&copied(&item, &copy)?, window, cx);
    Some(Handled::Done)
}

/// The clipboard text as a paste into `editor` inserts it.
pub(super) fn pasted_text(editor: &Editor, text: String, cx: &App) -> String {
    match fresh_copy(editor, &text, cx) {
        Some(copy) => copy.rename(&text),
        None => text,
    }
}

/// How pasted text becomes a copy: the fresh ID for each block ID it carries, and how its
/// `Page#^id` references to its own blocks name the destination.
struct FreshCopy {
    ids: HashMap<String, String>,
    destination: String,
    sources: Vec<String>,
}

impl FreshCopy {
    fn rename(&self, text: &str) -> String {
        let sources: Vec<&str> = self.sources.iter().map(String::as_str).collect();
        renamed_ids(
            text,
            &Addresses::scan(text),
            &self.ids,
            &self.destination,
            &sources,
        )
    }
}

/// How `text` pasted into `editor` becomes a copy, or None when it carries no block IDs, the
/// view is Source, or it is this notebook's cut whose IDs no open page has.
fn fresh_copy(editor: &Editor, text: &str, cx: &App) -> Option<FreshCopy> {
    if !crate::editor_projection::enabled(editor) {
        return None;
    }
    let addresses = Addresses::scan(text);
    if addresses.ids.is_empty() {
        return None;
    }
    let host = crate::references::host(editor);
    let notebook = host.as_ref().map(|host| host.entity_id());
    let origin = cx
        .try_global::<Clipped>()
        .and_then(|clipped| clipped.0.as_ref())
        .filter(|origin| origin.text == text && Some(origin.notebook) == notebook);
    if origin.is_some_and(|origin| origin.cut) {
        let open: Vec<String> = super::fresh_ids::open_pages(editor, cx)
            .into_iter()
            .map(|(_, page)| page.read(cx).text())
            .collect();
        let held = addresses.ids.iter().any(|range| {
            let id = &text[range.clone()];
            open.iter().any(|page| super::fresh_ids::holds(page, id))
        });
        if !held {
            return None;
        }
    }
    let destination = host
        .zip(super::source_buffer(editor, cx))
        .and_then(|(host, buffer)| host.read(cx).notebook().page_path(&buffer, cx));
    let sources = origin
        .and_then(|origin| origin.path.as_deref())
        .filter(|_| destination.is_some())
        .map(|path| {
            let name = page_name(path);
            let base = name.rsplit('/').next().unwrap_or(name);
            vec![name.to_owned(), base.to_owned()]
        })
        .unwrap_or_default();
    Some(FreshCopy {
        ids: fresh_ids(text, &addresses, crate::references::new_block_id),
        destination: destination.unwrap_or_default(),
        sources,
    })
}

/// The clipboard item with its text copied. Zed's copies carry each selection's length, so
/// each selection's text is copied on its own and its length follows the copy.
fn copied(item: &ClipboardItem, copy: &FreshCopy) -> Option<ClipboardItem> {
    let whole = || ClipboardItem::new_string(copy.rename(&item.text().unwrap_or_default()));
    let [ClipboardEntry::String(string)] = item.entries() else {
        return Some(whole());
    };
    let text = string.text();
    let Some(selections) = string.metadata_json::<Vec<ClipboardSelection>>() else {
        return Some(whole());
    };
    let mut start = 0;
    let mut pasted = String::new();
    let mut lengths = Vec::new();
    for selection in selections {
        let end = start + selection.len;
        let next = if selection.is_entire_line {
            end
        } else {
            (end + 1).min(text.len())
        };
        let (Some(chunk), Some(gap)) = (text.get(start..end), text.get(end..next)) else {
            return Some(whole());
        };
        let chunk = copy.rename(chunk);
        pasted.push_str(&chunk);
        pasted.push_str(gap);
        lengths.push(ClipboardSelection {
            len: chunk.len(),
            ..selection
        });
        start = next;
    }
    pasted.push_str(text.get(start..)?);
    Some(ClipboardItem::new_string_with_json_metadata(
        pasted, lengths,
    ))
}
