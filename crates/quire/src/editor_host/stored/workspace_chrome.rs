//! The workspace right of the sidebar, laid out as the reference's (`App.tsx` workspace header,
//! page toolbar, and page status bar): a header with the sidebar toggle, history, the breadcrumb,
//! and the appearance, Settings, and second pane buttons; each page pane's toolbar with the
//! Write and Source switch, Tidy, Vim, the save icon, the details toggle, and the page actions
//! menu; and its status bar.

use super::super::page_views::tidy_blocked;
use super::*;
use crate::app_menus::{ShowNotebookFolder, TogglePageDetails, ToggleVim};
use crate::navigation::pointer_menu::{Entry, Item};
use gpui::{AnyElement, ClickEvent, Pixels, Point, SharedString};
use quire_core::markdown::plural;
use ui::{
    ButtonLike, Color, Disableable as _, Divider, Icon, IconButton, IconName, IconSize, LabelSize,
    ToggleButtonGroup, ToggleButtonGroupStyle, ToggleButtonWithIcon, Toggleable as _, Tooltip,
    h_flex,
};

/// The page actions menu's name, as the reference labels its button.
pub(in crate::editor_host::stored) const PAGE_ACTIONS: &str = "Page actions";

const SUN: &str = "icons/quire/chrome/sun.svg";
const MOON: &str = "icons/quire/chrome/moon.svg";

impl StoredWindow {
    /// The notebook's name at the start of the breadcrumb: its folder's name.
    fn notebook_name(&self) -> SharedString {
        self.location
            .as_ref()
            .and_then(|location| location.root().file_name())
            .map_or_else(
                || "Notebook".into(),
                |name| name.to_string_lossy().into_owned().into(),
            )
    }

    /// The header over the workspace: it moves the window, and leaves room for the traffic
    /// lights while the sidebar is hidden.
    pub(in crate::editor_host::stored) fn render_workspace_header(
        &self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let hidden = self.nav.workspace.chrome.sidebar_hidden;
        let inset = if hidden {
            crate::title_bar::lights_inset(window)
        } else {
            gpui::px(0.)
        };
        let shown = self
            .nav
            .workspace
            .active
            .as_ref()
            .map(|active| self.destination_label(active, cx));
        let dark = !cx.theme().appearance().is_light();
        let appearance = if dark {
            "Switch to light appearance"
        } else {
            "Switch to dark appearance"
        };
        let side_open = self.nav.side.is_some();
        let [left_buttons, right_buttons] = crate::window_frame::controls(window, cx);
        let left = h_flex()
            .gap_1()
            .min_w_0()
            .children(left_buttons)
            .child(self.render_sidebar_toggle())
            .child(Divider::vertical().h_4().mx_1())
            .child(
                IconButton::new("back", IconName::ArrowLeft)
                    .disabled(!self.nav.history.can_go_back())
                    .tooltip(Tooltip::text("Back"))
                    .on_click(cx.listener(|this, _, window, cx| this.go(true, window, cx))),
            )
            .child(
                IconButton::new("forward", IconName::ArrowRight)
                    .disabled(!self.nav.history.can_go_forward())
                    .tooltip(Tooltip::text("Forward"))
                    .on_click(cx.listener(|this, _, window, cx| this.go(false, window, cx))),
            )
            .child(
                h_flex()
                    .debug_selector(|| "quire-breadcrumb".into())
                    .pl_2()
                    .gap_1()
                    .min_w_0()
                    .child(
                        Label::new(self.notebook_name())
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .single_line(),
                    )
                    .when_some(shown, |crumbs, shown| {
                        crumbs
                            .child(
                                Icon::new(IconName::ChevronRight)
                                    .size(IconSize::XSmall)
                                    .color(Color::Muted),
                            )
                            .child(Label::new(shown).size(LabelSize::Small).truncate())
                    }),
            );
        let right = h_flex()
            .gap_1()
            .flex_none()
            .child(
                ButtonLike::new("appearance")
                    .aria_label(appearance)
                    .child(Icon::from_path(if dark { SUN } else { MOON }))
                    .tooltip(Tooltip::text(appearance))
                    .on_click(|_, _, cx| crate::preferences::toggle_appearance(cx)),
            )
            .child(
                IconButton::new("settings", IconName::Settings)
                    .tooltip(Tooltip::text("Settings"))
                    .on_click(|_, _, cx| crate::preferences::view::open(cx)),
            )
            .child(
                IconButton::new("side-pane", IconName::ThreadsSidebarRightOpen)
                    .toggle_state(side_open)
                    .tooltip(Tooltip::text("Toggle second pane"))
                    .on_click(cx.listener(|this, _, window, cx| this.toggle_side(window, cx))),
            )
            .children(right_buttons);
        let edges = gpui::Tiling {
            left: !hidden,
            bottom: true,
            ..Default::default()
        };
        crate::title_bar::drag_region(
            "quire-workspace-header",
            crate::window_frame::round(h_flex(), edges, window)
                .id("workspace-header")
                .debug_selector(|| "quire-workspace-header".into())
                .flex_none()
                .w_full()
                .h(crate::title_bar::HEIGHT)
                .pl(inset + gpui::px(12.))
                .pr_3()
                .gap_2()
                .justify_between()
                .border_b_1()
                .border_color(cx.theme().colors().border_variant)
                .bg(cx.theme().colors().editor_background)
                .child(crate::title_bar::keep_clicks(left))
                .child(crate::title_bar::keep_clicks(right)),
            window,
            cx,
        )
    }

    /// The Write and Source switch of a pane's page editor.
    fn view_switch(
        id: &'static str,
        editor: &Entity<NotebookWindow>,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let writing = editor.read(cx).writing(cx);
        let choose = |write: bool| {
            let editor = editor.downgrade();
            move |_: &ClickEvent, window: &mut Window, cx: &mut App| {
                editor
                    .update(cx, |editor, cx| editor.set_writing(write, window, cx))
                    .ok();
            }
        };
        h_flex()
            .debug_selector(move || format!("quire-{id}"))
            .child(
                ToggleButtonGroup::single_row(
                    id,
                    [
                        ToggleButtonWithIcon::new("Write", IconName::Pencil, choose(true))
                            .tooltip(Tooltip::text("Write: the page as it reads")),
                        ToggleButtonWithIcon::new("Source", IconName::Code, choose(false))
                            .tooltip(Tooltip::text("Source: the page's Markdown")),
                    ],
                )
                .style(ToggleButtonGroupStyle::Outlined)
                .auto_width()
                .selected_index(if writing { 0 } else { 1 }),
            )
    }

    /// The main pane's page toolbar: Write and Source at the start, then Tidy on a daily page,
    /// Vim, the save icon, the details toggle, and the page actions.
    pub(in crate::editor_host::stored) fn render_page_toolbar(
        &self,
        editor: &Entity<NotebookWindow>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let page = self.page.read(cx);
        let daily = self
            .nav
            .title(page.id())
            .is_some_and(|title| self.daily_date_of(title).is_some());
        let vim = crate::preferences::Preferences::current(cx).vim;
        let tools = h_flex()
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
            .children(self.render_save_status(cx))
            .child(
                IconButton::new("page-details", IconName::Link)
                    .toggle_state(self.chrome.details)
                    .tooltip(Tooltip::text("Page outline and backlinks"))
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(TogglePageDetails), cx)
                    }),
            )
            .child(
                gpui::div()
                    .debug_selector(|| "quire-page-actions".into())
                    .child(
                        IconButton::new("page-actions", IconName::Ellipsis)
                            .aria_label(PAGE_ACTIONS)
                            .tooltip(Tooltip::text(PAGE_ACTIONS))
                            .on_click(cx.listener(|this, event: &ClickEvent, window, cx| {
                                this.deploy_page_actions(event.position(), window, cx)
                            })),
                    ),
            );
        Self::toolbar(Self::view_switch("view-switch", editor, cx), tools)
    }

    /// The side pane's toolbar, as rt's second page pane has one: its Write and Source switch,
    /// its page's save icon, its details toggle, and Close.
    pub(in crate::editor_host::stored) fn render_side_toolbar(
        &self,
        page: &Entity<OpenPage>,
        editor: &Entity<NotebookWindow>,
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let tools = h_flex()
            .gap_1()
            .children(Self::save_status(page, "quire-side-save-indicator", cx))
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
            .child(
                IconButton::new("close-side-pane", IconName::Close)
                    .tooltip(Tooltip::text("Close second pane"))
                    .on_click(cx.listener(|this, _, window, cx| this.close_side(window, cx))),
            );
        Self::toolbar(Self::view_switch("side-view-switch", editor, cx), tools)
    }

    /// Padded as rt's `.page-toolbar` (20 px top, 29 px sides, 8 px bottom).
    fn toolbar(switch: impl IntoElement, tools: gpui::Div) -> AnyElement {
        h_flex()
            .flex_none()
            .justify_between()
            .gap_2()
            .px(gpui::px(29.))
            .pt(gpui::px(20.))
            .pb(gpui::px(8.))
            .child(switch)
            .child(tools)
            .into_any_element()
    }

    /// A pane's status bar: the page's path, its inline tags, the Vim mode, and its words, as
    /// the reference's page status bar lists them.
    pub(in crate::editor_host::stored) fn render_status_bar(
        &self,
        page: &Entity<OpenPage>,
        editor: &Entity<NotebookWindow>,
        cx: &App,
    ) -> AnyElement {
        let path = page.read(cx).path(cx);
        let words = self
            .page_facts(page, cx)
            .map(|facts| format!("{} · Markdown · UTF-8", plural(facts.words, "word")));
        let editor = editor.read(cx);
        let tags = editor.tag_summary(cx);
        h_flex()
            .id("page-status")
            .debug_selector(|| "quire-page-status".into())
            .flex_none()
            .h(gpui::px(36.))
            .px_4()
            .gap_3()
            .border_t_1()
            .border_color(cx.theme().colors().border_variant)
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_3()
                    .when_some(path, |row, path| {
                        row.child(
                            h_flex()
                                .gap_1()
                                .min_w_0()
                                .child(
                                    Icon::new(IconName::FileTextOutlined)
                                        .size(IconSize::XSmall)
                                        .color(Color::Muted),
                                )
                                .child(
                                    Label::new(path)
                                        .size(LabelSize::Small)
                                        .color(Color::Muted)
                                        .truncate(),
                                ),
                        )
                    })
                    .when(!tags.is_empty(), |row| {
                        row.child(
                            Label::new(tags)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                    }),
            )
            .child(editor.mode_indicator())
            .when_some(words, |row, words| {
                row.child(
                    Label::new(words)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .single_line(),
                )
            })
            .into_any_element()
    }

    /// The page actions, grouped as the reference's menu groups them, then what Quire adds:
    /// saving, copying the page, retrying a failed load, and the notebook's backups and sync.
    fn page_actions(&self, view: gpui::WeakEntity<Self>, cx: &App) -> Vec<Entry> {
        let page = self.page.read(cx);
        let editable = page.persistence().is_some() && self.shown_page(cx).is_some();
        let loaded = page.loaded().is_some();
        let retry = page.can_retry();
        let run = |label: &'static str,
                   act: fn(&mut Self, &mut Window, &mut gpui::Context<Self>)| {
            let view = view.clone();
            Item::run(label, move |window, cx| {
                view.update(cx, |this, cx| act(this, window, cx)).ok();
            })
        };
        let block = |label: &'static str, action: Box<dyn gpui::Action>| {
            Item::boxed(label, action).disabled(!editable).done()
        };
        [
            vec![
                block(
                    "Copy block reference",
                    Box::new(crate::page_menu::CopyBlockReference),
                ),
                block(
                    "Copy block embed",
                    Box::new(crate::page_menu::CopyBlockEmbed),
                ),
                block(
                    "Move block to page",
                    Box::new(crate::block_move::MoveBlockToPage),
                ),
                run("Attach…", |this, window, cx| {
                    this.attach_files(window, cx)
                })
                .disabled(!editable)
                .done(),
                run("History…", |this, window, cx| {
                    this.show_history(window, cx)
                })
                .disabled(!editable)
                .done(),
                Entry::Separator,
                Item::action("Export as Markdown…", crate::exchange::ExportPageMarkdown)
                    .disabled(self.shown_page(cx).is_none())
                    .done(),
            ],
            Self::document_export_items(cx).into(),
            vec![
                Entry::Separator,
                run("Rename…", |this, window, cx| {
                    this.rename_shown_page(window, cx)
                })
                .disabled(!editable)
                .done(),
                run("Move…", |this, window, cx| {
                    this.move_shown_page(window, cx)
                })
                .disabled(!editable)
                .done(),
                run("Delete page…", |this, window, cx| {
                    this.delete_shown_page(window, cx)
                })
                .disabled(!editable)
                .done(),
                Entry::Separator,
                Item::action("Save locally", crate::editor_command::SavePage)
                    .disabled(page.persistence().is_none())
                    .done(),
                run("Copy Markdown", |this, _, cx| this.copy_markdown(cx))
                    .disabled(!loaded)
                    .done(),
            ],
            retry
                .then(|| {
                    run("Retry page", |this, _, cx| {
                        this.page.update(cx, |page, cx| page.retry(cx))
                    })
                    .done()
                })
                .into_iter()
                .collect(),
            vec![
                Entry::Separator,
                Item::action("Show notebook folder", ShowNotebookFolder).done(),
                Item::action("Backups…", crate::backups::ShowBackups).done(),
                Item::action("Sync…", crate::sync_status::ShowSync).done(),
            ],
        ]
        .into_iter()
        .flatten()
        .collect()
    }

    /// Opens the page actions under the "…" button. Items that act at the caret dispatch from
    /// the page editor.
    pub(in crate::editor_host::stored) fn deploy_page_actions(
        &mut self,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let entries = self.page_actions(cx.entity().downgrade(), cx);
        let context = self
            .editor
            .as_ref()
            .map(|editor| editor.read(cx).source.focus_handle(cx));
        self.deploy_menu(PAGE_ACTIONS.into(), entries, context, position, window, cx);
    }
}
