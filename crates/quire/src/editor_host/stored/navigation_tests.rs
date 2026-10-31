use super::super::*;
use crate::navigation::palette::PaletteItem;
use gpui::{TestAppContext, WindowHandle};
use quire_core::workspace::Destination;
use quire_store::notebook::{Edit, NotebookStore};
use std::time::{Duration, Instant};

pub(super) struct Notebook {
    pub _root: tempfile::TempDir,
    pub path: PathBuf,
    pub window: WindowHandle<StoredWindow>,
}

pub(super) fn key(name: &str) -> String {
    if cfg!(target_os = "macos") {
        format!("cmd-{name}")
    } else {
        format!("ctrl-{name}")
    }
}

pub(super) fn save(store: &NotebookStore, page: &str, path: &str, text: &str) {
    store
        .save(&Edit {
            page: page.into(),
            base: store.heads(page).unwrap(),
            path: path.into(),
            text: text.into(),
            deleted: false,
            created: 1,
            recovery: None,
        })
        .unwrap();
}

/// Runs the executor until `done` holds, letting the storage and index threads answer.
pub(super) fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    cx.executor().allow_parking();
    let deadline = Instant::now() + crate::test_patience::PATIENCE;
    loop {
        cx.executor().advance_clock(Duration::from_millis(200));
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

pub(super) fn open_notebook(
    pages: &[(&str, &str, &str)],
    first: &str,
    cx: &mut TestAppContext,
) -> Notebook {
    cx.update(|cx| init(cx).unwrap());
    open_initialized(pages, first, cx)
}

/// Opens a notebook in an app whose editor host is already initialized.
pub(super) fn open_initialized(
    pages: &[(&str, &str, &str)],
    first: &str,
    cx: &mut TestAppContext,
) -> Notebook {
    open_configured(pages, first, |_| {}, cx)
}

/// Opens a notebook whose window `configure` adjusts before its settings and index load.
pub(super) fn open_configured(
    pages: &[(&str, &str, &str)],
    first: &str,
    configure: impl FnOnce(&mut StoredWindow),
    cx: &mut TestAppContext,
) -> Notebook {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    for (page, file, text) in pages {
        save(&store, page, file, text);
    }
    drop(store);
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| session.open_page(first.into(), cx));
    let window = cx.add_window(|window, cx| {
        let mut view =
            StoredWindow::new(session, page.clone(), path.clone(), "nb".into(), window, cx);
        configure(&mut view);
        view
    });
    let notebook = Notebook {
        _root: root,
        path,
        window,
    };
    settle(&notebook, cx);
    notebook
}

pub(super) fn settle(notebook: &Notebook, cx: &mut TestAppContext) {
    wait_until(cx, "the index to start", |cx| {
        read(notebook, cx, |view, cx| view.index_client(cx).is_some())
    });
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    wait_until(cx, "indexing", |_| {
        futures::executor::block_on(client.unanalyzed().unwrap()).unwrap() == 0
    });
    notebook
        .window
        .update(cx, |view, _, cx| view.refresh_catalogue(cx))
        .unwrap();
    wait_until(cx, "the page list", |cx| {
        read(notebook, cx, |view, _| view.nav.catalogue.is_none())
    });
    draw(notebook, cx);
}

pub(super) fn draw(notebook: &Notebook, cx: &mut TestAppContext) {
    cx.update_window(notebook.window.into(), |_, window, cx| {
        window.draw(cx).clear(cx)
    })
    .unwrap();
}

pub(super) fn read<T>(
    notebook: &Notebook,
    cx: &mut TestAppContext,
    f: impl FnOnce(&StoredWindow, &App) -> T,
) -> T {
    notebook
        .window
        .update(cx, |view, _, cx| f(view, cx))
        .unwrap()
}

pub(super) fn active(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    read(notebook, cx, |view, cx| view.page.read(cx).id().to_owned())
}

/// The text after the caret on its line in the Write editor of the main page, or of the side
/// pane when `side`.
pub(super) fn caret_text(notebook: &Notebook, side: bool, cx: &mut TestAppContext) -> String {
    let editor = read(notebook, cx, |view, cx| {
        let window = match side {
            true => view.nav.side.as_ref().and_then(|side| side.editor.clone()),
            false => view.editor.clone(),
        };
        window.map(|window| window.read(cx).source.clone())
    })
    .expect("the page's editor is showing");
    editor.update(cx, |editor, cx| {
        let caret = editor
            .selections
            .newest::<editor::MultiBufferOffset>(&editor.display_snapshot(cx))
            .head();
        let text = editor.text(cx);
        text[caret.0..]
            .lines()
            .next()
            .unwrap_or_default()
            .to_owned()
    })
}

fn palette_items(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<PaletteItem> {
    read(notebook, cx, |view, cx| {
        view.nav
            .palette
            .as_ref()
            .map(|picker| picker.read(cx).delegate.items().to_vec())
            .unwrap_or_default()
    })
}

fn type_in_palette(notebook: &Notebook, text: &str, cx: &mut TestAppContext) {
    cx.simulate_input(notebook.window.into(), text);
    let expected = text.to_owned();
    wait_until(cx, "palette results", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav.palette.as_ref().is_some_and(|picker| {
                let picker = picker.read(cx);
                picker.query(cx) == expected && picker.delegate.settled()
            })
        })
    });
}

pub(super) const PAGES: [(&str, &str, &str); 4] = [
    ("a", "Alpha.md", "- apple pie\n"),
    ("b", "Beta.md", "- banana bread with apple\n"),
    ("d", "Daily/2026-10-30.md", "- worked\n"),
    (
        "n",
        "roam/js/clock.md",
        "---\ntitle: roam/js/clock\n---\n- tick\n",
    ),
];

#[gpui::test]
fn find_palette_filters_titles_and_text_then_opens_the_choice(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    wait_until(cx, "the first page", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    cx.simulate_keystrokes(notebook.window.into(), &key("k"));
    draw(&notebook, cx);
    assert!(read(&notebook, cx, |view, _| view.nav.palette.is_some()));
    type_in_palette(&notebook, "apple", cx);
    let items = palette_items(&notebook, cx);
    assert!(
        matches!(&items[0], PaletteItem::Hit { page, .. } if page == "a" || page == "b"),
        "{items:?}"
    );
    assert!(
        items
            .iter()
            .all(|item| !matches!(item, PaletteItem::Page { .. }))
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    cx.run_until_parked();
    assert!(read(&notebook, cx, |view, _| view.nav.palette.is_none()));

    cx.simulate_keystrokes(notebook.window.into(), &key("k"));
    type_in_palette(&notebook, "bet", cx);
    assert_eq!(
        palette_items(&notebook, cx)[0],
        PaletteItem::Page {
            page: "b".into(),
            title: "Beta".into(),
            path: "Beta.md".into()
        }
    );
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "Beta to open", |cx| active(&notebook, cx) == "b");
    read(&notebook, cx, |view, _| {
        assert!(view.nav.palette.is_none());
        assert_eq!(view.nav.workspace.recent[0], Destination::page("b"));
        assert_eq!(view.nav.workspace.recent[1], Destination::page("a"));
    });

    cx.simulate_keystrokes(notebook.window.into(), &key("k"));
    type_in_palette(&notebook, "2026-10-30", cx);
    assert!(matches!(
        &palette_items(&notebook, cx)[0],
        PaletteItem::Page { page, .. } if page == "d"
    ));
    cx.simulate_keystrokes(notebook.window.into(), "shift-enter");
    wait_until(cx, "the side pane", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.editor.is_some())
        })
    });
    assert_eq!(active(&notebook, cx), "b");
}

#[gpui::test]
fn palette_creates_a_missing_page_and_refuses_a_taken_name(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("n"));
    type_in_palette(&notebook, "Gamma Notes", cx);
    assert_eq!(
        palette_items(&notebook, cx),
        [PaletteItem::Create {
            title: "Gamma Notes".into()
        }]
    );
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the new page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .loaded()
                .is_some_and(|page| page.path.as_deref() == Some("Gamma Notes.md"))
                && view.editor.is_some()
        })
    });
    let created = active(&notebook, cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let checkout = store.checkout(&created, None).unwrap();
    assert_eq!(
        checkout.selected.unwrap().root().field("text").as_str(),
        Some(crate::navigation::NEW_PAGE_TEXT)
    );
    assert_eq!(
        created,
        quire_store::sync::format::page_id_for_path("Gamma Notes.md")
    );

    cx.simulate_keystrokes(notebook.window.into(), &key("n"));
    type_in_palette(&notebook, "alpha", cx);
    assert!(palette_items(&notebook, cx).is_empty());
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.create("ALPHA".into(), String::new(), false, window, cx)
        })
        .unwrap();
    wait_until(cx, "the refusal", |cx| {
        read(&notebook, cx, |view, _| view.nav.error.is_some())
    });
    assert!(
        read(&notebook, cx, |view, _| view.nav.error.clone().unwrap())
            .contains("ALPHA.md already exists")
    );
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.create("bad/name".into(), String::new(), false, window, cx)
        })
        .unwrap();
    wait_until(cx, "the name refusal", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .error
                .as_ref()
                .is_some_and(|error| error.contains("cannot be used"))
        })
    });
}

#[gpui::test]
fn palette_rejects_results_from_superseded_queries(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("k"));
    type_in_palette(&notebook, "be", cx);
    let picker = read(&notebook, cx, |view, _| view.nav.palette.clone().unwrap());
    picker.update(cx, |picker, _| {
        let delegate = &mut picker.delegate;
        let current = delegate.requested();
        let stale = crate::navigation::palette::Results {
            pages: Vec::new(),
            found: None,
            hits: Vec::new(),
        };
        assert!(!delegate.apply(current - 1, u64::MAX, "b", Ok(stale)));
        let older = crate::navigation::palette::Results {
            pages: Vec::new(),
            found: None,
            hits: Vec::new(),
        };
        assert!(!delegate.apply(current, 0, "be", Ok(older)));
        assert!(
            delegate
                .items()
                .iter()
                .any(|item| matches!(item, PaletteItem::Page { page, .. } if page == "b"))
        );
    });
}

fn page_text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let page = read(notebook, cx, |view, _| view.page.clone());
    cx.update(|cx| {
        let analysis = page.read(cx).loaded().unwrap().analysis.clone();
        analysis.read(cx).source.read(cx).text()
    })
}

fn editor_focused(notebook: &Notebook, cx: &mut TestAppContext) -> bool {
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.editor
                .as_ref()
                .is_some_and(|editor| super::holds_focus(editor, window, cx))
        })
        .unwrap()
}

#[gpui::test]
fn vim_insert_keys_outside_a_text_field_go_to_the_page_editor(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    wait_until(cx, "the first page", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-r"));
    draw(&notebook, cx);
    assert!(!editor_focused(&notebook, cx));
    cx.simulate_keystrokes(notebook.window.into(), "shift-a");
    cx.run_until_parked();
    assert!(editor_focused(&notebook, cx), "Insert keys reach the page");
    cx.simulate_input(notebook.window.into(), "!");
    assert_eq!(page_text(&notebook, cx), "- apple pie!\n");

    cx.simulate_keystrokes(notebook.window.into(), "escape");
    cx.simulate_keystrokes(notebook.window.into(), &key("k"));
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i");
    cx.run_until_parked();
    assert!(
        !editor_focused(&notebook, cx),
        "a text field keeps its own typing"
    );
}
