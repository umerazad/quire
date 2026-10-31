use super::file_tests::{buffer_text, dialog_error, rename_to, visit};
use super::tests::{Notebook, draw, open_notebook, read, save, wait_until};
use crate::file_actions::{LinkUpdate, STALE_MOVE};
use gpui::TestAppContext;
use quire_core::workspace::Destination;
use quire_store::notebook::NotebookStore;

const MOVED: [(&str, &str, &str); 4] = [
    ("o", "Old.md", "- Original ^stable ![i](assets/p.png)\n"),
    ("l", "Links.md", "- [[Old]] [o](Old.md) [[Old#^stable]]\r\n"),
    ("p", "Notes/Plan.md", "- see [x](../Old.md)\n"),
    ("k", "Keep.md", "- keep\n"),
];

pub(super) fn stored(store: &NotebookStore, page: &str) -> (String, String) {
    let heads = store.heads(page).unwrap();
    assert_eq!(heads.len(), 1, "{page} must have one head");
    let record = store.get(&heads[0]).unwrap().unwrap();
    let root = record.root();
    assert_eq!(root.field("deleted").as_bool(), Some(false));
    (
        root.field("path").as_str().unwrap().to_owned(),
        root.field("text").as_str().unwrap().to_owned(),
    )
}

/// The open dialog's preview: heading, detail, listed pages, and the link changes it shows.
pub(super) fn preview(
    notebook: &Notebook,
    cx: &mut TestAppContext,
) -> (String, String, Vec<String>, Vec<LinkUpdate>) {
    wait_until(cx, "the move preview", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav
                .dialog
                .as_ref()
                .is_some_and(|dialog| dialog.read(cx).previewed())
        })
    });
    read(notebook, cx, |view, cx| {
        let dialog = view.nav.dialog.as_ref().unwrap().read(cx);
        let summary = dialog.summary.clone().unwrap();
        (
            summary.heading,
            summary.detail,
            summary.listed,
            dialog.shown_updates(),
        )
    })
}

pub(super) fn update(path: &str, before: &str, after: &str) -> LinkUpdate {
    LinkUpdate {
        path: path.into(),
        before: before.into(),
        after: after.into(),
    }
}

pub(super) fn previewed(notebook: &Notebook, cx: &mut TestAppContext) -> bool {
    read(notebook, cx, |view, cx| {
        view.nav
            .dialog
            .as_ref()
            .is_some_and(|dialog| dialog.read(cx).previewed())
    })
}

#[gpui::test]
fn moving_a_page_previews_every_link_change_then_writes_them_all(cx: &mut TestAppContext) {
    let notebook = open_notebook(&MOVED, "p", cx);
    wait_until(cx, "Plan's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i y escape");
    visit(&notebook, Destination::page("o"), cx);
    wait_until(cx, "Old's editor", |cx| {
        read(&notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == "o"
        })
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::MovePage);
    rename_to(&notebook, "Archive/New", cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let before: Vec<_> = ["o", "l"]
        .iter()
        .map(|page| store.heads(page).unwrap())
        .collect();
    assert_eq!(
        preview(&notebook, cx),
        (
            "Old.md → Archive/New.md".into(),
            "2 other pages need link updates. Pages keep their block IDs and saved history.".into(),
            vec!["Links.md".into(), "Notes/Plan.md".into()],
            vec![
                update("Links.md", "Old.md", "Archive/New.md"),
                update("Notes/Plan.md", "../Old.md", "../Archive/New.md"),
                update("Old.md", "assets/p.png", "../assets/p.png"),
            ]
        )
    );
    let after: Vec<_> = ["o", "l"]
        .iter()
        .map(|page| store.heads(page).unwrap())
        .collect();
    assert_eq!(before, after, "a preview writes nothing");

    // Editing the path drops the preview; Enter previews again before anything moves.
    cx.simulate_input(notebook.window.into(), "x");
    assert!(!previewed(&notebook, cx));
    rename_to(&notebook, "Archive/New", cx);
    preview(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the move", |cx| {
        read(&notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    assert_eq!(
        stored(&store, "o"),
        (
            "Archive/New.md".into(),
            "---\ntitle: \"Old\"\n---\n\n- Original ^stable ![i](../assets/p.png)\n".into()
        )
    );
    assert_eq!(
        stored(&store, "l"),
        (
            "Links.md".into(),
            "- [[Old]] [o](Archive/New.md) [[Old#^stable]]\r\n".into()
        )
    );
    assert_eq!(
        stored(&store, "p"),
        (
            "Notes/Plan.md".into(),
            "y- see [x](../Archive/New.md)\n".into()
        )
    );
    wait_until(cx, "the open pages to take in the move", |cx| {
        buffer_text(&notebook, "p", cx) == "y- see [x](../Archive/New.md)\n"
            && buffer_text(&notebook, "o", cx).contains("../assets/p.png")
    });
    wait_until(cx, "the page list at the new path", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav.renamed.is_empty()
                && view
                    .nav
                    .pages
                    .iter()
                    .any(|listed| listed.page.path == "Archive/New.md")
        })
    });
    read(&notebook, cx, |view, cx| {
        assert_eq!(
            view.page.read(cx).path(cx).as_deref(),
            Some("Archive/New.md")
        );
        assert_eq!(view.nav.title("o"), Some("Old"), "a move keeps the title");
    });
    // Later typing in the moved page saves at its new path.
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i z escape");
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("s"));
    wait_until(cx, "the save after moving", |_| {
        stored(&store, "o").1.contains('z')
    });
    assert_eq!(stored(&store, "o").0, "Archive/New.md");
}

#[gpui::test]
fn a_move_refuses_a_page_changed_after_the_preview_and_previews_again(cx: &mut TestAppContext) {
    let notebook = open_notebook(&MOVED, "k", cx);
    visit(&notebook, Destination::page("o"), cx);
    wait_until(cx, "Old's editor", |cx| {
        read(&notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == "o"
        })
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::MovePage);
    for (path, error) in [
        ("Keep", quire_core::relocate::TAKEN),
        ("../Escape", quire_core::relocate::INVALID_PATH),
        ("old", quire_core::relocate::CASE_ONLY),
    ] {
        rename_to(&notebook, path, cx);
        wait_until(cx, "the refusal", |cx| {
            dialog_error(&notebook, cx).is_some()
        });
        assert_eq!(dialog_error(&notebook, cx).unwrap(), error, "{path}");
    }
    rename_to(&notebook, "Archive/New", cx);
    preview(&notebook, cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&store, "l", "Links.md", "- [o](Old.md) edited elsewhere\n");
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the refusal", |cx| {
        dialog_error(&notebook, cx).is_some()
    });
    assert_eq!(dialog_error(&notebook, cx).unwrap(), STALE_MOVE);
    assert!(!previewed(&notebook, cx));
    assert_eq!(stored(&store, "o").0, "Old.md");
    assert_eq!(stored(&store, "l").1, "- [o](Old.md) edited elsewhere\n");

    // The next Enter previews from the page as it is now, and the one after applies it.
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let (_, _, _, updates) = preview(&notebook, cx);
    assert!(
        updates.contains(&update("Links.md", "Old.md", "Archive/New.md")),
        "{updates:?}"
    );
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the move", |cx| {
        read(&notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    assert_eq!(
        stored(&store, "l").1,
        "- [o](Archive/New.md) edited elsewhere\n"
    );
    assert_eq!(stored(&store, "o").0, "Archive/New.md");
}

const FOLDER: [(&str, &str, &str); 4] = [
    (
        "a",
        "Projects/A.md",
        "- a [[B]] [b](B.md) [t](../Top.md) ![i](attachments/x.png)\n",
    ),
    ("b", "Projects/B.md", "- b [[Projects/A]]\n"),
    ("t", "Top.md", "- [[Projects/A]] [p](Projects/B.md) [[A]]\n"),
    ("z", "Projects2/Z.md", "- z [[Projects/B]]\n"),
];

pub(super) fn select_file(notebook: &Notebook, path: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("shift-e"));
    let row = read(notebook, cx, |view, _| {
        view.file_rows()
            .iter()
            .position(|row| row.path == path)
            .unwrap_or_else(|| panic!("{path} is not in Files"))
    });
    notebook
        .window
        .update(cx, |view, _, _| view.nav.browsers.files_selected = row)
        .unwrap();
}

#[gpui::test]
fn renaming_a_folder_moves_every_page_inside_and_rewrites_links_to_them(cx: &mut TestAppContext) {
    let notebook = open_notebook(&FOLDER, "a", cx);
    select_file(&notebook, "Projects", cx);
    cx.simulate_keystrokes(notebook.window.into(), "f2");
    wait_until(cx, "the folder dialog", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .dialog
                .as_ref()
                .and_then(|dialog| dialog.read(cx).name(cx))
                == Some("Projects".into())
        })
    });
    rename_to(&notebook, "Work", cx);
    assert_eq!(
        preview(&notebook, cx),
        (
            "Projects → Work".into(),
            "2 pages move. 2 other pages need link updates. Pages keep their block IDs and saved history.".into(),
            vec!["Projects2/Z.md".into(), "Top.md".into()],
            vec![
                update("Projects/B.md", "[[Projects/A]]", "[[Work/A]]"),
                update("Projects2/Z.md", "[[Projects/B]]", "[[Work/B]]"),
                update("Top.md", "[[Projects/A]]", "[[Work/A]]"),
                update("Top.md", "Projects/B.md", "Work/B.md"),
            ]
        )
    );
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the folder move", |cx| {
        read(&notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(
        stored(&store, "a"),
        (
            "Work/A.md".into(),
            "- a [[B]] [b](B.md) [t](../Top.md) ![i](attachments/x.png)\n".into()
        )
    );
    assert_eq!(
        stored(&store, "b"),
        ("Work/B.md".into(), "- b [[Work/A]]\n".into())
    );
    assert_eq!(stored(&store, "t").1, "- [[Work/A]] [p](Work/B.md) [[A]]\n");
    assert_eq!(stored(&store, "z").1, "- z [[Work/B]]\n");
    wait_until(cx, "Files to show the new folder", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav.renamed.is_empty()
                && view.file_rows().iter().any(|row| row.path == "Work")
                && !view.file_rows().iter().any(|row| row.path == "Projects")
        })
    });
    read(&notebook, cx, |view, cx| {
        assert_eq!(view.page.read(cx).path(cx).as_deref(), Some("Work/A.md"));
        assert_eq!(
            view.nav.title("a"),
            Some("A"),
            "titles stay without front matter"
        );
    });

    // A folder cannot move inside itself, and Escape leaves everything as it is.
    select_file(&notebook, "Work", cx);
    cx.simulate_keystrokes(notebook.window.into(), "shift-f2");
    rename_to(&notebook, "Work/Inner", cx);
    wait_until(cx, "the refusal", |cx| {
        dialog_error(&notebook, cx).is_some()
    });
    assert_eq!(
        dialog_error(&notebook, cx).unwrap(),
        quire_core::relocate::FOLDER_INSIDE_ITSELF
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    assert!(read(&notebook, cx, |view, _| view.nav.dialog.is_none()));
    assert_eq!(stored(&store, "a").0, "Work/A.md");
}

#[gpui::test]
fn a_page_list_that_shows_the_move_stops_following_it_without_another_index_change(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&MOVED, "o", cx);
    notebook
        .window
        .update(cx, |view, _, cx| {
            let listed = view
                .nav
                .pages
                .iter()
                .find(|listed| listed.page.path == "Keep.md")
                .unwrap();
            view.nav.renamed = vec![crate::file_actions::Renamed {
                previous: "k".into(),
                page: "k".into(),
                path: listed.page.path.clone(),
                title: listed.page.title.clone(),
            }];
            view.refresh_catalogue(cx);
        })
        .unwrap();
    wait_until(cx, "the page list", |cx| {
        read(&notebook, cx, |view, _| view.nav.catalogue.is_none())
    });
    read(&notebook, cx, |view, _| {
        assert!(
            view.nav.renamed.is_empty(),
            "a page list that already shows the move must settle it"
        )
    });
}
