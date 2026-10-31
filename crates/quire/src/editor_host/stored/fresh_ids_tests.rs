use super::tests::{Notebook, draw, open_notebook, read, settle, wait_until};
use gpui::{ClipboardItem, TestAppContext};

fn text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    read(notebook, cx, |view, cx| {
        view.editor
            .as_ref()
            .expect("an open page")
            .read(cx)
            .source_text(cx)
    })
}

#[gpui::test]
fn pasted_ids_another_page_has_become_fresh(cx: &mut TestAppContext) {
    let pages = [
        ("s", "Source.md", "- Here ^h\n"),
        ("o", "Other.md", "- There ^far\n"),
    ];
    let notebook = open_notebook(&pages, "s", cx);
    settle(&notebook, cx);
    draw(&notebook, cx);
    cx.write_to_clipboard(ClipboardItem::new_string(
        "- Copy ^far\n- Again ^h\n- Mine ^mine\n".into(),
    ));
    cx.simulate_keystrokes(notebook.window.into(), "p");
    wait_until(cx, "the copied ID to change", |cx| {
        !text(&notebook, cx).contains("^far")
    });
    let pasted = text(&notebook, cx);
    let lines: Vec<&str> = pasted.lines().collect();
    assert_eq!(lines[0], "- Here ^h");
    assert!(lines[1].starts_with("- Copy ^b-"), "{pasted}");
    assert!(lines[2].starts_with("- Again ^b-"), "{pasted}");
    assert_eq!(lines[3], "- Mine ^mine", "an ID no page has stays");
}
