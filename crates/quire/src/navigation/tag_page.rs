//! A tag's page: every block using the tag, with its parents and nested blocks, grouped by
//! page and kept current as pages change.

use super::{
    context_rows::{ContextItem, ContextRow, context_rows},
    list_keys,
    live::Live,
    tags::blocks,
};
use editor::{Editor, EditorEvent};
use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    Subscription, UniformListScrollHandle, Window, uniform_list,
};
use quire_core::workspace::{TagChoices, TaskStatus};
use quire_store::index::{IndexClient, Outline, TaggedBlock};
use ui::{Color, Icon, IconName, Label, LabelCommon, LabelSize, prelude::*, v_flex};

pub enum TagPageEvent {
    Open {
        page: String,
        jump: Option<super::Jump>,
        beside: bool,
    },
    /// The filter text changed and should be remembered.
    Remember,
}

pub type Tagged = Vec<(TaggedBlock, Outline)>;

/// The reference tag page filter: blocks whose page title or text contains the filter, laid
/// out by page.
pub fn tagged_rows(blocks: &[(TaggedBlock, Outline)], filter: &str) -> Vec<ContextRow> {
    let needle = filter.trim().to_lowercase();
    context_rows(
        blocks
            .iter()
            .filter(|(block, _)| {
                format!("{}\n{}", block.source.title, block.tag.snippet)
                    .to_lowercase()
                    .contains(&needle)
            })
            .map(|(block, outline)| ContextItem {
                page: block.source.page.to_lossy_string(),
                title: &block.source.title,
                text: &block.tag.snippet,
                outline,
                jump: super::Jump::at(block.tag.block_id.clone(), block.tag.from),
            }),
    )
}

pub struct TagPageView {
    tag: String,
    pub(crate) blocks: Live<Tagged>,
    filter: Entity<Editor>,
    status: TaskStatus,
    highlighted: usize,
    focus: FocusHandle,
    pub(crate) scroll: UniformListScrollHandle,
    /// The remembered first row, scrolled to once the rows arrive.
    restore: Option<usize>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TagPageEvent> for TagPageView {}

impl Focusable for TagPageView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.focus_handle(cx)
    }
}

impl TagPageView {
    pub fn new(
        tag: String,
        choices: TagChoices,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Filter by block text or page…", window, cx);
            editor.set_text(choices.query.as_str(), window, cx);
            editor
        });
        let subscriptions = vec![cx.subscribe(&filter, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.highlighted = 0;
                cx.emit(TagPageEvent::Remember);
                cx.notify();
            }
        })];
        Self {
            tag,
            blocks: Live::default(),
            filter,
            status: choices.status,
            highlighted: 0,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            restore: (choices.scroll > 0).then_some(choices.scroll),
            _subscriptions: subscriptions,
        }
    }

    /// Asks for the tagged blocks and their outlines in one index step.
    pub fn refresh(&mut self, client: &IndexClient, cx: &mut Context<Self>) {
        let tag = self.tag.clone();
        let answer = client.with_generation(move |index| {
            let blocks = index.tagged_blocks(&tag)?;
            let at: Vec<_> = blocks
                .iter()
                .map(|block| (block.source.revision.clone(), block.tag.from))
                .collect();
            let outlines = index.outlines(&at)?;
            Ok(blocks.into_iter().zip(outlines).collect())
        });
        self.blocks.request(
            answer,
            cx,
            |this| &mut this.blocks,
            |error| {
                format!(
                    "Cannot list tagged blocks: {error}. Wait for indexing to finish, then retry."
                )
            },
        );
    }

    /// What to remember of this page: the filter, task status, and first row shown.
    pub fn choices(&self, cx: &App) -> TagChoices {
        let shown = self.rows(cx).len();
        TagChoices {
            query: self.filter.read(cx).text(cx),
            status: self.status,
            scroll: self
                .restore
                .unwrap_or_else(|| super::first_shown(&self.scroll, shown)),
        }
    }

    pub fn rows(&self, cx: &App) -> Vec<ContextRow> {
        tagged_rows(&self.blocks.value, &self.filter.read(cx).text(cx))
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.rows(cx).len();
        if count == 0 {
            return;
        }
        let next = (self.highlighted as isize)
            .saturating_add(delta)
            .clamp(0, count as isize - 1);
        self.highlighted = next as usize;
        self.scroll
            .scroll_to_item(self.highlighted, ScrollStrategy::Top);
        cx.notify();
    }

    fn open(&mut self, index: usize, beside: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.rows(cx).get(index) {
            self.highlighted = index;
            cx.emit(TagPageEvent::Open {
                page: row.page().to_owned(),
                jump: row.jump().cloned(),
                beside,
            });
        }
    }
}

impl Render for TagPageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        if !rows.is_empty()
            && let Some(row) = self.restore.take()
        {
            self.scroll
                .scroll_to_item_strict(row.min(rows.len() - 1), ScrollStrategy::Top);
        }
        let total = self.blocks.value.len();
        let pages = self
            .blocks
            .value
            .iter()
            .map(|(block, _)| &block.source.revision)
            .collect::<std::collections::HashSet<_>>()
            .len();
        let empty = rows.is_empty().then_some(if total == 0 {
            "No blocks use this tag yet. Add it to a note and it will appear here."
        } else {
            "No matching blocks. Try another word or page name."
        });
        let list = uniform_list(
            "tag-page-rows",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let item = rows.get(index)?.item(
                            index,
                            index == this.highlighted,
                            ("match", "matches"),
                        );
                        Some(
                            item.on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.open(index, event.modifiers().shift, cx)
                            })),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1();
        let root = v_flex()
            .id("tag-page")
            .key_context("QuireList")
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2();
        list_keys(
            root,
            cx,
            |this, delta, cx| this.move_highlight(delta, cx),
            |this, beside, _, cx| {
                let index = this.highlighted;
                this.open(index, beside, cx)
            },
        )
        .child(
            h_flex()
                .gap_2()
                .child(Icon::new(IconName::Hash).color(Color::Muted))
                .child(Label::new(self.tag.clone())),
        )
        .child(
            Label::new(format!(
                "{} across {}",
                blocks(total),
                match pages {
                    1 => "1 page".to_owned(),
                    pages => format!("{pages} pages"),
                }
            ))
            .size(LabelSize::Small)
            .color(Color::Muted),
        )
        .child(
            div()
                .border_1()
                .border_color(cx.theme().colors().border)
                .p_1()
                .child(self.filter.clone()),
        )
        .when_some(self.blocks.error.clone(), |div, error| {
            div.child(Label::new(error).color(Color::Error))
        })
        .when_some(empty, |div, empty| {
            div.child(Label::new(empty).color(Color::Muted))
        })
        .child(list)
        .child(
            Label::new("Enter opens the source page · Shift-Enter or Shift-click opens it beside")
                .size(LabelSize::Small)
                .color(Color::Muted),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_store::{
        index::{IndexedTag, PageRef},
        sync::json::JsString,
    };

    fn block(page: &str, title: &str, snippet: &str) -> (TaggedBlock, Outline) {
        let block = TaggedBlock {
            source: PageRef {
                page: JsString::from(page),
                revision: format!("r-{page}"),
                path: format!("{page}.md"),
                title: title.into(),
            },
            tag: IndexedTag {
                tag: "#plan".into(),
                from: 0,
                block_id: None,
                snippet: snippet.into(),
            },
        };
        (block, Outline::default())
    }

    #[test]
    fn tag_rows_filter_by_block_text_or_page_title() {
        let blocks = [
            block("a", "Alpha", "first #plan"),
            block("a", "Alpha", "second #plan"),
            block("b", "Beta", "third #plan"),
        ];
        assert_eq!(tagged_rows(&blocks, "").len(), 5);
        assert_eq!(
            tagged_rows(&blocks, " BETA "),
            [
                ContextRow::Page {
                    page: "b".into(),
                    title: "Beta".into(),
                    matches: 1
                },
                ContextRow::Block {
                    page: "b".into(),
                    jump: crate::navigation::Jump::at(None, 0),
                    text: "third #plan".into()
                },
            ],
            "the page title matches"
        );
        assert_eq!(tagged_rows(&blocks, "second").len(), 2);
        assert!(tagged_rows(&blocks, "missing").is_empty());
    }
}
