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
use quire_store::index::{IndexClient, Outline, TaggedBlock, TaskItem};
use ui::{
    Button, ButtonStyle, Color, Icon, IconName, Label, LabelCommon, LabelSize, Toggleable,
    prelude::*, v_flex,
};

pub enum TagPageEvent {
    Open {
        page: String,
        jump: Option<super::Jump>,
        beside: bool,
    },
    /// The filter text changed and should be remembered.
    Remember,
}

/// Each tagged block with its outline, and its task when the block is one.
pub type Tagged = Vec<(TaggedBlock, Outline, Option<TaskItem>)>;

/// The reference `statusMatches`: All keeps every block; the others keep tasks in that state.
fn has_status(task: Option<&TaskItem>, status: TaskStatus, today: &str) -> bool {
    status == TaskStatus::All || task.is_some_and(|task| task.has_status(status, today))
}

/// The reference tag page filter: blocks whose page title or text contains the filter and whose
/// task matches `status`, laid out by page.
pub fn tagged_rows(
    blocks: &[(TaggedBlock, Outline, Option<TaskItem>)],
    filter: &str,
    status: TaskStatus,
    today: &str,
) -> Vec<ContextRow> {
    let needle = filter.trim().to_lowercase();
    context_rows(
        blocks
            .iter()
            .filter(|(block, _, task)| {
                has_status(task.as_ref(), status, today)
                    && format!("{}\n{}", block.source.title, block.tag.snippet)
                        .to_lowercase()
                        .contains(&needle)
            })
            .map(|(block, outline, _)| ContextItem {
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
    today: String,
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
            today: String::new(),
            highlighted: 0,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            restore: (choices.scroll > 0).then_some(choices.scroll),
            _subscriptions: subscriptions,
        }
    }

    /// Asks for the tagged blocks, their outlines, and their tasks in one index step; `today`
    /// decides which tasks are due.
    pub fn refresh(&mut self, client: &IndexClient, today: String, cx: &mut Context<Self>) {
        self.today = today;
        let tag = self.tag.clone();
        let answer = client.with_generation(move |index| {
            let blocks = index.tagged_blocks(&tag)?;
            let at: Vec<_> = blocks
                .iter()
                .map(|block| (block.source.revision.clone(), block.tag.from))
                .collect();
            let outlines = index.outlines(&at)?;
            let mut tasks = index.tagged_tasks(&tag)?;
            Ok(blocks
                .into_iter()
                .zip(outlines)
                .map(|(block, outline)| {
                    let task = tasks.remove(&(block.source.revision.clone(), block.tag.from));
                    (block, outline, task)
                })
                .collect())
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
        tagged_rows(
            &self.blocks.value,
            &self.filter.read(cx).text(cx),
            self.shown_status(),
            &self.today,
        )
    }

    fn has_tasks(&self) -> bool {
        self.blocks.value.iter().any(|(_, _, task)| task.is_some())
    }

    /// The chosen status, or All while no tagged block is a task, as the reference shows it.
    pub fn shown_status(&self) -> TaskStatus {
        if self.has_tasks() {
            self.status
        } else {
            TaskStatus::All
        }
    }

    pub fn set_status(&mut self, status: TaskStatus, cx: &mut Context<Self>) {
        self.status = status;
        self.highlighted = 0;
        cx.emit(TagPageEvent::Remember);
        cx.notify();
    }

    fn status_count(&self, status: TaskStatus) -> usize {
        self.blocks
            .value
            .iter()
            .filter(|(_, _, task)| has_status(task.as_ref(), status, &self.today))
            .count()
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
            .map(|(block, _, _)| &block.source.revision)
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
        .when(self.has_tasks(), |div| {
            let shown = self.shown_status();
            div.child(h_flex().gap_1().children(TaskStatus::ALL.map(|status| {
                Button::new(
                    SharedString::from(format!("tag-status-{status:?}")),
                    format!("{} {}", status.label(), self.status_count(status)),
                )
                .style(ButtonStyle::Subtle)
                .toggle_state(shown == status)
                .on_click(cx.listener(move |this, _, _, cx| this.set_status(status, cx)))
            })))
        })
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

    fn block(page: &str, title: &str, snippet: &str) -> (TaggedBlock, Outline, Option<TaskItem>) {
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
        (block, Outline::default(), None)
    }

    #[test]
    fn tag_rows_filter_by_block_text_or_page_title() {
        let blocks = [
            block("a", "Alpha", "first #plan"),
            block("a", "Alpha", "second #plan"),
            block("b", "Beta", "third #plan"),
        ];
        let rows = |filter: &str| tagged_rows(&blocks, filter, TaskStatus::All, "2026-10-31");
        assert_eq!(rows("").len(), 5);
        assert_eq!(
            rows(" BETA "),
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
        assert_eq!(rows("second").len(), 2);
        assert!(rows("missing").is_empty());
    }
}
