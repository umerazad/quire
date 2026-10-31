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
                editor.show_written(window, cx);
                window.focus(&editor.second.focus_handle(cx), cx);
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
