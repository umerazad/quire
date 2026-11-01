use super::links_tests::set_vim;
use super::tests::{Notebook, draw, key, open_notebook, read, settle, wait_until};
use editor::Editor;
use gpui::{ClipboardItem, Entity, Focusable as _, TestAppContext};
use multi_buffer::MultiBufferOffset;

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

const SOURCE: &str = "- Moved ^m\n  - see ((m)) and [[Source#^m]]\n- Stay\n";
const OTHER: &str = "- Links ((m))\n- End\n";
const MOVED: &str = "- Moved ^m\n  - see ((m)) and [[Source#^m]]\n";

fn main_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn side_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Option<Entity<Editor>> {
    read(notebook, cx, |view, cx| {
        let side = view.nav.side.as_ref()?.editor.clone()?;
        Some(side.read(cx).source.clone())
    })
}

/// Opens `first` in Source with Vim off, so selections name exact source text.
fn open_pages(first: &str, cx: &mut TestAppContext) -> (Notebook, Entity<Editor>) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let notebook = open_notebook(&pages, first, cx);
    set_vim(false, cx);
    let editor = main_editor(&notebook, cx);
    (notebook, editor)
}

/// Shows the editor's page in Write, the view whose pastes copy blocks.
fn write_view(notebook: &Notebook, editor: &Entity<Editor>, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |_, window, cx| {
            crate::editor_projection::bind(editor, window, cx).unwrap();
        })
        .unwrap();
    draw(notebook, cx);
}

fn select(
    notebook: &Notebook,
    editor: &Entity<Editor>,
    range: std::ops::Range<usize>,
    cx: &mut TestAppContext,
) {
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([
                        MultiBufferOffset(range.start)..MultiBufferOffset(range.end)
                    ])
                })
            });
        })
        .unwrap();
    draw(notebook, cx);
}

fn source(editor: &Entity<Editor>, cx: &mut TestAppContext) -> String {
    cx.update(|cx| editor.read(cx).text(cx))
}

fn keys(notebook: &Notebook, name: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(notebook.window.into(), &key(name));
    draw(notebook, cx);
}

/// Cuts `MOVED` from the start of the Source page in `editor`.
fn cut_moved(notebook: &Notebook, editor: &Entity<Editor>, cx: &mut TestAppContext) {
    select(notebook, editor, 0..MOVED.len(), cx);
    keys(notebook, "x", cx);
    assert_eq!(source(editor, cx), "- Stay\n");
}

/// Pastes at the end of the last line of the page in `editor`, in Write.
fn paste_at_end(notebook: &Notebook, editor: &Entity<Editor>, cx: &mut TestAppContext) {
    write_view(notebook, editor, cx);
    let end = source(editor, cx).trim_end().len();
    select(notebook, editor, end..end, cx);
    keys(notebook, "v", cx);
}

/// Waits until the index has read every saved page and the follow-up ID checks have answered.
fn indexed(notebook: &Notebook, cx: &mut TestAppContext) {
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    wait_until(cx, "the pasted page in the index", |cx| {
        draw(notebook, cx);
        futures::executor::block_on(crate::query_index::ask(client.resolve_block("m".into())))
            .ok()
            .flatten()
            .is_some_and(|page| page.path == "Other.md")
    });
    settle(notebook, cx);
}

/// Asserts `text` has the moved block once, with its ID and the references inside it.
fn assert_moved(text: &str) {
    assert_eq!(text.matches(" ^m\n").count(), 1, "{text}");
    assert!(text.contains("Moved ^m\n"), "{text}");
    assert!(text.contains("see ((m)) and [[Source#^m]]"), "{text}");
    assert!(!text.contains("^b-"), "a moved block keeps its ID: {text}");
}

/// The fresh ID of the pasted copy in `text`, after asserting the references inside the copy
/// point at it.
fn assert_copied(text: &str) -> String {
    let at = text.find("^b-").expect("a copy with a fresh ID") + 1;
    let id = text[at..].split_whitespace().next().unwrap().to_owned();
    assert!(
        text.contains(&format!("see (({id})) and [[Source#^{id}]]")),
        "references inside the copy point at the copy: {text}"
    );
    id
}

#[gpui::test]
fn a_copy_pasted_on_its_page_takes_fresh_ids_and_its_references_follow_it(cx: &mut TestAppContext) {
    let (notebook, editor) = open_pages("s", cx);
    select(&notebook, &editor, 0..MOVED.len(), cx);
    keys(&notebook, "c", cx);
    paste_at_end(&notebook, &editor, cx);
    let pasted = source(&editor, cx);
    assert!(pasted.starts_with(MOVED), "the original stays: {pasted}");
    assert_eq!(pasted.matches(" ^m\n").count(), 1, "{pasted}");
    assert_copied(&pasted);
    keys(&notebook, "z", cx);
    assert_eq!(source(&editor, cx), SOURCE, "one undo removes the copy");
}

#[gpui::test]
fn a_cut_pasted_on_its_page_keeps_its_ids_and_a_second_paste_copies(cx: &mut TestAppContext) {
    let (notebook, editor) = open_pages("s", cx);
    cut_moved(&notebook, &editor, cx);
    paste_at_end(&notebook, &editor, cx);
    let moved = source(&editor, cx);
    assert_moved(&moved);
    keys(&notebook, "v", cx);
    let twice = source(&editor, cx);
    assert!(twice.contains("Moved ^m\n"), "{twice}");
    assert_copied(&twice);
    keys(&notebook, "z", cx);
    assert_eq!(source(&editor, cx), moved);
    keys(&notebook, "z", cx);
    keys(&notebook, "z", cx);
    assert_eq!(source(&editor, cx), SOURCE, "undo restores the cut block");
}

#[gpui::test]
fn a_cut_pasted_on_another_page_moves_its_block_and_keeps_its_ids(cx: &mut TestAppContext) {
    let (notebook, editor) = open_pages("s", cx);
    cut_moved(&notebook, &editor, cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("o".into(), false, window, cx)
        })
        .unwrap();
    wait_until(cx, "the other page", |cx| {
        source(&main_editor(&notebook, cx), cx) == OTHER
    });
    let other = main_editor(&notebook, cx);
    paste_at_end(&notebook, &other, cx);
    assert_moved(&source(&other, cx));
    indexed(&notebook, cx);
    assert_moved(&source(&other, cx));
}

#[gpui::test]
fn a_cut_pasted_in_the_other_pane_moves_its_block_and_undo_restores_both(cx: &mut TestAppContext) {
    let (notebook, editor) = open_pages("s", cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("o".into(), true, window, cx)
        })
        .unwrap();
    wait_until(cx, "the side pane's editor", |cx| {
        side_editor(&notebook, cx).is_some()
    });
    let side = side_editor(&notebook, cx).unwrap();
    cut_moved(&notebook, &editor, cx);
    paste_at_end(&notebook, &side, cx);
    assert_moved(&source(&side, cx));
    indexed(&notebook, cx);
    assert_moved(&source(&side, cx));
    keys(&notebook, "z", cx);
    assert_eq!(
        source(&side, cx),
        OTHER,
        "undo in the pane removes the paste"
    );
    select(&notebook, &editor, 0..0, cx);
    keys(&notebook, "z", cx);
    assert_eq!(source(&editor, cx), SOURCE, "undo restores the cut block");
}

#[gpui::test]
fn a_cut_pasted_while_its_page_still_has_the_block_is_a_copy(cx: &mut TestAppContext) {
    let (notebook, editor) = open_pages("s", cx);
    cut_moved(&notebook, &editor, cx);
    keys(&notebook, "z", cx);
    assert_eq!(source(&editor, cx), SOURCE);
    paste_at_end(&notebook, &editor, cx);
    let pasted = source(&editor, cx);
    assert!(pasted.starts_with(MOVED), "{pasted}");
    assert_copied(&pasted);
}

#[gpui::test]
fn a_vim_delete_put_on_another_open_page_keeps_its_id(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let notebook = open_notebook(&pages, "s", cx);
    let editor = main_editor(&notebook, cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("o".into(), true, window, cx)
        })
        .unwrap();
    wait_until(cx, "the side pane's editor", |cx| {
        side_editor(&notebook, cx).is_some()
    });
    let side = side_editor(&notebook, cx).unwrap();
    select(&notebook, &editor, 0..0, cx);
    cx.simulate_keystrokes(notebook.window.into(), "d d");
    select(&notebook, &side, 0..0, cx);
    cx.simulate_keystrokes(notebook.window.into(), "p");
    let put = "- Links ((m))\n- Moved ^m\n- End\n";
    assert_eq!(source(&side, cx), put);
    indexed(&notebook, cx);
    assert_eq!(
        source(&side, cx),
        put,
        "the index still naming the source page does not renumber a moved block"
    );
}
