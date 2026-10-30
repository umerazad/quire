//! Task queries for the Tasks browser: every live task with its dates under the notebook's daily
//! layout, the counts of each list, project pages with their open, done, and next actions, and
//! one list filtered and ordered as the reference Tasks page orders it.

use super::query::{PAGE_COLUMNS, PageRef, PageRow, page_ref};
use super::{QueryIndex, REBUILD};
use anyhow::{Context as _, Result};
use quire_core::tasks::{TaskFilter, TaskState, add_days, memberships};
use quire_core::workspace::TaskView;
use std::{cmp::Ordering, collections::HashMap};

/// How far ahead Upcoming looks, in days.
pub const UPCOMING_DAYS: i64 = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskItem {
    pub source: PageRef,
    pub key: String,
    pub id: Option<String>,
    pub from: u32,
    pub raw: String,
    pub text: String,
    pub state: TaskState,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub created: Option<String>,
    pub due: Option<String>,
    pub completed: Option<String>,
}

impl TaskItem {
    pub fn is_open(&self) -> bool {
        self.state != TaskState::Done
    }

    pub fn is_due(&self, today: &str) -> bool {
        self.is_open() && self.due.as_deref().is_some_and(|due| due <= today)
    }

    pub fn is_upcoming(&self, today: &str) -> bool {
        let until = add_days(today, UPCOMING_DAYS);
        self.is_open()
            && self.due.as_deref().is_some_and(|due| {
                due > today && until.as_deref().is_some_and(|until| due <= until)
            })
    }

    fn shows_in(&self, view: TaskView, today: &str) -> bool {
        match view {
            TaskView::Due => self.is_due(today),
            TaskView::Upcoming => self.is_upcoming(today),
            TaskView::Inbox => self.state == TaskState::Inbox,
            TaskView::Next => self.state == TaskState::Next,
            TaskView::Waiting => self.state == TaskState::Waiting,
            TaskView::Someday => self.state == TaskState::Someday,
            TaskView::Done => self.state == TaskState::Done,
            TaskView::Projects => false,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskCounts {
    pub due: usize,
    pub upcoming: usize,
    pub inbox: usize,
    pub next: usize,
    pub waiting: usize,
    pub someday: usize,
    pub done: usize,
    pub projects: usize,
}

impl TaskCounts {
    pub fn of(&self, view: TaskView) -> usize {
        match view {
            TaskView::Due => self.due,
            TaskView::Upcoming => self.upcoming,
            TaskView::Inbox => self.inbox,
            TaskView::Next => self.next,
            TaskView::Waiting => self.waiting,
            TaskView::Someday => self.someday,
            TaskView::Projects => self.projects,
            TaskView::Done => self.done,
        }
    }

    pub fn open(&self) -> usize {
        self.inbox + self.next + self.waiting + self.someday
    }
}

/// A project page and its actions: tasks on it, tasks linking it, and tasks carrying its
/// `project/<name>` front matter tag.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectSummary {
    pub page: PageRef,
    pub open: usize,
    pub done: usize,
    pub next: usize,
    /// The state of its first open action, which Review opens when there is no next action.
    pub first_open: Option<TaskState>,
}

impl ProjectSummary {
    /// Active projects have at least one open action.
    pub fn is_active(&self) -> bool {
        self.open > 0
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskQuery {
    pub view: TaskView,
    pub today: String,
    pub filter: String,
    /// A project page identity; only tasks belonging to it show.
    pub project: Option<String>,
    pub limit: usize,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskBoard {
    pub counts: TaskCounts,
    /// The first `limit` tasks of the chosen list.
    pub rows: Vec<TaskItem>,
    /// How many tasks the chosen list holds in all.
    pub matching: usize,
    pub projects: Vec<ProjectSummary>,
}

type TaskRow = (
    PageRow,
    Option<String>,
    (String, Option<String>, u32, String, String, String),
    (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
    ),
);

fn parse_list(text: &str) -> Result<Vec<String>> {
    serde_json::from_str(text).context(REBUILD)
}

/// The reference `datedFirst`: dated tasks before undated ones, earliest first, or latest first
/// when `newest`.
fn dated_first(a: Option<&str>, b: Option<&str>, newest: bool) -> Ordering {
    match (a, b) {
        (Some(a), Some(b)) if newest => b.cmp(a),
        (Some(a), Some(b)) => a.cmp(b),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

impl QueryIndex {
    /// Every live task, unsaved edits included, ordered by page path and then position. Created
    /// and due dates fall back to the date of the daily page the task sits on.
    pub fn tasks(&self) -> Result<Vec<TaskItem>> {
        self.refresh()?;
        let rows: Vec<TaskRow> = self.connection.select(&format!(
            "SELECT {PAGE_COLUMNS},p.date,t.key,t.id,t.start,t.raw,t.text,t.state,t.tags,t.links,t.completed,t.created,t.due
             FROM live_tasks t JOIN live_pages p ON p.revision=t.revision WHERE p.deleted=0"
        ))?()?;
        let mut tasks = rows
            .into_iter()
            .map(
                |(
                    page,
                    date,
                    (key, id, from, raw, text, state),
                    (tags, links, completed, created, due),
                )| {
                    Ok(TaskItem {
                        source: page_ref(page)?,
                        key,
                        id,
                        from,
                        raw,
                        text,
                        state: TaskState::parse(&state).context(REBUILD)?,
                        tags: parse_list(&tags)?,
                        links: parse_list(&links)?,
                        created: created.or_else(|| date.clone()),
                        due: due.or(date),
                        completed,
                    })
                },
            )
            .collect::<Result<Vec<_>>>()?;
        tasks.sort_by(|a, b| {
            self.collator
                .compare(&a.source.path, &b.source.path)
                .then(a.from.cmp(&b.from))
        });
        Ok(tasks)
    }

    /// Everything the Tasks browser shows, read in one step so counts, projects, and rows always
    /// describe one index state.
    pub fn task_board(&self, query: &TaskQuery) -> Result<TaskBoard> {
        let tasks = self.tasks()?;
        let pages = self.project_pages()?;
        let projects: Vec<(String, Vec<String>)> = pages
            .iter()
            .map(|page| {
                let tags: Vec<String> = self.connection.select_bound(
                    "SELECT tag FROM live_front_tags WHERE revision=? ORDER BY ordinal",
                )?(page.revision.as_str())?;
                Ok((page.page.to_lossy_string(), tags))
            })
            .collect::<Result<_>>()?;
        let mut found: HashMap<String, Option<String>> = HashMap::new();
        let mut find = |link: &str| -> Option<String> {
            found
                .entry(link.to_owned())
                .or_insert_with(|| {
                    self.resolve_page(link)
                        .ok()
                        .flatten()
                        .map(|page| page.page.to_lossy_string())
                })
                .clone()
        };
        let members: Vec<Vec<usize>> = tasks
            .iter()
            .map(|task| {
                memberships(
                    &task.source.page.to_lossy_string(),
                    &task.links,
                    &task.tags,
                    &projects,
                    &mut find,
                )
                .into_iter()
                .filter_map(|id| projects.iter().position(|(page, _)| page == id))
                .collect()
            })
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
                .filter(|task| task.shows_in(view, today))
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
            .and_then(|id| projects.iter().position(|(page, _)| page == id));
        let filter = TaskFilter::new(&query.filter);
        let mut matching: Vec<&TaskItem> = tasks
            .iter()
            .zip(&members)
            .filter(|(task, belongs)| {
                task.shows_in(query.view, today)
                    && project.is_none_or(|project| belongs.contains(&project))
                    && filter.matches_parts(&task.text, &task.tags, &task.source.title)
            })
            .map(|(task, _)| task)
            .collect();
        matching.sort_by(|a, b| match query.view {
            TaskView::Done => dated_first(a.completed.as_deref(), b.completed.as_deref(), true),
            _ => dated_first(a.due.as_deref(), b.due.as_deref(), false),
        });
        Ok(TaskBoard {
            counts,
            matching: matching.len(),
            rows: matching.into_iter().take(query.limit).cloned().collect(),
            projects: summaries,
        })
    }
}

impl super::IndexClient {
    pub fn task_board(&self, query: TaskQuery) -> super::Answer<TaskBoard> {
        self.request(256 + query.filter.len(), move |index| {
            index.task_board(&query)
        })
    }
}
