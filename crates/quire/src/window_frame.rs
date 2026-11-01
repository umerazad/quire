//! The window frame Quire draws on Linux when the compositor offers none, as GNOME on Wayland
//! does: the window buttons on the sides the desktop's button layout names, a slim title bar for
//! windows without a header of their own, and Zed's shadow, border, and resize edges.

use gpui::{
    AnyElement, App, Div, IntoElement, SharedString, Stateful, Tiling, Window, prelude::*, px,
};
use theme::ClientDecorationsExt as _;
use ui::{ActiveTheme as _, Color, Label, LabelCommon as _, LabelSize, h_flex, v_flex};

/// The height of the title bar over windows without a header of their own.
const BAR_HEIGHT: gpui::Pixels = px(36.);

/// The window buttons for the left and right ends of a header. Both are empty where the system
/// draws the frame.
pub fn controls(window: &Window, cx: &App) -> [Option<AnyElement>; 2] {
    if crate::title_bar::client_frame(window).is_none() {
        return [None, None];
    }
    let layout = cx.button_layout();
    let close = || Box::new(crate::app_menus::CloseWindow) as Box<dyn gpui::Action>;
    [
        platform_title_bar::render_left_window_controls(layout, close(), window),
        platform_title_bar::render_right_window_controls(layout, close(), window),
    ]
}

/// Rounds the corners of `element` that meet the window's rounded corners. `edges` are the window
/// edges `element` does not touch, marked as tiled so their corners stay square.
pub fn round<E: gpui::Styled>(element: E, edges: Tiling, window: &Window) -> E {
    match crate::title_bar::client_frame(window) {
        Some(tiling) => element.rounded_client_corners(Tiling {
            top: tiling.top || edges.top,
            left: tiling.left || edges.left,
            right: tiling.right || edges.right,
            bottom: tiling.bottom || edges.bottom,
        }),
        None => element,
    }
}

/// A window with no header of its own, framed: where Quire draws the frame, a title bar with
/// `title` and the window buttons sits over `content`.
pub fn titled(
    title: impl Into<SharedString>,
    content: impl IntoElement,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let bar = crate::title_bar::client_frame(window).is_some().then(|| {
        let [left, right] = controls(window, cx);
        let colors = cx.theme().colors();
        let bar = h_flex()
            .id("quire-frame-bar")
            .debug_selector(|| "quire-frame-bar".into())
            .flex_none()
            .w_full()
            .h(BAR_HEIGHT)
            .bg(colors.title_bar_background)
            .border_b_1()
            .border_color(colors.border_variant)
            .child(h_flex().flex_1().children(left))
            .child(
                Label::new(title.into())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(h_flex().flex_1().justify_end().children(right));
        let bar = round(
            bar,
            Tiling {
                bottom: true,
                ..Tiling::default()
            },
            window,
        );
        crate::title_bar::drag_region("quire-frame-bar", bar, window, cx)
    });
    workspace::client_side_decorations(
        v_flex()
            .size_full()
            .children(bar)
            .child(gpui::div().flex_1().min_h_0().child(content)),
        window,
        cx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Blank;

    impl gpui::Render for Blank {
        fn render(&mut self, _: &mut Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
            gpui::div()
        }
    }

    #[gpui::test]
    fn windows_the_system_frames_get_no_quire_buttons(cx: &mut gpui::TestAppContext) {
        let window = cx.add_window(|_, _| Blank);
        window
            .update(cx, |_, window, cx| {
                assert!(crate::title_bar::client_frame(window).is_none());
                let [left, right] = controls(window, cx);
                assert!(left.is_none() && right.is_none(), "buttons drawn twice");
            })
            .unwrap();
    }
}
