use super::{search_tests::selection, *};
use gpui::TestAppContext;

#[gpui::test]
fn colon_command_jumps_to_a_line_without_a_workspace(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "one\ntwo\nthree\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    cx.simulate_keystrokes(window.into(), ": 2 enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), text);
            assert_eq!(selection(&view.source, cx), 4..4);
            assert!(view.source.focus_handle(cx).is_focused(window));
        })
        .unwrap();
}

#[gpui::test]
fn substitution_edits_the_shared_document_and_undoes_from_the_other_pane(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let original = "one one\ntwo one\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
    cx.simulate_keystrokes(window.into(), ": % s / o n e / x / g enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(
                !view.commands.read(cx).is_open(),
                "{:?}",
                view.commands.read(cx).error
            );
            assert_eq!(view.source.read(cx).text(cx), "x x\ntwo x\n");
            assert_eq!(view.second.read(cx).text(cx), "x x\ntwo x\n");
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), original)
        })
        .unwrap();
}

#[gpui::test]
fn invalid_commands_remain_editable_and_escape_preserves_the_note(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "one\ntwo\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    cx.simulate_keystrokes(window.into(), ": z z z enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            let prompt = view.commands.read(cx);
            assert!(prompt.is_open());
            assert!(prompt.error.as_deref().unwrap().contains("Edit"));
            assert_eq!(prompt.input.read(cx).text(cx), "zzz");
            assert!(prompt.input.focus_handle(cx).is_focused(window));
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    window
        .update(cx, |view, window, cx| {
            assert!(!view.commands.read(cx).is_open());
            assert!(view.source.focus_handle(cx).is_focused(window));
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
}

#[gpui::test]
fn visual_and_count_prefixes_use_the_copied_range_model(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let original = "one\ntwo\nthree\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
    cx.simulate_keystrokes(window.into(), "shift-v j :");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.commands.read(cx).input.read(cx).text(cx), "'<,'>")
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "d enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(
                !view.commands.read(cx).is_open(),
                "{:?}",
                view.commands.read(cx).error
            );
            assert_eq!(view.source.read(cx).text(cx), "three\n")
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u 2 :");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.commands.read(cx).input.read(cx).text(cx), ".,.+1")
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "d enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "three\n")
        })
        .unwrap();
}

#[gpui::test]
fn missing_marks_and_invalid_patterns_do_not_close_the_command_prompt(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "one\ntwo\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    cx.simulate_keystrokes(window.into(), ": ' a d enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.commands.read(cx).is_open());
            assert!(
                view.commands
                    .read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("mark a")
            );
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape : % s / [ / x / g enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.commands.read(cx).is_open());
            assert!(
                view.commands
                    .read(cx)
                    .error
                    .as_deref()
                    .unwrap()
                    .contains("search pattern")
            );
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
}

#[gpui::test]
fn command_errors_can_be_corrected_in_the_second_pane(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "one\ntwo\nthree\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.second.focus_handle(cx), cx)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), ": z z z enter");
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        },
    );
    cx.simulate_keystrokes(window.into(), "3 enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(selection(&view.source, cx), 0..0);
            assert_eq!(selection(&view.second, cx), 8..8);
            assert!(view.second.focus_handle(cx).is_focused(window));
            assert!(!view.commands.read(cx).is_open());
            assert!(view.commands.read(cx).error.is_none());
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
}

#[gpui::test]
fn local_options_change_subsequent_vim_search(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = cx.add_window(|window, cx| NotebookWindow::new("one TARGET\n", window, cx));
    cx.simulate_keystrokes(window.into(), ": s e t space n o i g n o r e c a s e enter");
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "/ t a r g e t");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(!view.source_search.read(cx).has_active_match())
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        "escape : s e t space i g n o r e c a s e enter",
    );
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "/ t a r g e t");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.source_search.read(cx).has_active_match())
        })
        .unwrap();
}

#[gpui::test]
fn global_delete_and_sort_reuse_range_actions(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let original = "a one\nb\na two\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(original, window, cx));
    cx.simulate_keystrokes(window.into(), ": g / ^ a / d enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "b\n")
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), original)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), ": s o r t enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "a one\na two\nb\n")
        })
        .unwrap();
}

#[gpui::test]
fn unavailable_workspace_commands_do_not_dispatch_from_the_prompt(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "unsaved example\n";
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    for keys in [": w enter", ": ! p w d enter"] {
        cx.simulate_keystrokes(window.into(), keys);
        cx.executor().run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert!(view.commands.read(cx).is_open());
                assert!(
                    view.commands
                        .read(cx)
                        .error
                        .as_deref()
                        .unwrap()
                        .contains("unavailable")
                );
                assert_eq!(view.source.read(cx).text(cx), text);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "escape");
    }
}

#[gpui::test]
fn substitution_accepts_the_same_backreferences_as_the_copied_search_engine(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = cx.add_window(|window, cx| NotebookWindow::new("oneone\nother\n", window, cx));
    cx.simulate_keystrokes(window.into(), ": % s / \\ ( o n e \\ ) \\ 1 / x / g enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(
                !view.commands.read(cx).is_open(),
                "{:?}",
                view.commands.read(cx).error
            );
            assert_eq!(view.source.read(cx).text(cx), "x\nother\n");
        })
        .unwrap();
}
