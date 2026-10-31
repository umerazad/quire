//! Settings > Data: the active profile, its notebook, backups, Personal Git, import and export,
//! and the move from or back to the Electron app. Profile records are read from the shared
//! profile store; commands that need the open notebook run in that notebook's window, so Settings
//! and the window's own buttons stay one set of actions.

use crate::tab_order::{self, TabOrder};
use crate::{
    backups::{BackUpNow, ChooseBackupFolder, ShowBackups},
    exchange::{ExportNotebookMarkdown, ExportRoamJson, ImportMarkdownFolder, ImportRoamData},
    profiles::{
        OpenNotebookDatabase, OpenNotebookFolder, UseCurrentProfile, UsePersonalProfile,
        UseWorkProfile,
    },
    sync_status::SyncWithGit,
};
use editor::Editor;
use gpui::{Action, App, Context, Entity, SharedString, Window, prelude::*};
use quire_core::daily::DailyCalendar;
use quire_store::profiles::{Notebook, ProfileId};
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, Disableable as _, Label, LabelCommon as _,
    LabelSize, ToggleButtonGroup, ToggleButtonSimple, Tooltip, h_flex, v_flex,
};

pub const NO_NOTEBOOK: &str = "Choose a notebook folder";
pub const NO_BACKUP_FOLDER: &str = "No backup destination configured";

pub struct DataSection {
    remote: Entity<Editor>,
    shown_remote: Option<String>,
    message: Option<SharedString>,
    error: Option<SharedString>,
}

fn profile_action(id: ProfileId) -> Box<dyn Action> {
    match id {
        ProfileId::Current => Box::new(UseCurrentProfile),
        ProfileId::Work => Box::new(UseWorkProfile),
        ProfileId::Personal => Box::new(UsePersonalProfile),
    }
}

/// A time in this computer's time zone, as the profile line shows it.
pub fn when(at: Option<u64>) -> String {
    let Some(at) = at else {
        return "Not completed yet".into();
    };
    DailyCalendar::system()
        .and_then(|calendar| calendar.clock_at(at as i64))
        .map(|(date, hour, minute)| format!("{date} {hour:02}:{minute:02}"))
        .unwrap_or_else(|_| format!("{at} ms after 1970 (the time zone could not be read)"))
}

fn describe(notebook: &Notebook) -> String {
    match notebook {
        Notebook::Folder { root } => root.display().to_string(),
        Notebook::Store { database, .. } => format!("Quire notebook {}", database.display()),
    }
}

impl DataSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shown_remote = Self::profile_remote(cx);
        let remote = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("git@github.com:you/personal-notes.git", window, cx);
            editor.set_text(shown_remote.clone().unwrap_or_default(), window, cx);
            editor
        });
        cx.observe_global_in::<crate::profiles::ProfileStore>(window, |this, window, cx| {
            this.follow_remote(window, cx);
            cx.notify();
        })
        .detach();
        Self {
            remote,
            shown_remote,
            message: None,
            error: None,
        }
    }

    fn profile_remote(cx: &App) -> Option<String> {
        crate::profiles::get(ProfileId::Personal, cx).git_remote
    }

    #[cfg(test)]
    pub fn remote_field(&self) -> &Entity<Editor> {
        &self.remote
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    #[cfg(test)]
    pub fn message(&self) -> Option<&SharedString> {
        self.message.as_ref()
    }

    /// Shows a remote saved elsewhere, such as in the notebook window's Git panel.
    fn follow_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let saved = Self::profile_remote(cx);
        if saved != self.shown_remote {
            self.shown_remote = saved.clone();
            self.remote.update(cx, |editor, cx| {
                editor.set_text(saved.unwrap_or_default(), window, cx)
            });
        }
    }

    /// Saves the typed remote as Personal's, as the reference "Save Git remote" does.
    pub fn save_remote(&mut self, cx: &mut Context<Self>) {
        let typed = self.remote.read(cx).text(cx);
        match crate::profiles::update(cx, |profiles| {
            profiles.set_git_remote(ProfileId::Personal, &typed)
        }) {
            Ok(remote) => {
                self.shown_remote = remote.clone();
                self.error = None;
                self.message = Some(match remote {
                    Some(remote) => format!("Git sync uses {remote}. Credentials come from Git's own credential helper or SSH agent.").into(),
                    None => "Personal Git is off.".into(),
                });
            }
            Err(error) => {
                self.message = None;
                self.error = Some(error.into());
            }
        }
        cx.notify();
    }

    /// Runs `action` in the window of the active profile's notebook.
    pub fn in_notebook(&mut self, action: Box<dyn Action>, cx: &mut Context<Self>) {
        self.message = None;
        self.error = crate::editor_host::dispatch_to_notebook(action, cx)
            .err()
            .map(Into::into);
        cx.notify();
    }

    fn command(
        &self,
        id: &'static str,
        label: &'static str,
        tooltip: &'static str,
        action: impl Action,
        order: &mut TabOrder,
        cx: &mut Context<Self>,
    ) -> Button {
        let action = action.boxed_clone();
        Button::new(id, label)
            .tab_index(order.next())
            .tooltip(Tooltip::text(tooltip))
            .on_click(cx.listener(move |this, _, _, cx| this.in_notebook(action.boxed_clone(), cx)))
    }

    fn app_command(
        id: &'static str,
        label: &'static str,
        action: impl Action,
        order: &mut TabOrder,
    ) -> Button {
        let action = action.boxed_clone();
        Button::new(id, label)
            .tab_index(order.next())
            .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
    }

    fn hint(text: impl Into<SharedString>) -> Label {
        Label::new(text).size(LabelSize::Small).color(Color::Muted)
    }

    fn path(title: &'static str, value: String) -> gpui::Div {
        v_flex()
            .child(Label::new(title))
            .child(Label::new(value).size(LabelSize::Small))
    }
}

impl Render for DataSection {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut order = TabOrder::default();
        let active = crate::profiles::active(cx);
        let profile = crate::profiles::get(active, cx);
        let profiles = ProfileId::ALL.map(|id| {
            let action = profile_action(id);
            ToggleButtonSimple::new(id.label(), move |_, window, cx| {
                window.dispatch_action(action.boxed_clone(), cx)
            })
        });
        let selected = ProfileId::ALL
            .iter()
            .position(|id| *id == active)
            .unwrap_or_default();
        let notebook = profile
            .notebook
            .as_ref()
            .map_or(NO_NOTEBOOK.into(), describe);
        let backup_folder = profile
            .backup_folder
            .as_ref()
            .map_or(NO_BACKUP_FOLDER.into(), |folder| {
                folder.display().to_string()
            });
        let has_backup_folder = profile.backup_folder.is_some();
        v_flex()
            .id("data-settings")
            .gap_2()
            .pt_3()
            .child(Label::new("Notebooks and durability").size(LabelSize::Large))
            .child(Label::new("Active profile"))
            .child(ToggleButtonGroup::single_row("active-profile", profiles)
                    .selected_index(selected)
                    .tab_index(order.counter()))
            .child(Self::hint(
                "Work and Personal use separate notebooks. Current keeps your earlier notebook. Switching saves open pages first.",
            ))
            .child(Self::path("Live notebook", notebook))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(Self::app_command(
                        "choose-notebook-folder",
                        "Choose notebook folder",
                        OpenNotebookFolder,
                        &mut order,
                    ))
                    .child(Self::app_command(
                        "choose-quire-notebook",
                        "Choose Quire notebook",
                        OpenNotebookDatabase,
                        &mut order,
                    )),
            )
            .child(Self::hint(
                "A Quire notebook keeps pages in a database on this computer; choose its local.sqlite file.",
            ))
            .child(Label::new("Import and export"))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.command(
                        "data-import-markdown",
                        "Import a Markdown folder",
                        "Copy a folder of Markdown pages and their files into the open notebook",
                        ImportMarkdownFolder,
                        &mut order,
                        cx,
                    ))
                    .child(self.command(
                        "data-export-markdown",
                        "Export notebook as Markdown",
                        "Write every page and file of the open notebook into a folder",
                        ExportNotebookMarkdown,
                        &mut order,
                        cx,
                    ))
                    .child(self.command(
                        "data-import-roam",
                        "Import Roam data",
                        "Read a Roam JSON or EDN export into a new folder notebook",
                        ImportRoamData,
                        &mut order,
                        cx,
                    ))
                    .child(self.command(
                        "data-export-roam",
                        "Export Roam JSON",
                        "Write every page of the open notebook into a Roam JSON file",
                        ExportRoamJson,
                        &mut order,
                        cx,
                    )),
            )
            .child(Self::path("Independent backups", backup_folder))
            .child(Self::hint(
                "Completed snapshots include notes, attachments and local recovery/history. Backups run hourly while this profile is open and keep every completed snapshot. Choose a separate backup drive or approved backup location.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(self.command(
                        "data-choose-backup-folder",
                        "Choose backup destination",
                        "Choose the folder that receives this profile's backups",
                        ChooseBackupFolder,
                        &mut order,
                        cx,
                    ))
                    .child(
                        self.command(
                            "data-back-up-now",
                            "Back up now",
                            "Back up the open notebook now",
                            BackUpNow,
                            &mut order,
                            cx,
                        )
                        .disabled(!has_backup_folder),
                    )
                    .child(
                        self.command(
                            "data-browse-backups",
                            "Browse backups",
                            "List completed backups with Verify and Restore in the notebook window",
                            ShowBackups,
                            &mut order,
                            cx,
                        )
                        .disabled(!has_backup_folder),
                    ),
            )
            .child(Self::hint(format!(
                "Last completed backup: {}",
                when(profile.last_backup_at)
            )))
            .when_some(profile.last_backup_path.clone(), |this, path| {
                this.child(Self::hint(path.display().to_string()))
            })
            .when_some(profile.backup_error.clone(), |this, error| {
                this.child(Label::new(format!("Backup failed: {error}")).color(Color::Error))
            })
            .when(active == ProfileId::Personal, |this| {
                let saved = profile.git_remote.clone();
                let typed = self.remote.read(cx).text(cx);
                let unsaved = typed.trim() != saved.clone().unwrap_or_default();
                this.child(Label::new("Personal Git").size(LabelSize::Large))
                    .child(Label::new("Private repository remote"))
                    .child(
                        tab_order::field(&self.remote, &mut order, window, cx),
                    )
                    .child(Self::hint(
                        "Use an empty remote for existing notes, or open a clone of your personal repository. Git must be installed with your identity and credentials configured. Git respects .gitignore. Sync backs up first when a backup folder is set, pulls safe fast-forwards, commits local files and pushes. Divergence requires review in Git.",
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("data-save-remote", "Save Git remote")
                                    .tab_index(order.next())
                                    .on_click(cx.listener(|this, _, _, cx| this.save_remote(cx))),
                            )
                            .child(
                                self.command(
                                    "data-sync-git",
                                    "Sync Personal now",
                                    "Back up, commit, pull, and push the Personal notebook",
                                    SyncWithGit,
                                    &mut order,
                                    cx,
                                )
                                .disabled(saved.is_none() || unsaved),
                            ),
                    )
                    .child(Self::hint(format!(
                        "Last successful push: {}",
                        when(profile.last_push_at)
                    )))
                    .when_some(profile.git_error.clone(), |this, error| {
                        this.child(
                            Label::new(format!("Git sync failed: {error}")).color(Color::Error),
                        )
                    })
            })
            .child(Label::new("Moving from the Electron app"))
            .child(
                h_flex().child(Self::app_command(
                    "data-rt-migration",
                    "Move from rt or back",
                    crate::rt_migration::view::OpenMigration,
                        &mut order,
                )),
            )
            .child(Self::hint(
                "Saved locally confirms this computer's write. Check that your cloud folder finished uploading before switching computers.",
            ))
            .when_some(self.message.clone(), |this, message| {
                this.child(Label::new(message))
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).color(Color::Error))
            })
    }
}
