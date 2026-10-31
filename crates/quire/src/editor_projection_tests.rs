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

fn activate_focused_control(
    window: gpui::WindowHandle<NotebookWindow>,
    key: &str,
    cx: &mut TestAppContext,
) {
    cx.executor().run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
        let keystroke = gpui::Keystroke::parse_with_os(key, cx.platform_info().os).unwrap();
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
}

fn resize_and_draw(window: gpui::WindowHandle<NotebookWindow>, cx: &mut TestAppContext) {
    cx.simulate_window_resize(window.into(), gpui::size(gpui::px(360.), gpui::px(600.)));
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.executor().run_until_parked();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// Task commands date completions 2026-10-29 in UTC.
fn fixed_task_clock(cx: &mut gpui::App) {
    cx.set_global(crate::tasks::TaskContext {
        now: std::rc::Rc::new(|| 1_793_275_200_000),
        calendar: Some(quire_core::daily::DailyCalendar::utc()),
        layout: Default::default(),
        title: std::rc::Rc::new(|_, _| None),
    });
}

#[gpui::test]
fn task_checkbox_edits_the_shared_marker_and_undoes_from_source(cx: &mut TestAppContext) {
    cx.update(|cx| {
        fixed_task_clock(cx);
        init(cx).expect("editor initialization")
    });
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new("- [ ] مرحبا🙂 ^task\n", window, cx);
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
                "☐ مرحبا🙂\n"
            );
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let bounds = visual
        .debug_bounds("quire-task-checkbox")
        .expect("rendered task checkbox");
    visual.simulate_click(bounds.center(), Default::default());
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                "- [x] مرحبا🙂 ✅ 2026-10-29 ^task\n"
            );
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "☑ مرحبا🙂 ✅ 2026-10-29\n"
            );
            window.focus(&view.source.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "u");
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- [ ] مرحبا🙂 ^task\n")
        })
        .unwrap();
}

#[gpui::test]
fn rendered_web_link_opens_its_url_without_editing_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "- [مرحبا🙂](https://example.com/path?q=1) ^id\n";
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(text, window, cx);
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
                "مرحبا🙂↗\n"
            );
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let bounds = visual
        .debug_bounds("quire-web-link")
        .expect("rendered web link");
    visual.simulate_click(bounds.center(), Default::default());
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/path?q=1")
    );
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), text);
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                text
            );
        })
        .unwrap();
}

#[gpui::test]
fn roam_checkbox_supports_keyboard_activation(cx: &mut TestAppContext) {
    cx.update(|cx| {
        fixed_task_clock(cx);
        init(cx).expect("editor initialization")
    });
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new("- {{[[TODO]]}} task ^id\n", window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            window.blur(cx);
            for _ in 0..3 {
                window.focus_next(cx);
            }
            assert!(!view.source.focus_handle(cx).is_focused(window));
            assert!(!view.second.focus_handle(cx).is_focused(window));
            assert!(view.second.focus_handle(cx).contains_focused(window, cx));
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "d d i x");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- {{[[TODO]]}} task ^id\n")
        })
        .unwrap();
    activate_focused_control(window, "enter", cx);
    window
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.source.read(cx).text(cx),
                "- {{[[DONE]]}} task ✅ 2026-10-29 ^id\n"
            );
            assert!(view.second.focus_handle(cx).contains_focused(window, cx));
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "☑ task ✅ 2026-10-29\n"
            );
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.blur(cx);
            for _ in 0..3 {
                window.focus_next(cx);
            }
        })
        .unwrap();
    activate_focused_control(window, "space", cx);
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "- {{[[TODO]]}} task ^id\n");
            window.blur(cx);
            for _ in 0..3 {
                window.focus_next(cx);
            }
        })
        .unwrap();
    cx.simulate_keystrokes(window.into(), "escape");
    window
        .update(cx, |view, window, cx| {
            assert!(view.second.focus_handle(cx).is_focused(window));
            assert_eq!(view.source.read(cx).text(cx), "- {{[[TODO]]}} task ^id\n");
        })
        .unwrap();
}

#[gpui::test]
fn link_excerpts_track_source_edits_and_leave_cut_links_as_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let note = "outside [Before](https://example.com/out)\n\n- [Inside](https://example.com/in) ^id\n\nend\n";
    let source = cx.update(|cx| crate::markdown::buffer(note, cx));
    let full = cx.update(|cx| {
        crate::editor_excerpt::buffer(
            source.clone(),
            note.find("- [Inside]").unwrap()..note.find("\n\nend").unwrap(),
            cx,
        )
        .unwrap()
    });
    let partial_range = note.find("[Inside]").unwrap()
        ..note.find("https://example.com/in").unwrap() + "https://".len();
    let partial = cx.update(|cx| {
        crate::editor_excerpt::buffer(source.clone(), partial_range.clone(), cx).unwrap()
    });
    let full = cx.add_window(|window, cx| {
        let view = NotebookWindow::for_buffer(source.clone(), Some(full), window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    let partial = cx.add_window(|window, cx| {
        let view = NotebookWindow::for_buffer(source.clone(), Some(partial), window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    let url = note.find("https://example.com/in").unwrap();
    let tail = url + "https://example.com/".len();
    assert!(
        tail >= partial_range.end,
        "the URL edit must stay outside the partial excerpt"
    );
    source.update(cx, |source, cx| {
        source.edit(
            [
                (0..0, "prefix\n"),
                (tail..url + "https://example.com/in".len(), "new"),
            ],
            None,
            cx,
        )
    });
    cx.executor().run_until_parked();
    full.update(cx, |view, window, cx| {
        assert_eq!(
            view.second.update(cx, |editor, cx| editor
                .snapshot(window, cx)
                .display_snapshot
                .text()),
            "Inside↗"
        );
        assert_eq!(
            view.source.read(cx).text(cx),
            format!(
                "prefix\n{}",
                note.replace("https://example.com/in", "https://example.com/new")
            )
        );
    })
    .unwrap();
    partial
        .update(cx, |view, window, cx| {
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                &note[partial_range]
            )
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(full.into(), cx);
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    let bounds = visual.debug_bounds("quire-web-link").unwrap();
    visual.simulate_click(bounds.center(), Default::default());
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/new"));
    let mut visual = gpui::VisualTestContext::from_window(partial.into(), cx);
    visual.update(|window, cx| {
        window.draw(cx).clear(cx);
    });
    assert!(visual.debug_bounds("quire-web-link").is_none());
}

#[gpui::test]
fn long_web_link_supports_keyboard_activation_without_editing(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let label = format!("{}🙂", "a".repeat(512));
    let text = format!("- [{label}](https://example.com/long) ^id\n");
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(&text, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    window
        .update(cx, |view, window, cx| {
            window.blur(cx);
            for _ in 0..3 {
                window.focus_next(cx);
            }
            assert!(!view.second.focus_handle(cx).is_focused(window));
            assert!(view.second.focus_handle(cx).contains_focused(window, cx));
        })
        .unwrap();
    activate_focused_control(window, "enter", cx);
    assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/long"));
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), text)
        })
        .unwrap();
}

#[gpui::test]
fn rendered_link_labels_wrap_inside_the_editor_viewport(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    for (label, key) in [
        ("مرحبا🙂 alpha beta gamma ".repeat(16), "enter"),
        (format!("{}🙂", "a".repeat(512)), "space"),
    ] {
        let text = format!("- [{label}](https://example.com/long) ^id\n");
        let window = cx.add_window(|window, cx| {
            let view = NotebookWindow::new(&text, window, cx);
            crate::editor_projection::bind(&view.second, window, cx).unwrap();
            view.second.update(cx, |editor, cx| {
                editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx)
            });
            view
        });
        cx.executor().run_until_parked();
        resize_and_draw(window, cx);
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        let bounds = visual
            .debug_bounds("quire-web-link")
            .expect("rendered link");
        assert!(
            bounds.right() <= gpui::px(360.),
            "link overflows the window: {bounds:?}"
        );
        assert!(
            bounds.size.height > gpui::px(40.),
            "link should span multiple wrapped rows: {bounds:?}"
        );
        let fragment = visual
            .debug_bounds(if key == "enter" {
                "quire-web-link-first-fragment"
            } else {
                "quire-web-link-last-fragment"
            })
            .unwrap();
        visual.simulate_click(fragment.center(), Default::default());
        assert_eq!(cx.opened_url().as_deref(), Some("https://example.com/long"));
        let keyboard_url = format!("https://example.com/{key}");
        window
            .update(cx, |view, _, cx| {
                let source = view
                    .source
                    .read(cx)
                    .buffer()
                    .read(cx)
                    .as_singleton()
                    .unwrap();
                let start = text.find("https://example.com/long").unwrap();
                source.update(cx, |source, cx| {
                    source.edit(
                        [(
                            start..start + "https://example.com/long".len(),
                            keyboard_url.as_str(),
                        )],
                        None,
                        cx,
                    )
                });
            })
            .unwrap();
        cx.executor().run_until_parked();
        window
            .update(cx, |_, window, cx| {
                window.blur(cx);
                for _ in 0..3 {
                    window.focus_next(cx);
                }
            })
            .unwrap();
        activate_focused_control(window, key, cx);
        assert_eq!(cx.opened_url().as_deref(), Some(keyboard_url.as_str()));
        window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.source.read(cx).text(cx),
                    text.replace("https://example.com/long", &keyboard_url)
                )
            })
            .unwrap();
    }
}

#[gpui::test]
fn focused_link_editing_and_copying_use_the_original_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    super::search_tests::set_vim_mode(cx, false);
    let text = "- [مرحبا🙂 link](https://example.com) ^id\n";
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(text, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        window.focus(&view.second.focus_handle(cx), cx);
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
                "[مرحبا🙂 link](https://example.com)\n"
            );
            view.second.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(3)..MultiBufferOffset(3 + "مر".len())])
                })
            });
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-c"
        } else {
            "ctrl-c"
        },
    );
    cx.update(|cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some("مر")
        )
    });
    cx.simulate_keystrokes(window.into(), "n e w");
    let edited = text.replace("مر", "new");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), edited)
        })
        .unwrap();
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-a cmd-c"
        } else {
            "ctrl-a ctrl-c"
        },
    );
    cx.update(|cx| {
        assert_eq!(
            cx.read_from_clipboard().unwrap().text().as_deref(),
            Some(edited.as_str())
        )
    });
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.source.focus_handle(cx), cx)
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
            assert_eq!(view.source.read(cx).text(cx), text)
        })
        .unwrap();
}

#[gpui::test]
fn wrapped_link_overlay_preserves_selection_in_fragment_gaps(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = format!(
        "- pre [{}xxxxxx](https://example.com/gap) ^id\n",
        "word ".repeat(11)
    );
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(&text, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view.second.update(cx, |editor, cx| {
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx)
        });
        view
    });
    cx.executor().run_until_parked();
    resize_and_draw(window, cx);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let bounds = visual.debug_bounds("quire-web-link").unwrap();
    let first = visual
        .debug_bounds("quire-web-link-first-fragment")
        .unwrap();
    assert!(
        bounds.right() > first.right() + gpui::px(2.),
        "fixture needs a gap beside a wrapped fragment: union {bounds:?}, first {first:?}"
    );
    let point = gpui::point((first.right() + bounds.right()) / 2., first.center().y);
    visual.simulate_click(point, Default::default());
    assert!(cx.opened_url().is_none());
    window
        .update(cx, |view, window, cx| {
            assert!(view.second.focus_handle(cx).is_focused(window));
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
}

#[gpui::test]
fn user_folds_hide_link_controls_and_survive_source_switching(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- [Link](https://example.com/fold) ^id\n";
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(text, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    let end = text.find(" ^id").unwrap();
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.fold_creases(
                    vec![editor::display_map::Crease::simple(
                        MultiBufferOffset(2)..MultiBufferOffset(end),
                        editor::FoldPlaceholder::default(),
                    )],
                    false,
                    window,
                    cx,
                )
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    assert!(visual.debug_bounds("quire-web-link").is_none());
    window
        .update(cx, |view, window, cx| {
            crate::editor_projection::unbind(&view.second, window, cx);
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                "- ⋯ ^id\n"
            );
            assert_eq!(view.source.read(cx).text(cx), text);
        })
        .unwrap();
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
fn checkbox_projection_respects_code_marker_boundaries_and_source_mode(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let text = "```md\n- [x] fake ^literal\n```\n\n- {{[[TODO]]}} Task ^id\n- [x]body ^b\n";
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(text, window, cx);
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
                "md · Format code · Copy code\n- [x] fake ^literal\n```\n\n☐ Task\n[x]body\n"
            );
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
            assert_eq!(
                view.second.update(cx, |editor, cx| editor
                    .snapshot(window, cx)
                    .display_snapshot
                    .text()),
                text
            );
            assert_eq!(view.source.read(cx).text(cx), text);
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
    activate_focused_control(window, "enter", cx);
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
                "md · Format code · Copy code\n- newest ^literal\n```\n",
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
