//! Live task lists in Write, driven by clicks and keys: rendering and grouping, ticking a task
//! on another page and on the same page (one undo step, Vim on and off), cycling its state,
//! query errors, refreshing after another writer saves, Show more, revealing the query with
//! the caret, and the Templates page keeping queries as written. Ported from the rt editor
//! tests "shows a live task list for {{tasks: …}} and ticks tasks through it" and "keeps
//! {{tasks: …}} as text on the Templates page".

use super::super::*;
use super::links_tests::set_vim;
use super::tags_tests::stored;
use super::tests::{Notebook, draw, key, open_notebook, read, save, wait_until};
use gpui::TestAppContext;
use quire_store::notebook::NotebookStore;

/// 2026-10-30T12:00:00Z, a date every inhabited time zone shares at that instant.
const NOW: i64 = 1_793_361_600_000;

fn host_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn lines(editor: &Entity<editor::Editor>, cx: &mut TestAppContext) -> Vec<Vec<String>> {
    cx.read(|cx| crate::task_lists::lines(editor.read(cx), cx))
}

fn headers(editor: &Entity<editor::Editor>, cx: &mut TestAppContext) -> Vec<String> {
    cx.read(|cx| crate::task_lists::header_labels(editor.read(cx)))
}

fn display(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    cx: &mut TestAppContext,
) -> String {
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.snapshot(window, cx).display_snapshot.text()
            })
        })
        .unwrap()
}

/// Opens `page` in Write on the notebook clock, with the caret on its first line.
fn open_in_write(
    pages: &[(&str, &str, &str)],
    page: &str,
    vim: bool,
    cx: &mut TestAppContext,
) -> (Notebook, Entity<editor::Editor>) {
    let notebook = open_notebook(pages, page, cx);
    set_vim(vim, cx);
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(1600.), gpui::px(1200.)),
    );
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.nav.daily.now = Rc::new(|| NOW);
            view.install_task_context(cx);
            view.focus_active(window, cx);
        })
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-m"));
    let editor = host_editor(&notebook, cx);
    (notebook, editor)
}

/// Clicks once the index has analyzed every save and each list shows its latest answer, so no
/// reply redraws the list between the press and the release.
fn click(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    selector: &'static str,
    cx: &mut TestAppContext,
) {
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    wait_until(cx, "a quiet index and settled lists", |cx| {
        futures::executor::block_on(client.unanalyzed().unwrap()).unwrap() == 0
            && cx.read(|cx| crate::task_lists::settled(editor.read(cx), cx))
    });
    draw(notebook, cx);
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element {selector}"));
    visual.simulate_click(bounds.center(), Default::default());
}

fn wait_for_lines(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    expected: &[&[&str]],
    cx: &mut TestAppContext,
) {
    wait_until(cx, "the task lists", |cx| {
        draw(notebook, cx);
        lines(editor, cx) == expected
    });
}

#[gpui::test]
fn a_task_list_groups_its_matches_ticks_another_page_and_shows_query_errors(
    cx: &mut TestAppContext,
) {
    const PROJECTS: &str = "- #project\n- {{tasks: next by:page}}\n- {{tasks: colour:red}}\n";
    let pages = [
        ("p", "Projects.md", PROJECTS),
        (
            "l",
            "Launch.md",
            "- #project\n- [ ] write the plan #next\n- [ ] book the venue #waiting\n",
        ),
        ("g", "Garden.md", "- [ ] dig beds #next\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "p", true, cx);
    wait_for_lines(
        &notebook,
        &editor,
        &[
            &[
                "## Garden",
                "[ ] dig beds · Next · Garden",
                "## Launch",
                "[ ] write the plan · Next · Launch",
            ],
            &[
                "error: Unknown term \"colour:red\". Use a state, #tag, [[Page]], project:Name, due:, done:, by:, sort:, limit: or words.",
            ],
        ],
        cx,
    );
    assert_eq!(
        headers(&editor, cx),
        [
            "☰ Tasks · next by:page · 2",
            "☰ Tasks · colour:red · query error"
        ]
    );
    wait_until(cx, "the folded queries", |cx| {
        draw(&notebook, cx);
        display(&notebook, &editor, cx) == "#project\n☰\n\n\n\n\n☰\n\n"
    });

    click(&notebook, &editor, "quire-task-list-tick-3", cx);
    wait_until(cx, "the ticked task to reach its page", |_| {
        stored(&notebook, "l")
            == "- #project\n- [x] write the plan #next ✅ 2026-10-30\n- [ ] book the venue #waiting\n"
    });
    wait_for_lines(
        &notebook,
        &editor,
        &[
            &["## Garden", "[ ] dig beds · Next · Garden"],
            &[
                "error: Unknown term \"colour:red\". Use a state, #tag, [[Page]], project:Name, due:, done:, by:, sort:, limit: or words.",
            ],
        ],
        cx,
    );
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), PROJECTS, "the query text stays"));
    assert_eq!(stored(&notebook, "p"), PROJECTS);
}

fn tick_on_the_same_page(vim: bool, cx: &mut TestAppContext) {
    const HERE: &str = "- {{tasks}}\n- [ ] first #next\n- [ ] second\n";
    let pages = [
        ("h", "Here.md", HERE),
        ("o", "Other.md", "- [ ] elsewhere #next\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "h", vim, cx);
    wait_for_lines(
        &notebook,
        &editor,
        &[&["[ ] first · Next · Here", "[ ] second · Inbox · Here"]],
        cx,
    );
    assert_eq!(headers(&editor, cx), ["☰ Tasks · here · 2"]);

    click(&notebook, &editor, "quire-task-list-tick-0", cx);
    let ticked = "- {{tasks}}\n- [x] first #next ✅ 2026-10-30\n- [ ] second\n";
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), ticked));
    wait_for_lines(&notebook, &editor, &[&["[ ] second · Inbox · Here"]], cx);
    wait_until(cx, "the tick to be saved", |_| {
        stored(&notebook, "h") == ticked
    });

    click(&notebook, &editor, "quire-task-list-state-0", cx);
    let moved = "- {{tasks}}\n- [x] first #next ✅ 2026-10-30\n- [ ] second #next\n";
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), moved));
    wait_for_lines(&notebook, &editor, &[&["[ ] second · Next · Here"]], cx);

    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    let undo = if vim { "escape u".to_owned() } else { key("z") };
    cx.simulate_keystrokes(notebook.window.into(), &undo);
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), ticked, "one undo per change"));
    cx.simulate_keystrokes(notebook.window.into(), &undo);
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), HERE));
    wait_for_lines(
        &notebook,
        &editor,
        &[&["[ ] first · Next · Here", "[ ] second · Inbox · Here"]],
        cx,
    );
    assert_eq!(stored(&notebook, "o"), "- [ ] elsewhere #next\n");
}

#[gpui::test]
fn ticking_a_listed_task_on_the_same_page_is_one_undo_step_with_vim(cx: &mut TestAppContext) {
    tick_on_the_same_page(true, cx);
}

#[gpui::test]
fn ticking_a_listed_task_on_the_same_page_is_one_undo_step_with_vim_off(cx: &mut TestAppContext) {
    tick_on_the_same_page(false, cx);
}

#[gpui::test]
fn a_task_list_refreshes_after_another_writer_saves_and_shows_more_on_request(
    cx: &mut TestAppContext,
) {
    let pages = [
        ("q", "Query.md", "- {{[[tasks]]: #next limit:1}}\n"),
        ("a", "Alpha.md", "- [ ] one #next\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "q", true, cx);
    wait_for_lines(&notebook, &editor, &[&["[ ] one · Next · Alpha"]], cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&store, "b", "Beta.md", "- [ ] two #next\n");
    wait_for_lines(
        &notebook,
        &editor,
        &[&["[ ] one · Next · Alpha", "Show more tasks"]],
        cx,
    );
    assert_eq!(headers(&editor, cx), ["☰ Tasks · #next limit:1 · 1 of 2"]);
    click(&notebook, &editor, "quire-task-list-more", cx);
    wait_for_lines(
        &notebook,
        &editor,
        &[&["[ ] one · Next · Alpha", "[ ] two · Next · Beta"]],
        cx,
    );
}

fn reveal_with_the_caret(vim: bool, cx: &mut TestAppContext) {
    const PAGE: &str = "- intro\n- {{tasks: next}}\n- after\n";
    let pages = [
        ("q", "Query.md", PAGE),
        ("g", "Garden.md", "- [ ] dig beds #next\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "q", vim, cx);
    wait_for_lines(&notebook, &editor, &[&["[ ] dig beds · Next · Garden"]], cx);
    wait_until(cx, "the folded query", |cx| {
        draw(&notebook, cx);
        display(&notebook, &editor, cx) == "intro\n☰\n\nafter\n"
    });
    cx.simulate_keystrokes(notebook.window.into(), if vim { "j" } else { "down" });
    wait_until(cx, "the revealed query", |cx| {
        draw(&notebook, cx);
        display(&notebook, &editor, cx) == "intro\n{{tasks: next}}\n\nafter\n"
    });
    cx.simulate_keystrokes(notebook.window.into(), if vim { "k" } else { "up" });
    wait_until(cx, "the folded query again", |cx| {
        draw(&notebook, cx);
        display(&notebook, &editor, cx) == "intro\n☰\n\nafter\n"
    });
}

#[gpui::test]
fn the_caret_reveals_a_task_query_with_vim(cx: &mut TestAppContext) {
    reveal_with_the_caret(true, cx);
}

#[gpui::test]
fn the_caret_reveals_a_task_query_with_vim_off(cx: &mut TestAppContext) {
    reveal_with_the_caret(false, cx);
}

#[gpui::test]
fn the_templates_page_keeps_task_queries_as_written(cx: &mut TestAppContext) {
    let pages = [
        ("t", "Templates.md", "- My Day\n  - {{tasks: next}}\n"),
        ("g", "Garden.md", "- [ ] dig beds #next\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "t", true, cx);
    wait_until(cx, "Write to render the page", |cx| {
        draw(&notebook, cx);
        display(&notebook, &editor, cx) == "My Day\n  {{tasks: next}}\n"
    });
    assert!(lines(&editor, cx).is_empty());
}

fn editor_focused(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    cx: &mut TestAppContext,
) -> bool {
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.focus_handle(cx).is_focused(window)
        })
        .unwrap()
}

fn keyboard_reaches_rows(vim: bool, cx: &mut TestAppContext) {
    const HERE: &str = "- {{tasks}}\n- [ ] first #next\n- [ ] second\n";
    let (notebook, editor) = open_in_write(&[("h", "Here.md", HERE)], "h", vim, cx);
    wait_for_lines(
        &notebook,
        &editor,
        &[&["[ ] first · Next · Here", "[ ] second · Inbox · Here"]],
        cx,
    );
    let window = notebook.window.into();
    cx.simulate_keystrokes(window, &key("enter"));
    assert!(
        !editor_focused(&notebook, &editor, cx),
        "Cmd/Ctrl-Enter on the query line enters its list"
    );
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), HERE, "the line stays as written"));

    cx.simulate_keystrokes(window, &format!("down {}", key("enter")));
    let ticked = "- {{tasks}}\n- [ ] first #next\n- [x] second ✅ 2026-10-30\n";
    cx.read(|cx| {
        assert_eq!(
            editor.read(cx).text(cx),
            ticked,
            "the highlighted task ticks"
        )
    });
    wait_for_lines(&notebook, &editor, &[&["[ ] first · Next · Here"]], cx);

    cx.simulate_keystrokes(window, &format!("up {}", key("shift-enter")));
    let moved = "- {{tasks}}\n- [ ] first #waiting\n- [x] second ✅ 2026-10-30\n";
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), moved, "the state moves on"));
    wait_for_lines(&notebook, &editor, &[&["[ ] first · Waiting · Here"]], cx);

    cx.simulate_keystrokes(window, "d");
    draw(&notebook, cx);
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    assert!(visual.debug_bounds("quire-task-list-date").is_some());
    cx.simulate_input(window, "soon");
    cx.simulate_keystrokes(window, "enter");
    let refused = format!(
        "notice: {}",
        quire_core::tasks::date_phrase("soon", "2026-10-30").unwrap_err()
    );
    wait_for_lines(
        &notebook,
        &editor,
        &[&[refused.as_str(), "[ ] first · Waiting · Here"]],
        cx,
    );
    cx.simulate_keystrokes(window, &format!("{} backspace", key("a")));
    cx.simulate_input(window, "tomorrow");
    cx.simulate_keystrokes(window, "enter");
    wait_until(cx, "the due date in the page", |cx| {
        cx.read(|cx| editor.read(cx).text(cx))
            == "- {{tasks}}\n- [ ] first #waiting 📅 [[2026-10-31]]\n- [x] second ✅ 2026-10-30\n"
    });
    wait_until(cx, "the list to settle", |cx| {
        cx.read(|cx| crate::task_lists::settled(editor.read(cx), cx))
    });
    cx.simulate_keystrokes(window, "escape");
    assert!(
        editor_focused(&notebook, &editor, cx),
        "Escape returns to the page"
    );
}

#[gpui::test]
fn the_keyboard_reaches_list_rows_ticks_moves_and_dates_them_with_vim(cx: &mut TestAppContext) {
    keyboard_reaches_rows(true, cx);
}

#[gpui::test]
fn the_keyboard_reaches_list_rows_ticks_moves_and_dates_them_with_vim_off(cx: &mut TestAppContext) {
    keyboard_reaches_rows(false, cx);
}

#[gpui::test]
fn a_list_inside_an_embed_lists_the_embedded_page_as_here(cx: &mut TestAppContext) {
    let pages = [
        ("h", "Host.md", "- ![[Inner]]\n- [ ] host task\n"),
        ("i", "Inner.md", "- {{tasks}}\n- [ ] inner task\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "h", false, cx);
    wait_until(cx, "the list inside the embed", |cx| {
        draw(&notebook, cx);
        let nested = cx.update(|cx| crate::embeds::nested_editors(editor.read(cx)));
        nested
            .iter()
            .any(|nested| lines(nested, cx) == [["[ ] inner task · Inbox · Inner"]])
    });
    assert!(
        lines(&editor, cx).is_empty(),
        "the host page has no query of its own"
    );
}

#[gpui::test]
fn ticking_a_task_whose_page_is_open_in_another_tab_updates_that_tab(cx: &mut TestAppContext) {
    let pages = [
        ("q", "Query.md", "- {{tasks: next}}\n"),
        ("a", "Alpha.md", "- [ ] one #next\n"),
    ];
    let notebook = open_notebook(&pages, "a", cx);
    let alpha = read(&notebook, cx, |view, _| view.page.clone());
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(quire_core::workspace::Destination::page("q"), window, cx)
        })
        .unwrap();
    wait_until(cx, "the query page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page.read(cx).id() == "q" && view.editor.is_some()
        })
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.nav.daily.now = Rc::new(|| NOW);
            view.install_task_context(cx);
            view.focus_active(window, cx);
        })
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-m"));
    let editor = host_editor(&notebook, cx);
    wait_for_lines(&notebook, &editor, &[&["[ ] one · Next · Alpha"]], cx);
    assert!(
        read(&notebook, cx, |view, cx| view
            .nav
            .workspace
            .tabs
            .iter()
            .any(|tab| view.destination_label(tab, cx) == "Alpha")),
        "Alpha stays open in its tab"
    );
    click(&notebook, &editor, "quire-task-list-tick-0", cx);
    let ticked = "- [x] one #next ✅ 2026-10-30\n";
    wait_until(
        cx,
        "the tick in the other tab's buffer and the store",
        |cx| {
            let text = cx.read(|cx| {
                alpha
                    .read(cx)
                    .loaded()
                    .map(|loaded| loaded.analysis.read(cx).source.read(cx).text())
            });
            text.as_deref() == Some(ticked) && stored(&notebook, "a") == ticked
        },
    );
    cx.read(|cx| assert_eq!(editor.read(cx).text(cx), "- {{tasks: next}}\n"));
}

/// Each open row of a live list ends with a calendar menu and buttons that make the task due
/// tomorrow or next Monday; T and W do the same for the highlighted task.
#[gpui::test]
fn quick_date_buttons_and_keys_push_listed_tasks(cx: &mut TestAppContext) {
    const HERE: &str = "- {{tasks: next}}\n- [ ] first #next\n";
    let pages = [
        ("h", "Here.md", HERE),
        ("o", "Other.md", "- [ ] remote #next\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "h", false, cx);
    wait_for_lines(
        &notebook,
        &editor,
        &[&["[ ] first · Next · Here", "[ ] remote · Next · Other"]],
        cx,
    );
    let page = |cx: &mut TestAppContext| cx.read(|cx| editor.read(cx).text(cx));
    click(&notebook, &editor, "quire-task-list-tomorrow-1", cx);
    wait_until(cx, "the other page due tomorrow", |_| {
        stored(&notebook, "o") == "- [ ] remote #next 📅 [[2026-10-31]]\n"
    });
    click(&notebook, &editor, "quire-task-list-next-week-0", cx);
    wait_until(cx, "this page due next Monday", |cx| {
        page(cx) == "- {{tasks: next}}\n- [ ] first #next 📅 [[2026-11-02]]\n"
    });

    click(&notebook, &editor, "quire-task-list-dates-0", cx);
    click(&notebook, &editor, "MENU_ITEM-Clear due date", cx);
    wait_until(cx, "the cleared due date", |cx| page(cx) == HERE);
    click(&notebook, &editor, "quire-task-list-dates-0", cx);
    click(&notebook, &editor, "MENU_ITEM-Next week · Nov 2", cx);
    wait_until(cx, "the due date from the menu", |cx| {
        page(cx) == "- {{tasks: next}}\n- [ ] first #next 📅 [[2026-11-02]]\n"
    });

    let window = notebook.window.into();
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    cx.simulate_keystrokes(window, &key("enter"));
    assert!(
        !editor_focused(&notebook, &editor, cx),
        "Cmd/Ctrl-Enter on the query line enters its list"
    );
    cx.simulate_keystrokes(window, "down w");
    wait_until(cx, "the other page due next Monday", |_| {
        stored(&notebook, "o") == "- [ ] remote #next 📅 [[2026-11-02]]\n"
    });
    wait_until(cx, "the list to settle", |cx| {
        cx.read(|cx| crate::task_lists::settled(editor.read(cx), cx))
    });
    cx.simulate_keystrokes(window, "up t");
    wait_until(cx, "the key's due date", |cx| {
        page(cx) == "- {{tasks: next}}\n- [ ] first #next 📅 [[2026-10-31]]\n"
            && stored(&notebook, "h") == page(cx)
    });
}
