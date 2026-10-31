//! Pointer menus for Files, Favorites, Recent, and tabs. A Files menu highlights its row and
//! dispatches the same actions as the tree's keys; tab and sidebar menus act on the clicked
//! destination through the methods those actions call, as Zed's tab menu does.

use super::*;
use crate::{
    file_actions::Target,
    navigation::{
        CloseTab, CopyLink, DeleteFile, DeletePage, MoveFile, MovePage, NewFolderHere, NewPageHere,
        RenameFile, RevealInFileManager, ToggleFavorite,
        file_dialog::Moving,
        pointer_menu::{self, Entry, Item},
    },
};
use gpui::{Pixels, Point, Subscription, WeakEntity, anchored, deferred, point, px};
use quire_core::workspace::{Destination, TreeKind};
use ui::ContextMenu;

pub(in crate::editor_host::stored) const REVEAL: &str = "Reveal in Finder / Explorer";
pub(in crate::editor_host::stored) const REVEAL_UNAVAILABLE: &str = "This notebook keeps its pages in its database, so Reveal in Finder / Explorer shows a copy of a page but has no folders to show. Export the notebook as Markdown to see its folders.";

pub(in crate::editor_host::stored) struct DeployedMenu {
    pub(in crate::editor_host::stored) menu: Entity<ContextMenu>,
    /// The menu's name, as the reference labels it, such as "Tab actions for Old".
    pub(in crate::editor_host::stored) name: SharedString,
    /// The entries as text, from [`pointer_menu::summary`].
    #[cfg(test)]
    pub(in crate::editor_host::stored) summary: Vec<String>,
    position: Point<Pixels>,
    _dismissed: Subscription,
}

/// Where a page's sidebar entry sits, which decides the menu's extra items.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor_host::stored) enum Sidebar {
    Favorite,
    Recent,
}

/// Stands in for Finder or Explorer in tests, as the folder tests' bin stands in for the Trash.
#[cfg(test)]
pub(in crate::editor_host::stored) struct RevealFiles(
    pub(in crate::editor_host::stored) Rc<dyn Fn(&std::path::Path)>,
);
#[cfg(test)]
impl gpui::Global for RevealFiles {}

/// Where tests have Reveal write Quire notebook pages, instead of the system's temporary folder.
#[cfg(test)]
pub(in crate::editor_host::stored) struct RevealCopies(pub(in crate::editor_host::stored) PathBuf);
#[cfg(test)]
impl gpui::Global for RevealCopies {}

#[derive(Clone, Copy)]
enum PageAction {
    Beside,
    Link,
    Reveal,
    Move,
    Delete,
}

type Act = fn(&mut StoredWindow, &str, &mut Window, &mut gpui::Context<StoredWindow>);

/// An item that runs `act` on `page` in the window.
fn on_page(view: &WeakEntity<StoredWindow>, label: &'static str, page: &str, act: Act) -> Item {
    let view = view.clone();
    let page = page.to_owned();
    Item::run(label, move |window, cx| {
        view.update(cx, |this, cx| act(this, &page, window, cx))
            .ok();
    })
}

fn favorite_label(favorite: bool) -> &'static str {
    if favorite {
        "Remove from favorites"
    } else {
        "Add to favorites"
    }
}

impl StoredWindow {
    /// The root of a folder notebook, where Reveal finds files.
    fn files_root(&self) -> Option<std::path::PathBuf> {
        self.session
            .borrow()
            .folder()
            .map(|(root, _)| root.to_path_buf())
    }

    fn deploy_menu(
        &mut self,
        name: String,
        entries: Vec<Entry>,
        context: Option<gpui::FocusHandle>,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let entries = pointer_menu::tidy(entries);
        #[cfg(test)]
        let summary = pointer_menu::summary(&entries);
        let menu = pointer_menu::build(entries, context, window, cx);
        let opener = window.focused(cx);
        window.focus(&menu.focus_handle(cx), cx);
        let dismissed = cx.subscribe_in(
            &menu,
            window,
            move |this, menu, _: &DismissEvent, window, cx| {
                let stranded = window.focused(cx).is_none()
                    || menu.focus_handle(cx).contains_focused(window, cx);
                if this
                    .nav
                    .menu
                    .as_ref()
                    .is_some_and(|deployed| &deployed.menu == menu)
                {
                    this.nav.menu = None;
                }
                if stranded && let Some(opener) = &opener {
                    window.focus(opener, cx);
                }
                cx.notify();
            },
        );
        self.nav.menu = Some(DeployedMenu {
            menu,
            name: name.into(),
            #[cfg(test)]
            summary,
            position,
            _dismissed: dismissed,
        });
        cx.notify();
    }

    pub(in crate::editor_host::stored) fn render_menu(&self) -> Option<impl IntoElement> {
        self.nav.menu.as_ref().map(|deployed| {
            deferred(
                anchored().position(deployed.position).child(
                    gpui::div()
                        .id("pointer-menu")
                        .role(gpui::Role::Group)
                        .aria_label(deployed.name.clone())
                        .child(deployed.menu.clone()),
                ),
            )
            .with_priority(3)
        })
    }

    /// The window position just under Files row `index`, from the list's last layout.
    pub(in crate::editor_host::stored) fn file_row_position(
        &self,
        index: usize,
        window: &Window,
    ) -> Point<Pixels> {
        let count = self.file_rows().len().max(1);
        let state = self.nav.browsers.files_scroll.0.borrow();
        state.last_item_size.map_or_else(
            || window.mouse_position(),
            |size| {
                let height = size.contents.height / count as f32;
                let bounds = state.base_handle.bounds();
                let top = bounds.top() + state.base_handle.offset().y;
                point(bounds.left() + px(24.), top + height * (index + 1) as f32)
            },
        )
    }

    /// Highlights Files row `index` and opens its menu at `position`, or under the row.
    pub(in crate::editor_host::stored) fn deploy_file_menu(
        &mut self,
        index: usize,
        position: Option<Point<Pixels>>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(row) = self.file_rows().into_iter().nth(index) else {
            return;
        };
        self.nav.browsers.files_selected = index;
        self.focus_files(window, cx);
        let position = position.unwrap_or_else(|| self.file_row_position(index, window));
        let focus = self.nav.browsers.files_focus.clone();
        let reveal = (self.files_root().is_some() || row.kind == TreeKind::File)
            .then(|| Item::action(REVEAL, RevealInFileManager).done());
        let (name, entries) = match row.kind {
            TreeKind::Folder => (
                format!("Folder actions for {}", row.path),
                [
                    vec![
                        Item::action("New note here", NewPageHere).done(),
                        Item::action("New folder here", NewFolderHere).done(),
                        Entry::Separator,
                        Item::action("Rename or move…", RenameFile).done(),
                    ],
                    reveal.into_iter().collect(),
                    vec![
                        Entry::Separator,
                        Item::action("Delete folder…", DeleteFile).done(),
                    ],
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>(),
            ),
            TreeKind::File => {
                let Some(page) = self.page_at_path(&row.path) else {
                    return;
                };
                let title = self.destination_label(&Destination::page(page.clone()), cx);
                let favorite = self.nav.workspace.is_favorite(&Destination::page(page));
                (
                    format!("Page actions for {title}"),
                    [
                        vec![
                            Item::action("Open", menu::Confirm).done(),
                            Item::action("Open beside", menu::SecondaryConfirm).done(),
                            Item::action("Copy link", CopyLink).done(),
                            Item::action(favorite_label(favorite), ToggleFavorite).done(),
                            Entry::Separator,
                            Item::action("New note here", NewPageHere).done(),
                            Item::action("New folder here", NewFolderHere).done(),
                            Entry::Separator,
                            Item::action("Rename", RenameFile).done(),
                            Item::action("Move to…", MoveFile).done(),
                        ],
                        reveal.into_iter().collect(),
                        vec![
                            Entry::Separator,
                            Item::action("Delete page…", DeleteFile).done(),
                        ],
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>(),
                )
            }
        };
        self.deploy_menu(name, entries, focus, position, window, cx);
    }

    /// A page action of sidebar entries and page tabs, which order them differently.
    fn page_item(&self, action: PageAction, page: &str, view: &WeakEntity<Self>) -> Vec<Entry> {
        let item = match action {
            PageAction::Beside => on_page(view, "Open beside", page, |this, page, window, cx| {
                this.open_beside(page.to_owned(), window, cx)
            }),
            PageAction::Link => on_page(view, "Copy link", page, |this, page, _, cx| {
                this.copy_link(page, cx)
            }),
            PageAction::Reveal => on_page(view, REVEAL, page, |this, page, _, cx| {
                this.reveal_page(page, cx)
            }),
            PageAction::Move => {
                on_page(view, "Rename or move…", page, |this, page, window, cx| {
                    this.ask_move(Moving::Page(page.to_owned()), window, cx)
                })
                .keys(MovePage)
            }
            PageAction::Delete => {
                on_page(view, "Delete page…", page, |this, page, window, cx| {
                    this.ask_delete(Target::Page(page.to_owned()), window, cx)
                })
                .keys(DeletePage)
            }
        };
        vec![item.done()]
    }

    fn favorite_entry(&self, destination: &Destination, view: &WeakEntity<Self>) -> Entry {
        let favorite = self.nav.workspace.is_favorite(destination);
        let view = view.clone();
        let destination = destination.clone();
        Item::run(favorite_label(favorite), move |_, cx| {
            view.update(cx, |this, cx| {
                this.toggle_favorite_of(destination.clone(), cx)
            })
            .ok();
        })
        .keys(ToggleFavorite)
        .done()
    }

    /// The menu of a page's Favorites or Recent entry, acting on that page.
    pub(in crate::editor_host::stored) fn deploy_sidebar_menu(
        &mut self,
        page: String,
        sidebar: Sidebar,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let view = cx.entity().downgrade();
        let destination = Destination::page(page.clone());
        let title = self.destination_label(&destination, cx);
        let item = |action| self.page_item(action, &page, &view);
        let recent = (sidebar == Sidebar::Recent).then(|| {
            on_page(&view, "Remove from recent", &page, |this, page, _, cx| {
                this.nav.workspace.remove_recent(&Destination::page(page));
                this.persist(cx);
                cx.notify();
            })
            .done()
        });
        let entries = [
            vec![
                on_page(&view, "Open", &page, |this, page, window, cx| {
                    this.visit(Destination::page(page), window, cx)
                })
                .done(),
            ],
            item(PageAction::Beside),
            item(PageAction::Link),
            vec![self.favorite_entry(&destination, &view), Entry::Separator],
            item(PageAction::Move),
            item(PageAction::Reveal),
            recent.into_iter().collect(),
            vec![Entry::Separator],
            item(PageAction::Delete),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        self.deploy_menu(
            format!("Page actions for {title}"),
            entries,
            None,
            position,
            window,
            cx,
        );
    }

    /// The menu of tab `index`: closing tabs around it, then the reference page actions.
    pub(in crate::editor_host::stored) fn deploy_tab_menu(
        &mut self,
        index: usize,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let tabs = self.nav.workspace.tabs.clone();
        let Some(tab) = tabs.get(index).cloned() else {
            return;
        };
        let view = cx.entity().downgrade();
        let title = self.destination_label(&tab, cx);
        let count = tabs.len();
        let close = |label: &'static str, keep: fn(usize, usize) -> bool, prefer: bool| {
            let view = view.clone();
            let tabs = tabs.clone();
            let tab = tab.clone();
            Item::run(label, move |window, cx| {
                let closing: Vec<Destination> = tabs
                    .iter()
                    .enumerate()
                    .filter(|(at, _)| !keep(*at, index))
                    .map(|(_, tab)| tab.clone())
                    .collect();
                let preferred = prefer.then_some(&tab);
                view.update(cx, |this, cx| {
                    this.close_tabs_preferring(|tab| closing.contains(tab), preferred, window, cx)
                })
                .ok();
            })
        };
        let entries = [
            vec![
                close("Close", |at, index| at != index, false)
                    .keys(CloseTab)
                    .done(),
                close("Close others", |at, index| at == index, true)
                    .disabled(count < 2)
                    .done(),
                close("Close tabs to the left", |at, index| at >= index, true)
                    .disabled(index == 0)
                    .done(),
                close("Close tabs to the right", |at, index| at <= index, true)
                    .disabled(index + 1 >= count)
                    .done(),
                close("Close all", |_, _| false, false).done(),
                Entry::Separator,
                self.favorite_entry(&tab, &view),
            ],
            tab.page_id()
                .map(|page| {
                    let item = |action| self.page_item(action, page, &view);
                    [
                        item(PageAction::Beside),
                        item(PageAction::Link),
                        item(PageAction::Reveal),
                        vec![Entry::Separator],
                        item(PageAction::Move),
                        item(PageAction::Delete),
                    ]
                    .into_iter()
                    .flatten()
                    .collect::<Vec<_>>()
                })
                .unwrap_or_default(),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
        self.deploy_menu(
            format!("Tab actions for {title}"),
            entries,
            None,
            position,
            window,
            cx,
        );
    }

    /// Shift-F10 outside Files and the page: the active tab's menu.
    pub(in crate::editor_host::stored) fn deploy_active_tab_menu(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let active = self.nav.workspace.active.clone();
        if let Some(index) = self
            .nav
            .workspace
            .tabs
            .iter()
            .position(|tab| Some(tab) == active.as_ref())
        {
            let position = window.mouse_position();
            self.deploy_tab_menu(index, position, window, cx);
        }
    }

    pub(in crate::editor_host::stored) fn toggle_favorite_of(
        &mut self,
        destination: Destination,
        cx: &mut gpui::Context<Self>,
    ) {
        self.nav.workspace.toggle_favorite(destination);
        self.persist(cx);
        cx.notify();
    }

    /// Copies the reference `[[title]]` link to a page.
    pub(in crate::editor_host::stored) fn copy_link(&self, page: &str, cx: &mut App) {
        let title = self.destination_label(&Destination::page(page), cx);
        cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!("[[{title}]]")));
    }

    fn show_in_file_manager(path: &std::path::Path, cx: &mut gpui::Context<Self>) {
        #[cfg(test)]
        if let Some(reveal) = cx
            .try_global::<RevealFiles>()
            .map(|reveal| reveal.0.clone())
        {
            return reveal(path);
        }
        cx.reveal_path(path)
    }

    fn reveal_path(&mut self, path: &str, cx: &mut gpui::Context<Self>) {
        match self.files_root() {
            Some(root) => Self::show_in_file_manager(&root.join(path), cx),
            None => {
                self.nav.error = Some(REVEAL_UNAVAILABLE.into());
                cx.notify();
            }
        }
    }

    pub(in crate::editor_host::stored) fn reveal_page(
        &mut self,
        page: &str,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.files_root().is_none() {
            return self.reveal_copy(page.to_owned(), cx);
        }
        if let Some(path) = self.page_path(page, cx) {
            self.reveal_path(&path, cx);
        }
    }

    /// Shows a Quire notebook page as the reference app does: its current text, unsaved typing
    /// included, written as a read-only Markdown copy in the temporary `Quire pages` folder.
    fn reveal_copy(&mut self, page: String, cx: &mut gpui::Context<Self>) {
        let storage = self.session.borrow().storage();
        let open = self.session.borrow().open_pages();
        #[cfg(test)]
        let folder = cx
            .try_global::<RevealCopies>()
            .map(|copies| copies.0.clone())
            .unwrap_or_else(crate::file_actions::reveal_folder);
        #[cfg(not(test))]
        let folder = crate::file_actions::reveal_folder();
        cx.spawn(async move |this, cx| {
            let result = match crate::page_writes::read(page, &open, &storage, cx).await {
                Ok(Some(source)) => {
                    cx.background_spawn(async move {
                        crate::file_actions::export_for_reveal(
                            &folder,
                            &source.path,
                            &source.stored,
                        )
                    })
                    .await
                }
                Ok(None) => Err("This page is not in the notebook. Choose another page.".into()),
                Err(error) => Err(error),
            };
            this.update(cx, |this, cx| match result {
                Ok(path) => Self::show_in_file_manager(&path, cx),
                Err(error) => {
                    this.nav.error = Some(error.into());
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// The highlighted Files page, which Files-scoped page actions act on.
    pub(in crate::editor_host::stored) fn selected_file_page(&self) -> Option<String> {
        match self.selected_file()? {
            Target::Page(page) => Some(page),
            Target::Folder(_) => None,
        }
    }

    fn selected_row(&self) -> Option<quire_core::workspace::TreeRow> {
        self.file_rows()
            .into_iter()
            .nth(self.nav.browsers.files_selected)
    }

    pub(in crate::editor_host::stored) fn reveal_selected_file(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(row) = self.selected_row() else {
            return;
        };
        match (self.files_root(), row.kind) {
            (None, TreeKind::File) => {
                if let Some(page) = self.page_at_path(&row.path) {
                    self.reveal_copy(page, cx);
                }
            }
            _ => self.reveal_path(&row.path, cx),
        }
    }

    /// The highlighted folder, or the highlighted page's folder.
    fn selected_folder(&self) -> Option<String> {
        let row = self.selected_row()?;
        Some(match row.kind {
            TreeKind::Folder => row.path,
            TreeKind::File => row
                .path
                .rsplit_once('/')
                .map_or_else(String::new, |(parent, _)| parent.to_owned()),
        })
    }

    /// New folder here: asks for a path inside the highlighted folder.
    pub(in crate::editor_host::stored) fn new_folder_in_selected_folder(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(parent) = self.selected_folder() else {
            return;
        };
        self.ask_new_folder(parent, window, cx);
    }

    /// New note here: in the highlighted folder, or in the highlighted page's folder.
    pub(in crate::editor_host::stored) fn new_page_in_selected_folder(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(folder) = self.selected_folder() else {
            return;
        };
        self.open_palette_in(
            crate::navigation::palette::PaletteMode::Create,
            folder,
            window,
            cx,
        );
    }
}
