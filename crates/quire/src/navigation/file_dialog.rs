//! The modal that confirms deleting a page or folder: it names what goes and how many links stop
//! resolving, and applies only when confirmed.

use crate::{
    file_actions::{self, DeletionPlan, Target},
    navigation::tag_edits::Writers,
};
use gpui::{App, Context, DismissEvent, EventEmitter, FocusHandle, Focusable, Task, Window, rems};
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

pub struct FileDialog {
    focus: FocusHandle,
    writers: Writers,
    target: Target,
    plan: Option<DeletionPlan>,
    pub(crate) summary: Option<Summary>,
    pub(crate) error: Option<SharedString>,
    work: Option<Task<()>>,
}

impl EventEmitter<DismissEvent> for FileDialog {}
impl EventEmitter<FileDialogEvent> for FileDialog {}

impl Focusable for FileDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
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
                        this.summary = Some(Summary::of(&plan));
                        this.plan = Some(plan);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        });
        Self {
            focus: cx.focus_handle(),
            writers,
            target,
            plan: None,
            summary: None,
            error: None,
            work: Some(work),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.work.is_some()
    }

    pub fn can_confirm(&self) -> bool {
        !self.is_busy() && self.plan.is_some()
    }

    fn title(&self) -> &'static str {
        match self.target {
            Target::Page(_) => "Delete page",
            Target::Folder(_) => "Delete folder",
        }
    }

    /// Deletes what the dialog names. A refusal leaves nothing deleted and shows why.
    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let Some(plan) = self.plan.take() else {
            return;
        };
        let folder = plan.folder().map(str::to_owned);
        let (_, storage) = (self.writers)(cx);
        let work = file_actions::delete(plan, storage, cx);
        self.error = None;
        self.work = Some(cx.spawn(async move |this, cx| {
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
        }));
        cx.notify();
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
}

impl Render for FileDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let path = match &self.target {
            Target::Page(_) => self
                .plan
                .as_ref()
                .and_then(|plan| plan.pages.first())
                .map(|(_, path)| path.clone())
                .unwrap_or_default(),
            Target::Folder(folder) => folder.clone(),
        };
        let mut modal = Modal::new("file-dialog", None)
            .header(
                ModalHeader::new()
                    .headline(self.title())
                    .description(path)
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
                            Button::new(
                                "file-dialog-delete",
                                if busy && self.summary.is_some() {
                                    "Deleting…"
                                } else {
                                    "Delete"
                                },
                            )
                            .style(ButtonStyle::Tinted(ui::TintColor::Error))
                            .disabled(!self.can_confirm())
                            .on_click(cx.listener(|this, _, _, cx| this.confirm(cx))),
                        ),
                ),
            );
        match &self.summary {
            Some(summary) => modal = modal.section(self.render_summary(summary)),
            None if busy => {
                modal = modal.section(
                    Section::new()
                        .child(Label::new("Finding what this deletes…").color(Color::Muted)),
                )
            }
            None => {}
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
