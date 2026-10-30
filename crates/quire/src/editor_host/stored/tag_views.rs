//! The Tags browser and tag pages inside the stored window: built on first visit, refreshed
//! whenever the index changes while they are shown.

use super::*;
use crate::navigation::{
    tag_page::{TagPageEvent, TagPageView},
    tags::{TagsEvent, TagsView},
};
use quire_core::workspace::{BrowserView, Destination};
use quire_store::index::tag_key;

#[derive(Default)]
pub(in crate::editor_host::stored) struct TagViews {
    pub(in crate::editor_host::stored) list: Option<Entity<TagsView>>,
    /// Tag pages by tag key, kept for the session so each keeps its filter.
    pub(in crate::editor_host::stored) pages: HashMap<String, Entity<TagPageView>>,
    pub(in crate::editor_host::stored) shown: Option<String>,
}

impl StoredWindow {
    pub(in crate::editor_host::stored) fn show_tags(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let list =
            match self.nav.tags.list.clone() {
                Some(list) => list,
                None => {
                    let session = self.session.clone();
                    let writers: crate::navigation::tag_edits::Writers = Rc::new(move |_| {
                        let session = session.borrow();
                        (session.open_pages(), session.storage())
                    });
                    let list = cx.new(|cx| TagsView::new(writers, window, cx));
                    cx.subscribe_in(&list, window, |this, _, event: &TagsEvent, window, cx| {
                        match event {
                            TagsEvent::Open(tag) => {
                                this.visit(Destination::Tag { tag: tag.clone() }, window, cx)
                            }
                        }
                    })
                    .detach();
                    self.nav.tags.list = Some(list.clone());
                    list
                }
            };
        self.refresh_tags(cx);
        window.focus(&list.focus_handle(cx), cx);
    }

    pub(in crate::editor_host::stored) fn show_tag(
        &mut self,
        tag: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let key = tag_key(&tag);
        let page = match self.nav.tags.pages.get(&key).cloned() {
            Some(page) => page,
            None => {
                let page = cx.new(|cx| TagPageView::new(tag, window, cx));
                cx.subscribe_in(
                    &page,
                    window,
                    |this, _, event: &TagPageEvent, window, cx| match event {
                        TagPageEvent::Open { page, beside } => {
                            this.open_page(page.clone(), *beside, window, cx)
                        }
                    },
                )
                .detach();
                self.nav.tags.pages.insert(key, page.clone());
                page
            }
        };
        self.refresh_tags(cx);
        window.focus(&page.focus_handle(cx), cx);
    }

    /// Re-runs every query shown on screen, after the index changed or restarted.
    pub(in crate::editor_host::stored) fn refresh_queries(&mut self, cx: &mut gpui::Context<Self>) {
        self.refresh_tags(cx);
    }

    /// Re-runs the queries of the tag views on screen.
    pub(in crate::editor_host::stored) fn refresh_tags(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(client) = self.index_client(cx) else {
            return;
        };
        if self.nav.view == Some(BrowserView::Tags)
            && let Some(list) = &self.nav.tags.list
        {
            list.update(cx, |list, cx| list.refresh(&client, cx));
        }
        if let Some(page) = self.shown_tag_page() {
            page.update(cx, |page, cx| page.refresh(&client, cx));
        }
    }

    pub(in crate::editor_host::stored) fn shown_tag_page(&self) -> Option<Entity<TagPageView>> {
        let tag = self.nav.tags.shown.as_ref()?;
        self.nav.tags.pages.get(&tag_key(tag)).cloned()
    }

    pub(in crate::editor_host::stored) fn tag_content(&self) -> Option<gpui::AnyElement> {
        if self.nav.view == Some(BrowserView::Tags) {
            return self
                .nav
                .tags
                .list
                .clone()
                .map(|list| list.into_any_element());
        }
        self.shown_tag_page().map(|page| page.into_any_element())
    }
}
