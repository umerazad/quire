//! Keyboard focus order for forms such as Settings and Help: Tab and Shift-Tab walk the controls
//! in reading order. GPUI orders tab stops by index before insertion, and a toggle button group
//! numbers its buttons from its own index, so each form hands out indices in reading order.

use editor::Editor;
use gpui::{App, Context, Entity, Focusable as _, KeyBinding, Window, prelude::*};
use theme::ActiveTheme as _;

/// Tab indices for one form or tab group, in reading order.
#[derive(Default)]
pub struct TabOrder(isize);

impl TabOrder {
    pub fn next(&mut self) -> isize {
        let index = self.0;
        self.0 += 1;
        index
    }

    /// For a toggle button group, which takes as many indices as it has buttons.
    pub fn counter(&mut self) -> &mut isize {
        &mut self.0
    }

    /// A container whose own controls are numbered from zero, at the next index.
    pub fn group(&mut self) -> gpui::Div {
        gpui::div().tab_group().tab_index(self.next())
    }
}

/// A bordered single-line field that Tab reaches at the next index.
pub fn field(
    editor: &Entity<Editor>,
    order: &mut TabOrder,
    window: &Window,
    cx: &App,
) -> gpui::Div {
    let focus = editor
        .focus_handle(cx)
        .tab_index(order.next())
        .tab_stop(true);
    let colors = cx.theme().colors();
    let border = if focus.contains_focused(window, cx) {
        colors.border_focused
    } else {
        colors.border
    };
    gpui::div()
        .track_focus(&focus)
        .px_2()
        .py_1()
        .border_1()
        .border_color(border)
        .rounded_sm()
        .child(editor.clone())
}

/// Binds Tab and Shift-Tab to move focus within the windows with these key contexts.
pub fn bind_keys(contexts: &[&str], cx: &mut App) {
    cx.bind_keys(contexts.iter().flat_map(|context| {
        [
            KeyBinding::new("tab", menu::SelectNext, Some(context)),
            KeyBinding::new("shift-tab", menu::SelectPrevious, Some(context)),
        ]
    }));
}

/// Makes `element` move focus on Tab and Shift-Tab.
pub fn handle<E: InteractiveElement, T: 'static>(element: E, cx: &mut Context<T>) -> E {
    element
        .on_action(cx.listener(|_, _: &menu::SelectNext, window, cx| window.focus_next(cx)))
        .on_action(cx.listener(|_, _: &menu::SelectPrevious, window, cx| window.focus_prev(cx)))
}

/// Presses each space-separated key in `keys` as the platform would, key down then key up,
/// drawing first so that focus and key handlers are current. Focused buttons click on key up.
#[cfg(test)]
pub fn press(window: gpui::AnyWindowHandle, keys: &str, cx: &mut gpui::TestAppContext) {
    for key in keys.split_whitespace() {
        cx.update_window(window, |_, window, cx| {
            window.draw(cx).clear(cx);
            let keystroke = gpui::Keystroke::parse_with_os(key, cx.platform_info().os).unwrap();
            window.dispatch_event(
                gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                    keystroke: keystroke.clone(),
                    is_held: false,
                    prefer_character_input: false,
                }),
                cx,
            );
            window.dispatch_event(
                gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
                cx,
            );
        })
        .unwrap();
        cx.run_until_parked();
    }
}
