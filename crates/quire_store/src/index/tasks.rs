//! Task queries for the Tasks browser: every live task with its dates under the notebook's daily
//! layout, the counts of each list, project pages with their open, done, and next actions, and
//! one list filtered and ordered as the reference Tasks page orders it.

use super::query::{PAGE_COLUMNS, PageRef, PageRow, page_ref};
use super::{QueryIndex, REBUILD};
use anyhow::{Context as _, Result};
use quire_core::task_query;
use quire_core::tasks::{TaskFilter, TaskState, add_days, memberships};
use quire_core::workspace::{TaskStatus, TaskView};
use std::{cell::RefCell, cmp::Ordering, collections::HashMap};

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

    /// The reference `statusMatches` for tag and linked task filters.
    pub fn has_status(&self, status: TaskStatus, today: &str) -> bool {
        match status {
            TaskStatus::All => true,
            TaskStatus::Open => self.is_open(),
            TaskStatus::Due => self.is_due(today),
            TaskStatus::Done => self.state == TaskState::Done,
        }
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

/// Live tasks with their pages, narrowed by `clause`.
fn task_select(clause: &str) -> String {
    format!(
        "SELECT {PAGE_COLUMNS},p.date,t.key,t.id,t.start,t.raw,t.text,t.state,t.tags,t.links,t.completed,t.created,t.due
         FROM live_tasks t JOIN live_pages p ON p.revision=t.revision WHERE p.deleted=0{clause}"
    )
}

/// A task row with created and due dates falling back to the date of its daily page.
fn task_item(
    (page, date, (key, id, from, raw, text, state), (tags, links, completed, created, due)): TaskRow,
) -> Result<TaskItem> {
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
}

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
        let rows: Vec<TaskRow> = self.connection.select(&task_select(""))?()?;
        let mut tasks = rows
            .into_iter()
            .map(task_item)
            .collect::<Result<Vec<_>>>()?;
        self.sort_tasks(&mut tasks);
        Ok(tasks)
    }

    fn sort_tasks(&self, tasks: &mut [TaskItem]) {
        tasks.sort_by(|a, b| {
            self.collator
                .compare(&a.source.path, &b.source.path)
                .then(a.from.cmp(&b.from))
        });
    }

    /// The tasks among `tag`'s tagged blocks, by the revision and offset its tagged block reports,
    /// for the tag page's task status filter.
    pub fn tagged_tasks(&self, tag: &str) -> Result<HashMap<(String, u32), TaskItem>> {
        let tasks = self.tasks()?;
        let starts: Vec<(String, u32, u32)> = self.connection.select_bound(
            "SELECT g.revision,g.start,b.start FROM live_tagged g
             JOIN live_blocks b ON b.revision=g.revision AND b.content_from=g.start
             WHERE g.tag_key=?",
        )?(super::tag_key(tag))?;
        let mut tagged: HashMap<(String, u32), u32> = starts
            .into_iter()
            .map(|(revision, from, block)| ((revision, block), from))
            .collect();
        Ok(tasks
            .into_iter()
            .filter_map(|task| {
                let from = tagged.remove(&(task.source.revision.clone(), task.from))?;
                Some(((task.source.revision.clone(), from), task))
            })
            .collect())
    }

    /// The reference `linkedTasks`: tasks on other pages that link to `page`, directly or through
    /// a parent block, such as action items written under a [[Meeting]] line.
    ///
    /// Only links that could name the page, by its name, title, or date, are resolved, and only
    /// matching tasks are read in full: reading and sorting all 48,260 tasks at 5x on every
    /// keystroke kept the index thread busy while typing.
    pub fn linked_tasks(&self, page: &str) -> Result<Vec<TaskItem>> {
        self.refresh()?;
        let identity = crate::sync::json::JsString::from(page).to_json();
        let Some((name, title, date)): Option<(String, String, Option<String>)> =
            self.connection.select_row_bound(
                "SELECT name_key,title_key,date FROM live_pages WHERE page=? AND deleted=0",
            )?(identity.as_str())?
        else {
            return Ok(Vec::new());
        };
        let candidates: Vec<(String, i64, String)> = self.connection.select_bound(
            "SELECT t.revision,t.ordinal,t.links FROM live_tasks t JOIN live_pages p ON p.revision=t.revision
             WHERE p.deleted=0 AND p.page!=? AND t.links!='[]'",
        )?(identity.as_str())?;
        let mut names: HashMap<String, bool> = HashMap::new();
        let mut names_page = |link: &str| -> Result<bool> {
            if let Some(known) = names.get(link) {
                return Ok(*known);
            }
            let key = super::page_key(link);
            let could = key == name || key == title || (date.is_some() && self.date(link) == date);
            let found = could
                && self
                    .resolve_page(link)?
                    .is_some_and(|found| found.page.to_lossy_string() == page);
            names.insert(link.to_owned(), found);
            Ok(found)
        };
        let mut linked = Vec::new();
        for (revision, ordinal, links) in candidates {
            let mut matched = false;
            for link in parse_list(&links)? {
                if names_page(&link)? {
                    matched = true;
                    break;
                }
            }
            if matched {
                let row: Option<TaskRow> = self
                    .connection
                    .select_row_bound(&task_select(" AND t.revision=? AND t.ordinal=?"))?(
                    (
                    revision.as_str(),
                    ordinal,
                )
                )?;
                linked.push(task_item(row.context(REBUILD)?)?);
            }
        }
        self.sort_tasks(&mut linked);
        Ok(linked)
    }

    /// Resolves a page name to a page identity, remembering answers for one query.
    fn page_finder(&self) -> Result<impl Fn(&str) -> Option<String> + '_> {
        let resolve = self.page_resolver()?;
        let found: RefCell<HashMap<String, Option<String>>> = RefCell::default();
        Ok(move |name: &str| {
            found
                .borrow_mut()
                .entry(name.to_owned())
                .or_insert_with(|| {
                    resolve(name)
                        .ok()
                        .flatten()
                        .map(|page| page.page.to_lossy_string())
                })
                .clone()
        })
    }

    /// Project pages, ordered by title, and for each task the projects it belongs to.
    fn project_members(
        &self,
        tasks: &[TaskItem],
        find: &dyn Fn(&str) -> Option<String>,
    ) -> Result<(Vec<PageRef>, Vec<Vec<usize>>)> {
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
        let members = tasks
            .iter()
            .map(|task| {
                memberships(
                    &task.source.page.to_lossy_string(),
                    &task.links,
                    &task.tags,
                    &projects,
                    find,
                )
                .into_iter()
                .filter_map(|id| projects.iter().position(|(page, _)| page == id))
                .collect()
            })
            .collect();
        Ok((pages, members))
    }

    /// Everything the Tasks browser shows, read in one step so counts, projects, and rows always
    /// describe one index state.
    pub fn task_board(&self, query: &TaskQuery) -> Result<TaskBoard> {
        let tasks = self.tasks()?;
        let find = self.page_finder()?;
        let (pages, members) = self.project_members(&tasks, &find)?;
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
            .and_then(|id| projects.iter().position(|page| page == id));
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

/// A live task list written in a page: the parsed query, the page holding it, and today.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskListQuery {
    pub query: task_query::TaskQuery,
    /// The identity of the page holding the query, which `here` names.
    pub here: String,
    pub today: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskListGroup {
    /// Empty without grouping.
    pub label: String,
    /// The page a page or project group names.
    pub page: Option<String>,
    pub tasks: Vec<TaskItem>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskList {
    pub groups: Vec<TaskListGroup>,
    /// How many tasks match, beyond the limit too.
    pub total: usize,
}

impl TaskList {
    pub fn shown(&self) -> usize {
        self.groups.iter().map(|group| group.tasks.len()).sum()
    }
}

impl QueryIndex {
    /// The tasks a `{{tasks: ...}}` block lists, unsaved edits included.
    pub fn task_list(&self, list: &TaskListQuery) -> Result<TaskList> {
        let tasks = self.tasks()?;
        let find = self.page_finder()?;
        let (projects, members) = if list.query.uses_projects() {
            let (pages, members) = self.project_members(&tasks, &find)?;
            let projects = pages
                .into_iter()
                .map(|page| task_query::Project {
                    page: page.page.to_lossy_string(),
                    title: page.title,
                })
                .collect();
            (projects, members)
        } else {
            (Vec::new(), Vec::new())
        };
        let pages: Vec<String> = tasks
            .iter()
            .map(|task| task.source.page.to_lossy_string())
            .collect();
        let listed: Vec<task_query::Listed> = tasks
            .iter()
            .zip(&pages)
            .map(|(task, page)| task_query::Listed {
                page,
                title: &task.source.title,
                text: &task.text,
                tags: &task.tags,
                links: &task.links,
                state: task.state,
                due: task.due.as_deref(),
            })
            .collect();
        let compare = |a: &str, b: &str| self.collator.compare(a, b);
        let listing = task_query::run(
            &list.query,
            &listed,
            &task_query::Notebook {
                here: &list.here,
                today: &list.today,
                projects: &projects,
                members: &members,
                find_page: &find,
                compare: &compare,
            },
        );
        Ok(TaskList {
            total: listing.total,
            groups: listing
                .groups
                .into_iter()
                .map(|group| TaskListGroup {
                    label: group.label,
                    page: group.page,
                    tasks: group
                        .tasks
                        .into_iter()
                        .map(|index| tasks[index].clone())
                        .collect(),
                })
                .collect(),
        })
    }
}
