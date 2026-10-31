//! The window's sync commands, and rereading notes a Git sync changed.

use super::*;
use crate::sync_status::{CheckSyncFolder, Kind, ShowSync, SyncStatus, SyncWithGit, Synced};
use quire_store::git_sync::Git;

/// Git as the window runs it. Tests run Git without the user's configuration, so they never read
/// the real identity or credentials.
pub(super) fn git() -> Git {
    if cfg!(test) {
        [
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
            ("GIT_AUTHOR_NAME", "Quire Test"),
            ("GIT_AUTHOR_EMAIL", "test@example.invalid"),
            ("GIT_COMMITTER_NAME", "Quire Test"),
            ("GIT_COMMITTER_EMAIL", "test@example.invalid"),
        ]
        .into_iter()
        .fold(Git::default(), |git, (key, value)| git.env(key, value))
    } else {
        Git::default()
    }
}

pub(super) fn new_sync_status(
    session: &Rc<RefCell<NotebookSession>>,
    notebook: Option<quire_store::profiles::Notebook>,
    calendar: Option<DailyCalendar>,
    cx: &mut gpui::Context<StoredWindow>,
) -> Entity<SyncStatus> {
    let session = session.borrow();
    let folder = session
        .folder()
        .map(|(root, state)| {
            (
                root.to_owned(),
                Kind::Folder {
                    state: state.to_owned(),
                },
            )
        })
        .or_else(|| match (&notebook, session.storage()) {
            (
                Some(quire_store::profiles::Notebook::Store {
                    database,
                    folder: Some(folder),
                    ..
                }),
                crate::notebook_backend::NotebookBackend::Store(client),
            ) => Some((
                folder.clone(),
                Kind::Quire {
                    database: database.clone(),
                    client,
                },
            )),
            _ => None,
        });
    let label = backups::clock_label(calendar.unwrap_or_else(DailyCalendar::utc));
    cx.new(|cx| SyncStatus::new(folder, notebook, git(), label, cx))
}

impl StoredWindow {
    pub(super) fn watch_sync(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        cx.subscribe_in(
            &self.sync,
            window,
            |this, _, _: &Synced, _, cx| match this.live.clone() {
                Some(live) => live.update(cx, |live, cx| live.import(cx)),
                None => this.folder_changed(None, cx),
            },
        )
        .detach();
    }

    pub(super) fn toggle_sync(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.sync.update(cx, |sync, cx| sync.toggle(window, cx));
    }

    pub(super) fn register_sync(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        use gpui::InteractiveElement as _;
        div.on_action(cx.listener(|this, _: &ShowSync, window, cx| this.toggle_sync(window, cx)))
            .on_action(cx.listener(|this, _: &SyncWithGit, _, cx| {
                this.sync.update(cx, |sync, cx| sync.sync(cx))
            }))
            .on_action(cx.listener(|this, _: &CheckSyncFolder, _, cx| {
                this.sync.update(cx, |sync, cx| sync.check(cx))
            }))
    }
}
