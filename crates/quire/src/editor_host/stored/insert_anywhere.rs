//! With Vim on, a key that starts Insert mode works wherever focus is, except in text fields and
//! menus: it goes to the page editor used last, or else the shown page's, as if typed there (the
//! reference `bindInsertAnywhere`).

use super::*;
use editor::Editor;
use settings::Settings as _;

/// Keys that start Insert mode from Normal mode.
const INSERT_KEYS: [&str; 9] = ["i", "a", "o", "I", "A", "O", "s", "S", "C"];

impl StoredWindow {
    /// Remembers which page editor took focus last, for keys typed outside any editor.
    pub(in crate::editor_host::stored) fn track_editor_focus(
        &mut self,
        view: &Entity<NotebookWindow>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let editors = {
            let view = view.read(cx);
            [view.source.clone(), view.second.clone()]
        };
        for editor in editors {
            let focus = editor.focus_handle(cx);
            let editor = editor.downgrade();
            cx.on_focus_in(&focus, window, move |this, _, _| {
                this.nav.last_editor = Some(editor.clone())
            })
            .detach();
        }
    }

    /// The page editors on screen: the main page's when the body shows it, and the side pane's.
    fn shown_editors(&self, cx: &App) -> Vec<Entity<Editor>> {
        let main = (self.nav.view.is_none() && self.nav.tags.shown.is_none())
            .then(|| self.editor.clone())
            .flatten();
        let side = self.nav.side.as_ref().and_then(|side| side.editor.clone());
        main.into_iter()
            .chain(side)
            .flat_map(|view| {
                let view = view.read(cx);
                [view.source.clone(), view.second.clone()]
            })
            .collect()
    }

    /// The page editor used last while it is on screen, or else the shown page's.
    pub(in crate::editor_host::stored) fn active_page_editor(
        &self,
        cx: &App,
    ) -> Option<Entity<Editor>> {
        let shown = self.shown_editors(cx);
        let last = self
            .nav
            .last_editor
            .as_ref()
            .and_then(|editor| editor.upgrade())
            .filter(|editor| shown.contains(editor));
        last.or_else(|| shown.into_iter().next())
    }

    pub(in crate::editor_host::stored) fn insert_anywhere(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let keystroke = &event.keystroke;
        let modifiers = keystroke.modifiers;
        let in_field = window.context_stack().iter().any(|context| {
            context.contains("Editor") || context.contains(crate::navigation::MENU_CONTEXT)
        });
        if !vim_mode_setting::VimModeSetting::get_global(cx).0
            || modifiers.control
            || modifiers.alt
            || modifiers.platform
            || modifiers.function
            || in_field
            || !keystroke
                .key_char
                .as_deref()
                .is_some_and(|key| INSERT_KEYS.contains(&key))
        {
            return;
        }
        let Some(target) = self.active_page_editor(cx) else {
            return;
        };
        cx.stop_propagation();
        let keystroke = keystroke.clone();
        let focus = target.focus_handle(cx);
        window.defer(cx, move |window, cx| {
            window.focus(&focus, cx);
            let inserting = window
                .context_stack()
                .iter()
                .any(|context| context.get("vim_mode").is_some_and(|mode| mode == "insert"));
            if !inserting {
                window.dispatch_keystroke(keystroke, cx);
            }
        });
    }
}
