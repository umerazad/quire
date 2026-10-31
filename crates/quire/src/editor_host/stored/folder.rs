//! Folder notebooks in the stored window: opening one from the command line, following edits
//! other applications make to its files, and choosing a version when unsaved edits meet a changed
//! file, as the reference folder notebook does.

use super::*;
use crate::navigation::create_page_with;
use ::fs::Fs as _;
use anyhow::Context as _;
use futures::StreamExt as _;
use gpui::{SharedString, Task};
use quire_store::{
    folder_notebook::{RecoveredDraft, StuckChange, Trash, is_markdown, list, validate},
    sync::format::page_id_for_path,
};
use std::{
    path::{Component, Path},
    time::Duration,
};
use ui::Disableable as _;

/// Coalesces bursts of file events, such as an editor's save, into one reread.
const WATCH_LATENCY: Duration = Duration::from_millis(100);
/// How often the whole folder is reread in case the watcher missed a change, as the reference
/// app does.
const RESCAN_EVERY: Duration = Duration::from_secs(30);
/// Recovered pages the notice offers to open.
const LISTED_RECOVERED: usize = 10;
/// Lines of the disk version the comparison shows.
const COMPARED_LINES: usize = 400;

pub(in crate::editor_host) struct FolderOptions {
    root: PathBuf,
    page: Option<String>,
    /// Shown in the window's notices, such as an import's report.
    notices: Vec<String>,
    /// Quire's state folder for the notebook, when not the one in application data.
    state: Option<PathBuf>,
}

/// Opens the folder notebook at a path in a new window, showing the given notices.
pub(in crate::editor_host) type OpenFolderFn =
    dyn Fn(PathBuf, Vec<String>, &mut App) -> Result<(), String>;

/// Opens a folder notebook in a new window with `notices`; tests replace it to keep Quire's
/// application data untouched.
#[derive(Clone)]
pub(in crate::editor_host) struct OpenFolder(pub std::rc::Rc<OpenFolderFn>);

impl gpui::Global for OpenFolder {}

/// Opens the folder notebook at `root` in a new window, showing `notices` there.
pub(super) fn open_folder_window(
    root: PathBuf,
    notices: Vec<String>,
    cx: &mut App,
) -> Result<(), String> {
    match cx.try_global::<OpenFolder>().cloned() {
        Some(open) => (open.0)(root, notices, cx),
        None => FolderOptions::new(root, notices, None).open(cx),
    }
}

impl FolderOptions {
    /// `--notebook-folder <folder> [--page <relative path>]`.
    pub fn parse(arguments: &[String]) -> Result<Option<Self>, String> {
        let value = |flag: &str| -> Result<Option<String>, String> {
            let Some(index) = arguments.iter().position(|argument| argument == flag) else {
                return Ok(None);
            };
            arguments
                .get(index + 1)
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .cloned()
                .map(Some)
                .ok_or_else(|| {
                    format!("Missing value for {flag}. Pass a nonempty value after the flag.")
                })
        };
        let Some(root) = value("--notebook-folder")? else {
            return Ok(None);
        };
        let page = value("--page")?;
        if let Some(page) = &page {
            validate(page).map_err(|error| format!("{error:#}"))?;
        }
        Ok(Some(Self {
            root: root.into(),
            page,
            notices: Vec::new(),
            state: None,
        }))
    }

    /// The folder notebook at `root`, opening on its first note.
    pub fn new(root: PathBuf, notices: Vec<String>, state: Option<PathBuf>) -> Self {
        Self {
            root,
            page: None,
            notices,
            state,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn has_page(&self) -> bool {
        self.page.is_some()
    }

    pub fn open(self, cx: &mut App) -> Result<(), String> {
        let root = std::fs::canonicalize(&self.root).map_err(|error| {
            format!(
                "Cannot open notebook folder {}: {error}. Pass an existing folder to --notebook-folder.",
                self.root.display()
            )
        })?;
        let state = self.state.unwrap_or_else(|| state_folder(&root));
        show_folder_window(root, state, self.page, self.notices, cx)
    }
}

/// Opens the folder notebook at `root` with its state in `state`, on `page` or its first note,
/// showing `notices`. Without a chosen page, an imported Roam graph opens on its pages, as the
/// reference does.
pub(super) fn show_folder_window(
    root: PathBuf,
    state: PathBuf,
    page: Option<String>,
    notices: Vec<String>,
    cx: &mut App,
) -> Result<(), String> {
    crate::profiles::check_open(
        &quire_store::profiles::Notebook::Folder { root: root.clone() },
        cx,
    )?;
    let notes = list(&root).files;
    let restored = page
        .is_none()
        .then(|| {
            crate::navigation::WorkspaceFile::beside(
                &state.join("folder.sqlite"),
                &format!("folder:{}", root.display()),
            )
            .load()
            .active?
            .page_id()
            .map(str::to_owned)
        })
        .flatten()
        .filter(|id| notes.iter().any(|note| page_id_for_path(&note.path) == *id));
    let roam =
        page.is_none() && restored.is_none() && crate::exchange::roam::is_roam_notebook(&root);
    let page = match (page, restored) {
        (Some(page), _) => page_id_for_path(&page),
        (None, Some(restored)) => restored,
        (None, None) => notes.into_iter().next().map(|file| page_id_for_path(&file.path)).ok_or_else(|| {
            format!(
                "{} has no Markdown notes. Add a .md file or pass --page with the note to create.",
                root.display()
            )
        })?,
    };
    let mut session =
        NotebookSession::open_folder(root.clone(), state.clone(), Some(system_trash(cx)))?;
    let page = session.open_page(page, cx);
    let options = gpui::WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(gpui::px(1100.), gpui::px(760.)),
            cx,
        ))),
        ..Default::default()
    };
    let notebook = format!("folder:{}", root.display());
    cx.open_window(options, move |window, cx| {
        window.set_window_title("Quire Native");
        cx.new(|cx| {
            let mut this = StoredWindow::new(
                session,
                page,
                state.join("folder.sqlite"),
                notebook,
                window,
                cx,
            );
            this.watch_folder(cx);
            this.rescan_folder_backstop(window, cx);
            this.folder
                .notices
                .extend(notices.into_iter().map(SharedString::from));
            this.load_folder_notices(cx);
            this.import_reference_history(cx);
            if roam {
                this.visit(
                    quire_core::workspace::Destination::Browser {
                        view: quire_core::workspace::BrowserView::Pages,
                    },
                    window,
                    cx,
                );
            }
            this
        })
    })
    .map_err(|error| {
        format!(
            "Cannot open Quire's window: {error}. Check desktop and graphics access, then retry."
        )
    })?;
    cx.activate(true);
    Ok(())
}

/// Quire's recovery, history, index, and workspace for the folder at `root`, kept in application
/// data so opening a folder never writes inside it.
pub(super) fn state_folder(root: &Path) -> PathBuf {
    quire_store::folder_notebook::state_folder_in(paths::data_dir(), root)
}

/// Moves deleted notes and folders to the system Trash through Zed's file system, where the user
/// can restore them.
fn system_trash(cx: &App) -> Trash {
    let fs = ::fs::RealFs::new(None, cx.background_executor().clone());
    std::sync::Arc::new(move |path: &Path| {
        let options = ::fs::RemoveOptions {
            recursive: true,
            ignore_if_not_exists: false,
        };
        futures::executor::block_on(fs.trash(path, options))
            .map(drop)
            .with_context(|| {
                format!(
                    "Cannot move {} to the Trash. Nothing else was deleted; delete it in Finder or Explorer.",
                    path.display()
                )
            })
    })
}

/// Notebook-relative note paths for a batch of watcher events, or None when the batch needs a
/// full reread: a folder moved, a watcher overflowed, or a path was not a note.
fn changed_notes(roots: &[PathBuf], events: &[::fs::PathEvent]) -> Option<Vec<String>> {
    let mut notes = Vec::new();
    for event in events {
        if event.kind == Some(::fs::PathEventKind::Rescan) {
            return None;
        }
        let Some(relative) = roots
            .iter()
            .find_map(|root| event.path.strip_prefix(root).ok())
        else {
            continue;
        };
        let mut parts = Vec::new();
        for component in relative.components() {
            match component {
                Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
                _ => return None,
            }
        }
        if parts.is_empty() || parts.iter().any(|part| part.starts_with('.')) {
            continue;
        }
        if !parts.last().is_some_and(|name| is_markdown(name)) {
            return None;
        }
        notes.push(parts.join("/"));
    }
    notes.sort();
    notes.dedup();
    Some(notes)
}

/// The window's folder conflict controls.
#[derive(Default)]
pub(super) struct FolderView {
    comparing: bool,
    copying: Option<Task<()>>,
    pub(super) error: Option<SharedString>,
    /// What opening the folder found and did, such as finishing an interrupted rename.
    pub(super) notices: Vec<SharedString>,
    /// Pages that gained a recovery draft from the reference app's records, to open for review.
    pub(super) recovered: Vec<RecoveredDraft>,
    /// Interrupted changes across notes that opening could not finish, to retry or set aside.
    pub(super) stuck: Vec<StuckChange>,
    pub(super) resolving: Option<Task<()>>,
    /// The periodic reread and the reread on activating the window, kept for its lifetime.
    backstop: Option<(Task<()>, gpui::Subscription)>,
}

/// How to resolve an interrupted change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    Retry,
    SetAside,
}

impl StoredWindow {
    /// Watches the notebook folder with Zed's file watcher for the window's lifetime.
    pub(super) fn watch_folder(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(root) = self
            .session
            .borrow()
            .folder()
            .map(|(root, _)| root.to_owned())
        else {
            return;
        };
        let watcher = ::fs::RealFs::new(None, cx.background_executor().clone());
        cx.spawn(async move |this, cx| {
            let (mut events, _watcher) = watcher.watch(&root, WATCH_LATENCY).await;
            let roots: Vec<PathBuf> = std::fs::canonicalize(&root)
                .into_iter()
                .chain([root])
                .collect();
            while let Some(batch) = events.next().await {
                let notes = changed_notes(&roots, &batch);
                if notes.as_ref().is_some_and(Vec::is_empty) {
                    continue;
                }
                if this
                    .update(cx, |this, cx| this.folder_changed(notes, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
    }

    /// Rereads the whole folder every `RESCAN_EVERY` and whenever the window becomes active, a
    /// backstop for changes the watcher missed, such as edits made while the computer slept or
    /// on a network drive.
    pub(super) fn rescan_folder_backstop(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.session.borrow().folder().is_none() {
            return;
        }
        let activated = cx.observe_window_activation(window, |this, window, cx| {
            if window.is_window_active() {
                this.folder_changed(None, cx);
            }
        });
        let periodic = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RESCAN_EVERY).await;
                if this
                    .update(cx, |this, cx| this.folder_changed(None, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self.folder.backstop = Some((periodic, activated));
    }

    /// Files changed outside Quire: open pages take in their new versions and the index rescans.
    pub(super) fn folder_changed(
        &mut self,
        notes: Option<Vec<String>>,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(index) = self.index_client(cx) {
            index
                .invalidate()
                .map(|reply| cx.background_spawn(reply).detach())
                .ok();
        }
        if notes.is_none() {
            crate::media::load::files_arrived(cx);
        }
        let work = self.session.borrow().folder_changed(notes, cx);
        cx.spawn(async move |this, cx| {
            if let Err(error) = work.await {
                this.update(cx, |this, cx| {
                    this.folder.error = Some(format!("Could not reread changed notes: {error}. Make the folder available locally; Quire rereads it on the next change.").into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(super) fn conflicted(
        &self,
        cx: &App,
    ) -> Option<Entity<crate::page_persistence::PagePersistence>> {
        self.page
            .read(cx)
            .persistence()
            .filter(|persistence| persistence.read(cx).conflict().is_some())
            .cloned()
    }

    /// Discards this page's unsaved edits after the user confirms, showing the file on disk.
    fn use_disk_version(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let Some(persistence) = self.conflicted(cx) else {
            return;
        };
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            "Discard your local edits and use the version currently on disk?",
            None,
            &["Use disk version", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| {
                    persistence.update(cx, |persistence, cx| persistence.use_disk(cx));
                    this.folder.comparing = false;
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Saves this page's text as a new page, with fresh block IDs, then shows the disk version
    /// here and opens the copy. Typing during the copy keeps the conflict open.
    pub(super) fn save_conflict_copy(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let (Some(persistence), Some(index)) = (self.conflicted(cx), self.index_client(cx)) else {
            return;
        };
        let (text, path, version) = {
            let persistence = persistence.read(cx);
            (
                persistence.local_text(cx),
                persistence.path().to_owned(),
                persistence.buffer().read(cx).version(),
            )
        };
        let text = match text {
            Ok(text) => text,
            Err(error) => {
                self.folder.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let date = self.today().unwrap_or_default();
        let (title, copied) = match crate::file_actions::page_copy(&path, &text, "recovered", &date)
        {
            Ok(copy) => copy,
            Err(error) => {
                self.folder.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let work = create_page_with(
            self.session.borrow().storage(),
            index,
            title,
            String::new(),
            copied,
        );
        self.folder.error = None;
        self.folder.copying = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.folder.copying = None;
                match result {
                    Ok(copy) => {
                        let unchanged = persistence.read(cx).buffer().read(cx).version() == version;
                        if unchanged {
                            persistence.update(cx, |persistence, cx| persistence.use_disk(cx));
                            this.folder.comparing = false;
                        } else {
                            this.folder.error = Some("Your version was saved as a copy, but you kept typing here, so this page still needs a choice.".into());
                        }
                        this.refresh_catalogue(cx);
                        this.open_page(copy, false, window, cx);
                    }
                    Err(error) => {
                        this.folder.error = Some(format!("Could not save your version as a copy: {error} Your edits stay open here and in recovery; retry or use Copy Markdown.").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Shows what opening the folder found and did, such as an interrupted rename finished.
    pub(super) fn load_folder_notices(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(folder) = self.session.borrow().storage().folder().cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            let notices = crate::query_index::ask(folder.notices()).await;
            let recovered = crate::query_index::ask(folder.recovered_drafts()).await;
            let stuck = crate::query_index::ask(folder.stuck_changes()).await;
            this.update(cx, |this, cx| {
                match notices.and_then(|notices| Ok((notices, recovered?, stuck?))) {
                    Ok((notices, recovered, stuck)) => {
                        this.folder
                            .notices
                            .extend(notices.into_iter().map(SharedString::from));
                        this.folder.recovered = recovered;
                        this.folder.stuck = stuck;
                    }
                    Err(error) => this.folder.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Takes in the reference app's saved history queued on opening, one bounded step per
    /// request behind the first page's load, so a long history never delays opening or saving.
    pub(super) fn import_reference_history(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(folder) = self.session.borrow().storage().folder().cloned() else {
            return;
        };
        cx.spawn(async move |this, cx| {
            loop {
                match crate::query_index::ask(folder.import_reference_history()).await {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        this.update(cx, |this, cx| {
                            this.folder.notices.push(error.into());
                            cx.notify();
                        })
                        .ok();
                        break;
                    }
                }
            }
        })
        .detach();
    }

    /// Retries or sets aside an interrupted change. Either way the folder is reread, so pages the
    /// change wrote show their new text, and what happened joins the notices.
    pub(super) fn resolve_stuck(
        &mut self,
        name: String,
        resolution: Resolution,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(folder) = self.session.borrow().storage().folder().cloned() else {
            return;
        };
        if self.folder.resolving.is_some() {
            return;
        }
        self.folder.error = None;
        self.folder.resolving = Some(cx.spawn(async move |this, cx| {
            let outcome = match resolution {
                Resolution::Retry => crate::query_index::ask(folder.retry_change(name.clone()))
                    .await
                    .map(|()| "Finished the interrupted change. Its notes and links are now saved.".to_owned()),
                Resolution::SetAside => crate::query_index::ask(folder.set_aside_change(name.clone()))
                    .await
                    .map(|kept| format!(
                        "Set the interrupted change aside. Its notes stay as they are now; every version it planned is kept in {}.",
                        kept.display()
                    )),
            };
            let stuck = crate::query_index::ask(folder.stuck_changes()).await;
            this.update(cx, |this, cx| {
                this.folder.resolving = None;
                match outcome {
                    Ok(notice) => {
                        this.folder.notices.push(notice.into());
                        this.folder_changed(None, cx);
                        this.refresh_catalogue(cx);
                    }
                    Err(error) => {
                        this.folder.error = Some(
                            format!("{error} Put the note back or set the change aside.").into(),
                        )
                    }
                }
                match stuck {
                    Ok(stuck) => this.folder.stuck = stuck,
                    Err(error) => this.folder.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_stuck(&self, cx: &mut gpui::Context<Self>) -> Vec<gpui::AnyElement> {
        self.folder
            .stuck
            .iter()
            .enumerate()
            .map(|(index, stuck)| {
                let (retry, aside) = (stuck.name.clone(), stuck.name.clone());
                let busy = self.folder.resolving.is_some();
                gpui::div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(Label::new(format!(
                        "An interrupted change across notes is waiting. {}",
                        stuck.summary()
                    )))
                    .child(
                        gpui::div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new(("retry-stuck", index), "Retry")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.resolve_stuck(retry.clone(), Resolution::Retry, cx)
                                    })),
                            )
                            .child(
                                Button::new(("set-aside-stuck", index), "Set aside")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.resolve_stuck(aside.clone(), Resolution::SetAside, cx)
                                    })),
                            ),
                    )
                    .into_any_element()
            })
            .collect()
    }

    fn render_folder_notices(
        &self,
        error: Option<SharedString>,
        cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::AnyElement> {
        if error.is_none() && self.folder.notices.is_empty() && self.folder.stuck.is_empty() {
            return None;
        }
        let open = self
            .folder
            .recovered
            .iter()
            .take(LISTED_RECOVERED)
            .enumerate()
            .map(|(index, draft)| {
                let page = draft.page.clone();
                Button::new(("open-recovered", index), format!("Open {}", draft.path)).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.open_page(page.clone(), false, window, cx)
                    }),
                )
            })
            .collect::<Vec<_>>();
        Some(
            gpui::div()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .when_some(error, |div, error| div.child(Label::new(error)))
                .children(self.folder.notices.iter().cloned().map(Label::new))
                .child(gpui::div().flex().flex_wrap().gap_2().children(open))
                .children(self.render_stuck(cx))
                .when(!self.folder.notices.is_empty(), |div| {
                    div.child(Button::new("dismiss-folder-notices", "Dismiss").on_click(
                        cx.listener(|this, _, _, cx| {
                            this.folder.notices.clear();
                            this.folder.recovered.clear();
                            cx.notify();
                        }),
                    ))
                })
                .into_any_element(),
        )
    }

    /// The banner shown while the active page's edits conflict with its file.
    pub(super) fn render_folder_conflict(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<gpui::AnyElement> {
        let persistence = self.conflicted(cx);
        let error = self.folder.error.clone();
        if persistence.is_none() {
            return self.render_folder_notices(error, cx);
        }
        let persistence = persistence?;
        let conflict = persistence.read(cx).conflict()?.clone();
        let comparison = self.folder.comparing.then(|| {
            let lines: Vec<String> = match &conflict.disk {
                Some(disk) => disk
                    .text
                    .lines()
                    .take(COMPARED_LINES)
                    .map(str::to_owned)
                    .collect(),
                None => vec!["The file is missing on disk.".into()],
            };
            gpui::div()
                .id("disk-version")
                .p_2()
                .max_h_64()
                .overflow_y_scroll()
                .border_1()
                .border_color(cx.theme().colors().border)
                .children(lines.into_iter().map(|line| gpui::div().child(line)))
        });
        Some(
            gpui::div()
                .p_2()
                .flex()
                .flex_col()
                .gap_1()
                .bg(cx.theme().colors().surface_background)
                .child(Label::new(persistence.read(cx).status(cx)))
                .when_some(error, |div, error| div.child(Label::new(error)))
                .child(
                    gpui::div()
                        .flex()
                        .gap_2()
                        .child(
                            Button::new(
                                "save-conflict-copy",
                                if self.folder.copying.is_some() {
                                    "Saving a copy…"
                                } else {
                                    "Save my version as a copy"
                                },
                            )
                            .on_click(cx.listener(
                                |this, _, window, cx| this.save_conflict_copy(window, cx),
                            )),
                        )
                        .child(
                            Button::new("use-disk-version", "Use disk version").on_click(
                                cx.listener(|this, _, window, cx| {
                                    this.use_disk_version(window, cx)
                                }),
                            ),
                        )
                        .child(
                            Button::new(
                                "compare-versions",
                                if self.folder.comparing {
                                    "Hide disk version"
                                } else {
                                    "Compare versions"
                                },
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.folder.comparing = !this.folder.comparing;
                                cx.notify();
                            })),
                        ),
                )
                .when_some(comparison, |div, comparison| div.child(comparison))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watcher_batches_name_notes_or_ask_for_a_reread() {
        let root = PathBuf::from("/notes");
        let event = |path: &str, kind| ::fs::PathEvent {
            path: root.join(path),
            kind,
        };
        let changed = Some(::fs::PathEventKind::Changed);
        assert_eq!(
            changed_notes(
                std::slice::from_ref(&root),
                &[
                    event("A.md", changed),
                    event("Folder/B.MD", changed),
                    event("A.md", changed),
                    event(".quire-x.tmp", changed),
                    event(".git/index", changed),
                ]
            ),
            Some(vec!["A.md".to_owned(), "Folder/B.MD".to_owned()])
        );
        assert_eq!(
            changed_notes(std::slice::from_ref(&root), &[event("Folder", changed)]),
            None
        );
        assert_eq!(
            changed_notes(
                std::slice::from_ref(&root),
                &[event("A.md", Some(::fs::PathEventKind::Rescan))]
            ),
            None
        );
        assert_eq!(
            changed_notes(
                std::slice::from_ref(&root),
                &[::fs::PathEvent {
                    path: "/elsewhere/A.md".into(),
                    kind: changed,
                }]
            ),
            Some(vec![])
        );
    }
}
