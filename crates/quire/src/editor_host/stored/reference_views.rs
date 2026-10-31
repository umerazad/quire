//! The linked references panel under the main page: one view per page for the session, so
//! each page keeps its own filter, refreshed whenever the index changes while it is shown.

use super::*;
use crate::navigation::{
    linked_tasks::{LinkedTasksEvent, LinkedTasksView},
    references::{ReferencesEvent, ReferencesView},
};
use crate::rich_text::RowLink;

impl StoredWindow {
    /// Opens what a link in a result row names: the page or block it links, the tag's page, or
    /// a web address in the browser. This is `RowLink::open` for views the window owns, which
    /// cannot reach the window through its notebook while it handles their events.
    pub(in crate::editor_host::stored) fn open_row_link(
        &mut self,
        link: &RowLink,
        beside: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match link {
            RowLink::Reference(target) => self.follow(
                crate::references::Follow {
                    target: target.clone(),
                    beside,
                    backlinks: false,
                },
                window,
                cx,
            ),
            RowLink::Tag(tag) => self.visit(
                quire_core::workspace::Destination::Tag { tag: tag.clone() },
                window,
                cx,
            ),
            RowLink::Url(url) => cx.open_url(url),
        }
    }

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

    /// Tasks linking to the page in the main pane, shown above its linked references.
    pub(in crate::editor_host::stored) fn shown_linked_tasks(
        &self,
        cx: &App,
    ) -> Option<Entity<LinkedTasksView>> {
        self.shown_references(cx)?;
        self.nav.linked_tasks.get(self.page.read(cx).id()).cloned()
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
                    ReferencesEvent::Link { link, beside } => {
                        this.open_row_link(link, *beside, window, cx)
                    }
                },
            )
            .detach();
            self.nav.references.insert(id.clone(), view);
        }
        if !self.nav.linked_tasks.contains_key(&id) {
            let session = self.session.clone();
            let writers: crate::navigation::tag_edits::Writers = Rc::new(move |_| {
                let session = session.borrow();
                (session.open_pages(), session.storage())
            });
            let view = cx.new(|cx| LinkedTasksView::new(id.clone(), writers, cx));
            cx.subscribe_in(
                &view,
                window,
                |this, _, event: &LinkedTasksEvent, window, cx| match event {
                    LinkedTasksEvent::Open { page, jump, beside } => {
                        this.open_page_at(page.clone(), Some(jump.clone()), *beside, window, cx)
                    }
                    LinkedTasksEvent::Link { link, beside } => {
                        this.open_row_link(link, *beside, window, cx)
                    }
                },
            )
            .detach();
            self.nav.linked_tasks.insert(id, view);
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
        if let Some(tasks) = self.shown_linked_tasks(cx) {
            let today = self.today().unwrap_or_default();
            tasks.update(cx, |tasks, cx| tasks.refresh(&client, today, cx));
        }
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
