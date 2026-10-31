use super::*;
use editor::display_map::ToDisplayPoint;
use gpui::{TestAppContext, px};
use multi_buffer::MultiBufferOffset;
use text::Bias;

fn draw_narrow(window: gpui::WindowHandle<NotebookWindow>, cx: &mut TestAppContext) {
    cx.simulate_window_resize(window.into(), gpui::size(px(360.), px(600.)));
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                assert!(
                    editor
                        .snapshot(window, cx)
                        .display_snapshot
                        .max_point()
                        .row()
                        .0
                        > 2
                );
            });
        })
        .unwrap();
}

#[gpui::test]
fn wrapped_unicode_body_boundaries_round_trip_with_hidden_syntax(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let body = "مرحبا🙂 اردو e\u{301} alpha beta gamma delta epsilon zeta eta theta";
    let note = format!("- {body} ^id\n  - child words for wrapping ^child\n");
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(&note, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.display_map.update(cx, |map, cx| {
                    map.set_wrap_width(Some(px(96.)), cx);
                });
                let display = editor.snapshot(window, cx).display_snapshot;
                assert!(
                    display.max_point().row().0 > 3,
                    "must exercise real soft wraps: {:?}",
                    display.text()
                );
                for boundary in body.char_indices().map(|(i, _)| i).chain([body.len()]) {
                    let offset = MultiBufferOffset(2 + boundary);
                    let point = offset.to_display_point(&display);
                    let left = point.to_offset(&display, Bias::Left);
                    let right = point.to_offset(&display, Bias::Right);
                    assert_eq!(left.to_display_point(&display), point);
                    assert_eq!(right.to_display_point(&display), point);
                    if boundary > 0 && boundary < body.len() {
                        assert_eq!(left, offset);
                        assert_eq!(right, offset);
                    }
                }
                let end = MultiBufferOffset(2 + body.len());
                let point = end.to_display_point(&display);
                for byte in end.0..end.0 + " ^id".len() + 1 {
                    assert_eq!(MultiBufferOffset(byte).to_display_point(&display), point);
                }
                let start = MultiBufferOffset(2).to_display_point(&display);
                for byte in 0..=2 {
                    assert_eq!(MultiBufferOffset(byte).to_display_point(&display), start);
                }
                assert_eq!(editor.text(cx), note);
            });
            crate::editor_projection::unbind(&view.second, window, cx);
            view.second.update(cx, |editor, cx| {
                editor.display_map.update(cx, |map, cx| {
                    map.set_wrap_width(None, cx);
                });
                assert_eq!(editor.snapshot(window, cx).display_snapshot.text(), note);
            });
        })
        .unwrap();
}

#[gpui::test]
fn complete_single_paragraph_replacement_drops_its_old_identity(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    super::search_tests::set_vim_mode(cx, false);
    for note in ["paragraph ^id", "^id"] {
        let paste = if cfg!(target_os = "macos") {
            "cmd-a cmd-v"
        } else {
            "ctrl-a ctrl-v"
        };
        for (keys, expected, metadata) in [
            (paste, "replacement🙂", true),
            (
                if cfg!(target_os = "macos") {
                    "cmd-a cmd-v"
                } else {
                    "ctrl-a ctrl-v"
                },
                "replacement🙂",
                false,
            ),
            (
                if cfg!(target_os = "macos") {
                    "cmd-a x"
                } else {
                    "ctrl-a x"
                },
                "x",
                false,
            ),
        ] {
            let window = cx.add_window(|window, cx| {
                let view = NotebookWindow::new(note, window, cx);
                crate::editor_projection::bind(&view.second, window, cx).unwrap();
                window.focus(&view.second.focus_handle(cx), cx);
                view
            });
            cx.executor().run_until_parked();
            cx.update(|cx| {
                let text = "replacement🙂".to_string();
                let item = if metadata {
                    gpui::ClipboardItem::new_string_with_json_metadata(
                        text.clone(),
                        vec![editor::ClipboardSelection {
                            len: text.len(),
                            is_entire_line: false,
                            first_line_indent: 0,
                            file_path: None,
                            line_range: None,
                        }],
                    )
                } else {
                    gpui::ClipboardItem::new_string(text)
                };
                cx.write_to_clipboard(item)
            });
            cx.simulate_keystrokes(window.into(), keys);
            window
                .update(cx, |view, _, cx| {
                    assert_eq!(
                        view.source.read(cx).text(cx),
                        expected,
                        "{keys} in {note:?}"
                    )
                })
                .unwrap();
        }
    }
}

#[gpui::test]
fn caret_at_a_wrapped_identity_boundary_types_before_the_id(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    super::search_tests::set_vim_mode(cx, false);
    let body = "مرحبا🙂 alpha beta gamma delta epsilon zeta eta theta";
    let note = format!("- {body} ^id\n");
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(&note, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.display_map.update(cx, |map, cx| {
                    map.set_wrap_width(Some(px(96.)), cx);
                });
                let display = editor.snapshot(window, cx).display_snapshot;
                assert!(display.max_point().row().0 > 2);
                let point = MultiBufferOffset(2 + body.len()).to_display_point(&display);
                let position = point.to_offset(&display, Bias::Right);
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([position..position])
                });
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    cx.simulate_keystrokes(window.into(), "!");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), format!("- {body}! ^id\n"));
        })
        .unwrap();
}

#[gpui::test]
fn wrapped_select_all_paste_replaces_the_page_and_undoes_from_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    super::search_tests::set_vim_mode(cx, false);
    let note = "- alpha beta gamma delta epsilon zeta eta theta ^first\n- second block ^second\n";
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(note, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.executor().run_until_parked();
    window
        .update(cx, |view, _, cx| {
            view.second.update(cx, |editor, cx| {
                editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
                editor.display_map.update(cx, |map, cx| {
                    map.set_wrap_width(Some(px(96.)), cx);
                });
            });
            cx.write_to_clipboard(gpui::ClipboardItem::new_string("replacement🙂".into()));
        })
        .unwrap();
    draw_narrow(window, cx);
    cx.simulate_keystrokes(
        window.into(),
        if cfg!(target_os = "macos") {
            "cmd-a cmd-v"
        } else {
            "ctrl-a ctrl-v"
        },
    );
    window
        .update(cx, |view, window, cx| {
            assert_eq!(view.source.read(cx).text(cx), "replacement🙂");
            window.focus(&view.source.focus_handle(cx), cx);
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
            assert_eq!(view.source.read(cx).text(cx), note)
        })
        .unwrap();
}

#[gpui::test]
fn complete_excerpt_replacement_stays_bounded_with_clipboard_typing_and_deletion(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| init(cx).unwrap());
    super::search_tests::set_vim_mode(cx, false);
    let excerpt = "- alpha beta gamma delta epsilon zeta eta theta ^first\n  - child ^child";
    let note = format!("before ^outside\n\n{excerpt}\n\nafter ^outside2\n");
    let paste = if cfg!(target_os = "macos") {
        "cmd-v"
    } else {
        "ctrl-v"
    };
    let select_all = if cfg!(target_os = "macos") {
        "cmd-a"
    } else {
        "ctrl-a"
    };
    let undo = if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };
    for (keys, expected) in [(paste, "replacement🙂"), ("x", "x"), ("backspace", "")] {
        let window = cx.add_window(|window, cx| {
            let buffer = crate::markdown::buffer(&note, cx);
            let start = note.find(excerpt).unwrap();
            let excerpts =
                crate::editor_excerpt::buffer(buffer.clone(), start..start + excerpt.len(), cx)
                    .unwrap();
            let view = NotebookWindow::for_buffer(buffer, Some(excerpts), window, cx);
            crate::editor_projection::bind(&view.second, window, cx).unwrap();
            window.focus(&view.second.focus_handle(cx), cx);
            view
        });
        cx.executor().run_until_parked();
        window
            .update(cx, |view, window, cx| {
                view.second.update(cx, |editor, cx| {
                    editor
                        .set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
                    editor.display_map.update(cx, |map, cx| {
                        map.set_wrap_width(Some(px(96.)), cx);
                    });
                    assert!(
                        editor
                            .snapshot(window, cx)
                            .display_snapshot
                            .max_point()
                            .row()
                            .0
                            > 2
                    );
                });
                cx.write_to_clipboard(gpui::ClipboardItem::new_string("replacement🙂".into()));
            })
            .unwrap();
        draw_narrow(window, cx);
        cx.simulate_keystrokes(window.into(), select_all);
        cx.simulate_keystrokes(window.into(), keys);
        window
            .update(cx, |view, window, cx| {
                assert_eq!(
                    view.source.read(cx).text(cx),
                    note.replace(excerpt, expected),
                    "{keys}"
                );
                assert_eq!(view.second.read(cx).text(cx), expected);
                window.focus(&view.source.focus_handle(cx), cx);
            })
            .unwrap();
        cx.simulate_keystrokes(window.into(), undo);
        cx.executor().run_until_parked();
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), note);
                assert_eq!(view.second.read(cx).text(cx), excerpt);
            })
            .unwrap();
    }
}

/// Reads every display row and summarizes every wrap row, which maps each soft wrap's start back
/// through folds and fails if a wrap lands inside a fold placeholder.
fn walk_rows(window: gpui::WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> Vec<String> {
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                let display = editor.snapshot(window, cx).display_snapshot;
                let wraps = display.wrap_snapshot();
                use editor::display_map::WrapRow;
                for row in 0..wraps.max_point().row().0 {
                    wraps.text_summary_for_range(WrapRow(row)..WrapRow(row + 1));
                }
                (0..=display.max_point().row().0)
                    .map(|row| display.line(editor::display_map::DisplayRow(row)))
                    .collect()
            })
        })
        .unwrap()
}

fn resize_and_draw(
    window: gpui::WindowHandle<NotebookWindow>,
    width: f32,
    cx: &mut TestAppContext,
) {
    cx.simulate_window_resize(window.into(), gpui::size(px(width), px(600.)));
    cx.update_window(window.into(), |_, window, cx| {
        window.draw(cx).clear(cx);
    })
    .unwrap();
    cx.executor().run_until_parked();
}

#[gpui::test]
fn code_toolbar_stays_whole_when_a_narrow_pane_wraps_before_layout(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let note = "| a | b |\n| -- | -- |\n| 1 | 2 |\n\n```typescript\nconst a = 1;\n```\n\nafter\n";
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(note, window, cx);
        view.second.update(cx, |editor, cx| {
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
            editor.display_map.update(cx, |map, cx| {
                map.set_wrap_width(Some(px(96.)), cx);
            });
        });
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    let toolbar = "typescript · Format code · Copy code";
    assert!(
        walk_rows(window, cx)
            .iter()
            .any(|row| row.contains(toolbar)),
        "the code toolbar is one unbroken display row before layout measures it"
    );
    resize_and_draw(window, 160., cx);
    walk_rows(window, cx);
}

const FRAGMENTS: &[&str] = &[
    "plain words that run long enough to wrap ",
    "**bold** ",
    "_italic_ ",
    "`code` ",
    "~~gone~~ ",
    "\\* ",
    "[[README]] ",
    "[label](https://example.com/a/long/path) ",
    "https://example.com/bare ",
    "((one)) ",
    "اردو العربية ",
    "🙂 ",
    "\t",
];

const LINES: &[&str] = &[
    "---",
    "| Type | **Bold** |\n| :-- | --: |\n| `x` | [[README]] |",
    "```typescript\nconst next = 1;\n```",
    "```json\n{ \"a\": 1 }\n```",
    "{{embed: ((one))}}",
    "> quoted **text**",
    "## Heading **bold**",
    "- [ ] task ",
    "- [x] done ",
    "1. ordered ",
    "  - child ",
    "- item ",
    "",
];

fn random_note(rng: &mut rand::rngs::StdRng) -> String {
    use rand::Rng as _;
    let mut note = String::from("- One shared block. ^one\n\n");
    for _ in 0..rng.random_range(3..14) {
        let line = LINES[rng.random_range(0..LINES.len())];
        note.push_str(line);
        if !line.contains('\n') && !line.starts_with("---") && !line.starts_with("{{") {
            for _ in 0..rng.random_range(0..6) {
                note.push_str(FRAGMENTS[rng.random_range(0..FRAGMENTS.len())]);
            }
            if rng.random_bool(0.3) {
                note.push_str(&format!("^id{}", rng.random_range(0..1000)));
            }
        }
        note.push_str("\n\n");
    }
    note
}

fn char_boundary(text: &str, rng: &mut rand::rngs::StdRng) -> usize {
    use rand::Rng as _;
    let mut offset = rng.random_range(0..=text.len());
    while !text.is_char_boundary(offset) {
        offset -= 1;
    }
    offset
}

/// Random Write pages with marks, rules, tables, code toolbars, IDs, links and embeds, drawn at
/// several widths while the caret reveals syntax and edits land, must never panic.
#[gpui::test(iterations = 12)]
fn random_write_pages_wrap_without_panicking(cx: &mut TestAppContext, mut rng: rand::rngs::StdRng) {
    use rand::Rng as _;
    cx.update(|cx| init(cx).unwrap());
    let note = random_note(&mut rng);
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(&note, window, cx);
        view.second.update(cx, |editor, cx| {
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
            editor.display_map.update(cx, |map, cx| {
                map.set_wrap_width(Some(px(rng.random_range(30.0..200.0))), cx);
            });
        });
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        window.focus(&view.second.focus_handle(cx), cx);
        view
    });
    cx.executor().run_until_parked();
    walk_rows(window, cx);
    for _ in 0..8 {
        let text = window
            .update(cx, |view, _, cx| view.source.read(cx).text(cx))
            .unwrap();
        let at = char_boundary(&text, &mut rng);
        let edit = rng.random_bool(0.4).then(|| {
            if rng.random_bool(0.5) {
                FRAGMENTS[rng.random_range(0..FRAGMENTS.len())].to_owned()
            } else {
                format!("\n{}\n", LINES[rng.random_range(0..LINES.len())])
            }
        });
        window
            .update(cx, |view, window, cx| {
                view.second.update(cx, |editor, cx| {
                    if let Some(edit) = &edit {
                        let at = MultiBufferOffset(at);
                        editor.buffer().update(cx, |buffer, cx| {
                            buffer.edit([(at..at, edit.as_str())], None, cx)
                        });
                    }
                    let caret = editor
                        .buffer()
                        .read(cx)
                        .snapshot(cx)
                        .clip_offset(MultiBufferOffset(at), Bias::Left);
                    editor.change_selections(
                        editor::SelectionEffects::no_scroll(),
                        window,
                        cx,
                        |selections| selections.select_ranges([caret..caret]),
                    );
                });
            })
            .unwrap();
        cx.executor().run_until_parked();
        walk_rows(window, cx);
        resize_and_draw(window, rng.random_range(80.0..520.0), cx);
        walk_rows(window, cx);
    }
}
