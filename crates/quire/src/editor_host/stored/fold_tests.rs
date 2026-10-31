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
