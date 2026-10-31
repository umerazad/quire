//! Navigation inside the stored-notebook window: the find palette, opening and creating pages,
//! and the notebook-scoped workspace that records where the reader has been.

use super::*;
use crate::navigation::{
    CaptureTask, CloseTab, DeleteFile, DeletePage, FindPage, FocusFiles, FocusReferences, GoBack,
    GoForward, NewPage, NextTab, OpenToday, PreviousTab, RenameFile, RenamePage, ShowDaily,
    ShowPages, ShowTags, ShowTasks, ToggleFavorite, ToggleSidePane, WorkspaceFile, create_page,
    palette::{PagePalette, PaletteChoice, PaletteMode},
};
use gpui::{DismissEvent, EntityId, SharedString, Task};
use picker::Picker;
use quire_core::workspace::{BrowserView, Destination, History, Workspace};
use quire_store::index::{IndexClient, ListedPage};
use std::collections::HashSet;

#[path = "chrome.rs"]
mod chrome;
#[path = "daily.rs"]
mod daily;
#[path = "file_views.rs"]
mod file_views;
#[path = "reference_views.rs"]
mod reference_views;
#[path = "tag_views.rs"]
mod tag_views;
#[path = "task_views.rs"]
mod task_views;

pub(super) struct Navigation {
    file: WorkspaceFile,
    pub(super) workspace: Workspace,
    pub(super) history: History,
    editors: HashMap<EntityId, Entity<NotebookWindow>>,
    watched: HashSet<EntityId>,
    pub(super) palette: Option<Entity<Picker<PagePalette>>>,
    pub(super) dialog: Option<Entity<crate::navigation::file_dialog::FileDialog>>,
    /// A renamed page, until the page list shows its new path and title.
    pub(super) renamed: Option<crate::file_actions::Renamed>,
    pub(super) pages: Vec<ListedPage>,
    catalogue: Option<Task<()>>,
    saving: Option<Task<()>>,
    saves: usize,
    creating: Option<Task<()>>,
    pub(super) error: Option<SharedString>,
    pub(super) side: Option<SidePane>,
    pub(super) view: Option<BrowserView>,
    pub(super) browsers: chrome::Browsers,
    pub(super) daily: daily::DailyState,
    pub(super) database: std::path::PathBuf,
    pub(super) tags: tag_views::TagViews,
    pub(super) tasks: task_views::TaskViews,
    /// Linked reference panels by page, kept for the session so each keeps its filter.
    pub(super) references: HashMap<String, Entity<crate::navigation::references::ReferencesView>>,
}

impl Navigation {
    pub(super) fn new(
        database: &std::path::Path,
        notebook: &str,
        calendar: Option<quire_core::daily::DailyCalendar>,
    ) -> Self {
        let file = WorkspaceFile::beside(database, notebook);
        Self {
            workspace: file.load(),
            file,
            history: History::default(),
            editors: HashMap::new(),
            watched: HashSet::new(),
            palette: None,
            dialog: None,
            renamed: None,
            pages: Vec::new(),
            catalogue: None,
            saving: None,
            saves: 0,
            creating: None,
            error: None,
            side: None,
            view: None,
            browsers: chrome::Browsers::default(),
            daily: daily::DailyState::new(calendar),
            database: database.to_owned(),
            tags: tag_views::TagViews::default(),
            tasks: task_views::TaskViews::default(),
            references: HashMap::new(),
        }
    }

    #[cfg(test)]
    pub(super) fn saving_idle(&self) -> bool {
        self.saving.is_none()
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
                    Ok(pages) => {
                        this.nav.pages = pages;
                        this.forget_missing_pages(cx);
                    }
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

    /// Drops saved tabs, favorites, recents, and history entries for pages that no longer
    /// exist, as the reference does when a notebook loads. Open pages always count as present.
    fn forget_missing_pages(&mut self, cx: &mut gpui::Context<Self>) {
        let mut present: HashSet<String> = self
            .nav
            .pages
            .iter()
            .map(|listed| listed.page.page.to_lossy_string())
            .collect();
        present.extend(
            self.session
                .borrow()
                .open_pages()
                .iter()
                .map(|page| page.read(cx).id().to_owned()),
        );
        let before = self.nav.workspace.clone();
        self.nav.workspace.retain_pages(|id| present.contains(id));
        self.nav
            .history
            .retain(|item| item.page_id().is_none_or(|id| present.contains(id)));
        if self.nav.workspace != before {
            self.persist(cx);
        }
    }

    fn persist(&mut self, cx: &mut gpui::Context<Self>) {
        let file = self.nav.file.clone();
        let workspace = self.nav.workspace.clone();
        let previous = self.nav.saving.take();
        self.nav.saves += 1;
        let save = self.nav.saves;
        self.nav.saving = Some(cx.spawn(async move |this, cx| {
            if let Some(previous) = previous {
                previous.await;
            }
            let result = cx
                .background_spawn(async move { file.save(&workspace) })
                .await;
            this.update(cx, |this, cx| {
                if this.nav.saves == save {
                    this.nav.saving = None;
                }
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
        self.nav.view = match destination {
            Destination::Browser { view } => Some(*view),
            Destination::Page { .. } | Destination::Tag { .. } => None,
        };
        self.nav.tags.shown = match destination {
            Destination::Tag { tag } => Some(tag.clone()),
            _ => None,
        };
        match (self.nav.view, self.nav.tags.shown.clone()) {
            (Some(BrowserView::Tags), _) => self.show_tags(window, cx),
            (Some(BrowserView::Tasks), _) => self.show_tasks(window, cx),
            (Some(view), _) => self.open_browser(view, window, cx),
            (None, Some(tag)) => self.show_tag(tag, window, cx),
            (None, None) => {}
        }
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
            self.show_references(window, cx);
        }
        cx.notify();
    }

    pub(super) fn update_title(&self, window: &mut Window, cx: &App) {
        let title = self
            .page
            .read(cx)
            .path(cx)
            .unwrap_or_else(|| "Unsaved page".into());
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
            .on_action(cx.listener(|this, _: &GoBack, window, cx| this.go(true, window, cx)))
            .on_action(cx.listener(|this, _: &GoForward, window, cx| this.go(false, window, cx)))
            .on_mouse_up(
                gpui::MouseButton::Navigate(gpui::NavigationDirection::Back),
                cx.listener(|this, _, window, cx| this.go(true, window, cx)),
            )
            .on_mouse_up(
                gpui::MouseButton::Navigate(gpui::NavigationDirection::Forward),
                cx.listener(|this, _, window, cx| this.go(false, window, cx)),
            )
            .on_action(cx.listener(|this, _: &CloseTab, window, cx| {
                if let Some(active) = this.nav.workspace.active.clone() {
                    this.close_tabs(|tab| tab == &active, window, cx);
                }
            }))
            .on_action(cx.listener(|this, _: &NextTab, window, cx| this.cycle_tab(1, window, cx)))
            .on_action(
                cx.listener(|this, _: &PreviousTab, window, cx| this.cycle_tab(-1, window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &ToggleSidePane, window, cx| this.toggle_side(window, cx)),
            )
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| this.toggle_favorite(cx)))
            .on_action(cx.listener(|this, _: &ShowPages, window, cx| {
                this.visit(
                    Destination::Browser {
                        view: BrowserView::Pages,
                    },
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &ShowDaily, window, cx| {
                this.visit(
                    Destination::Browser {
                        view: BrowserView::Daily,
                    },
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &ShowTags, window, cx| {
                this.visit(
                    Destination::Browser {
                        view: BrowserView::Tags,
                    },
                    window,
                    cx,
                )
            }))
            .on_action(cx.listener(|this, _: &ShowTasks, window, cx| {
                this.visit(
                    Destination::Browser {
                        view: BrowserView::Tasks,
                    },
                    window,
                    cx,
                )
            }))
            .on_action(
                cx.listener(|this, _: &CaptureTask, window, cx| this.capture_task(window, cx)),
            )
            .on_action(cx.listener(|this, _: &FocusReferences, window, cx| {
                this.focus_references(window, cx)
            }))
            .on_action(cx.listener(|this, _: &FocusFiles, window, cx| this.focus_files(window, cx)))
            .on_action(cx.listener(|this, _: &OpenToday, window, cx| this.open_today(window, cx)))
            .on_action(
                cx.listener(|this, _: &DeletePage, window, cx| this.delete_shown_page(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &DeleteFile, window, cx| {
                    this.delete_selected_file(window, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &RenamePage, window, cx| this.rename_shown_page(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &RenameFile, window, cx| {
                    this.rename_selected_file(window, cx)
                }),
            )
    }

    /// Moves through history without recording recents, restoring a closed tab if needed.
    pub(super) fn go(&mut self, back: bool, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.nav.palette.is_some() {
            return;
        }
        let destination = if back {
            self.nav.history.back()
        } else {
            self.nav.history.forward()
        };
        if let Some(destination) = destination {
            self.nav.workspace.show(destination.clone());
            self.show(&destination, window, cx);
            self.persist(cx);
        }
    }

    pub(super) fn close_tabs(
        &mut self,
        closing: impl Fn(&Destination) -> bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(next) = self.nav.workspace.close(closing, None) {
            self.nav.history.push(next.clone());
            self.show(&next, window, cx);
        }
        self.persist(cx);
        cx.notify();
    }

    fn cycle_tab(&mut self, step: isize, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let tabs = &self.nav.workspace.tabs;
        if tabs.is_empty() {
            return;
        }
        let current = self
            .nav
            .workspace
            .active
            .as_ref()
            .and_then(|active| tabs.iter().position(|tab| tab == active))
            .unwrap_or(0) as isize;
        let next = (current + step).rem_euclid(tabs.len() as isize) as usize;
        let destination = tabs[next].clone();
        self.visit(destination, window, cx);
    }

    fn toggle_side(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.nav.side.is_some() {
            self.close_side(window, cx);
        } else if self.nav.view.is_none() && self.nav.tags.shown.is_none() {
            let id = self.page.read(cx).id().to_owned();
            self.open_beside(id, window, cx);
        }
    }

    pub(super) fn toggle_favorite(&mut self, cx: &mut gpui::Context<Self>) {
        if let Some(active) = self.nav.workspace.active.clone() {
            self.nav.workspace.toggle_favorite(active);
            self.persist(cx);
            cx.notify();
        }
    }
}

/// The second pane beside the main page. Holds a page only, as in the reference.
pub(super) struct SidePane {
    pub(super) page: Entity<OpenPage>,
    pub(super) editor: Option<Entity<NotebookWindow>>,
    focus: bool,
}

impl StoredWindow {
    pub(super) fn open_beside(
        &mut self,
        id: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.place_beside(id, true, window, cx);
    }

    /// Opens a page in the side pane; `focus` moves the caret there once it loads, which a
    /// restored pane at startup must not do.
    pub(super) fn place_beside(
        &mut self,
        id: String,
        focus: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let page = self.session.borrow_mut().open_page(id.clone(), cx);
        self.watch(&page, window, cx);
        self.nav.side = Some(SidePane {
            page,
            editor: None,
            focus,
        });
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
        let focus = side.focus;
        let previous = window.focused(cx);
        let editor = cx.new(|cx| {
            let editor = NotebookWindow::for_page(analysis, None, window, cx);
            if writes {
                editor
                    .commands
                    .update(cx, |commands, _| commands.enable_writes());
            }
            editor
        });
        self.link_editors(&editor, window, cx);
        match (focus, previous) {
            (true, _) => window.focus(&editor.read(cx).source.focus_handle(cx), cx),
            (false, Some(previous)) => window.focus(&previous, cx),
            (false, None) => window.blur(cx),
        }
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
        let content = match (self.nav.view, self.tag_content().or(self.task_content())) {
            (Some(BrowserView::Tags | BrowserView::Tasks), Some(view)) | (None, Some(view)) => view,
            (Some(view), _) => self.render_browser(view, cx).into_any_element(),
            (None, None) => match self.editor.clone() {
                Some(editor) => gpui::div()
                    .flex_1()
                    .min_h_0()
                    .child(editor)
                    .into_any_element(),
                None => gpui::div().into_any_element(),
            },
        };
        let main = gpui::div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(self.render_tabs(cx))
            .child(content)
            .when_some(self.shown_references(cx), |main, references| {
                main.child(references)
            });
        let side = self.nav.side.as_ref().and_then(|side| side.editor.clone());
        gpui::div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.render_sidebar(cx))
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

#[cfg(test)]
#[path = "workspace_tests.rs"]
mod workspace_tests;

#[cfg(test)]
#[path = "daily_tests.rs"]
mod daily_tests;

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tags_tests;

#[cfg(test)]
#[path = "references_tests.rs"]
mod references_tests;

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tasks_tests;

#[cfg(test)]
#[path = "links_tests.rs"]
mod links_tests;

#[cfg(test)]
#[path = "completion_tests.rs"]
mod completion_tests;

#[cfg(test)]
#[path = "rendered_links_tests.rs"]
mod rendered_links_tests;

#[cfg(test)]
#[path = "embeds_tests.rs"]
mod embeds_tests;

#[cfg(test)]
#[path = "file_tests.rs"]
mod file_tests;

#[cfg(test)]
#[path = "task_lists_tests.rs"]
mod task_lists_tests;

#[cfg(test)]
#[path = "folder_tests.rs"]
mod folder_tests;

#[cfg(test)]
#[path = "media_tests.rs"]
mod media_tests;

#[cfg(test)]
#[path = "media_pdf_tests.rs"]
mod media_pdf_tests;

#[cfg(test)]
#[path = "media_attach_tests.rs"]
mod media_attach_tests;

#[cfg(test)]
#[path = "media_folder_tests.rs"]
mod media_folder_tests;
