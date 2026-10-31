//! Switching profiles: every open page is recorded first, AI runs are cancelled, and only then
//! does the profile's notebook open and the previous notebook's windows close. A switch that
//! cannot record the pages, or that meets a running backup or Git sync, changes nothing and says
//! why, so no edit is left behind.

use super::*;
use crate::profiles::{OpenNotebookFolder, UseCurrentProfile, UsePersonalProfile, UseWorkProfile};
use quire_store::profiles::{Notebook, ProfileId};

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
}

pub(super) fn stored_windows(cx: &App) -> Vec<gpui::WindowHandle<StoredWindow>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<StoredWindow>())
        .collect()
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
        cx.update(|cx| match std::fs::canonicalize(&root) {
            Ok(root) => open_in_profile(id, Notebook::Folder { root }, cx),
            Err(error) => crate::outline::show_notice(
                format!(
                    "Cannot open {}: {error}. Choose an existing folder of Markdown notes.",
                    root.display()
                ),
                cx,
            ),
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
pub(super) fn open_in_profile(id: ProfileId, notebook: Notebook, cx: &mut App) {
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
    match notebook {
        Notebook::Folder { root } => folder::open_folder_window(root.clone(), Vec::new(), cx),
        Notebook::Store {
            database,
            notebook,
            writer,
        } => {
            let page = crate::navigation::WorkspaceFile::beside(database, notebook)
                .load()
                .active
                .and_then(|active| active.page_id().map(str::to_owned))
                .ok_or_else(|| format!("Quire does not know which page of the Quire notebook {} to show. Open it once with --page-id; Quire remembers the page after that.", database.display()))?;
            OpenOptions {
                database: database.clone(),
                notebook: notebook.clone(),
                writer: writer.clone(),
                page,
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

    /// The profile line: a choice of profile, then this notebook's last backup and push, kept
    /// apart from the page's local save status above.
    pub(super) fn render_profile(&self, cx: &mut gpui::Context<Self>) -> gpui::Div {
        use ui::{ButtonCommon as _, Toggleable as _};
        let active = crate::profiles::active(cx);
        let held = self
            .location
            .as_ref()
            .and_then(|location| crate::profiles::holding(location, cx));
        let status: gpui::SharedString = match held {
            Some(id) => crate::profiles::status(id, &*self.clock, cx),
            None => "This notebook belongs to no profile, so backups and Git sync are off. Choose File > Profile to open a profile's notebook.".into(),
        };
        gpui::div()
            .px_2()
            .flex()
            .items_center()
            .gap_1()
            .children(ProfileId::ALL.into_iter().map(|id| {
                let action: Box<dyn gpui::Action> = match id {
                    ProfileId::Current => Box::new(UseCurrentProfile),
                    ProfileId::Work => Box::new(UseWorkProfile),
                    ProfileId::Personal => Box::new(UsePersonalProfile),
                };
                Button::new(
                    gpui::SharedString::from(format!("profile-{}", id.label())),
                    id.label(),
                )
                .toggle_state(id == active)
                .tooltip(ui::Tooltip::text(format!(
                    "Save open pages and open the {} notebook",
                    id.label()
                )))
                .on_click(move |_, window, cx| window.dispatch_action(action.boxed_clone(), cx))
            }))
            .child(Label::new(status))
    }

    /// Closes the window through its close guard, as the native close button does.
    pub(super) fn close_window(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.close.allow(CloseIntent::Window, cx) {
            window.remove_window();
        }
    }
}
