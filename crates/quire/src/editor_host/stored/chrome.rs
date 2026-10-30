//! The window's navigation surfaces: sidebar (Favorites, Recent, Files), tab strip, and the
//! Pages and Daily notes browsers. Every list is a virtualized `uniform_list`.

use super::*;
use editor::{Editor, EditorEvent};
use gpui::{ClickEvent, FocusHandle, ScrollStrategy, UniformListScrollHandle, uniform_list};
use quire_core::workspace::{
    BrowserView, Destination, SHOWN_RECENT, TreeKind, TreeRow, folder_rows, in_namespace,
    namespaces,
};
use std::collections::BTreeSet;
use ui::{
    Color, Disableable, IconButton, IconName, LabelSize, ListItem, ListItemSpacing, Tab, TabBar,
    Toggleable, h_flex, v_flex,
};

#[derive(Default)]
pub(in crate::editor_host::stored) struct Browsers {
    pub(in crate::editor_host::stored) filter: Option<Entity<Editor>>,
    pub(in crate::editor_host::stored) namespace: Option<String>,
    pub(in crate::editor_host::stored) selected: usize,
    pub(in crate::editor_host::stored) list_focus: Option<FocusHandle>,
    scroll: UniformListScrollHandle,
    pub(in crate::editor_host::stored) files_selected: usize,
    pub(in crate::editor_host::stored) files_focus: Option<FocusHandle>,
    files_scroll: UniformListScrollHandle,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::editor_host::stored) struct BrowserRow {
    pub(in crate::editor_host::stored) page: String,
    pub(in crate::editor_host::stored) title: String,
    pub(in crate::editor_host::stored) detail: String,
}

fn shift(event: &ClickEvent) -> bool {
    event.modifiers().shift
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
        let filter = self
            .nav
            .browsers
            .filter
            .get_or_insert_with(|| {
                let editor = cx.new(|cx| {
                    let mut editor = Editor::single_line(window, cx);
                    editor.set_placeholder_text("Filter…", window, cx);
                    editor
                });
                cx.subscribe(&editor, |this, _, event: &EditorEvent, cx| {
                    if matches!(event, EditorEvent::BufferEdited) {
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
                        .browsers
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
                dated
                    .into_iter()
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
            Some(BrowserView::Tags) | None => Vec::new(),
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

    pub(in crate::editor_host::stored) fn file_rows(&self) -> Vec<TreeRow> {
        let paths: Vec<String> = self
            .nav
            .pages
            .iter()
            .map(|listed| listed.page.path.clone())
            .collect();
        let expanded: BTreeSet<String> = self.nav.workspace.expanded.iter().cloned().collect();
        folder_rows(&paths, &[], &expanded, |a, b| {
            a.to_lowercase()
                .cmp(&b.to_lowercase())
                .then_with(|| a.cmp(b))
        })
    }

    fn page_at_path(&self, path: &str) -> Option<String> {
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
        let Some(row) = self.file_rows().into_iter().nth(index) else {
            return;
        };
        self.nav.browsers.files_selected = index;
        match row.kind {
            TreeKind::Folder => {
                self.nav.workspace.toggle_expanded(&row.path);
                self.persist(cx);
            }
            TreeKind::File => {
                if let Some(page) = self.page_at_path(&row.path) {
                    self.open_page(page, beside, window, cx);
                }
            }
        }
        cx.notify();
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

    fn render_files(&mut self, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let focus = self
            .nav
            .browsers
            .files_focus
            .get_or_insert_with(|| cx.focus_handle())
            .clone();
        let count = self.file_rows().len();
        let list = uniform_list(
            "files",
            count,
            cx.processor(|this, range: std::ops::Range<usize>, _, cx| {
                let rows = this.file_rows();
                range
                    .filter_map(|index| {
                        let row = rows.get(index)?.clone();
                        let expanded = this.nav.workspace.expanded.contains(&row.path);
                        let icon = match (row.kind, expanded) {
                            (TreeKind::Folder, true) => IconName::FolderOpen,
                            (TreeKind::Folder, false) => IconName::Folder,
                            (TreeKind::File, _) => IconName::File,
                        };
                        Some(
                            ListItem::new(("file", index))
                                .spacing(ListItemSpacing::Dense)
                                .indent_level(row.depth)
                                .toggle_state(index == this.nav.browsers.files_selected)
                                .start_slot(ui::Icon::new(icon).color(Color::Muted))
                                .child(Label::new(row.name.clone()).truncate())
                                .on_click(cx.listener(move |this, event, window, cx| {
                                    this.activate_file(index, shift(event), window, cx)
                                })),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.nav.browsers.files_scroll)
        .flex_1();
        v_flex()
            .id("files-tree")
            .key_context("QuireList")
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
            .child(
                Label::new("FILES")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .when(count == 0, |div| {
                div.child(Label::new("A fresh notebook. Add your first page.").color(Color::Muted))
            })
            .child(list)
    }

    fn destination_item(
        &self,
        id: impl Into<gpui::ElementId>,
        destination: &Destination,
        cx: &mut gpui::Context<Self>,
    ) -> ListItem {
        let label = self.destination_label(destination, cx);
        let target = destination.clone();
        ListItem::new(id)
            .spacing(ListItemSpacing::Dense)
            .toggle_state(self.nav.workspace.active.as_ref() == Some(destination))
            .child(Label::new(label).truncate())
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                match (target.page_id(), shift(event)) {
                    (Some(page), true) => this.open_beside(page.to_owned(), window, cx),
                    _ => this.visit(target.clone(), window, cx),
                }
            }))
    }

    pub(in crate::editor_host::stored) fn render_sidebar(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement {
        let favorites: Vec<_> = self
            .nav
            .workspace
            .favorites
            .iter()
            .enumerate()
            .map(|(index, favorite)| {
                self.destination_item(("favorite", index), favorite, cx)
                    .start_slot(ui::Icon::new(IconName::Star).color(Color::Accent))
            })
            .collect();
        let recent: Vec<_> = self
            .nav
            .workspace
            .recent
            .iter()
            .take(SHOWN_RECENT)
            .enumerate()
            .map(|(index, item)| {
                let removing = item.clone();
                self.destination_item(("recent", index), item, cx).end_slot(
                    IconButton::new(("remove-recent", index), IconName::Close)
                        .icon_size(ui::IconSize::XSmall)
                        .tooltip(ui::Tooltip::text("Remove from recent"))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.nav.workspace.remove_recent(&removing);
                            this.persist(cx);
                            cx.notify();
                        })),
                )
            })
            .collect();
        v_flex()
            .w(gpui::px(240.))
            .h_full()
            .p_1()
            .gap_1()
            .border_r_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .gap_1()
                    .flex_wrap()
                    .child(Button::new("find", "Find").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_palette(
                                crate::navigation::palette::PaletteMode::Find,
                                window,
                                cx,
                            )
                        },
                    )))
                    .child(
                        Button::new("today", "Today").on_click(
                            cx.listener(|this, _, window, cx| this.open_today(window, cx)),
                        ),
                    )
                    .child(Button::new("pages", "Pages").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.visit(
                                Destination::Browser {
                                    view: BrowserView::Pages,
                                },
                                window,
                                cx,
                            )
                        },
                    )))
                    .child(Button::new("daily", "Daily").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.visit(
                                Destination::Browser {
                                    view: BrowserView::Daily,
                                },
                                window,
                                cx,
                            )
                        },
                    )))
                    .child(Button::new("tags", "Tags").on_click(cx.listener(
                        |this, _, window, cx| {
                            this.visit(
                                Destination::Browser {
                                    view: BrowserView::Tags,
                                },
                                window,
                                cx,
                            )
                        },
                    ))),
            )
            .when(!favorites.is_empty(), |div| {
                div.child(
                    Label::new("FAVORITES")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .children(favorites)
            })
            .when(!recent.is_empty(), |div| {
                div.child(
                    Label::new("RECENT")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .children(recent)
            })
            .child(self.render_files(cx))
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
                Tab::new(("tab", index))
                    .toggle_state(self.nav.workspace.active.as_ref() == Some(tab))
                    .child(Label::new(label).truncate())
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
                let chosen = self.nav.browsers.namespace.as_deref() == Some(namespace.as_str());
                let value = namespace.clone();
                Button::new(("namespace", index), format!("{namespace} ({count})"))
                    .toggle_state(chosen)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.nav.browsers.namespace = (!chosen).then(|| value.clone());
                        this.nav.browsers.selected = 0;
                        cx.notify();
                    }))
                    .into_any_element()
            })
            .collect(),
            BrowserView::Daily => vec![self.render_daily_settings(cx).into_any_element()],
            BrowserView::Tags => Vec::new(),
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
