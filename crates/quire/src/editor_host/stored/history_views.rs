//! Opening page history for the shown page, and following a restore: the restored page shows its
//! new version through its open buffer or the page list, and a restored copy opens. Also the
//! toolbar's Attach…, the other page action that acts on the shown page's editor.

use super::*;
use crate::navigation::page_history::{Clock, PageHistory, PageHistoryEvent};
use gpui::DismissEvent;

impl StoredWindow {
    /// Save times in the notebook's time zone, as a date and a 24-hour time.
    fn history_label(&self) -> Rc<dyn Fn(i64) -> String> {
        super::backups::clock_label(
            self.nav
                .daily
                .calendar
                .clone()
                .unwrap_or_else(quire_core::daily::DailyCalendar::utc),
        )
    }

    /// Attach… in the page toolbar, the reference Page actions > Add attachment: asks for files
    /// and links them at the page's caret, as Mod-Alt-A does there.
    pub(in crate::editor_host::stored) fn attach_files(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.focus_active(window, cx);
        window.dispatch_action(Box::new(crate::media::attach::AttachFile), cx);
    }

    pub(in crate::editor_host::stored) fn show_history(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(page) = self.shown_page(cx) else {
            return;
        };
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Page history needs the notebook index, which did not start. Reopen the notebook to retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        let Some(path) = self.page.read(cx).path(cx) else {
            return;
        };
        let session = self.session.clone();
        let writers: crate::navigation::tag_edits::Writers = Rc::new(move |_| {
            let session = session.borrow();
            (session.open_pages(), session.storage())
        });
        let clock = Clock {
            label: self.history_label(),
            today: self.today().unwrap_or_default(),
        };
        let history = cx.new(|cx| PageHistory::new(page, path, writers, index, clock, window, cx));
        cx.subscribe_in(&history, window, |this, _, _: &DismissEvent, window, cx| {
            this.nav.page_history = None;
            this.focus_active(window, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &history,
            window,
            |this, _, event: &PageHistoryEvent, window, cx| {
                this.refresh_catalogue(cx);
                if let PageHistoryEvent::Copied(page) = event {
                    this.open_page(page.clone(), false, window, cx);
                }
            },
        )
        .detach();
        window.focus(&history.focus_handle(cx), cx);
        self.nav.page_history = Some(history);
        cx.notify();
    }
}
