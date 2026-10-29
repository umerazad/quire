use super::*;
use gpui::{EntityInputHandler, TestAppContext};

#[gpui::test]
fn composition_uses_utf16_ranges_and_shared_undo(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    super::search_tests::set_vim_mode(cx, false);
    let original = "a🙂b مرحبا\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                editor.replace_and_mark_text_in_range(Some(1..3), "に", Some(1..1), window, cx);
                assert_eq!(editor.marked_text_range(window, cx), Some(1..2));
                editor.replace_and_mark_text_in_range(None, "日本", Some(2..2), window, cx);
                assert_eq!(editor.marked_text_range(window, cx), Some(1..3));
                assert_eq!(
                    editor.selected_text_range(false, window, cx).unwrap().range,
                    3..3
                );
                editor.replace_text_in_range(None, "日本語", window, cx);
                assert_eq!(editor.marked_text_range(window, cx), None);
            });
            assert_eq!(view.source.read(cx).text(cx), "a日本語b مرحبا\n");
            assert_eq!(view.second.read(cx).text(cx), "a日本語b مرحبا\n");
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
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-shift-z"
        } else {
            "ctrl-shift-z"
        },
    );
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "a日本語b مرحبا\n");
            assert_eq!(view.second.read(cx).text(cx), "a日本語b مرحبا\n");
        })
        .unwrap();
}

#[gpui::test]
fn vim_gates_platform_input_and_accepts_composition_in_insert_mode(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let original = "a🙂b\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                assert!(editor.selected_text_range(false, window, cx).is_none());
                editor.replace_and_mark_text_in_range(Some(1..3), "漢", Some(1..1), window, cx);
                editor.replace_text_in_range(Some(1..3), "漢", window, cx);
                assert_eq!(editor.text(cx), original);
                assert_eq!(editor.marked_text_range(window, cx), None);
            });
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "i");
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                assert!(editor.selected_text_range(false, window, cx).is_some());
                editor.replace_and_mark_text_in_range(Some(1..3), "漢", Some(1..1), window, cx);
                editor.replace_text_in_range(None, "漢字", window, cx);
                assert_eq!(editor.marked_text_range(window, cx), None);
            });
            assert_eq!(view.second.read(cx).text(cx), "a漢字b\n");
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), original);
            assert_eq!(view.second.read(cx).text(cx), original);
        })
        .unwrap();
}
