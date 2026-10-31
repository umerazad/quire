//! The modal for deleting a page or folder and for renaming a page. Deleting names what goes and
//! how many links stop resolving, and applies only when confirmed. Renaming takes the new name and
//! applies it with every link that follows.

use crate::{
    file_actions::{self, DeletionPlan, Target},
    navigation::tag_edits::Writers,
};
use editor::{Editor, EditorEvent};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Subscription, Task,
    Window, rems,
};
use quire_core::markdown::page_name;
use quire_store::index::IndexClient;
use ui::{
    Button, ButtonStyle, Color, Disableable, Label, LabelCommon, LabelSize, Modal, ModalFooter,
    ModalHeader, Section, prelude::*, v_flex,
};

pub const DIALOG_CONTEXT: &str = "QuireDialog";

pub enum FileDialogEvent {
    /// These pages were deleted, from `folder` when a folder went.
    Deleted {
        pages: Vec<String>,
        folder: Option<String>,
    },
    /// The page now lives at `path`, and every link to it followed.
    Renamed { page: String, path: String },
}

/// What the dialog shows about a planned deletion; it stays after a refused apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub heading: String,
    pub detail: String,
    pub listed: Vec<String>,
    pub more: usize,
}

impl Summary {
    fn of(plan: &DeletionPlan) -> Self {
        let (listed, more) = plan.listed();
        Self {
            heading: plan.heading(),
            detail: plan.detail(),
            listed: listed.into_iter().map(str::to_owned).collect(),
            more,
        }
    }
}

enum Mode {
    Delete {
        folder: bool,
        plan: Option<DeletionPlan>,
    },
    Rename {
        page: String,
        name: Entity<Editor>,
        index: IndexClient,
    },
}

pub struct FileDialog {
    focus: FocusHandle,
    writers: Writers,
    mode: Mode,
    path: SharedString,
    pub(crate) summary: Option<Summary>,
    pub(crate) error: Option<SharedString>,
    work: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<DismissEvent> for FileDialog {}
impl EventEmitter<FileDialogEvent> for FileDialog {}

impl Focusable for FileDialog {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.mode {
            Mode::Rename { name, .. } => name.focus_handle(cx),
            Mode::Delete { .. } => self.focus.clone(),
        }
    }
}

impl FileDialog {
    /// Asks to delete a page or a folder, planning what goes before anything can be confirmed.
    pub fn delete(
        target: Target,
        writers: Writers,
        index: IndexClient,
        cx: &mut Context<Self>,
    ) -> Self {
        let (open, storage) = writers(cx);
        let plan = file_actions::plan_deletion(target.clone(), open, index, storage, cx);
        let work = cx.spawn(async move |this, cx| {
            let plan = plan.await;
            this.update(cx, |this, cx| {
                this.work = None;
                match plan {
                    Ok(plan) => {
                        if let Some((_, path)) = plan.pages.first()
                            && plan.folder().is_none()
                        {
                            this.path = path.clone().into();
                        }
                        this.summary = Some(Summary::of(&plan));
                        if let Mode::Delete { plan: planned, .. } = &mut this.mode {
                            *planned = Some(plan);
                        }
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        });
        let (folder, path) = match target {
            Target::Folder(folder) => (true, folder),
            Target::Page(_) => (false, String::new()),
        };
        Self {
            focus: cx.focus_handle(),
            writers,
            mode: Mode::Delete { folder, plan: None },
            path: path.into(),
            summary: None,
            error: None,
            work: Some(work),
            _subscriptions: Vec::new(),
        }
    }

    /// Asks for a page's new name, starting from its current one.
    pub fn rename(
        page: String,
        path: String,
        writers: Writers,
        index: IndexClient,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let current = page_name(path.rsplit('/').next().unwrap_or_default()).to_owned();
        let name = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(current, window, cx);
            editor.select_all(&editor::actions::SelectAll, window, cx);
            editor
        });
        let subscriptions = vec![cx.subscribe(&name, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) && this.error.take().is_some() {
                cx.notify();
            }
        })];
        Self {
            focus: cx.focus_handle(),
            writers,
            mode: Mode::Rename { page, name, index },
            path: path.into(),
            summary: None,
            error: None,
            work: None,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    /// The name typed so far, for a rename.
    pub fn name(&self, cx: &App) -> Option<String> {
        match &self.mode {
            Mode::Rename { name, .. } => Some(name.read(cx).text(cx)),
            Mode::Delete { .. } => None,
        }
    }

    #[cfg(test)]
    /// Selects the whole typed name, so typing replaces it.
    pub fn select_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Mode::Rename { name, .. } = &self.mode {
            name.update(cx, |name, cx| {
                name.select_all(&editor::actions::SelectAll, window, cx)
            });
        }
    }

    pub fn is_busy(&self) -> bool {
        self.work.is_some()
    }

    pub fn can_confirm(&self) -> bool {
        !self.is_busy()
            && match &self.mode {
                Mode::Delete { plan, .. } => plan.is_some(),
                Mode::Rename { .. } => true,
            }
    }

    fn title(&self) -> &'static str {
        match self.mode {
            Mode::Delete { folder: false, .. } => "Delete page",
            Mode::Delete { folder: true, .. } => "Delete folder",
            Mode::Rename { .. } => "Rename page",
        }
    }

    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        if !self.can_confirm() {
            return;
        }
        self.error = None;
        let work = match &mut self.mode {
            Mode::Delete { plan, .. } => {
                let Some(plan) = plan.take() else {
                    return;
                };
                self.delete_now(plan, cx)
            }
            Mode::Rename { page, name, index } => {
                let (page, name, index) = (page.clone(), name.read(cx).text(cx), index.clone());
                self.rename_now(page, name, index, cx)
            }
        };
        self.work = Some(work);
        cx.notify();
    }

    /// Deletes what the dialog names. A refusal leaves nothing deleted and shows why.
    fn delete_now(&mut self, plan: DeletionPlan, cx: &mut Context<Self>) -> Task<()> {
        let folder = plan.folder().map(str::to_owned);
        let (_, storage) = (self.writers)(cx);
        let work = file_actions::delete(plan, storage, cx);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(pages) => {
                        cx.emit(FileDialogEvent::Deleted { pages, folder });
                        cx.emit(DismissEvent);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
    }

    /// Renames the page and rewrites every link to it in one write. A refusal changes nothing.
    fn rename_now(
        &mut self,
        page: String,
        name: String,
        index: IndexClient,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let writers = self.writers.clone();
        let (open, storage) = writers(cx);
        let plan = file_actions::plan_rename(page.clone(), name, open, index, storage, cx);
        cx.spawn(async move |this, cx| {
            let result = match plan.await {
                Ok(Some(plan)) => {
                    let path = plan.target.clone();
                    let written = cx.update(|cx| {
                        let (_, storage) = writers(cx);
                        file_actions::rename(plan, storage, cx)
                    });
                    written.await.map(|_| Some(path))
                }
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            };
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(renamed) => {
                        if let Some(path) = renamed {
                            cx.emit(FileDialogEvent::Renamed { page, path });
                        }
                        cx.emit(DismissEvent);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_busy() {
            cx.emit(DismissEvent);
        }
    }

    fn render_summary(&self, summary: &Summary) -> Section {
        Section::new()
            .child(Label::new(summary.heading.clone()))
            .child(Label::new(summary.detail.clone()).color(Color::Muted))
            .children(summary.listed.iter().map(|path| {
                Label::new(path.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted)
            }))
            .when(summary.more > 0, |section| {
                section.child(
                    Label::new(format!("And {} more.", summary.more))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
    }

    fn confirm_label(&self) -> &'static str {
        match (&self.mode, self.is_busy()) {
            (Mode::Delete { .. }, true) if self.summary.is_some() => "Deleting…",
            (Mode::Delete { .. }, _) => "Delete",
            (Mode::Rename { .. }, true) => "Renaming…",
            (Mode::Rename { .. }, false) => "Rename",
        }
    }
}

impl Render for FileDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let style = match self.mode {
            Mode::Delete { .. } => ButtonStyle::Tinted(ui::TintColor::Error),
            Mode::Rename { .. } => ButtonStyle::Filled,
        };
        let mut modal = Modal::new("file-dialog", None)
            .header(
                ModalHeader::new()
                    .headline(self.title())
                    .description(self.path.clone())
                    .show_dismiss_button(true),
            )
            .footer(
                ModalFooter::new().end_slot(
                    h_flex()
                        .gap_1()
                        .child(
                            Button::new("file-dialog-cancel", "Cancel")
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                        )
                        .child(
                            Button::new("file-dialog-confirm", self.confirm_label())
                                .style(style)
                                .disabled(!self.can_confirm())
                                .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                        ),
                ),
            );
        match (&self.mode, &self.summary) {
            (Mode::Rename { name, .. }, _) => modal = modal.section(
                Section::new()
                    .child(
                        div()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .p_1()
                            .child(name.clone()),
                    )
                    .child(
                        Label::new(
                            "The file name and title change together; links to this page follow.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ),
            ),
            (Mode::Delete { .. }, Some(summary)) => {
                modal = modal.section(self.render_summary(summary))
            }
            (Mode::Delete { .. }, None) if busy => {
                modal = modal.section(
                    Section::new()
                        .child(Label::new("Finding what this deletes…").color(Color::Muted)),
                )
            }
            (Mode::Delete { .. }, None) => {}
        }
        if let Some(error) = self.error.clone() {
            modal = modal.section(Section::new().child(Label::new(error).color(Color::Error)));
        }
        v_flex()
            .key_context(DIALOG_CONTEXT)
            .track_focus(&self.focus)
            .elevation_3(cx)
            .w(rems(34.))
            .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.confirm(cx)))
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.cancel(cx)))
            .child(modal)
    }
}
