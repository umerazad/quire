//! A tag's page: every block using the tag, grouped by page and
//! kept current as pages change.

use super::{list_keys, live::Live, tags::blocks};
use editor::{Editor, EditorEvent};
use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    Subscription, UniformListScrollHandle, Window, uniform_list,
};
use quire_store::index::{IndexClient, TaggedBlock};
use ui::{
    Color, Icon, IconName, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, Toggleable,
    prelude::*, v_flex,
};

pub enum TagPageEvent {
    Open { page: String, beside: bool },
}

/// One line of the grouped results: a page heading or a block under it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TagRow {
    Page {
        page: String,
        title: String,
        matches: usize,
    },
    Block {
        page: String,
        text: String,
    },
}

impl TagRow {
    pub fn page(&self) -> &str {
        match self {
            Self::Page { page, .. } | Self::Block { page, .. } => page,
        }
    }
}

/// Groups blocks by page in result order, keeping blocks whose page title or text contains
/// the filter.
pub fn grouped(blocks: &[TaggedBlock], filter: &str) -> Vec<TagRow> {
    let needle = filter.trim().to_lowercase();
    let mut rows: Vec<TagRow> = Vec::new();
    let mut heading = None;
    for block in blocks.iter().filter(|block| {
        format!("{}\n{}", block.source.title, block.tag.snippet)
            .to_lowercase()
            .contains(&needle)
    }) {
        let page = block.source.page.to_lossy_string();
        if heading.as_ref().is_none_or(|(_, id)| id != &page) {
            rows.push(TagRow::Page {
                page: page.clone(),
                title: block.source.title.clone(),
                matches: 0,
            });
            heading = Some((rows.len() - 1, page.clone()));
        }
        if let Some((index, _)) = &heading
            && let TagRow::Page { matches, .. } = &mut rows[*index]
        {
            *matches += 1;
        }
        rows.push(TagRow::Block {
            page,
            text: block.tag.snippet.clone(),
        });
    }
    rows
}

pub struct TagPageView {
    tag: String,
    pub(crate) blocks: Live<Vec<TaggedBlock>>,
    filter: Entity<Editor>,
    highlighted: usize,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TagPageEvent> for TagPageView {}

impl Focusable for TagPageView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.focus_handle(cx)
    }
}

impl TagPageView {
    pub fn new(tag: String, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Filter by block text or page…", window, cx);
            editor
        });
        let subscriptions = vec![cx.subscribe(&filter, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.highlighted = 0;
                cx.notify();
            }
        })];
        Self {
            tag,
            blocks: Live::default(),
            filter,
            highlighted: 0,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn refresh(&mut self, client: &IndexClient, cx: &mut Context<Self>) {
        let tag = self.tag.clone();
        let answer = client.with_generation(move |index| index.tagged_blocks(&tag));
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

    pub fn rows(&self, cx: &App) -> Vec<TagRow> {
        grouped(&self.blocks.value, &self.filter.read(cx).text(cx))
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
                beside,
            });
        }
    }
}

impl Render for TagPageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let total = self.blocks.value.len();
        let pages = self
            .blocks
            .value
            .iter()
            .map(|block| &block.source.revision)
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
                        let item = match rows.get(index)? {
                            TagRow::Page { title, matches, .. } => {
                                ListItem::new(("tag-row", index))
                                    .spacing(ListItemSpacing::Dense)
                                    .start_slot(Icon::new(IconName::File).color(Color::Muted))
                                    .child(
                                        h_flex()
                                            .gap_2()
                                            .child(Label::new(title.clone()).truncate())
                                            .child(
                                                Label::new(match matches {
                                                    1 => "1 match".to_owned(),
                                                    count => format!("{count} matches"),
                                                })
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                            ),
                                    )
                            }
                            TagRow::Block { text, .. } => ListItem::new(("tag-row", index))
                                .spacing(ListItemSpacing::Dense)
                                .indent_level(1)
                                .child(Label::new(text.clone()).truncate()),
                        };
                        Some(
                            item.toggle_state(index == this.highlighted)
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
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

    fn block(page: &str, title: &str, snippet: &str) -> TaggedBlock {
        TaggedBlock {
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
        }
    }

    #[test]
    fn groups_blocks_under_their_page_and_filters_by_text_or_title() {
        let blocks = [
            block("a", "Alpha", "first #plan"),
            block("a", "Alpha", "second #plan"),
            block("b", "Beta", "third #plan"),
        ];
        assert_eq!(
            grouped(&blocks, ""),
            [
                TagRow::Page {
                    page: "a".into(),
                    title: "Alpha".into(),
                    matches: 2
                },
                TagRow::Block {
                    page: "a".into(),
                    text: "first #plan".into()
                },
                TagRow::Block {
                    page: "a".into(),
                    text: "second #plan".into()
                },
                TagRow::Page {
                    page: "b".into(),
                    title: "Beta".into(),
                    matches: 1
                },
                TagRow::Block {
                    page: "b".into(),
                    text: "third #plan".into()
                },
            ]
        );
        assert_eq!(
            grouped(&blocks, " BETA ").len(),
            2,
            "the page title matches"
        );
        assert_eq!(grouped(&blocks, "second").len(), 2);
        assert!(grouped(&blocks, "missing").is_empty());
    }
}
