use super::*;
use gpui::TestAppContext;

fn init_without_vim(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init(cx).expect("editor initialization");
        let result = settings::SettingsStore::update_global(cx, |store, cx| {
            store.set_user_settings(
                r#"{"vim_mode":false,"session":{"restore_unsaved_buffers":false}}"#,
                cx,
            )
        });
        assert!(!matches!(
            result.parse_status,
            settings::ParseStatus::Failed { .. }
        ));
    });
}

fn find_shortcut() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-f"
    } else {
        "ctrl-f"
    }
}

fn selection(editor: &Entity<Editor>, cx: &mut App) -> std::ops::Range<usize> {
    editor.update(cx, |editor, cx| {
        let selected = editor
            .selections
            .newest::<editor::MultiBufferOffset>(&editor.display_snapshot(cx));
        selected.start.0..selected.end.0
    })
}

#[gpui::test]
fn keyboard_find_selects_a_result_without_inserting_the_query_into_the_note(
    cx: &mut TestAppContext,
) {
    init_without_vim(cx);
    let text = "one target two target\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    cx.simulate_keystrokes(window.into(), find_shortcut());
    cx.simulate_keystrokes(window.into(), "t a r g e t");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source_search.read(cx).query(cx), "target");
            assert!(view.source_search.read(cx).has_active_match());
            assert_eq!(selection(&view.source, cx), 4..10);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source_search.read(cx).query(cx), "target");
            assert_eq!(selection(&view.source, cx), 15..21)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "shift-enter escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), text);
            assert_eq!(selection(&view.source, cx), 4..10);
            assert!(view.source.focus_handle(cx).is_focused(window));
            assert!(view.source_search.read(cx).is_dismissed());
        })
        .unwrap();
}

#[gpui::test]
fn keyboard_find_keeps_vim_cursor_semantics(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "one target two target\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    cx.simulate_keystrokes(window.into(), find_shortcut());
    cx.simulate_keystrokes(window.into(), "t a r g e t");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(selection(&view.source, cx), 4..4)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "enter escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), text);
            assert_eq!(selection(&view.source, cx), 15..15);
            assert!(view.source.focus_handle(cx).is_focused(window));
        })
        .unwrap();
}

#[gpui::test]
fn replace_all_updates_both_views_and_undoes_from_the_other_pane(cx: &mut TestAppContext) {
    init_without_vim(cx);
    let original = "one target two target\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
    cx.simulate_keystrokes(window.into(), find_shortcut());
    cx.simulate_keystrokes(window.into(), "t a r g e t");
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-alt-f"
        } else {
            "ctrl-h"
        },
    );
    cx.simulate_keystrokes(window.into(), "c h a n g e d");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source_search.read(cx).query(cx), "target");
            view.source_search
                .update(cx, |bar, cx| assert_eq!(bar.replacement(cx), "changed"));
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-enter"
        } else {
            "ctrl-enter"
        },
    );
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "one changed two changed\n");
            assert_eq!(view.second.read(cx).text(cx), "one changed two changed\n");
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-z"
        } else {
            "ctrl-z"
        },
    );
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), original);
            assert_eq!(view.second.read(cx).text(cx), original);
        })
        .unwrap();
}

#[gpui::test]
fn each_pane_keeps_its_own_search_target_and_query(cx: &mut TestAppContext) {
    init_without_vim(cx);
    let window =
        cx.add_window(|window, cx| NotebookWindow::new("one target two target\n", window, cx));
    cx.simulate_keystrokes(window.into(), find_shortcut());
    cx.simulate_keystrokes(window.into(), "t a r g e t");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.second.focus_handle(cx), cx)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), find_shortcut());
    cx.simulate_keystrokes(window.into(), "t w o");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source_search.read(cx).query(cx), "target");
            assert_eq!(view.second_search.read(cx).query(cx), "two");
            assert_eq!(selection(&view.source, cx), 4..10);
            assert_eq!(selection(&view.second, cx), 11..14);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    cx.simulate_keystrokes(window.into(), "t h r e e");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "one target three target\n");
            assert!(!view.second_search.read(cx).has_active_match());
            assert!(view.source_search.read(cx).has_active_match());
        })
        .unwrap();
}

#[gpui::test]
fn replacement_fields_cycle_by_keyboard_and_enter_replaces_one_match(cx: &mut TestAppContext) {
    init_without_vim(cx);
    let window =
        cx.add_window(|window, cx| NotebookWindow::new("one target two target\n", window, cx));
    cx.simulate_keystrokes(window.into(), find_shortcut());
    cx.simulate_keystrokes(window.into(), "t a r g e t");
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-alt-f"
        } else {
            "ctrl-h"
        },
    );
    cx.simulate_keystrokes(window.into(), "c h a n g e d shift-tab");
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        },
    );
    cx.simulate_keystrokes(window.into(), "t w o");
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "tab enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "one target changed target\n");
            assert_eq!(view.second.read(cx).text(cx), "one target changed target\n");
            assert_eq!(view.source_search.read(cx).query(cx), "two");
            assert!(!view.source_search.read(cx).has_active_match());
        })
        .unwrap();
}
