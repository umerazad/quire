//! Backups of the open notebook: the backup folder, Back up now, a backup every hour while the
//! notebook is open, the list of completed backups with Verify, and Restore as a new notebook.
//! The work runs in `quire_store::backup`, off the UI thread.

use gpui::{
    App, AppContext as _, Context, EventEmitter, IntoElement, ParentElement, PathPromptOptions,
    PromptLevel, Render, SharedString, Styled, Task, Window,
};
use quire_store::backup::{self, BackupInfo, Location, RestoreRequest, Source};
use quire_store::profiles::Notebook;
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use ui::{Button, Clickable, Label, LabelCommon as _, prelude::FluentBuilder as _};

gpui::actions!(
    quire_backup,
    [
        /// Shows the backup folder, the completed backups, and their Verify and Restore actions.
        ShowBackups,
        /// Chooses the folder that receives this notebook's backups.
        ChooseBackupFolder,
        /// Backs up this notebook now.
        BackUpNow,
    ]
);

/// How often a notebook is backed up while it is open, as in the reference app.
pub const INTERVAL: Duration = Duration::from_secs(60 * 60);
/// How long an automatic backup waits after a failed attempt.
pub const RETRY: Duration = Duration::from_secs(15 * 60);
const TICK: Duration = Duration::from_secs(60);

pub const BUSY: &str = "A backup or restore is already running. Wait for it to finish.";
pub const NO_FOLDER: &str = "Choose a backup folder before backing up this notebook.";
pub const UNCHANGED: &str = "Nothing changed since the last backup, so the hourly backup was skipped. Back up now makes one anyway.";

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn folder_prompt(prompt: &str) -> PathPromptOptions {
    PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(prompt.to_owned().into()),
    }
}

type Chosen = futures::channel::oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>>;

/// The folder a folder dialog returned, or None when it was dismissed or failed.
async fn first_path(chosen: Chosen) -> Option<PathBuf> {
    chosen.await.ok()?.ok()??.into_iter().next()
}

fn describe(error: anyhow::Error) -> String {
    format!("{error:#}")
}

/// The backup folder of the profile holding `notebook`.
fn profile_folder(notebook: Option<&Notebook>, cx: &App) -> Option<PathBuf> {
    let id = crate::profiles::holding(notebook?, cx)?;
    crate::profiles::get(id, cx).backup_folder
}

pub const NO_PROFILE: &str = "This notebook belongs to no profile, so it has no backup folder. Open it from File > Profile, or close the notebook that holds its profile, then choose a folder.";

/// A restore finished: the window opens the restored notebook.
pub struct Restored(pub Location);

pub struct Backups {
    source: Source,
    /// The notebook, whose profile keeps the backup folder and the outcome of the last backup.
    notebook: Option<Notebook>,
    destination: Option<PathBuf>,
    /// Quire's state folder for a restored folder notebook at a given root.
    state_for: Arc<dyn Fn(&Path) -> PathBuf + Send + Sync>,
    /// Formats milliseconds since the epoch for people.
    label: Rc<dyn Fn(i64) -> String>,
    running: Option<SharedString>,
    message: Option<SharedString>,
    /// Completed backups, newest first, while the panel is shown.
    listed: Option<Vec<BackupInfo>>,
    /// When the newest backup completed, or an hourly check found nothing changed since it, on
    /// the executor's clock.
    completed: Option<Instant>,
    attempted: Option<Instant>,
    _schedule: Task<()>,
}

impl EventEmitter<Restored> for Backups {}

impl Backups {
    pub fn new(
        source: Source,
        notebook: Option<Notebook>,
        state_for: Arc<dyn Fn(&Path) -> PathBuf + Send + Sync>,
        label: Rc<dyn Fn(i64) -> String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let destination = profile_folder(notebook.as_ref(), cx);
        let schedule = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(TICK).await;
                if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                    break;
                }
            }
        });
        let mut this = Self {
            source,
            notebook,
            destination,
            state_for,
            label,
            running: None,
            message: None,
            listed: None,
            completed: None,
            attempted: None,
            _schedule: schedule,
        };
        this.load_list(false, cx);
        this
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    /// Takes up the profile's backup folder when it changed elsewhere, or drops it when this
    /// notebook no longer belongs to a profile.
    fn follow_profile(&mut self, cx: &mut Context<Self>) {
        let folder = profile_folder(self.notebook.as_ref(), cx);
        if folder != self.destination {
            self.destination = folder;
            self.completed = None;
            self.attempted = None;
            self.load_list(self.listed.is_some(), cx);
        }
    }

    fn record(&self, outcome: Result<(u64, PathBuf), String>, cx: &mut Context<Self>) {
        if let Some(id) = self
            .notebook
            .as_ref()
            .and_then(|notebook| crate::profiles::holding(notebook, cx))
            && let Err(error) =
                crate::profiles::update(cx, |profiles| profiles.record_backup(id, outcome))
        {
            crate::outline::show_notice(error, cx);
        }
    }

    #[cfg(test)]
    pub fn destination(&self) -> Option<&Path> {
        self.destination.as_deref()
    }

    pub fn message(&self) -> Option<SharedString> {
        self.running.clone().or_else(|| self.message.clone())
    }

    #[cfg(test)]
    pub fn listed(&self) -> Option<&[BackupInfo]> {
        self.listed.as_deref()
    }

    fn busy(&mut self, cx: &mut Context<Self>) -> bool {
        if self.is_running() {
            self.message = Some(BUSY.into());
            cx.notify();
        }
        self.is_running()
    }

    fn start(&mut self, label: &str, cx: &mut Context<Self>) {
        self.running = Some(label.to_owned().into());
        self.message = None;
        cx.notify();
    }

    fn finish(&mut self, message: String, cx: &mut Context<Self>) {
        self.running = None;
        self.message = Some(message.into());
        cx.notify();
    }

    /// The backup schedule: once an hour since the last completed backup, and no sooner than
    /// fifteen minutes after a failed attempt.
    fn tick(&mut self, cx: &mut Context<Self>) {
        self.follow_profile(cx);
        let now = cx.background_executor().now();
        let due = |since: Option<Instant>, wait: Duration| {
            since.is_none_or(|since| now.saturating_duration_since(since) >= wait)
        };
        if self.destination.is_some()
            && !self.is_running()
            && due(self.completed, INTERVAL)
            && due(self.attempted, RETRY)
        {
            self.back_up(true, cx);
        }
    }

    /// Shows or hides the list of completed backups.
    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.listed.take().is_none() {
            self.load_list(true, cx);
        }
        cx.notify();
    }

    fn load_list(&mut self, show: bool, cx: &mut Context<Self>) {
        let Some(destination) = self.destination.clone() else {
            if show {
                self.listed = Some(Vec::new());
            }
            return;
        };
        let source = self.source.clone();
        let work = cx.background_spawn(async move { backup::list(&source, &destination) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(backups) => {
                        this.note_completed(backups.first(), cx);
                        if show || this.listed.is_some() {
                            this.listed = Some(backups);
                        }
                    }
                    Err(error) => {
                        this.message =
                            Some(format!("Cannot list backups: {}", describe(error)).into());
                        if show {
                            this.listed = Some(Vec::new());
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Records when the newest backup was made on the executor's clock, so the schedule counts
    /// from it.
    fn note_completed(&mut self, newest: Option<&BackupInfo>, cx: &mut Context<Self>) {
        if let Some(newest) = newest {
            let age = Duration::from_millis(now_millis().saturating_sub(newest.created));
            let now = cx.background_executor().now();
            let at = now.checked_sub(age).unwrap_or(now);
            if self.completed.is_none_or(|completed| completed < at) {
                self.completed = Some(at);
            }
        }
    }

    /// Asks for the folder that receives this notebook's backups.
    pub fn choose_folder(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(folder_prompt("Use for backups"));
        cx.spawn(async move |this, cx| {
            let Some(folder) = first_path(chosen).await else {
                return;
            };
            this.update(cx, |this, cx| this.set_destination(&folder, cx))
                .ok();
        })
        .detach();
    }

    pub fn set_destination(&mut self, folder: &Path, cx: &mut Context<Self>) {
        let profile = self
            .notebook
            .as_ref()
            .and_then(|notebook| crate::profiles::holding(notebook, cx));
        let result = backup::check_destination(&self.source, folder)
            .map_err(describe)
            .and_then(|folder| {
                if folder.starts_with(paths::data_dir()) {
                    return Err("Choose a backup folder outside Quire's application data.".into());
                }
                let id = profile.ok_or_else(|| NO_PROFILE.to_owned())?;
                crate::profiles::update(cx, |profiles| profiles.set_backup_folder(id, &folder))?;
                Ok(folder)
            });
        match result {
            Ok(folder) => {
                self.message = Some(format!("Backups go to {}.", folder.display()).into());
                self.destination = Some(folder);
                self.completed = None;
                self.attempted = None;
                self.load_list(self.listed.is_some(), cx);
            }
            Err(error) => {
                self.message = Some(format!("Cannot use that backup folder: {error}").into());
            }
        }
        cx.notify();
    }

    /// Backs up the notebook. An automatic backup is skipped when nothing changed since the newest
    /// backup ([`backup::create_if_changed`]), and one that fails waits [`RETRY`] before trying again.
    pub fn back_up(&mut self, automatic: bool, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        self.follow_profile(cx);
        let Some(destination) = self.destination.clone() else {
            self.message = Some(NO_FOLDER.into());
            cx.notify();
            return;
        };
        self.attempted = Some(cx.background_executor().now());
        self.start("Backing up the notebook…", cx);
        let source = self.source.clone();
        let created = now_millis();
        let work = cx.background_spawn(async move {
            if automatic {
                backup::create_if_changed(&source, &destination, created)
            } else {
                backup::create(&source, &destination, created).map(Some)
            }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let message = match result {
                    Ok(None) => {
                        this.completed = Some(cx.background_executor().now());
                        UNCHANGED.to_owned()
                    }
                    Ok(Some(backup)) => {
                        this.completed = Some(cx.background_executor().now());
                        this.load_list(false, cx);
                        this.record(Ok((backup.created, backup.path.clone())), cx);
                        format!(
                            "Backed up at {} to {}.",
                            (this.label)(backup.created as i64),
                            backup.path.display()
                        )
                    }
                    Err(error) => {
                        let error = describe(error);
                        this.record(Err(error.clone()), cx);
                        if automatic {
                            format!(
                                "Automatic backup failed: {error}. Open Backups and back up now to retry."
                            )
                        } else {
                            format!("Backup failed: {error}")
                        }
                    }
                };
                this.finish(message, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Checks a backup completely without changing it.
    pub fn verify(&mut self, backup: BackupInfo, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        self.start("Verifying the backup…", cx);
        let work = cx.background_spawn({
            let path = backup.path.clone();
            async move { backup::verify(&path) }
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let when = (this.label)(backup.created as i64);
                let message = match result {
                    Ok(manifest) => format!(
                        "The backup from {when} is complete: {} files match their recorded hashes and every database passed its checks.",
                        manifest.files.len()
                    ),
                    Err(error) => format!("The backup from {when} failed verification: {}", describe(error)),
                };
                this.finish(message, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Restores `backup` as a new notebook after the user confirms and chooses where, backing up
    /// this notebook first. This notebook is never changed.
    pub fn restore(&mut self, backup: BackupInfo, window: &mut Window, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        let Some(destination) = self.destination.clone() else {
            self.message = Some(NO_FOLDER.into());
            cx.notify();
            return;
        };
        let when = (self.label)(backup.created as i64);
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Restore the backup from {when} as a new notebook?"),
            Some(
                "Quire first backs up this notebook, then verifies the backup and restores it into a new folder you choose, and opens it. This notebook stays unchanged.",
            ),
            &["Restore as a new notebook", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let Ok(chosen) =
                this.update(cx, |_, cx| cx.prompt_for_paths(folder_prompt("Restore here")))
            else {
                return;
            };
            let Some(parent) = first_path(chosen).await else {
                return;
            };
            let Ok((source, state_for)) = this.update(cx, |this, cx| {
                this.start("Restoring the backup as a new notebook…", cx);
                (this.source.clone(), this.state_for.clone())
            }) else {
                return;
            };
            let created = now_millis();
            let result = cx
                .background_spawn(async move {
                    backup::restore(
                        &RestoreRequest {
                            backup: &backup.path,
                            parent: &parent,
                            current: Some((&source, &destination)),
                            state_for: &*state_for,
                            created,
                        },
                        &mut |_| Ok(()),
                    )
                })
                .await;
            this.update(cx, |this, cx| {
                let message = match result {
                    Ok(restored) => {
                        this.load_list(false, cx);
                        let place = match &restored.location {
                            Location::Store { database, .. } => {
                                database.parent().unwrap_or(database.as_path()).to_path_buf()
                            }
                            Location::Folder { root, .. } => root.clone(),
                        };
                        cx.emit(Restored(restored.location));
                        format!(
                            "Restored the backup from {when} into {}. This notebook was backed up first and is unchanged.",
                            place.display()
                        )
                    }
                    Err(error) => format!("Restore failed: {}", describe(error)),
                };
                this.finish(message, cx);
            })
            .ok();
        })
        .detach();
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.is_running() {
            self.message = None;
            cx.notify();
        }
    }
}

impl Render for Backups {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message();
        let running = self.is_running();
        let panel = self.listed.clone().map(|backups| {
            let folder = self.destination.as_ref().map_or_else(
                || "No backup folder chosen".to_owned(),
                |folder| folder.display().to_string(),
            );
            let newest = backups
                .first()
                .map_or_else(|| "none yet".to_owned(), |backup| (self.label)(backup.created as i64));
            gpui::div()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .child(Label::new(format!("Backup folder: {folder}")))
                .child(
                    gpui::div()
                        .flex()
                        .gap_2()
                        .child(Button::new("choose-backup-folder", "Choose backup folder…").on_click(
                            cx.listener(|this, _, _, cx| this.choose_folder(cx)),
                        ))
                        .when(self.destination.is_some(), |div| {
                            div.child(Button::new("back-up-now", "Back up now").on_click(
                                cx.listener(|this, _, _, cx| this.back_up(false, cx)),
                            ))
                        })
                        .child(
                            Button::new("hide-backups", "Hide backups")
                                .on_click(cx.listener(|this, _, _, cx| this.toggle(cx))),
                        ),
                )
                .child(Label::new(
                    "Backups include pages, history, recovery drafts, and attachments. Quire backs up every hour while this notebook is open and keeps every completed backup. Choose a folder on another drive.",
                ).size(ui::LabelSize::Small))
                .child(Label::new(format!("Last completed backup: {newest}")))
                .children(backups.into_iter().enumerate().map(|(index, backup)| {
                    let label = format!("{} · {}", (self.label)(backup.created as i64), backup.source);
                    let verify = backup.clone();
                    gpui::div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Label::new(label))
                        .child(Button::new(("verify-backup", index), "Verify").on_click(
                            cx.listener(move |this, _, _, cx| this.verify(verify.clone(), cx)),
                        ))
                        .child(Button::new(("restore-backup", index), "Restore…").on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.restore(backup.clone(), window, cx)
                            }),
                        ))
                }))
        });
        gpui::div()
            .flex()
            .flex_col()
            .when_some(message, |div, message| {
                div.child(
                    gpui::div()
                        .p_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(Label::new(message))
                        .when(!running, |div| {
                            div.child(
                                Button::new("dismiss-backup-message", "Dismiss")
                                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                            )
                        }),
                )
            })
            .when_some(panel, |div, panel| div.child(panel))
    }
}
