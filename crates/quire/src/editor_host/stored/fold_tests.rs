use super::tests::{Notebook, draw, open_notebook, read, wait_until};
use super::workspace_tests::reopen;
use editor::Editor;
use gpui::{Entity, TestAppContext};

fn write_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<Editor> {
    wait_until(cx, "the page editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    read(notebook, cx, |view, cx| {
        let page = view.editor.clone().expect("an open page");
        let page = page.read(cx);
        [page.source.clone(), page.second.clone()]
            .into_iter()
            .find(|editor| crate::editor_projection::enabled(editor.read(cx)))
            .expect("a Write editor")
    })
}

fn show_written(notebook: &Notebook, cx: &mut TestAppContext) {
    wait_until(cx, "the page editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            let editor = view.editor.clone().expect("an open page");
            editor.update(cx, |editor, cx| editor.show_written(window, cx));
        })
        .unwrap();
    cx.run_until_parked();
}

fn shown(editor: &Entity<Editor>, cx: &mut TestAppContext) -> String {
    editor.update(cx, |editor, cx| editor.display_snapshot(cx).text())
}

fn saved_folds(notebook: &Notebook) -> String {
    std::fs::read_to_string(notebook.path.with_file_name("quire-folds.json")).unwrap_or_default()
}

/// A fold made in a zoom is saved with the page's, and a page view that keeps editing does
/// not drop it, since each view saves only the folds it changed.
#[gpui::test]
fn folds_made_in_a_zoom_are_saved_with_the_page(cx: &mut TestAppContext) {
    let page = "- Top ^t\n  - Kid\n- Parent ^p\n  - Child ^c\n    - Grand\n- Last\n";
    let notebook = open_notebook(&[("s", "Source.md", page)], "s", cx);
    show_written(&notebook, cx);
    let editor = write_editor(&notebook, cx);
    let place = |at: &str, notebook: &Notebook, cx: &mut TestAppContext| {
        let at = editor::MultiBufferOffset(page.find(at).unwrap());
        notebook
            .window
            .update(cx, |view, window, cx| {
                view.focus_active(window, cx);
                editor.update(cx, |editor, cx| {
                    editor.change_selections(Default::default(), window, cx, |selections| {
                        selections.select_ranges([at..at])
                    });
                    window.focus(&gpui::Focusable::focus_handle(editor, cx), cx);
                });
            })
            .unwrap();
        draw(notebook, cx);
    };
    place("Top", &notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "z c");
    wait_until(cx, "the page's fold", |_| {
        saved_folds(&notebook).contains("\"id\":\"t\"")
    });

    place("Parent", &notebook, cx);
    let zoom = if cfg!(target_os = "macos") {
        "cmd-."
    } else {
        "ctrl-."
    };
    cx.simulate_keystrokes(notebook.window.into(), zoom);
    let zoomed = read(&notebook, cx, |view, cx| {
        let page = view.editor.clone().expect("an open page");
        let page = page.read(cx);
        (0..2)
            .find(|&pane| page.zooms[pane].is_some())
            .map(|pane| page.shown(pane))
            .expect("a zoom")
    });
    assert_eq!(shown(&zoomed, cx), "Parent\n  Child\n    Grand");
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j z c");
    assert_eq!(shown(&zoomed, cx), "Parent\n  Child⋯");
    wait_until(cx, "the zoom's fold", |_| {
        let saved = saved_folds(&notebook);
        saved.contains("\"id\":\"c\"") && saved.contains("\"id\":\"t\"")
    });

    // The page view saves again after an edit; the zoom's fold stays saved.
    editor.update(cx, |editor, cx| {
        editor.buffer().update(cx, |buffer, cx| {
            buffer.edit(
                [(
                    editor::MultiBufferOffset(page.len())..editor::MultiBufferOffset(page.len()),
                    "- More\n",
                )],
                None,
                cx,
            )
        })
    });
    cx.run_until_parked();
    assert!(saved_folds(&notebook).contains("\"id\":\"c\""));

    let notebook = reopen(notebook, "s", cx);
    show_written(&notebook, cx);
    let editor = write_editor(&notebook, cx);
    // The added line may or may not be saved yet; the folds above it come back either way.
    wait_until(cx, "the saved folds to apply", |cx| {
        shown(&editor, cx).starts_with("Top⋯\nParent\n  Child⋯\nLast\n")
    });
}

#[gpui::test]
fn folded_blocks_come_back_when_the_page_opens_again(cx: &mut TestAppContext) {
    let page = "- Parent ^p\n  - Kid\n- Other\n  - Kid two\n- Last\n";
    let notebook = open_notebook(&[("s", "Source.md", page)], "s", cx);
    show_written(&notebook, cx);
    let editor = write_editor(&notebook, cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.focus_active(window, cx);
            editor.update(cx, |editor, cx| {
                let start = editor::MultiBufferOffset(page.find("Other").unwrap());
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([start..start])
                });
                window.focus(&gpui::Focusable::focus_handle(editor, cx), cx);
            });
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "z c k k z c");
    assert_eq!(shown(&editor, cx), "Parent⋯\nOther⋯\nLast\n");
    wait_until(cx, "the saved folds", |_| {
        let saved = saved_folds(&notebook);
        saved.contains("\"id\":\"p\"") && saved.contains("Other")
    });
    assert_eq!(
        editor.read_with(cx, |editor, cx| editor.text(cx)),
        page,
        "folding never edits the page"
    );

    let notebook = reopen(notebook, "s", cx);
    show_written(&notebook, cx);
    let editor = write_editor(&notebook, cx);
    wait_until(cx, "the saved folds to apply", |cx| {
        shown(&editor, cx) == "Parent⋯\nOther⋯\nLast\n"
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.focus_active(window, cx);
            window.focus(&gpui::Focusable::focus_handle(editor.read(cx), cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "z shift-r");
    assert_eq!(
        shown(&editor, cx),
        "Parent\n  Kid\nOther\n  Kid two\nLast\n"
    );
    wait_until(cx, "the cleared folds", |_| {
        !saved_folds(&notebook).contains("Other")
    });
}

/// A fold made or opened in a zoom reaches the page's view while the zoom is still open, not
/// only when the page opens again.
#[gpui::test]
fn folds_made_in_a_zoom_reach_the_page_view_at_once(cx: &mut TestAppContext) {
    let page = "- Parent ^p\n  - Child ^c\n    - Grand\n- Last\n";
    let notebook = open_notebook(&[("s", "Source.md", page)], "s", cx);
    show_written(&notebook, cx);
    let editor = write_editor(&notebook, cx);
    let at = editor::MultiBufferOffset(page.find("Parent").unwrap());
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.focus_active(window, cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([at..at])
                });
                window.focus(&gpui::Focusable::focus_handle(editor, cx), cx);
            });
        })
        .unwrap();
    draw(&notebook, cx);
    let zoom = if cfg!(target_os = "macos") {
        "cmd-."
    } else {
        "ctrl-."
    };
    cx.simulate_keystrokes(notebook.window.into(), zoom);
    let zoomed = read(&notebook, cx, |view, cx| {
        let page = view.editor.clone().expect("an open page");
        let page = page.read(cx);
        (0..2)
            .find(|&pane| page.zooms[pane].is_some())
            .map(|pane| page.shown(pane))
            .expect("a zoom")
    });
    assert_ne!(zoomed, editor, "the zoom has its own view");
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j z c");
    assert_eq!(shown(&zoomed, cx), "Parent\n  Child⋯");
    wait_until(cx, "the page view to fold Child", |cx| {
        shown(&editor, cx) == "Parent\n  Child⋯\nLast\n"
    });
    cx.simulate_keystrokes(notebook.window.into(), "z o");
    wait_until(cx, "the page view to open Child", |cx| {
        shown(&editor, cx) == "Parent\n  Child\n    Grand\nLast\n"
    });
}
