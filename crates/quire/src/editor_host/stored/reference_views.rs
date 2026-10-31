//! The linked references panel under the main page: one view per page for the session, so
//! each page keeps its own filter, refreshed whenever the index changes while it is shown.

use super::*;
use crate::navigation::references::{ReferencesEvent, ReferencesView};

impl StoredWindow {
    /// The panel for the page in the main pane, while a page (not a browser) is shown.
    pub(in crate::editor_host::stored) fn shown_references(
        &self,
        cx: &App,
    ) -> Option<Entity<ReferencesView>> {
        if self.nav.view.is_some() || self.nav.tags.shown.is_some() {
            return None;
        }
        self.nav.references.get(self.page.read(cx).id()).cloned()
    }

    pub(in crate::editor_host::stored) fn show_references(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let id = self.page.read(cx).id().to_owned();
        if !self.nav.references.contains_key(&id) {
            let oldest = self.nav.workspace.is_oldest_first(&id);
            let view = cx.new(|cx| ReferencesView::new(id.clone(), oldest, window, cx));
            let page = id.clone();
            cx.subscribe_in(
                &view,
                window,
                move |this, view, event: &ReferencesEvent, window, cx| match event {
                    ReferencesEvent::Open { page, jump, beside } => {
                        this.open_page_at(page.clone(), jump.clone(), *beside, window, cx)
                    }
                    ReferencesEvent::Order { oldest } => {
                        this.nav.workspace.set_oldest_first(&page, *oldest);
                        this.persist(cx);
                        view.update(cx, |view, cx| view.set_oldest_first(*oldest, cx));
                    }
                    ReferencesEvent::Synthesize => {
                        let Some(title) = this.nav.title(&page).map(str::to_owned) else {
                            return;
                        };
                        let seed = crate::ai::answer::Seed {
                            title: Some(format!("Linked references to {title}")),
                            references: Some(title),
                            ..Default::default()
                        };
                        this.open_answer(seed, window, cx);
                    }
                },
            )
            .detach();
            self.nav.references.insert(id, view);
        }
        self.refresh_references(cx);
    }

    pub(in crate::editor_host::stored) fn refresh_references(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) {
        let (Some(client), Some(view)) = (self.index_client(cx), self.shown_references(cx)) else {
            return;
        };
        view.update(cx, |view, cx| view.refresh(&client, cx));
    }

    pub(in crate::editor_host::stored) fn focus_references(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(view) = self.shown_references(cx) {
            window.focus(&view.focus_handle(cx), cx);
        }
    }
}
