use super::tests::{Notebook, active, draw, key, open_notebook, read, save, wait_until};
use crate::file_actions::STALE_DELETION;
use gpui::TestAppContext;
use quire_core::workspace::Destination;
use quire_store::notebook::NotebookStore;

const PAGES: [(&str, &str, &str); 5] = [
    ("o", "Old.md", "- Original ^stable\n"),
    ("l", "Links.md", "- [[Old]] and ((stable))\n"),
    ("k", "Keep.md", "- keep\n"),
    ("pa", "Projects/A.md", "- links [[Old]] and [[B]]\n"),
    ("pb", "Projects/Deep/B.md", "- deep\n"),
];

fn visit(notebook: &Notebook, destination: Destination, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |view, window, cx| view.visit(destination, window, cx))
        .unwrap();
    cx.run_until_parked();
}

/// The dialog's heading and detail once planning finished.
fn summary(notebook: &Notebook, cx: &mut TestAppContext) -> (String, String, Vec<String>) {
    wait_until(cx, "the deletion summary", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.read(cx).can_confirm())
        })
    });
    read(notebook, cx, |view, cx| {
        let dialog = view.nav.dialog.as_ref().unwrap().read(cx);
        let summary = dialog.summary.clone().unwrap();
        (summary.heading, summary.detail, summary.listed)
    })
}

fn dialog_error(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
    read(notebook, cx, |view, cx| {
        view.nav
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.read(cx).error.clone())
            .map(|error| error.to_string())
    })
}

fn head(store: &NotebookStore, page: &str) -> (bool, String, String) {
    let heads = store.heads(page).unwrap();
    assert_eq!(heads.len(), 1, "{page} must have one head");
    let record = store.get(&heads[0]).unwrap().unwrap();
    let root = record.root();
    (
        root.field("deleted").as_bool().unwrap(),
        root.field("path").as_str().unwrap().to_owned(),
        root.field("text").as_str().unwrap().to_owned(),
    )
}

#[gpui::test]
fn deleting_the_shown_page_names_broken_links_then_closes_and_forgets_it(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    visit(&notebook, Destination::page("o"), cx);
    notebook
        .window
        .update(cx, |view, _, cx| view.toggle_favorite(cx))
        .unwrap();
    wait_until(cx, "Old's editor", |cx| {
        read(&notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == "o"
        })
    });
    draw(&notebook, cx);
    // Unsaved typing is saved first, so the deletion keeps the last text in history.
    cx.simulate_keystrokes(notebook.window.into(), "i x escape");
    cx.dispatch_action(notebook.window.into(), crate::navigation::DeletePage);
    let (heading, detail, listed) = summary(&notebook, cx);
    assert_eq!(heading, "Delete Old?");
    assert_eq!(
        detail,
        "3 links from other pages will point to a missing page. Every computer on this notebook removes it too."
    );
    assert!(listed.is_empty());
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the deletion", |cx| {
        read(&notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(
        head(&store, "o"),
        (true, "Old.md".into(), "x- Original ^stable\n".into())
    );
    assert!(!head(&store, "k").0);
    read(&notebook, cx, |view, cx| {
        let gone = Destination::page("o");
        assert!(!view.nav.workspace.tabs.contains(&gone));
        assert!(!view.nav.workspace.favorites.contains(&gone));
        assert!(!view.nav.workspace.recent.contains(&gone));
        assert_ne!(view.nav.workspace.active.as_ref(), Some(&gone));
        assert!(
            view.session
                .borrow()
                .open_pages()
                .iter()
                .all(|page| page.read(cx).id() != "o")
        );
        assert!(view.nav.title("o").is_none());
    });
    assert_eq!(active(&notebook, cx), "k");
    wait_until(cx, "the catalogue without Old", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav.catalogue.is_none() && view.nav.title("o").is_none()
        })
    });
    // Nothing typed later revives the page.
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    assert!(head(&store, "o").0);
}

#[gpui::test]
fn deleting_a_folder_refuses_a_page_added_meanwhile_then_deletes_every_page(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&PAGES, "k", cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-e"));
    cx.simulate_keystrokes(notebook.window.into(), "right down right home");
    read(&notebook, cx, |view, _| {
        assert_eq!(view.nav.workspace.expanded, ["Projects", "Projects/Deep"])
    });
    cx.simulate_keystrokes(notebook.window.into(), "delete");
    let (heading, detail, listed) = summary(&notebook, cx);
    assert_eq!(heading, "Delete Projects with its 2 pages?");
    assert_eq!(
        detail,
        "No other page links here. Every computer on this notebook removes it too."
    );
    assert_eq!(listed, ["Projects/A.md", "Projects/Deep/B.md"]);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&store, "late", "Projects/Late.md", "- late\n");
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the refusal", |cx| {
        dialog_error(&notebook, cx).is_some()
    });
    assert_eq!(
        dialog_error(&notebook, cx).unwrap(),
        "Projects/Late.md appeared in Projects after you chose to delete it. Nothing was deleted; try again."
    );
    assert!(!head(&store, "pa").0 && !head(&store, "pb").0);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    assert!(read(&notebook, cx, |view, _| view.nav.dialog.is_none()));

    cx.simulate_keystrokes(notebook.window.into(), &key("shift-e"));
    cx.simulate_keystrokes(notebook.window.into(), "home");
    cx.simulate_keystrokes(notebook.window.into(), &key("backspace"));
    let (heading, _, listed) = summary(&notebook, cx);
    assert_eq!(heading, "Delete Projects with its 3 pages?");
    assert_eq!(
        listed,
        ["Projects/A.md", "Projects/Deep/B.md", "Projects/Late.md"]
    );
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the folder deletion", |cx| {
        read(&notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    for page in ["pa", "pb", "late"] {
        assert!(head(&store, page).0, "{page} must be deleted");
    }
    assert!(!head(&store, "o").0);
    read(&notebook, cx, |view, _| {
        assert!(view.nav.workspace.expanded.is_empty());
        assert!(
            view.file_rows()
                .iter()
                .all(|row| !row.path.starts_with("Projects"))
        );
    });
}

#[gpui::test]
fn a_page_saved_elsewhere_after_the_choice_is_not_deleted(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "o", cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-e"));
    let keep = read(&notebook, cx, |view, _| {
        view.file_rows()
            .iter()
            .position(|row| row.path == "Keep.md")
            .unwrap()
    });
    notebook
        .window
        .update(cx, |view, _, _| view.nav.browsers.files_selected = keep)
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "delete");
    let (heading, detail, _) = summary(&notebook, cx);
    assert_eq!(heading, "Delete Keep?");
    assert!(detail.starts_with("No other page links here."));
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&store, "k", "Keep.md", "- keep, edited elsewhere\n");
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the refusal", |cx| {
        dialog_error(&notebook, cx).is_some()
    });
    assert_eq!(dialog_error(&notebook, cx).unwrap(), STALE_DELETION);
    assert!(!head(&store, "k").0);
}
