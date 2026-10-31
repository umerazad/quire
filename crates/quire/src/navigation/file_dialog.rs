//! The modal for deleting a page or folder, renaming a page, and moving a page or folder.
//! Deleting names what goes and how many links stop resolving, and applies only when confirmed.
//! Renaming takes the new name and applies it with every link that follows. Moving takes a path,
//! previews every page and link that changes, and applies that preview or nothing. A new folder
//! takes a path and stays on this computer until a page is saved in it, as the reference's does.

use crate::{
    file_actions::{self, DeletionPlan, LinkUpdate, Relocate, RelocationPlan, Target},
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

/// Pages and link changes a move preview lists before "Show more".
pub const LISTED_CHANGES: usize = 20;

pub enum FileDialogEvent {
    /// These pages were deleted, from `folder` when a folder went.
    Deleted {
        pages: Vec<String>,
        folder: Option<String>,
    },
    /// These pages were renamed or moved, and every link to them followed.
    Relocated(Vec<file_actions::Renamed>),
    /// A new, empty folder at this path.
    FolderCreated(String),
    /// A folder holding no page moved, with any other files in it.
    FolderMoved { from: String, to: String },
}

/// What the dialog shows about a planned deletion or move; it stays after a refused apply.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Summary {
    pub heading: String,
    pub detail: String,
    pub listed: Vec<String>,
    pub more: usize,
    /// The link changes a move makes, each as page, before, and after.
    pub updates: Vec<LinkUpdate>,
}

impl Summary {
    fn of(plan: &DeletionPlan) -> Self {
        let (listed, more) = plan.listed();
        Self {
            heading: plan.heading(),
            detail: plan.detail(),
            listed: listed.into_iter().map(str::to_owned).collect(),
            more,
            updates: Vec::new(),
        }
    }

    fn of_move(plan: &RelocationPlan) -> Self {
        Self {
            heading: plan.heading(),
            detail: plan.detail(),
            listed: plan.updated.iter().take(LISTED_CHANGES).cloned().collect(),
            more: plan.updated.len().saturating_sub(LISTED_CHANGES),
            updates: plan.updates.clone(),
        }
    }
}

/// What is moved: one page by identity, or a folder by path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Moving {
    Page(String),
    Folder(String),
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
    Move {
        moving: Moving,
        path: Entity<Editor>,
        index: IndexClient,
        plan: Option<RelocationPlan>,
        /// How many link changes the preview shows.
        shown: usize,
    },
    Folder {
        path: Entity<Editor>,
        /// Folders and pages already in the notebook, which a new folder may not reuse.
        taken: Vec<String>,
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
            Mode::Move { path, .. } | Mode::Folder { path, .. } => path.focus_handle(cx),
            Mode::Delete { .. } => self.focus.clone(),
        }
    }
}

fn field(text: String, window: &mut Window, cx: &mut Context<FileDialog>) -> Entity<Editor> {
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_text(text, window, cx);
        editor.select_all(&editor::actions::SelectAll, window, cx);
        editor
    })
}

/// The reference `createFolder` path check, refusing a name a folder or page already uses.
pub fn new_folder(value: &str, taken: &[String]) -> Result<String, String> {
    let folder = quire_core::relocate::folder_path(value).map_err(str::to_owned)?;
    match taken
        .iter()
        .any(|path| path.to_lowercase() == folder.to_lowercase())
    {
        true => Err(format!(
            "{folder} already exists. Choose another name for the new folder."
        )),
        false => Ok(folder),
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
        let name = field(current, window, cx);
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

    /// Asks where a page or folder goes, starting from where it is. Editing the path drops any
    /// preview, so only a previewed move can be applied.
    pub fn relocate(
        moving: Moving,
        path: String,
        writers: Writers,
        index: IndexClient,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let editor = field(path.clone(), window, cx);
        let subscriptions = vec![cx.subscribe(&editor, |this, _, event: &EditorEvent, cx| {
            if !matches!(event, EditorEvent::BufferEdited) {
                return;
            }
            this.error = None;
            this.summary = None;
            if let Mode::Move { plan, .. } = &mut this.mode {
                *plan = None;
            }
            cx.notify();
        })];
        Self {
            focus: cx.focus_handle(),
            writers,
            mode: Mode::Move {
                moving,
                path: editor,
                index,
                plan: None,
                shown: LISTED_CHANGES,
            },
            path: path.into(),
            summary: None,
            error: None,
            work: None,
            _subscriptions: subscriptions,
        }
    }

    /// Asks for a new folder's path, starting inside `parent`.
    pub fn new_folder(
        parent: String,
        taken: Vec<String>,
        writers: Writers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let start = if parent.is_empty() {
            String::new()
        } else {
            format!("{parent}/")
        };
        let path = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_text(start, window, cx);
            editor.move_to_end(&editor::actions::MoveToEnd, window, cx);
            editor
        });
        let subscriptions = vec![cx.subscribe(&path, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) && this.error.take().is_some() {
                cx.notify();
            }
        })];
        Self {
            focus: cx.focus_handle(),
            writers,
            mode: Mode::Folder { path, taken },
            path: if parent.is_empty() {
                "Notebook root".into()
            } else {
                parent.into()
            },
            summary: None,
            error: None,
            work: None,
            _subscriptions: subscriptions,
        }
    }

    #[cfg(test)]
    /// The name or path typed so far, for a rename or move.
    pub fn name(&self, cx: &App) -> Option<String> {
        match &self.mode {
            Mode::Rename { name, .. } => Some(name.read(cx).text(cx)),
            Mode::Move { path, .. } | Mode::Folder { path, .. } => Some(path.read(cx).text(cx)),
            Mode::Delete { .. } => None,
        }
    }

    #[cfg(test)]
    /// Selects the whole typed name or path, so typing replaces it.
    pub fn select_name(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Mode::Rename { name: field, .. } | Mode::Move { path: field, .. } = &self.mode {
            field.update(cx, |field, cx| {
                field.select_all(&editor::actions::SelectAll, window, cx)
            });
        }
    }

    #[cfg(test)]
    /// Whether a move preview is ready to apply.
    pub fn previewed(&self) -> bool {
        matches!(&self.mode, Mode::Move { plan: Some(_), .. })
    }

    #[cfg(test)]
    /// The link changes the preview shows now.
    pub fn shown_updates(&self) -> Vec<LinkUpdate> {
        match (&self.mode, &self.summary) {
            (Mode::Move { shown, .. }, Some(summary)) => {
                summary.updates.iter().take(*shown).cloned().collect()
            }
            _ => Vec::new(),
        }
    }

    pub fn is_busy(&self) -> bool {
        self.work.is_some()
    }

    pub fn can_confirm(&self) -> bool {
        !self.is_busy()
            && match &self.mode {
                Mode::Delete { plan, .. } => plan.is_some(),
                Mode::Rename { .. } | Mode::Move { .. } | Mode::Folder { .. } => true,
            }
    }

    fn title(&self) -> &'static str {
        match &self.mode {
            Mode::Delete { folder: false, .. } => "Delete page",
            Mode::Delete { folder: true, .. } => "Delete folder",
            Mode::Rename { .. } => "Rename page",
            Mode::Move {
                moving: Moving::Page(_),
                ..
            } => "Move page",
            Mode::Move {
                moving: Moving::Folder(_),
                ..
            } => "Rename or move folder",
            Mode::Folder { .. } => "New folder",
        }
    }

    pub fn confirm(&mut self, cx: &mut Context<Self>) {
        if !self.can_confirm() {
            return;
        }
        self.error = None;
        let work = match &mut self.mode {
            Mode::Folder { path, taken } => {
                match new_folder(&path.read(cx).text(cx), taken) {
                    Ok(folder) => {
                        cx.emit(FileDialogEvent::FolderCreated(folder));
                        cx.emit(DismissEvent);
                    }
                    Err(error) => self.error = Some(error.into()),
                }
                cx.notify();
                return;
            }
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
            Mode::Move { plan, .. } if plan.is_some() => {
                let Some(plan) = plan.take() else {
                    return;
                };
                self.move_now(plan, cx)
            }
            Mode::Move {
                moving,
                path,
                index,
                ..
            } => {
                let path = path.read(cx).text(cx);
                let request = match moving {
                    Moving::Page(page) => Relocate::Move {
                        page: page.clone(),
                        path,
                    },
                    Moving::Folder(folder) => Relocate::Folder {
                        folder: folder.clone(),
                        path,
                    },
                };
                let index = index.clone();
                self.preview_now(request, index, cx)
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
                    let moved = plan.moved.clone();
                    let written = cx.update(|cx| {
                        let (_, storage) = writers(cx);
                        file_actions::rename(plan, storage, cx)
                    });
                    written.await.map(|_| Some(moved))
                }
                Ok(None) => Ok(None),
                Err(error) => Err(error),
            };
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(moved) => {
                        if let Some(moved) = moved {
                            cx.emit(FileDialogEvent::Relocated(moved));
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

    /// Plans the move and shows what it changes; nothing is written yet.
    fn preview_now(
        &mut self,
        request: Relocate,
        index: IndexClient,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        let noteless = match &request {
            Relocate::Folder { folder, path } => Some((folder.clone(), path.clone())),
            _ => None,
        };
        let (open, storage) = (self.writers)(cx);
        let plan = file_actions::plan_relocation(request, open, index, storage, cx);
        cx.spawn(async move |this, cx| {
            let plan = plan.await;
            this.update(cx, |this, cx| {
                this.work = None;
                match plan {
                    Err(error) if error == quire_core::relocate::FOLDER_EMPTY => {
                        if let Some((from, to)) = noteless {
                            this.work = this.move_noteless(from, to, cx);
                        }
                    }
                    Ok(Some(plan)) => {
                        this.summary = Some(Summary::of_move(&plan));
                        if let Mode::Move {
                            plan: planned,
                            shown,
                            ..
                        } = &mut this.mode
                        {
                            *planned = Some(plan);
                            *shown = LISTED_CHANGES;
                        }
                    }
                    Ok(None) => {}
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
    }

    /// Moves a folder that holds no page, which has no links to preview: a folder notebook moves
    /// it on disk with its other files; a Quire notebook keeps such folders only here.
    fn move_noteless(
        &mut self,
        from: String,
        to: String,
        cx: &mut Context<Self>,
    ) -> Option<Task<()>> {
        let to = match quire_core::relocate::folder_path(&to) {
            Ok(to) => to,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return None;
            }
        };
        let (_, storage) = (self.writers)(cx);
        let request = storage
            .folder()
            .map(|folder| folder.move_noteless_folder(from.clone(), to.clone()));
        Some(cx.spawn(async move |this, cx| {
            let result = match request {
                Some(request) => crate::query_index::ask(request).await,
                None => Ok(()),
            };
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(()) => {
                        cx.emit(FileDialogEvent::FolderMoved { from, to });
                        cx.emit(DismissEvent);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }))
    }

    /// Applies the previewed move or nothing. A refusal drops the preview, so the next confirm
    /// previews again from the pages as they are now.
    fn move_now(&mut self, plan: RelocationPlan, cx: &mut Context<Self>) -> Task<()> {
        let (_, storage) = (self.writers)(cx);
        let moved = plan.moved.clone();
        let work = file_actions::relocate(plan, storage, file_actions::STALE_MOVE, cx);
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(_) => {
                        cx.emit(FileDialogEvent::Relocated(moved));
                        cx.emit(DismissEvent);
                    }
                    Err(error) => {
                        this.summary = None;
                        this.error = Some(error.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
    }

    /// Shows the next link changes of a long preview.
    pub fn show_more(&mut self, cx: &mut Context<Self>) {
        if let Mode::Move { shown, .. } = &mut self.mode {
            *shown += LISTED_CHANGES;
            cx.notify();
        }
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.is_busy() {
            cx.emit(DismissEvent);
        }
    }

    fn render_summary(&self, summary: &Summary, cx: &mut Context<Self>) -> Section {
        let shown = match &self.mode {
            Mode::Move { shown, .. } => *shown,
            _ => 0,
        };
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
            .when(!summary.updates.is_empty(), |section| {
                section
                    .child(Label::new(match summary.updates.len() {
                        1 => "1 link change:".to_owned(),
                        count => format!("{count} link changes:"),
                    }))
                    .children(summary.updates.iter().take(shown).map(|update| {
                        h_flex()
                            .gap_1()
                            .child(
                                Label::new(update.path.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(
                                Label::new(update.before.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Deleted)
                                    .strikethrough(),
                            )
                            .child(
                                Label::new(update.after.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Created),
                            )
                    }))
                    .when(summary.updates.len() > shown, |section| {
                        section.child(
                            Button::new("file-dialog-more", "Show more link changes")
                                .on_click(cx.listener(|this, _, _, cx| this.show_more(cx))),
                        )
                    })
            })
    }

    fn confirm_label(&self) -> &'static str {
        match (&self.mode, self.is_busy()) {
            (Mode::Delete { .. }, true) if self.summary.is_some() => "Deleting…",
            (Mode::Delete { .. }, _) => "Delete",
            (Mode::Rename { .. }, true) => "Renaming…",
            (Mode::Rename { .. }, false) => "Rename",
            (Mode::Move { .. }, true) if self.summary.is_some() => "Moving…",
            (Mode::Move { .. }, true) => "Previewing…",
            (Mode::Move { plan: Some(_), .. }, false) => "Move",
            (Mode::Move { .. }, false) => "Preview",
            (Mode::Folder { .. }, _) => "Create folder",
        }
    }

    fn render_field(field: &Entity<Editor>, help: &'static str, cx: &App) -> Section {
        Section::new()
            .child(
                div()
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .p_1()
                    .child(field.clone()),
            )
            .child(Label::new(help).size(LabelSize::Small).color(Color::Muted))
    }
}

impl Render for FileDialog {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let style = match self.mode {
            Mode::Delete { .. } => ButtonStyle::Tinted(ui::TintColor::Error),
            Mode::Rename { .. } | Mode::Move { .. } | Mode::Folder { .. } => ButtonStyle::Filled,
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
            (Mode::Folder { path, .. }, _) => {
                modal = modal.section(Self::render_field(
                    path,
                    "Use / to separate folders. The folder stays on this computer until a page is saved in it.",
                    cx,
                ))
            }
            (Mode::Rename { name, .. }, _) => {
                modal = modal.section(Self::render_field(
                    name,
                    "The file name and title change together; links to this page follow.",
                    cx,
                ))
            }
            (Mode::Move { path, moving, .. }, summary) => {
                let help = match moving {
                    Moving::Page(_) => {
                        "Use / to separate folders. The page title and block IDs stay the same; this changes its filename or folder."
                    }
                    Moving::Folder(_) => {
                        "Use / to separate folders. Every page inside moves and links to it follow; other files stay where they are."
                    }
                };
                modal = modal.section(Self::render_field(path, help, cx));
                if let Some(summary) = summary {
                    modal = modal.section(self.render_summary(summary, cx));
                }
            }
            (Mode::Delete { .. }, Some(summary)) => {
                modal = modal.section(self.render_summary(summary, cx))
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
