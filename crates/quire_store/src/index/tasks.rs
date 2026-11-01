//! Task queries for the Tasks browser: every live task with its dates under the notebook's daily
//! layout, the counts of each list, project pages with their open, done, and next actions, and
//! one list filtered and ordered as the reference Tasks page orders it.

use super::query::{PAGE_COLUMNS, PageRef, PageRow, page_ref};
use super::{QueryIndex, REBUILD, strip_markdown};
use anyhow::{Context as _, Result};
use quire_core::task_query;
use quire_core::tasks::{TaskFilter, TaskState, add_days, memberships};
use quire_core::workspace::{TaskStatus, TaskView};
use std::{
    cell::RefCell,
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
};

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

/// Saved revisions whose task rows lists skip: open pages, which their unsaved rows stand for,
/// and deleted pages.
const SKIPPED: &str =
    "SELECT revision FROM main.pages WHERE page IN (SELECT page FROM temp.overlay_pages)
   UNION SELECT revision FROM main.pages WHERE analyzed=1 AND deleted=1";
/// The states of open tasks, as SQL.
const OPEN: &str = "'inbox','next','waiting','someday'";

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
    /// Task links are indexed by page key and date, so only links that could name the page, by
    /// its name, title, or date, are read and resolved, and only matching tasks are read in full.
    /// Scanning every task's links on each index change took 27% of the index thread's time while
    /// typing at 5x.
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
            "SELECT k.revision,k.ordinal,k.target FROM live_task_links k JOIN live_pages p ON p.revision=k.revision
               WHERE p.deleted=0 AND p.page!=?1 AND k.page_key IN (?2,?3)
             UNION SELECT k.revision,k.ordinal,k.target FROM live_task_links k JOIN live_pages p ON p.revision=k.revision
               WHERE p.deleted=0 AND p.page!=?1 AND k.date=?4",
        )?((identity.as_str(), name.as_str(), title.as_str(), date.as_deref()))?;
        self.count_rows(candidates.len());
        let mut names: HashMap<String, bool> = HashMap::new();
        let mut matched = BTreeSet::new();
        for (revision, ordinal, target) in candidates {
            let found = match names.get(&target) {
                Some(found) => *found,
                None => {
                    let found = self
                        .resolve_page(&target)?
                        .is_some_and(|found| found.page.to_lossy_string() == page);
                    names.insert(target, found);
                    found
                }
            };
            if found {
                matched.insert((revision, ordinal));
            }
        }
        let mut task = self
            .connection
            .select_row_bound(&task_select(" AND t.revision=? AND t.ordinal=?"))?;
        let mut linked = matched
            .iter()
            .map(|(revision, ordinal)| {
                let row: Option<TaskRow> = task((revision.as_str(), *ordinal))?;
                task_item(row.context(REBUILD)?)
            })
            .collect::<Result<Vec<_>>>()?;
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
    ///
    /// Counts are tallied in SQL, project actions are read only for tasks that could belong to a
    /// project, and the chosen list reads only the columns its filter and order need before
    /// reading its first `limit` tasks in full: reading all 48,260 tasks at 5x took 245 ms.
    pub fn task_board(&self, query: &TaskQuery) -> Result<TaskBoard> {
        self.refresh()?;
        let today = query.today.as_str();
        let until = add_days(today, UPCOMING_DAYS);
        let (pages, actions) = self.project_actions()?;
        let mut summaries: Vec<ProjectSummary> = pages
            .iter()
            .map(|page| ProjectSummary {
                page: page.clone(),
                open: 0,
                done: 0,
                next: 0,
                first_open: None,
            })
            .collect();
        for (task, belongs) in &actions {
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
        let counts = TaskCounts {
            projects: summaries.len(),
            ..self.task_counts(today, until.as_deref())?
        };
        let project = query.project.as_deref().and_then(|id| {
            pages
                .iter()
                .position(|page| page.page.to_lossy_string() == id)
        });
        let members: Option<BTreeSet<(&str, u32)>> = project.map(|project| {
            actions
                .iter()
                .filter(|(_, belongs)| belongs.contains(&project))
                .map(|(task, _)| (task.source.revision.as_str(), task.from))
                .collect()
        });
        let (matching, total) = self.task_list_rows(query, until.as_deref(), members.as_ref())?;
        let mut task = self
            .connection
            .select_row_bound(&task_select(" AND t.revision=? AND t.ordinal=?"))?;
        self.count_rows(matching.len());
        let rows = matching
            .iter()
            .map(|listed| {
                let row: Option<TaskRow> = task((listed.revision.as_str(), listed.ordinal))?;
                task_item(row.context(REBUILD)?)
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(TaskBoard {
            counts,
            matching: total,
            rows,
            projects: summaries,
        })
    }

    /// Each list's count, from the state and day index alone: every saved row, less the saved
    /// rows of skipped pages, plus unsaved rows.
    fn task_counts(&self, today: &str, until: Option<&str>) -> Result<TaskCounts> {
        let mut totals = [0i64; 7];
        for (rows, sign) in [
            ("main.tasks".to_owned(), 1),
            (format!("main.tasks WHERE revision IN ({SKIPPED})"), -1),
            ("temp.overlay_tasks".to_owned(), 1),
        ] {
            let groups: Vec<(String, i64, i64, i64)> = self.connection.select_bound(&format!(
                "SELECT state,count(*),count(CASE WHEN day<=?1 THEN 1 END),
                   count(CASE WHEN day>?1 AND day<=?2 THEN 1 END) FROM {rows} GROUP BY state"
            ))?((today, until))?;
            for (state, all, due, upcoming) in groups {
                let state = TaskState::parse(&state).context(REBUILD)?;
                let list = TaskState::ALL.iter().position(|each| *each == state);
                totals[list.context(REBUILD)?] += sign * all;
                if state != TaskState::Done {
                    totals[5] += sign * due;
                    totals[6] += sign * upcoming;
                }
            }
        }
        let [inbox, next, waiting, someday, done, due, upcoming] =
            totals.map(|count| count.max(0) as usize);
        Ok(TaskCounts {
            due,
            upcoming,
            inbox,
            next,
            waiting,
            someday,
            done,
            projects: 0,
        })
    }

    /// The tasks of the chosen list that pass its filter and project, the first `limit` of them in
    /// the reference order (dated first, then by page path and position), and how many pass.
    /// Saved and unsaved rows are read from their tables directly: joining the live views per
    /// row tripled the time of the same reads.
    fn task_list_rows(
        &self,
        query: &TaskQuery,
        until: Option<&str>,
        members: Option<&BTreeSet<(&str, u32)>>,
    ) -> Result<(Vec<Listed>, usize)> {
        let filter = TaskFilter::new(&query.filter);
        let tested = if filter.is_empty() {
            "'','[]',''"
        } else {
            "t.text,t.tags,p.title"
        };
        let select = |condition: &str, date: &str| {
            format!(
                "SELECT t.revision,t.ordinal,t.start,p.path,{date},{tested}
                 FROM main.tasks t JOIN main.pages p ON p.revision=t.revision
                 WHERE {condition} AND t.revision NOT IN ({SKIPPED})
                 UNION ALL SELECT t.revision,t.ordinal,t.start,p.path,{date},{tested}
                 FROM temp.overlay_tasks t JOIN temp.overlay_pages p ON p.revision=t.revision
                 WHERE {condition}"
            )
        };
        let state = |state: TaskState| -> Result<Vec<ListedRow>> {
            let date = match state {
                TaskState::Done => "t.completed",
                _ => "t.day",
            };
            self.connection.select_bound(&select("t.state=?1", date))?(state.as_str())
        };
        let rows: Vec<ListedRow> = match query.view {
            TaskView::Projects => return Ok((Vec::new(), 0)),
            TaskView::Due => self.connection.select_bound(&select(
                &format!("t.state IN ({OPEN}) AND t.day<=?1"),
                "t.day",
            ))?(query.today.as_str())?,
            TaskView::Upcoming => self.connection.select_bound(&select(
                &format!("t.state IN ({OPEN}) AND t.day>?1 AND t.day<=?2"),
                "t.day",
            ))?((query.today.as_str(), until))?,
            TaskView::Inbox => state(TaskState::Inbox)?,
            TaskView::Next => state(TaskState::Next)?,
            TaskView::Waiting => state(TaskState::Waiting)?,
            TaskView::Someday => state(TaskState::Someday)?,
            TaskView::Done => state(TaskState::Done)?,
        };
        self.count_rows(rows.len());
        let mut listed = Vec::new();
        for (revision, ordinal, from, path, date, (text, tags, title)) in rows {
            if members.is_some_and(|members| !members.contains(&(revision.as_str(), from))) {
                continue;
            }
            if !filter.is_empty() && !filter.matches_parts(&text, &parse_list(&tags)?, &title) {
                continue;
            }
            listed.push(Listed {
                revision,
                ordinal,
                from,
                path,
                date,
            });
        }
        let total = listed.len();
        let newest = query.view == TaskView::Done;
        let order = |a: &Listed, b: &Listed| {
            dated_first(a.date.as_deref(), b.date.as_deref(), newest)
                .then_with(|| self.collator.compare(&a.path, &b.path))
                .then(a.from.cmp(&b.from))
        };
        if query.limit < listed.len() {
            listed.select_nth_unstable_by(query.limit, order);
            listed.truncate(query.limit);
        }
        listed.sort_by(order);
        Ok((listed, total))
    }

    /// Project pages ordered by title, and the tasks that belong to at least one of them, in
    /// page path order, with the projects each belongs to. Only tasks on project pages, tasks
    /// linking a name that could resolve to one, and tasks tagged `project/...` are read.
    fn project_actions(&self) -> Result<ProjectActions> {
        let pages = self.project_pages()?;
        if pages.is_empty() {
            return Ok((pages, Vec::new()));
        }
        let projects: Vec<(String, Vec<String>)> = pages
            .iter()
            .map(|page| {
                let tags: Vec<String> = self.connection.select_bound(
                    "SELECT tag FROM live_front_tags WHERE revision=? ORDER BY ordinal",
                )?(page.revision.as_str())?;
                Ok((page.page.to_lossy_string(), tags))
            })
            .collect::<Result<_>>()?;
        let names = serde_json::to_string(
            &pages
                .iter()
                .map(|page| {
                    [
                        Some(page.revision.clone()),
                        Some(strip_markdown(&page.path).to_lowercase()),
                        Some(page.title.to_lowercase()),
                        self.date(&page.title),
                    ]
                })
                .collect::<Vec<_>>(),
        )?;
        let candidates: Vec<(String, i64)> = self.connection.select_bound(
            "SELECT revision,ordinal FROM live_tasks WHERE revision IN (SELECT value->>0 FROM json_each(?1))
             UNION SELECT revision,ordinal FROM live_tasks WHERE instr(lower(tags),'project/')>0
             UNION SELECT revision,ordinal FROM live_task_links WHERE page_key IN
               (SELECT value->>1 FROM json_each(?1) UNION SELECT value->>2 FROM json_each(?1))
             UNION SELECT revision,ordinal FROM live_task_links WHERE date IN (SELECT value->>3 FROM json_each(?1))",
        )?(names.as_str())?;
        self.count_rows(candidates.len());
        let mut task = self
            .connection
            .select_row_bound(&task_select(" AND t.revision=? AND t.ordinal=?"))?;
        let mut tasks = Vec::new();
        for (revision, ordinal) in &candidates {
            let row: Option<TaskRow> = task((revision.as_str(), *ordinal))?;
            if let Some(row) = row {
                tasks.push(task_item(row)?);
            }
        }
        self.sort_tasks(&mut tasks);
        let found: RefCell<HashMap<String, Option<String>>> = RefCell::default();
        let find = |name: &str| {
            found
                .borrow_mut()
                .entry(name.to_owned())
                .or_insert_with(|| {
                    self.resolve_page(name)
                        .ok()
                        .flatten()
                        .map(|page| page.page.to_lossy_string())
                })
                .clone()
        };
        let actions = tasks
            .into_iter()
            .filter_map(|task| {
                let belongs: Vec<usize> = memberships(
                    &task.source.page.to_lossy_string(),
                    &task.links,
                    &task.tags,
                    &projects,
                    &find,
                )
                .into_iter()
                .filter_map(|id| projects.iter().position(|(page, _)| page == id))
                .collect();
                (!belongs.is_empty()).then_some((task, belongs))
            })
            .collect();
        Ok((pages, actions))
    }
}

/// Project pages, and the tasks belonging to them with the projects each belongs to.
type ProjectActions = (Vec<PageRef>, Vec<(TaskItem, Vec<usize>)>);

/// A task of the chosen list as its filter and order read it.
struct Listed {
    revision: String,
    ordinal: i64,
    from: u32,
    path: String,
    date: Option<String>,
}

type ListedRow = (
    String,
    i64,
    u32,
    String,
    Option<String>,
    (String, String, String),
);
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

#[cfg(test)]
#[path = "tasks_tests.rs"]
mod tests;
