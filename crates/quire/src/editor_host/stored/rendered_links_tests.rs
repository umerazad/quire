use super::super::*;
use super::tests::{Notebook, active, draw, open_notebook, read, wait_until};
use gpui::{TestAppContext, VisualTestContext};

const PAGES: [(&str, &str, &str); 2] = [
    (
        "h",
        "Home.md",
        "- see [[Target|the target]] and ((blk))\n- next\n",
    ),
    ("t", "Target.md", "- target text ^blk\n"),
];

fn page_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn display(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let editor = page_editor(notebook, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.snapshot(window, cx).display_snapshot.text()
            })
        })
        .unwrap()
}

fn show_home_in_write(notebook: &Notebook, cx: &mut TestAppContext) {
    wait_until(cx, "Home", |cx| {
        read(notebook, cx, |view, cx| {
            view.page.read(cx).id() == "h" && view.editor.is_some()
        })
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    let editor = page_editor(notebook, cx);
    if !cx.update(|cx| crate::editor_projection::enabled(editor.read(cx))) {
        let mode = if cfg!(target_os = "macos") {
            "cmd-shift-m"
        } else {
            "ctrl-shift-m"
        };
        cx.simulate_keystrokes(notebook.window.into(), mode);
    }
    cx.simulate_keystrokes(notebook.window.into(), "g g j");
    draw(notebook, cx);
}

fn labels(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    let editor = page_editor(notebook, cx);
    cx.update(|cx| crate::editor_projection::preview_labels(editor.read(cx)))
}

/// Waits until the Write view shows `expected`, with `label` drawn for its one block preview.
fn wait_for_display(notebook: &Notebook, expected: &str, label: &str, cx: &mut TestAppContext) {
    wait_until(
        cx,
        &format!("the display {expected:?} with {label:?}"),
        |cx| {
            draw(notebook, cx);
            display(notebook, cx) == expected && labels(notebook, cx) == [label]
        },
    );
}

#[gpui::test]
fn write_renders_page_links_and_live_block_previews_that_open_on_click(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "h", cx);
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(1600.), gpui::px(900.)),
    );
    show_home_in_write(&notebook, cx);
    wait_for_display(
        &notebook,
        "see the target and ↗\nnext\n",
        "↗ target text",
        cx,
    );

    let mut visual = VisualTestContext::from_window(notebook.window.into(), cx);
    let preview = visual
        .debug_bounds("quire-block-preview")
        .expect("a rendered block preview");
    visual.simulate_click(preview.center(), Default::default());
    wait_until(cx, "the previewed block's page", |cx| {
        active(&notebook, cx) == "t"
    });

    let target = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .loaded()
            .unwrap()
            .analysis
            .read(cx)
            .source
            .clone()
    });
    target.update(cx, |buffer, cx| {
        let at = buffer.text().find("target text").unwrap();
        buffer.edit([(at..at + "target".len(), "changed")], None, cx)
    });
    let back = if cfg!(target_os = "macos") {
        "cmd-["
    } else {
        "alt-left"
    };
    cx.simulate_keystrokes(notebook.window.into(), back);
    show_home_in_write(&notebook, cx);
    wait_for_display(
        &notebook,
        "see the target and ↗\nnext\n",
        "↗ changed text",
        cx,
    );

    let home = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .loaded()
            .unwrap()
            .analysis
            .read(cx)
            .source
            .clone()
    });
    home.update(cx, |buffer, cx| {
        let at = buffer.text().find("blk").unwrap();
        buffer.edit([(at..at + 3, "nope")], None, cx)
    });
    wait_for_display(
        &notebook,
        "see the target and ?\nnext\n",
        "nope · unresolved",
        cx,
    );

    let mut visual = VisualTestContext::from_window(notebook.window.into(), cx);
    let link = visual
        .debug_bounds("quire-web-link")
        .expect("a rendered page link");
    visual.simulate_click(
        link.center(),
        gpui::Modifiers {
            shift: true,
            ..Default::default()
        },
    );
    wait_until(cx, "Target beside", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.page.read(cx).id() == "t")
        })
    });
    assert_eq!(active(&notebook, cx), "h");
}
