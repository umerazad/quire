//! The window's backup commands, and opening a restored notebook in place of this one.

use super::*;
use crate::backups::{BackUpNow, Backups, ChooseBackupFolder, Restored, ShowBackups};
use quire_store::{
    backup::{Location, Source},
    profiles::Notebook,
};
use std::{path::Path, sync::Arc};

/// Times in the notebook's time zone, as a date and a 24-hour time.
pub(super) fn clock_label(calendar: DailyCalendar) -> Rc<dyn Fn(i64) -> String> {
    Rc::new(move |at| match calendar.clock_at(at) {
        Ok((date, hour, minute)) => format!("{date} {hour:02}:{minute:02}"),
        Err(_) => format!("Saved at {at}"),
    })
}

pub(super) fn new_backups(
    session: &Rc<RefCell<NotebookSession>>,
    database: &Path,
    notebook: Option<Notebook>,
    calendar: Option<DailyCalendar>,
    cx: &mut gpui::Context<StoredWindow>,
) -> Entity<Backups> {
    let source = match session.borrow().folder() {
        Some((root, state)) => Source::Folder {
            root: root.to_owned(),
            state: state.to_owned(),
        },
        None => Source::Store {
            database: database.to_owned(),
        },
    };
    let label = clock_label(calendar.unwrap_or_else(DailyCalendar::utc));
    cx.new(|cx| {
        Backups::new(
            source,
            notebook,
            Arc::new(|root: &Path| folder::state_folder(root)),
            label,
            cx,
        )
    })
}

impl StoredWindow {
    pub(super) fn watch_backups(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        cx.subscribe_in(&self.backups, window, Self::open_restored)
            .detach();
    }

    pub(super) fn toggle_backups(&mut self, cx: &mut gpui::Context<Self>) {
        self.backups.update(cx, |backups, cx| backups.toggle(cx));
    }

    pub(super) fn register_backups(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        use gpui::InteractiveElement as _;
        div.on_action(cx.listener(|this, _: &ShowBackups, _, cx| this.toggle_backups(cx)))
            .on_action(cx.listener(|this, _: &ChooseBackupFolder, _, cx| {
                this.backups
                    .update(cx, |backups, cx| backups.choose_folder(cx))
            }))
            .on_action(cx.listener(|this, _: &BackUpNow, _, cx| {
                this.backups
                    .update(cx, |backups, cx| backups.back_up(false, cx))
            }))
    }

    /// Makes the restored notebook the notebook of the profile holding this one, as the reference
    /// app opens a restore "as this profile"; the backup folder stays and the old notebook's
    /// files stay where they are.
    fn join_profile(&mut self, location: &Location, cx: &mut gpui::Context<Self>) {
        let Some(id) = self
            .location
            .as_ref()
            .and_then(|notebook| crate::profiles::holding(notebook, cx))
        else {
            return;
        };
        let restored = match location.clone() {
            Location::Store {
                database,
                notebook,
                writer,
            } => Notebook::Store {
                database,
                notebook,
                writer,
            },
            Location::Folder { root, .. } => Notebook::Folder { root },
        };
        if let Err(error) = crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(id, restored)?;
            profiles.activate(id)
        }) {
            crate::outline::show_notice(
                format!(
                    "The restored notebook opens without a profile: {error} Choose it again from File > Profile to back it up."
                ),
                cx,
            );
        }
    }

    /// Opens the restored notebook in a new window at the page shown here, then closes this
    /// window once its pages are secured. This notebook's data stays as it was.
    fn open_restored(
        &mut self,
        _: &Entity<Backups>,
        Restored(location): &Restored,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.join_profile(location, cx);
        let page = self.page.read(cx);
        let opened = match location.clone() {
            Location::Store {
                database,
                notebook,
                writer,
            } => OpenOptions {
                database,
                notebook,
                writer,
                page: page.id().to_owned(),
            }
            .open(cx),
            Location::Folder { root, state } => {
                let path = page.path(cx).filter(|path| root.join(path).is_file());
                folder::show_folder_window(root, state, path, Vec::new(), cx)
            }
        };
        match opened {
            Ok(()) if self.session.borrow().is_secured(cx) => {
                cx.defer_in(window, |_, window, _| window.remove_window());
            }
            Ok(()) => {
                self.nav.error = Some(
                    "The restored notebook opened in a new window. This window stays open until its unsaved edits are recorded; close it when they are."
                        .into(),
                );
            }
            Err(error) => {
                self.nav.error = Some(
                    format!("The backup was restored but its notebook did not open: {error}")
                        .into(),
                );
            }
        }
        cx.notify();
    }
}
