//! The sidebar around Files, laid out as the reference's: the brand, the notebook card with its
//! profile menu, Find, the view list with the current view highlighted, Recent, and the footer
//! with New page, Open a folder, and the Roam import and export.

use super::*;
use crate::navigation::{
    FindPage, NewPage,
    pointer_menu::{self, Entry, Item},
};
use crate::profiles::{OpenNotebookFolder, UseCurrentProfile, UsePersonalProfile, UseWorkProfile};
use gpui::{ClickEvent, FontWeight, px};
use quire_core::workspace::{BrowserView, Destination};
use quire_store::profiles::{Notebook, ProfileId};
use ui::{
    ButtonLike, ButtonSize, ButtonStyle, Color, Disableable, FixedWidth, IconName, KeyBinding,
    LabelSize, ListItem, PopoverMenu, Toggleable, Tooltip, h_flex, v_flex,
};

const BOOK_OPEN: &str = "icons/quire/sidebar/book-open.svg";
const CALENDAR: &str = "icons/quire/sidebar/calendar-days.svg";
const TEMPLATE: &str = "icons/quire/sidebar/layout-template.svg";
const UPLOAD: &str = "icons/quire/sidebar/upload.svg";
pub(in crate::editor_host::stored) const NEW_FILE: &str =
    "icons/quire/sidebar/file-plus-corner.svg";

/// The sidebar takes 21% of the window between the reference's narrowest and widest widths, which
/// matches its 220, 225, and 272 px media-query steps at 850, 1067, and 1440 px windows.
pub(in crate::editor_host::stored) const SIDEBAR_WIDTH: [f32; 2] = [220., 272.];
pub(in crate::editor_host::stored) const SIDEBAR_SHARE: f32 = 0.21;

/// The view list, in the reference's order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::editor_host::stored) enum NavEntry {
    Readme,
    Today,
    Tasks,
    Templates,
    Pages,
    Tags,
    Daily,
    Help,
}

impl NavEntry {
    pub(in crate::editor_host::stored) const ALL: [Self; 8] = [
        Self::Readme,
        Self::Today,
        Self::Tasks,
        Self::Templates,
        Self::Pages,
        Self::Tags,
        Self::Daily,
        Self::Help,
    ];

    pub(in crate::editor_host::stored) fn label(self) -> &'static str {
        match self {
            Self::Readme => "README.md",
            Self::Today => "Today",
            Self::Tasks => "Tasks",
            Self::Templates => quire_core::templates::TEMPLATES_TITLE,
            Self::Pages => "Pages",
            Self::Tags => "Tags",
            Self::Daily => "Daily notes",
            Self::Help => "Help & shortcuts",
        }
    }

    /// The element id; tests find the entry by `quire-nav-<id>`.
    pub(in crate::editor_host::stored) fn id(self) -> &'static str {
        match self {
            Self::Readme => "readme",
            Self::Today => "today",
            Self::Tasks => "tasks",
            Self::Templates => "templates",
            Self::Pages => "pages",
            Self::Tags => "tags",
            Self::Daily => "daily",
            Self::Help => "help",
        }
    }

    fn icon(self) -> ui::Icon {
        match self {
            Self::Readme => ui::Icon::new(IconName::FileTextOutlined),
            Self::Today | Self::Daily => ui::Icon::from_path(CALENDAR),
            Self::Tasks => ui::Icon::new(IconName::ListTodo),
            Self::Templates => ui::Icon::from_path(TEMPLATE),
            Self::Pages => ui::Icon::new(IconName::FileMultiple),
            Self::Tags => ui::Icon::new(IconName::Hash),
            Self::Help => ui::Icon::new(IconName::CircleHelp),
        }
    }

    fn browser(self) -> Option<BrowserView> {
        match self {
            Self::Tasks => Some(BrowserView::Tasks),
            Self::Pages => Some(BrowserView::Pages),
            Self::Tags => Some(BrowserView::Tags),
            Self::Daily => Some(BrowserView::Daily),
            _ => None,
        }
    }

    /// The action whose keys open the entry, shown in its tooltip.
    fn keys(self) -> Option<Box<dyn gpui::Action>> {
        match self {
            Self::Today => Some(Box::new(crate::navigation::OpenToday)),
            Self::Help => Some(Box::new(crate::help::OpenHelp)),
            _ => None,
        }
    }
}

/// A section heading such as FAVORITES: small, muted, spaced capitals.
pub(in crate::editor_host::stored) fn section_label(text: &'static str) -> Label {
    Label::new(text)
        .size(LabelSize::XSmall)
        .weight(FontWeight::BOLD)
        .color(Color::Muted)
}

/// The name a notebook goes by: its folder's name.
fn notebook_name(location: Option<&Notebook>) -> String {
    location
        .and_then(|notebook| notebook.root().file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "Your notebook".into())
}

fn profile_action(id: ProfileId) -> Box<dyn gpui::Action> {
    match id {
        ProfileId::Current => Box::new(UseCurrentProfile),
        ProfileId::Work => Box::new(UseWorkProfile),
        ProfileId::Personal => Box::new(UsePersonalProfile),
    }
}

impl StoredWindow {
    /// The view list entries showing now: the open browser, today's daily page, Templates, and
    /// the README or Help pane while it is open beside the page.
    pub(in crate::editor_host::stored) fn current_nav(&self, cx: &App) -> Vec<NavEntry> {
        let page_shown = self.nav.view.is_none() && self.nav.tags.shown.is_none();
        let today = self.today().ok();
        let showing_today = page_shown
            && today.is_some()
            && self
                .nav
                .workspace
                .active
                .as_ref()
                .and_then(Destination::page_id)
                .and_then(|page| {
                    self.nav
                        .pages
                        .iter()
                        .find(|listed| listed.page.page.as_str() == Some(page))
                })
                .is_some_and(|listed| listed.date == today);
        NavEntry::ALL
            .into_iter()
            .filter(|entry| match entry {
                NavEntry::Readme => self.chrome.showcase.is_some(),
                NavEntry::Help => self.help.is_some(),
                NavEntry::Today => showing_today,
                NavEntry::Templates => self.showing_templates(cx),
                entry => entry.browser().is_some() && self.nav.view == entry.browser(),
            })
            .collect()
    }

    pub(in crate::editor_host::stored) fn open_nav(
        &mut self,
        entry: NavEntry,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        match (entry, entry.browser()) {
            (_, Some(view)) => self.visit(Destination::Browser { view }, window, cx),
            (NavEntry::Today, _) => self.open_today(window, cx),
            (NavEntry::Templates, _) => self.open_templates(window, cx),
            (NavEntry::Readme, _) => {
                window.dispatch_action(Box::new(crate::showcase::OpenShowcase), cx)
            }
            _ => crate::help::view::open(cx),
        }
    }

    /// The menu the notebook card opens: each profile, the open one ticked, then this notebook's
    /// last backup and push.
    pub(in crate::editor_host::stored) fn profile_menu_entries(&self, cx: &App) -> Vec<Entry> {
        let active = self
            .location
            .as_ref()
            .and_then(|location| crate::profiles::holding(location, cx));
        let status: SharedString = match active {
            Some(id) => crate::profiles::status(id, &*self.clock, cx),
            None => "No profile holds this notebook, so backups and Git sync are off".into(),
        };
        ProfileId::ALL
            .into_iter()
            .map(|id| {
                // A closure, not the action itself: a Zed menu catches an action its own entries
                // name and redispatches it after a delay, by which time this menu is gone.
                Item::run(id.label(), move |window, cx| {
                    window.dispatch_action(profile_action(id), cx)
                })
                .checked(Some(id) == active)
                .done()
            })
            .chain([
                Entry::Separator,
                Item::run(status, |_, _| {}).disabled(true).done(),
            ])
            .collect()
    }

    pub(in crate::editor_host::stored) fn render_brand(
        &self,
        cx: &App,
    ) -> impl IntoElement + use<> {
        h_flex()
            .gap_2p5()
            .mx_1()
            .mt_1()
            .mb_6()
            .child(
                h_flex()
                    .size(px(33.))
                    .justify_center()
                    .rounded_lg()
                    .border_1()
                    .border_color(cx.theme().colors().border_variant)
                    .bg(cx.theme().colors().element_background)
                    .child(
                        ui::Icon::from_path(BOOK_OPEN)
                            .size(ui::IconSize::Medium)
                            .color(Color::Accent),
                    ),
            )
            .child(
                h_flex()
                    .child(
                        Label::new("quire")
                            .size(LabelSize::Large)
                            .weight(FontWeight::SEMIBOLD),
                    )
                    .child(
                        Label::new(".")
                            .size(LabelSize::Large)
                            .weight(FontWeight::SEMIBOLD)
                            .color(Color::Accent),
                    ),
            )
    }

    /// The notebook card: its initial, the profile and notebook name, where it lives, and a
    /// chevron that opens the profile menu.
    pub(in crate::editor_host::stored) fn render_profile_card(
        &self,
        tab_index: isize,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let name = notebook_name(self.location.as_ref());
        let held = self
            .location
            .as_ref()
            .and_then(|location| crate::profiles::holding(location, cx));
        let title = match held {
            Some(id) => format!("{} · {name}", id.label()),
            None => name.clone(),
        };
        let trouble = held.and_then(|id| {
            let profile = crate::profiles::get(id, cx);
            (profile.backup_error.is_some() || profile.git_error.is_some())
                .then(|| crate::profiles::status(id, &*self.clock, cx))
        });
        let place: SharedString = match (&trouble, held, self.location.as_ref()) {
            (Some(trouble), ..) => trouble.clone(),
            (None, None, _) => "No profile: backups and Git sync off".into(),
            (
                None,
                Some(_),
                Some(Notebook::Store {
                    folder: Some(_), ..
                }),
            ) => "Synced through a folder".into(),
            _ => "On your computer".into(),
        };
        let initial: String = name.chars().next().into_iter().collect();
        let view = cx.entity().downgrade();
        let card = ButtonLike::new("profile-card")
            .full_width()
            .size(ButtonSize::Large)
            .height(px(50.).into())
            .tab_index(tab_index)
            .child(
                h_flex()
                    .w_full()
                    .gap_2p5()
                    .px_1()
                    .debug_selector(|| "quire-profile-card".into())
                    .child(
                        h_flex()
                            .flex_none()
                            .w(px(32.))
                            .h(px(34.))
                            .justify_center()
                            .rounded_md()
                            .border_1()
                            .border_color(cx.theme().colors().border_variant)
                            .bg(cx.theme().colors().element_background)
                            .child(
                                Label::new(initial)
                                    .size(LabelSize::Large)
                                    .color(Color::Muted),
                            ),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .items_start()
                            .child(Label::new(title).weight(FontWeight::SEMIBOLD).truncate())
                            .child(
                                Label::new(place)
                                    .size(LabelSize::Small)
                                    .color(if trouble.is_some() {
                                        Color::Error
                                    } else {
                                        Color::Muted
                                    })
                                    .truncate(),
                            ),
                    )
                    .child(
                        ui::Icon::new(IconName::ChevronDown)
                            .size(ui::IconSize::Small)
                            .color(Color::Muted),
                    ),
            );
        PopoverMenu::new("profile-menu")
            .full_width(true)
            .with_handle(self.nav.browsers.profile_menu.clone())
            .anchor(gpui::Anchor::TopLeft)
            .attach(gpui::Anchor::BottomLeft)
            .trigger_with_tooltip(card, Tooltip::text("Choose a profile's notebook"))
            .menu(move |window, cx| {
                let entries = view.upgrade()?.read(cx).profile_menu_entries(cx);
                Some(pointer_menu::build(entries, None, window, cx))
            })
    }

    /// The Find field: opens Find over the notebook, with its keys.
    pub(in crate::editor_host::stored) fn render_find_trigger(
        &self,
        tab_index: isize,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        ButtonLike::new("find")
            .full_width()
            .style(ButtonStyle::Outlined)
            .size(ButtonSize::Large)
            .height(px(36.).into())
            .tab_index(tab_index)
            .tooltip(Tooltip::for_action_title("Find a page or block", &FindPage))
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .px_1()
                    .debug_selector(|| "quire-find".into())
                    .child(
                        ui::Icon::new(IconName::MagnifyingGlass)
                            .size(ui::IconSize::Small)
                            .color(Color::Muted),
                    )
                    .child(Label::new("Find a thought…").color(Color::Muted).truncate())
                    .child(gpui::div().flex_1())
                    .child(KeyBinding::for_action(&FindPage, cx).size(px(10.))),
            )
            .on_click(cx.listener(|this, _, window, cx| {
                this.open_palette(crate::navigation::palette::PaletteMode::Find, window, cx)
            }))
    }

    /// The view list: one row per view, the current ones highlighted, Pages with its count.
    pub(in crate::editor_host::stored) fn render_nav(
        &self,
        order: &mut crate::tab_order::TabOrder,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let current = self.current_nav(cx);
        let count = self.nav.pages.len();
        let rows: Vec<_> = NavEntry::ALL
            .into_iter()
            .map(|entry| {
                let item = ListItem::new(entry.id())
                    .rounded()
                    .toggle_state(current.contains(&entry))
                    .start_slot(entry.icon().color(Color::Muted))
                    .child(
                        h_flex()
                            .h(px(37.))
                            .min_w_0()
                            .child(Label::new(entry.label()).truncate()),
                    )
                    .when(entry == NavEntry::Pages, |item| {
                        item.end_slot(
                            Label::new(count.to_string())
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    })
                    .when_some(entry.keys(), |item, keys| {
                        item.tooltip(move |_, cx| {
                            Tooltip::for_action(entry.label(), keys.as_ref(), cx)
                        })
                    });
                gpui::div()
                    .debug_selector(move || format!("quire-nav-{}", entry.id()))
                    .child(self.entry_stop(
                        ("nav-entry", entry as usize),
                        order.next(),
                        item,
                        move |this, _, window, cx| this.open_nav(entry, window, cx),
                        cx,
                    ))
            })
            .collect();
        v_flex().gap_px().children(rows)
    }

    /// Recent, folded until opened as the reference's disclosure is.
    pub(in crate::editor_host::stored) fn render_recent(
        &self,
        order: &mut crate::tab_order::TabOrder,
        cx: &mut gpui::Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if self.nav.workspace.recent.is_empty() {
            return None;
        }
        let open = self.nav.browsers.recent_open;
        let heading = Button::new("recent-heading", "RECENT")
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
            .tab_index(order.next())
            .on_click(cx.listener(|this, _, _, cx| {
                this.nav.browsers.recent_open = !this.nav.browsers.recent_open;
                cx.notify();
            }));
        let rows: Vec<_> = if open {
            self.nav
                .workspace
                .recent
                .iter()
                .take(quire_core::workspace::SHOWN_RECENT)
                .enumerate()
                .map(|(index, item)| {
                    let removing = item.clone();
                    let entry = order.next();
                    let remove = order.next();
                    let row = self
                        .destination_item(
                            ("recent", index),
                            item,
                            super::menu_views::Sidebar::Recent,
                            cx,
                        )
                        .end_slot(
                            IconButton::new(("remove-recent", index), IconName::Close)
                                .icon_size(ui::IconSize::XSmall)
                                .tab_index(remove)
                                .tooltip(Tooltip::text("Remove from recent"))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.nav.workspace.remove_recent(&removing);
                                    this.persist(cx);
                                    cx.notify();
                                })),
                        );
                    self.destination_stop(("recent-entry", index), entry, row, item, cx)
                })
                .collect()
        } else {
            Vec::new()
        };
        Some(
            v_flex()
                .flex_none()
                .max_h(px(260.))
                .debug_selector(|| "quire-recent".into())
                .child(
                    h_flex().mt_2().child(
                        gpui::div()
                            .debug_selector(|| "quire-recent-heading".into())
                            .child(heading),
                    ),
                )
                .children(rows),
        )
    }

    /// New page, Open a folder, the Roam import and export, and the local promise.
    pub(in crate::editor_host::stored) fn render_sidebar_footer(
        &self,
        order: &mut crate::tab_order::TabOrder,
        cx: &mut gpui::Context<Self>,
    ) -> impl IntoElement + use<> {
        let busy = self.exchange.read(cx).is_running();
        let footer_button = |id: &'static str, label: &'static str, icon: ui::Icon| {
            ButtonLike::new(id).full_width().child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .px_1()
                    .debug_selector(move || format!("quire-{id}"))
                    .child(icon.size(ui::IconSize::Small).color(Color::Muted))
                    .child(Label::new(label).size(LabelSize::Small)),
            )
        };
        let new_page = ButtonLike::new("new-page")
            .full_width()
            .tab_index(order.next())
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .px_1()
                    .debug_selector(|| "quire-new-page".into())
                    .child(ui::Icon::new(IconName::Plus).size(ui::IconSize::Small))
                    .child(Label::new("New page").weight(FontWeight::SEMIBOLD))
                    .child(gpui::div().flex_1())
                    .child(KeyBinding::for_action(&NewPage, cx).size(px(10.))),
            )
            .on_click(|_: &ClickEvent, window, cx| window.dispatch_action(Box::new(NewPage), cx));
        let open_folder = footer_button(
            "open-folder",
            "Open a folder",
            ui::Icon::new(IconName::FolderOpen),
        )
        .tab_index(order.next())
        .tooltip(Tooltip::text(
            "Choose a folder of Markdown notes as this profile's notebook",
        ))
        .on_click(|_, window, cx| window.dispatch_action(Box::new(OpenNotebookFolder), cx));
        let import = footer_button(
            "import-roam-data",
            "Import Roam data",
            ui::Icon::from_path(UPLOAD),
        )
        .tab_index(order.next())
        .disabled(busy)
        .on_click(cx.listener(|this, _, _, cx| this.import_roam_data(cx)));
        let export = footer_button(
            "export-roam-json",
            "Export Roam JSON",
            ui::Icon::new(IconName::Download),
        )
        .tab_index(order.next())
        .disabled(busy)
        .on_click(cx.listener(|this, _, window, cx| this.export_roam_json(window, cx)));
        v_flex()
            .flex_none()
            .child(gpui::div().mt_3().mb_2().child(new_page))
            .child(
                v_flex()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
                    .child(open_folder)
                    .child(import)
                    .child(export),
            )
            .child(
                h_flex()
                    .gap_1p5()
                    .px_2()
                    .pt_2()
                    .pb_4()
                    .child(
                        gpui::div()
                            .size(px(5.))
                            .rounded_full()
                            .bg(cx.theme().colors().text_accent),
                    )
                    .child(
                        Label::new("Local. Yours. Always.")
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                    ),
            )
    }
}
