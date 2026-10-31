//! The page around its editor, as the reference page pane draws it: a header with the page's
//! kind, title, and size; the toolbar's save indicator, sidebar, Tidy, Vim, and details
//! toggles; the details panel with headings to jump to and the page's connections; and
//! README.md, the rendering showcase, beside the page.

use super::*;
use crate::app_menus::{ShowNotebookFolder, TogglePageDetails, ToggleSidebar, ToggleVim};
use crate::navigation::Jump;
use crate::page_details::{Heading, PageFacts, PageInfo};
use crate::showcase::{CloseShowcase, OpenShowcase, ShowcaseWindow};
use gpui::{AnyElement, EntityId, SharedString};
use quire_core::markdown::{plural, reading_minutes};
use quire_core::workspace::Destination;
use settings::Settings as _;
use ui::{
    Color, Disableable as _, Headline, HeadlineSize, Icon, IconButton, IconName, IconSize,
    LabelSize, ListItem, ListItemSpacing, Toggleable as _, Tooltip, h_flex, v_flex,
};

/// The placeholder of an empty page.
pub(super) const EMPTY_PAGE: &str = "Let a thought begin…";

#[derive(Default)]
pub(super) struct PageChrome {
    infos: HashMap<EntityId, Entity<PageInfo>>,
    /// The details panel is open beside the page.
    pub(super) details: bool,
    /// The side pane's page has its own details panel open.
    pub(super) side_details: bool,
    /// README.md beside the pages, while it is open.
    pub(super) showcase: Option<Entity<ShowcaseWindow>>,
}

/// How far a page's text is saved, as the toolbar icon and a tab's dot show it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SaveState {
    Saved,
    Pending,
    Failed,
}

impl SaveState {
    /// The state of a page that can save, or None while it loads or when it is read-only.
    pub(super) fn of(page: &OpenPage, cx: &App) -> Option<Self> {
        let persistence = page.persistence()?.read(cx);
        Some(if persistence.has_error() {
            Self::Failed
        } else if persistence.is_dirty(cx) {
            Self::Pending
        } else {
            Self::Saved
        })
    }

    fn icon(self) -> Icon {
        let (path, color) = match self {
            Self::Saved => ("quire/icons/save/saved.svg", Color::Success),
            Self::Pending => ("quire/icons/save/pending.svg", Color::Warning),
            Self::Failed => ("quire/icons/save/failed.svg", Color::Error),
        };
        Icon::from_path(path).size(IconSize::Small).color(color)
    }

    /// What hovering the icon says before the page's own status.
    pub(super) fn summary(self) -> &'static str {
        match self {
            Self::Saved => "Saved locally on this computer. Sync is not confirmed.",
            Self::Pending => "Changes are being saved locally.",
            Self::Failed => "Save needs attention.",
        }
    }

    /// The color of a tab's dot, for a page whose saving is not finished.
    pub(super) fn dot(self) -> Option<Color> {
        match self {
            Self::Saved => None,
            Self::Pending => Some(Color::Warning),
            Self::Failed => Some(Color::Error),
        }
    }
}

/// "3 blocks · 1 min read", as the reference header counts them.
pub(super) fn subtitle(facts: &PageFacts) -> String {
    let blocks = plural(facts.blocks, "block");
    match facts.words {
        0 => blocks,
        words => format!("{blocks} · {} min read", reading_minutes(words)),
    }
}

/// Why Tidy cannot run now: AI is off or untested, AI writes are Off, or the page is read-only.
pub(super) fn tidy_blocked(page: &OpenPage, cx: &App) -> Option<&'static str> {
    crate::ai::answer::blocked(cx)
        .or_else(|| {
            (crate::ai::settings::AiSettings::get_global(cx).writes
                == quire_core::ai::tools::Writes::Off)
                .then_some(quire_core::ai::tidy::WRITES_OFF)
        })
        .or_else(|| {
            page.persistence()
                .is_none()
                .then_some("This page is read-only, so Tidy cannot change it.")
        })
}

impl StoredWindow {
    pub(super) fn register_page_views(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        div.on_action(cx.listener(|this, _: &ToggleSidebar, _, cx| this.toggle_sidebar(cx)))
            .on_action(
                cx.listener(|this, _: &TogglePageDetails, _, cx| this.toggle_page_details(cx)),
            )
            .on_action(
                cx.listener(|this, _: &ShowNotebookFolder, _, cx| this.show_notebook_folder(cx)),
            )
            .on_action(
                cx.listener(|this, _: &OpenShowcase, window, cx| this.open_showcase(window, cx)),
            )
    }

    fn toggle_sidebar(&mut self, cx: &mut gpui::Context<Self>) {
        self.remember(
            |workspace| workspace.chrome.sidebar_hidden = !workspace.chrome.sidebar_hidden,
            cx,
        );
        cx.notify();
    }

    fn toggle_page_details(&mut self, cx: &mut gpui::Context<Self>) {
        self.chrome.details = !self.chrome.details;
        cx.notify();
    }

    /// Shows the folder the notebook lives in: a folder notebook's root, a Quire notebook's
    /// synced folder, or the database of a notebook kept on this computer only.
    pub(super) fn show_notebook_folder(&mut self, cx: &mut gpui::Context<Self>) {
        use quire_store::profiles::Notebook;
        let location = self.session.borrow().location();
        let target = match location {
            Some(Notebook::Folder { root }) => root,
            Some(Notebook::Store {
                folder: Some(folder),
                ..
            }) => folder,
            Some(Notebook::Store { database, .. }) => database,
            None => self.nav.database.clone(),
        };
        Self::show_in_file_manager(&target, cx);
    }

    /// The page's figures, kept for the page while the window lives, shown in `editor`'s
    /// status bar.
    pub(super) fn attach_page_info(
        &mut self,
        page: &Entity<OpenPage>,
        editor: &Entity<NotebookWindow>,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(analysis) = page.read(cx).loaded().map(|loaded| loaded.analysis.clone()) else {
            return;
        };
        let info = match self.chrome.infos.get(&page.entity_id()) {
            Some(info) => info.clone(),
            None => {
                let info = cx.new(|cx| PageInfo::new(page.clone(), analysis, cx));
                cx.observe(&info, |_, _, cx| cx.notify()).detach();
                self.chrome.infos.insert(page.entity_id(), info.clone());
                info
            }
        };
        editor.update(cx, |editor, _| editor.status = Some(info.into()));
    }

    /// What an empty page says in each of its editors, as the reference editor does.
    pub(super) fn invite_writing(
        editor: &Entity<NotebookWindow>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let panes = {
            let view = editor.read(cx);
            [view.source.clone(), view.second.clone()]
        };
        for pane in panes {
            pane.update(cx, |pane, cx| {
                pane.set_placeholder_text(EMPTY_PAGE, window, cx)
            });
        }
    }

    pub(super) fn page_facts(&self, page: &Entity<OpenPage>, cx: &App) -> Option<PageFacts> {
        self.chrome
            .infos
            .get(&page.entity_id())?
            .read(cx)
            .facts()
            .cloned()
    }

    /// The eyebrow over a page's title: a daily note under the notebook's daily layout, or a page.
    pub(super) fn page_kind(&self, title: &str) -> &'static str {
        if self.daily_date_of(title).is_some() {
            "DAILY NOTE"
        } else {
            "A PAGE IN YOUR NOTEBOOK"
        }
    }

    /// The page's kind, title, and size above its editor.
    pub(super) fn render_page_header(&self, page: &Entity<OpenPage>, cx: &App) -> AnyElement {
        let title = self.destination_label(&Destination::page(page.read(cx).id()), cx);
        let kind = self.page_kind(&title);
        let facts = self.page_facts(page, cx);
        v_flex()
            .px_4()
            .pt_3()
            .pb_1()
            .gap_1()
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Icon::new(IconName::FileTextOutlined)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Label::new(kind).size(LabelSize::XSmall).color(Color::Muted)),
            )
            .child(Headline::new(title).size(HeadlineSize::Large))
            .when_some(facts, |header, facts| {
                header.child(
                    Label::new(subtitle(&facts))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
            })
            .into_any_element()
    }

    /// The save icon with its explanation on hover, and the status in words when it needs
    /// reading: an error, a conflict, a deletion, a warning, or a page that cannot save.
    pub(super) fn render_save_status(&self, cx: &App) -> Vec<AnyElement> {
        Self::save_status(&self.page, "quire-save-indicator", cx)
    }

    fn save_status(page: &Entity<OpenPage>, selector: &'static str, cx: &App) -> Vec<AnyElement> {
        let page = page.read(cx);
        let status = page.status(cx);
        let Some(state) = SaveState::of(page, cx) else {
            return vec![Label::new(status).into_any_element()];
        };
        let notice = page
            .persistence()
            .is_some_and(|persistence| persistence.read(cx).has_notice());
        let explained = format!("{} {status}", state.summary());
        let icon = gpui::div()
            .id("save-indicator")
            .debug_selector(move || selector.into())
            .size_5()
            .flex()
            .items_center()
            .justify_center()
            .child(state.icon())
            .tooltip(Tooltip::text(explained))
            .into_any_element();
        std::iter::once(icon)
            .chain(notice.then(|| Label::new(status).into_any_element()))
            .collect()
    }

    /// The tools at the end of the page toolbar, as the reference orders them.
    pub(super) fn render_page_tools(&self, cx: &mut gpui::Context<Self>) -> gpui::Div {
        let showing_page = self.nav.view.is_none() && self.nav.tags.shown.is_none();
        let page = self.page.read(cx);
        let daily = showing_page
            && self
                .nav
                .title(page.id())
                .is_some_and(|title| self.daily_date_of(title).is_some());
        let vim = crate::preferences::Preferences::current(cx).vim;
        h_flex()
            .ml_auto()
            .gap_1()
            .when(daily, |tools| {
                let blocked = tidy_blocked(page, cx);
                tools.child(
                    Button::new("tidy-day", "Tidy")
                        .start_icon(Icon::new(IconName::Sparkle).size(IconSize::Small))
                        .disabled(blocked.is_some())
                        .tooltip(Tooltip::text(blocked.unwrap_or(
                            "Propose links, task states and due dates for this day. Nothing changes until you apply it.",
                        )))
                        .on_click(cx.listener(|this, _, window, cx| this.tidy_shown(window, cx))),
                )
            })
            .child(
                Button::new("vim-toggle", "Vim")
                    .toggle_state(vim)
                    .tooltip(Tooltip::text("Toggle Vim bindings for all editors"))
                    .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleVim), cx)),
            )
            .child(
                IconButton::new("page-details", IconName::Link)
                    .toggle_state(self.chrome.details)
                    .disabled(!showing_page)
                    .tooltip(Tooltip::text("Page outline and backlinks"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(TogglePageDetails), cx)
                    }),
            )
            .child(
                IconButton::new("show-notebook-folder", IconName::FolderOpen)
                    .tooltip(Tooltip::text("Show notebook folder"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ShowNotebookFolder), cx)
                    }),
            )
    }

    /// The side pane's own toolbar, as rt's second page pane has one: its page's save icon and
    /// its details toggle.
    pub(super) fn render_side_tools(
        &self,
        page: &Entity<OpenPage>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        h_flex()
            .px_2()
            .pt_1()
            .gap_1()
            .children(Self::save_status(page, "quire-side-save-indicator", cx))
            .child(gpui::div().flex_1())
            .child(
                gpui::div()
                    .debug_selector(|| "quire-side-details-toggle".into())
                    .child(
                        IconButton::new("side-page-details", IconName::Link)
                            .toggle_state(self.chrome.side_details)
                            .tooltip(Tooltip::text("Page outline and backlinks"))
                            .on_click(cx.listener(|this, _, _, cx| this.toggle_details(true, cx))),
                    ),
            )
            .into_any_element()
    }

    pub(super) fn render_sidebar_toggle(&self) -> IconButton {
        let hidden = self.nav.workspace.chrome.sidebar_hidden;
        IconButton::new(
            "toggle-sidebar",
            if hidden {
                IconName::ThreadsSidebarLeftClosed
            } else {
                IconName::ThreadsSidebarLeftOpen
            },
        )
        .toggle_state(!hidden)
        .tooltip(Tooltip::text("Toggle sidebar"))
        .on_click(|_, window, cx| window.dispatch_action(Box::new(ToggleSidebar), cx))
    }

    fn jump_to_heading(
        &mut self,
        heading: &Heading,
        side: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let jump = Jump::at(heading.block.clone(), heading.from);
        if side {
            self.reveal_in_side(jump, window, cx)
        } else {
            self.reveal_in_shown(jump, window, cx)
        }
    }

    fn toggle_details(&mut self, side: bool, cx: &mut gpui::Context<Self>) {
        if side {
            self.chrome.side_details = !self.chrome.side_details;
            cx.notify();
        } else {
            self.toggle_page_details(cx);
        }
    }

    /// The details panel beside the shown page: its headings, then its connections.
    pub(super) fn render_details(&self, cx: &mut gpui::Context<Self>) -> Option<AnyElement> {
        let showing_page = self.nav.view.is_none() && self.nav.tags.shown.is_none();
        (self.chrome.details && showing_page).then(|| self.details_panel(&self.page, false, cx))
    }

    /// The side pane page's own details panel, while its toggle is on.
    pub(super) fn render_side_details(
        &self,
        page: &Entity<OpenPage>,
        cx: &mut gpui::Context<Self>,
    ) -> Option<AnyElement> {
        self.chrome
            .side_details
            .then(|| self.details_panel(page, true, cx))
    }

    fn details_panel(
        &self,
        page: &Entity<OpenPage>,
        side: bool,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let facts = self.page_facts(page, cx).unwrap_or_default();
        let references = self
            .nav
            .references
            .get(page.read(cx).id())
            .map(|view| view.read(cx).results.value.total);
        let heading =
            |text: &'static str| Label::new(text).size(LabelSize::XSmall).color(Color::Muted);
        let count = |label: &'static str, value: SharedString| {
            h_flex()
                .justify_between()
                .child(Label::new(label).size(LabelSize::Small))
                .child(Label::new(value).size(LabelSize::Small))
        };
        let outline: Vec<_> = facts
            .headings
            .iter()
            .enumerate()
            .map(|(index, entry)| {
                let chosen = entry.clone();
                ListItem::new(("details-heading", index))
                    .spacing(ListItemSpacing::Dense)
                    .start_slot(
                        Icon::new(IconName::Hash)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(Label::new(entry.text.clone()).truncate())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.jump_to_heading(&chosen, side, window, cx)
                    }))
            })
            .collect();
        v_flex()
            .id(if side {
                "side-page-details"
            } else {
                "page-details"
            })
            .debug_selector(move || {
                if side {
                    "quire-side-page-details"
                } else {
                    "quire-page-details"
                }
                .into()
            })
            .w(gpui::px(260.))
            .flex_none()
            .min_h_0()
            .overflow_y_scroll()
            .p_2()
            .gap_1()
            .border_l_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .justify_between()
                    .child(heading("ON THIS PAGE"))
                    .child(
                        IconButton::new("close-details", IconName::Close)
                            .icon_size(IconSize::XSmall)
                            .tooltip(Tooltip::text("Close page details"))
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.toggle_details(side, cx)),
                            ),
                    ),
            )
            .when(outline.is_empty(), |panel| {
                panel.child(Label::new("No headings yet.").color(Color::Muted))
            })
            .children(outline)
            .child(gpui::div().pt_2().child(heading("CONNECTIONS")))
            .child(count(
                "Linked references",
                references.map_or_else(|| "…".into(), |total| total.to_string().into()),
            ))
            .child(count("Blocks with an ID", facts.ids.to_string().into()))
            .child(
                h_flex()
                    .pt_2()
                    .gap_1()
                    .child(
                        Icon::new(IconName::Link)
                            .size(IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Label::new(
                            "Shift-click a link to keep this page open beside the next thought.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    ),
            )
            .into_any_element()
    }

    /// Opens README.md beside the pages, as the reference's info pane does, in place of Help.
    pub(super) fn open_showcase(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(showcase) = &self.chrome.showcase {
            let focus = showcase.read(cx).notebook.read(cx).source.focus_handle(cx);
            window.focus(&focus, cx);
            return;
        }
        self.help = None;
        let page = crate::showcase::sample(cx);
        let showcase = cx.new(|cx| ShowcaseWindow::new(page, true, window, cx));
        cx.subscribe_in(
            &showcase,
            window,
            |this, _, _: &CloseShowcase, window, cx| {
                this.chrome.showcase = None;
                this.focus_active(window, cx);
                cx.notify();
            },
        )
        .detach();
        self.chrome.showcase = Some(showcase);
        cx.notify();
    }

    pub(super) fn showcase_pane(&self, cx: &gpui::Context<Self>) -> Option<AnyElement> {
        let showcase = self.chrome.showcase.clone()?;
        Some(
            gpui::div()
                .tab_group()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .border_l_1()
                .border_color(cx.theme().colors().border)
                .child(showcase)
                .into_any_element(),
        )
    }
}
