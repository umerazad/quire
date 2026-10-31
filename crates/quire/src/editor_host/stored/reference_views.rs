//! The linked references panel under the page in each pane: one view per page for the
//! session, so each page keeps its own filter, refreshed whenever the index changes while it is
//! shown.

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

    /// The side pane's page, while it differs from a page shown in the main pane, whose panel
    /// is already drawn there.
    fn side_only(&self, cx: &App) -> Option<String> {
        let side = self.nav.side.as_ref()?;
        side.editor.as_ref()?;
        let id = side.page.read(cx).id();
        (self.shown_references(cx).is_none() || id != self.page.read(cx).id())
            .then(|| id.to_owned())
    }

    /// The linked tasks and references of the page in the side pane, drawn under its editor as
    /// the main pane draws its own.
    pub(in crate::editor_host::stored) fn side_references(
        &self,
        cx: &App,
    ) -> Option<(Entity<LinkedTasksView>, Entity<ReferencesView>)> {
        let id = self.side_only(cx)?;
        Some((
            self.nav.linked_tasks.get(&id)?.clone(),
            self.nav.references.get(&id)?.clone(),
        ))
    }

    /// The side pane's linked tasks above its linked references, or the matching tasks alone
    /// while a task status is chosen, as under the main page.
    pub(in crate::editor_host::stored) fn render_side_references(
        &self,
        cx: &App,
    ) -> Option<gpui::Div> {
        let (tasks, references) = self.side_references(cx)?;
        let listing = tasks.read(cx).lists_tasks();
        Some(
            gpui::div()
                .flex()
                .flex_col()
                .debug_selector(|| "quire-side-references".into())
                .child(tasks)
                .when(!listing, |panel| panel.child(references)),
        )
    }

    /// Whether a source opened from `page`'s panel goes to the side pane: a panel in the side
    /// pane opens in its own pane, as the main pane's does, and Shift opens in the other.
    fn opens_beside(&self, page: &str, beside: bool, cx: &App) -> bool {
        beside != (self.side_only(cx).as_deref() == Some(page))
    }

    pub(in crate::editor_host::stored) fn show_references(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let id = self.page.read(cx).id().to_owned();
        self.reference_views(id, window, cx);
        if let Some(side) = self.side_only(cx) {
            self.reference_views(side, window, cx);
        }
        self.refresh_references(cx);
    }

    /// Builds the side pane's panel once its editor exists.
    pub(in crate::editor_host::stored) fn show_side_references(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(id) = self.side_only(cx) {
            self.reference_views(id, window, cx);
            self.refresh_references(cx);
        }
    }

    /// The panel views for page `id`, built on first use and kept for the session.
    fn reference_views(&mut self, id: String, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if !self.nav.references.contains_key(&id) {
            let oldest = self.nav.workspace.is_oldest_first(&id);
            let view = cx.new(|cx| ReferencesView::new(id.clone(), oldest, window, cx));
            let owner = id.clone();
            cx.subscribe_in(
                &view,
                window,
                move |this, view, event: &ReferencesEvent, window, cx| match event {
                    ReferencesEvent::Open { page, jump, beside } => {
                        let beside = this.opens_beside(&owner, *beside, cx);
                        this.open_page_at(page.clone(), jump.clone(), beside, window, cx)
                    }
                    ReferencesEvent::Order { oldest } => {
                        this.nav.workspace.set_oldest_first(&owner, *oldest);
                        this.persist(cx);
                        view.update(cx, |view, cx| view.set_oldest_first(*oldest, cx));
                    }
                    ReferencesEvent::Synthesize => {
                        let Some(title) = this.nav.title(&owner).map(str::to_owned) else {
                            return;
                        };
                        let seed = crate::ai::answer::Seed {
                            title: Some(format!("Linked references to {title}")),
                            scope: Some(crate::ai::answer::Scope::References(title)),
                            ..Default::default()
                        };
                        this.open_answer(seed, window, cx);
                    }
                    ReferencesEvent::Link { link, beside } => {
                        let beside = this.opens_beside(&owner, *beside, cx);
                        this.open_row_link(link, beside, window, cx)
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
            let owner = id.clone();
            cx.subscribe_in(
                &view,
                window,
                move |this, _, event: &LinkedTasksEvent, window, cx| match event {
                    LinkedTasksEvent::Open { page, jump, beside } => {
                        let beside = this.opens_beside(&owner, *beside, cx);
                        this.open_page_at(page.clone(), Some(jump.clone()), beside, window, cx)
                    }
                    LinkedTasksEvent::Link { link, beside } => {
                        let beside = this.opens_beside(&owner, *beside, cx);
                        this.open_row_link(link, beside, window, cx)
                    }
                },
            )
            .detach();
            self.nav.linked_tasks.insert(id, view);
        }
    }

    /// Re-runs the panels shown in either pane.
    pub(in crate::editor_host::stored) fn refresh_references(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(client) = self.index_client(cx) else {
            return;
        };
        let main = self.shown_linked_tasks(cx).zip(self.shown_references(cx));
        let today = self.today().unwrap_or_default();
        for (tasks, view) in main.into_iter().chain(self.side_references(cx)) {
            view.update(cx, |view, cx| view.refresh(&client, cx));
            let today = today.clone();
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
