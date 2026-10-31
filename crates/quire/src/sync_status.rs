//! The notebook's sync status: which provider synchronizes the folder, notes it keeps only in the
//! cloud, the conflict copies it made, and Personal Git with its remote, last push, and last
//! error. The checks and Git run in `quire_store`, off the UI thread.

use editor::Editor;
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, IntoElement, ParentElement, PromptLevel,
    Render, SharedString, Styled, Window,
};
use quire_store::{
    git_sync::{self, Git, Profile, Target},
    provider::{self, Provider, Scan},
};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
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
        /// Commits this notebook, brings in the Personal remote's notes, and pushes.
        SyncWithGit,
        /// Looks again for notes kept only in the cloud and for conflict copies.
        CheckSyncFolder,
    ]
);

pub const BUSY: &str = "Git sync is already running. Wait for it to finish.";
pub const NO_FOLDER: &str = "Git sync and provider checks work on folder notebooks. Open the notebook's folder to sync it with Git.";
pub const CONFIRM_PERSONAL: &str = "Use Git sync for this notebook as your Personal notebook?";
pub const PERSONAL: &str = "Use as Personal";

/// What this computer remembers about a notebook's Personal Git.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRecord {
    /// The person confirmed this notebook holds Personal notes. Until profiles exist, this is the
    /// only thing that permits Git sync.
    pub personal: bool,
    pub remote: Option<String>,
    pub last_push_at: Option<u64>,
    pub error: Option<String>,
}

/// Where this computer keeps each notebook's Git record: a JSON object from the notebook folder
/// to its record. Without a file (tests), records last until the window closes.
#[derive(Clone)]
pub struct Settings(Option<PathBuf>);

impl Settings {
    pub fn new(file: Option<PathBuf>) -> Self {
        Self(file)
    }

    fn entries(&self) -> BTreeMap<String, GitRecord> {
        self.0
            .as_ref()
            .and_then(|file| std::fs::read(file).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    fn load(&self, key: &str) -> GitRecord {
        self.entries().remove(key).unwrap_or_default()
    }

    fn save(&self, key: &str, record: &GitRecord) -> Result<(), String> {
        let Some(file) = &self.0 else {
            return Ok(());
        };
        let mut entries = self.entries();
        entries.insert(key.to_owned(), record.clone());
        let text = serde_json::to_vec_pretty(&entries).map_err(|error| error.to_string())?;
        let temporary = file.with_extension("json.tmp");
        std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
            .and_then(|()| std::fs::write(&temporary, text))
            .and_then(|()| std::fs::rename(&temporary, file))
            .map_err(|error| {
                format!(
                    "Cannot remember Git settings in {}: {error}. Check write access to Quire's application data and try again.",
                    file.display()
                )
            })
    }
}

/// A Git sync changed notes in the folder: the window rereads them.
pub struct Synced;

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

fn key(root: &Path) -> String {
    std::fs::canonicalize(root)
        .unwrap_or_else(|_| root.to_owned())
        .display()
        .to_string()
}

pub struct SyncStatus {
    root: Option<PathBuf>,
    git: Git,
    settings: Settings,
    record: GitRecord,
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
        root: Option<PathBuf>,
        git: Git,
        settings: Settings,
        label: Rc<dyn Fn(i64) -> String>,
        cx: &mut Context<Self>,
    ) -> Self {
        let record = root
            .as_deref()
            .map(|root| settings.load(&key(root)))
            .unwrap_or_default();
        let mut this = Self {
            provider: root.as_deref().and_then(provider::detect),
            root,
            git,
            settings,
            record,
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

    #[cfg(test)]
    pub fn record(&self) -> &GitRecord {
        &self.record
    }

    /// Looks for cloud-only notes and conflict copies, and keeps live data out of a repository
    /// at the notebook folder, as opening a notebook does in the reference app.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            return;
        };
        let git = self.git.clone();
        let work = cx.background_spawn(async move {
            let scan = provider::scan(&root).map_err(|error| format!("{error:#}"));
            (scan, git_sync::protect_notebook(&git, &root))
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
                let text = self.record.remote.clone().unwrap_or_default();
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

    fn remember(&mut self, cx: &mut Context<Self>) {
        let Some(root) = &self.root else {
            return;
        };
        if let Err(error) = self.settings.save(&key(root), &self.record) {
            self.message = Some(error.into());
            cx.notify();
        }
    }

    /// Saves the remote typed in the field. The first remote asks the person to confirm this
    /// notebook holds Personal notes, since Git sync must never carry Work notes.
    pub fn save_remote(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self
            .remote
            .as_ref()
            .map(|remote| remote.read(cx).text(cx))
            .unwrap_or_default();
        self.set_remote(text, window, cx);
    }

    pub fn set_remote(&mut self, text: String, window: &mut Window, cx: &mut Context<Self>) {
        if self.root.is_none() {
            self.finish(NO_FOLDER, cx);
            return;
        }
        if self.is_running() {
            self.message = Some(BUSY.into());
            cx.notify();
            return;
        }
        let remote = match git_sync::check_remote(Profile::Personal, &text) {
            Ok(remote) => remote,
            Err(error) => return self.finish(error.to_string(), cx),
        };
        if self.record.personal || remote.is_none() {
            self.apply_remote(remote, cx);
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            CONFIRM_PERSONAL,
            Some(
                "Git sync is only for Personal notes: never push Work notes to a personal remote. Quire remembers this choice for this notebook on this computer.",
            ),
            &[PERSONAL, "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            this.update(cx, |this, cx| {
                this.record.personal = true;
                this.apply_remote(remote, cx);
            })
            .ok();
        })
        .detach();
    }

    fn apply_remote(&mut self, remote: Option<String>, cx: &mut Context<Self>) {
        let message = match &remote {
            Some(remote) => format!(
                "Git sync uses {remote}. Credentials come from Git's own credential helper or SSH agent."
            ),
            None => "Git sync is off for this notebook.".to_owned(),
        };
        self.record = GitRecord {
            personal: self.record.personal,
            remote,
            last_push_at: None,
            error: None,
        };
        self.message = Some(message.into());
        self.remember(cx);
        cx.notify();
    }

    /// Commits the notebook, brings in what the remote has, and pushes. A failure keeps every
    /// note and commit here and is shown until the next attempt.
    pub fn sync(&mut self, cx: &mut Context<Self>) {
        if self.is_running() {
            self.message = Some(BUSY.into());
            cx.notify();
            return;
        }
        let Some(root) = self.root.clone() else {
            return self.finish(NO_FOLDER, cx);
        };
        self.running = Some("Syncing with Git…".into());
        self.message = None;
        cx.notify();
        let git = self.git.clone();
        let remote = self.record.remote.clone();
        let profile = if self.record.personal {
            Profile::Personal
        } else {
            Profile::Current
        };
        let at = now_millis();
        let work = cx.background_spawn(async move {
            let target = Target {
                profile,
                root: &root,
                remote: remote.as_deref(),
            };
            git_sync::sync_folder(&git, &target, at).map_err(|error| format!("{error:#}"))
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                let message = match result {
                    Ok(pushed) => {
                        this.record.last_push_at = Some(pushed.at);
                        this.record.error = None;
                        cx.emit(Synced);
                        format!(
                            "Pushed {} at {}. Other computers have these notes once they sync.",
                            &pushed.commit[..pushed.commit.len().min(10)],
                            (this.label)(pushed.at as i64)
                        )
                    }
                    Err(error) => {
                        this.record.error = Some(error.clone());
                        format!("Git sync failed: {error}")
                    }
                };
                this.remember(cx);
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
    pub fn lines(&self) -> Vec<String> {
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
        lines.push(match (&self.record.remote, self.record.personal) {
            (None, _) => "Personal Git is off. Enter a remote to sync this notebook with Git.".into(),
            (Some(remote), true) => format!("Personal Git: {remote}"),
            (Some(remote), false) => format!(
                "Git remote {remote} is set, but this notebook is not confirmed as Personal. Save the remote again to confirm."
            ),
        });
        if self.record.remote.is_some() {
            lines.push(match self.record.last_push_at {
                Some(at) => format!(
                    "Last successful push: {}. A push puts the commits on the remote; another computer has them only after it syncs.",
                    (self.label)(at as i64)
                ),
                None => "No successful push yet.".into(),
            });
        }
        if let Some(error) = &self.record.error {
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
        let panel = self.shown.then(|| {
            let folder = self.root.is_some();
            gpui::div()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .children(self.lines().into_iter().map(Label::new))
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
                                    cx.listener(|this, _, window, cx| this.save_remote(window, cx)),
                                )),
                        )
                    })
                    .child(
                        gpui::div()
                            .flex()
                            .gap_2()
                            .when(self.record.remote.is_some(), |div| {
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
