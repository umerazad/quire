//! The window's navigation surfaces: sidebar (Favorites, Recent, Files), tab strip, and the
//! Pages and Daily notes browsers. Every list is a virtualized `uniform_list`.

use super::super::page_views::SaveState;
use super::*;
use editor::{Editor, EditorEvent};
use gpui::{ClickEvent, FocusHandle, ScrollStrategy, UniformListScrollHandle, uniform_list};
use quire_core::workspace::{
    BrowserView, Destination, TreeKind, TreeRow, folder_rows, in_namespace, namespaces,
};
use std::collections::BTreeSet;

#[cfg(test)]
#[path = "sidebar_tests.rs"]
mod sidebar_tests;
#[path = "sidebar_views.rs"]
mod sidebar_views;
use ui::{
    Color, ContextMenu, Disableable, DropdownMenu, IconButton, IconName, LabelSize, ListItem,
    ListItemSpacing, Tab, TabBar, Toggleable, h_flex, v_flex,
};

#[derive(Default)]
pub(in crate::editor_host::stored) struct Browsers {
    pub(in crate::editor_host::stored) filter: Option<Entity<Editor>>,
    pub(in crate::editor_host::stored) selected: usize,
    pub(in crate::editor_host::stored) list_focus: Option<FocusHandle>,
    scroll: UniformListScrollHandle,
    pub(in crate::editor_host::stored) files_selected: usize,
    pub(in crate::editor_host::stored) files_focus: Option<FocusHandle>,
    pub(in crate::editor_host::stored) files_scroll: UniformListScrollHandle,
    /// Empty folders and attachments of a folder notebook.
    pub(in crate::editor_host::stored) on_disk: super::file_views::OnDisk,
    file_rows: FileRows,
    /// Recent is unfolded, which the reference does not remember either.
    pub(in crate::editor_host::stored) recent_open: bool,
    pub(in crate::editor_host::stored) profile_menu: ui::PopoverMenuHandle<ContextMenu>,
}

/// The Files tree for the paths, folders, and expanded folders it was built from. Rendering asks
/// for the rows twice a frame, and rebuilding them sorts every path.
#[derive(Default)]
struct FileRows(std::cell::RefCell<Option<BuiltRows>>);

struct BuiltRows {
    paths: Vec<String>,
    folders: Vec<String>,
    expanded: Vec<String>,
    rows: Rc<[TreeRow]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::editor_host::stored) struct BrowserRow {
    pub(in crate::editor_host::stored) page: String,
    pub(in crate::editor_host::stored) title: String,
    pub(in crate::editor_host::stored) detail: String,
}

/// The answer pane's title, which its tab takes, as rt names it.
const ASK_THE_NOTEBOOK: &str = "Ask the notebook";

fn shift(event: &ClickEvent) -> bool {
    event.modifiers().shift
}

/// The icon before a tab's label, by what the tab shows.
fn tab_icon(destination: &Destination) -> IconName {
    match destination {
        Destination::Page { .. } => IconName::FileTextOutlined,
        Destination::Tag { .. }
        | Destination::Browser {
            view: BrowserView::Tags,
        } => IconName::Hash,
        Destination::Browser {
            view: BrowserView::Tasks,
        } => IconName::ListTodo,
        Destination::Browser {
            view: BrowserView::Daily,
        } => IconName::Book,
        Destination::Browser {
            view: BrowserView::Pages,
        } => IconName::FileMultiple,
        Destination::Browser {
            view: BrowserView::Ai,
        } => IconName::Sparkle,
    }
}

/// "October 2026" for `2026-10`, or the text itself when it is not a month.
fn month_name(month: &str) -> String {
    const NAMES: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    month
        .split_once('-')
        .and_then(|(year, number)| {
            let name = NAMES.get(number.parse::<usize>().ok()?.checked_sub(1)?)?;
            Some(format!("{name} {year}"))
        })
        .unwrap_or_else(|| month.to_owned())
}

impl StoredWindow {
    pub(in crate::editor_host::stored) fn destination_label(
        &self,
        destination: &Destination,
        cx: &App,
    ) -> SharedString {
        match destination {
            Destination::Browser {
                view: BrowserView::Pages,
            } => "Pages".into(),
            Destination::Browser {
                view: BrowserView::Daily,
            } => "Daily notes".into(),
            Destination::Browser {
                view: BrowserView::Tags,
            } => "Tags".into(),
            Destination::Browser {
                view: BrowserView::Tasks,
            } => "Tasks".into(),
            Destination::Browser {
                view: BrowserView::Ai,
            } => ASK_THE_NOTEBOOK.into(),
            Destination::Tag { tag } => tag.clone().into(),
            Destination::Page { page } => self
                .nav
                .title(page)
                .map(str::to_owned)
                .or_else(|| {
                    let open = self.session.borrow().open_pages();
                    open.iter()
                        .find(|open| open.read(cx).id() == page)
                        .and_then(|open| open.read(cx).loaded()?.path.clone())
                        .map(|path| quire_core::markdown::page_name(&path).to_owned())
                })
                .unwrap_or_else(|| "Untitled page".into())
                .into(),
        }
    }

    pub(in crate::editor_host::stored) fn open_browser(
        &mut self,
        _view: BrowserView,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let query = self.nav.workspace.pages.query.clone();
        let filter = self
            .nav
            .browsers
            .filter
            .get_or_insert_with(|| {
                let editor = cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text("Filter…", window, cx);
                    editor.set_text(query.as_str(), window, cx);
                    editor
                });
                cx.subscribe(&editor, |this, editor, event: &EditorEvent, cx| {
                    if matches!(event, EditorEvent::BufferEdited) {
                        let query = editor.read(cx).text(cx);
                        this.remember(|workspace| workspace.pages.query = query, cx);
                        this.nav.browsers.selected = 0;
                        cx.notify();
                    }
                })
                .detach();
                editor
            })
            .clone();
        self.nav.browsers.selected = 0;
        self.nav
            .browsers
            .list_focus
            .get_or_insert_with(|| cx.focus_handle());
        window.focus(&filter.focus_handle(cx), cx);
    }

    fn filter_text(&self, cx: &App) -> String {
        self.nav
            .browsers
            .filter
            .as_ref()
            .map(|filter| filter.read(cx).text(cx))
            .unwrap_or_default()
            .to_lowercase()
    }

    /// The rows of the active browser: Pages by title within the chosen namespace, or daily
    /// pages newest first, each filtered by the typed text.
    pub(in crate::editor_host::stored) fn browser_rows(&self, cx: &App) -> Vec<BrowserRow> {
        let query = self.filter_text(cx);
        match self.nav.view {
            Some(BrowserView::Pages) => self
                .nav
                .pages
                .iter()
                .filter(|listed| {
                    self.nav
                        .workspace
                        .pages
                        .namespace
                        .as_deref()
                        .is_none_or(|namespace| in_namespace(&listed.page.title, namespace))
                })
                .filter(|listed| {
                    format!("{} {}", listed.page.title, listed.page.path)
                        .to_lowercase()
                        .contains(&query)
                })
                .map(|listed| BrowserRow {
                    page: listed.page.page.to_lossy_string(),
                    title: listed.page.title.clone(),
                    detail: listed.page.path.clone(),
                })
                .collect(),
            Some(BrowserView::Daily) => {
                let mut dated: Vec<_> = self
                    .nav
                    .pages
                    .iter()
                    .filter_map(|listed| Some((listed.date.clone()?, listed)))
                    .collect();
                dated.sort_by(|a, b| {
                    b.0.cmp(&a.0)
                        .then_with(|| a.1.page.path.cmp(&b.1.page.path))
                });
                let primary = |date: &str| self.primary_daily(date);
                let month = &self.nav.workspace.chrome.daily_month;
                dated
                    .into_iter()
                    .filter(|(date, _)| date.starts_with(month.as_str()))
                    .filter(|(date, listed)| {
                        format!("{date}{}", listed.page.title)
                            .to_lowercase()
                            .contains(&query)
                    })
                    .map(|(date, listed)| {
                        let page = listed.page.page.to_lossy_string();
                        let detail = if primary(&date).as_deref() == Some(page.as_str()) {
                            date.clone()
                        } else {
                            format!("{date} · also {}", listed.page.title)
                        };
                        BrowserRow {
                            page,
                            title: listed.page.title.clone(),
                            detail,
                        }
                    })
                    .collect()
            }
            Some(BrowserView::Tags | BrowserView::Tasks | BrowserView::Ai) | None => Vec::new(),
        }
    }

    /// The reference `dailyPage` choice among pages for one date: the one in the configured
    /// daily folder, otherwise the first by path.
    pub(in crate::editor_host::stored) fn primary_daily(&self, date: &str) -> Option<String> {
        let folder = self.daily_folder(date);
        let mut pages: Vec<_> = self
            .nav
            .pages
            .iter()
            .filter(|listed| listed.date.as_deref() == Some(date))
            .collect();
        pages.sort_by(|a, b| a.page.path.cmp(&b.page.path));
        pages
            .iter()
            .find(|listed| {
                folder.as_deref().is_some_and(|folder| {
                    listed
                        .page
                        .path
                        .rsplit_once('/')
                        .map_or("", |(parent, _)| parent)
                        == folder
                })
            })
            .or(pages.first())
            .map(|listed| listed.page.page.to_lossy_string())
    }

    fn select_row(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let count = self.browser_rows(cx).len();
        if count == 0 {
            return;
        }
        let next = (self.nav.browsers.selected as isize + delta).clamp(0, count as isize - 1);
        self.nav.browsers.selected = next as usize;
        self.nav
            .browsers
            .scroll
            .scroll_to_item(next as usize, ScrollStrategy::Top);
        cx.notify();
    }

    fn confirm_row(&mut self, beside: bool, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(row) = self.browser_rows(cx).get(self.nav.browsers.selected) {
            let page = row.page.clone();
            self.open_page(page, beside, window, cx);
        }
    }

    pub(in crate::editor_host::stored) fn file_rows(&self) -> Rc<[TreeRow]> {
        let disk = &self.nav.browsers.on_disk;
        let expanded = &self.nav.workspace.expanded;
        let listed_paths = || {
            self.nav
                .pages
                .iter()
                .map(|listed| &listed.page.path)
                .chain(&disk.files)
        };
        let listed_folders = || self.nav.workspace.folders.iter().chain(&disk.folders);
        let mut cached = self.nav.browsers.file_rows.0.borrow_mut();
        if let Some(built) = cached.as_ref()
            && built.expanded == *expanded
            && built.paths.iter().eq(listed_paths())
            && built.folders.iter().eq(listed_folders())
        {
            return built.rows.clone();
        }
        let paths: Vec<String> = listed_paths().cloned().collect();
        let folders: Vec<String> = listed_folders().cloned().collect();
        let open: BTreeSet<String> = expanded.iter().cloned().collect();
        let rows: Rc<[TreeRow]> = folder_rows(&paths, &folders, &open, |a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        })
        .into();
        *cached = Some(BuiltRows {
            paths,
            folders,
            expanded: expanded.clone(),
            rows: rows.clone(),
        });
        rows
    }

    pub(in crate::editor_host::stored) fn page_at_path(&self, path: &str) -> Option<String> {
        self.nav
            .pages
            .iter()
            .find(|listed| listed.page.path == path)
            .map(|listed| listed.page.page.to_lossy_string())
    }

    pub(in crate::editor_host::stored) fn focus_files(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let focus = self
            .nav
            .browsers
            .files_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        window.focus(&focus, cx);
        cx.notify();
    }

    fn activate_file(
        &mut self,
        index: usize,
        beside: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(row) = self.file_rows().get(index).cloned() else {
            return;
        };
        self.nav.browsers.files_selected = index;
        match row.kind {
            TreeKind::Folder => {
                self.nav.workspace.toggle_expanded(&row.path);
                self.persist(cx);
            }
            TreeKind::File => match self.page_at_path(&row.path) {
                Some(page) => self.open_page(page, beside, window, cx),
                None => self.open_attachment(&row.path, cx),
            },
        }
        cx.notify();
    }

    /// Opens a folder notebook file that is not a note with the system's app for it.
    fn open_attachment(&mut self, path: &str, cx: &mut gpui::Context<Self>) {
        let root = self
            .session
            .borrow()
            .folder()
            .map(|(root, _)| root.to_owned());
        if let Some(root) = root
            && self
                .nav
                .browsers
                .on_disk
                .files
                .iter()
                .any(|file| file == path)
        {
            cx.open_with_system(&root.join(path));
        }
    }

    fn move_file(&mut self, delta: isize, cx: &mut gpui::Context<Self>) {
        let count = self.file_rows().len();
        if count == 0 {
            return;
        }
        let next = (self.nav.browsers.files_selected as isize + delta).clamp(0, count as isize - 1)
            as usize;
        self.nav.browsers.files_selected = next;
        self.nav
            .browsers
            .files_scroll
            .scroll_to_item(next, ScrollStrategy::Top);
        cx.notify();
    }

    /// Right expands a folder; Left collapses it or moves to the parent folder.
    fn file_child(&mut self, open: bool, cx: &mut gpui::Context<Self>) {
        let rows = self.file_rows();
        let Some(row) = rows.get(self.nav.browsers.files_selected) else {
            return;
        };
        let expanded = self.nav.workspace.expanded.contains(&row.path);
        if row.kind == TreeKind::Folder && open != expanded {
            self.nav.workspace.toggle_expanded(&row.path.clone());
            self.persist(cx);
        } else if !open
            && let Some((parent, _)) = row.path.rsplit_once('/')
            && let Some(index) = rows.iter().position(|row| row.path == parent)
        {
            self.nav.browsers.files_selected = index;
        }
        cx.notify();
    }

    /// How far the page a tab shows is saved, when it is open in this window.
    pub(in crate::editor_host::stored) fn tab_save_state(
        &self,
        tab: &Destination,
        cx: &App,
    ) -> Option<SaveState> {
        let id = tab.page_id()?;
        let open = self.session.try_borrow().ok()?.open_pages();
        let page = open.iter().find(|page| page.read(cx).id() == id)?;
        SaveState::of(page.read(cx), cx)
    }

    pub(in crate::editor_host::stored) fn toggle_files(&mut self, cx: &mut gpui::Context<Self>) {
        self.remember(
            |workspace| workspace.chrome.files_collapsed = !workspace.chrome.files_collapsed,
            cx,
        );
        cx.notify();
    }

    /// Reads the page list again, and for a folder notebook rereads its folder first, for
    /// changes the file watcher has not reported yet.
    pub(in crate::editor_host::stored) fn refresh_files(&mut self, cx: &mut gpui::Context<Self>) {
        if self.session.borrow().folder().is_some() {
            self.folder_changed(None, cx);
        }
        self.refresh_catalogue(cx);
    }

    /// Shows only `month` (`YYYY-MM`) in Daily notes, or every month when it is empty.
    pub(in crate::editor_host::stored) fn set_daily_month(
        &mut self,
        month: String,
        cx: &mut gpui::Context<Self>,
    ) {
        self.remember(|workspace| workspace.chrome.daily_month = month, cx);
        self.nav.browsers.selected = 0;
        cx.notify();
    }

    /// The months with daily pages, newest first, with how many pages each has.
    fn daily_months(&self) -> Vec<(String, usize)> {
        let mut months = std::collections::BTreeMap::<String, usize>::new();
        for date in self
            .nav
            .pages
            .iter()
            .filter_map(|listed| listed.date.as_ref())
        {
            if let Some(month) = date.get(..7) {
                *months.entry(month.to_owned()).or_default() += 1;
            }
        }
        months.into_iter().rev().collect()
    }

    fn daily_month_menu(&self, window: &mut Window, cx: &mut gpui::Context<Self>) -> DropdownMenu {
        let chosen = self.nav.workspace.chrome.daily_month.clone();
        let months = self.daily_months();
        let label = if chosen.is_empty() {
            "All months".to_owned()
        } else {
            month_name(&chosen)
        };
        let view = cx.entity().downgrade();
        let menu = ContextMenu::build(window, cx, move |menu, _, _| {
            std::iter::once((String::new(), "All months".to_owned()))
                .chain(months.into_iter().map(|(month, count)| {
                    let name = format!("{} ({count})", month_name(&month));
                    (month, name)
                }))
                .fold(menu, |menu, (month, name)| {
                    let view = view.clone();
                    let toggled = month == chosen;
                    menu.toggleable_entry(name, toggled, ui::IconPosition::Start, None, {
                        move |_, cx| {
                            let month = month.clone();
                            view.update(cx, |this, cx| this.set_daily_month(month, cx))
                                .ok();
                        }
                    })
                })
        });
        DropdownMenu::new("daily-month", label, menu)
            .trigger_tooltip(ui::Tooltip::text("Show the daily notes of one month"))
    }

    fn render_files(
        &mut self,
        [heading, new_page, new_folder, refresh, tab_index]: [isize; 5],
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = !self.nav.workspace.chrome.files_collapsed;
        let focus = self
            .nav
            .browsers
            .files_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone()
            .tab_index(tab_index)
            .tab_stop(open);
        let count = self.file_rows().len();
        let list = uniform_list(
            "files",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, window, cx| {
                let rows = this.file_rows();
                // As in rt, the shown page's row is highlighted; while the tree has focus the
                // keyboard's row is.
                let focused = this
                    .nav
                    .browsers
                    .files_focus
                    .as_ref()
                    .is_some_and(|focus| focus.contains_focused(window, cx));
                let shown = this.page.read(cx).path(cx);
                let attachments: BTreeSet<&str> = this
                    .nav
                    .browsers
                    .on_disk
                    .files
                    .iter()
                    .map(String::as_str)
                    .collect();
                range
                    .filter_map(|index| {
                        let row = rows.get(index)?.clone();
                        let expanded = this.nav.workspace.expanded.contains(&row.path);
                        let icon = match (row.kind, expanded) {
                            (TreeKind::Folder, true) => IconName::FolderOpen,
                            (TreeKind::Folder, false) => IconName::Folder,
                            (TreeKind::File, _) if attachments.contains(row.path.as_str()) => {
                                IconName::Paperclip
                            }
                            (TreeKind::File, _) => IconName::File,
                        };
                        Some(
                            ListItem::new(("file", index))
                                .rounded()
                                .indent_level(row.depth)
                                .toggle_state(if focused {
                                    index == this.nav.browsers.files_selected
                                } else {
                                    shown.as_deref() == Some(row.path.as_str())
                                })
                                .start_slot(
                                    h_flex()
                                        .gap_1p5()
                                        .when(row.kind == TreeKind::Folder, |slot| {
                                            slot.child(
                                                ui::Icon::new(if expanded {
                                                    IconName::ChevronDown
                                                } else {
                                                    IconName::ChevronRight
                                                })
                                                .size(ui::IconSize::XSmall)
                                                .color(Color::Muted),
                                            )
                                        })
                                        .child(ui::Icon::new(icon).color(Color::Muted)),
                                )
                                .child(
                                    h_flex()
                                        .h(gpui::px(36.))
                                        .min_w_0()
                                        .child(Label::new(row.name.clone()).truncate()),
                                )
                                .on_click(cx.listener(move |this, event, window, cx| {
                                    this.activate_file(index, shift(event), window, cx)
                                }))
                                .on_secondary_mouse_down(cx.listener(
                                    move |this, event: &gpui::MouseDownEvent, window, cx| {
                                        cx.stop_propagation();
                                        this.deploy_file_menu(
                                            index,
                                            Some(event.position),
                                            window,
                                            cx,
                                        )
                                    },
                                )),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.nav.browsers.files_scroll)
        .flex_1();
        // The heading's buttons stay outside the tree's QuireList context, where Enter would
        // open the highlighted file instead of pressing the focused button.
        let tree = v_flex()
            .id("files-tree")
            .key_context("QuireList QuireFiles")
            .track_focus(&focus)
            .flex_1()
            .min_h_0()
            .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.move_file(1, cx)))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.move_file(-1, cx)))
            .on_action(
                cx.listener(|this, _: &menu::SelectFirst, _, cx| {
                    this.move_file(isize::MIN / 2, cx)
                }),
            )
            .on_action(
                cx.listener(|this, _: &menu::SelectLast, _, cx| this.move_file(isize::MAX / 2, cx)),
            )
            .on_action(cx.listener(|this, _: &menu::SelectChild, _, cx| this.file_child(true, cx)))
            .on_action(
                cx.listener(|this, _: &menu::SelectParent, _, cx| this.file_child(false, cx)),
            )
            .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                let index = this.nav.browsers.files_selected;
                this.activate_file(index, false, window, cx)
            }))
            .on_action(cx.listener(|this, _: &menu::SecondaryConfirm, window, cx| {
                let index = this.nav.browsers.files_selected;
                this.activate_file(index, true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &OpenContextMenu, window, cx| {
                let index = this.nav.browsers.files_selected;
                this.deploy_file_menu(index, None, window, cx)
            }))
            .on_action(cx.listener(|this, _: &CopyLink, _, cx| {
                if let Some(page) = this.selected_file_page() {
                    this.copy_link(&page, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &ToggleFavorite, _, cx| {
                if let Some(page) = this.selected_file_page() {
                    this.toggle_favorite_of(Destination::page(page), cx)
                }
            }))
            .on_action(
                cx.listener(|this, _: &RevealInFileManager, _, cx| this.reveal_selected_file(cx)),
            )
            .on_action(cx.listener(|this, _: &NewPageHere, window, cx| {
                this.new_page_in_selected_folder(window, cx)
            }))
            .on_action(
                cx.listener(|this, _: &crate::navigation::NewFolderHere, window, cx| {
                    this.new_folder_in_selected_folder(window, cx)
                }),
            )
            .when(open, |div| div.child(list));
        let heading = h_flex()
            .mt_6()
            .mb_1()
            .pl_1()
            .justify_between()
            .child(
                Button::new("files-heading", "FILES")
                    .start_icon(
                        ui::Icon::new(if open {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(ui::IconSize::XSmall)
                        .color(Color::Muted),
                    )
                    .label_size(LabelSize::XSmall)
                    .color(Color::Muted)
                    .aria_expanded(open)
                    .tab_index(heading)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_files(cx))),
            )
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        ui::ButtonLike::new("files-new-page")
                            .child(
                                ui::Icon::from_path(sidebar_views::NEW_FILE)
                                    .size(ui::IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .tab_index(new_page)
                            .tooltip(ui::Tooltip::text("New page"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.open_palette(
                                    crate::navigation::palette::PaletteMode::Create,
                                    window,
                                    cx,
                                )
                            })),
                    )
                    .child(
                        IconButton::new("files-new-folder", IconName::FolderAdd)
                            .icon_size(ui::IconSize::Small)
                            .tab_index(new_folder)
                            .tooltip(ui::Tooltip::text("New folder"))
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.ask_new_folder(String::new(), window, cx)
                            })),
                    )
                    .child(
                        IconButton::new("refresh-files", IconName::RotateCw)
                            .icon_size(ui::IconSize::Small)
                            .tab_index(refresh)
                            .tooltip(ui::Tooltip::text("Refresh files"))
                            .on_click(cx.listener(|this, _, _, cx| this.refresh_files(cx))),
                    ),
            );
        v_flex()
            .flex_1()
            .min_h_0()
            .child(heading)
            .when(open && count == 0, |div| {
                div.child(Label::new("A fresh notebook. Add your first page.").color(Color::Muted))
            })
            .child(tree)
    }

    fn destination_item(
        &self,
        id: impl Into<gpui::ElementId>,
        destination: &Destination,
        sidebar: menu_views::Sidebar,
        cx: &mut gpui::Context<Self>,
    ) -> ListItem {
        let label = self.destination_label(destination, cx);
        let page = destination.page_id().map(str::to_owned);
        ListItem::new(id)
            .when_some(page, |item, page| {
                item.on_secondary_mouse_down(cx.listener(
                    move |this, event: &gpui::MouseDownEvent, window, cx| {
                        cx.stop_propagation();
                        this.deploy_sidebar_menu(page.clone(), sidebar, event.position, window, cx)
                    },
                ))
            })
            .spacing(ListItemSpacing::Dense)
            .toggle_state(self.nav.workspace.active.as_ref() == Some(destination))
            .child(Label::new(label).truncate())
    }

    /// A sidebar entry as a tab stop at `index`: Tab reaches it, and Enter or Space opens it
    /// as a click does (Shift-Enter beside, for a page).
    fn entry_stop(
        &self,
        id: impl Into<gpui::ElementId>,
        index: isize,
        item: ListItem,
        open: impl Fn(&mut Self, bool, &mut Window, &mut gpui::Context<Self>) + 'static,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let ring = cx.theme().colors().border_focused;
        gpui::div()
            .id(id)
            .tab_index(index)
            .tab_stop(true)
            .rounded_sm()
            .focus_visible(move |style| style.border_1().border_color(ring))
            .child(item)
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                open(this, shift(event), window, cx)
            }))
    }

    fn destination_stop(
        &self,
        id: impl Into<gpui::ElementId>,
        index: isize,
        item: ListItem,
        destination: &Destination,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let target = destination.clone();
        self.entry_stop(
            id,
            index,
            item,
            move |this, beside, window, cx| match (target.page_id(), beside) {
                (Some(page), true) => this.open_beside(page.to_owned(), window, cx),
                _ => this.visit(target.clone(), window, cx),
            },
            cx,
        )
    }

    /// The left panel, top to bottom as the reference's: the brand, the notebook card, Find,
    /// the view list, Favorites, Files, Recent, and the footer.
    pub(in crate::editor_host::stored) fn render_sidebar(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let mut order = crate::tab_order::TabOrder::default();
        let card = self.render_profile_card(order.next(), cx);
        let find = self.render_find_trigger(order.next(), cx);
        let nav = self.render_nav(&mut order, cx);
        let favorites: Vec<_> = self
            .nav
            .workspace
            .favorites
            .iter()
            .enumerate()
            .map(|(index, favorite)| {
                let item = self
                    .destination_item(
                        ("favorite", index),
                        favorite,
                        menu_views::Sidebar::Favorite,
                        cx,
                    )
                    .start_slot(
                        ui::Icon::new(IconName::Star)
                            .size(ui::IconSize::Small)
                            .color(Color::Accent),
                    );
                self.destination_stop(("favorite-entry", index), order.next(), item, favorite, cx)
            })
            .collect();
        let files = [
            order.next(),
            order.next(),
            order.next(),
            order.next(),
            order.next(),
        ];
        let files = self.render_files(files, cx);
        let recent = self.render_recent(&mut order, cx);
        let footer = self.render_sidebar_footer(&mut order, cx);
        let colors = cx.theme().colors();
        v_flex()
            .id("sidebar")
            .key_context("QuireSidebar")
            .tab_group()
            .tab_index(0)
            .on_action(
                cx.listener(|_, _: &crate::navigation::NextControl, window, cx| {
                    window.focus_next(cx)
                }),
            )
            .on_action(
                cx.listener(|_, _: &crate::navigation::PreviousControl, window, cx| {
                    window.focus_prev(cx)
                }),
            )
            .w(gpui::relative(sidebar_views::SIDEBAR_SHARE))
            .min_w(gpui::px(sidebar_views::SIDEBAR_WIDTH[0]))
            .max_w(gpui::px(sidebar_views::SIDEBAR_WIDTH[1]))
            .flex_none()
            .h_full()
            .px_3()
            .bg(colors.panel_background)
            .border_r_1()
            .border_color(colors.border)
            .children(self.render_sidebar_drag())
            .when(!crate::title_bar::custom(), |div| div.pt_3())
            .child(self.render_brand(cx))
            .child(gpui::div().mb_4().child(card))
            .child(gpui::div().mx_px().mb_5().child(find))
            .child(nav)
            .when(!favorites.is_empty(), |div| {
                div.child(
                    v_flex()
                        .flex_none()
                        .max_h(gpui::px(180.))
                        .child(
                            gpui::div()
                                .mt_6()
                                .mb_2()
                                .mx_2()
                                .child(sidebar_views::section_label("FAVORITES")),
                        )
                        .children(favorites),
                )
            })
            .child(files)
            .children(recent)
            .child(footer)
    }

    pub(in crate::editor_host::stored) fn render_tabs(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let tabs: Vec<_> = self
            .nav
            .workspace
            .tabs
            .iter()
            .enumerate()
            .map(|(index, tab)| {
                let label = self.destination_label(tab, cx);
                let target = tab.clone();
                let closing = tab.clone();
                let middle = tab.clone();
                let saving = self
                    .tab_save_state(tab, cx)
                    .filter(|state| state.dot().is_some());
                let tooltip = match saving {
                    Some(SaveState::Failed) => {
                        format!("{label} · Needs attention")
                    }
                    Some(_) => format!("{label} · Saving changes"),
                    None => label.to_string(),
                };
                Tab::new(("tab", index))
                    .debug_selector(move || format!("quire-tab-{index}"))
                    .toggle_state(self.nav.workspace.active.as_ref() == Some(tab))
                    .tooltip(ui::Tooltip::text(tooltip))
                    .start_slot(
                        ui::Icon::new(tab_icon(tab))
                            .size(ui::IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .min_w_0()
                            .child(Label::new(label).truncate())
                            .children(saving.and_then(|state| state.dot()).map(|color| {
                                gpui::div()
                                    .debug_selector(move || format!("quire-tab-dot-{index}"))
                                    .size_1p5()
                                    .flex_none()
                                    .rounded_full()
                                    .bg(color.color(cx))
                            })),
                    )
                    .end_slot(
                        IconButton::new(("close-tab", index), IconName::Close)
                            .icon_size(ui::IconSize::XSmall)
                            .tooltip(ui::Tooltip::text("Close tab"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                let closing = closing.clone();
                                this.close_tabs(move |tab| tab == &closing, window, cx)
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.visit(target.clone(), window, cx)
                    }))
                    .on_mouse_down(
                        gpui::MouseButton::Right,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            this.deploy_tab_menu(index, event.position, window, cx)
                        }),
                    )
                    .on_mouse_down(
                        gpui::MouseButton::Middle,
                        cx.listener(move |this, _, window, cx| {
                            let middle = middle.clone();
                            this.close_tabs(move |tab| tab == &middle, window, cx)
                        }),
                    )
            })
            .collect();
        let favorite = self
            .nav
            .workspace
            .active
            .as_ref()
            .is_some_and(|active| self.nav.workspace.is_favorite(active));
        TabBar::new("tabs")
            .start_child(
                IconButton::new("back", IconName::ArrowLeft)
                    .disabled(!self.nav.history.can_go_back())
                    .tooltip(ui::Tooltip::text("Back"))
                    .on_click(cx.listener(|this, _, window, cx| this.go(true, window, cx))),
            )
            .start_child(
                IconButton::new("forward", IconName::ArrowRight)
                    .disabled(!self.nav.history.can_go_forward())
                    .tooltip(ui::Tooltip::text("Forward"))
                    .on_click(cx.listener(|this, _, window, cx| this.go(false, window, cx))),
            )
            .children(tabs)
            .end_child(
                IconButton::new(
                    "favorite",
                    if favorite {
                        IconName::StarFilled
                    } else {
                        IconName::Star
                    },
                )
                .toggle_state(favorite)
                .tooltip(ui::Tooltip::text("Toggle favorite"))
                .on_click(cx.listener(|this, _, _, cx| this.toggle_favorite(cx))),
            )
            .end_child(
                IconButton::new("side-pane", IconName::Split)
                    .toggle_state(self.nav.side.is_some())
                    .tooltip(ui::Tooltip::text("Toggle second pane"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_side(window, cx))),
            )
    }

    pub(in crate::editor_host::stored) fn render_browser(
        &mut self,
        view: BrowserView,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let count = self.browser_rows(cx).len();
        let focus = self
            .nav
            .browsers
            .list_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let list = uniform_list(
            "browser",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let rows = this.browser_rows(cx);
                range
                    .filter_map(|index| {
                        let row = rows.get(index)?.clone();
                        let page = row.page.clone();
                        Some(
                            ListItem::new(("browser-row", index))
                                .spacing(ListItemSpacing::Sparse)
                                .toggle_state(index == this.nav.browsers.selected)
                                .child(
                                    v_flex().child(Label::new(row.title).truncate()).child(
                                        Label::new(row.detail)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .truncate(),
                                    ),
                                )
                                .on_click(cx.listener(move |this, event, window, cx| {
                                    this.open_page(page.clone(), shift(event), window, cx)
                                })),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.nav.browsers.scroll)
        .flex_1();
        let heading = match view {
            BrowserView::Pages => "Pages",
            BrowserView::Daily => "Daily notes",
            BrowserView::Tags => "Tags",
            BrowserView::Tasks => "Tasks",
            BrowserView::Ai => ASK_THE_NOTEBOOK,
        };
        let chips: Vec<_> = match view {
            BrowserView::Pages => namespaces(
                self.nav
                    .pages
                    .iter()
                    .map(|listed| listed.page.title.as_str()),
            )
            .into_iter()
            .enumerate()
            .map(|(index, (namespace, count))| {
                let chosen =
                    self.nav.workspace.pages.namespace.as_deref() == Some(namespace.as_str());
                let value = namespace.clone();
                Button::new(("namespace", index), format!("{namespace} ({count})"))
                    .toggle_state(chosen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let namespace = (!chosen).then(|| value.clone());
                        this.remember(|workspace| workspace.pages.namespace = namespace, cx);
                        this.nav.browsers.selected = 0;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect(),
            BrowserView::Daily => vec![
                self.daily_month_menu(window, cx).into_any_element(),
                self.render_daily_settings(cx).into_any_element(),
            ],
            BrowserView::Tags | BrowserView::Tasks | BrowserView::Ai => Vec::new(),
        };
        let rows =
            v_flex()
                .id("browser")
                .key_context("QuireList")
                .track_focus(&focus)
                .flex_1()
                .min_h_0()
                .gap_2()
                .on_action(cx.listener(|this, _: &menu::SelectNext, _, cx| this.select_row(1, cx)))
                .on_action(
                    cx.listener(|this, _: &menu::SelectPrevious, _, cx| this.select_row(-1, cx)),
                )
                .on_action(cx.listener(|this, _: &menu::SelectFirst, _, cx| {
                    this.select_row(isize::MIN / 2, cx)
                }))
                .on_action(cx.listener(|this, _: &menu::SelectLast, _, cx| {
                    this.select_row(isize::MAX / 2, cx)
                }))
                .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                    this.confirm_row(false, window, cx)
                }))
                .on_action(cx.listener(|this, _: &menu::SecondaryConfirm, window, cx| {
                    this.confirm_row(true, window, cx)
                }))
                .when_some(self.nav.browsers.filter.clone(), |div, filter| {
                    div.child(
                        gpui::div()
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .p_1()
                            .child(filter),
                    )
                })
                .when(count == 0, |div| {
                    div.child(Label::new("No pages match").color(Color::Muted))
                })
                .child(list);
        v_flex()
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .child(Label::new(heading))
            .child(h_flex().gap_1().flex_wrap().children(chips))
            .child(rows)
    }
}
