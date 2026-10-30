use super::super::*;
use super::tests::{Notebook, active, key, open_notebook, read, save, wait_until};
use crate::navigation::{context_rows::ContextRow, references::ReferencesView};
use gpui::TestAppContext;
use quire_core::reference_filter::UNCLOSED_QUOTE;
use quire_store::notebook::NotebookStore;

const PEOPLE: [(&str, &str, &str); 3] = [
    ("ana", "Ana.md", "- A person\n"),
    (
        "launch",
        "Launch.md",
        "- #project\n- [[Ana]] owns the venue #waiting\n",
    ),
    (
        "notes",
        "Notes.md",
        "- Met [[Ana]] #meeting\n  - she asked about pricing #question\n- [[Ana]] sent the deck\n",
    ),
];

fn panel(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<ReferencesView> {
    wait_until(cx, "the linked references", |cx| {
        read(notebook, cx, |view, cx| {
            view.shown_references(cx)
                .is_some_and(|panel| panel.read(cx).results.settled())
        })
    });
    read(notebook, cx, |view, cx| view.shown_references(cx).unwrap())
}

/// Waits until the panel answers its latest filter, then returns the block texts shown and
/// the total.
fn blocks(notebook: &Notebook, cx: &mut TestAppContext) -> (Vec<String>, usize) {
    let panel = panel(notebook, cx);
    cx.read(|cx| {
        let panel = panel.read(cx);
        let blocks = panel
            .rows()
            .into_iter()
            .filter_map(|row| match row {
                ContextRow::Block { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        (blocks, panel.results.value.total)
    })
}

#[gpui::test]
fn linked_references_filter_by_typed_terms_and_chips(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PEOPLE, "ana", cx);
    let (shown, total) = blocks(&notebook, cx);
    assert_eq!(total, 3);
    assert_eq!(
        shown,
        [
            "[[Ana]] owns the venue #waiting",
            "Met [[Ana]] #meeting",
            "[[Ana]] sent the deck"
        ]
    );
    let panel = panel(&notebook, cx);
    let rows = cx.read(|cx| panel.read(cx).rows());
    assert!(
        rows.contains(&ContextRow::Child {
            page: "notes".into(),
            depth: 1,
            text: "she asked about pricing #question".into()
        }),
        "nested blocks show under their reference: {rows:?}"
    );

    cx.simulate_keystrokes(notebook.window.into(), &key("shift-r"));
    cx.simulate_input(notebook.window.into(), "pricing");
    assert_eq!(
        blocks(&notebook, cx),
        (vec!["Met [[Ana]] #meeting".to_owned()], 3),
        "a word in a nested block keeps its reference; typing each letter never leaves an older result"
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    assert_eq!(cx.read(|cx| panel.read(cx).text(cx)), "");
    assert_eq!(blocks(&notebook, cx).0.len(), 3);

    let launch = cx.read(|cx| {
        panel
            .read(cx)
            .results
            .value
            .facets
            .iter()
            .find(|facet| facet.label == "Launch")
            .map(|facet| facet.term.clone())
            .unwrap()
    });
    notebook
        .window
        .update(cx, |_, window, cx| {
            panel.update(cx, |panel, cx| {
                panel.toggle_facet(&launch, true, window, cx)
            })
        })
        .unwrap();
    assert_eq!(cx.read(|cx| panel.read(cx).text(cx)), "-[[Launch]]");
    assert_eq!(
        blocks(&notebook, cx).0,
        ["Met [[Ana]] #meeting", "[[Ana]] sent the deck"]
    );

    notebook
        .window
        .update(cx, |_, window, cx| {
            panel.update(cx, |panel, cx| panel.set_filter("#nothing", window, cx))
        })
        .unwrap();
    assert_eq!(blocks(&notebook, cx), (Vec::new(), 3));

    notebook
        .window
        .update(cx, |_, window, cx| {
            panel.update(cx, |panel, cx| panel.set_filter("\"open", window, cx))
        })
        .unwrap();
    assert_eq!(
        cx.read(|cx| panel.read(cx).parsed(cx)),
        Err(UNCLOSED_QUOTE),
        "the parse error shows inline"
    );
    assert_eq!(
        blocks(&notebook, cx).0.len(),
        3,
        "an unreadable filter hides nothing"
    );
}

#[gpui::test]
fn linked_references_order_daily_pages_follow_saves_and_open_sources(cx: &mut TestAppContext) {
    let pages = [
        ("ana", "Ana.md", "- A person\n"),
        ("early", "Daily/2026-10-01.md", "- [[Ana]] early\n"),
        ("late", "Daily/2026-10-05.md", "- [[Ana]] late\n"),
        ("other", "Other.md", "- [[Ana]] undated\n"),
    ];
    let notebook = open_notebook(&pages, "ana", cx);
    assert_eq!(
        blocks(&notebook, cx).0,
        ["[[Ana]] late", "[[Ana]] early", "[[Ana]] undated"],
        "newest daily pages first, then other pages"
    );
    let panel = panel(&notebook, cx);
    panel.update(cx, |_, cx| {
        cx.emit(crate::navigation::references::ReferencesEvent::Order { oldest: true })
    });
    assert_eq!(
        blocks(&notebook, cx).0,
        ["[[Ana]] early", "[[Ana]] late", "[[Ana]] undated"]
    );
    assert_eq!(
        read(&notebook, cx, |view, _| view
            .nav
            .workspace
            .oldest_first
            .clone()),
        ["ana"],
        "the order is kept per page in the workspace"
    );

    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&store, "new", "New.md", "- [[Ana]] arrives later\n");
    wait_until(cx, "the new reference", |cx| blocks(&notebook, cx).1 == 4);

    notebook
        .window
        .update(cx, |view, window, cx| view.focus_references(window, cx))
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "down shift-enter");
    wait_until(cx, "the page beside", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.page.read(cx).id() == "early")
        })
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_references(window, cx))
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    assert_eq!(active(&notebook, cx), "early");
}
