use super::*;
use gpui::{AppContext as _, Focusable as _, TestAppContext};
use std::collections::BTreeSet;

fn init_app(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
}

fn help_window(cx: &mut TestAppContext) -> Option<gpui::WindowHandle<view::HelpWindow>> {
    cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|window| window.downcast::<view::HelpWindow>())
    })
}

#[gpui::test]
fn the_reference_lists_every_bound_action_from_the_keymap(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(|cx| {
        let rows = shortcuts(cx);
        let keymap = cx.key_bindings();
        let bound: BTreeSet<&str> = keymap
            .borrow()
            .bindings()
            .map(|binding| binding.action().name())
            .filter(|name| *name != "zed::NoAction" && *name != "zed::Unbind")
            .collect();
        let listed: BTreeSet<&str> = rows.iter().map(|row| row.name).collect();
        assert_eq!(listed, bound);
        assert!(bound.len() > 100, "{}", bound.len());
        let primary = if cfg!(target_os = "macos") {
            "Command-K"
        } else {
            "Ctrl-K"
        };
        let find = rows
            .iter()
            .find(|row| row.name == "quire_navigation::FindPage")
            .unwrap();
        assert_eq!(find.keys, primary);
        assert_eq!(
            find.action,
            "Finds a page by title, name, or date, or text inside pages"
        );
        assert_eq!(find.scope, "Notebook");
        assert!(rows.iter().any(|row| row.name == "quire_help::OpenHelp"
            && row.keys == "F1"
            && row.scope == "Everywhere"));
        assert!(
            rows.iter()
                .any(|row| row.name == "quire_preferences::OpenPreferences")
        );
    });
}

#[gpui::test]
fn a_new_binding_appears_in_the_reference_without_editing_help(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(|cx| {
        cx.bind_keys([KeyBinding::new("f9", OpenHelp, Some("QuireTaskList"))]);
        assert!(shortcuts(cx).iter().any(|row| row.keys == "F9"
            && row.scope == "Task list"
            && row.action == "Opens Help and shortcuts"));
    });
}

#[test]
fn undocumented_actions_read_as_words() {
    assert_eq!(humanize("vim::MoveToNextWord"), "Move to next word");
    assert_eq!(
        scope("Editor && vim_mode == normal && !QuireControl"),
        "Vim Normal"
    );
    assert_eq!(scope("SomethingElse"), "SomethingElse");
}

#[gpui::test]
fn f1_opens_help_once_and_search_filters_topics_and_shortcuts(cx: &mut TestAppContext) {
    init_app(cx);
    let page =
        cx.add_window(|window, cx| crate::editor_host::NotebookWindow::new("hello", window, cx));
    cx.simulate_keystrokes(page.into(), "f1");
    cx.run_until_parked();
    cx.simulate_keystrokes(page.into(), "f1");
    cx.run_until_parked();
    let count = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .filter(|window| window.downcast::<view::HelpWindow>().is_some())
            .count()
    });
    assert_eq!(count, 1);
    let help = help_window(cx).unwrap();
    help.update(cx, |view, _, cx| {
        assert_eq!(view.topics(cx).len(), content::SECTIONS.len());
        assert!(view.rows(cx).len() > 100);
    })
    .unwrap();
    cx.update_window(help.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    help.update(cx, |view, window, cx| {
        view.search()
            .update(cx, |editor, cx| editor.set_text("#[[waiting]]", window, cx));
        assert_eq!(view.topics(cx), ["tasks"]);
        view.search()
            .update(cx, |editor, cx| editor.set_text("redo", window, cx));
        assert!(!view.topics(cx).contains(&"links"));
        assert!(view.rows(cx).iter().any(|row| row.name == "editor::Redo"));
        view.search().update(cx, |editor, cx| {
            editor.set_text("no such help xyz", window, cx)
        });
        assert!(view.topics(cx).is_empty());
        assert!(view.rows(cx).is_empty());
    })
    .unwrap();
    help.update(cx, |view, window, cx| {
        window.focus(&view.search().focus_handle(cx), cx)
    })
    .unwrap();
    cx.simulate_keystrokes(help.into(), "escape");
    cx.run_until_parked();
    assert!(help_window(cx).is_none());
}

#[test]
fn topics_follow_the_reference_order_without_ai() {
    let ids: Vec<_> = content::SECTIONS.iter().map(|section| section.id).collect();
    assert_eq!(
        ids,
        [
            "start",
            "tasks",
            "blocks",
            "links",
            "vim",
            "templates",
            "data"
        ]
    );
}
