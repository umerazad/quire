use super::move_tests::stored;
use super::tests::{Notebook, active, draw, open_notebook, read, wait_until};
use crate::block_move::MoveBlockToPage;
use crate::navigation::palette::PaletteItem;
use gpui::TestAppContext;
use quire_store::notebook::NotebookStore;

const PAGES: [(&str, &str, &str); 4] = [
    (
        "s",
        "Source.md",
        "- Keep ^keep\n- Move ^m\n  - Child ^c\n- After\n",
    ),
    ("d", "Dest.md", "# Dest\n"),
    (
        "r",
        "Refs.md",
        "- [[Source#^m]] ((c)) [[Source#^keep]] [[Source#^c|kid]]\n",
    ),
    ("o", "Other.md", "- unrelated\n"),
];

fn choose(notebook: &Notebook, query: &str, page: &str, cx: &mut TestAppContext) {
    cx.simulate_input(notebook.window.into(), query);
    wait_until(cx, "the destination in the palette", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav.palette.as_ref().is_some_and(|picker| {
                let picker = picker.read(cx);
                picker.delegate.settled()
                    && matches!(
                        picker.delegate.items().first(),
                        Some(PaletteItem::Page { page: found, .. }) if found == page
                    )
            })
        })
    });
    cx.simulate_keystrokes(notebook.window.into(), "enter");
}

#[gpui::test]
fn moving_a_block_takes_its_children_and_retargets_page_references(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j");
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    assert!(read(&notebook, cx, |view, _| view.nav.palette.is_some()));
    choose(&notebook, "Dest", "d", cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    wait_until(cx, "the move", |_| stored(&store, "d").1.contains("Move"));
    assert_eq!(
        stored(&store, "s").1,
        "- Keep ^keep\n- After\n",
        "the block and its child leave the source"
    );
    assert_eq!(stored(&store, "d").1, "# Dest\n\n- Move ^m\n  - Child ^c\n");
    assert_eq!(
        stored(&store, "r").1,
        "- [[Dest#^m]] ((c)) [[Source#^keep]] [[Dest#^c|kid]]\n"
    );
    assert_eq!(stored(&store, "o").1, "- unrelated\n");
    wait_until(cx, "the destination to open", |cx| {
        active(&notebook, cx) == "d"
    });
    read(&notebook, cx, |view, _| assert!(view.nav.error.is_none()));
}

#[gpui::test]
fn a_block_without_an_id_gets_one_and_the_same_page_is_refused(cx: &mut TestAppContext) {
    let pages = [
        ("s", "Source.md", "- Plain\n- Other\n"),
        ("d", "Dest.md", ""),
    ];
    let notebook = open_notebook(&pages, "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    choose(&notebook, "Source", "s", cx);
    wait_until(cx, "the refusal", |cx| {
        read(&notebook, cx, |view, _| view.nav.error.is_some())
    });
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.error.as_deref(),
            Some(crate::block_move::SAME_PAGE)
        )
    });
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(stored(&store, "d").1, "", "a refused move writes nothing");

    draw(&notebook, cx);
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    choose(&notebook, "Dest", "d", cx);
    wait_until(cx, "the move", |_| stored(&store, "d").1.contains("Plain"));
    let moved = stored(&store, "d").1;
    let id = moved
        .strip_prefix("- Plain ^")
        .and_then(|rest| rest.strip_suffix('\n'))
        .expect("the moved block keeps the ID it was given");
    assert!(!id.is_empty());
    assert_eq!(stored(&store, "s").1, "- Other\n");
}
