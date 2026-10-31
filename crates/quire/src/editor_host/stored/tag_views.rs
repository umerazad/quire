//! The Tags browser and tag pages inside the stored window: built on first visit, refreshed
//! whenever the index changes while they are shown.

use super::*;
use crate::navigation::{
    tag_page::{TagPageEvent, TagPageView},
    tags::{TagsEvent, TagsView},
};
use quire_core::workspace::{BrowserView, Destination, TagChoices};
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
                    let choices = self.nav.workspace.tag_list.clone();
                    let list = cx.new(|cx| TagsView::new(choices, writers, window, cx));
                    cx.subscribe_in(&list, window, |this, _, event: &TagsEvent, window, cx| {
                        match event {
                            TagsEvent::Open(tag) => {
                                this.visit(Destination::Tag { tag: tag.clone() }, window, cx)
                            }
                            TagsEvent::Remember(choices) => {
                                let choices = choices.clone();
                                this.remember(|workspace| workspace.tag_list = choices, cx)
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
                let choices = self
                    .nav
                    .workspace
                    .tag_pages
                    .get(&key)
                    .cloned()
                    .unwrap_or_default();
                let page = cx.new(|cx| TagPageView::new(tag, choices, window, cx));
                cx.subscribe_in(
                    &page,
                    window,
                    |this, _, event: &TagPageEvent, window, cx| match event {
                        TagPageEvent::Open { page, jump, beside } => {
                            this.open_page_at(page.clone(), jump.clone(), *beside, window, cx)
                        }
                        TagPageEvent::Remember => this.remember_tag_page(cx),
                        TagPageEvent::Link { link, beside } => {
                            this.open_row_link(link, *beside, window, cx)
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
        let index = self.index_client(cx);
        self.links
            .host
            .update(cx, |host, cx| host.changed(index, cx));
        self.refresh_tags(cx);
        self.refresh_references(cx);
        self.refresh_tasks(cx);
        self.follow_rename(cx);
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
            let today = self.today().unwrap_or_default();
            page.update(cx, |page, cx| page.refresh(&client, today, cx));
        }
    }

    /// Records the shown tag page's filter and first row shown.
    pub(in crate::editor_host::stored) fn remember_tag_page(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some((key, page)) = self
            .nav
            .tags
            .shown
            .as_deref()
            .map(tag_key)
            .zip(self.shown_tag_page())
        else {
            return;
        };
        let choices = page.read(cx).choices(cx);
        self.remember(
            |workspace| match choices == TagChoices::default() {
                true => _ = workspace.tag_pages.remove(&key),
                false => _ = workspace.tag_pages.insert(key, choices),
            },
            cx,
        );
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
