//! Offers this window's notebook to the AI notebook tools: its open pages, storage, index,
//! clock, and capture destination, read fresh on every call.

use super::*;
use crate::notebook_tools::live::{self, LiveNotebook};

impl StoredWindow {
    pub(in crate::editor_host::stored) fn offer_notebook_tools(
        &self,
        cx: &mut gpui::Context<Self>,
    ) {
        let this = cx.weak_entity();
        live::register(
            Rc::new(move |cx| {
                let this = this.upgrade()?;
                let window = this.read(cx);
                let index = window.index_client(cx)?;
                let session = window.session.try_borrow().ok()?;
                let identity = session.folder().map_or_else(
                    || window.nav.database.display().to_string(),
                    |(root, _)| root.display().to_string(),
                );
                Some(LiveNotebook {
                    identity,
                    open_pages: session.open_pages(),
                    storage: session.storage(),
                    index,
                    today: window.today(),
                    calendar: window.nav.daily.calendar.clone(),
                    daily: window.today_page(),
                    capture: window.capture_to(cx),
                })
            }),
            cx,
        );
    }
}
