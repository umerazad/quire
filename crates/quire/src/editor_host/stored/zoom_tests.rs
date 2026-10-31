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
