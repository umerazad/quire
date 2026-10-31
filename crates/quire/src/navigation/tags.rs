//! The Tags browser: every tag with its block count, filtered and sorted, with several tags
//! selectable for bulk rename and delete.

use super::{
    ToggleTagSelected, list_keys,
    live::Live,
    tag_edits::{TagEdits, TagEditsEvent, Writers},
};
use editor::{Editor, EditorEvent};
use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    SharedString, Subscription, UniformListScrollHandle, Window, uniform_list,
};
use quire_core::workspace::TagListChoices;
use quire_store::index::{IndexClient, TagCount};
use ui::{
    Button, Checkbox, Color, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, ToggleState,
    Toggleable, prelude::*, v_flex,
};

pub use quire_core::workspace::TagSort;

pub enum TagsEvent {
    Open(String),
    /// The filter text or order changed and should be remembered.
    Remember(TagListChoices),
}

pub struct TagsView {
    pub(crate) tags: Live<Vec<TagCount>>,
    filter: Entity<Editor>,
    sort: TagSort,
    highlighted: usize,
    selected: Vec<String>,
    pub(crate) edits: Entity<TagEdits>,
    client: Option<IndexClient>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TagsEvent> for TagsView {}

impl Focusable for TagsView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.filter.focus_handle(cx)
    }
}

pub fn blocks(count: usize) -> String {
    match count {
        1 => "1 block".into(),
        count => format!("{count} blocks"),
    }
}

/// The reference Tags list: labels containing the filter, by name or by block count with
/// ties by name.
pub fn visible(tags: &[TagCount], filter: &str, sort: TagSort) -> Vec<TagCount> {
    let needle = filter.trim().to_lowercase();
    let mut rows: Vec<TagCount> = tags
        .iter()
        .filter(|tag| tag.label.to_lowercase().contains(&needle))
        .cloned()
        .collect();
    match sort {
        TagSort::Name => {}
        TagSort::Most => rows.sort_by_key(|tag| std::cmp::Reverse(tag.count)),
        TagSort::Fewest => rows.sort_by_key(|tag| tag.count),
    }
    rows
}

impl TagsView {
    pub fn new(
        choices: TagListChoices,
        writers: Writers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let edits = cx.new(|cx| TagEdits::new(writers, window, cx));
        let filter = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Filter tags…", window, cx);
            editor.set_text(choices.query.as_str(), window, cx);
            editor
        });
        let filtered = cx.subscribe(&filter, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.highlighted = 0;
                this.remember(cx);
                cx.notify();
            }
        });
        let applied = cx.subscribe(&edits, |this, _, event: &TagEditsEvent, cx| {
            this.selected.clear();
            this.sync_selection(cx);
            if matches!(event, TagEditsEvent::Applied)
                && let Some(client) = this.client.clone()
            {
                this.refresh(&client, cx);
            }
        });
        let subscriptions = vec![filtered, applied];
        Self {
            tags: Live::default(),
            filter,
            sort: choices.sort,
            highlighted: 0,
            selected: Vec::new(),
            edits,
            client: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn refresh(&mut self, client: &IndexClient, cx: &mut Context<Self>) {
        self.client = Some(client.clone());
        self.edits
            .update(cx, |edits, _| edits.set_client(client.clone()));
        let answer = client.with_generation(|index| index.tag_names());
        self.tags.request(
            answer,
            cx,
            |this| &mut this.tags,
            |error| format!("Cannot list tags: {error}. Wait for indexing to finish, then retry."),
        );
    }

    fn remember(&self, cx: &mut Context<Self>) {
        cx.emit(TagsEvent::Remember(TagListChoices {
            query: self.filter.read(cx).text(cx),
            sort: self.sort,
        }));
    }

    pub fn rows(&self, cx: &App) -> Vec<TagCount> {
        visible(&self.tags.value, &self.filter.read(cx).text(cx), self.sort)
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

    fn open(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(tag) = self.rows(cx).get(index) {
            self.highlighted = index;
            cx.emit(TagsEvent::Open(tag.label.clone()));
        }
    }

    /// Adds the tag to the selection, or takes it out when already chosen.
    pub fn toggle(&mut self, label: &str, cx: &mut Context<Self>) {
        match self.selected.iter().position(|item| item == label) {
            Some(index) => {
                self.selected.remove(index);
            }
            None => self.selected.push(label.to_owned()),
        }
        self.sync_selection(cx);
    }

    #[cfg(test)]
    pub fn selected(&self) -> &[String] {
        &self.selected
    }

    fn sync_selection(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected.clone();
        self.edits
            .update(cx, |edits, cx| edits.set_selected(selected, cx));
        cx.notify();
    }

    fn toggle_highlighted(&mut self, cx: &mut Context<Self>) {
        if let Some(tag) = self.rows(cx).get(self.highlighted) {
            let label = tag.label.clone();
            self.toggle(&label, cx);
        }
    }

    fn render_rows(&self, rows: Vec<TagCount>, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "tag-rows",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let tag = rows.get(index)?.clone();
                        let chosen = this.selected.contains(&tag.label);
                        let label = tag.label.clone();
                        let view = cx.entity().downgrade();
                        Some(
                            ListItem::new(("tag", index))
                                .spacing(ListItemSpacing::Sparse)
                                .toggle_state(index == this.highlighted)
                                .start_slot(
                                    Checkbox::new(
                                        ("select-tag", index),
                                        if chosen {
                                            ToggleState::Selected
                                        } else {
                                            ToggleState::Unselected
                                        },
                                    )
                                    .on_click(
                                        move |_, _, cx| {
                                            view.update(cx, |this, cx| this.toggle(&label, cx))
                                                .ok();
                                        },
                                    ),
                                )
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(Label::new(tag.label.clone()).truncate())
                                        .child(
                                            Label::new(blocks(tag.count))
                                                .size(LabelSize::Small)
                                                .color(Color::Muted),
                                        ),
                                )
                                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                                    this.open(index, cx)
                                })),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
    }
}

impl Render for TagsView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows(cx);
        let count = rows.len();
        let empty = (count == 0).then_some(if self.tags.value.is_empty() {
            "No tags yet. Add a #tag to a block and it appears here."
        } else {
            "No tags match. Try another filter."
        });
        let root = v_flex()
            .id("tags")
            .key_context("QuireList QuireTags")
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .on_action(
                cx.listener(|this, _: &ToggleTagSelected, _, cx| this.toggle_highlighted(cx)),
            );
        list_keys(
            root,
            cx,
            |this, delta, cx| this.move_highlight(delta, cx),
            |this, _, _, cx| {
                let index = this.highlighted;
                this.open(index, cx)
            },
        )
        .child(Label::new("Tags"))
        .child(
            Label::new("Read connected blocks across your notebook.")
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
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new("tag-sort", self.sort.label()).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.sort = this.sort.next();
                            this.highlighted = 0;
                            this.remember(cx);
                            cx.notify();
                        },
                    )),
                )
                .child(
                    Label::new(match count {
                        1 => "1 tag".to_owned(),
                        count => format!("{count} tags"),
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
        )
        .child(self.edits.clone())
        .when_some(self.tags.error.clone(), |div, error: SharedString| {
            div.child(Label::new(error).color(Color::Error))
        })
        .when_some(empty, |div, empty| {
            div.child(Label::new(empty).color(Color::Muted))
        })
        .child(self.render_rows(rows, cx))
    }
}
