//! Task query parsing and matching against vectors from the frozen rt modules
//! (`tools/task-query-vectors.ts` over `tests/reference/core/task-query.ts`).

use quire_core::task_query::{Listed, Notebook, Project, TERMS, TaskQuery, query_line, run};

/// The terms rt's unknown-term message offers.
const RT_TERMS: &str = "a state, #tag, [[Page]], project:Name, by:, sort:, limit: or words";
use quire_core::tasks::{TaskState, memberships};
use serde::Deserialize;

#[derive(Deserialize)]
struct Vectors {
    today: String,
    here: String,
    pages: Vec<VectorPage>,
    tasks: Vec<VectorTask>,
    queries: Vec<VectorQuery>,
    lines: Vec<VectorLine>,
}

#[derive(Deserialize)]
struct VectorPage {
    path: String,
    title: String,
    front: Vec<String>,
    #[serde(rename = "projectBlock")]
    project_block: bool,
}

#[derive(Deserialize)]
struct VectorTask {
    path: String,
    text: String,
    tags: Vec<String>,
    links: Vec<String>,
    state: String,
    due: Option<String>,
}

#[derive(Deserialize, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
struct Parsed {
    source: String,
    views: Vec<String>,
    tags: Vec<String>,
    not_tags: Vec<String>,
    pages: Vec<String>,
    not_pages: Vec<String>,
    projects: Vec<String>,
    not_projects: Vec<String>,
    words: Vec<String>,
    not_words: Vec<String>,
    by: Option<String>,
    sort: String,
    limit: f64,
}

#[derive(Deserialize, Debug, PartialEq)]
struct Group {
    label: String,
    path: Option<String>,
    tasks: Vec<usize>,
}

#[derive(Deserialize)]
struct VectorQuery {
    source: String,
    error: Option<String>,
    query: Option<Parsed>,
    groups: Option<Vec<Group>>,
    total: Option<usize>,
}

#[derive(Deserialize)]
struct VectorLine {
    line: String,
    query: Option<String>,
}

fn vectors() -> Vectors {
    serde_json::from_str(include_str!(
        "../../../tests/fixtures/task-query-vectors.json"
    ))
    .unwrap()
}

fn parsed(query: &TaskQuery) -> Parsed {
    Parsed {
        source: query.source.clone(),
        views: query
            .views
            .iter()
            .map(|view| view.as_str().to_owned())
            .collect(),
        tags: query.tags.clone(),
        not_tags: query.not_tags.clone(),
        pages: query.pages.clone(),
        not_pages: query.not_pages.clone(),
        projects: query.projects.clone(),
        not_projects: query.not_projects.clone(),
        words: query.words.clone(),
        not_words: query.not_words.clone(),
        by: query.by.map(|by| by.as_str().to_owned()),
        sort: query.sort.as_str().to_owned(),
        limit: query.limit as f64,
    }
}

#[test]
fn queries_parse_match_order_and_group_as_the_reference_does() {
    let vectors = vectors();
    let collator = icu_collator::Collator::try_new(Default::default(), Default::default()).unwrap();
    let title = |path: &str| {
        vectors
            .pages
            .iter()
            .find(|page| page.path == path)
            .map(|page| page.title.as_str())
            .unwrap()
    };
    let find_page = |name: &str| {
        let key = name.to_lowercase();
        vectors
            .pages
            .iter()
            .find(|page| {
                page.title.to_lowercase() == key
                    || page.path.trim_end_matches(".md").to_lowercase() == key
            })
            .map(|page| page.path.clone())
    };
    let mut projects: Vec<&VectorPage> = vectors
        .pages
        .iter()
        .filter(|page| {
            page.project_block
                || page.title.to_lowercase().starts_with("projects/")
                || page.front.iter().any(|tag| {
                    let tag = tag.to_lowercase();
                    tag == "project"
                        || tag
                            .strip_prefix("project/")
                            .is_some_and(|rest| !rest.is_empty())
                })
        })
        .collect();
    projects.sort_by(|a, b| collator.compare(&a.title, &b.title));
    let fronts: Vec<(String, Vec<String>)> = projects
        .iter()
        .map(|page| (page.path.clone(), page.front.clone()))
        .collect();
    let project_list: Vec<Project> = projects
        .iter()
        .map(|page| Project {
            page: page.path.clone(),
            title: page.title.clone(),
        })
        .collect();
    let members: Vec<Vec<usize>> = vectors
        .tasks
        .iter()
        .map(|task| {
            memberships(&task.path, &task.links, &task.tags, &fronts, find_page)
                .into_iter()
                .map(|page| fronts.iter().position(|(own, _)| own == page).unwrap())
                .collect()
        })
        .collect();
    let listed: Vec<Listed> = vectors
        .tasks
        .iter()
        .map(|task| Listed {
            page: &task.path,
            title: title(&task.path),
            text: &task.text,
            tags: &task.tags,
            links: &task.links,
            state: TaskState::parse(&task.state).unwrap(),
            due: task.due.as_deref(),
            completed: None,
        })
        .collect();
    let compare = |a: &str, b: &str| collator.compare(a, b);
    let notebook = Notebook {
        here: &vectors.here,
        today: &vectors.today,
        projects: &project_list,
        members: &members,
        find_page: &find_page,
        compare: &compare,
    };
    for case in &vectors.queries {
        let query = TaskQuery::parse(&case.source);
        if let Some(error) = &case.error {
            // Quire's unknown-term message also offers the date terms rt lacks.
            let error = error.replace(RT_TERMS, TERMS);
            assert_eq!(query.as_ref().err(), Some(&error), "{:?}", case.source);
            continue;
        }
        let query = query.unwrap();
        assert_eq!(Some(parsed(&query)), case.query, "{:?}", case.source);
        let result = run(&query, &listed, &notebook);
        let groups: Vec<Group> = result
            .groups
            .iter()
            .map(|group| Group {
                label: group.label.clone(),
                path: group.page.clone(),
                tasks: group.tasks.clone(),
            })
            .collect();
        assert_eq!(Some(groups), case.groups, "{:?}", case.source);
        assert_eq!(Some(result.total), case.total, "{:?}", case.source);
    }
    assert!(vectors.queries.len() > 300);
}

#[test]
fn query_lines_match_the_reference_pattern() {
    for case in vectors().lines {
        let found = query_line(&case.line);
        assert_eq!(
            found.as_ref().map(|line| line.query),
            case.query.as_deref(),
            "{:?}",
            case.line
        );
        if let Some(found) = found {
            let braces = &case.line[found.range];
            assert!(
                braces.starts_with("{{") && braces.ends_with("}}"),
                "{braces:?}"
            );
        }
    }
}

#[test]
fn date_terms_list_tasks_by_completion_and_due_date() {
    let tasks = [
        ("shipped", TaskState::Done, None, Some("2026-10-30")),
        (
            "filed",
            TaskState::Done,
            Some("2026-10-28"),
            Some("2026-10-19"),
        ),
        ("call", TaskState::Next, Some("2026-10-30"), None),
        ("late", TaskState::Waiting, Some("2026-10-01"), None),
        ("plan", TaskState::Someday, None, None),
        ("trip", TaskState::Inbox, Some("2026-11-20"), None),
    ];
    let listed: Vec<Listed> = tasks
        .iter()
        .map(|(text, state, due, completed)| Listed {
            page: "Work.md",
            title: "Work",
            text,
            tags: &[],
            links: &[],
            state: *state,
            due: *due,
            completed: *completed,
        })
        .collect();
    let compare = |a: &str, b: &str| a.cmp(b);
    let notebook = Notebook {
        here: "Work.md",
        today: "2026-10-30",
        projects: &[],
        members: &[],
        find_page: &|_| None,
        compare: &compare,
    };
    let texts = |source: &str| -> Vec<&str> {
        let query = TaskQuery::parse(source).unwrap_or_else(|error| panic!("{source}: {error}"));
        run(&query, &listed, &notebook).groups[0]
            .tasks
            .iter()
            .map(|index| tasks[*index].0)
            .collect()
    };
    assert_eq!(
        texts("done:today"),
        ["shipped"],
        "done: lists done tasks without a state"
    );
    assert_eq!(texts("done:month"), ["shipped", "filed"]);
    assert_eq!(texts("done:last-week"), ["filed"]);
    assert_eq!(texts("done:week -done:today"), Vec::<&str>::new());
    assert_eq!(texts("due:today"), ["call"], "due: keeps the open default");
    assert_eq!(texts("due:week"), ["call"]);
    assert_eq!(texts("all due:week"), ["filed", "call"]);
    assert_eq!(texts("due:month sort:due"), ["late", "call"]);
    assert_eq!(texts("due:overdue"), ["late"]);
    assert_eq!(texts("due:none"), ["plan"]);
    assert_eq!(texts("-due:none sort:due"), ["late", "call", "trip"]);
    assert_eq!(texts("due:2026-11"), ["trip"]);
    assert_eq!(texts("due:2026-10-01..2026-10-29"), ["late"]);
    assert_eq!(texts("someday due:none"), ["plan"]);

    let parsed = TaskQuery::parse("due:week -due:none done:2026").unwrap();
    assert_eq!(
        (parsed.dates.len(), parsed.not_dates.len()),
        (2, 1),
        "date terms are negatable"
    );
    assert_eq!(
        TaskQuery::parse("due:soon").unwrap_err(),
        format!(
            "In \"due:soon\": \"soon\" is not a period. Type one of {}. For due: also overdue or none.",
            quire_core::tasks::PERIODS
        )
    );
    assert_eq!(
        TaskQuery::parse("colour:red").unwrap_err(),
        format!("Unknown term \"colour:red\". Use {TERMS}.")
    );
}
