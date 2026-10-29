use super::*;
use gpui::TestAppContext;
use multi_buffer::ToOffset as _;
use text::ToOffset as _;

const NOTE: &str = "before\n- Parent ^parent\n  - child🙂\nafter\n";

fn excerpt_window(cx: &mut TestAppContext) -> gpui::WindowHandle<NotebookWindow> {
    cx.add_window(|window, cx| {
        let buffer = crate::markdown::buffer(NOTE, cx);
        let start = NOTE.find("- Parent").unwrap();
        let end = NOTE.find("\nafter").unwrap();
        let excerpts = crate::editor_excerpt::buffer(buffer.clone(), start..end, cx).unwrap();
        let view = NotebookWindow::for_buffer(buffer, Some(excerpts), window, cx);
        window.focus(&view.second.focus_handle(cx), cx);
        view
    })
}

#[gpui::test]
fn excerpt_vim_edits_and_source_undo_share_one_document(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = excerpt_window(cx);
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child🙂"
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "j A ! escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                NOTE.replace("child🙂", "child🙂!")
            );
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), NOTE);
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child🙂"
            );
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "ctrl-r");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                NOTE.replace("child🙂", "child🙂!")
            );
        })
        .unwrap();
}

#[gpui::test]
fn excerpt_boundary_deletion_and_select_all_cannot_escape(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    super::search_tests::set_vim_mode(cx, false);
    let window = excerpt_window(cx);
    cx.simulate_keystrokes(window.into(), "backspace");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), NOTE)
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-a"
        } else {
            "ctrl-a"
        },
    );
    cx.update(|cx| cx.write_to_clipboard(gpui::ClipboardItem::new_string("replacement🙂".into())));
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-v"
        } else {
            "ctrl-v"
        },
    );
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                "before\nreplacement🙂\nafter\n"
            );
            assert_eq!(view.second.read(cx).text(cx), "replacement🙂");
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "delete");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                "before\nreplacement🙂\nafter\n"
            );
        })
        .unwrap();
}

#[gpui::test]
fn excerpt_anchors_follow_source_edits_and_map_back_to_the_page(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = excerpt_window(cx);
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.source.focus_handle(cx), cx)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "g g shift-o n e w escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), format!("new\n{NOTE}"));
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child🙂"
            );
            let source = view
                .source
                .read(cx)
                .buffer()
                .read(cx)
                .as_singleton()
                .unwrap();
            let source = source.read(cx).snapshot();
            let excerpt = view.second.read(cx).buffer().read(cx).snapshot(cx);
            let range = excerpt.excerpts().next().unwrap().context;
            assert_eq!(
                range.start.to_offset(&source),
                NOTE.find("- Parent").unwrap() + 4
            );
            assert_eq!(
                range.end.to_offset(&source),
                NOTE.find("\nafter").unwrap() + 4
            );
            let child = NOTE.find("child").unwrap();
            let anchor = excerpt
                .anchor_in_buffer(source.anchor_before(child + 4))
                .unwrap();
            assert_eq!(
                anchor.to_offset(&excerpt).0,
                child - NOTE.find("- Parent").unwrap()
            );
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), NOTE)
        })
        .unwrap();
}

#[gpui::test]
fn excerpt_vim_join_delete_and_substitution_stay_inside_the_range(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = excerpt_window(cx);
    cx.simulate_keystrokes(window.into(), "shift-g shift-j");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), NOTE)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "d d");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                NOTE.replace("\n  - child🙂", "")
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u : % s / b e f o r e / x / g enter");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), NOTE)
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        ": % s / \\ C P a r e n t / C h a n g e d / g enter",
    );
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(
                !view.commands.read(cx).is_open(),
                "{:?}",
                view.commands.read(cx).error
            );
            assert_eq!(
                view.source.read(cx).text(cx),
                NOTE.replace("Parent", "Changed")
            );
        })
        .unwrap();
}

#[gpui::test]
fn invalid_excerpt_ranges_are_rejected_without_changing_the_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    cx.update(|cx| {
        let source = crate::markdown::buffer("a🙂b", cx);
        for (start, end) in [(2, 3), (4, 1), (0, 99)] {
            let error = crate::editor_excerpt::buffer(source.clone(), start..end, cx).unwrap_err();
            assert!(error.contains("Resolve the block again"));
        }
        assert_eq!(source.read(cx).text(), "a🙂b");
    });
}

#[gpui::test]
fn editor_context_expansion_cannot_widen_a_page_embed(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = excerpt_window(cx);
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(editor::actions::ExpandExcerpts::default()), cx);
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child🙂"
            );
            assert_eq!(view.source.read(cx).text(cx), NOTE);
        })
        .unwrap();
}

#[gpui::test]
fn two_excerpt_views_share_history_after_cloning_the_view(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let first = excerpt_window(cx);
    let (source, excerpts) = first
        .update(cx, |view, _, cx| {
            let source = view
                .source
                .read(cx)
                .buffer()
                .read(cx)
                .as_singleton()
                .unwrap();
            let excerpts = view.second.read(cx).buffer().clone();
            let clone = excerpts.update(cx, |excerpts, cx| cx.new(|cx| excerpts.clone(cx)));
            (source, clone)
        })
        .unwrap();
    let second = cx.add_window(|window, cx| {
        let view = NotebookWindow::for_buffer(source, Some(excerpts), window, cx);
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.simulate_keystrokes(first.into(), "j shift-a ! escape");
    second
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child🙂!"
            );
        })
        .unwrap();
    cx.simulate_keystrokes(second.into(), "u");
    first
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), NOTE)
        })
        .unwrap();
    cx.simulate_keystrokes(first.into(), "ctrl-r");
    second
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                NOTE.replace("child🙂", "child🙂!")
            );
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child🙂!"
            );
        })
        .unwrap();
}
