//! The window's live sync for a Quire notebook with a synced folder: the status line, imported
//! changes reaching open pages, and publishing before a close.

use super::*;
use crate::live_sync::{AttachmentsArrived, Imported, LiveSync, LiveSyncView};
use crate::notebook_backend::NotebookBackend;
use quire_store::profiles::Notebook;

/// Live sync for a Quire notebook whose synced folder Quire knows; nothing otherwise.
pub(super) fn new_live_sync(
    session: &Rc<RefCell<NotebookSession>>,
    location: Option<&Notebook>,
    calendar: Option<DailyCalendar>,
    cx: &mut gpui::Context<StoredWindow>,
) -> Option<(Entity<LiveSync>, Entity<LiveSyncView>)> {
    let Some(Notebook::Store {
        writer,
        folder: Some(folder),
        ..
    }) = location
    else {
        return None;
    };
    let NotebookBackend::Store(client) = session.borrow().storage() else {
        return None;
    };
    let sync = crate::live_sync::start(client, folder.clone(), writer.clone(), cx);
    let time = backups::clock_label(calendar.unwrap_or_else(DailyCalendar::utc));
    let view = cx.new(|cx| LiveSyncView::new(sync.clone(), time, cx));
    Some((sync, view))
}

impl StoredWindow {
    /// Open pages take in what other writers changed, and the page list follows.
    pub(super) fn watch_live_sync(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let Some(live) = self.live.clone() else {
            return;
        };
        cx.subscribe_in(&live, window, |this, _, Imported(pages), _, cx| {
            let open: Vec<_> = pages
                .iter()
                .filter_map(|page| {
                    this.session
                        .borrow()
                        .open_pages()
                        .into_iter()
                        .find(|open| open.read(cx).id() == page)
                })
                .filter_map(|page| page.read(cx).persistence().cloned())
                .collect();
            for persistence in open {
                persistence.update(cx, |persistence, cx| persistence.store_moved(cx));
            }
            this.refresh_catalogue(cx);
        })
        .detach();
        cx.subscribe(&live, |_, _, _: &AttachmentsArrived, cx| {
            crate::media::load::files_arrived(cx)
        })
        .detach();
    }

    /// A close began: publish saved changes now instead of on schedule.
    pub(super) fn prepare_live_close(&mut self, cx: &mut gpui::Context<Self>) {
        if let (Some(live), Some(since)) = (self.live.clone(), self.close.since.get()) {
            live.update(cx, |live, cx| live.prepare_close(since, cx));
        }
    }
}
