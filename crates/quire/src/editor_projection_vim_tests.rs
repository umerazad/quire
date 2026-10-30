use super::*;
use gpui::TestAppContext;

fn projected_note(text: &str, cx: &mut TestAppContext) -> gpui::WindowHandle<NotebookWindow> {
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(text, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.executor().run_until_parked();
    window
}

#[gpui::test]
fn line_changes_preserve_block_markers_and_identity(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let mut failures = Vec::new();
    for (note, keys, expected) in [
        ("- alpha ^abc\n", "c c n e w escape", "- new ^abc\n"),
        ("- alpha ^abc\n", "shift-s n e w escape", "- new ^abc\n"),
        ("- alpha ^abc\n", "shift-c n e w escape", "- new ^abc\n"),
        ("- alpha ^abc", "c c n e w escape", "- new ^abc"),
        ("- alpha ^abc", "shift-s n e w escape", "- new ^abc"),
        ("- alpha", "c c n e w escape", "- new"),
        ("  - alpha ^abc\n", "c c n e w escape", "  - new ^abc\n"),
        ("12. alpha ^abc\n", "c c n e w escape", "12. new ^abc\n"),
        ("- alpha ^abc", "shift-v c n e w escape", "- new ^abc"),
        ("- ", "c c n e w escape", "- new"),
        ("- ", "shift-s n e w escape", "- new"),
        ("- ", "shift-c n e w escape", "- new"),
        ("plain ^id", "c c n e w escape", "new ^id"),
        ("^id", "c c n e w escape", "new ^id"),
        ("- [ ] Task ^id\n", "c c n e w escape", "- [ ] new ^id\n"),
        ("- [x] Task ^id", "shift-s n e w escape", "- [x] new ^id"),
        ("- [ ] ^id", "c c n e w escape", "- [ ] new ^id"),
        ("- [ ] ", "c c n e w escape", "- [ ] new"),
        (
            "- {{[[TODO]]}} Task ^id",
            "c c n e w escape",
            "- {{[[TODO]]}} new ^id",
        ),
    ] {
        let window = projected_note(note, cx);
        cx.simulate_keystrokes(window.into(), keys);
        window
            .update(cx, |view, window, cx| {
                let actual = view.source.read(cx).text(cx);
                if actual != expected {
                    failures.push(format!(
                        "{keys:?} in {note:?}: expected {expected:?}, got {actual:?}"
                    ));
                }
                window.focus(&view.source.focus_handle(cx), cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "u");
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.source.read(cx).text(cx),
                    note,
                    "shared undo after {keys}"
                )
            })
            .unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[gpui::test]
fn empty_block_prefix_survives_clearing_before_typing(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    for (note, display) in [("- ", ""), ("- [ ] ", "☐ ")] {
        let window = projected_note(note, cx);
        window
            .update(cx, |view, window, cx| {
                let buffer = view
                    .source
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .unwrap();
                let blocks = crate::markdown_blocks::blocks(&buffer.read(cx).snapshot());
                assert_eq!(
                    view.second.update(cx, |editor, cx| editor
                        .snapshot(window, cx)
                        .display_snapshot
                        .text()),
                    display,
                    "blocks={blocks:?}"
                );
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "c c");
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.source.read(cx).text(cx),
                    note,
                    "the empty prefix must survive cc before input"
                )
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "n e w escape");
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), format!("{note}new"))
            })
            .unwrap();
    }
}

#[gpui::test]
fn line_deletion_removes_the_whole_block_with_or_without_a_terminal_newline(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| init(cx).unwrap());
    let mut failures = Vec::new();
    for (note, keys, expected) in [
        ("- alpha ^abc\n", "d d", ""),
        ("- alpha ^abc", "d d", ""),
        ("- alpha", "d d", ""),
        ("- ^abc", "d d", ""),
        ("- ", "d d", ""),
        ("plain ^id", "d d", ""),
        ("^id", "d d", ""),
        ("- مرحبا🙂 ^ar\n- second ^b", "d d", "- second ^b"),
        ("- alpha ^abc", "shift-v d", ""),
        ("- alpha ^abc", "d a p", ""),
        ("- first ^a\n- second ^b\n", "d d", "- second ^b\n"),
        ("- first ^a\n- second ^b\n", "j d d", "- first ^a\n"),
        ("- first ^a\n- second ^b", "j d d", "- first ^a"),
        (
            "- first ^a\n- second ^b\n- third ^c",
            "j d d",
            "- first ^a\n- third ^c",
        ),
    ] {
        let window = projected_note(note, cx);
        cx.simulate_keystrokes(window.into(), keys);
        window
            .update(cx, |view, window, cx| {
                let actual = view.source.read(cx).text(cx);
                if actual != expected {
                    failures.push(format!(
                        "{keys:?} in {note:?}: expected {expected:?}, got {actual:?}"
                    ));
                }
                window.focus(&view.source.focus_handle(cx), cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), "u");
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), note)
            })
            .unwrap();
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[gpui::test]
fn projected_excerpt_line_deletion_stays_bounded_and_undoes_from_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let note = "before ^outside\n\n- selected ^id\n\nafter ^outside2\n";
    let window = cx.add_window(|window, cx| {
        let source = crate::markdown::buffer(note, cx);
        let excerpt = crate::editor_excerpt::buffer(
            source.clone(),
            note.find("- selected").unwrap()..note.find("\n\nafter").unwrap(),
            cx,
        )
        .unwrap();
        let view = NotebookWindow::for_buffer(source, Some(excerpt), window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "d d");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                "before ^outside\n\n\n\nafter ^outside2\n"
            );
            assert_eq!(view.second.read(cx).text(cx), "");
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), note)
        })
        .unwrap();
}
