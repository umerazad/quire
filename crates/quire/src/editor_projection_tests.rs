use super::*;
use gpui::TestAppContext;
use multi_buffer::MultiBufferOffset;

fn projected_window(cx: &mut TestAppContext) -> gpui::WindowHandle<NotebookWindow> {
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new("- alpha ^abc\n", window, cx);
        crate::editor_projection::bind(&view.second, window, cx).expect("Markdown projection");
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.executor().run_until_parked();
    window
}

#[gpui::test]
fn switching_to_source_releases_id_guards_and_keeps_shared_undo(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    let toggle = if cfg!(target_os = "macos") {
        "cmd-shift-m"
    } else {
        "ctrl-shift-m"
    };
    cx.simulate_keystrokes(window.into(), toggle);
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "- alpha ^abc\n"
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "shift-d");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- \n");
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- alpha ^abc\n");
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), toggle);
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "alpha\n"
            );
            assert_eq!(
                view.source.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "- alpha ^abc\n"
            );
        })
        .unwrap();
}

#[gpui::test]
fn hidden_syntax_uses_the_shared_source_without_display_ellipsis(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- alpha ^abc\n");
            assert_eq!(view.second.read(cx).text(cx), "- alpha ^abc\n");
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "alpha\n"
            );
        })
        .unwrap();
}

#[gpui::test]
fn presentation_button_supports_keyboard_activation_and_returns_editor_focus(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    window
        .update(cx, |view, window, cx| {
            window.blur(cx);
            window.focus_next(cx);
            assert!(!view.source.focus_handle(cx).contains_focused(window, cx));
        })
        .unwrap();
    cx.executor().run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let keystroke = gpui::Keystroke::parse_with_os("enter", cx.platform_info().os).unwrap();
        window.dispatch_event(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
            cx,
        );
    })
    .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert!(crate::editor_projection::enabled(view.source.read(cx)));
            assert!(crate::editor_projection::enabled(view.second.read(cx)));
            assert!(view.source.focus_handle(cx).contains_focused(window, cx));
            assert_eq!(view.source.read(cx).text(cx), "- alpha ^abc\n");
        })
        .unwrap();
}

#[gpui::test]
fn repeated_mode_changes_keep_user_folds_and_reparse_source_edits(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    let toggle = if cfg!(target_os = "macos") {
        "cmd-shift-m"
    } else {
        "ctrl-shift-m"
    };
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.fold_ranges(
                    vec![MultiBufferOffset(3)..MultiBufferOffset(4)],
                    false,
                    window,
                    cx,
                )
            });
        })
        .unwrap();
    for _ in 0..3 {
        cx.simulate_keystrokes(window.into(), toggle);
        window
            .update(cx, |view, window, cx| {
                assert_eq!(
                    view.second.update(cx, |editor, cx| editor
                        .snapshot(window, cx)
                        .display_snapshot
                        .text()),
                    "- a⋯pha ^abc\n"
                );
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), toggle);
    }
    cx.simulate_keystrokes(window.into(), toggle);
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                let end = editor.buffer().read(cx).snapshot(cx).len();
                editor.transact(window, cx, |editor, _, cx| {
                    editor.edit([(end..end, "- beta ^b\n")], cx)
                });
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "- a⋯pha ^abc\n- beta ^b\n"
            );
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), toggle);
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "a⋯pha\nbeta\n"
            );
            assert_eq!(view.source.read(cx).text(cx), "- alpha ^abc\n- beta ^b\n");
        })
        .unwrap();
}

#[gpui::test]
fn opening_the_projection_places_input_after_its_hidden_marker(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    cx.simulate_keystrokes(window.into(), "i shift-z escape");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- Zalpha ^abc\n")
        })
        .unwrap();
}

#[gpui::test]
fn deleting_to_visible_line_end_preserves_the_hidden_id(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    cx.simulate_keystrokes(window.into(), "l l shift-d");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- al ^abc\n");
        })
        .unwrap();
}

#[gpui::test]
fn clearing_visible_text_and_typing_again_keeps_the_id_separate(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    window
        .update(cx, |view, _, cx| {
            let source = view
                .source
                .read(cx)
                .buffer()
                .read(cx)
                .as_singleton()
                .unwrap();
            source.update(cx, |source, _| {
                source.set_group_interval(std::time::Duration::ZERO)
            });
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "shift-d");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- ^abc\n")
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "i x y z escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- xyz ^abc\n");
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- ^abc\n");
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "ctrl-r");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- xyz ^abc\n")
        })
        .unwrap();
}

#[gpui::test]
fn whole_block_deletion_and_undo_restore_source_and_display(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    cx.simulate_keystrokes(window.into(), "d d");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "");
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- alpha ^abc\n");
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "alpha\n"
            );
        })
        .unwrap();
}

#[gpui::test]
fn native_id_deletions_match_the_legacy_outline_fixtures(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    for (from, to, expected) in [
        (4, 12, "- al ^abc\n"),
        (7, 8, "- alpha ^abc\n"),
        (2, 12, "- ^abc\n"),
    ] {
        let window = projected_window(cx);
        window
            .update(cx, |view, window, cx| {
                view.second.update(cx, |editor, cx| {
                    editor.transact(window, cx, |editor, _, cx| {
                        editor.edit([(MultiBufferOffset(from)..MultiBufferOffset(to), "")], cx)
                    });
                });
                assert_eq!(view.source.read(cx).text(cx), expected);
            })
            .unwrap();
    }
}

#[gpui::test]
fn replacing_a_partial_selection_keeps_its_hidden_id_and_typing_order(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    super::search_tests::set_vim_mode(cx, false);
    let window = projected_window(cx);
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(4)..MultiBufferOffset(12)]);
                })
            });
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("X".into()));
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-v"
        } else {
            "ctrl-v"
        },
    );
    cx.simulate_input(window.into(), "Y");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- alXY ^abc\n")
        })
        .unwrap();
}

#[gpui::test]
fn deleting_a_block_and_typing_new_text_does_not_hide_the_new_text(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    cx.simulate_keystrokes(window.into(), "d d i n e w escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "new");
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "new"
            );
        })
        .unwrap();
}

#[gpui::test]
fn source_edits_refresh_syntax_without_removing_user_folds(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.fold_ranges(
                    vec![MultiBufferOffset(3)..MultiBufferOffset(4)],
                    false,
                    window,
                    cx,
                )
            });
            view.source.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(2)..MultiBufferOffset(2)])
                })
            });
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "i shift-z escape");
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- Zalpha ^abc\n");
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "Za⋯pha\n"
            );
        })
        .unwrap();
}

#[gpui::test]
fn projection_recognizes_new_blocks_after_source_edits(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                editor.set_text("- alpha ^abc\n- second ^two\n", window, cx)
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                "- alpha ^abc\n- second ^two\n"
            );
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "alpha\nsecond\n"
            );
        })
        .unwrap();
}

#[gpui::test]
fn fresh_unicode_blocks_use_the_new_id_guards(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                editor.set_text("- مرحبا🙂 ^ar\n- alpha ^last\n", window, cx)
            });
            window.focus(&view.second.focus_handle(cx), cx);
        })
        .unwrap();
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), ": 2 enter");
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "l l shift-d");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- مرحبا🙂 ^ar\n- al ^last\n")
        })
        .unwrap();
}

#[gpui::test]
fn pending_parse_results_cannot_apply_obsolete_projection_spans(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = projected_window(cx);
    let code = "```md\n- newest ^literal\n```\n";
    window
        .update(cx, |view, window, cx| {
            let source = view
                .source
                .read(cx)
                .buffer()
                .read(cx)
                .as_singleton()
                .unwrap();
            source.update(cx, |source, _| source.set_sync_parse_timeout(None));
            view.source.update(cx, |editor, cx| {
                editor.set_text("- stale ^one\n  - old ^two\n", window, cx);
                editor.set_text(code, window, cx);
            });
            assert!(
                crate::markdown_blocks::blocks(&source.read(cx).snapshot()).is_none(),
                "an interpolated tree must not publish semantic spans"
            );
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), code);
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                code
            );
        })
        .unwrap();
}

#[gpui::test]
fn semantic_spans_map_into_an_excerpt_after_outside_source_edits(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let note = "before ^outside\n- Parent ^p\n  - child🙂 ^c\n\nafter ^outside2\n";
    let window = cx.add_window(|window, cx| {
        let source = crate::markdown::buffer(note, cx);
        let excerpt = crate::editor_excerpt::buffer(
            source.clone(),
            note.find("- Parent").unwrap()..note.find("\n\nafter").unwrap(),
            cx,
        )
        .unwrap();
        let view = NotebookWindow::for_buffer(source, Some(excerpt), window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "Parent\n  child🙂"
            );
            view.source.update(cx, |editor, cx| {
                editor.transact(window, cx, |editor, _, cx| {
                    editor.edit([(MultiBufferOffset(0)..MultiBufferOffset(0), "new\n")], cx)
                })
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), format!("new\n{note}"));
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "Parent\n  child🙂"
            );
        })
        .unwrap();
}
