//! Backups of the open notebook: the backup folder, Back up now, a backup every hour while the
//! notebook is open, the list of completed backups with Verify, and Restore as a new notebook.
//! The work runs in `quire_store::backup`, off the UI thread.

use gpui::{
    AppContext as _, Context, EventEmitter, IntoElement, ParentElement, PathPromptOptions,
    PromptLevel, Render, SharedString, Styled, Task, Window,
};
use quire_store::backup::{self, BackupInfo, Location, RestoreRequest, Source};
use std::{
    collections::BTreeMap,
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

/// Where this computer keeps each notebook's backup folder: a JSON object from the notebook's
/// backup key to the folder. Without a file (tests), choices last until the window closes.
#[derive(Clone)]
pub struct Settings(Option<PathBuf>);

impl Settings {
    pub fn new(file: Option<PathBuf>) -> Self {
        Self(file)
    }

    fn entries(&self) -> BTreeMap<String, PathBuf> {
        self.0
            .as_ref()
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn load(&self, key: &str) -> Option<PathBuf> {
        self.entries().remove(key)
    }

    fn save(&self, key: &str, folder: &Path) -> Result<(), String> {
        let Some(file) = &self.0 else {
            return Ok(());
        };
        let mut entries = self.entries();
        entries.insert(key.to_owned(), folder.to_owned());
        let text = serde_json::to_vec_pretty(&entries).map_err(|error| error.to_string())?;
        let temporary = file.with_extension("json.tmp");
        std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
            .and_then(|()| std::fs::write(&temporary, text))
            .and_then(|()| std::fs::rename(&temporary, file))
            .map_err(|error| {
                format!(
                    "Cannot remember the backup folder in {}: {error}. Check write access to Quire's application data and choose the folder again.",
                    file.display()
                )
            })
    }
}

/// A restore finished: the window opens the restored notebook.
pub struct Restored(pub Location);

pub struct Backups {
    source: Source,
    settings: Settings,
    destination: Option<PathBuf>,
    /// Quire's state folder for a restored folder notebook at a given root.
    state_for: Arc<dyn Fn(&Path) -> PathBuf + Send + Sync>,
    /// Formats milliseconds since the epoch for people.
    label: Rc<dyn Fn(i64) -> String>,
    running: Option<SharedString>,
    message: Option<SharedString>,
    /// Completed backups, newest first, while the panel is shown.
    listed: Option<Vec<BackupInfo>>,
    /// When the newest backup completed, on the executor's clock.
    completed: Option<Instant>,
    attempted: Option<Instant>,
    _schedule: Task<()>,
}

impl EventEmitter<Restored> for Backups {}

impl Backups {
    pub fn new(
        source: Source,
        settings: Settings,
        state_for: Arc<dyn Fn(&Path) -> PathBuf + Send + Sync>,
        label: Rc<dyn Fn(i64) -> String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let destination = source.key().ok().and_then(|key| settings.load(&key));
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
            settings,
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
        let result = backup::check_destination(&self.source, folder)
            .and_then(|folder| Ok((self.source.key()?, folder)))
            .map_err(describe)
            .and_then(|(key, folder)| {
                self.settings.save(&key, &folder)?;
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

    /// Backs up the notebook. An automatic backup that fails waits [`RETRY`] before trying again.
    pub fn back_up(&mut self, automatic: bool, cx: &mut Context<Self>) {
        if self.busy(cx) {
            return;
        }
        let Some(destination) = self.destination.clone() else {
            self.message = Some(NO_FOLDER.into());
            cx.notify();
            return;
        };
        self.attempted = Some(cx.background_executor().now());
        self.start("Backing up the notebook…", cx);
        let source = self.source.clone();
        let created = now_millis();
        let work =
            cx.background_spawn(async move { backup::create(&source, &destination, created) });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let message = match result {
                    Ok(backup) => {
                        this.completed = Some(cx.background_executor().now());
                        this.load_list(false, cx);
                        format!(
                            "Backed up at {} to {}.",
                            (this.label)(backup.created as i64),
                            backup.path.display()
                        )
                    }
                    Err(error) if automatic => format!(
                        "Automatic backup failed: {}. Open Backups and back up now to retry.",
                        describe(error)
                    ),
                    Err(error) => format!("Backup failed: {}", describe(error)),
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
