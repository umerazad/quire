//! Navigation inside the stored-notebook window: the find palette, opening and creating pages,
//! and the notebook-scoped workspace that records where the reader has been.

use super::*;
use crate::navigation::{
    FindPage, NewPage, WorkspaceFile, create_page,
    palette::{PagePalette, PaletteChoice, PaletteMode},
};
use gpui::{DismissEvent, EntityId, SharedString, Task};
use picker::Picker;
use quire_core::workspace::{Destination, History, Workspace};
use quire_store::index::{IndexClient, ListedPage};
use std::collections::HashSet;

pub(super) struct Navigation {
    file: WorkspaceFile,
    pub(super) workspace: Workspace,
    pub(super) history: History,
    editors: HashMap<EntityId, Entity<NotebookWindow>>,
    watched: HashSet<EntityId>,
    pub(super) palette: Option<Entity<Picker<PagePalette>>>,
    pub(super) pages: Vec<ListedPage>,
    catalogue: Option<Task<()>>,
    saving: Option<Task<()>>,
    creating: Option<Task<()>>,
    pub(super) error: Option<SharedString>,
    pub(super) side: Option<SidePane>,
}

impl Navigation {
    pub(super) fn new(database: &std::path::Path, notebook: &str) -> Self {
        let file = WorkspaceFile::beside(database, notebook);
        Self {
            workspace: file.load(),
            file,
            history: History::default(),
            editors: HashMap::new(),
            watched: HashSet::new(),
            palette: None,
            pages: Vec::new(),
            catalogue: None,
            saving: None,
            creating: None,
            error: None,
            side: None,
        }
    }

    pub(super) fn title(&self, page: &str) -> Option<&str> {
        self.pages
            .iter()
            .find(|listed| listed.page.page.as_str() == Some(page))
            .map(|listed| listed.page.title.as_str())
    }
}

impl StoredWindow {
    pub(super) fn index_client(&self, cx: &App) -> Option<IndexClient> {
        self.index
            .as_ref()
            .ok()
            .map(|index| index.read(cx).client().clone())
    }

    /// Observes a page so its editor attaches once loaded, and feeds it to the index.
    pub(super) fn watch(
        &mut self,
        page: &Entity<OpenPage>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if !self.nav.watched.insert(page.entity_id()) {
            return;
        }
        cx.observe_in(page, window, |this, _, window, cx| {
            this.attach_editor(window, cx);
            this.finish_close(window, cx);
            cx.notify();
        })
        .detach();
        if let Ok(index) = &self.index {
            index.update(cx, |index, cx| index.track(page.clone(), cx));
        }
    }

    pub(super) fn cached_editor(&self, page: &Entity<OpenPage>) -> Option<Entity<NotebookWindow>> {
        self.nav.editors.get(&page.entity_id()).cloned()
    }

    pub(super) fn cache_editor(&mut self, page: &Entity<OpenPage>, editor: Entity<NotebookWindow>) {
        self.nav.editors.insert(page.entity_id(), editor);
    }

    pub(super) fn refresh_catalogue(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(client) = self.index_client(cx) else {
            return;
        };
        self.nav.catalogue = Some(cx.spawn(async move |this, cx| {
            let pages = match client.pages() {
                Ok(reply) => reply.await.map_err(|error| format!("{error:#}")),
                Err(error) => Err(error.to_string()),
            };
            this.update(cx, |this, cx| {
                match pages {
                    Ok(pages) => this.nav.pages = pages,
                    Err(error) => this.nav.error = Some(
                        format!(
                            "Cannot list pages: {error}. Wait for indexing to finish, then retry."
                        )
                        .into(),
                    ),
                }
                this.nav.catalogue = None;
                cx.notify();
            })
            .ok();
        }));
    }

    fn persist(&mut self, cx: &mut gpui::Context<Self>) {
        let file = self.nav.file.clone();
        let workspace = self.nav.workspace.clone();
        let previous = self.nav.saving.take();
        self.nav.saving = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx
                .background_spawn(async move { file.save(&workspace) })
                .await;
            this.update(cx, |this, cx| {
                if let Err(error) = result {
                    this.nav.error = Some(error.into());
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Records a navigation in history, recents, and tabs, then shows it.
    pub(super) fn visit(
        &mut self,
        destination: Destination,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.nav.history.push(destination.clone());
        self.nav.workspace.visit(destination.clone());
        self.show(&destination, window, cx);
        self.persist(cx);
    }

    pub(super) fn show(
        &mut self,
        destination: &Destination,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(id) = destination.page_id() {
            let page = self.session.borrow_mut().open_page(id.to_owned(), cx);
            self.watch(&page, window, cx);
            self.page = page;
            self.editor = self.cached_editor(&self.page);
            self.attach_editor(window, cx);
            if let Some(editor) = &self.editor {
                let focus = editor.read(cx).source.focus_handle(cx);
                window.focus(&focus, cx);
            }
            self.update_title(window, cx);
        }
        cx.notify();
    }

    pub(super) fn update_title(&self, window: &mut Window, cx: &App) {
        let page = self.page.read(cx);
        let title = page
            .loaded()
            .and_then(|loaded| loaded.path.as_deref())
            .unwrap_or("Unsaved page");
        window.set_window_title(&format!("{title} · Quire Native"));
    }

    pub(super) fn open_palette(
        &mut self,
        mode: PaletteMode,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(client) = self.index_client(cx) else {
            self.nav.error = Some(
                "Find needs the notebook index, which did not start. Reopen the notebook to retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        let this = cx.entity().downgrade();
        let choose = Rc::new(move |choice, beside, window: &mut Window, cx: &mut App| {
            let this = this.clone();
            window.defer(cx, move |window, cx| {
                this.update(cx, |this, cx| this.choose(choice, beside, window, cx))
                    .ok();
            });
        });
        let picker = cx.new(|cx| {
            Picker::uniform_list(PagePalette::new(client, mode, choose), window, cx)
                .reopenable(false, cx)
        });
        cx.subscribe_in(&picker, window, |this, _, _: &DismissEvent, window, cx| {
            this.nav.palette = None;
            this.focus_active(window, cx);
            cx.notify();
        })
        .detach();
        picker.update(cx, |picker, cx| picker.focus(window, cx));
        self.nav.palette = Some(picker);
        cx.notify();
    }

    pub(super) fn focus_active(&self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(editor) = &self.editor {
            let focus = editor.read(cx).source.focus_handle(cx);
            window.focus(&focus, cx);
        }
    }

    fn choose(
        &mut self,
        choice: PaletteChoice,
        beside: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match choice {
            PaletteChoice::Open { page, .. } => self.open_page(page, beside, window, cx),
            PaletteChoice::Create { title } => {
                self.create(title, String::new(), beside, window, cx)
            }
        }
    }

    pub(super) fn open_page(
        &mut self,
        page: String,
        beside: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if beside {
            self.open_beside(page, window, cx);
        } else {
            self.visit(Destination::page(page), window, cx);
        }
    }

    pub(super) fn create(
        &mut self,
        title: String,
        folder: String,
        beside: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(index) = self.index_client(cx) else {
            return;
        };
        let storage = self.session.borrow().storage();
        let work = create_page(storage, index, title, folder);
        self.nav.creating = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.nav.creating = None;
                match result {
                    Ok(page) => {
                        this.nav.error = None;
                        this.open_page(page, beside, window, cx);
                        this.refresh_catalogue(cx);
                    }
                    Err(error) => this.nav.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(super) fn register_navigation(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        div.key_context(crate::navigation::NAVIGATION_CONTEXT)
            .on_action(cx.listener(|this, _: &FindPage, window, cx| {
                this.open_palette(PaletteMode::Find, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NewPage, window, cx| {
                this.open_palette(PaletteMode::Create, window, cx)
            }))
    }
}

/// The second pane beside the main page. Holds a page only, as in the reference.
pub(super) struct SidePane {
    pub(super) page: Entity<OpenPage>,
    pub(super) editor: Option<Entity<NotebookWindow>>,
}

impl StoredWindow {
    pub(super) fn open_beside(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let page = self.session.borrow_mut().open_page(id.clone(), cx);
        self.watch(&page, window, cx);
        self.nav.side = Some(SidePane { page, editor: None });
        self.nav.workspace.side = Some(id);
        self.attach_side(window, cx);
        self.persist(cx);
        cx.notify();
    }

    pub(super) fn close_side(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.nav.side = None;
        self.nav.workspace.side = None;
        self.focus_active(window, cx);
        self.persist(cx);
        cx.notify();
    }

    /// Builds the side pane's own editor once its page loads. Two views of one page keep one
    /// shared buffer, so edits on either side reach the same persistence stream.
    pub(super) fn attach_side(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let Some(side) = &self.nav.side else {
            return;
        };
        if side.editor.is_some() {
            return;
        }
        let page = side.page.clone();
        let Some(analysis) = page.read(cx).loaded().map(|loaded| loaded.analysis.clone()) else {
            return;
        };
        let writes = page.read(cx).persistence().is_some();
        let editor = cx.new(|cx| {
            let editor = NotebookWindow::for_page(analysis, None, window, cx);
            if writes {
                editor
                    .commands
                    .update(cx, |commands, _| commands.enable_writes());
            }
            editor
        });
        let focus = editor.read(cx).source.focus_handle(cx);
        window.focus(&focus, cx);
        if let Some(side) = &mut self.nav.side {
            side.editor = Some(editor);
        }
    }
}

pub(super) fn holds_focus(editor: &Entity<NotebookWindow>, window: &Window, cx: &App) -> bool {
    let editor = editor.read(cx);
    editor.source.focus_handle(cx).contains_focused(window, cx)
        || editor.second.focus_handle(cx).contains_focused(window, cx)
}

impl StoredWindow {
    /// Saves the page whose editor holds focus, so Command-S in the side pane saves that page.
    pub(super) fn save_focused(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let side = self.nav.side.as_ref().filter(|side| {
            side.editor
                .as_ref()
                .is_some_and(|editor| holds_focus(editor, window, cx))
        });
        match side.and_then(|side| side.page.read(cx).persistence().cloned()) {
            Some(persistence) => {
                persistence.update(cx, |page, cx| page.save(cx));
                self.close_error = None;
            }
            None => self.save_page(cx),
        }
    }

    pub(super) fn render_body(
        &mut self,
        _window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Div {
        let main = gpui::div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .when_some(self.editor.clone(), |div, editor| {
                div.child(gpui::div().flex_1().min_h_0().child(editor))
            });
        let side = self.nav.side.as_ref().and_then(|side| side.editor.clone());
        gpui::div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(main)
            .when_some(side, |div, editor| {
                div.child(
                    gpui::div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .border_l_1()
                        .border_color(cx.theme().colors().border)
                        .child(editor),
                )
            })
    }
}

#[cfg(test)]
#[path = "navigation_tests.rs"]
mod tests;
