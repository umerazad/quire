use super::super::*;
use super::tests::{
    Notebook, active, caret_text, draw, key, open_notebook, read, save, wait_until,
};
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
        rows.iter().any(|row| matches!(
            row,
            ContextRow::Child { page, depth: 1, text, .. }
                if page == "notes" && text == "she asked about pricing #question"
        )),
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

#[gpui::test]
fn opening_a_reference_reveals_its_block(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PEOPLE, "ana", cx);
    let panel = panel(&notebook, cx);
    // The panel can settle on an answer from before the last page was analyzed; the index
    // announces the change once it has sealed, so wait for the row rather than the first answer.
    let mut index = None;
    wait_until(cx, "the deck reference", |cx| {
        index = cx.read(|cx| panel.read(cx).rows()).iter().position(
            |row| matches!(row, ContextRow::Block { text, .. } if text == "[[Ana]] sent the deck"),
        );
        index.is_some()
    });
    let index = index.expect("the wait found the row");
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_references(window, cx))
        .unwrap();
    let keys = vec!["down"; index].join(" ");
    cx.simulate_keystrokes(notebook.window.into(), &keys);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the source page's editor", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page.read(cx).id() == "notes" && view.editor.is_some() && view.nav.jump.is_none()
        })
    });
    assert_eq!(caret_text(&notebook, false, cx), "[[Ana]] sent the deck");
}

#[gpui::test]
fn linked_tasks_filter_above_references_tick_and_open(cx: &mut TestAppContext) {
    use quire_core::workspace::TaskStatus;
    let notebook = open_notebook(
        &[
            ("ana", "Ana.md", "- [ ] Own task [[Ana]]\n"),
            (
                "work",
                "Work.md",
                "- Call [[Ana]]\n  - [ ] Book the venue\n- [x] Sent [[Ana]] the deck\n- [ ] Unrelated\n",
            ),
        ],
        "ana",
        cx,
    );
    let list = read(&notebook, cx, |view, cx| {
        view.shown_linked_tasks(cx).unwrap()
    });
    wait_until(cx, "the linked tasks", |cx| {
        cx.read(|cx| list.read(cx).tasks.value.len() == 2)
    });
    let texts = |cx: &mut TestAppContext| -> Vec<String> {
        cx.read(|cx| {
            list.read(cx)
                .shown()
                .into_iter()
                .map(|task| task.text)
                .collect()
        })
    };
    assert_eq!(
        texts(cx),
        ["Book the venue", "Sent [[Ana]] the deck"],
        "a parent's link counts and the page's own tasks do not"
    );
    list.update(cx, |list, cx| list.set_status(TaskStatus::Open, cx));
    draw(&notebook, cx);
    assert!(cx.read(|cx| list.read(cx).lists_tasks()));
    assert_eq!(texts(cx), ["Book the venue"]);
    list.update(cx, |list, cx| list.toggle(0, cx));
    wait_until(cx, "the ticked task", |_| {
        super::tags_tests::stored(&notebook, "work").contains("- [x] Book the venue")
    });
    wait_until(cx, "the ticked task listed as done", |cx| {
        cx.read(|cx| {
            list.read(cx).tasks.value.iter().any(|task| {
                task.text == "Book the venue" && task.state == quire_core::tasks::TaskState::Done
            })
        })
    });
    assert_eq!(cx.read(|cx| list.read(cx).tasks.value.len()), 2);
    assert!(
        texts(cx).is_empty(),
        "Open lists no task once both are done"
    );

    list.update(cx, |list, cx| list.set_status(TaskStatus::Done, cx));
    notebook
        .window
        .update(cx, |_, window, cx| window.focus(&list.focus_handle(cx), cx))
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "down enter");
    wait_until(cx, "the done task's page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page.read(cx).id() == "work" && view.editor.is_some() && view.nav.jump.is_none()
        })
    });
    assert_eq!(
        caret_text(&notebook, false, cx),
        "- [x] Sent [[Ana]] the deck"
    );
}

fn side_page(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
    read(notebook, cx, |view, cx| {
        view.nav
            .side
            .as_ref()
            .filter(|side| side.editor.is_some())
            .map(|side| side.page.read(cx).id().to_owned())
    })
}

#[gpui::test]
fn the_side_pane_shows_its_own_pages_linked_references(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PEOPLE, "launch", cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_beside("ana".into(), window, cx)
        })
        .unwrap();
    wait_until(cx, "the side panel", |cx| {
        read(&notebook, cx, |view, cx| {
            view.side_references(cx)
                .is_some_and(|(_, panel)| panel.read(cx).results.settled())
        })
    });
    let panel = read(&notebook, cx, |view, cx| {
        view.side_references(cx).unwrap().1
    });
    let rows = cx.read(|cx| panel.read(cx).rows());
    assert_eq!(
        cx.read(|cx| panel.read(cx).results.value.total),
        3,
        "the side pane lists the references to Ana, not to Launch"
    );
    draw(&notebook, cx);
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    assert!(visual.debug_bounds("quire-side-references").is_some());
    let index = rows
        .iter()
        .position(
            |row| matches!(row, ContextRow::Block { text, .. } if text == "[[Ana]] sent the deck"),
        )
        .unwrap();
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&panel.focus_handle(cx), cx)
        })
        .unwrap();
    let window = notebook.window.into();
    cx.simulate_keystrokes(window, "escape");
    cx.simulate_keystrokes(window, &vec!["down"; index].join(" "));
    cx.simulate_keystrokes(window, "enter");
    wait_until(cx, "the source in the side pane", |cx| {
        side_page(&notebook, cx).as_deref() == Some("notes")
    });
    assert_eq!(active(&notebook, cx), "launch", "the main pane stays");
    assert_eq!(caret_text(&notebook, true, cx), "[[Ana]] sent the deck");
    assert!(
        read(&notebook, cx, |view, cx| view.side_references(cx)).is_some(),
        "the side pane now lists the references to Notes"
    );

    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_beside("ana".into(), window, cx)
        })
        .unwrap();
    wait_until(cx, "Ana beside again", |cx| {
        side_page(&notebook, cx).as_deref() == Some("ana")
            && read(&notebook, cx, |view, cx| {
                view.side_references(cx)
                    .is_some_and(|(_, panel)| panel.read(cx).results.settled())
            })
    });
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&panel.focus_handle(cx), cx)
        })
        .unwrap();
    cx.simulate_keystrokes(window, "escape");
    cx.simulate_keystrokes(window, &vec!["down"; index].join(" "));
    cx.simulate_keystrokes(window, "shift-enter");
    wait_until(cx, "the source in the main pane", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page.read(cx).id() == "notes" && view.editor.is_some()
        })
    });
    assert_eq!(side_page(&notebook, cx).as_deref(), Some("ana"));
}
