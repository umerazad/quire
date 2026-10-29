use editor::Editor;
use gpui::{App, AppContext, Entity, KeyBinding, Window};
use search::{BufferSearchBar, buffer_search};
use workspace::ToolbarItemView;

pub fn new(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> Entity<BufferSearchBar> {
    let bar = cx.new(|cx| {
        let mut bar = BufferSearchBar::new(None, window, cx);
        bar.set_active_pane_item(Some(editor), window, cx);
        bar
    });
    editor.update(cx, |editor, _| vim::set_search_bar(editor, &bar));
    bar
}

pub fn bind_keys(cx: &mut App) {
    let (find, replace, next, previous) = if cfg!(target_os = "macos") {
        ("cmd-f", "cmd-alt-f", "cmd-g", "cmd-shift-g")
    } else {
        ("ctrl-f", "ctrl-h", "f3", "shift-f3")
    };
    cx.bind_keys([
        KeyBinding::new(find, buffer_search::Deploy::find(), Some("Editor")),
        KeyBinding::new(replace, buffer_search::Deploy::replace(), Some("Editor")),
        KeyBinding::new(next, search::SelectNextMatch, Some("Editor")),
        KeyBinding::new(previous, search::SelectPreviousMatch, Some("Editor")),
        KeyBinding::new("escape", buffer_search::Dismiss, Some("BufferSearchBar")),
        KeyBinding::new(
            "enter",
            search::SelectNextMatch,
            Some("BufferSearchBar > Editor"),
        ),
        KeyBinding::new(
            "shift-enter",
            search::SelectPreviousMatch,
            Some("BufferSearchBar > Editor"),
        ),
        KeyBinding::new(
            "shift-tab",
            editor::actions::Backtab,
            Some("BufferSearchBar > Editor"),
        ),
        KeyBinding::new(
            "enter",
            vim::SearchSubmit,
            Some("BufferSearchBar && !in_replace > Editor"),
        ),
        KeyBinding::new(
            "enter",
            search::ReplaceNext,
            Some("BufferSearchBar && in_replace > Editor"),
        ),
        KeyBinding::new(
            if cfg!(target_os = "macos") {
                "cmd-enter"
            } else {
                "ctrl-enter"
            },
            search::ReplaceAll,
            Some("BufferSearchBar && in_replace > Editor"),
        ),
    ]);
}
