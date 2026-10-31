//! Help beside the notebook's pages, as the reference app shows it: F1 opens it with its search
//! focused, and Escape or Close puts focus back where it was.

use super::*;
use crate::help::{
    OpenHelp,
    view::{CloseHelp, HelpWindow},
};

impl StoredWindow {
    pub(super) fn register_help(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        div.on_action(cx.listener(|this, _: &OpenHelp, window, cx| this.open_help(window, cx)))
    }

    /// Opens Help beside the pages, or moves focus into it when it is already open.
    pub(super) fn open_help(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if let Some(help) = &self.help {
            let search = help.read(cx).search_focus(cx);
            window.focus(&search, cx);
            return;
        }
        let returning = window.focused(cx);
        let help = cx.new(|cx| HelpWindow::new(true, window, cx));
        cx.subscribe_in(&help, window, move |this, _, _: &CloseHelp, window, cx| {
            this.help = None;
            match &returning {
                Some(focus) => window.focus(focus, cx),
                None => window.focus_next(cx),
            }
            cx.notify();
        })
        .detach();
        self.help = Some(help);
        cx.notify();
    }

    pub(super) fn help_pane(&self, cx: &gpui::Context<Self>) -> Option<gpui::AnyElement> {
        let help = self.help.clone()?;
        Some(
            gpui::div()
                .tab_group()
                .w(gpui::px(440.))
                .flex_none()
                .min_h_0()
                .border_l_1()
                .border_color(cx.theme().colors().border)
                .child(help)
                .into_any_element(),
        )
    }
}
