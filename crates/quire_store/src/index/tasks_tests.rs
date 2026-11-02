//! The task board and linked tasks answer as the versions that read every task did, which are
//! kept here as references, while reading only the rows they show.

use super::super::test_notebook::{TODAY, TestNotebook};
use super::*;

fn shows_in(task: &TaskItem, view: TaskView, today: &str) -> bool {
    match view {
        TaskView::Due => task.is_due(today),
        TaskView::Upcoming => task.is_upcoming(today),
        TaskView::Inbox => task.state == TaskState::Inbox,
        TaskView::Next => task.state == TaskState::Next,
        TaskView::Waiting => task.state == TaskState::Waiting,
        TaskView::Someday => task.state == TaskState::Someday,
        TaskView::Done => task.state == TaskState::Done,
        TaskView::Projects => false,
        TaskView::All => true,
    }
}

/// The task board as it was: every task read, assigned to projects, counted, and sorted.
fn board_reading_every_task(index: &QueryIndex, query: &TaskQuery) -> Result<TaskBoard> {
    let tasks = index.tasks()?;
    let find = index.page_finder()?;
    let (pages, members) = index.project_members(&tasks, &find)?;
    let projects: Vec<String> = pages
        .iter()
        .map(|page| page.page.to_lossy_string())
        .collect();
    let mut summaries: Vec<ProjectSummary> = pages
        .into_iter()
        .map(|page| ProjectSummary {
            page,
            open: 0,
            done: 0,
            next: 0,
            first_open: None,
        })
        .collect();
    for (task, belongs) in tasks.iter().zip(&members) {
        for &index in belongs {
            let summary = &mut summaries[index];
            if task.is_open() {
                summary.open += 1;
                summary.first_open.get_or_insert(task.state);
            } else {
                summary.done += 1;
            }
            if task.state == TaskState::Next {
                summary.next += 1;
            }
        }
    }
    let today = query.today.as_str();
    let count = |view| {
        tasks
            .iter()
            .filter(|task| shows_in(task, view, today))
            .count()
    };
    let counts = TaskCounts {
        due: count(TaskView::Due),
        upcoming: count(TaskView::Upcoming),
        inbox: count(TaskView::Inbox),
        next: count(TaskView::Next),
        waiting: count(TaskView::Waiting),
        someday: count(TaskView::Someday),
        done: count(TaskView::Done),
        projects: summaries.len(),
    };
    let project = query
        .project
        .as_deref()
        .and_then(|id| projects.iter().position(|page| page == id));
    let filter = TaskFilter::new(&query.filter, today);
    let mut matching: Vec<&TaskItem> = tasks
        .iter()
        .zip(&members)
        .filter(|(task, belongs)| {
            shows_in(task, query.view, today)
                && project.is_none_or(|project| belongs.contains(&project))
                && filter.matches_parts(&Candidate {
                    text: &task.text,
                    tags: &task.tags,
                    page: &task.source.title,
                    state: task.state,
                    due: task.due.as_deref(),
                    completed: task.completed.as_deref(),
                })
        })
        .map(|(task, _)| task)
        .collect();
    matching.sort_by(|a, b| {
        let done = |task: &TaskItem| task.state == TaskState::Done;
        done(a).cmp(&done(b)).then_with(|| match done(a) {
            true => dated_first(a.completed.as_deref(), b.completed.as_deref(), true),
            false => dated_first(a.due.as_deref(), b.due.as_deref(), false),
        })
    });
    Ok(TaskBoard {
        counts,
        matching: matching.len(),
        rows: matching.into_iter().take(query.limit).cloned().collect(),
        projects: summaries,
    })
}

/// Linked tasks as they were: every task's links read and resolved.
fn linked_by_scanning(index: &QueryIndex, page: &str) -> Result<Vec<TaskItem>> {
    let identity = JsString::from(page).to_json();
    let Some((name, title, date)): Option<(String, String, Option<String>)> =
        index.connection.select_row_bound(
            "SELECT name_key,title_key,date FROM live_pages WHERE page=? AND deleted=0",
        )?(identity.as_str())?
    else {
        return Ok(Vec::new());
    };
    let linked = index
        .tasks()?
        .into_iter()
        .filter(|task| task.source.page.to_lossy_string() != page)
        .filter(|task| {
            task.links.iter().any(|link| {
                let key = super::super::page_key(link);
                (key == name || key == title || (date.is_some() && index.date(link) == date))
                    && index
                        .resolve_page(link)
                        .unwrap()
                        .is_some_and(|found| found.page.to_lossy_string() == page)
            })
        })
        .collect();
    Ok(linked)
}

use crate::sync::json::JsString;

fn boards(notebook: &TestNotebook) -> Result<()> {
    let index = &notebook.index;
    let launch = notebook.page("Projects/Launch.md");
    let garden = notebook.page("Garden.md");
    let plain = notebook.page("Topic 3.md");
    for view in TaskView::ALL {
        for filter in [
            "",
            "budget",
            "-plan",
            "-#topic-1",
            "#topic-1",
            "#project",
            "#next -#done",
            "launch review",
            "ملاحظات",
            "done:week",
            "done:month -done:today",
            "due:month",
            "due:overdue",
            "due:none",
            "-due:none #topic-2",
            "due:2024-01-01..2024-01-20 budget",
        ] {
            for project in [None, Some(&launch), Some(&garden), Some(&plain)] {
                for limit in [3, 200] {
                    let query = TaskQuery {
                        view,
                        today: TODAY.into(),
                        filter: filter.into(),
                        project: project.cloned(),
                        limit,
                    };
                    let board = index.task_board(&query)?;
                    assert_eq!(board, board_reading_every_task(index, &query)?, "{query:?}");
                }
            }
        }
    }
    Ok(())
}

#[test]
fn the_task_board_answers_as_reading_every_task_did() -> Result<()> {
    let notebook = TestNotebook::new(240, 7)?;
    let board = notebook.index.task_board(&TaskQuery {
        view: TaskView::Inbox,
        today: TODAY.into(),
        limit: 200,
        ..Default::default()
    })?;
    assert_eq!(board.projects.len(), 2, "both project pages are found");
    assert!(
        board
            .projects
            .iter()
            .all(|project| project.open + project.done > 0)
    );
    assert!(board.counts.due > 0 && board.counts.upcoming > 0 && board.counts.done > 0);
    boards(&notebook)?;
    notebook.open("Projects/Launch.md", 99)?;
    notebook.open("Topic 3.md", 98)?;
    notebook.open("daily/2024-01-05.md", 97)?;
    boards(&notebook)
}

/// All lists every task, open ones by due date and then done ones by latest completion, and its
/// date terms keep the tasks done or due in their periods.
#[test]
fn the_all_list_spans_states_and_date_terms_narrow_it() -> Result<()> {
    let notebook = TestNotebook::new(240, 7)?;
    let index = &notebook.index;
    let tasks = index.tasks()?;
    let board = |filter: &str| {
        index.task_board(&TaskQuery {
            view: TaskView::All,
            today: TODAY.into(),
            filter: filter.into(),
            limit: 10_000,
            ..Default::default()
        })
    };
    let all = board("")?;
    assert_eq!(all.matching, tasks.len());
    assert_eq!(all.counts.of(TaskView::All), tasks.len());
    let first_done = all.rows.iter().position(|task| !task.is_open()).unwrap();
    assert!(first_done > 0, "open tasks come first");
    assert!(all.rows[first_done..].iter().all(|task| !task.is_open()));
    let completions: Vec<_> = all.rows[first_done..]
        .iter()
        .map(|task| task.completed.clone())
        .collect();
    assert!(
        completions.windows(2).all(|pair| pair[0] >= pair[1]),
        "done tasks come latest first"
    );

    // 2024-02-10 is a Saturday: its week runs from Monday 5 to Sunday 11 February.
    let week = board("done:week")?;
    assert!(week.matching > 0);
    assert!(week.rows.iter().all(|task| {
        !task.is_open()
            && task
                .completed
                .as_deref()
                .is_some_and(|day| ("2024-02-05"..="2024-02-11").contains(&day))
    }));
    let due = board("due:2024-01")?;
    assert!(due.matching > 0);
    assert!(due.rows.iter().all(|task| {
        task.due
            .as_deref()
            .is_some_and(|day| day.starts_with("2024-01"))
    }));
    let undated = board("due:none")?.matching;
    let dated = board("-due:none")?.matching;
    assert_eq!(undated + dated, tasks.len());
    assert!(undated > 0 && dated > 0);
    Ok(())
}

#[test]
fn task_tags_count_the_tasks_carrying_each_tag() -> Result<()> {
    let notebook = TestNotebook::new(240, 7)?;
    let index = &notebook.index;
    notebook.open("Topic 3.md", 98)?;
    let mut expected: HashMap<String, usize> = HashMap::new();
    for task in index.tasks()? {
        for tag in task.tags {
            *expected.entry(tag.to_lowercase()).or_default() += 1;
        }
    }
    let mut expected: Vec<(String, usize)> = expected.into_iter().collect();
    expected.sort_by(|(a, x), (b, y)| y.cmp(x).then(a.cmp(b)));
    expected.truncate(TASK_TAG_LIMIT);
    assert!(expected.len() > 3);
    assert_eq!(index.task_tags()?, expected);
    Ok(())
}

/// The board reads the chosen list's matching rows and its first `limit` tasks in full, not
/// every task.
#[test]
fn the_task_board_reads_only_the_list_it_shows() -> Result<()> {
    let notebook = TestNotebook::new(240, 7)?;
    let index = &notebook.index;
    let tasks: Option<usize> = index
        .connection
        .select_row("SELECT count(*) FROM tasks WHERE state='waiting'")?(
    )?;
    let all: Option<usize> = index.connection.select_row("SELECT count(*) FROM tasks")?()?;
    let (waiting, all) = (tasks.unwrap(), all.unwrap());
    let actions: Option<usize> = index.connection.select_row(
        "SELECT count(*) FROM tasks t JOIN pages p ON p.revision=t.revision
         WHERE p.path IN ('Projects/Launch.md','Garden.md') OR instr(lower(t.tags),'project/')>0
           OR EXISTS (SELECT 1 FROM task_links k WHERE k.revision=t.revision AND k.ordinal=t.ordinal
             AND k.page_key IN ('launch','garden','projects/launch'))",
    )?()?;
    index.rows_read.set(0);
    index.task_board(&TaskQuery {
        view: TaskView::Waiting,
        today: TODAY.into(),
        limit: 5,
        ..Default::default()
    })?;
    let read = index.rows_read.get();
    assert!(
        read <= waiting + 5 + actions.unwrap(),
        "read {read} rows for {waiting} waiting tasks of {all}"
    );
    assert!(read < all, "read {read} rows of {all} tasks");
    Ok(())
}

#[test]
fn linked_tasks_answer_as_scanning_every_task_did() -> Result<()> {
    let notebook = TestNotebook::new(240, 11)?;
    let compare = || -> Result<usize> {
        let mut found = 0;
        for path in &notebook.paths {
            let page = notebook.page(path);
            let linked = notebook.index.linked_tasks(&page)?;
            assert_eq!(
                linked,
                linked_by_scanning(&notebook.index, &page)?,
                "{path}"
            );
            found += linked.len();
        }
        Ok(found)
    };
    assert!(compare()? > 50, "the notebook links many tasks");
    notebook.open("Topic 5.md", 3)?;
    notebook.open("daily/2024-01-03.md", 4)?;
    compare()?;
    Ok(())
}

/// Linked tasks read the task links that could name the page, not every task's links.
#[test]
fn linked_tasks_read_only_links_that_could_name_the_page() -> Result<()> {
    let notebook = TestNotebook::new(240, 11)?;
    let index = &notebook.index;
    let all: Option<usize> = index
        .connection
        .select_row("SELECT count(*) FROM task_links")?()?;
    let naming: Option<usize> = index.connection.select_row(
        "SELECT count(*) FROM task_links WHERE page_key IN ('topic 5','topic 5.md')",
    )?()?;
    index.rows_read.set(0);
    index.linked_tasks(&notebook.page("Topic 5.md"))?;
    let read = index.rows_read.get();
    assert!(read <= naming.unwrap(), "read {read} of {all:?} task links");
    Ok(())
}

/// Project pages found by index lookups are the pages a test of every page finds.
#[test]
fn project_pages_are_found_as_testing_every_page_did() -> Result<()> {
    let notebook = TestNotebook::new(120, 17)?;
    let index = &notebook.index;
    let every_page = || -> Result<Vec<PageRef>> {
        let rows: Vec<PageRow> = index.connection.select(&format!(
            "SELECT {PAGE_COLUMNS} FROM live_pages p WHERE p.deleted=0 AND (
               EXISTS (SELECT 1 FROM live_tagged t WHERE t.revision=p.revision AND t.tag_key='project')
               OR p.title_key LIKE 'projects/%'
               OR EXISTS (SELECT 1 FROM live_front_tags f WHERE f.revision=p.revision
                 AND (lower(f.tag)='project' OR lower(f.tag) LIKE 'project/_%')))"
        ))?()?;
        let mut pages = rows.into_iter().map(page_ref).collect::<Result<Vec<_>>>()?;
        pages.sort_by(|a, b| index.collator.compare(&a.title, &b.title));
        Ok(pages)
    };
    assert_eq!(index.project_pages()?.len(), 2);
    assert_eq!(index.project_pages()?, every_page()?);
    for (path, title) in [
        ("Topic 1.md", "Projects/Plan"),
        ("Topic 2.md", "projects/"),
        ("Garden.md", "Projectsx"),
    ] {
        index.set_overlay(
            &JsString::from(notebook.page(path).as_str()),
            &super::super::Overlay {
                path: path.into(),
                title: title.into(),
                index: Default::default(),
            },
        )?;
    }
    let pages = index.project_pages()?;
    assert_eq!(
        pages.len(),
        3,
        "two titled projects join Launch; Garden lost its tag"
    );
    assert_eq!(pages, every_page()?);
    Ok(())
}
