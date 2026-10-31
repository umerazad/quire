//! Page history: the page's earlier saved versions with their times, a read-only Zed editor
//! showing what restoring the chosen version changes in the current page as expanded diff hunks,
//! and two ways back. Restore makes the version the page's newest one in one checked save;
//! Restore as a new page saves it beside the page with fresh block IDs, as the reference app's
//! only restore does. Neither overwrites anything unseen: unsaved typing is saved first and stays
//! in history, and a page changed meanwhile refuses the restore.

use crate::{
    file_actions,
    navigation::{create_page_with, tag_edits::Writers},
    query_index::ask,
};
use buffer_diff::BufferDiff;
use editor::{Editor, EditorMode};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Task, Window, rems,
};
use language::{Buffer, Capability};
use multi_buffer::MultiBuffer;
use quire_core::source_text::SourceText;
use quire_store::{index::IndexClient, notebook::SavedVersion};
use std::rc::Rc;
use ui::{
    Button, ButtonStyle, Color, Disableable, Label, LabelCommon, LabelSize, Modal, ModalFooter,
    ModalHeader, Section, prelude::*, v_flex,
};

pub const HISTORY_CONTEXT: &str = "QuireHistory";
pub const NO_VERSIONS: &str = "No earlier saved versions for this page.";

pub enum PageHistoryEvent {
    /// The chosen version is now the page's newest one.
    Restored,
    /// The chosen version was saved as this new page.
    Copied(String),
}

/// A version's text and the read-only diff editor showing it against the current page.
pub struct Preview {
    pub text: String,
    /// The page text the version was compared with, in stored form.
    pub compared: String,
    pub editor: Entity<Editor>,
}

pub enum Load<T> {
    Loading,
    Ready(T),
    Failed(SharedString),
}

pub struct PageHistory {
    focus: FocusHandle,
    page: String,
    path: String,
    writers: Writers,
    index: IndexClient,
    label: Rc<dyn Fn(i64) -> String>,
    date: String,
    pub(crate) versions: Load<Vec<SavedVersion>>,
    pub(crate) selected: usize,
    pub(crate) preview: Load<Preview>,
    pub(crate) error: Option<SharedString>,
    loading: Option<Task<()>>,
    restoring: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for PageHistory {}
impl EventEmitter<PageHistoryEvent> for PageHistory {}

impl Focusable for PageHistory {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// How page history names times: `label` turns a save time into text, and `today` dates a
/// restored copy's title.
pub struct Clock {
    pub label: Rc<dyn Fn(i64) -> String>,
    pub today: String,
}

impl PageHistory {
    /// Lists `page`'s saved versions and previews the newest.
    pub fn new(
        page: String,
        path: String,
        writers: Writers,
        index: IndexClient,
        clock: Clock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            focus: cx.focus_handle(),
            page,
            path,
            writers,
            index,
            label: clock.label,
            date: clock.today,
            versions: Load::Loading,
            selected: 0,
            preview: Load::Loading,
            error: None,
            loading: None,
            restoring: None,
        };
        this.load_versions(window, cx);
        this
    }

    /// Reads the version list again, after a failure or to retry.
    pub fn load_versions(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (_, storage) = (self.writers)(cx);
        let page = self.page.clone();
        self.versions = Load::Loading;
        self.preview = Load::Loading;
        self.loading = Some(cx.spawn_in(window, async move |this, cx| {
            let listed = ask(storage.history(page)).await;
            this.update_in(cx, |this, window, cx| {
                this.loading = None;
                match listed {
                    Ok(versions) => {
                        this.versions = Load::Ready(versions);
                        this.selected = 0;
                        this.load_preview(window, cx);
                    }
                    Err(error) => {
                        this.versions = Load::Failed(
                            format!(
                                "Page history could not be loaded: {error} Retry loading history."
                            )
                            .into(),
                        );
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn chosen(&self) -> Option<&SavedVersion> {
        match &self.versions {
            Load::Ready(versions) => versions.get(self.selected),
            _ => None,
        }
    }

    /// Reads the chosen version and the current page, and diffs them in a read-only editor. A
    /// newer choice replaces this one, so a late answer never shows the wrong version.
    pub fn load_preview(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(version) = self.chosen().cloned() else {
            self.preview = Load::Loading;
            return;
        };
        let (open, storage) = (self.writers)(cx);
        let page = self.page.clone();
        self.preview = Load::Loading;
        self.error = None;
        self.loading = Some(cx.spawn_in(window, async move |this, cx| {
            let text = ask(storage.version_text(page.clone(), version.id)).await;
            let current = crate::page_writes::read(page, &open, &storage, cx).await;
            let (text, current) = match (text, current) {
                (Ok(text), Ok(current)) => (
                    text,
                    current.map(|source| source.stored).unwrap_or_default(),
                ),
                (Err(error), _) | (_, Err(error)) => {
                    this.update(cx, |this, cx| {
                        this.loading = None;
                        this.preview = Load::Failed(
                            format!("This saved version could not be loaded: {error} Retry loading the version.")
                                .into(),
                        );
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            let (_, version_text) = SourceText::prepare(text.clone());
            let (_, current_text) = SourceText::prepare(current.clone());
            let Ok((buffer, diff, update)) = this.update(cx, |_, cx| {
                let buffer = cx.new(|cx| {
                    let mut buffer = Buffer::local(version_text, cx);
                    buffer.set_capability(Capability::ReadOnly, cx);
                    buffer
                });
                let snapshot = buffer.read(cx).text_snapshot();
                let diff = cx.new(|cx| BufferDiff::new(&snapshot, None, None, cx));
                let update = diff.update(cx, |diff, cx| {
                    diff.set_base_text(Some(current_text.into()), snapshot, cx)
                });
                (buffer, diff, update)
            }) else {
                return;
            };
            update.await;
            this.update_in(cx, |this, window, cx| {
                let multibuffer = cx.new(|cx| {
                    let mut multibuffer = MultiBuffer::singleton(buffer, cx);
                    multibuffer.add_diff(diff, cx);
                    multibuffer
                });
                let editor = cx.new(|cx| {
                    let mut editor = Editor::new(EditorMode::full(), multibuffer, None, window, cx);
                    editor.set_read_only(true);
                    editor.set_expand_all_diff_hunks(cx);
                    editor
                });
                this.loading = None;
                this.preview = Load::Ready(Preview {
                    text,
                    compared: current,
                    editor,
                });
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn select(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let count = match &self.versions {
            Load::Ready(versions) => versions.len(),
            _ => 0,
        };
        if self.is_restoring() || index >= count || index == self.selected {
            return;
        }
        self.selected = index;
        self.load_preview(window, cx);
    }

    fn step(&mut self, delta: isize, window: &mut Window, cx: &mut Context<Self>) {
        let next = self.selected.saturating_add_signed(delta);
        self.select(next, window, cx);
    }

    pub fn is_restoring(&self) -> bool {
        self.restoring.is_some()
    }

    pub fn can_restore(&self) -> bool {
        !self.is_restoring() && matches!(self.preview, Load::Ready(_))
    }

    /// Makes the previewed version the page's newest one. A page changed since the preview
    /// refuses, and the preview is compared again with the page as it is now.
    pub fn restore(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Load::Ready(preview) = &self.preview else {
            return;
        };
        if self.is_restoring() {
            return;
        }
        let (open, storage) = (self.writers)(cx);
        let work = file_actions::restore_version(
            self.page.clone(),
            preview.text.clone(),
            preview.compared.clone(),
            open,
            storage,
            cx,
        );
        self.error = None;
        self.restoring = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.restoring = None;
                match result {
                    Ok(_) => {
                        cx.emit(PageHistoryEvent::Restored);
                        cx.emit(DismissEvent);
                    }
                    Err(error) => {
                        if error == file_actions::STALE_RESTORE {
                            this.load_preview(window, cx);
                        }
                        this.error =
                            Some(format!("The version could not be restored: {error}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Saves the previewed version as a new page beside this one, with fresh block IDs.
    pub fn restore_as_page(&mut self, cx: &mut Context<Self>) {
        let Load::Ready(preview) = &self.preview else {
            return;
        };
        if self.is_restoring() {
            return;
        }
        let (title, copied) =
            match file_actions::page_copy(&self.path, &preview.text, "restored", &self.date) {
                Ok(copy) => copy,
                Err(error) => {
                    self.error = Some(error.into());
                    cx.notify();
                    return;
                }
            };
        let (_, storage) = (self.writers)(cx);
        let folder = self
            .path
            .rsplit_once('/')
            .map_or(String::new(), |(folder, _)| folder.to_owned());
        let work = create_page_with(storage, self.index.clone(), title, folder, copied);
        self.error = None;
        self.restoring = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.restoring = None;
                match result {
                    Ok(page) => {
                        cx.emit(PageHistoryEvent::Copied(page));
                        cx.emit(DismissEvent);
                    }
                    Err(error) => {
                        this.error = Some(
                            format!("The version could not be restored: {error} Retry restoring it as a new page.")
                                .into(),
                        )
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_restoring() {
            cx.emit(DismissEvent);
        }
    }

    #[cfg(test)]
    /// The listed save times, as the list shows them.
    pub fn labels(&self) -> Vec<String> {
        match &self.versions {
            Load::Ready(versions) => versions
                .iter()
                .map(|version| (self.label)(version.saved_at))
                .collect(),
            _ => Vec::new(),
        }
    }

    #[cfg(test)]
    pub fn preview_text(&self) -> Option<&str> {
        match &self.preview {
            Load::Ready(preview) => Some(&preview.text),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn preview_editor(&self) -> Option<Entity<Editor>> {
        match &self.preview {
            Load::Ready(preview) => Some(preview.editor.clone()),
            _ => None,
        }
    }

    fn render_list(&self, versions: &[SavedVersion], cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .id("history-versions")
            .w(rems(12.))
            .max_h(rems(24.))
            .overflow_y_scroll()
            .gap_0p5()
            .children(versions.iter().enumerate().map(|(index, version)| {
                Button::new(("history-version", index), (self.label)(version.saved_at))
                    .toggle_state(index == self.selected)
                    .disabled(self.is_restoring())
                    .on_click(
                        cx.listener(move |this, _, window, cx| this.select(index, window, cx)),
                    )
            }))
    }

    fn render_preview(&self, cx: &mut Context<Self>) -> AnyElement {
        match &self.preview {
            Load::Loading => Label::new("Loading saved version…")
                .color(Color::Muted)
                .into_any_element(),
            Load::Failed(error) => v_flex()
                .gap_1()
                .child(Label::new(error.clone()).color(Color::Error))
                .child(
                    Button::new("history-retry-version", "Retry loading version")
                        .on_click(cx.listener(|this, _, window, cx| this.load_preview(window, cx))),
                )
                .into_any_element(),
            Load::Ready(preview) => v_flex()
                .gap_1()
                .child(
                    Label::new("What restoring this version changes in the current page:")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    div()
                        .h(rems(24.))
                        .border_1()
                        .border_color(cx.theme().colors().border)
                        .child(preview.editor.clone()),
                )
                .into_any_element(),
        }
    }
}

impl Render for PageHistory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let restoring = self.is_restoring();
        let mut modal = Modal::new("page-history", None)
            .header(
                ModalHeader::new()
                    .headline("Previous saved versions")
                    .description(self.path.clone())
                    .show_dismiss_button(true),
            )
            .footer(
                ModalFooter::new().end_slot(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("history-cancel", "Close")
                                .disabled(restoring)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        )
                        .child(
                            Button::new("history-copy", "Restore as a new page")
                                .disabled(!self.can_restore())
                                .on_click(cx.listener(|this, _, _, cx| this.restore_as_page(cx))),
                        )
                        .child(
                            Button::new(
                                "history-restore",
                                if restoring { "Restoring…" } else { "Restore" },
                            )
                            .style(ButtonStyle::Filled)
                            .disabled(!self.can_restore())
                            .on_click(cx.listener(|this, _, window, cx| this.restore(window, cx))),
                        ),
                ),
            );
        let body = match &self.versions {
            Load::Loading => Label::new("Loading saved versions…")
                .color(Color::Muted)
                .into_any_element(),
            Load::Failed(error) => v_flex()
                .gap_1()
                .child(Label::new(error.clone()).color(Color::Error))
                .child(
                    Button::new("history-retry", "Retry loading history").on_click(
                        cx.listener(|this, _, window, cx| this.load_versions(window, cx)),
                    ),
                )
                .into_any_element(),
            Load::Ready(versions) if versions.is_empty() => {
                Label::new(NO_VERSIONS).into_any_element()
            }
            Load::Ready(versions) => h_flex()
                .items_start()
                .gap_2()
                .child(self.render_list(versions, cx))
                .child(div().flex_1().child(self.render_preview(cx)))
                .into_any_element(),
        };
        modal = modal.section(Section::new().child(body));
        if let Some(error) = self.error.clone() {
            modal = modal.section(Section::new().child(Label::new(error).color(Color::Error)));
        }
        v_flex()
            .key_context(HISTORY_CONTEXT)
            .track_focus(&self.focus)
            .elevation_3(cx)
            .w(rems(56.))
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| this.restore(window, cx)))
            .on_action(
                cx.listener(|this, _: &menu::SecondaryConfirm, _, cx| this.restore_as_page(cx)),
            )
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.cancel(cx)))
            .on_action(
                cx.listener(|this, _: &menu::SelectNext, window, cx| this.step(1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &menu::SelectPrevious, window, cx| this.step(-1, window, cx)),
            )
            .child(modal)
    }
}
