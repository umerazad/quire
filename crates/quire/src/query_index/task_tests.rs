//! Task rows through the production analyzer and index: the Tasks browser's counts, lists,
//! ordering, and projects, ported from rt `tests/tasks-page.test.tsx`.

use super::*;
use gpui::TestAppContext;
use quire_core::workspace::TaskView;
use quire_store::index::{TaskBoard, TaskQuery};
use quire_store::notebook::{Edit, NotebookStore};

const TODAY: &str = "2026-10-28";

async fn index(
    pages: &[(&str, &str)],
    cx: &mut TestAppContext,
) -> (IndexClient, tempfile::TempDir, Entity<IndexSync>) {
    cx.executor().allow_parking();
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "local").unwrap();
    for (path, text) in pages {
        store
            .save(&Edit {
                page: (*path).into(),
                path: (*path).into(),
                text: (*text).into(),
                base: vec![],
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
    }
    let sync = cx
        .update(|cx| IndexSync::spawn(database, default_config(DailyCalendar::utc()), cx))
        .unwrap();
    let client = cx.read(|cx| sync.read(cx).client().clone());
    super::tests::settle(&client, cx).await;

    (client, root, sync)
}

async fn board(client: &IndexClient, view: TaskView, filter: &str) -> TaskBoard {
    board_for(client, view, filter, None).await
}

async fn board_for(
    client: &IndexClient,
    view: TaskView,
    filter: &str,
    project: Option<&str>,
) -> TaskBoard {
    client
        .task_board(TaskQuery {
            view,
            today: TODAY.into(),
            filter: filter.into(),
            project: project.map(str::to_owned),
            limit: 40,
        })
        .unwrap()
        .await
        .unwrap()
}

fn texts(board: &TaskBoard) -> Vec<(String, Option<String>)> {
    board
        .rows
        .iter()
        .map(|task| {
            let date = if task.state == quire_core::tasks::TaskState::Done {
                task.completed.clone()
            } else {
                task.due.clone()
            };
            (task.text.clone(), date)
        })
        .collect()
}

fn pair(text: &str, date: Option<&str>) -> (String, Option<String>) {
    (text.into(), date.map(str::to_owned))
}

#[gpui::test]
async fn due_next_and_done_lists_follow_the_reference_counts_and_order(cx: &mut TestAppContext) {
    let (client, _root, _sync) = index(
        &[
            (
                "Work.md",
                "- [ ] Later [[2026-11-02]] #next\n- [ ] Overdue [[2026-10-20]] #next\n- [ ] Today [[2026-10-28]]\n- [ ] Undated #next\n- [x] Old win ✅ 2026-10-01\n- [x] Fresh win ✅ 2026-10-27\n",
            ),
            ("2026-10-25.md", "- [ ] From a daily page\n"),
            ("Code.md", "```\n- [ ] not a task\n```\n"),
        ],
        cx,
    )
    .await;
    let due = board(&client, TaskView::Due, "").await;
    assert_eq!(due.counts.due, 3);
    assert_eq!(
        (due.counts.inbox, due.counts.next, due.counts.done),
        (2, 3, 2)
    );
    assert_eq!(
        texts(&due),
        [
            pair("Overdue [[2026-10-20]]", Some("2026-10-20")),
            pair("From a daily page", Some("2026-10-25")),
            pair("Today [[2026-10-28]]", Some("2026-10-28")),
        ],
        "a task on a daily page is due that day"
    );
    assert_eq!(
        texts(&board(&client, TaskView::Next, "").await),
        [
            pair("Overdue [[2026-10-20]]", Some("2026-10-20")),
            pair("Later [[2026-11-02]]", Some("2026-11-02")),
            pair("Undated", None),
        ]
    );
    assert_eq!(
        texts(&board(&client, TaskView::Done, "").await),
        [
            pair("Fresh win", Some("2026-10-27")),
            pair("Old win", Some("2026-10-01")),
        ],
        "done tasks list the latest completion first"
    );
    let filtered = board(&client, TaskView::Next, "later").await;
    assert_eq!((filtered.matching, filtered.counts.next), (1, 3));
}

#[gpui::test]
async fn upcoming_lists_the_next_week_soonest_first_and_hides_excluded_tags(
    cx: &mut TestAppContext,
) {
    let (client, _root, _sync) = index(
        &[(
            "A.md",
            "- [ ] Later #next 📅 [[2026-11-03]]\n- [ ] Soon #next 📅 [[2026-10-29]]\n- [ ] Side #next #cloud 📅 [[2026-10-30]]\n- [ ] Far #next 📅 [[2026-11-06]]\n- [ ] Now #next 📅 [[2026-10-28]]\n",
        )],
        cx,
    )
    .await;
    let names = |board: &TaskBoard| -> Vec<String> {
        board.rows.iter().map(|task| task.text.clone()).collect()
    };
    let upcoming = board(&client, TaskView::Upcoming, "").await;
    assert_eq!(names(&upcoming), ["Soon", "Side  #cloud", "Later"]);
    assert_eq!(upcoming.counts.upcoming, 3);
    assert_eq!(
        names(&board(&client, TaskView::Upcoming, "-#cloud").await),
        ["Soon", "Later"]
    );
}

#[gpui::test]
async fn projects_come_from_front_matter_tags_links_and_their_pages(cx: &mut TestAppContext) {
    let (client, _root, _sync) = index(
        &[
            (
                "Projects/Atlas.md",
                "---\ntags:\n  - project/atlas\n---\n# Atlas\n\n- Goal: launch\n",
            ),
            (
                "Areas/Hiring.md",
                "---\ntags: [project, people]\n---\n- Open roles\n",
            ),
            (
                "2026-10-27-Tuesday.md",
                "- Sync\n  - [ ] Book the venue #project/atlas\n  - [ ] Draft offer [[Hiring]] #next\n",
            ),
            ("Notes.md", "- Thinking about #project/atlas later\n"),
            (
                "Launch.md",
                "- #project\n- [ ] Define outcome #inbox ^outcome\n- [x] Kickoff ✅ 2026-10-01\n",
            ),
            ("Finished.md", "---\ntags: [project]\n---\n- [x] Ship it ✅ 2026-10-20\n"),
        ],
        cx,
    )
    .await;
    let projects = board(&client, TaskView::Projects, "").await.projects;
    let summary: Vec<_> = projects
        .iter()
        .map(|project| {
            (
                project.page.title.as_str(),
                project.open,
                project.done,
                project.next,
                project.is_active(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            ("Atlas", 1, 0, 0, true),
            ("Finished", 0, 1, 0, false),
            ("Hiring", 1, 0, 1, true),
            ("Launch", 1, 1, 0, true),
        ],
        "daily and ordinary pages with a nested project tag are not projects"
    );
    let atlas = board_for(&client, TaskView::Inbox, "", Some("Projects/Atlas.md")).await;
    assert_eq!(
        atlas
            .rows
            .iter()
            .map(|task| task.text.as_str())
            .collect::<Vec<_>>(),
        ["Book the venue #project/atlas"]
    );
    assert_eq!(atlas.counts.inbox, 2, "counts ignore the project filter");
}
