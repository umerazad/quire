use super::super::*;
use super::file_tests::buffer_text;
use super::move_tests::stored;
use super::tests::{Notebook, draw, read, settle, wait_until};
use crate::{
    file_actions::STALE_RESTORE,
    navigation::page_history::{Load, NO_VERSIONS, PageHistory},
};
use gpui::TestAppContext;
use quire_core::daily::DailyCalendar;
use quire_store::notebook::{Edit, NotebookStore};

/// 2023-11-14 22:13:20 UTC.
const SAVED: u64 = 1_700_000_000_000;

/// Opens a notebook whose pages were saved in order at the given times, showing `first`.
fn open_dated(saves: &[(&str, &str, &str, u64)], first: &str, cx: &mut TestAppContext) -> Notebook {
    cx.update(|cx| init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    for (page, file, text, created) in saves {
        store
            .save(&Edit {
                page: (*page).into(),
                base: store.heads(page).unwrap(),
                path: (*file).into(),
                text: (*text).into(),
                deleted: false,
                created: *created,
                recovery: None,
            })
            .unwrap();
    }
    drop(store);
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| session.open_page(first.into(), cx));
    let window = cx.add_window(|window, cx| {
        StoredWindow::new(session, page.clone(), path.clone(), "nb".into(), window, cx)
    });
    let notebook = Notebook {
        _root: root,
        path,
        window,
    };
    settle(&notebook, cx);
    notebook
}

fn history(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<PageHistory> {
    read(notebook, cx, |view, _| view.nav.page_history.clone()).expect("page history is open")
}

/// Opens page history in UTC and waits for the list and the newest version's preview.
fn open_history(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<PageHistory> {
    notebook
        .window
        .update(cx, |view, _, _| {
            view.nav.daily.calendar = Some(DailyCalendar::utc())
        })
        .unwrap();
    cx.dispatch_action(notebook.window.into(), crate::navigation::ShowPageHistory);
    wait_until(cx, "the saved versions", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav
                .page_history
                .as_ref()
                .is_some_and(|history| history.read(cx).can_restore())
        })
    });
    history(notebook, cx)
}

fn labels(history: &Entity<PageHistory>, cx: &mut TestAppContext) -> Vec<String> {
    history.read_with(cx, |history, _| history.labels())
}

/// The previewed version's text, the diff editor's display text, and its hunk count.
fn preview(history: &Entity<PageHistory>, cx: &mut TestAppContext) -> (String, String, usize) {
    history.update(cx, |history, cx| {
        let editor = history.preview_editor().unwrap();
        let (display, hunks) = editor.update(cx, |editor, cx| {
            let hunks = editor.buffer().read(cx).snapshot(cx).diff_hunks().count();
            (editor.display_text(cx), hunks)
        });
        (history.preview_text().unwrap().to_owned(), display, hunks)
    })
}

fn history_error(history: &Entity<PageHistory>, cx: &mut TestAppContext) -> Option<String> {
    history.read_with(cx, |history, _| {
        history.error.as_ref().map(|error| error.to_string())
    })
}

fn wait_for_editor(notebook: &Notebook, page: &str, cx: &mut TestAppContext) {
    wait_until(cx, "the page's editor", |cx| {
        read(notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == page
        })
    });
}

#[gpui::test]
fn page_history_lists_versions_diffs_them_and_restores_over_unsaved_typing(
    cx: &mut TestAppContext,
) {
    let notebook = open_dated(
        &[
            ("p", "Plan.md", "- first\n", 1),
            ("p", "Plan.md", "- second\n- kept\n", SAVED),
            ("p", "Plan.md", "- third\n- kept\n", SAVED + 60_000),
        ],
        "p",
        cx,
    );
    wait_for_editor(&notebook, "p", cx);
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i y escape");
    assert_eq!(buffer_text(&notebook, "p", cx), "y- third\n- kept\n");

    let history = open_history(&notebook, cx);
    assert_eq!(
        labels(&history, cx),
        ["2023-11-14 22:13", "1970-01-01 00:00"],
        "earlier versions, newest first, without the current one"
    );
    let (text, display, hunks) = preview(&history, cx);
    assert_eq!(text, "- second\n- kept\n");
    // The read-only editor holds the version, with the current page's changed line shown as
    // deleted, unsaved typing included.
    assert!(display.contains("- second"), "{display}");
    assert!(display.contains("y- third"), "{display}");
    assert_eq!(hunks, 1);
    history.read_with(cx, |history, cx| {
        assert!(history.preview_editor().unwrap().read(cx).read_only(cx));
    });
    // Down chooses the older version; Up goes back.
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "down");
    wait_until(cx, "the older version", |cx| {
        history.read_with(cx, |history, _| history.preview_text() == Some("- first\n"))
    });
    cx.simulate_keystrokes(notebook.window.into(), "up");
    wait_until(cx, "the newer version again", |cx| {
        history.read_with(cx, |history, _| {
            history.preview_text() == Some("- second\n- kept\n")
        })
    });

    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the restore", |cx| {
        read(&notebook, cx, |view, _| view.nav.page_history.is_none())
    });
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(stored(&store, "p"), ("Plan.md".into(), text.clone()));
    wait_until(cx, "the open page to show the restored version", |cx| {
        buffer_text(&notebook, "p", cx) == "- second\n- kept\n"
    });
    // The typing made before restoring was saved first and stays in history.
    let kept: Vec<String> = store
        .history("p")
        .unwrap()
        .iter()
        .map(|saved| store.version_text("p", &saved.id).unwrap())
        .collect();
    assert!(kept.contains(&"y- third\n- kept\n".to_owned()), "{kept:?}");
    assert_eq!(store.heads("p").unwrap().len(), 1);
    // Later typing saves on top of the restored version.
    wait_for_editor(&notebook, "p", cx);
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i z escape");
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("s"));
    wait_until(cx, "the save after restoring", |_| {
        stored(&store, "p").1 == "z- second\n- kept\n"
    });
}

#[gpui::test]
fn a_restore_refuses_a_page_changed_after_the_preview_and_can_save_a_copy_instead(
    cx: &mut TestAppContext,
) {
    let notebook = open_dated(
        &[
            ("p", "Plan.md", "- first ^b1\n", 1),
            ("p", "Plan.md", "- second\n", SAVED),
        ],
        "p",
        cx,
    );
    wait_for_editor(&notebook, "p", cx);
    let history = open_history(&notebook, cx);
    assert_eq!(labels(&history, cx), ["1970-01-01 00:00"]);
    assert_eq!(preview(&history, cx).0, "- first ^b1\n");

    // Typing reaches the page after the preview compared the version with it.
    let buffer = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    });
    buffer.update(cx, |buffer, cx| buffer.edit([(0..0, "late ")], None, cx));
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the refusal", |cx| {
        history_error(&history, cx).is_some()
    });
    assert_eq!(
        history_error(&history, cx).unwrap(),
        format!("The version could not be restored: {STALE_RESTORE}")
    );
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_ne!(stored(&store, "p").1, "- first ^b1\n");
    assert_eq!(buffer_text(&notebook, "p", cx), "late - second\n");
    wait_until(cx, "the preview against the page as it is now", |cx| {
        history.read_with(cx, |history, _| history.can_restore())
    });
    assert!(preview(&history, cx).1.contains("late - second"));

    // Restore as a new page keeps the page and opens a copy with fresh block IDs.
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("enter"));
    wait_until(cx, "the copy to open", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav.page_history.is_none() && view.page.read(cx).id() != "p"
        })
    });
    assert_eq!(buffer_text(&notebook, "p", cx), "late - second\n");
    let copy = read(&notebook, cx, |view, cx| view.page.read(cx).id().to_owned());
    let (path, text) = stored(&store, &copy);
    assert!(path.starts_with("Plan (restored "), "{path}");
    assert!(text.starts_with("- first ^b-"), "{text}");
    assert!(!text.contains("^b1\n"), "{text}");
}

#[gpui::test]
fn a_page_without_earlier_versions_says_so(cx: &mut TestAppContext) {
    let notebook = open_dated(&[("p", "Plan.md", "- only\n", 1)], "p", cx);
    wait_for_editor(&notebook, "p", cx);
    cx.dispatch_action(notebook.window.into(), crate::navigation::ShowPageHistory);
    wait_until(cx, "the empty list", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav.page_history.as_ref().is_some_and(|history| {
                matches!(&history.read(cx).versions, Load::Ready(versions) if versions.is_empty())
            })
        })
    });
    let history = history(&notebook, cx);
    assert!(!history.read_with(cx, |history, _| history.can_restore()));
    assert_eq!(NO_VERSIONS, "No earlier saved versions for this page.");
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    assert!(read(&notebook, cx, |view, _| view
        .nav
        .page_history
        .is_none()));
}
