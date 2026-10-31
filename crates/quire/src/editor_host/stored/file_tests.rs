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

pub(super) fn visit(notebook: &Notebook, destination: Destination, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |view, window, cx| view.visit(destination, window, cx))
        .unwrap();
    cx.run_until_parked();
}

/// The dialog's heading and detail once planning finished.
pub(super) fn summary(
    notebook: &Notebook,
    cx: &mut TestAppContext,
) -> (String, String, Vec<String>) {
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

pub(super) fn dialog_error(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
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
const RENAMED: [(&str, &str, &str); 4] = [
    (
        "o",
        "Old name.md",
        "---\ntags: [x]\ntitle: \"Old name\"\n---\n- Body ^stable [[Old name]]\n",
    ),
    (
        "l",
        "Links.md",
        "- [[Old name]] and [[Old name#Part|alias]] and ![[Old name#^stable]]\r\n- `[[Old name]]` [x](Old%20name.md) [[Keep]]\r\n",
    ),
    ("p", "Notes/Plan.md", "- see [[Old name]]\n"),
    ("k", "Keep.md", "- keep\n"),
];

fn stored_text(store: &NotebookStore, page: &str) -> (String, String) {
    let (deleted, path, text) = head(store, page);
    assert!(!deleted);
    (path, text)
}

pub(super) fn buffer_text(notebook: &Notebook, page: &str, cx: &mut TestAppContext) -> String {
    read(notebook, cx, |view, cx| {
        let open = view.session.borrow().open_pages();
        let open = open.iter().find(|open| open.read(cx).id() == page).unwrap();
        let analysis = open.read(cx).loaded().unwrap().analysis.clone();
        analysis.read(cx).source.read(cx).text()
    })
}

pub(super) fn rename_to(notebook: &Notebook, name: &str, cx: &mut TestAppContext) {
    wait_until(cx, "the rename dialog", |cx| {
        read(notebook, cx, |view, _| view.nav.dialog.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            let dialog = view.nav.dialog.clone().unwrap();
            dialog.update(cx, |dialog, cx| dialog.select_name(window, cx));
        })
        .unwrap();
    draw(notebook, cx);
    cx.simulate_input(notebook.window.into(), name);
    assert_eq!(
        read(notebook, cx, |view, cx| view
            .nav
            .dialog
            .as_ref()
            .unwrap()
            .read(cx)
            .name(cx)),
        Some(name.to_owned())
    );
    cx.simulate_keystrokes(notebook.window.into(), "enter");
}

#[gpui::test]
fn renaming_rewrites_links_in_open_and_closed_pages_and_keeps_unsaved_typing(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&RENAMED, "p", cx);
    wait_until(cx, "Plan's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i y escape");
    visit(&notebook, Destination::page("o"), cx);
    wait_until(cx, "Old name's editor", |cx| {
        read(&notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == "o"
        })
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::RenamePage);
    rename_to(&notebook, "New name", cx);
    wait_until(cx, "the rename", |cx| {
        read(&notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(
        stored_text(&store, "o"),
        (
            "New name.md".into(),
            "---\ntags: [x]\ntitle: \"New name\"\n---\n- Body ^stable [[New name]]\n".into()
        )
    );
    assert_eq!(
        stored_text(&store, "l"),
        (
            "Links.md".into(),
            "- [[New name]] and [[New name#Part|alias]] and ![[New name#^stable]]\r\n- `[[Old name]]` [x](New%20name.md) [[Keep]]\r\n".into()
        )
    );
    assert_eq!(
        stored_text(&store, "p"),
        ("Notes/Plan.md".into(), "y- see [[New name]]\n".into())
    );
    wait_until(cx, "the open pages to take in the rename", |cx| {
        buffer_text(&notebook, "p", cx) == "y- see [[New name]]\n"
            && buffer_text(&notebook, "o", cx).contains("[[New name]]")
    });
    // Later saves of the open page keep its new name instead of moving it back.
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i z escape");
    cx.simulate_keystrokes(notebook.window.into(), &key("s"));
    wait_until(cx, "the save after renaming", |cx| {
        let (_, text) = stored_text(&store, "o");
        let _ = cx;
        text.starts_with('z')
    });
    assert_eq!(stored_text(&store, "o").0, "New name.md");
    wait_until(cx, "the catalogue with the new name", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav.title("o") == Some("New name")
        })
    });
    read(&notebook, cx, |view, cx| {
        assert_eq!(view.page.read(cx).path(cx).as_deref(), Some("New name.md"));
        assert!(view.file_rows().iter().any(|row| row.path == "New name.md"));
    });
}

#[gpui::test]
fn renaming_refuses_taken_invalid_and_case_only_names_and_stale_pages(cx: &mut TestAppContext) {
    let notebook = open_notebook(&RENAMED, "k", cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-e"));
    let old = read(&notebook, cx, |view, _| {
        view.file_rows()
            .iter()
            .position(|row| row.path == "Old name.md")
            .unwrap()
    });
    notebook
        .window
        .update(cx, |view, _, _| view.nav.browsers.files_selected = old)
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "f2");
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let before = store.heads("o").unwrap();
    for (name, error) in [
        ("keep", quire_core::relocate::TAKEN),
        ("a/b", quire_core::relocate::NAME_HAS_SLASH),
        ("con", quire_core::relocate::INVALID_NAME),
        ("old NAME", quire_core::relocate::CASE_ONLY),
    ] {
        rename_to(&notebook, name, cx);
        wait_until(cx, "the refusal", |cx| {
            dialog_error(&notebook, cx).is_some()
        });
        assert_eq!(dialog_error(&notebook, cx).unwrap(), error, "{name}");
    }
    // Typing clears the error; the dialog stays open to correct the name.
    cx.simulate_input(notebook.window.into(), "s");
    assert!(dialog_error(&notebook, cx).is_none());
    assert_eq!(store.heads("o").unwrap(), before);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    assert!(read(&notebook, cx, |view, _| view.nav.dialog.is_none()));

    // A page saved by another writer between planning and writing refuses the whole rename.
    let (open, storage, index) = read(&notebook, cx, |view, cx| {
        let session = view.session.borrow();
        (
            session.open_pages(),
            session.storage(),
            view.index_client(cx).unwrap(),
        )
    });
    let slot = std::rc::Rc::new(std::cell::RefCell::new(None));
    let filled = slot.clone();
    let planning = storage.clone();
    cx.update(|cx| {
        let plan = crate::file_actions::plan_rename(
            "o".into(),
            "Renamed".into(),
            open,
            index,
            planning,
            cx,
        );
        cx.spawn(async move |_| *filled.borrow_mut() = Some(plan.await))
            .detach();
    });
    wait_until(cx, "the rename plan", |_| slot.borrow().is_some());
    let plan = slot.borrow_mut().take().unwrap().unwrap().unwrap();
    save(&store, "l", "Links.md", "- [[Old name]] edited elsewhere\n");
    let written = std::rc::Rc::new(std::cell::RefCell::new(None));
    let result = written.clone();
    cx.update(|cx| {
        let write = crate::file_actions::rename(plan, storage, cx);
        cx.spawn(async move |_| *result.borrow_mut() = Some(write.await))
            .detach();
    });
    wait_until(cx, "the refused rename", |_| written.borrow().is_some());
    assert_eq!(
        written.borrow_mut().take().unwrap(),
        Err(crate::file_actions::STALE_RENAME.to_owned())
    );
    assert_eq!(stored_text(&store, "o").0, "Old name.md");
    assert_eq!(
        stored_text(&store, "l").1,
        "- [[Old name]] edited elsewhere\n"
    );
}

#[gpui::test]
fn a_rename_is_followed_until_the_page_list_shows_its_new_title(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "o", cx);
    let listed = |title: &str| quire_store::index::ListedPage {
        page: quire_store::index::PageRef {
            page: "o".into(),
            revision: String::new(),
            path: "New.md".into(),
            title: title.into(),
        },
        date: None,
    };
    let renamed = crate::file_actions::Renamed {
        previous: "o".into(),
        page: "o".into(),
        path: "New.md".into(),
        title: "New".into(),
    };
    notebook
        .window
        .update(cx, |view, _, cx| {
            // An overlay with the new path but the text from before the merge.
            view.nav.pages = vec![listed("Old")];
            view.nav.renamed = vec![renamed.clone()];
            view.nav.catalogue = None;
            view.follow_rename(cx);
            assert_eq!(view.nav.renamed, std::slice::from_ref(&renamed));
            assert!(view.nav.catalogue.is_some(), "the page list is asked again");

            view.nav.pages = vec![listed("New")];
            view.follow_rename(cx);
            assert!(view.nav.renamed.is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn the_files_tree_is_rebuilt_only_when_its_pages_or_folders_change(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "o", cx);
    let listed = |path: &str| quire_store::index::ListedPage {
        page: quire_store::index::PageRef {
            page: path.into(),
            revision: String::new(),
            path: path.into(),
            title: path.into(),
        },
        date: None,
    };
    let paths = |rows: &[quire_core::workspace::TreeRow]| -> Vec<String> {
        rows.iter().map(|row| row.path.clone()).collect()
    };
    notebook
        .window
        .update(cx, |view, _, _| {
            view.nav.pages = vec![listed("Work/A.md"), listed("B.md")];
            let first = view.file_rows();
            assert!(
                std::rc::Rc::ptr_eq(&first, &view.file_rows()),
                "a second look in the same frame reuses the rows"
            );
            assert_eq!(paths(&first), ["Work", "B.md"]);

            view.nav.workspace.toggle_expanded("Work");
            assert_eq!(paths(&view.file_rows()), ["Work", "Work/A.md", "B.md"]);

            view.nav.pages = vec![listed("Work/A.md"), listed("C.md")];
            assert_eq!(paths(&view.file_rows()), ["Work", "Work/A.md", "C.md"]);
        })
        .unwrap();
}
