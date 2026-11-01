use super::holds_focus;
use super::tests::{draw, key, open_notebook, read, wait_until};
use gpui::{Focusable as _, TestAppContext};

#[gpui::test]
fn a_focused_zoom_counts_as_its_page_holding_focus(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("s", "Source.md", "- Parent\n  - Kid\n")], "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            let editor = view.editor.clone().expect("an open page");
            editor.update(cx, |editor, cx| {
                editor.set_writing(true, window, cx);
                window.focus(&editor.source.focus_handle(cx), cx);
            });
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("."));
    cx.run_until_parked();
    let focused = notebook
        .window
        .update(cx, |view, window, cx| {
            let editor = view.editor.clone().expect("an open page");
            let page = editor.read(cx);
            assert!(
                !page.source.focus_handle(cx).contains_focused(window, cx)
                    && !page.second.focus_handle(cx).contains_focused(window, cx),
                "the zoom, not a page editor, has focus"
            );
            holds_focus(&editor, window, cx)
        })
        .unwrap();
    assert!(focused, "Cmd-S saves the page a focused zoom shows");
}

/// Vim `o` on the last line of a block embed opens the new item inside the embedded block, so
/// the embed shows it and the caret stays there.
#[gpui::test]
fn vim_open_on_an_embeds_last_line_stays_in_the_embed(cx: &mut TestAppContext) {
    let source = "- Original ^shared\n- After\n";
    let pages = [
        ("s", "Source.md", source),
        ("o", "Other.md", "- {{embed: ((shared))}}\n"),
    ];
    let notebook = open_notebook(&pages, "o", cx);
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(1600.), gpui::px(1200.)),
    );
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-m"));
    let host = read(&notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    });
    let nested =
        |cx: &mut TestAppContext| cx.update(|cx| crate::embeds::nested_editors(host.read(cx)));
    wait_until(cx, "the embed", |cx| {
        draw(&notebook, cx);
        nested(cx).len() == 1
    });
    let embed = nested(cx).remove(0);
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&embed.focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "o n e w escape");
    cx.run_until_parked();
    notebook
        .window
        .update(cx, |_, window, cx| {
            let editor = embed.read(cx);
            assert_eq!(
                editor.text(cx),
                "- Original ^shared\n  - new",
                "the embed shows the new item"
            );
            assert!(embed.focus_handle(cx).is_focused(window));
        })
        .unwrap();
}

/// rt: "does not merge lines inside an embedded block editor". Backspace at a child's text
/// start in a block embed leaves the source page alone.
#[gpui::test]
fn backspace_does_not_merge_lines_in_a_block_embed(cx: &mut TestAppContext) {
    let source = "- Root ^r\n  - child\n    - grand\n- other\n";
    let pages = [
        ("s", "Source.md", source),
        ("o", "Other.md", "- {{embed: ((r))}}\n"),
    ];
    let notebook = open_notebook(&pages, "o", cx);
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(1600.), gpui::px(1200.)),
    );
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-m"));
    let host = read(&notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    });
    let nested =
        |cx: &mut TestAppContext| cx.update(|cx| crate::embeds::nested_editors(host.read(cx)));
    wait_until(cx, "the embed", |cx| {
        draw(&notebook, cx);
        nested(cx).len() == 1
    });
    let embed = nested(cx).remove(0);
    let child = editor::MultiBufferOffset(source.find("child").unwrap());
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&embed.focus_handle(cx), cx);
            embed.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([child..child])
                })
            });
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i backspace");
    cx.run_until_parked();
    assert_eq!(
        embed.read_with(cx, |editor, cx| editor.text(cx)),
        "- Root ^r\n  - child\n    - grand"
    );
}

/// Opens `host` in Write at `size` with its one embed of Source's `source` drawn, and puts the
/// caret in the embed at `caret`. Returns the host's editor and the embed's.
fn caret_in_embed(
    source: &str,
    host: &str,
    caret: usize,
    size: (f32, f32),
    vim: bool,
    cx: &mut TestAppContext,
) -> (
    super::tests::Notebook,
    gpui::Entity<editor::Editor>,
    gpui::Entity<editor::Editor>,
) {
    let pages = [("s", "Source.md", source), ("o", "Other.md", host)];
    let notebook = open_notebook(&pages, "o", cx);
    super::links_tests::set_vim(vim, cx);
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(size.0), gpui::px(size.1)),
    );
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-m"));
    let host = read(&notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    });
    let nested =
        |cx: &mut TestAppContext| cx.update(|cx| crate::embeds::nested_editors(host.read(cx)));
    wait_until(cx, "the embed", |cx| {
        draw(&notebook, cx);
        nested(cx).len() == 1
    });
    let embed = nested(cx).remove(0);
    let at = editor::MultiBufferOffset(caret);
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&embed.focus_handle(cx), cx);
            embed.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([at..at])
                })
            });
        })
        .unwrap();
    draw(&notebook, cx);
    (notebook, host, embed)
}

/// rt S36: opening a line below an embed's last line, with Vim `o` or Enter, adds an item
/// inside the embedded subtree, whether the block has children, ends its page, or is a page.
#[gpui::test]
fn opening_a_line_on_an_embeds_last_line_stays_in_the_embed(cx: &mut TestAppContext) {
    let cases = [
        (
            "- Root ^r\n  - child\n- After\n",
            "- {{embed: ((r))}}\n",
            "- Root ^r\n  - child\n  - new",
        ),
        (
            "- Root ^r\n  - child\n",
            "- {{embed: ((r))}}\n",
            "- Root ^r\n  - child\n  - new",
        ),
        (
            "- Root\n  - child\n",
            "- ![[Source]]\n",
            "- Root\n  - child\n  - new\n",
        ),
    ];
    for (vim, keys) in [(true, "o n e w escape"), (false, "end enter n e w")] {
        for (source, host, shown) in cases {
            let caret = source.find("child").unwrap();
            let (notebook, _, embed) = caret_in_embed(source, host, caret, (1600., 1200.), vim, cx);
            cx.simulate_keystrokes(notebook.window.into(), keys);
            cx.run_until_parked();
            notebook
                .window
                .update(cx, |_, window, cx| {
                    assert_eq!(embed.read(cx).text(cx), shown, "{keys} in {host}");
                    assert!(
                        embed.focus_handle(cx).is_focused(window),
                        "{keys} in {host}"
                    );
                })
                .unwrap();
        }
    }
}

/// The host page scrolls to keep the caret in sight while it moves through an embed taller
/// than the window, since the embed's own editor sizes to its content and never scrolls.
#[gpui::test]
fn the_host_scrolls_to_keep_the_caret_in_a_tall_embed_visible(cx: &mut TestAppContext) {
    let children: String = (0..80).map(|n| format!("  - child {n}\n")).collect();
    let source = format!("- Root ^r\n{children}");
    let (notebook, host, embed) = caret_in_embed(
        &source,
        "- intro\n- {{embed: ((r))}}\n- after\n",
        0,
        (800., 400.),
        true,
        cx,
    );
    let caret_shown = |cx: &mut TestAppContext| {
        notebook
            .window
            .update(cx, |_, window, cx| {
                let row = embed.update(cx, |embed, cx| {
                    let display = embed.display_snapshot(cx);
                    let head = embed.selections.newest::<text::Point>(&display).head();
                    display
                        .point_to_display_point(head, text::Bias::Left)
                        .row()
                        .0
                });
                host.update(cx, |host, cx| {
                    let top = host.scroll_position(cx).y;
                    let visible = host.visible_line_count().unwrap();
                    let snapshot = host.snapshot(window, cx);
                    let block = snapshot
                        .display_snapshot
                        .blocks_in_range(
                            editor::display_map::DisplayRow(0)
                                ..editor::display_map::DisplayRow(u32::MAX),
                        )
                        .find_map(|(row, block)| {
                            matches!(block, editor::display_map::Block::Custom(_)).then_some(row.0)
                        })
                        .unwrap();
                    // The embed's first line sits on its folded reference's line, above its block.
                    let caret = f64::from(block + row - 1);
                    (top, top <= caret && caret < top + visible)
                })
            })
            .unwrap()
    };
    let (first, shown) = caret_shown(cx);
    assert!(shown, "the embed's first line starts in sight");
    cx.simulate_keystrokes(notebook.window.into(), "shift-g");
    draw(&notebook, cx);
    let (top, shown) = caret_shown(cx);
    assert!(
        top > first + 40. && shown,
        "the host scrolled down to the last child, top {top}"
    );
    cx.simulate_keystrokes(notebook.window.into(), "g g");
    draw(&notebook, cx);
    let (up, shown) = caret_shown(cx);
    assert!(
        shown && up < top - 40.,
        "the host scrolled back up, top {up}"
    );
}
