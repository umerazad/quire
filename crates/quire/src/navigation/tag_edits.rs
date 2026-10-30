//! Bulk rename and delete for the tags chosen in the Tags browser: a name field, a preview of
//! every change by page, and one all-or-nothing apply.

use crate::{
    notebook_session::OpenPage,
    tag_edit::{self, TagEditPlan},
};
use editor::{Editor, EditorEvent};
use gpui::{App, Context, Entity, EventEmitter, Subscription, Task, Window};
use quire_core::{markdown::page_name, tag_edits::Retag};
use quire_store::{index::IndexClient, worker::StorageClient};
use std::rc::Rc;
use ui::{
    Button, ButtonStyle, Color, Disableable, IconButton, IconName, Label, LabelCommon, LabelSize,
    prelude::*, v_flex,
};

/// Changes listed before "Show more changes".
pub const SHOWN_CHANGES: usize = 20;

/// The open pages and storage an edit writes through, read when it is planned and applied.
pub type Writers = Rc<dyn Fn(&App) -> (Vec<Entity<OpenPage>>, StorageClient)>;

pub enum TagEditsEvent {
    /// The edit was written; the selection no longer names existing tags.
    Applied,
    ClearSelection,
}

pub struct Preview {
    pub rename: Option<String>,
    pub chosen: Vec<String>,
    plan: TagEditPlan,
}

/// One listed change: the page's name, the tag before, and what replaces it (empty deletes).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ChangeRow {
    pub page: String,
    pub before: String,
    pub after: String,
}

impl Preview {
    pub fn pages(&self) -> usize {
        self.plan.pages.len()
    }

    pub fn changes(&self) -> Vec<ChangeRow> {
        self.plan
            .pages
            .iter()
            .flat_map(|page| {
                let name = page_name(page.path()).to_owned();
                page.changes.iter().map(move |change| ChangeRow {
                    page: name.clone(),
                    before: change.before.clone(),
                    after: change.after.clone(),
                })
            })
            .collect()
    }

    /// The first `limit` changes, and how many more stay hidden.
    pub fn shown(&self, limit: usize) -> (Vec<ChangeRow>, usize) {
        let mut changes = self.changes();
        let more = changes.len().saturating_sub(limit);
        changes.truncate(limit);
        (changes, more)
    }

    pub fn summary(&self) -> String {
        let pages = match self.pages() {
            1 => "1 page".to_owned(),
            pages => format!("{pages} pages"),
        };
        let chosen = self.chosen.join(", ");
        match &self.rename {
            Some(name) => {
                let name = name.trim();
                let name = name.strip_prefix('#').unwrap_or(name);
                format!("Rename {chosen} to #{name} on {pages}?")
            }
            None => format!("Delete {chosen} on {pages}?"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Planning,
    Applying,
}

pub struct TagEdits {
    writers: Writers,
    client: Option<IndexClient>,
    selected: Vec<String>,
    pub(crate) name: Entity<Editor>,
    pub(crate) preview: Option<Preview>,
    pub(crate) limit: usize,
    pub(crate) error: Option<SharedString>,
    pub(crate) done: Option<SharedString>,
    work: Option<(Phase, Task<()>)>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<TagEditsEvent> for TagEdits {}

impl TagEdits {
    pub fn new(writers: Writers, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("New name", window, cx);
            editor
        });
        let subscriptions = vec![cx.subscribe(&name, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.discard(cx);
            }
        })];
        Self {
            writers,
            client: None,
            selected: Vec::new(),
            name,
            preview: None,
            limit: SHOWN_CHANGES,
            error: None,
            done: None,
            work: None,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_client(&mut self, client: IndexClient) {
        self.client = Some(client);
    }

    /// A new selection invalidates any preview of the old one.
    pub fn set_selected(&mut self, selected: Vec<String>, cx: &mut Context<Self>) {
        if self.selected != selected {
            self.selected = selected;
            self.discard(cx);
        }
    }

    pub fn is_busy(&self) -> bool {
        self.work.is_some()
    }

    /// Drops the preview and any planning in flight. An apply already started finishes, so
    /// open pages never stay held.
    pub fn discard(&mut self, cx: &mut Context<Self>) {
        if self.phase() == Some(Phase::Applying) {
            return;
        }
        self.preview = None;
        self.error = None;
        self.work = None;
        cx.notify();
    }

    fn phase(&self) -> Option<Phase> {
        self.work.as_ref().map(|(phase, _)| *phase)
    }

    /// Plans renaming the selection to the typed name, or deleting it, and shows the preview.
    pub fn preview(&mut self, rename: bool, cx: &mut Context<Self>) {
        if self.phase() == Some(Phase::Applying) {
            return;
        }
        self.preview = None;
        self.done = None;
        self.limit = SHOWN_CHANGES;
        let name = rename.then(|| self.name.read(cx).text(cx));
        let retag = match Retag::new(&self.selected, name.as_deref()) {
            Ok(retag) => retag,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let Some(client) = self.client.clone() else {
            self.error = Some(
                "Tag edits need the notebook index, which has not started. Wait for it, then retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        self.error = None;
        let (open, storage) = (self.writers)(cx);
        let plan = tag_edit::plan(retag, open, client, storage, cx);
        let chosen = self.selected.clone();
        self.work = Some((
            Phase::Planning,
            cx.spawn(async move |this, cx| {
                let plan = plan.await;
                this.update(cx, |this, cx| {
                    this.work = None;
                    match plan {
                        Ok(plan) => {
                            this.preview = Some(Preview {
                                rename: name,
                                chosen,
                                plan,
                            })
                        }
                        Err(error) => this.error = Some(error.into()),
                    }
                    cx.notify();
                })
                .ok();
            }),
        ));
        cx.notify();
    }

    /// Writes the previewed edit, refusing it when any page changed since the preview.
    pub fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(preview) = self.preview.take() else {
            return;
        };
        let (_, storage) = (self.writers)(cx);
        let work = tag_edit::apply(preview.plan, storage, cx);
        self.error = None;
        let name = self.name.clone();
        self.work = Some((
            Phase::Applying,
            cx.spawn_in(window, async move |this, cx| {
                let result = work.await;
                this.update_in(cx, |this, window, cx| {
                    this.work = None;
                    match result {
                        Ok(pages) => {
                            this.done = Some(
                                match pages {
                                    1 => "Changed tags on 1 page.".to_owned(),
                                    pages => format!("Changed tags on {pages} pages."),
                                }
                                .into(),
                            );
                            name.update(cx, |name, cx| name.clear(window, cx));
                            this.preview = None;
                            cx.emit(TagEditsEvent::Applied);
                        }
                        Err(error) => this.error = Some(error.into()),
                    }
                    cx.notify();
                })
                .ok();
            }),
        ));
        cx.notify();
    }

    pub fn show_more(&mut self, cx: &mut Context<Self>) {
        self.limit += SHOWN_CHANGES;
        cx.notify();
    }

    fn render_preview(&self, preview: &Preview, cx: &mut Context<Self>) -> impl IntoElement {
        let (changes, more) = preview.shown(self.limit);
        v_flex()
            .gap_1()
            .p_1()
            .border_1()
            .border_color(cx.theme().colors().border)
            .child(Label::new(preview.summary()))
            .when(preview.rename.is_some(), |div| {
                div.child(
                    Label::new("Tags nested under these move with them.")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .children(changes.into_iter().enumerate().map(|(index, change)| {
                h_flex()
                    .id(("tag-change", index))
                    .gap_2()
                    .child(
                        Label::new(change.page)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(change.before)
                            .strikethrough()
                            .color(Color::Deleted),
                    )
                    .when(!change.after.is_empty(), |row| {
                        row.child(Label::new(change.after).color(Color::Created))
                    })
            }))
            .when(more > 0, |div| {
                div.child(
                    Button::new("more-changes", format!("Show more changes ({more} more)"))
                        .style(ButtonStyle::Subtle)
                        .on_click(cx.listener(|this, _, _, cx| this.show_more(cx))),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("tag-edit-back", "Back")
                            .disabled(self.is_busy())
                            .on_click(cx.listener(|this, _, _, cx| this.discard(cx))),
                    )
                    .child(
                        Button::new("tag-edit-apply", "Apply")
                            .style(ButtonStyle::Filled)
                            .disabled(self.is_busy() || preview.pages() == 0)
                            .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
                    ),
            )
    }
}

impl Render for TagEdits {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let named = !self.name.read(cx).text(cx).trim().is_empty();
        let busy = self.is_busy();
        let selected = self.selected.len();
        let preview = self
            .preview
            .as_ref()
            .map(|preview| self.render_preview(preview, cx).into_any_element());
        v_flex()
            .key_context("QuireTagEdit")
            .gap_1()
            .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| {
                if !this.is_busy() {
                    this.preview(true, cx)
                }
            }))
            .when(selected > 0, |panel| {
                panel.child(
                    h_flex()
                        .gap_2()
                        .child(Label::new(format!("{selected} selected")))
                        .child(
                            div()
                                .w(px(200.))
                                .border_1()
                                .border_color(cx.theme().colors().border)
                                .p_1()
                                .child(self.name.clone()),
                        )
                        .child(
                            Button::new("tag-rename", "Rename…")
                                .disabled(busy || !named)
                                .on_click(cx.listener(|this, _, _, cx| this.preview(true, cx))),
                        )
                        .child(
                            Button::new("tag-delete", "Delete…")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.preview(false, cx))),
                        )
                        .child(
                            IconButton::new("tag-clear", IconName::Close)
                                .disabled(busy)
                                .tooltip(ui::Tooltip::text("Clear selection"))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.discard(cx);
                                    cx.emit(TagEditsEvent::ClearSelection);
                                })),
                        ),
                )
            })
            .when(busy, |div| {
                div.child(
                    Label::new(match self.phase() {
                        Some(Phase::Applying) => "Applying…",
                        _ => "Preparing the preview…",
                    })
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                )
            })
            .children(preview)
            .when_some(self.error.clone(), |div, error| {
                div.child(Label::new(error).color(Color::Error))
            })
            .when_some(self.done.clone(), |div, done| {
                div.child(Label::new(done).size(LabelSize::Small).color(Color::Muted))
            })
    }
}
