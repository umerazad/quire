//! The Templates page: the pinned sidebar entry that opens `Templates.md`, creating it with the
//! starter templates the first time, and the guide shown beside it.

use super::*;
use crate::query_index::ask;
use crate::templates::guide::TemplatesGuide;
use quire_core::templates::{TEMPLATES_TITLE, guide::starter_templates, is_templates_page};

const NO_INDEX: &str =
    "Templates need the notebook index, which has not started. Wait for indexing, then retry.";

impl StoredWindow {
    /// Whether the main pane shows the Templates page.
    pub(in crate::editor_host::stored) fn showing_templates(&self, cx: &App) -> bool {
        self.nav.view.is_none()
            && self.nav.tags.shown.is_none()
            && self
                .page
                .read(cx)
                .path(cx)
                .is_some_and(|path| is_templates_page(&path))
    }

    /// Opens `Templates.md` at the notebook root, creating it with the starter templates when
    /// the notebook has none.
    pub(in crate::editor_host::stored) fn open_templates(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(NO_INDEX.into());
            cx.notify();
            return;
        };
        let storage = self.session.borrow().storage();
        self.nav.creating = Some(cx.spawn_in(window, async move |this, cx| {
            let found = ask(index.pages()).await.map(|pages| {
                pages
                    .into_iter()
                    .find(|listed| is_templates_page(&listed.page.path))
                    .map(|listed| listed.page.page.to_lossy_string())
            });
            let page = match found {
                Ok(Some(page)) => Ok(page),
                Ok(None) => {
                    crate::navigation::create_page_with(
                        storage,
                        index,
                        TEMPLATES_TITLE.into(),
                        String::new(),
                        starter_templates(),
                    )
                    .await
                }
                Err(error) => Err(format!(
                    "Cannot find the Templates page: {error}. Wait for indexing to finish, then retry."
                )),
            };
            this.update_in(cx, |this, window, cx| {
                this.nav.creating = None;
                match page {
                    Ok(page) => {
                        this.nav.error = None;
                        this.open_page(page, false, window, cx);
                        this.refresh_catalogue(cx);
                    }
                    Err(error) => this.nav.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The guide beside the Templates page, while the main pane shows it.
    pub(in crate::editor_host::stored) fn shown_templates_guide(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> Option<Entity<TemplatesGuide>> {
        if !self.showing_templates(cx) {
            return None;
        }
        let page = self.page.read(cx);
        let id = page.id().to_owned();
        let path = page.path(cx)?;
        let buffer = page.loaded()?.analysis.read(cx).source.clone();
        if let Some((shown, guide)) = &self.nav.templates_guide
            && *shown == id
            && guide.read(cx).shows(&path, &buffer)
        {
            return Some(guide.clone());
        }
        let guide = cx.new(|cx| TemplatesGuide::new(path, buffer, cx));
        self.nav.templates_guide = Some((id, guide.clone()));
        Some(guide)
    }
}
