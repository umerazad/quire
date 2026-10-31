use super::super::*;
use super::tests::{Notebook, PAGES, active, draw, key, open_notebook, read, settle, wait_until};
use gpui::TestAppContext;
use quire_core::workspace::{BrowserView, Destination, Workspace};
use quire_store::notebook::{Edit, NotebookStore};

fn visit(notebook: &Notebook, destination: Destination, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |view, window, cx| view.visit(destination, window, cx))
        .unwrap();
    cx.run_until_parked();
}

fn dispatch(notebook: &Notebook, action: impl gpui::Action, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), action);
    cx.run_until_parked();
}

pub(super) fn saved(notebook: &Notebook, cx: &mut TestAppContext) {
    wait_until(cx, "the saved workspace", |cx| {
        read(notebook, cx, |view, _| view.nav.saving_idle())
    });
}

pub(super) fn reopen(notebook: Notebook, first: &str, cx: &mut TestAppContext) -> Notebook {
    cx.update_window(notebook.window.into(), |_, window, _| {
        window.remove_window()
    })
    .unwrap();
    cx.run_until_parked();
    // Work the closed window started on storage threads may hold its session, and with it the
    // notebook lock, a little longer.
    let mut session = None;
    wait_until(cx, "the closed window to release the notebook", |_| {
        session = NotebookSession::open(notebook.path.clone(), "nb".into(), "local".into()).ok();
        session.is_some()
    });
    let mut session = session.unwrap();
    let page = cx.update(|cx| session.open_page(first.into(), cx));
    let path = notebook.path.clone();
    let window = cx.add_window(|window, cx| {
        StoredWindow::new(session, page.clone(), path.clone(), "nb".into(), window, cx)
    });
    let reopened = Notebook {
        _root: notebook._root,
        path: notebook.path,
        window,
    };
    settle(&reopened, cx);
    reopened
}

#[gpui::test]
fn history_moves_back_and_forward_without_reordering_recents(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    visit(&notebook, Destination::page("b"), cx);
    visit(&notebook, Destination::page("n"), cx);
    let (back, forward) = if cfg!(target_os = "macos") {
        ("cmd-[", "cmd-]")
    } else {
        ("alt-left", "alt-right")
    };
    cx.simulate_keystrokes(notebook.window.into(), back);
    assert_eq!(active(&notebook, cx), "b");
    cx.simulate_keystrokes(notebook.window.into(), back);
    assert_eq!(active(&notebook, cx), "a");
    cx.simulate_keystrokes(notebook.window.into(), back);
    assert_eq!(active(&notebook, cx), "a");
    cx.simulate_keystrokes(notebook.window.into(), forward);
    assert_eq!(active(&notebook, cx), "b");
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.recent,
            [
                Destination::page("n"),
                Destination::page("b"),
                Destination::page("a")
            ]
        );
    });
    visit(&notebook, Destination::page("d"), cx);
    cx.simulate_keystrokes(notebook.window.into(), forward);
    assert_eq!(active(&notebook, cx), "d");
    cx.simulate_keystrokes(notebook.window.into(), back);
    assert_eq!(active(&notebook, cx), "b");
}

#[gpui::test]
fn tabs_cycle_close_to_the_last_tab_and_split_beside(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    visit(&notebook, Destination::page("b"), cx);
    visit(&notebook, Destination::page("n"), cx);
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.tabs,
            [
                Destination::page("a"),
                Destination::page("b"),
                Destination::page("n")
            ]
        )
    });
    cx.simulate_keystrokes(notebook.window.into(), "ctrl-tab");
    assert_eq!(active(&notebook, cx), "a");
    cx.simulate_keystrokes(notebook.window.into(), "ctrl-shift-tab");
    assert_eq!(active(&notebook, cx), "n");
    cx.simulate_keystrokes(notebook.window.into(), &key("w"));
    assert_eq!(active(&notebook, cx), "b");
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.tabs,
            [Destination::page("a"), Destination::page("b")]
        )
    });
    wait_until(cx, "Beta's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    cx.simulate_keystrokes(notebook.window.into(), &key("\\"));
    wait_until(cx, "the mirrored side pane", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.editor.is_some())
        })
    });
    read(&notebook, cx, |view, _| {
        assert_eq!(view.nav.side.as_ref().unwrap().page, view.page)
    });
    cx.simulate_keystrokes(notebook.window.into(), &key("\\"));
    assert!(read(&notebook, cx, |view, _| view.nav.side.is_none()));
    dispatch(&notebook, crate::navigation::CloseTab, cx);
    dispatch(&notebook, crate::navigation::CloseTab, cx);
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.tabs,
            [Destination::Browser {
                view: BrowserView::Pages
            }]
        );
        assert_eq!(view.nav.view, Some(BrowserView::Pages));
    });
}

#[gpui::test]
fn favorites_tabs_and_side_pane_persist_per_notebook(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    visit(&notebook, Destination::page("b"), cx);
    dispatch(&notebook, crate::navigation::ToggleFavorite, cx);
    visit(&notebook, Destination::page("n"), cx);
    dispatch(&notebook, crate::navigation::ToggleFavorite, cx);
    visit(&notebook, Destination::page("a"), cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_beside("d".into(), window, cx)
        })
        .unwrap();
    saved(&notebook, cx);
    let other = crate::navigation::WorkspaceFile::beside(&notebook.path, "other");
    assert_eq!(other.load(), Workspace::default());
    let notebook = reopen(notebook, "a", cx);
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.favorites,
            [Destination::page("b"), Destination::page("n")]
        );
        assert_eq!(
            view.nav.workspace.tabs,
            [
                Destination::page("a"),
                Destination::page("b"),
                Destination::page("n")
            ]
        );
        assert_eq!(view.nav.workspace.side.as_deref(), Some("d"));
        assert!(view.nav.side.is_some());
    });
    dispatch(&notebook, crate::navigation::ToggleFavorite, cx);
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.favorites,
            [
                Destination::page("b"),
                Destination::page("n"),
                Destination::page("a")
            ]
        );
    });
    dispatch(&notebook, crate::navigation::ToggleFavorite, cx);
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.workspace.favorites,
            [Destination::page("b"), Destination::page("n")]
        );
    });
}

#[gpui::test]
fn saved_destinations_for_deleted_pages_are_dropped_on_open(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    visit(&notebook, Destination::page("b"), cx);
    dispatch(&notebook, crate::navigation::ToggleFavorite, cx);
    visit(&notebook, Destination::page("a"), cx);
    saved(&notebook, cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    store
        .save(&Edit {
            page: "b".into(),
            base: store.heads("b").unwrap(),
            path: "Beta.md".into(),
            text: String::new(),
            deleted: true,
            created: 2,
            recovery: None,
        })
        .unwrap();
    drop(store);
    let notebook = reopen(notebook, "a", cx);
    read(&notebook, cx, |view, _| {
        assert!(view.nav.workspace.favorites.is_empty());
        assert_eq!(view.nav.workspace.tabs, [Destination::page("a")]);
        assert_eq!(view.nav.workspace.recent, [Destination::page("a")]);
    });
}

#[gpui::test]
fn files_tree_expands_folders_and_opens_pages_from_the_keyboard(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    let names = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, _| {
            view.file_rows()
                .iter()
                .map(|row| row.path.clone())
                .collect::<Vec<_>>()
        })
    };
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-e"));
    assert_eq!(names(cx), ["Daily", "roam", "Alpha.md", "Beta.md"]);
    cx.simulate_keystrokes(notebook.window.into(), "right");
    assert_eq!(
        names(cx),
        [
            "Daily",
            "Daily/2026-10-30.md",
            "roam",
            "Alpha.md",
            "Beta.md"
        ]
    );
    cx.simulate_keystrokes(notebook.window.into(), "down enter");
    wait_until(cx, "the daily page", |cx| active(&notebook, cx) == "d");
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-e"));
    cx.simulate_keystrokes(notebook.window.into(), "left");
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.browsers.files_selected),
        0
    );
    cx.simulate_keystrokes(notebook.window.into(), "left");
    assert_eq!(names(cx), ["Daily", "roam", "Alpha.md", "Beta.md"]);
    cx.simulate_keystrokes(notebook.window.into(), "end shift-enter");
    wait_until(cx, "Beta beside", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.page.read(cx).id() == "b")
        })
    });
}

#[gpui::test]
fn a_page_that_loads_after_focus_moves_leaves_focus_where_it_went(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("d".into(), false, window, cx);
            assert!(view.editor.is_none(), "the daily page is still loading");
        })
        .unwrap();
    let focus_files = if cfg!(target_os = "macos") {
        "cmd-shift-e"
    } else {
        "ctrl-shift-e"
    };
    let focus_files = gpui::Keystroke::parse(focus_files).unwrap();
    cx.dispatch_keystroke(notebook.window.into(), focus_files);
    wait_until(cx, "the daily page's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, _| {
            assert!(
                view.nav
                    .browsers
                    .files_focus
                    .as_ref()
                    .is_some_and(|focus| focus.is_focused(window)),
                "the loaded page took focus from the Files tree"
            )
        })
        .unwrap();
}

#[gpui::test]
fn pages_browser_filters_by_namespace_and_text(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "a", cx);
    dispatch(&notebook, crate::navigation::ShowPages, cx);
    draw(&notebook, cx);
    let titles = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, cx| {
            view.browser_rows(cx)
                .into_iter()
                .map(|row| row.title)
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(titles(cx), ["2026-10-30", "Alpha", "Beta", "roam/js/clock"]);
    let choose = |namespace: Option<&str>, cx: &mut TestAppContext| {
        notebook
            .window
            .update(cx, |view, _, cx| {
                view.nav.workspace.pages.namespace = namespace.map(str::to_owned);
                cx.notify();
            })
            .unwrap();
    };
    choose(Some("roam"), cx);
    assert_eq!(titles(cx), ["roam/js/clock"]);
    choose(Some("roam/js"), cx);
    assert_eq!(titles(cx), ["roam/js/clock"]);
    choose(None, cx);
    cx.simulate_input(notebook.window.into(), "ta");
    assert_eq!(titles(cx), ["Beta"]);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "Beta from the browser", |cx| {
        active(&notebook, cx) == "b"
    });
    assert_eq!(read(&notebook, cx, |view, _| view.nav.view), None);
}

#[gpui::test]
fn reading_positions_filters_and_drafts_survive_reopening(cx: &mut TestAppContext) {
    use quire_core::workspace::position_key;
    let long: String = (0..200)
        .map(|line| format!("- line {line} #work\n"))
        .collect();
    let pages = [
        ("a", "Alpha.md", "- apple pie\n"),
        ("long", "Long.md", long.as_str()),
    ];
    let notebook = open_notebook(&pages, "long", cx);
    wait_until(cx, "the long page", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    let source = |notebook: &Notebook, cx: &mut TestAppContext| {
        read(notebook, cx, |view, cx| {
            view.editor.as_ref().unwrap().read(cx).source.clone()
        })
    };
    let top = |notebook: &Notebook, cx: &mut TestAppContext| {
        let editor = source(notebook, cx);
        editor.update(cx, |editor, cx| editor.scroll_position(cx).y)
    };
    let editor = source(&notebook, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.set_scroll_position(gpui::point(0., 120.), window, cx);
            })
        })
        .unwrap();
    wait_until(cx, "the remembered reading position", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .workspace
                .positions
                .get(&position_key(false, "long"))
                == Some(&120)
        })
    });

    dispatch(&notebook, crate::navigation::ShowPages, cx);
    draw(&notebook, cx);
    cx.simulate_input(notebook.window.into(), "lo");
    visit(
        &notebook,
        Destination::Tag {
            tag: "#work".into(),
        },
        cx,
    );
    let tag_page = read(&notebook, cx, |view, _| view.shown_tag_page().unwrap());
    wait_until(cx, "the tagged blocks", |cx| {
        cx.read(|cx| tag_page.read(cx).blocks.value.len() == 200)
    });
    draw(&notebook, cx);
    cx.simulate_input(notebook.window.into(), "line 1");
    tag_page.update(cx, |page, _| {
        page.scroll
            .scroll_to_item_strict(40, gpui::ScrollStrategy::Top)
    });
    draw(&notebook, cx);
    dispatch(&notebook, crate::navigation::ShowTasks, cx);
    let tasks = read(&notebook, cx, |view, _| {
        view.nav.tasks.list.clone().unwrap()
    });
    notebook
        .window
        .update(cx, |_, window, cx| {
            tasks.update(cx, |tasks, cx| tasks.focus_capture(window, cx))
        })
        .unwrap();
    cx.simulate_input(notebook.window.into(), "Email Ana");
    visit(&notebook, Destination::page("a"), cx);
    saved(&notebook, cx);
    wait_until(cx, "the remembered views", |cx| {
        read(&notebook, cx, |view, _| {
            let workspace = &view.nav.workspace;
            workspace.pages.query == "lo"
                && workspace.tasks.draft == "Email Ana"
                && workspace
                    .tag_pages
                    .get("work")
                    .map(|page| page.query.as_str())
                    == Some("line 1")
        })
    });
    drop((editor, tag_page, tasks));
    saved(&notebook, cx);

    let notebook = reopen(notebook, "long", cx);
    wait_until(cx, "the long page again", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    assert_eq!(top(&notebook, cx), 120., "the page opens where it was read");
    dispatch(&notebook, crate::navigation::ShowPages, cx);
    draw(&notebook, cx);
    let rows = read(&notebook, cx, |view, cx| {
        view.browser_rows(cx)
            .into_iter()
            .map(|row| row.title)
            .collect::<Vec<_>>()
    });
    assert_eq!(rows, ["Long"], "the Pages filter is kept");
    visit(
        &notebook,
        Destination::Tag {
            tag: "#work".into(),
        },
        cx,
    );
    let tag_page = read(&notebook, cx, |view, _| view.shown_tag_page().unwrap());
    assert_eq!(cx.read(|cx| tag_page.read(cx).choices(cx).query), "line 1");
    dispatch(&notebook, crate::navigation::ShowTasks, cx);
    let tasks = read(&notebook, cx, |view, _| {
        view.nav.tasks.list.clone().unwrap()
    });
    assert_eq!(
        cx.read(|cx| tasks.read(cx).capture.read(cx).text(cx)),
        "Email Ana"
    );
}
