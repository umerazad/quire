//! The Tasks browser in the stored window, driven by keys: lists and counts, ticking and state
//! changes in place, bulk moves with stale refusal, date controls, capture, Projects, and the
//! notebook clock and layout reaching task commands. Ported from rt `tests/tasks-page.test.tsx`.

use super::super::*;
use super::tags_tests::stored;
use super::tests::{Notebook, caret_text, key, open_notebook, read, wait_until};
use super::workspace_tests::{reopen, saved};
use crate::navigation::tasks::{TasksView, next_action};
use gpui::TestAppContext;
use quire_core::{
    daily::{DailyLayout, write_settings},
    workspace::{BrowserView, ProjectStatus, TaskView},
};
use quire_store::{index::SETTINGS_PAGE, notebook::NotebookStore};

/// 2026-10-30T12:00:00Z, a date every inhabited time zone shares at that instant.
const NOW: i64 = 1_793_361_600_000;
const STAMP: &str = " ➕ [[2026-10-30]]";

fn set_clock(notebook: &Notebook, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |view, _, cx| {
            view.nav.daily.now = Rc::new(|| NOW);
            view.install_task_context(cx);
            view.refresh_tasks(cx);
        })
        .unwrap();
}

fn list(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<TasksView> {
    read(notebook, cx, |view, _| view.nav.tasks.list.clone().unwrap())
}

fn settle_tasks(notebook: &Notebook, cx: &mut TestAppContext) {
    wait_until(cx, "the task list", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav
                .tasks
                .list
                .as_ref()
                .is_some_and(|list| list.read(cx).idle())
        })
    });
}

fn show_tasks(notebook: &Notebook, cx: &mut TestAppContext) {
    set_clock(notebook, cx);
    cx.dispatch_action(notebook.window.into(), crate::navigation::ShowTasks);
    set_clock(notebook, cx);
    settle_tasks(notebook, cx);
}

fn texts(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    let list = list(notebook, cx);
    cx.read(|cx| {
        list.read(cx)
            .rows()
            .iter()
            .map(|task| task.text.clone())
            .collect()
    })
}

fn selected(notebook: &Notebook, cx: &mut TestAppContext) -> usize {
    let list = list(notebook, cx);
    cx.read(|cx| list.read(cx).selected_count())
}

fn count(notebook: &Notebook, view: TaskView, cx: &mut TestAppContext) -> usize {
    let list = list(notebook, cx);
    cx.read(|cx| list.read(cx).board.value.counts.of(view))
}

fn keys(notebook: &Notebook, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(notebook.window.into(), keys);
    settle_tasks(notebook, cx);
}

/// Waits until the stored page reads `expected`, for changes the index has yet to see.
fn wait_stored(notebook: &Notebook, page: &str, expected: &str, cx: &mut TestAppContext) {
    wait_until(cx, "the stored page", |_| {
        stored(notebook, page) == expected
    });
}

fn wait_rows(notebook: &Notebook, expected: &[&str], cx: &mut TestAppContext) {
    crate::test_wait::until(
        cx,
        std::time::Duration::ZERO,
        |cx| {
            cx.executor()
                .advance_clock(std::time::Duration::from_millis(200));
            settle_tasks(notebook, cx);
        },
        |cx| texts(notebook, cx) == expected,
        |cx| {
            format!(
                "rows {expected:?}; listed {:?}, {:?}",
                texts(notebook, cx),
                message(notebook, cx)
            )
        },
    );
}

fn message(notebook: &Notebook, cx: &mut TestAppContext) -> (Option<String>, Option<String>) {
    let list = list(notebook, cx);
    cx.read(|cx| {
        let list = list.read(cx);
        (
            list.notice.as_ref().map(ToString::to_string),
            list.error.as_ref().map(ToString::to_string),
        )
    })
}

const WORK: &str = "- [ ] Later [[2026-11-02]] #next\n- [ ] Overdue [[2026-10-20]] #next\n- [ ] Today [[2026-10-30]]\n- [ ] Undated #next\n- [x] Fresh win ✅ 2026-10-27\n";

#[gpui::test]
fn task_lists_count_order_and_change_tasks_in_place(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("w", "Work.md", WORK),
            ("d", "2026-10-25.md", "- [ ] From a daily page\n"),
        ],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.view),
        Some(BrowserView::Tasks)
    );
    assert_eq!(
        texts(&notebook, cx),
        ["From a daily page", "Today [[2026-10-30]]"],
    );
    assert_eq!(
        [
            TaskView::Due,
            TaskView::Inbox,
            TaskView::Next,
            TaskView::Done
        ]
        .map(|view| count(&notebook, view, cx)),
        [3, 2, 3, 1]
    );
    keys(&notebook, "[ [", cx);
    assert_eq!(
        texts(&notebook, cx),
        [
            "Overdue [[2026-10-20]]",
            "From a daily page",
            "Today [[2026-10-30]]"
        ],
        "Due lists overdue tasks first; a daily page dates its tasks"
    );
    keys(&notebook, &key("enter"), cx);
    wait_stored(
        &notebook,
        "w",
        &WORK.replace(
            "- [ ] Overdue [[2026-10-20]] #next\n",
            "- [x] Overdue [[2026-10-20]] #next ✅ 2026-10-30\n",
        ),
        cx,
    );
    assert_eq!(message(&notebook, cx).0.as_deref(), Some("Completed task."));
    wait_rows(
        &notebook,
        &["From a daily page", "Today [[2026-10-30]]"],
        cx,
    );
    keys(&notebook, "down 2", cx);
    wait_stored(
        &notebook,
        "w",
        &WORK
            .replace(
                "- [ ] Overdue [[2026-10-20]] #next\n",
                "- [x] Overdue [[2026-10-20]] #next ✅ 2026-10-30\n",
            )
            .replace(
                "- [ ] Today [[2026-10-30]]\n",
                "- [ ] Today [[2026-10-30]] #next\n",
            ),
        cx,
    );
    assert_eq!(
        message(&notebook, cx).0.as_deref(),
        Some("Moved 1 task to Next.")
    );
    keys(&notebook, "[ [", cx);
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.workspace.tasks.view),
        TaskView::Done,
        "the list wraps around and is remembered"
    );
    wait_rows(&notebook, &["Overdue [[2026-10-20]]", "Fresh win"], cx);
    keys(&notebook, &key("enter"), cx);
    wait_stored(
        &notebook,
        "w",
        &WORK.replace(
            "- [ ] Today [[2026-10-30]]\n",
            "- [ ] Today [[2026-10-30]] #next\n",
        ),
        cx,
    );
    assert_eq!(message(&notebook, cx).0.as_deref(), Some("Reopened task."));
}

#[gpui::test]
fn opening_a_task_reveals_its_block(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[("a", "Alpha.md", "- start\n"), ("w", "Work.md", WORK)],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    assert_eq!(texts(&notebook, cx), ["Today [[2026-10-30]]"]);
    keys(&notebook, "enter", cx);
    wait_until(cx, "the task's page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page.read(cx).id() == "w" && view.editor.is_some() && view.nav.jump.is_none()
        })
    });
    assert_eq!(
        caret_text(&notebook, false, cx),
        "- [ ] Today [[2026-10-30]]"
    );
}

/// rt `TasksPage.tsx` 805: each row has its own state select, which moves that task alone and
/// leaves the other selected tasks where they are.
#[gpui::test]
fn a_rows_state_menu_moves_that_task_alone(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[("a", "A.md", "- [ ] One #inbox\n- [ ] Two #inbox\n")],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    wait_rows(&notebook, &["One", "Two"], cx);
    keys(&notebook, "space", cx);
    assert_eq!(selected(&notebook, cx), 1);
    super::tags_tests::click_at(&notebook, "quire-task-state-1".into(), cx);
    super::tags_tests::click_at(&notebook, "MENU_ITEM-Waiting".into(), cx);
    wait_stored(&notebook, "a", "- [ ] One #inbox\n- [ ] Two #waiting\n", cx);
    wait_until(cx, "the notice", |cx| {
        message(&notebook, cx).0.as_deref() == Some("Moved to Waiting.")
    });
    wait_rows(&notebook, &["One"], cx);
}

#[gpui::test]
fn selected_tasks_move_together_and_stale_selections_are_refused(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            (
                "a",
                "A.md",
                "- [ ] One #inbox\n- [ ] Two #inbox\n- [ ] Three #pending\n",
            ),
            ("b", "B.md", "- [ ] Four #inbox\n"),
        ],
        "a",
        cx,
    );
    let buffer = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    });
    buffer.update(cx, |buffer, cx| {
        let end = buffer.len();
        buffer.edit([(end..end, "\ntyped\n")], None, cx)
    });
    show_tasks(&notebook, cx);
    wait_rows(&notebook, &["One", "Two", "Four"], cx);
    keys(&notebook, "space down down shift-space", cx);
    assert_eq!(selected(&notebook, cx), 3);
    keys(&notebook, "down", cx);
    keys(&notebook, "2", cx);
    wait_stored(&notebook, "b", "- [ ] Four #next\n", cx);
    assert_eq!(
        stored(&notebook, "a"),
        "- [ ] One #next\n- [ ] Two #next\n- [ ] Three #pending\n\ntyped\n",
        "the open page keeps its unsaved typing"
    );
    // The open page takes in the written version after a background diff.
    let merged = "- [ ] One #next\n- [ ] Two #next\n- [ ] Three #pending\n\ntyped\n";
    wait_until(cx, "the open page to take in the move", |cx| {
        cx.read(|cx| buffer.read(cx).text()) == merged
    });
    assert_eq!(
        message(&notebook, cx).0.as_deref(),
        Some("Moved 3 tasks to Next.")
    );
    assert_eq!(selected(&notebook, cx), 0);

    keys(&notebook, "] ]", cx);
    wait_rows(&notebook, &["Three"], cx);
    keys(&notebook, "space escape", cx);
    assert_eq!(selected(&notebook, cx), 0);
    keys(&notebook, "[", cx);
    wait_rows(&notebook, &["One", "Two", "Four"], cx);
    keys(&notebook, "space down down space", cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    super::tests::save(&store, "b", "B.md", "- [ ] Four, rewritten #next\n");
    keys(&notebook, "3", cx);
    let (_, error) = message(&notebook, cx);
    assert_eq!(
        error.as_deref(),
        Some("A task on B changed since it was listed. Review the list and try again.")
    );
    assert_eq!(
        stored(&notebook, "a"),
        "- [ ] One #next\n- [ ] Two #next\n- [ ] Three #pending\n\ntyped\n",
        "a refused move writes no page"
    );
}

#[gpui::test]
fn date_controls_set_change_and_clear_task_dates(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("w", "Work.md", "- [ ] Plan #next\n"),
        ],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    keys(&notebook, "]", cx);
    wait_rows(&notebook, &["Plan"], cx);
    keys(&notebook, "d", cx);
    cx.simulate_input(notebook.window.into(), "tomorrow");
    keys(&notebook, "enter", cx);
    wait_stored(&notebook, "w", "- [ ] Plan #next 📅 [[2026-10-31]]\n", cx);
    assert_eq!(
        message(&notebook, cx).0.as_deref(),
        Some("Set the due date.")
    );
    wait_until(cx, "the due date", |cx| {
        settle_tasks(&notebook, cx);
        let list = list(&notebook, cx);
        cx.read(|cx| {
            list.read(cx)
                .rows()
                .first()
                .is_some_and(|task| task.due.as_deref() == Some("2026-10-31"))
        })
    });
    keys(&notebook, "d", cx);
    cx.simulate_input(notebook.window.into(), "soon");
    keys(&notebook, "enter", cx);
    assert!(
        message(&notebook, cx)
            .1
            .is_some_and(|error| error.contains("\"soon\" is not a date")),
        "an unreadable date is refused in place"
    );
    keys(&notebook, "escape", cx);
    keys(&notebook, "shift-d", cx);
    cx.simulate_input(notebook.window.into(), "today");
    keys(&notebook, "enter", cx);
    assert_eq!(
        message(&notebook, cx).1.as_deref(),
        Some(quire_core::tasks::OPEN_COMPLETION)
    );
    keys(&notebook, "d backspace enter", cx);
    wait_stored(&notebook, "w", "- [ ] Plan #next\n", cx);
    assert_eq!(
        message(&notebook, cx).0.as_deref(),
        Some("Cleared the due date.")
    );
}

#[gpui::test]
fn capture_creates_the_inbox_page_then_appends_and_links_projects(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("p", "Launch.md", "- #project\n"),
        ],
        "a",
        cx,
    );
    set_clock(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-t"));
    set_clock(&notebook, cx);
    settle_tasks(&notebook, cx);
    let focused = read(&notebook, cx, |view, _| view.nav.view);
    assert_eq!(focused, Some(BrowserView::Tasks));
    cx.simulate_input(notebook.window.into(), "Call Jane #meeting");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let inbox = wait_for_page(&notebook, "Inbox", cx);
    wait_stored(
        &notebook,
        &inbox,
        &format!("- [ ] Call Jane #meeting #inbox{STAMP}\n"),
        cx,
    );
    wait_rows(&notebook, &["Call Jane #meeting"], cx);
    assert_eq!(message(&notebook, cx).0.as_deref(), Some("Task captured."));

    let launch = read(&notebook, cx, |_, _| "p".to_owned());
    list(&notebook, cx).update(cx, |list, cx| list.set_capture_project(Some(launch), cx));
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-t"));
    cx.simulate_input(notebook.window.into(), "Draft brief #next");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_stored(
        &notebook,
        &inbox,
        &format!(
            "- [ ] Call Jane #meeting #inbox{STAMP}\n- [ ] Draft brief #next [[Launch]]{STAMP}\n"
        ),
        cx,
    );
    wait_rows(&notebook, &["Draft brief  [[Launch]]"], cx);
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.workspace.tasks.view),
        TaskView::Next,
        "capture shows the list the task went to"
    );
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-t"));
    keys(&notebook, "enter", cx);
    assert_eq!(
        message(&notebook, cx).1.as_deref(),
        Some(quire_core::tasks::CAPTURE_ONE),
        "an empty capture is refused"
    );
}

#[gpui::test]
fn capture_follows_the_settings_choice_to_todays_page_without_a_date(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("a", "Alpha.md", "- start\n")], "a", cx);
    cx.update(|cx| {
        crate::preferences::apply_user_settings(r#"{"quire":{"capture_target":"today"}}"#, cx)
    });
    set_clock(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-t"));
    set_clock(&notebook, cx);
    settle_tasks(&notebook, cx);
    cx.simulate_input(notebook.window.into(), "Call Jane");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let today = wait_for_page(&notebook, "2026-10-30", cx);
    wait_stored(&notebook, &today, "- [ ] Call Jane #inbox\n", cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-t"));
    cx.simulate_input(notebook.window.into(), "Book room #next");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_stored(
        &notebook,
        &today,
        "- [ ] Call Jane #inbox\n- [ ] Book room #next\n",
        cx,
    );

    cx.update(|cx| crate::preferences::apply_user_settings("{}", cx));
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-t"));
    cx.simulate_input(notebook.window.into(), "File taxes");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let inbox = wait_for_page(&notebook, "Inbox", cx);
    wait_stored(
        &notebook,
        &inbox,
        &format!("- [ ] File taxes #inbox{STAMP}\n"),
        cx,
    );
}

fn wait_for_page(notebook: &Notebook, title: &str, cx: &mut TestAppContext) -> String {
    let mut found = None;
    wait_until(cx, "the created page", |cx| {
        notebook
            .window
            .update(cx, |view, _, cx| view.refresh_catalogue(cx))
            .unwrap();
        cx.run_until_parked();
        found = read(notebook, cx, |view, _| {
            view.nav
                .pages
                .iter()
                .find(|listed| listed.page.title == title)
                .map(|listed| listed.page.page.to_lossy_string())
        });
        found.is_some()
    });
    found.unwrap()
}

#[gpui::test]
fn projects_filter_by_status_flag_missing_next_actions_and_remember_the_choice(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            (
                "o",
                "Open.md",
                "---\ntags: [project]\n---\n- [ ] Write the brief\n",
            ),
            (
                "f",
                "Finished.md",
                "---\ntags: [project]\n---\n- [x] Ship it ✅ 2026-10-20\n",
            ),
            ("e", "Empty.md", "---\ntags: [project]\n---\n- Just notes\n"),
            ("d", "Daily.md", "- [ ] Send brief [[Open]] #next\n"),
        ],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    list(&notebook, cx).update(cx, |list, cx| list.choose(TaskView::Projects, cx));
    settle_tasks(&notebook, cx);
    let shown = |cx: &mut TestAppContext| -> Vec<(String, String)> {
        let list = list(&notebook, cx);
        cx.read(|cx| {
            list.read(cx)
                .shown_projects(cx)
                .iter()
                .map(|project| (project.page.title.clone(), next_action(project).0))
                .collect()
        })
    };
    let statuses = |cx: &mut TestAppContext| {
        let list = list(&notebook, cx);
        cx.read(|cx| ProjectStatus::ALL.map(|status| list.read(cx).status_count(status)))
    };
    assert_eq!(statuses(cx), [1, 2, 3]);
    assert_eq!(shown(cx), [("Open".into(), "1 next action".into())]);
    list(&notebook, cx).update(cx, |list, cx| list.set_status(ProjectStatus::Inactive, cx));
    assert_eq!(
        shown(cx),
        [
            ("Empty".into(), "No next action".into()),
            ("Finished".into(), "All actions complete".into()),
        ]
    );
    saved(&notebook, cx);
    let notebook = reopen(notebook, "a", cx);
    let remembered = read(&notebook, cx, |view, _| view.nav.workspace.tasks.clone());
    assert_eq!(
        (remembered.view, remembered.project_status),
        (TaskView::Projects, ProjectStatus::Inactive)
    );
    show_tasks(&notebook, cx);
    list(&notebook, cx).update(cx, |list, cx| list.set_status(ProjectStatus::All, cx));
    keys(&notebook, "down down r", cx);
    wait_rows(&notebook, &["Send brief [[Open]]"], cx);
    let chosen = read(&notebook, cx, |view, _| view.nav.workspace.tasks.clone());
    assert_eq!(
        (chosen.view, chosen.project.as_deref()),
        (TaskView::Next, Some("o")),
        "Review lists a project's next actions, including tasks that link it"
    );
    wait_until(cx, "the project's open count", |cx| {
        let list = list(&notebook, cx);
        cx.read(|cx| {
            list.read(cx)
                .board
                .value
                .projects
                .iter()
                .any(|project| project.page.title == "Open" && project.open == 2)
        })
    });
}

#[gpui::test]
fn new_projects_start_with_a_next_action_and_open(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("a", "Alpha.md", "- start\n")], "a", cx);
    show_tasks(&notebook, cx);
    list(&notebook, cx).update(cx, |list, cx| list.choose(TaskView::Projects, cx));
    notebook
        .window
        .update(cx, |view, window, cx| {
            let list = view.nav.tasks.list.clone().unwrap();
            list.update(cx, |list, cx| list.start_new_project(window, cx))
        })
        .unwrap();
    cx.simulate_input(notebook.window.into(), "Atlas");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the new project page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .loaded()
                .is_some_and(|page| page.path.as_deref() == Some("Projects/Atlas.md"))
        })
    });
    let page = read(&notebook, cx, |view, cx| view.page.read(cx).id().to_owned());
    assert_eq!(
        stored(&notebook, &page),
        crate::navigation::tasks::NEW_PROJECT_TEXT
    );
}

#[gpui::test]
fn task_commands_use_the_notebook_daily_layout_and_page_titles(cx: &mut TestAppContext) {
    let layout = DailyLayout {
        folder: "Journal".into(),
        title: "{DD}.{MM}.{YYYY}".into(),
    };
    let notebook = open_notebook(
        &[
            ("j", "Journal/30.10.2026.md", "- call\n"),
            ("a", "Alpha.md", "- start\n"),
            (SETTINGS_PAGE, "notebook-settings", &write_settings(&layout)),
        ],
        "j",
        cx,
    );
    set_clock(&notebook, cx);
    let editor_key = key("enter");
    cx.simulate_keystrokes(notebook.window.into(), &editor_key);
    let text = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .persistence()
                .unwrap()
                .read(cx)
                .buffer()
                .read(cx)
                .text()
        })
    };
    assert_eq!(
        text(cx),
        "- [ ] call\n",
        "a task made on today's daily page needs no created stamp"
    );
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(quire_core::workspace::Destination::page("a"), window, cx)
        })
        .unwrap();
    wait_until(cx, "the second page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page.read(cx).id() == "a" && view.editor.is_some()
        })
    });
    cx.simulate_keystrokes(notebook.window.into(), &editor_key);
    assert_eq!(
        text(cx),
        "- [ ] start ➕ [[30.10.2026]]\n",
        "created stamps link the daily page by the configured title"
    );
}

#[gpui::test]
fn the_status_line_says_saving_until_a_task_change_is_written(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("w", "Work.md", "- [ ] One\n"),
        ],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    wait_rows(&notebook, &["One"], cx);
    let status = |cx: &mut TestAppContext| {
        let list = list(&notebook, cx);
        cx.read(|cx| list.read(cx).save_status())
    };
    assert_eq!(status(cx), "Saved locally");
    list(&notebook, cx).update(cx, |list, cx| list.toggle(0, cx));
    assert_eq!(status(cx), "Saving locally…");
    wait_stored(&notebook, "w", "- [x] One ✅ 2026-10-30\n", cx);
    wait_until(cx, "the saved status", |cx| status(cx) == "Saved locally");
}

fn list_focused(notebook: &Notebook, cx: &mut TestAppContext) -> bool {
    let list = list(notebook, cx);
    notebook
        .window
        .update(cx, |_, window, cx| list.read(cx).focus.is_focused(window))
        .unwrap()
}

#[gpui::test]
fn a_failed_task_save_writes_nothing_says_why_and_keeps_focus_for_a_retry(cx: &mut TestAppContext) {
    use sqlez::connection::Connection;
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("w", "Work.md", "- [ ] One #next\n"),
        ],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    keys(&notebook, "]", cx);
    wait_rows(&notebook, &["One"], cx);
    assert!(
        list_focused(&notebook, cx),
        "Tasks opens with its list focused"
    );

    let lock = Connection::try_open_file(notebook.path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    keys(&notebook, &key("enter"), cx);
    wait_until(cx, "the refused save", |cx| {
        settle_tasks(&notebook, cx);
        message(&notebook, cx).1.is_some()
    });
    let (_, error) = message(&notebook, cx);
    assert!(
        error
            .as_deref()
            .is_some_and(|error| error.starts_with("Cannot begin a notebook transaction")),
        "the failure says what failed and what to do: {error:?}"
    );
    assert_eq!(texts(&notebook, cx), ["One"], "the task stays listed");
    assert!(list_focused(&notebook, cx), "focus stays on the list");

    lock.exec("ROLLBACK").unwrap()().unwrap();
    assert_eq!(
        stored(&notebook, "w"),
        "- [ ] One #next\n",
        "nothing is written"
    );
    keys(&notebook, &key("enter"), cx);
    wait_stored(&notebook, "w", "- [x] One #next ✅ 2026-10-30\n", cx);
    assert!(
        list_focused(&notebook, cx),
        "focus stays on the list after the retry"
    );
}

fn filter_text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let list = list(notebook, cx);
    cx.read(|cx| list.read(cx).filter.read(cx).text(cx))
}

fn choose(notebook: &Notebook, view: TaskView, cx: &mut TestAppContext) {
    list(notebook, cx).update(cx, |list, cx| list.choose(view, cx));
    settle_tasks(notebook, cx);
}

fn chip(notebook: &Notebook, term: &str, cx: &mut TestAppContext) {
    settle_tasks(notebook, cx);
    super::tags_tests::click_at(notebook, format!("quire-task-chip-{term}"), cx);
    settle_tasks(notebook, cx);
}

const FILTERED: &str = "- [ ] Later [[2026-11-02]] #next\n- [ ] Overdue [[2026-10-20]] #next\n- [ ] Today [[2026-10-30]]\n- [ ] Undated #next\n- [x] Fresh win ✅ 2026-10-27\n- [x] Old win ✅ 2026-09-02\n- [ ] Dream #someday\n";

/// Quick filter chips toggle `due:` and `done:` terms in the filter text, which stays the one
/// remembered source of the filter; a tag term keeps tasks carrying the tag or in its state.
#[gpui::test]
fn filter_chips_toggle_date_and_tag_terms_in_the_filter_text(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[("a", "Alpha.md", "- start\n"), ("w", "Work.md", FILTERED)],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    choose(&notebook, TaskView::Next, cx);
    wait_rows(
        &notebook,
        &["Overdue [[2026-10-20]]", "Later [[2026-11-02]]", "Undated"],
        cx,
    );
    chip(&notebook, "due:overdue", cx);
    assert_eq!(filter_text(&notebook, cx), "due:overdue");
    wait_rows(&notebook, &["Overdue [[2026-10-20]]"], cx);
    chip(&notebook, "due:none", cx);
    assert_eq!(
        filter_text(&notebook, cx),
        "due:none",
        "a due chip replaces the other due term"
    );
    wait_rows(&notebook, &["Undated"], cx);
    chip(&notebook, "due:none", cx);
    assert_eq!(
        filter_text(&notebook, cx),
        "",
        "an active chip removes its term"
    );
    wait_rows(
        &notebook,
        &["Overdue [[2026-10-20]]", "Later [[2026-11-02]]", "Undated"],
        cx,
    );

    choose(&notebook, TaskView::Done, cx);
    wait_rows(&notebook, &["Fresh win", "Old win"], cx);
    chip(&notebook, "done:week", cx);
    wait_rows(&notebook, &["Fresh win"], cx);
    assert_eq!(
        read(&notebook, cx, |view, _| view
            .nav
            .workspace
            .tasks
            .query
            .clone()),
        "done:week",
        "the filter text is remembered"
    );
    chip(&notebook, "done:today", cx);
    assert_eq!(filter_text(&notebook, cx), "done:today");
    wait_rows(&notebook, &[], cx);
    chip(&notebook, "done-on", cx);
    assert_eq!(filter_text(&notebook, cx), "done:");
    let tasks = list(&notebook, cx);
    let error = cx.read(|cx| tasks.read(cx).filter_error(cx));
    assert!(
        error
            .as_deref()
            .is_some_and(|error| error.starts_with("In \"done:\": A period is missing.")),
        "{error:?}"
    );
    cx.simulate_input(notebook.window.into(), "2026-09");
    settle_tasks(&notebook, cx);
    assert_eq!(filter_text(&notebook, cx), "done:2026-09");
    wait_rows(&notebook, &["Old win"], cx);

    choose(&notebook, TaskView::All, cx);
    assert_eq!(count(&notebook, TaskView::All, cx), 7);
    let tasks = list(&notebook, cx);
    let tags = cx.read(|cx| tasks.read(cx).tags.value.clone());
    assert_eq!(tags, [("#next".to_owned(), 3), ("#someday".to_owned(), 1)]);
    let toggle = |term: &'static str, cx: &mut TestAppContext| {
        let list = list(&notebook, cx);
        notebook
            .window
            .update(cx, |_, window, cx| {
                list.update(cx, |list, cx| {
                    list.set_filter("", window, cx);
                    list.toggle_term(term, window, cx)
                })
            })
            .unwrap();
        settle_tasks(&notebook, cx);
    };
    toggle("#someday", cx);
    wait_rows(&notebook, &["Dream"], cx);
    toggle("#inbox", cx);
    wait_rows(&notebook, &["Today [[2026-10-30]]"], cx);
    chip(&notebook, "due:week", cx);
    assert_eq!(filter_text(&notebook, cx), "#inbox due:week");
    wait_rows(&notebook, &["Today [[2026-10-30]]"], cx);
    toggle("-#done", cx);
    chip(&notebook, "due:none", cx);
    wait_rows(&notebook, &["Undated", "Dream"], cx);
}

/// Every open row ends with a calendar menu and buttons that make the task due tomorrow or next
/// Monday, and T and W do the same for the selection or the highlighted task.
#[gpui::test]
fn quick_date_buttons_and_keys_push_due_dates(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("w", "Work.md", "- [ ] Plan #next\n- [ ] Call #next\n"),
        ],
        "a",
        cx,
    );
    show_tasks(&notebook, cx);
    choose(&notebook, TaskView::Next, cx);
    wait_rows(&notebook, &["Plan", "Call"], cx);
    let click = |selector: &str, cx: &mut TestAppContext| {
        settle_tasks(&notebook, cx);
        super::tags_tests::click_at(&notebook, selector.to_owned(), cx);
    };
    click("quire-task-tomorrow-1", cx);
    wait_stored(
        &notebook,
        "w",
        "- [ ] Plan #next\n- [ ] Call #next 📅 [[2026-10-31]]\n",
        cx,
    );
    wait_notice(&notebook, "Due Oct 31.", cx);
    wait_rows(&notebook, &["Call", "Plan"], cx);
    click("quire-task-next-week-0", cx);
    wait_stored(
        &notebook,
        "w",
        "- [ ] Plan #next\n- [ ] Call #next 📅 [[2026-11-02]]\n",
        cx,
    );
    wait_notice(&notebook, "Due Nov 2.", cx);
    click("quire-task-dates-0", cx);
    click("MENU_ITEM-Clear due date", cx);
    wait_stored(&notebook, "w", "- [ ] Plan #next\n- [ ] Call #next\n", cx);
    wait_rows(&notebook, &["Plan", "Call"], cx);
    click("quire-task-dates-1", cx);
    click("MENU_ITEM-Today · Oct 30", cx);
    wait_stored(
        &notebook,
        "w",
        "- [ ] Plan #next\n- [ ] Call #next 📅 [[2026-10-30]]\n",
        cx,
    );
    wait_rows(&notebook, &["Call", "Plan"], cx);

    let tasks = list(&notebook, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            let focus = tasks.read(cx).focus.clone();
            window.focus(&focus, cx);
            tasks.update(cx, |list, _| list.highlighted = 0);
        })
        .unwrap();
    keys(&notebook, "space down space w", cx);
    wait_stored(
        &notebook,
        "w",
        "- [ ] Plan #next 📅 [[2026-11-02]]\n- [ ] Call #next 📅 [[2026-11-02]]\n",
        cx,
    );
    wait_notice(&notebook, "Made 2 tasks due Nov 2.", cx);
    wait_rows(&notebook, &["Plan", "Call"], cx);
    keys(&notebook, "up t", cx);
    wait_stored(
        &notebook,
        "w",
        "- [ ] Plan #next 📅 [[2026-10-31]]\n- [ ] Call #next 📅 [[2026-11-02]]\n",
        cx,
    );
    wait_notice(&notebook, "Made 1 task due Oct 31.", cx);
}

fn wait_notice(notebook: &Notebook, expected: &str, cx: &mut TestAppContext) {
    wait_until(cx, "the notice", |cx| {
        message(notebook, cx).0.as_deref() == Some(expected)
    });
}
