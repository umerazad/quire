//! Switching profiles: every open page is recorded first, AI runs are cancelled, and only then
//! does the profile's notebook open and the previous notebook's windows close. A switch that
//! cannot record the pages, or that meets a running backup or Git sync, changes nothing and says
//! why, so no edit is left behind.

use super::*;
use crate::profiles::{
    NewQuireNotebook, OpenNotebookDatabase, OpenNotebookFolder, UseCurrentProfile,
    UsePersonalProfile, UseWorkProfile,
};
use quire_store::profiles::{Notebook, ProfileId};

/// The page a Quire notebook with no pages shows until today's daily page opens; it is created
/// only if someone types into it.
const EMPTY_NOTEBOOK_PAGE: &str = "Untitled.md";

/// How long a switch waits for open pages to be recorded before giving up.
pub(super) const SWITCH_PATIENCE: std::time::Duration = std::time::Duration::from_secs(10);

/// A switch is in progress.
#[derive(Default)]
struct Switching(bool);

impl gpui::Global for Switching {}

pub(super) fn init(cx: &mut App) {
    cx.on_action(|_: &UseCurrentProfile, cx| switch_later(ProfileId::Current, cx));
    cx.on_action(|_: &UseWorkProfile, cx| switch_later(ProfileId::Work, cx));
    cx.on_action(|_: &UsePersonalProfile, cx| switch_later(ProfileId::Personal, cx));
    cx.on_action(|_: &OpenNotebookFolder, cx| {
        let id = crate::profiles::active(cx);
        choose_folder(id, cx)
    });
    cx.on_action(|_: &OpenNotebookDatabase, cx| {
        let id = crate::profiles::active(cx);
        choose_database(id, cx)
    });
    cx.on_action(|_: &NewQuireNotebook, cx| {
        let id = crate::profiles::active(cx);
        choose_new_quire_notebook(id, cx)
    });
}

pub(super) fn stored_windows(cx: &App) -> Vec<gpui::WindowHandle<StoredWindow>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<StoredWindow>())
        .collect()
}

/// Brings forward the window showing the active profile's notebook, or else any notebook window,
/// and runs `action` there. Settings and the menus use it for a notebook's data commands.
pub(crate) fn dispatch_to_notebook(
    action: Box<dyn gpui::Action>,
    cx: &mut App,
) -> Result<(), String> {
    let active = crate::profiles::get(crate::profiles::active(cx), cx).notebook;
    let window = active
        .as_ref()
        .and_then(|notebook| showing(notebook, cx))
        .or_else(|| stored_windows(cx).into_iter().next())
        .ok_or_else(|| {
            "No notebook is open. Open the profile's notebook from File > Profile, then try again."
                .to_owned()
        })?;
    window
        .update(cx, |_, window, cx| {
            window.activate_window();
            window.dispatch_action(action, cx);
        })
        .map_err(|error| {
            format!("Cannot reach the notebook window: {error}. Bring it forward and use the command there.")
        })
}

/// How many notebook windows are open.
pub(crate) fn notebook_windows(cx: &App) -> usize {
    stored_windows(cx).len()
}

/// The window already showing `notebook`.
pub(super) fn showing(notebook: &Notebook, cx: &App) -> Option<gpui::WindowHandle<StoredWindow>> {
    stored_windows(cx).into_iter().find(|window| {
        window.read(cx).is_ok_and(|this| {
            this.location
                .as_ref()
                .is_some_and(|location| location.same(notebook))
        })
    })
}

/// Switches once the action that asked for it is done: while an action is dispatched, its window
/// cannot be updated, so it could not save its pages or end its AI runs first.
fn switch_later(id: ProfileId, cx: &mut App) {
    cx.defer(move |cx| switch(id, cx));
}

/// Opens the profile's notebook, asking for a folder when it has none yet.
pub(super) fn switch(id: ProfileId, cx: &mut App) {
    match crate::profiles::get(id, cx).notebook {
        Some(notebook) => open_in_profile(id, notebook, cx),
        None => choose_folder(id, cx),
    }
}

fn choose_folder(id: ProfileId, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(format!("Use as the {} notebook", id.label()).into()),
    });
    cx.spawn(async move |cx| {
        let Some(root) = chosen
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|paths| paths.into_iter().next())
        else {
            return;
        };
        cx.update(|cx| open_chosen_folder(id, root, paths::data_dir().clone(), cx));
    })
    .detach();
}

/// Opens the folder chosen for `id`: a Quire notebook's synced folder opens as that notebook,
/// building this computer's database under `quire_data` the first time, and any other folder
/// opens as a folder of Markdown notes.
pub(in crate::editor_host) fn open_chosen_folder(
    id: ProfileId,
    root: std::path::PathBuf,
    quire_data: std::path::PathBuf,
    cx: &mut App,
) {
    let root = match std::fs::canonicalize(&root) {
        Ok(root) => root,
        Err(error) => {
            return crate::outline::show_notice(
                format!(
                    "Cannot open {}: {error}. Choose an existing folder of Markdown notes.",
                    root.display()
                ),
                cx,
            );
        }
    };
    if !quire_store::quire_folder::is_quire_notebook(&root) {
        return open_in_profile(id, Notebook::Folder { root }, cx);
    }
    open_quire_folder(
        id,
        cx.background_spawn(async move { quire_store::quire_folder::open(&root, &quire_data) }),
        cx,
    );
}

/// Opens the Quire notebook that `work` finds or builds, saying which change files still wait.
fn open_quire_folder(
    id: ProfileId,
    work: gpui::Task<anyhow::Result<(Notebook, Vec<String>)>>,
    cx: &mut App,
) {
    cx.spawn(async move |cx| {
        let result = work.await;
        cx.update(|cx| match result {
            Ok((notebook, problems)) => {
                if !problems.is_empty() {
                    crate::outline::show_notice(
                        format!(
                            "Some of the notebook's change files could not be read yet ({}). Quire reads them again once the sync tool delivers them.",
                            problems.join("; ")
                        ),
                        cx,
                    );
                }
                open_in_profile(id, notebook, cx)
            }
            Err(error) => refuse(id, format!("{error:#}."), cx),
        });
    })
    .detach();
}

/// Asks for an empty folder, makes it a new Quire notebook, and opens it as the notebook of `id`.
fn choose_new_quire_notebook(id: ProfileId, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some("Create notebook here".into()),
    });
    cx.spawn(async move |cx| {
        let Some(root) = chosen
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|paths| paths.into_iter().next())
        else {
            return;
        };
        cx.update(|cx| create_quire_notebook(id, root, paths::data_dir().clone(), cx));
    })
    .detach();
}

/// Makes `root` a new Quire notebook and opens it as the notebook of `id`.
pub(in crate::editor_host) fn create_quire_notebook(
    id: ProfileId,
    root: std::path::PathBuf,
    quire_data: std::path::PathBuf,
    cx: &mut App,
) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        });
    open_quire_folder(
        id,
        cx.background_spawn(async move {
            quire_store::quire_folder::create(&root, now)?;
            quire_store::quire_folder::open(&root, &quire_data)
        }),
        cx,
    );
}

/// The Quire notebook whose database is `path`, with the identities it records.
pub(in crate::editor_host) fn database_notebook(
    path: &std::path::Path,
) -> Result<Notebook, String> {
    let database = std::fs::canonicalize(path).map_err(|error| {
        format!(
            "Cannot open {}: {error}. Choose an existing Quire notebook database.",
            path.display()
        )
    })?;
    let identity = quire_store::notebook::NotebookStore::identity(&database)
        .map_err(|error| format!("{error:#}"))?;
    Ok(Notebook::Store {
        database,
        notebook: identity.notebook,
        writer: identity.writer,
        folder: None,
    })
}

/// Asks for a Quire notebook's database and opens it as the notebook of `id`.
pub(in crate::editor_host) fn choose_database(id: ProfileId, cx: &mut App) {
    let chosen = cx.prompt_for_paths(gpui::PathPromptOptions {
        files: true,
        directories: false,
        multiple: false,
        prompt: Some(format!("Use as the {} notebook", id.label()).into()),
    });
    cx.spawn(async move |cx| {
        let Some(path) = chosen
            .await
            .ok()
            .and_then(Result::ok)
            .flatten()
            .and_then(|paths| paths.into_iter().next())
        else {
            return;
        };
        cx.update(|cx| match database_notebook(&path) {
            Ok(notebook) => open_in_profile(id, notebook, cx),
            Err(error) => refuse(id, error, cx),
        });
    })
    .detach();
}

fn refuse(id: ProfileId, reason: impl std::fmt::Display, cx: &mut App) {
    crate::outline::show_notice(
        format!(
            "Quire did not switch to the {} notebook: {reason}",
            id.label()
        ),
        cx,
    );
}

/// Records every open page, then makes `notebook` the notebook of `id` and opens it.
pub(crate) fn open_in_profile(id: ProfileId, notebook: Notebook, cx: &mut App) {
    if cx.default_global::<Switching>().0 {
        return refuse(
            id,
            "another switch is still running. Wait for it to finish.",
            cx,
        );
    }
    if let Err(error) = cx
        .global::<crate::profiles::ProfileStore>()
        .check(id, &notebook)
    {
        return refuse(id, error, cx);
    }
    let windows = stored_windows(cx);
    let running = windows.iter().any(|window| {
        window
            .read(cx)
            .is_ok_and(|this| this.backups.read(cx).is_running() || this.sync.read(cx).is_running())
    });
    if running {
        return refuse(
            id,
            "a backup or Git sync is still running. Wait for it to finish, then switch again.",
            cx,
        );
    }
    let unprepared = windows.iter().any(|window| {
        window
            .update(cx, |this, _, cx| this.prepare_switch(cx))
            .is_err()
    });
    if unprepared {
        return refuse(
            id,
            "a notebook window was busy and could not save its pages first. Switch again.",
            cx,
        );
    }
    cx.set_global(Switching(true));
    cx.spawn(async move |cx| {
        let started = cx.background_executor().now();
        let outcome = loop {
            let activity =
                cx.update(|cx| {
                    crate::lifecycle::Activity::all(windows.iter().filter_map(|window| {
                        window.read(cx).ok().map(|this| this.switch_activity(cx))
                    }))
                });
            match activity {
                crate::lifecycle::Activity::Idle => break Ok(()),
                crate::lifecycle::Activity::Failed(reason) => break Err(reason),
                crate::lifecycle::Activity::Busy(reason)
                    if cx
                        .background_executor()
                        .now()
                        .saturating_duration_since(started)
                        >= SWITCH_PATIENCE =>
                {
                    break Err(format!("{reason} Wait a moment, then switch again.").into());
                }
                crate::lifecycle::Activity::Busy(_) => {
                    cx.background_executor().timer(crate::lifecycle::POLL).await
                }
            }
        };
        cx.update(|cx| {
            cx.set_global(Switching(false));
            match outcome {
                Ok(()) => finish_switch(id, notebook, windows, cx),
                Err(reason) => refuse(
                    id,
                    format!("{reason} Your edits stay open in this window."),
                    cx,
                ),
            }
        });
    })
    .detach();
}

fn finish_switch(
    id: ProfileId,
    notebook: Notebook,
    windows: Vec<gpui::WindowHandle<StoredWindow>>,
    cx: &mut App,
) {
    let previous = crate::profiles::active(cx);
    let changed = crate::profiles::update(cx, |profiles| {
        profiles.set_notebook(id, notebook.clone())?;
        profiles.activate(id)
    });
    if let Err(error) = changed {
        return refuse(id, error, cx);
    }
    let shown = showing(&notebook, cx);
    let opened = match shown {
        Some(window) => window
            .update(cx, |_, window, _| window.activate_window())
            .map_err(|error| error.to_string()),
        None => open_location(&notebook, cx),
    };
    if let Err(error) = opened {
        crate::profiles::update(cx, |profiles| profiles.activate(previous)).ok();
        return refuse(
            id,
            format!("{error} The open notebook stays as it was."),
            cx,
        );
    }
    crate::outline::show_notice(format!("Switched to the {} profile.", id.label()), cx);
    for window in windows.into_iter().filter(|window| Some(*window) != shown) {
        window
            .update(cx, |this, window, cx| {
                if this.close.activity(cx) == crate::lifecycle::Activity::Idle {
                    this.end_close_request();
                    window.remove_window();
                } else {
                    this.nav.error = Some("This notebook stays open because typing since the switch is not recorded yet. Close it once its pages are saved.".into());
                    cx.notify();
                }
            })
            .ok();
    }
}

/// Opens a profile's notebook in a new window, at the page it last showed.
pub(in crate::editor_host) fn open_location(
    notebook: &Notebook,
    cx: &mut App,
) -> Result<(), String> {
    open_location_at(notebook, false, cx)
}

/// Opens a profile's notebook as a launch does: its last page, then today's daily page.
pub(in crate::editor_host) fn launch_location(
    notebook: &Notebook,
    cx: &mut App,
) -> Result<(), String> {
    open_location_at(notebook, true, cx)
}

fn open_location_at(notebook: &Notebook, today: bool, cx: &mut App) -> Result<(), String> {
    match notebook {
        Notebook::Folder { root } if today => FolderOptions::new(root.clone(), Vec::new(), None)
            .on_today()
            .open(cx),
        Notebook::Folder { root } => folder::open_folder_window(root.clone(), Vec::new(), cx),
        Notebook::Store {
            database,
            notebook,
            writer,
            folder,
        } => {
            let remembered = crate::navigation::WorkspaceFile::beside(database, notebook)
                .load()
                .active
                .and_then(|active| active.page_id().map(str::to_owned));
            let first = match remembered {
                Some(page) => Some(page),
                None => {
                    quire_store::notebook::NotebookStore::identity(database)
                        .map_err(|error| format!("{error:#}"))?
                        .first_page
                }
            };
            // A notebook with no pages yet, such as a new one, starts on today's daily page.
            let empty = first.is_none();
            OpenOptions {
                database: database.clone(),
                notebook: notebook.clone(),
                writer: writer.clone(),
                page: first.unwrap_or_else(|| {
                    quire_store::sync::format::page_id_for_path(EMPTY_NOTEBOOK_PAGE)
                }),
                folder: folder.clone(),
                today: today || empty,
            }
            .open(cx)
        }
    }
}

impl StoredWindow {
    /// What a switch waits for: everything a close waits for, and every open page saved locally,
    /// not only recorded for recovery.
    fn switch_activity(&self, cx: &App) -> crate::lifecycle::Activity {
        use crate::lifecycle::Activity;
        let unsaved = self
            .session
            .borrow()
            .open_pages()
            .into_iter()
            .filter_map(|page| page.read(cx).persistence().cloned())
            .any(|page| page.read(cx).is_dirty(cx));
        Activity::all([
            self.close.activity(cx),
            if unsaved {
                Activity::Busy("Pages are still being saved locally.".into())
            } else {
                Activity::Idle
            },
        ])
    }

    /// Before a switch: cancels AI runs, whose proposals never reach the notebook unaccepted, and
    /// saves and records every open page.
    fn prepare_switch(&mut self, cx: &mut gpui::Context<Self>) {
        let editors: Vec<_> = self
            .nav
            .editors
            .values()
            .chain(self.editor.iter())
            .flat_map(|view| {
                let view = view.read(cx);
                [view.source.clone(), view.second.clone()]
            })
            .collect();
        for editor in editors {
            editor.update(cx, crate::ai::proposal::dismiss);
        }
        if let Some(pane) = self.nav.answer.pane.clone() {
            pane.update(cx, |pane, cx| pane.cancel(cx));
        }
        let pages: Vec<_> = self
            .session
            .borrow()
            .open_pages()
            .into_iter()
            .filter_map(|page| page.read(cx).persistence().cloned())
            .collect();
        for page in pages {
            page.update(cx, |page, cx| page.save(cx));
        }
        self.session.borrow().recover(cx);
    }

    /// Closes the window through its close guard, as the native close button does.
    pub(super) fn close_window(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.close.allow(CloseIntent::Window, cx) {
            window.remove_window();
        }
    }
}
