//! Completion after `[[` (page names) and `((` (blocks) through Zed's completion menu. Choosing
//! a block that has no ID writes a new `^b-…` ID into its page's shared buffer, as the
//! reference `ensureId` does, so that page's own save stream persists it.

use super::{Notebook, ReferenceHost};
use crate::query_index::ask;
use editor::{CompletionProvider, Editor};
use gpui::{App, Context, Entity, KeyBinding, Task, WeakEntity, Window};
use language::{Buffer, CodeLabel};
use project::{
    Completion, CompletionDisplayOptions, CompletionIntent, CompletionResponse, CompletionSource,
    lsp_store::CompletionDocumentation,
};
use std::{ops::Range, rc::Rc, sync::Arc};
use text::ToOffset;

/// How many blocks one `((` query offers.
const BLOCK_LIMIT: usize = 50;
const LABEL_CHARS: usize = 90;

const NO_INDEX: &str = "Link suggestions need the notebook index, which is still starting. Keep typing or retry in a moment.";
const CHANGED: &str = "The source block changed while suggestions were open. Reopen the suggestions and choose it again.";
const NOT_READY: &str = "The source page is still being read. Choose the block again in a moment.";

pub(super) fn bind_keys(cx: &mut App) {
    const MENU: &str = "Editor && showing_completions";
    cx.bind_keys([
        KeyBinding::new(
            "enter",
            editor::actions::ConfirmCompletion::default(),
            Some(MENU),
        ),
        KeyBinding::new(
            "tab",
            editor::actions::ConfirmCompletion::default(),
            Some(MENU),
        ),
        KeyBinding::new("up", editor::actions::ContextMenuPrevious, Some(MENU)),
        KeyBinding::new("down", editor::actions::ContextMenuNext, Some(MENU)),
        KeyBinding::new("ctrl-p", editor::actions::ContextMenuPrevious, Some(MENU)),
        KeyBinding::new("ctrl-n", editor::actions::ContextMenuNext, Some(MENU)),
        KeyBinding::new(
            "escape",
            editor::actions::Cancel,
            Some("Editor && showing_completions && !VimControl"),
        ),
    ]);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Opening {
    Page,
    Block,
}

/// The unclosed `[[` or `((` before the caret on its line, with the offset after it.
fn opening(prefix: &str) -> Option<(Opening, usize)> {
    let page = prefix
        .rfind("[[")
        .filter(|at| !prefix[at + 2..].contains(']'));
    let block = prefix
        .rfind("((")
        .filter(|at| !prefix[at + 2..].contains(')'));
    match (page, block) {
        (Some(page), Some(block)) if block > page => Some((Opening::Block, block + 2)),
        (Some(page), _) => Some((Opening::Page, page + 2)),
        (None, Some(block)) => Some((Opening::Block, block + 2)),
        (None, None) => None,
    }
}

/// Where a typed reference starts and how far its replacement reaches: up to two closing
/// characters already after the caret, which automatic brackets add.
struct Typed {
    kind: Opening,
    query: String,
    from: usize,
    to: usize,
}

fn typed(snapshot: &text::BufferSnapshot, offset: usize) -> Option<Typed> {
    let row = snapshot.offset_to_point(offset).row;
    let line_start = snapshot.point_to_offset(text::Point::new(row, 0));
    let prefix: String = snapshot.text_for_range(line_start..offset).collect();
    let (kind, after) = opening(&prefix)?;
    let close = match kind {
        Opening::Page => ']',
        Opening::Block => ')',
    };
    let closing = snapshot
        .chars_at(offset)
        .take(2)
        .take_while(|c| *c == close)
        .count();
    Some(Typed {
        kind,
        query: prefix[after..].to_owned(),
        from: line_start + after,
        to: offset + closing,
    })
}

fn new_id() -> String {
    format!("b-{}", uuid::Uuid::new_v4().simple())
}

fn plain_label(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| !matches!(c, '*' | '`' | '#'))
        .collect();
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    match cleaned.char_indices().nth(LABEL_CHARS) {
        Some((end, _)) => format!("{}…", &cleaned[..end]),
        None => cleaned,
    }
}

fn completion(
    range: Range<text::Anchor>,
    match_start: text::Anchor,
    label: String,
    detail: String,
    new_text: String,
    confirm: Option<Confirm>,
) -> Completion {
    Completion {
        replace_range: range,
        new_text,
        label: CodeLabel::plain(label, None),
        documentation: Some(CompletionDocumentation::SingleLine(detail.into())),
        source: CompletionSource::Custom,
        icon_path: None,
        icon_color: None,
        match_start: Some(match_start),
        snippet_deduplication_key: None,
        insert_text_mode: None,
        confirm,
        group: None,
    }
}

/// The block a `((` choice names, as indexed when suggestions opened.
#[derive(Clone)]
struct Chosen {
    page: String,
    key: String,
    text: String,
    id: String,
}

pub(super) struct References {
    host: Entity<ReferenceHost>,
}

impl References {
    pub fn new(host: Entity<ReferenceHost>) -> Self {
        Self { host }
    }
}

impl CompletionProvider for References {
    fn completions(
        &self,
        buffer: &Entity<Buffer>,
        buffer_position: text::Anchor,
        _: editor::CompletionContext,
        window: &mut Window,
        cx: &mut Context<Editor>,
    ) -> Task<anyhow::Result<Vec<CompletionResponse>>> {
        let page = buffer.read(cx).snapshot();
        let caret = buffer_position.to_offset(&page);
        if let Some(after) = crate::templates::insert::trigger_at(&page, caret) {
            return crate::templates::insert::completions(
                &self.host, buffer, after, caret, window, cx,
            );
        }
        let snapshot = buffer.read(cx).text_snapshot();
        let Some(typed) = typed(&snapshot, buffer_position.to_offset(&snapshot)) else {
            return Task::ready(Ok(Vec::new()));
        };
        let notebook = self.host.read(cx).notebook().clone();
        let Some(index) = self.host.read(cx).index().cloned() else {
            crate::outline::set_notice(Some(NO_INDEX), cx);
            return Task::ready(Ok(Vec::new()));
        };
        let range = snapshot.anchor_before(typed.from)..snapshot.anchor_after(typed.to);
        let match_start = snapshot.anchor_before(typed.from);
        let host = self.host.downgrade();
        let current = notebook.page_id(buffer, cx);
        cx.spawn(async move |_, _| {
            let completions = match typed.kind {
                Opening::Page => ask(index.pages())
                    .await
                    .map_err(anyhow::Error::msg)?
                    .into_iter()
                    .map(|listed| {
                        let title = listed.page.title;
                        completion(
                            range.clone(),
                            match_start,
                            title.clone(),
                            listed.page.path,
                            format!("{title}]]"),
                            None,
                        )
                    })
                    .collect(),
                Opening::Block => {
                    let typing = format!("(({}", typed.query);
                    ask(index.block_choices(typed.query.clone(), BLOCK_LIMIT))
                        .await
                        .map_err(anyhow::Error::msg)?
                        .into_iter()
                        .filter(|choice| {
                            current.as_deref() != choice.source.page.as_str()
                                || !choice.text.contains(&typing)
                        })
                        .map(|choice| {
                            let (id, confirm) = match choice.id {
                                Some(id) => (id, None),
                                None => {
                                    let chosen = Chosen {
                                        page: choice.source.page.to_lossy_string(),
                                        key: choice.key,
                                        text: choice.text.clone(),
                                        id: new_id(),
                                    };
                                    (chosen.id.clone(), Some(write_on_confirm(&host, chosen)))
                                }
                            };
                            completion(
                                range.clone(),
                                match_start,
                                plain_label(&choice.text),
                                choice.source.title,
                                format!("{id}))"),
                                confirm,
                            )
                        })
                        .collect()
                }
            };
            Ok(vec![CompletionResponse {
                completions,
                display_options: CompletionDisplayOptions::default(),
                is_incomplete: typed.kind == Opening::Block,
            }])
        })
    }

    fn is_completion_trigger(
        &self,
        buffer: &Entity<Buffer>,
        position: language::Anchor,
        text: &str,
        _: bool,
        cx: &mut Context<Editor>,
    ) -> bool {
        if text.contains('\n') {
            return false;
        }
        let page = buffer.read(cx).snapshot();
        let caret = position.to_offset(&page);
        if crate::templates::insert::trigger_at(&page, caret).is_some() {
            return true;
        }
        let snapshot = buffer.read(cx).text_snapshot();
        typed(&snapshot, caret).is_some()
    }
}

type Confirm = Arc<dyn Send + Sync + Fn(CompletionIntent, &mut Window, &mut App) -> bool>;

/// Writes the planned ID into the chosen block's page once the reference is inserted.
fn write_on_confirm(host: &WeakEntity<ReferenceHost>, chosen: Chosen) -> Confirm {
    let host = host.clone();
    Arc::new(move |_, window, cx| {
        let Some(host) = host.upgrade() else {
            return false;
        };
        let notebook: Rc<dyn Notebook> = host.read(cx).notebook().clone();
        let Some(page) = notebook.open_page(&chosen.page, window, cx) else {
            crate::outline::set_notice(Some(CHANGED), cx);
            return false;
        };
        let chosen = chosen.clone();
        super::when_loaded(
            page,
            move |buffer, cx| {
                if let Err(message) = write_id(&buffer, &chosen, cx) {
                    crate::outline::set_notice(Some(message), cx);
                }
            },
            cx,
        );
        false
    })
}

/// Appends ` ^id` to the chosen block, refusing if the block changed since it was offered.
fn write_id(buffer: &Entity<Buffer>, chosen: &Chosen, cx: &mut App) -> Result<(), &'static str> {
    let snapshot = buffer.read(cx).snapshot();
    let blocks = crate::markdown_blocks::current_blocks(&snapshot).ok_or(NOT_READY)?;
    let block = blocks
        .iter()
        .find(|block| {
            block.key == chosen.key
                && crate::markdown_blocks::block_text(&snapshot, block) == chosen.text
        })
        .ok_or(CHANGED)?;
    if block.id.is_some() {
        return Err(CHANGED);
    }
    let at = block.content_to;
    let gap = if block.content_from == at { "" } else { " " };
    let text = format!("{gap}^{}", chosen.id);
    buffer.update(cx, |buffer, cx| buffer.edit([(at..at, text)], None, cx));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openings_find_the_unclosed_reference_before_the_caret() {
        assert_eq!(opening("- see [[Pla"), Some((Opening::Page, 8)));
        assert_eq!(opening("[[Done]] and ((zeb"), Some((Opening::Block, 15)));
        assert_eq!(opening("((a)) [[b"), Some((Opening::Page, 8)));
        assert_eq!(opening("[[Done]] text"), None);
        assert_eq!(opening("((a)) text"), None);
        assert_eq!(opening("plain"), None);
    }

    #[test]
    fn block_labels_drop_markup_and_stop_at_ninety_characters() {
        assert_eq!(plain_label("**Bold** `code` #tag"), "Bold code tag");
        let long = "a".repeat(100);
        assert_eq!(plain_label(&long), format!("{}…", "a".repeat(90)));
    }
}
