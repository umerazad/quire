//! Connects page editors' references to this window: following links, opening pages that
//! embeds and new block IDs write through, and telling editors when the index changed.

use super::*;
use crate::query_index::ask;
use crate::references::{Follow, Notebook, ReferenceHost, Target};
use quire_core::workspace::Destination;
use quire_store::index::PageRef;

pub(super) struct Links {
    pub(super) host: Entity<ReferenceHost>,
    following: Option<gpui::Task<()>>,
}

impl Links {
    pub(super) fn new(window: gpui::WeakEntity<StoredWindow>, cx: &mut App) -> Self {
        Self {
            host: cx.new(|_| ReferenceHost::new(Rc::new(WindowNotebook(window)))),
            following: None,
        }
    }

    #[cfg(test)]
    pub(super) fn idle(&self) -> bool {
        self.following.is_none()
    }
}

struct WindowNotebook(gpui::WeakEntity<StoredWindow>);

impl Notebook for WindowNotebook {
    fn follow(&self, follow: Follow, window: &mut Window, cx: &mut App) {
        self.0
            .update(cx, |this, cx| this.follow(follow, window, cx))
            .ok();
    }

    fn open_page(&self, page: &str, window: &mut Window, cx: &mut App) -> Option<Entity<OpenPage>> {
        self.0
            .update(cx, |this, cx| {
                let page = this.session.borrow_mut().open_page(page.to_owned(), cx);
                this.watch(&page, window, cx);
                page
            })
            .ok()
    }

    fn page_id(&self, buffer: &Entity<language::Buffer>, cx: &App) -> Option<String> {
        let this = self.0.upgrade()?;
        let pages = this.read(cx).session.borrow().open_pages();
        pages.into_iter().find_map(|page| {
            let page = page.read(cx);
            let loaded = page.loaded()?;
            (loaded.analysis.read(cx).source == *buffer).then(|| page.id().to_owned())
        })
    }

    fn page_path(&self, buffer: &Entity<language::Buffer>, cx: &App) -> Option<String> {
        let this = self.0.upgrade()?;
        let pages = this.read(cx).session.borrow().open_pages();
        pages.into_iter().find_map(|page| {
            let loaded = page.read(cx).loaded()?;
            (loaded.analysis.read(cx).source == *buffer)
                .then(|| loaded.path.clone())
                .flatten()
        })
    }

    fn show_page(&self, page: &str, beside: bool, window: &mut Window, cx: &mut App) {
        self.0
            .update(cx, |this, cx| {
                this.open_page(page.to_owned(), beside, window, cx)
            })
            .ok();
    }

    fn writers(
        &self,
        cx: &App,
    ) -> Option<(Vec<Entity<OpenPage>>, quire_store::worker::StorageClient)> {
        let this = self.0.upgrade()?;
        let session = this.read(cx).session.borrow();
        Some((session.open_pages(), session.storage()))
    }
}

impl StoredWindow {
    /// Lets both editors of a page view follow and complete references in this notebook.
    pub(super) fn link_editors(
        &self,
        view: &Entity<NotebookWindow>,
        window: &mut Window,
        cx: &mut App,
    ) {
        let (source, second) = {
            let view = view.read(cx);
            (view.source.clone(), view.second.clone())
        };
        for editor in [source, second] {
            crate::references::install(&editor, &self.links.host, window, cx);
        }
    }

    /// Resolves a reference through the index and opens what it names. A newer request
    /// replaces an unfinished one, so a slow answer never opens a page after a later click.
    pub(super) fn follow(
        &mut self,
        follow: Follow,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(client) = self.index_client(cx) else {
            self.nav.error = Some(
                "Links need the notebook index, which has not started. Wait for indexing, then retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        let lookup = match &follow.target {
            Target::Page(name) => client.find_page(name.clone()),
            Target::Block(id) => client.resolve_block(id.clone()),
        };
        self.links.following = Some(cx.spawn_in(window, async move |this, cx| {
            let found = ask(lookup).await;
            this.update_in(cx, |this, window, cx| {
                this.links.following = None;
                this.arrive(follow, found, window, cx);
                cx.notify();
            })
            .ok();
        }));
    }

    fn arrive(
        &mut self,
        follow: Follow,
        found: Result<Option<PageRef>, String>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match (found, &follow.target) {
            (Ok(Some(page)), _) => {
                self.nav.error = None;
                let id = page.page.to_lossy_string();
                if follow.backlinks {
                    self.visit(Destination::page(id), window, cx);
                    self.focus_references(window, cx);
                } else {
                    self.open_page(id, follow.beside, window, cx);
                }
            }
            (Ok(None), Target::Page(name)) if !follow.backlinks => {
                let title = name.split('#').next().unwrap_or_default().to_owned();
                self.create(title, String::new(), follow.beside, window, cx);
            }
            (Ok(None), Target::Page(name)) => {
                self.nav.error = Some(
                    format!("No page named {name} exists, so nothing links to it yet. Follow the link to create the page.")
                        .into(),
                );
            }
            (Ok(None), Target::Block(id)) => {
                self.nav.error = Some(
                    format!("Block (({id})) is missing or its ID is used by several blocks. Check the ID in Source, then retry.")
                        .into(),
                );
            }
            (Err(error), _) => {
                self.nav.error = Some(
                    format!(
                        "Cannot follow the link: {error}. Wait for indexing to finish, then retry."
                    )
                    .into(),
                );
            }
        }
    }
}
