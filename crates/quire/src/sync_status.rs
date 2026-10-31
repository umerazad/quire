//! The notebook's sync status: which provider synchronizes the folder, notes it keeps only in the
//! cloud, the conflict copies it made, and Personal Git with its remote, last push, and last
//! error. The checks and Git run in `quire_store`, off the UI thread. Git sync belongs to the
//! Personal profile: its remote and results live in the profile, and each sync backs the notebook
//! up first when the profile has a backup folder, as the reference app does.

use editor::Editor;
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Styled, Window,
};
use quire_store::{
    backup::{self, Source},
    git_sync::{self, Git, Target},
    profiles::{Notebook, Profile, ProfileId},
    provider::{self, Provider, Scan},
    worker::StorageClient,
};
use std::{
    path::PathBuf,
    rc::Rc,
    time::{SystemTime, UNIX_EPOCH},
};
use ui::{Button, Clickable, Label, prelude::FluentBuilder as _};

gpui::actions!(
    quire_sync,
    [
        /// Shows where the notebook is synchronized, its cloud-only notes and conflict copies,
        /// and Personal Git.
        ShowSync,
        /// Backs up this notebook, commits it, brings in the Personal remote's notes, and pushes.
        SyncWithGit,
        /// Looks again for notes kept only in the cloud and for conflict copies.
        CheckSyncFolder,
    ]
);

pub const BUSY: &str = "Git sync is already running. Wait for it to finish.";
pub const NO_FOLDER: &str = "Git sync and provider checks need the notebook's folder. Open a folder notebook, or a Quire notebook with its synced folder, to sync it with Git.";
pub const NOT_PERSONAL: &str = "Git sync is available only in Personal, so Work notes never reach a personal remote. Choose File > Profile > Personal and open this notebook there, then enter the remote.";

/// A Git sync changed notes in the folder: the window rereads them.
pub struct Synced;

/// What the synced folder holds, which decides how a Git sync backs it up and merges it.
#[derive(Clone)]
pub enum Kind {
    /// A Markdown folder notebook, with Quire's state folder for it in application data.
    Folder { state: PathBuf },
    /// A Quire notebook's synced folder. Its saves publish there before each Git sync, and
    /// other computers' change files merge in Git because each writer adds only its own.
    Quire {
        database: PathBuf,
        client: StorageClient,
    },
}

/// Why a Git sync stopped before Git ran.
enum Stopped {
    Backup(String),
    Publication(String),
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

pub struct SyncStatus {
    root: Option<PathBuf>,
    /// The notebook, whose profile keeps the remote and the last push.
    notebook: Option<Notebook>,
    /// How the folder is backed up and merged.
    kind: Option<Kind>,
    git: Git,
    provider: Option<Provider>,
    scan: Option<Result<Scan, String>>,
    warnings: Vec<String>,
    remote: Option<Entity<Editor>>,
    shown: bool,
    running: Option<SharedString>,
    message: Option<SharedString>,
    /// Formats milliseconds since the epoch for people.
    label: Rc<dyn Fn(i64) -> String>,
}

impl EventEmitter<Synced> for SyncStatus {}

impl SyncStatus {
    pub fn new(
        root: Option<(PathBuf, Kind)>,
        notebook: Option<Notebook>,
        git: Git,
        label: Rc<dyn Fn(i64) -> String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let (root, kind) = root.unzip();
        let mut this = Self {
            provider: root.as_deref().and_then(provider::detect),
            root,
            notebook,
            kind,
            git,
            scan: None,
            warnings: Vec::new(),
            remote: None,
            shown: false,
            running: None,
            message: None,
            label,
        };
        this.check(cx);
        this
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    #[cfg(test)]
    pub fn is_shown(&self) -> bool {
        self.shown
    }

    pub fn message(&self) -> Option<SharedString> {
        self.running.clone().or_else(|| self.message.clone())
    }

    /// The profile holding this notebook.
    pub fn profile(&self, cx: &App) -> Option<ProfileId> {
        crate::profiles::holding(self.notebook.as_ref()?, cx)
    }

    /// The profile's remote, last push, and last Git error.
    pub fn record(&self, cx: &App) -> Profile {
        self.profile(cx)
            .map(|id| crate::profiles::get(id, cx))
            .unwrap_or_default()
    }

    /// Looks for cloud-only notes and conflict copies, and keeps live data out of a repository
    /// at the notebook folder, as opening a notebook does in the reference app.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let git = self.git.clone();
        let folder = matches!(self.kind, Some(Kind::Folder { .. }));
        let work = cx.background_spawn(async move {
            let scan = provider::scan(&root).map_err(|error| format!("{error:#}"));
            let warnings = if folder {
                git_sync::protect_notebook(&git, &root)
            } else {
                Vec::new()
            };
            (scan, warnings)
        });
        cx.spawn(async move |this, cx| {
            let (scan, warnings) = work.await;
            this.update(cx, |this, cx| {
                this.scan = Some(scan);
                this.warnings = warnings;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn toggle(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.shown = !self.shown;
        if self.shown {
            if self.remote.is_none() {
                let text = self.record(cx).git_remote.unwrap_or_default();
                self.remote = Some(cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text("git@example.com:me/notes.git", window, cx);
                    editor.set_text(text, window, cx);
                    editor
                }));
            }
            self.check(cx);
        }
        cx.notify();
    }

    fn finish(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.running = None;
        self.message = Some(message.into());
        cx.notify();
    }

    /// Saves the remote typed in the field.
    pub fn save_remote(&mut self, cx: &mut Context<Self>) {
        let text = self
            .remote
            .as_ref()
            .map(|remote| remote.read(cx).text(cx))
            .unwrap_or_default();
        self.set_remote(text, cx);
    }

    /// Sets Personal's remote. Every other profile refuses Git, since Git sync must never carry
    /// Work notes.
    pub fn set_remote(&mut self, text: String, cx: &mut Context<Self>) {
        if self.root.is_none() {
            return self.finish(NO_FOLDER, cx);
        }
        if self.is_running() {
            return self.finish(BUSY, cx);
        }
        let Some(ProfileId::Personal) = self.profile(cx) else {
            return self.finish(NOT_PERSONAL, cx);
        };
        let message = match crate::profiles::update(cx, |profiles| {
            profiles.set_git_remote(ProfileId::Personal, &text)
        }) {
            Ok(Some(remote)) => format!(
                "Git sync uses {remote}. Credentials come from Git's own credential helper or SSH agent."
            ),
            Ok(None) => "Git sync is off for this notebook.".to_owned(),
            Err(error) => error,
        };
        self.finish(message, cx);
    }

    fn record_git(&self, outcome: Result<u64, String>, cx: &mut Context<Self>) {
        if let Err(error) = crate::profiles::update(cx, |profiles| {
            profiles.record_git(ProfileId::Personal, outcome)
        }) {
            crate::outline::show_notice(error, cx);
        }
    }

    /// Backs the notebook up when Personal has a backup folder, then commits it, brings in what
    /// the remote has, and pushes. A failed backup stops the sync; a failed sync keeps every note
    /// and commit here and is shown until the next attempt.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        if self.is_running() {
            return self.finish(BUSY, cx);
        }
        let (Some(root), Some(kind)) = (self.root.clone(), self.kind.clone()) else {
            return self.finish(NO_FOLDER, cx);
        };
        let Some(ProfileId::Personal) = self.profile(cx) else {
            return self.finish(NOT_PERSONAL, cx);
        };
        let profile = crate::profiles::get(ProfileId::Personal, cx);
        self.running = Some(
            if profile.backup_folder.is_some() {
                "Backing up, then syncing with Git…"
            } else {
                "Syncing with Git…"
            }
            .into(),
        );
        self.message = None;
        cx.notify();
        let git = self.git.clone();
        let at = now_millis();
        let work = cx.background_spawn(async move {
            let source = match &kind {
                Kind::Folder { state } => Source::Folder {
                    root: root.clone(),
                    state: state.clone(),
                },
                Kind::Quire { database, .. } => Source::Store {
                    database: database.clone(),
                },
            };
            let backed_up = profile
                .backup_folder
                .map(|folder| {
                    backup::create(&source, &folder, at)
                        .map(|made| (made.created, made.path))
                        .map_err(|error| Stopped::Backup(format!("{error:#}")))
                })
                .transpose()?;
            if let Kind::Quire { client, .. } = &kind {
                crate::live_sync::publish_all(client.clone(), root.clone(), at)
                    .await
                    .0
                    .map_err(Stopped::Publication)?;
            }
            let target = Target {
                profile: ProfileId::Personal.git(),
                root: &root,
                remote: profile.git_remote.as_deref(),
            };
            let synced = match kind {
                Kind::Folder { .. } => git_sync::sync_folder(&git, &target, at),
                Kind::Quire { .. } => git_sync::sync_quire(&git, &target, at),
            }
            .map_err(|error| format!("{error:#}"));
            Ok::<_, Stopped>((backed_up, synced))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let backup = match &result {
                    Ok((made, _)) => made.clone().map(Ok),
                    Err(Stopped::Backup(error)) => Some(Err(error.clone())),
                    Err(Stopped::Publication(_)) => None,
                };
                if let Some(outcome) = backup {
                    crate::profiles::update(cx, |profiles| {
                        profiles.record_backup(ProfileId::Personal, outcome)
                    })
                    .ok();
                }
                let message = match result {
                    Err(Stopped::Backup(error)) => format!(
                        "Git sync did not run because the backup before it failed: {error}. Fix the backup folder in Backups…, then sync again."
                    ),
                    Err(Stopped::Publication(error)) => format!(
                        "Git sync did not run because saved changes could not be published to the synced folder first: {error}. They stay saved on this computer; make the folder available, then sync again."
                    ),
                    Ok((_, Ok(pushed))) => {
                        this.record_git(Ok(pushed.at), cx);
                        cx.emit(Synced);
                        format!(
                            "Pushed {} at {}. Other computers have these notes once they sync.",
                            &pushed.commit[..pushed.commit.len().min(10)],
                            (this.label)(pushed.at as i64)
                        )
                    }
                    Ok((_, Err(error))) => {
                        this.record_git(Err(error.clone()), cx);
                        format!("Git sync failed: {error}")
                    }
                };
                this.finish(message, cx);
                this.check(cx);
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

    /// The lines the panel shows, in order; tests read them too.
    pub fn lines(&self, cx: &App) -> Vec<String> {
        let Some(root) = &self.root else {
            return vec![NO_FOLDER.to_owned()];
        };
        let mut lines = vec![match self.provider {
            Some(provider) => format!(
                "This notebook is in {}. Saved locally confirms this computer's write; check that {} finished uploading before switching computers.",
                provider.name(),
                provider.name()
            ),
            None => format!(
                "This notebook is in {}, which is not a known OneDrive, Dropbox, iCloud Drive, or Google Drive folder.",
                root.display()
            ),
        }];
        match &self.scan {
            None => lines.push("Checking the folder…".into()),
            Some(Err(error)) => lines.push(format!("Cannot check the folder: {error}")),
            Some(Ok(scan)) => {
                if !scan.placeholders.is_empty() {
                    let place = self.provider.map_or("the cloud", Provider::name);
                    let fix = self.provider.map_or(
                        "Make the notebook folder available offline",
                        Provider::keep_offline,
                    );
                    lines.push(format!(
                        "{} notes are only in {place}, not on this computer: {}. {fix}, then Check again.",
                        scan.placeholders.len(),
                        list(&scan.placeholders)
                    ));
                }
                if !scan.conflict_copies.is_empty() {
                    lines.push(format!(
                        "{} conflict copies: {}. Compare each with its original, keep what you need, and delete the copy.",
                        scan.conflict_copies.len(),
                        list(&scan.conflict_copies)
                    ));
                }
                if scan.placeholders.is_empty() && scan.conflict_copies.is_empty() {
                    lines.push(
                        "Every note is on this computer and there are no conflict copies.".into(),
                    );
                }
            }
        }
        lines.extend(self.warnings.iter().cloned());
        let record = self.record(cx);
        lines.push(match (self.profile(cx), &record.git_remote) {
            (Some(ProfileId::Personal), None) => {
                "Personal Git is off. Enter a remote to sync this notebook with Git.".into()
            }
            (Some(ProfileId::Personal), Some(remote)) => format!("Personal Git: {remote}"),
            _ => NOT_PERSONAL.into(),
        });
        if record.git_remote.is_some() {
            lines.push(match record.last_push_at {
                Some(at) => format!(
                    "Last successful push: {}. A push puts the commits on the remote; another computer has them only after it syncs.",
                    (self.label)(at as i64)
                ),
                None => "No successful push yet.".into(),
            });
        }
        if let Some(error) = &record.git_error {
            lines.push(format!("Last Git error: {error}"));
        }
        lines
    }
}

fn list(paths: &[PathBuf]) -> String {
    let shown: Vec<String> = paths
        .iter()
        .take(5)
        .map(|path| path.display().to_string())
        .collect();
    let more = paths.len().saturating_sub(shown.len());
    if more == 0 {
        shown.join(", ")
    } else {
        format!("{}, and {more} more", shown.join(", "))
    }
}

impl Render for SyncStatus {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let message = self.message();
        let running = self.is_running();
        let panel =
            self.shown.then(|| {
                let folder = self.root.is_some();
                gpui::div()
                    .p_2()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(self.lines(cx).into_iter().map(Label::new))
                    .when(folder, |div| {
                        div.when_some(self.remote.clone(), |div, remote| {
                            div.child(
                                gpui::div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .child(Label::new("Git remote"))
                                    .child(gpui::div().w_96().child(remote))
                                    .child(Button::new("save-git-remote", "Save remote").on_click(
                                        cx.listener(|this, _, _, cx| this.save_remote(cx)),
                                    )),
                            )
                        })
                        .child(
                            gpui::div()
                                .flex()
                                .gap_2()
                                .when(self.record(cx).git_remote.is_some(), |div| {
                                    div.child(
                                        Button::new("sync-with-git", "Sync with Git")
                                            .on_click(cx.listener(|this, _, _, cx| this.sync(cx))),
                                    )
                                })
                                .child(
                                    Button::new("check-sync-folder", "Check again")
                                        .on_click(cx.listener(|this, _, _, cx| this.check(cx))),
                                ),
                        )
                    })
                    .child(
                        Button::new("hide-sync", "Hide sync")
                            .on_click(cx.listener(|this, _, window, cx| this.toggle(window, cx))),
                    )
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
                                Button::new("dismiss-sync-message", "Dismiss")
                                    .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                            )
                        }),
                )
            })
            .when(!self.shown, |div| {
                div.children(
                    self.warnings
                        .iter()
                        .map(|warning| gpui::div().p_2().child(Label::new(warning.clone()))),
                )
            })
            .when_some(panel, |div, panel| div.child(panel))
    }
}
