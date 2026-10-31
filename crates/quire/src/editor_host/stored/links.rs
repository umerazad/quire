//! Connects page editors' references to this window: following links, opening pages that
//! embeds and new block IDs write through, and telling editors when the index changed.

use super::*;
use crate::query_index::ask;
use crate::references::{Follow, Notebook, ReferenceHost, Target};
use quire_core::workspace::Destination;
use quire_store::index::{PageRef, heading_of};

/// The page a followed link opens, and where the heading it names starts there, if it names one.
struct Arrival {
    page: PageRef,
    heading: Option<Result<u32, ()>>,
}

pub(super) struct Links {
    pub(super) host: Entity<ReferenceHost>,
    following: Option<gpui::Task<()>>,
}

impl Links {
    pub(super) fn new(
        window: gpui::WeakEntity<StoredWindow>,
        session: std::rc::Weak<RefCell<NotebookSession>>,
        cx: &mut App,
    ) -> Self {
        let notebook = WindowNotebook { window, session };
        Self {
            host: cx.new(|_| ReferenceHost::new(Rc::new(notebook))),
            following: None,
        }
    }

    #[cfg(test)]
    pub(super) fn idle(&self) -> bool {
        self.following.is_none()
    }
}

/// Reads open pages through the session rather than the window, so editors bound while the window
/// is being updated can still ask which page they show.
struct WindowNotebook {
    window: gpui::WeakEntity<StoredWindow>,
    session: std::rc::Weak<RefCell<NotebookSession>>,
}

impl WindowNotebook {
    fn loaded<T>(
        &self,
        buffer: &Entity<language::Buffer>,
        cx: &App,
        read: impl Fn(&OpenPage, &crate::notebook_session::LoadedPage) -> Option<T>,
    ) -> Option<T> {
        let pages = self.session.upgrade()?.try_borrow().ok()?.open_pages();
        pages.into_iter().find_map(|page| {
            let page = page.read(cx);
            let loaded = page.loaded()?;
            (loaded.analysis.read(cx).source == *buffer)
                .then(|| read(page, loaded))
                .flatten()
        })
    }
}

impl Notebook for WindowNotebook {
    fn follow(&self, follow: Follow, window: &mut Window, cx: &mut App) {
        self.window
            .update(cx, |this, cx| this.follow(follow, window, cx))
            .ok();
    }

    fn open_page(&self, page: &str, window: &mut Window, cx: &mut App) -> Option<Entity<OpenPage>> {
        self.window
            .update(cx, |this, cx| {
                let page = this.session.borrow_mut().open_page(page.to_owned(), cx);
                this.watch(&page, window, cx);
                page
            })
            .ok()
    }

    fn page_id(&self, buffer: &Entity<language::Buffer>, cx: &App) -> Option<String> {
        self.loaded(buffer, cx, |page, _| Some(page.id().to_owned()))
    }

    fn page_path(&self, buffer: &Entity<language::Buffer>, cx: &App) -> Option<String> {
        self.loaded(buffer, cx, |_, loaded| loaded.path.clone())
    }

    fn show_page(&self, page: &str, beside: bool, window: &mut Window, cx: &mut App) {
        self.window
            .update(cx, |this, cx| {
                this.open_page(page.to_owned(), beside, window, cx)
            })
            .ok();
    }

    fn show_tag(&self, tag: &str, window: &mut Window, cx: &mut App) {
        let tag = tag.to_owned();
        self.window
            .update(cx, |this, cx| {
                this.visit(Destination::Tag { tag }, window, cx)
            })
            .ok();
    }

    fn show_page_at(
        &self,
        page: &str,
        jump: crate::navigation::Jump,
        beside: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.window
            .update(cx, |this, cx| {
                this.open_page_at(page.to_owned(), Some(jump), beside, window, cx)
            })
            .ok();
    }

    fn writers(
        &self,
        _: &App,
    ) -> Option<(
        Vec<Entity<OpenPage>>,
        crate::notebook_backend::NotebookBackend,
    )> {
        let session = self.session.upgrade()?;
        let session = session.try_borrow().ok()?;
        Some((session.open_pages(), session.storage()))
    }

    fn prompt(
        &self,
        prompt: crate::templates::prompt::Prompt,
        window: &mut Window,
        cx: &mut App,
    ) -> futures::channel::oneshot::Receiver<Option<String>> {
        let (picker, answer) = crate::templates::prompt::picker(prompt, window, cx);
        self.window
            .update(cx, |this, cx| this.show_template_prompt(picker, window, cx))
            .ok();
        answer
    }

    fn media_files(&self) -> Option<quire_media::files::Files> {
        self.session.upgrade()?.try_borrow().ok()?.media_files()
    }

    /// Beside the notebook's Quire data: the folder notebook's state folder, or the database.
    fn transcripts(&self, cx: &App) -> Option<PathBuf> {
        let session = self.session.upgrade()?;
        if let Some((_, state)) = session.try_borrow().ok()?.folder() {
            return Some(state.join("ai"));
        }
        let window = self.window.upgrade()?;
        Some(window.read(cx).nav.database.with_file_name("quire-ai"))
    }

    fn ask(&self, seed: crate::ai::answer::Seed, window: &mut Window, cx: &mut App) -> bool {
        self.window
            .update(cx, |this, cx| this.open_answer(seed, window, cx))
            .is_ok()
    }

    fn daily_date(&self, title: &str, cx: &App) -> Option<String> {
        self.window.upgrade()?.read(cx).daily_date_of(title)
    }

    fn daily_title(&self, date: &str, cx: &App) -> Option<String> {
        self.window.upgrade()?.read(cx).daily_title_of(date)
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
        let heading = match &follow.target {
            Target::Page(name) if heading_of(name).is_some() => {
                Some(client.find_heading(name.clone()))
            }
            _ => None,
        };
        let lookup = match &follow.target {
            Target::Page(name) => client.find_page(name.clone()),
            Target::Block(id) => client.resolve_block(id.clone()),
        };
        self.links.following = Some(cx.spawn_in(window, async move |this, cx| {
            let found = match heading {
                Some(heading) => ask(heading).await.map(|target| {
                    target.map(|target| Arrival {
                        heading: Some(target.heading.map(|(_, start)| start).ok_or(())),
                        page: target.page,
                    })
                }),
                None => ask(lookup).await.map(|page| {
                    page.map(|page| Arrival {
                        page,
                        heading: None,
                    })
                }),
            };
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
        found: Result<Option<Arrival>, String>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match (found, &follow.target) {
            (Ok(Some(arrival)), _) => {
                self.nav.error = None;
                let id = arrival.page.page.to_lossy_string();
                if follow.backlinks {
                    self.visit(Destination::page(id), window, cx);
                    self.focus_references(window, cx);
                } else {
                    let block = match &follow.target {
                        Target::Block(target) => target.rsplit("#^").next(),
                        Target::Page(name) => name.split_once("#^").map(|(_, id)| id),
                    };
                    let jump = match arrival.heading {
                        Some(Ok(start)) => Some(crate::navigation::Jump::at(None, start)),
                        _ => block.map(crate::navigation::Jump::block),
                    };
                    self.open_page_at(id, jump, follow.beside, window, cx);
                }
                if let (Some(Err(())), Target::Page(name)) = (arrival.heading, &follow.target) {
                    self.nav.error = Some(
                        format!(
                            "{} has no heading “{}”, so it opened at the top. Check the heading in the link.",
                            arrival.page.title,
                            heading_of(name).unwrap_or_default()
                        )
                        .into(),
                    );
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
