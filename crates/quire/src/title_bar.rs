//! The reference app's hidden-inset title bar: on macOS the window content reaches the top edge
//! and the traffic lights sit over the top of the full-height sidebar, beside the workspace
//! header. The strip under the lights and the header move the window, and a double click there
//! does what the system setting says (zoom or minimize). Other platforms keep their native title
//! bar, as the reference app does, except where a Linux compositor offers none (GNOME on
//! Wayland): there Quire draws the frame, and the header moves the window and holds the window
//! buttons. The drag handling follows Zed's `platform_title_bar`.

use gpui::{
    App, Bounds, Div, InteractiveElement as _, IntoElement, Pixels, Stateful,
    StatefulInteractiveElement as _, Styled as _, Window, WindowBounds, WindowControlArea,
    WindowOptions, div, point, prelude::FluentBuilder as _, px, size,
};

/// The height of the workspace header and of the strip over the sidebar that holds the traffic
/// lights, as the reference's 57 px header measures in its window.
pub const HEIGHT: Pixels = px(56.);
/// Where the traffic lights sit, centred in the strip.
const TRAFFIC_LIGHTS: gpui::Point<Pixels> = point(px(21.), px(21.));
/// The traffic lights' width from the window's left edge, with a margin after them.
const LIGHTS_WIDTH: Pixels = px(84.);

/// The application ID every window carries: Wayland's `app_id` and the X11 `WM_CLASS` on Linux,
/// which desktops match to the installed `app.quire.native.desktop` entry for the dock icon and
/// window grouping, as Zed gives each window its app ID.
pub const APP_ID: &str = "app.quire.native";

/// What every Quire window starts from.
pub fn base_options() -> WindowOptions {
    WindowOptions {
        app_id: Some(APP_ID.into()),
        ..Default::default()
    }
}

/// Whether windows draw their own title bar.
pub fn custom() -> bool {
    cfg!(target_os = "macos")
}

/// The window's tiling when Quire draws the window frame itself: on Linux when the compositor
/// offers no server-side decorations, as GNOME on Wayland does. None where the system draws it.
pub fn client_frame(window: &Window) -> Option<gpui::Tiling> {
    match window.window_decorations() {
        gpui::Decorations::Client { tiling } if cfg!(target_os = "linux") => Some(tiling),
        _ => None,
    }
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
        ..base_options()
    }
}

/// Makes `element` move the window when dragged and zoom or minimize on a double click, as the
/// system setting says. Where Quire draws a Linux frame, a double click maximizes and a right
/// click opens the window menu, as Zed's title bar does. `key` names the drag state, one per
/// region in a window. Controls inside stop their own mouse downs with [`keep_clicks`].
pub fn drag_region(
    key: &'static str,
    element: Stateful<Div>,
    window: &mut Window,
    cx: &mut App,
) -> Stateful<Div> {
    let linux_frame = client_frame(window).is_some();
    if !custom() && !linux_frame {
        return element;
    }
    let moving = window.use_keyed_state(key, cx, |_, _| false);
    let (down, up, out) = (moving.clone(), moving.clone(), moving.clone());
    element
        .window_control_area(WindowControlArea::Drag)
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
        .on_click(move |event, window, _| {
            if event.click_count() != 2 {
                return;
            }
            if linux_frame {
                window.zoom_window();
            } else {
                window.titlebar_double_click();
            }
        })
        .when(linux_frame, |element| {
            element.on_mouse_down(gpui::MouseButton::Right, |event, window, _| {
                window.show_window_menu(event.position);
            })
        })
}

/// Keeps presses on the controls in `element` from moving the window around them.
pub fn keep_clicks(element: Div) -> Div {
    element.on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
}

/// The strip over the top of the sidebar where the traffic lights sit, as the reference's
/// hidden-inset window draws its sidebar under them. None where the platform draws the title
/// bar.
pub fn lights_strip(window: &mut Window, cx: &mut App) -> Option<impl IntoElement> {
    custom().then(|| {
        drag_region(
            "quire-lights-strip",
            div()
                .id("quire-title-bar")
                .debug_selector(|| "quire-title-bar".into())
                .flex_none()
                .w_full()
                .h(HEIGHT),
            window,
            cx,
        )
    })
}

/// Room a header at the window's top-left edge leaves for the traffic lights when the sidebar
/// is hidden.
pub fn lights_inset(window: &Window) -> Pixels {
    if custom() && !window.is_fullscreen() {
        LIGHTS_WIDTH
    } else {
        px(0.)
    }
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
        assert_eq!(notebook.app_id.as_deref(), Some(APP_ID));
    }

    #[test]
    fn windows_carry_the_app_id_the_linux_desktop_entry_is_named_after() {
        let entry = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("resources")
            .join(format!("{APP_ID}.desktop"));
        let text = std::fs::read_to_string(&entry).unwrap();
        for line in [format!("StartupWMClass={APP_ID}"), format!("Icon={APP_ID}")] {
            assert!(text.lines().any(|found| found == line), "{line} missing");
        }
        assert_eq!(base_options().app_id.as_deref(), Some(APP_ID));
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
            lights.x + px(60.) <= LIGHTS_WIDTH,
            "a header beside hidden sidebar overlaps the lights"
        );
    }
}
