use super::*;
use crate::{page_analysis::PageAnalysis, semantic_excerpt::ExcerptState};
use gpui::TestAppContext;

const NOTE: &str = "before\n\n- Parent ^parent\n  - child ^child\n- outside ^outside\n";

#[gpui::test]
fn semantic_write_can_open_from_current_text_before_background_parse_finishes(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| init(cx).unwrap());
    let buffer = cx.update(|cx| crate::markdown::buffer(NOTE, cx));
    cx.executor().run_until_parked();
    buffer.update(cx, |buffer, cx| {
        buffer.set_sync_parse_timeout(None);
        let start = NOTE.find("Parent").unwrap();
        buffer.edit([(start..start + "Parent".len(), "Current")], None, cx);
    });
    let page = cx.new(|cx| PageAnalysis::new(buffer.clone(), cx));
    cx.update(|cx| {
        assert_ne!(
            *buffer.read(cx).parse_status().borrow(),
            language::ParseStatus::Idle
        );
        assert!(page.read(cx).current(cx).is_some());
    });
    let window = cx.add_window(|window, cx| NotebookWindow::for_block(page, "parent", window, cx));
    window
        .update(cx, |view, window, cx| {
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
            assert!(!editor.read(cx).read_only(cx));
            assert_eq!(
                editor.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "Current\n  child"
            );
        })
        .unwrap();
}

#[gpui::test]
fn semantic_typing_retains_all_characters_during_background_parsing(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let buffer = cx.update(|cx| crate::markdown::buffer(NOTE, cx));
    let page = cx.new(|cx| PageAnalysis::new(buffer.clone(), cx));
    let window = cx.add_window(|window, cx| NotebookWindow::for_block(page, "parent", window, cx));
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            crate::editor_projection::bind(&view.second, window, cx).unwrap();
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    buffer.update(cx, |buffer, _| buffer.set_sync_parse_timeout(None));
    cx.simulate_keystrokes(window.into(), "A x y z escape");
    cx.executor().run_until_parked();
    assert_eq!(
        cx.update(|cx| buffer.read(cx).text()),
        NOTE.replace("Parent", "Parentxyz")
    );
}

#[gpui::test]
fn semantic_editors_keep_shared_history_and_write_ranges_after_reparenting(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| init(cx).unwrap());
    let buffer = cx.update(|cx| crate::markdown::buffer(NOTE, cx));
    let page = cx.new(|cx| PageAnalysis::new(buffer.clone(), cx));
    let first =
        cx.add_window(|window, cx| NotebookWindow::for_block(page.clone(), "parent", window, cx));
    let second =
        cx.add_window(|window, cx| NotebookWindow::for_block(page.clone(), "parent", window, cx));
    cx.executor().run_until_parked();
    for window in [first, second] {
        window
            .update(cx, |view, window, cx| {
                crate::editor_projection::bind(&view.second, window, cx).unwrap();
                window.focus(&view.second.focus_handle(cx), cx);
            })
            .unwrap();
    }
    cx.simulate_keystrokes(first.into(), "j A ! escape");
    cx.executor().run_until_parked();
    second
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child! ^child"
            );
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(second.into(), "u");
    cx.executor().run_until_parked();
    assert_eq!(cx.update(|cx| buffer.read(cx).text()), NOTE);
    cx.simulate_keystrokes(first.into(), "ctrl-r");
    assert_eq!(
        cx.update(|cx| buffer.read(cx).text()),
        NOTE.replace("child ^child", "child! ^child")
    );
    cx.simulate_keystrokes(first.into(), "u");
    cx.executor().run_until_parked();
    assert_eq!(cx.update(|cx| buffer.read(cx).text()), NOTE);
    let indent = NOTE.find("  - child").unwrap();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(indent..indent + 2, "")], None, cx)
    });
    cx.executor().run_until_parked();
    for window in [first, second] {
        window
            .update(cx, |view, window, cx| {
                assert_eq!(view.second.read(cx).text(cx), "- Parent ^parent");
                assert_eq!(
                    view.second.update(cx, |editor, cx| editor
                        .snapshot(window, cx)
                        .display_snapshot
                        .text()),
                    "Parent"
                );
            })
            .unwrap();
    }
    first
        .update(cx, |view, window, cx| {
            window.focus(&view.second.focus_handle(cx), cx);
            window.dispatch_action(Box::new(editor::actions::ExpandExcerpts::default()), cx);
        })
        .unwrap();
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(first.into(), "A x escape");
    cx.executor().run_until_parked();
    assert_eq!(
        cx.update(|cx| buffer.read(cx).text()),
        NOTE.replace("  - child", "- child")
            .replace("Parent", "Parentx")
    );
    second
        .update(cx, |view, window, cx| {
            window.focus(&view.second.focus_handle(cx), cx)
        })
        .unwrap();
    cx.simulate_keystrokes(second.into(), "A y escape");
    cx.executor().run_until_parked();
    assert_eq!(
        cx.update(|cx| buffer.read(cx).text()),
        NOTE.replace("  - child", "- child")
            .replace("Parent", "Parentxy")
    );
    first
        .update(cx, |view, _, cx| {
            assert_eq!(view.second.read(cx).text(cx), "- Parentxy ^parent")
        })
        .unwrap();
}

#[gpui::test]
fn removed_semantic_block_disables_editor_and_source_undo_restores_it(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let buffer = cx.update(|cx| crate::markdown::buffer(NOTE, cx));
    let page = cx.new(|cx| PageAnalysis::new(buffer.clone(), cx));
    let window = cx.add_window(|window, cx| NotebookWindow::for_block(page, "parent", window, cx));
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            crate::editor_projection::bind(&view.second, window, cx).unwrap();
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    let start = NOTE.find("- Parent").unwrap();
    let end = NOTE.find("- outside").unwrap();
    buffer.update(cx, |buffer, cx| buffer.edit([(start..end, "")], None, cx));
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert!(view.second.read(cx).read_only(cx));
            assert_eq!(
                view.semantic_excerpt.as_ref().unwrap().read(cx).state,
                ExcerptState::Missing
            );
            assert_eq!(view.second.read(cx).text(cx), "");
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "i e s c a p e escape");
    assert_eq!(
        cx.update(|cx| buffer.read(cx).text()),
        "before\n\n- outside ^outside\n"
    );
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.source.focus_handle(cx), cx)
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(!view.second.read(cx).read_only(cx));
            assert_eq!(
                view.second.read(cx).text(cx),
                "- Parent ^parent\n  - child ^child"
            );
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "Parent\n  child"
            );
        })
        .unwrap();
}
