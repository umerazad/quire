use super::super::*;
use super::links_tests::set_vim;
use super::tests::{Notebook, active, draw, open_notebook, read, wait_until};
use gpui::TestAppContext;

const SOURCE: &str = "- Original ^shared\n  - Child\n";
const OTHER: &str = "- intro\n- {{embed: ((shared))}}\n- ![[Source]]\n- ((missing))\n";

fn host_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn nested(editor: &Entity<editor::Editor>, cx: &mut TestAppContext) -> Vec<Entity<editor::Editor>> {
    cx.update(|cx| crate::embeds::nested_editors(editor.read(cx)))
}

fn stored_text(notebook: &Notebook, page: &str) -> String {
    let store = quire_store::notebook::NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    store
        .checkout(page, None)
        .unwrap()
        .selected
        .unwrap()
        .root()
        .field("text")
        .as_str()
        .unwrap()
        .to_owned()
}

fn display(notebook: &Notebook, host: &Entity<editor::Editor>, cx: &mut TestAppContext) -> String {
    notebook
        .window
        .update(cx, |_, window, cx| {
            host.update(cx, |host, cx| {
                host.snapshot(window, cx).display_snapshot.text()
            })
        })
        .unwrap()
}

fn toggle_mode(notebook: &Notebook, cx: &mut TestAppContext) {
    let mode = if cfg!(target_os = "macos") {
        "cmd-shift-m"
    } else {
        "ctrl-shift-m"
    };
    cx.simulate_keystrokes(notebook.window.into(), mode);
}

/// Opens `page` in Write with the caret on its first line, and waits for `count` nested editors.
fn open_in_write(
    pages: &[(&str, &str, &str)],
    page: &str,
    count: usize,
    cx: &mut TestAppContext,
) -> (Notebook, Entity<editor::Editor>) {
    let notebook = open_notebook(pages, page, cx);
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
    toggle_mode(&notebook, cx);
    let editor = host_editor(&notebook, cx);
    wait_until(cx, "the embeds", |cx| {
        draw(&notebook, cx);
        nested(&editor, cx).len() == count
    });
    (notebook, editor)
}

fn edit_embedded_block(vim: bool, cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    if !vim {
        set_vim(false, cx);
    }
    let editors = nested(&host, cx);
    cx.update(|cx| {
        assert_eq!(
            editors[0].read(cx).text(cx),
            "- Original ^shared\n  - Child"
        );
        assert_eq!(editors[1].read(cx).text(cx), SOURCE);
    });
    wait_until(cx, "the embed headers", |cx| {
        draw(&notebook, cx);
        cx.update(|cx| crate::embeds::header_labels(host.read(cx)))
            == [
                "↳ Embedded block from Source ↗",
                "↳ Embedded page Source ↗",
                "((missing)) · unresolved. Check the reference in Source.",
            ]
    });
    assert_eq!(
        display(&notebook, &host, cx),
        "intro\n↳\n\n\n↳\n\n\n\n↳\n",
        "each reference folds into its header, with its nested editor's rows below"
    );
    assert!(
        gpui::VisualTestContext::from_window(notebook.window.into(), cx)
            .debug_bounds("quire-embed")
            .is_some()
    );
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editors[0].focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    if vim {
        cx.simulate_keystrokes(notebook.window.into(), "I N e w space escape");
    } else {
        cx.simulate_keystrokes(notebook.window.into(), "home");
        cx.simulate_input(notebook.window.into(), "New ");
    }
    let edited = "- New Original ^shared\n  - Child\n";
    cx.update(|cx| {
        assert_eq!(
            editors[1].read(cx).text(cx),
            edited,
            "both embeds share one buffer"
        );
        assert_eq!(host.read(cx).text(cx), OTHER, "the host page is unchanged");
    });
    wait_until(cx, "the edit to reach the source page's store head", |_| {
        stored_text(&notebook, "s") == edited
    });
    assert_eq!(stored_text(&notebook, "o"), OTHER);

    let undo = if vim {
        "u"
    } else if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };
    cx.simulate_keystrokes(notebook.window.into(), undo);
    cx.update(|cx| assert_eq!(editors[1].read(cx).text(cx), SOURCE));
    wait_until(cx, "the undo to reach the source page's store head", |_| {
        stored_text(&notebook, "s") == SOURCE
    });
}

#[gpui::test]
fn vim_edits_in_a_block_embed_persist_through_the_source_page(cx: &mut TestAppContext) {
    edit_embedded_block(true, cx);
}

#[gpui::test]
fn edits_in_a_block_embed_persist_through_the_source_page_with_vim_off(cx: &mut TestAppContext) {
    edit_embedded_block(false, cx);
}

#[gpui::test]
fn the_caret_reveals_an_embed_reference_its_header_opens_the_source_and_source_view_drops_embeds(
    cx: &mut TestAppContext,
) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j");
    wait_until(cx, "the revealed reference", |cx| {
        draw(&notebook, cx);
        display(&notebook, &host, cx) == "intro\n{{embed: ((shared))}}\n\n\n↳\n\n\n\n↳\n"
    });
    cx.simulate_keystrokes(notebook.window.into(), "k");
    wait_until(cx, "the folded header", |cx| {
        draw(&notebook, cx);
        display(&notebook, &host, cx) == "intro\n↳\n\n\n↳\n\n\n\n↳\n"
    });

    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    let header = visual
        .debug_bounds("quire-embed-header")
        .expect("a rendered embed header");
    visual.simulate_click(header.center(), Default::default());
    wait_until(cx, "the embedded block's page", |cx| {
        active(&notebook, cx) == "s"
    });

    let back = if cfg!(target_os = "macos") {
        "cmd-["
    } else {
        "alt-left"
    };
    cx.simulate_keystrokes(notebook.window.into(), back);
    wait_until(cx, "the embedding page", |cx| active(&notebook, cx) == "o");
    let host = host_editor(&notebook, cx);
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    if !nested(&host, cx).is_empty() {
        toggle_mode(&notebook, cx);
    }
    wait_until(cx, "Source view without embeds", |cx| {
        draw(&notebook, cx);
        nested(&host, cx).is_empty() && display(&notebook, &host, cx) == OTHER
    });
}

#[gpui::test]
fn an_embed_of_its_own_ancestor_stops_at_a_recursion_notice(cx: &mut TestAppContext) {
    let pages = [(
        "c",
        "Cycle.md",
        "- Parent ^cycle\n  - {{embed: ((cycle))}}\n",
    )];
    let (notebook, host) = open_in_write(&pages, "c", 1, cx);
    let inner = nested(&host, cx).remove(0);
    wait_until(cx, "the recursion notice", |cx| {
        draw(&notebook, cx);
        cx.update(|cx| crate::embeds::shows_recursion(inner.read(cx)))
    });
    assert!(nested(&inner, cx).is_empty());
    cx.update(|cx| {
        assert_eq!(
            inner.read(cx).text(cx),
            "- Parent ^cycle\n  - {{embed: ((cycle))}}"
        )
    });
}
