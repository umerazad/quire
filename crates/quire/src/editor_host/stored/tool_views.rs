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
                Some(LiveNotebook {
                    open_pages: session.open_pages(),
                    storage: session.storage(),
                    index,
                    today: window.today(),
                    calendar: window.nav.daily.calendar.clone(),
                })
            }),
            cx,
        );
    }
}
