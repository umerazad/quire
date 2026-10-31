//! The README rendering showcase: the reference sample page in a buffer of its own. It belongs to
//! no notebook, so it never reaches a notebook's storage, index, queries, backups, or exports.
//! Edits last until Quire quits, and Reset sample restores the original as one undo step.

use crate::editor_host::NotebookWindow;
use gpui::{
    App, AppContext as _, Context, Entity, Global, Window, WindowOptions, actions, prelude::*, px,
};
use theme::ActiveTheme as _;
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, IconButton, IconName, IconSize, Label,
    LabelCommon as _, LabelSize, Tooltip, h_flex, v_flex,
};

pub const SAMPLE: &str = include_str!("showcase/README.md");

actions!(
    quire_showcase,
    [
        /// Opens README.md, the rendering showcase, beside your notebooks.
        OpenShowcase,
        /// Restores the README sample to its original text.
        ResetShowcase,
    ]
);

/// The sample's page for this run of Quire, shared by every showcase window.
struct Sample(Entity<crate::page_analysis::PageAnalysis>);
impl Global for Sample {}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenShowcase, cx| open(cx));
}

pub(crate) fn sample(cx: &mut App) -> Entity<crate::page_analysis::PageAnalysis> {
    if let Some(Sample(page)) = cx.try_global::<Sample>() {
        return page.clone();
    }
    let buffer = crate::markdown::buffer(SAMPLE, cx);
    let page = cx.new(|cx| crate::page_analysis::PageAnalysis::new(buffer, cx));
    cx.set_global(Sample(page.clone()));
    page
}

/// Replaces the sample's text with the original, as one undo step.
pub fn reset(cx: &mut App) {
    let buffer = sample(cx).read(cx).source.clone();
    buffer.update(cx, |buffer, cx| {
        buffer.finalize_last_transaction();
        buffer.edit([(0..buffer.len(), SAMPLE)], None, cx);
        buffer.finalize_last_transaction();
    });
}

pub fn open(cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ShowcaseWindow>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let page = sample(cx);
    let options = WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(px(1100.), px(800.)),
            cx,
        ))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_window_title("README.md");
        cx.new(|cx| ShowcaseWindow::new(page, false, window, cx))
    });
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Cannot open README.md: {error}. Check desktop access, then retry."),
            cx,
        );
    }
}

/// Asks the window holding a showcase pane to close it.
pub struct CloseShowcase;

/// README.md in a window of its own, or, with `pane`, beside a notebook's pages as the reference
/// shows it.
pub struct ShowcaseWindow {
    pub notebook: Entity<NotebookWindow>,
    pane: bool,
}

impl gpui::EventEmitter<CloseShowcase> for ShowcaseWindow {}

impl ShowcaseWindow {
    pub fn new(
        page: Entity<crate::page_analysis::PageAnalysis>,
        pane: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let notebook = cx.new(|cx| NotebookWindow::for_page(page, None, window, cx));
        notebook.update(cx, |view, cx| view.show_written(window, cx));
        Self { notebook, pane }
    }
}

impl Render for ShowcaseWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(|_: &ResetShowcase, _, cx| reset(cx))
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_3()
                    .justify_between()
                    .child(Label::new("README.md"))
                    .child(
                        Label::new(
                            "Rendering showcase · Sample edits stay here and reset when the app restarts.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("reset-sample", "Reset sample")
                                    .on_click(|_, _, cx| reset(cx)),
                            )
                            .when(self.pane, |tools| {
                                tools.child(
                                    IconButton::new("close-showcase", IconName::Close)
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text("Close README.md"))
                                        .on_click(
                                            cx.listener(|_, _, _, cx| cx.emit(CloseShowcase)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(gpui::div().flex_1().min_h_0().child(self.notebook.clone()))
            .child(
                h_flex().px_3().py_1().child(
                    Label::new("Sample only · Your notebooks are separate")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
    }
}

#[cfg(test)]
mod tests;
