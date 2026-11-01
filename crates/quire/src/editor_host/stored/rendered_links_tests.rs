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
            && read(&notebook, cx, |view, cx| {
                view.page.read(cx).loaded().is_some()
            })
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

/// The index re-analyzes every save, so its progress comes and goes while the user works. It
/// once took a line above the page, which moved a link 42 px down between a click's press and
/// release under load, so the release missed the link and the click was lost.
#[gpui::test]
fn indexing_progress_never_moves_the_page_under_the_pointer(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "h", cx);
    show_home_in_write(&notebook, cx);
    wait_for_display(
        &notebook,
        "see the target and ↗\nnext\n",
        "↗ target text",
        cx,
    );
    let bounds = |selector: &'static str, cx: &mut TestAppContext| {
        draw(&notebook, cx);
        VisualTestContext::from_window(notebook.window.into(), cx).debug_bounds(selector)
    };
    let link = bounds("quire-web-link", cx).expect("a rendered page link");
    assert_eq!(bounds("quire-index-progress", cx), None);

    let index = read(&notebook, cx, |view, _| view.index.clone().unwrap());
    index.update(cx, |index, cx| index.show_remaining(1, cx));
    assert!(
        bounds("quire-index-progress", cx).is_some(),
        "the progress shows"
    );
    assert_eq!(
        bounds("quire-web-link", cx),
        Some(link),
        "the link stays put"
    );
}

#[gpui::test]
fn clicking_a_tag_in_write_opens_its_tag_page_unless_the_caret_is_in_its_block(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(
        &[("h", "Home.md", "- first\n- tagged #Plan/sub here\n")],
        "h",
        cx,
    );
    show_home_in_write(&notebook, cx);
    let tag_bounds = |cx: &mut TestAppContext| {
        draw(&notebook, cx);
        VisualTestContext::from_window(notebook.window.into(), cx).debug_bounds("quire-tag-link")
    };
    cx.run_until_parked();
    assert!(
        tag_bounds(cx).is_none(),
        "the caret is in the tag's block, so a click there edits the tag"
    );

    cx.simulate_keystrokes(notebook.window.into(), "g g");
    wait_until(cx, "a clickable tag", |cx| tag_bounds(cx).is_some());
    let tag = tag_bounds(cx).unwrap();
    VisualTestContext::from_window(notebook.window.into(), cx)
        .simulate_click(tag.center(), Default::default());
    wait_until(cx, "the tag page", |cx| {
        read(&notebook, cx, |view, _| view.nav.workspace.active.clone())
            == Some(quire_core::workspace::Destination::Tag {
                tag: "#Plan/sub".into(),
            })
    });
}

#[gpui::test]
fn hovering_a_page_link_previews_the_page_it_names(cx: &mut TestAppContext) {
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
    let link = visual
        .debug_bounds("quire-web-link")
        .expect("a rendered page link");
    visual.simulate_mouse_move(link.center(), None, gpui::Modifiers::none());
    wait_until(cx, "the page preview", |cx| {
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(100));
        draw(&notebook, cx);
        VisualTestContext::from_window(notebook.window.into(), cx)
            .debug_bounds("quire-page-preview")
            .is_some()
    });

    let index = read(&notebook, cx, |view, cx| view.index_client(cx)).unwrap();
    let content = |target: &str, cx: &mut TestAppContext| {
        let tooltip = cx.new(|cx| {
            crate::references::preview::PagePreviewTooltip::new(Some(&index), target.into(), cx)
        });
        let mut shown = (String::new(), Vec::new());
        wait_until(cx, "the preview's answer", |cx| {
            let (title, lines) = cx.read(|cx| tooltip.read(cx).content());
            shown = (
                title.to_string(),
                lines.iter().map(ToString::to_string).collect(),
            );
            shown.1 != ["Loading…"]
        });
        shown
    };
    assert_eq!(
        content("Target", cx),
        ("Target".to_owned(), vec!["target text".to_owned()])
    );
    assert_eq!(
        content("Nowhere", cx),
        (
            "Nowhere".to_owned(),
            vec!["No page has this name yet. Follow the link to create it.".to_owned()]
        )
    );
}

/// A link in a drawn table opens its page, as rt's rendered table does, while a press elsewhere
/// in the table shows its source for editing.
#[gpui::test]
fn a_link_in_a_drawn_table_opens_its_page(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            (
                "h",
                "Home.md",
                "- first\n\n| Page | Note |\n| --- | --- |\n| [[Target]] | **bold** |\n",
            ),
            ("t", "Target.md", "- target text\n"),
        ],
        "h",
        cx,
    );
    show_home_in_write(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "g g");
    let link = |cx: &mut TestAppContext| {
        draw(&notebook, cx);
        VisualTestContext::from_window(notebook.window.into(), cx)
            .debug_bounds("quire-table-link-2")
    };
    wait_until(cx, "a drawn table link", |cx| link(cx).is_some());
    let at = link(cx).unwrap();
    VisualTestContext::from_window(notebook.window.into(), cx)
        .simulate_click(at.center(), Default::default());
    wait_until(cx, "the linked page", |cx| active(&notebook, cx) == "t");
}
