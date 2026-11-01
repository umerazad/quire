//! The reference app's hidden-inset title bar: on macOS the window content reaches the top edge,
//! the traffic lights sit inset in a strip that moves the window, and a double click there does
//! what the system setting says (zoom or minimize). Other platforms keep their native title bar,
//! as the reference app does. The strip follows Zed's `platform_title_bar`.

use gpui::{
    App, Bounds, InteractiveElement as _, IntoElement, ParentElement as _, Pixels, SharedString,
    StatefulInteractiveElement as _, Styled as _, Window, WindowBounds, WindowControlArea,
    WindowOptions, div, point, px, size,
};
use theme::ActiveTheme as _;
use ui::{Color, Label, LabelCommon as _, LabelSize};

/// The height of the strip that holds the traffic lights.
pub const HEIGHT: Pixels = px(40.);
/// Where the traffic lights sit, centred in the strip.
const TRAFFIC_LIGHTS: gpui::Point<Pixels> = point(px(16.), px(13.));
/// Room left of the title for the traffic lights.
const TRAFFIC_LIGHT_PADDING: Pixels = px(80.);

/// Whether windows draw their own title bar.
pub fn custom() -> bool {
    cfg!(target_os = "macos")
}

/// A notebook window's first size, as the reference opens its window.
pub const NOTEBOOK_SIZE: gpui::Size<Pixels> = size(px(1440.), px(960.));
/// The smallest a notebook or README window gets, as the reference allows: the sidebar, a page,
/// and the toolbar still fit.
pub const MIN_SIZE: gpui::Size<Pixels> = size(px(850.), px(580.));

/// Options for a centred window of `window_size`, no smaller than [`MIN_SIZE`], with the
/// hidden-inset title bar. The size shrinks to fit a smaller screen.
pub fn options(window_size: gpui::Size<Pixels>, cx: &App) -> WindowOptions {
    let fitting = cx.primary_display().map_or(window_size, |display| {
        let screen = display.bounds().size;
        size(
            window_size.width.min(screen.width).max(MIN_SIZE.width),
            window_size.height.min(screen.height).max(MIN_SIZE.height),
        )
    });
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, fitting, cx))),
        window_min_size: Some(MIN_SIZE),
        titlebar: Some(gpui::TitlebarOptions {
            title: None,
            appears_transparent: custom(),
            traffic_light_position: custom().then_some(TRAFFIC_LIGHTS),
        }),
        ..Default::default()
    }
}

/// The title of a notebook window: the notebook's folder name.
pub fn notebook_title(notebook: Option<&quire_store::profiles::Notebook>) -> SharedString {
    notebook
        .and_then(|notebook| notebook.root().file_name())
        .map_or_else(
            || "Quire".into(),
            |name| format!("{} · Quire", name.to_string_lossy()).into(),
        )
}

/// The strip at the top of a window: it moves the window when dragged and shows `title`. None
/// where the platform draws the title bar.
pub fn render(title: SharedString, window: &mut Window, cx: &mut App) -> Option<impl IntoElement> {
    if !custom() {
        return None;
    }
    let moving = window.use_keyed_state("quire-title-bar-moving", cx, |_, _| false);
    let (down, up, out) = (moving.clone(), moving.clone(), moving.clone());
    let inset = if window.is_fullscreen() {
        px(8.)
    } else {
        TRAFFIC_LIGHT_PADDING
    };
    Some(
        div()
            .id("quire-title-bar")
            .debug_selector(|| "quire-title-bar".into())
            .window_control_area(WindowControlArea::Drag)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .w_full()
            .h(HEIGHT)
            .pl(inset)
            .pr(px(8.))
            .border_b_1()
            .border_color(cx.theme().colors().border_variant)
            .bg(cx.theme().colors().title_bar_background)
            .on_mouse_down(gpui::MouseButton::Left, move |_, _, cx| {
                down.write(cx, true);
            })
            .on_mouse_up(gpui::MouseButton::Left, move |_, _, cx| {
                up.write(cx, false);
            })
            .on_mouse_down_out(move |_, _, cx| out.write(cx, false))
            .on_mouse_move(move |_, window, cx| {
                if *moving.read(cx) {
                    moving.write(cx, false);
                    window.start_window_move();
                }
            })
            .on_click(|event, window, _| {
                if event.click_count() == 2 {
                    window.titlebar_double_click();
                }
            })
            .child(
                Label::new(title)
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            ),
    )
}

#[cfg(test)]
mod size_tests {
    use super::*;

    #[gpui::test]
    fn notebook_windows_open_at_the_reference_size_and_keep_a_minimum(
        cx: &mut gpui::TestAppContext,
    ) {
        let notebook = cx.update(|cx| options(NOTEBOOK_SIZE, cx));
        assert_eq!(notebook.window_min_size, Some(size(px(850.), px(580.))));
        let Some(WindowBounds::Windowed(bounds)) = notebook.window_bounds else {
            panic!("notebook windows open windowed");
        };
        assert!(bounds.size.width <= px(1440.) && bounds.size.height <= px(960.));
        assert!(bounds.size.width >= MIN_SIZE.width && bounds.size.height >= MIN_SIZE.height);
        let tiny = cx.update(|cx| options(size(px(200.), px(100.)), cx));
        let Some(WindowBounds::Windowed(bounds)) = tiny.window_bounds else {
            panic!("notebook windows open windowed");
        };
        assert_eq!(bounds.size, MIN_SIZE, "no window opens below the minimum");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn notebook_windows_hide_the_system_title_bar_and_inset_the_traffic_lights(
        cx: &mut TestAppContext,
    ) {
        let options = cx.update(|cx| options(NOTEBOOK_SIZE, cx));
        let titlebar = options.titlebar.unwrap();
        assert!(titlebar.appears_transparent);
        let lights = titlebar.traffic_light_position.unwrap();
        assert!(
            lights.y * 2. + px(14.) <= HEIGHT + px(1.),
            "lights leave the strip"
        );
        assert!(
            lights.x + px(60.) <= TRAFFIC_LIGHT_PADDING,
            "the title overlaps the lights"
        );
    }
}
