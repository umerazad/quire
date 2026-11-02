//! Live task lists written in a page as `{{tasks: QUERY}}` or `{{[[tasks]]: QUERY}}`. A query
//! holds states (`inbox`, `next`, `waiting`, `someday`, `done`, `open`, `due`, `upcoming`,
//! `all`; several match any of them, none means `open`, or `all` when a `done:` term asks for
//! completed tasks), `#tag`, `[[Page]]`, `here` (the page holding the query), `project:Name`,
//! `done:PERIOD`, `due:PERIOD`, `due:overdue`, `due:none`, words and `"quoted words"`, each but
//! the states negatable with `-`, and `by:page|project|state`, `sort:page|due`, and `limit:N`
//! (50 by default). Every term other than a state must hold; an empty query means `here`.
//! Ported from rt `5ea5c22310ff` `src/core/task-query.ts`; the date terms are Quire's own.

use crate::markdown::{JS_SPACE, js_space};
use crate::reference_filter::{UNCLOSED_QUOTE, query_tokens};
use crate::tasks::{DateTerm, DateTest, TaskState, add_days, strip_id};
use regex::Regex;
use std::{cmp::Ordering, collections::HashMap, ops::Range, sync::LazyLock};

/// Which tasks a state term selects.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum View {
    State(TaskState),
    Open,
    Due,
    Upcoming,
    All,
}

impl View {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "open" => Self::Open,
            "due" => Self::Due,
            "upcoming" => Self::Upcoming,
            "all" => Self::All,
            _ => Self::State(TaskState::parse(name)?),
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::State(state) => state.as_str(),
            Self::Open => "open",
            Self::Due => "due",
            Self::Upcoming => "upcoming",
            Self::All => "all",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    Page,
    Project,
    State,
}

impl Grouping {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Project => "project",
            Self::State => "state",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Order {
    #[default]
    Page,
    Due,
}

impl Order {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Due => "due",
        }
    }
}

/// The page term naming the page that holds the query.
pub const HERE: &str = "here";
pub const DEFAULT_LIMIT: usize = 50;
/// How far ahead `upcoming` looks, in days.
pub const UPCOMING_DAYS: i64 = 7;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskQuery {
    pub source: String,
    pub views: Vec<View>,
    pub tags: Vec<String>,
    pub not_tags: Vec<String>,
    pub pages: Vec<String>,
    pub not_pages: Vec<String>,
    pub projects: Vec<String>,
    pub not_projects: Vec<String>,
    pub words: Vec<String>,
    pub not_words: Vec<String>,
    pub dates: Vec<DateTerm>,
    pub not_dates: Vec<DateTerm>,
    pub by: Option<Grouping>,
    pub sort: Order,
    pub limit: usize,
}

static LINE: LazyLock<Regex> = LazyLock::new(|| {
    let space = &JS_SPACE[1..JS_SPACE.len() - 1];
    Regex::new(&format!(
        r"^[{space}]*(\{{\{{(?:tasks|\[\[tasks\]\])(?::([^}}\n]*))?\}}\}})[{space}]*$"
    ))
    .expect("the task query line pattern is valid")
});

static UNKNOWN_TERM: LazyLock<Regex> = LazyLock::new(|| {
    let space = &JS_SPACE[1..JS_SPACE.len() - 1];
    Regex::new(&format!(r"^[a-zA-Z]+:[^{space}]")).expect("the unknown term pattern is valid")
});

/// The terms an unknown term's error offers instead.
pub const TERMS: &str =
    "a state, #tag, [[Page]], project:Name, due:, done:, by:, sort:, limit: or words";

/// A block whose first line, apart from a trailing `^id`, is one task query.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueryLine<'a> {
    /// The text after `tasks:`, untrimmed; empty for `{{tasks}}`.
    pub query: &'a str,
    /// Where `{{...}}` sits in the line.
    pub range: Range<usize>,
}

/// The reference `taskQueryLine` test on a block's content up to the end of its first line.
pub fn query_line(line: &str) -> Option<QueryLine<'_>> {
    let captures = LINE.captures(strip_id(line))?;
    let whole = captures.get(1)?;
    Some(QueryLine {
        query: captures.get(2).map_or("", |query| query.as_str()),
        range: whole.range(),
    })
}

fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// JavaScript `Number(text)` for text without whitespace; NaN when it does not read as one.
fn js_number(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }
    let radix = match text.get(..2) {
        Some("0x") => Some(16),
        Some("0o") => Some(8),
        Some("0b") => Some(2),
        _ => None,
    };
    if let Some(radix) = radix {
        let digits = &text[2..];
        if digits.is_empty() || !digits.chars().all(|digit| digit.is_digit(radix)) {
            return f64::NAN;
        }
        return digits.chars().fold(0.0, |value, digit| {
            value * f64::from(radix) + f64::from(digit.to_digit(radix).unwrap_or(0))
        });
    }
    let unsigned = text.strip_prefix(['+', '-']).unwrap_or(text);
    let (mantissa, exponent) = match unsigned.split_once('e') {
        Some((mantissa, exponent)) => (mantissa, Some(exponent)),
        None => (unsigned, None),
    };
    let (whole, fraction) = mantissa.split_once('.').unwrap_or((mantissa, ""));
    let digits = |part: &str| part.bytes().all(|byte| byte.is_ascii_digit());
    let exponent_ok = exponent.is_none_or(|exponent| {
        let exponent = exponent.strip_prefix(['+', '-']).unwrap_or(exponent);
        !exponent.is_empty() && digits(exponent)
    });
    if (whole.is_empty() && fraction.is_empty())
        || !digits(whole)
        || !digits(fraction)
        || !exponent_ok
    {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

impl TaskQuery {
    /// The reference `parseTaskQuery`.
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut query = Self {
            source: source.trim_matches(js_space).to_owned(),
            views: Vec::new(),
            tags: Vec::new(),
            not_tags: Vec::new(),
            pages: Vec::new(),
            not_pages: Vec::new(),
            projects: Vec::new(),
            not_projects: Vec::new(),
            words: Vec::new(),
            not_words: Vec::new(),
            dates: Vec::new(),
            not_dates: Vec::new(),
            by: None,
            sort: Order::Page,
            limit: DEFAULT_LIMIT,
        };
        let mut tokens = query_tokens(source).peekable();
        if tokens.peek().is_none() {
            query.pages.push(HERE.into());
        }
        for token in tokens {
            let negated = token.starts_with('-') && utf16_len(token) > 1;
            let body = if negated { &token[1..] } else { token };
            let lower = body.to_lowercase();
            let pick = |yes: &mut Vec<String>, no: &mut Vec<String>, value: String| {
                if negated { no } else { yes }.push(value)
            };
            if body.starts_with('"') {
                if utf16_len(body) < 2 || !body.ends_with('"') {
                    return Err(UNCLOSED_QUOTE.into());
                }
                let quoted = body[1..body.len() - 1].to_lowercase();
                pick(&mut query.words, &mut query.not_words, quoted);
            } else if body.starts_with("[[") && body.ends_with("]]") {
                let name = body[2..body.len() - 2].trim_matches(js_space).to_owned();
                pick(&mut query.pages, &mut query.not_pages, name);
            } else if body.starts_with('#') && utf16_len(body) > 1 {
                pick(&mut query.tags, &mut query.not_tags, lower);
            } else if lower == HERE {
                pick(&mut query.pages, &mut query.not_pages, lower);
            } else if let Some(view) = View::parse(&lower).filter(|_| !negated) {
                query.views.push(view);
            } else if let Some(term) = DateTerm::parse(body) {
                let term = term?;
                if negated {
                    query.not_dates.push(term);
                } else {
                    query.dates.push(term);
                }
            } else if lower.starts_with("project:") && utf16_len(body) > 8 {
                let name = lower["project:".len()..].to_owned();
                pick(&mut query.projects, &mut query.not_projects, name);
            } else if let Some(by) = lower.strip_prefix("by:").filter(|_| !negated) {
                query.by = Some(match by {
                    "page" => Grouping::Page,
                    "project" => Grouping::Project,
                    "state" => Grouping::State,
                    _ => {
                        return Err(format!(
                            "Unknown grouping \"{by}\". Use by:page, by:project or by:state."
                        ));
                    }
                });
            } else if let Some(sort) = lower.strip_prefix("sort:").filter(|_| !negated) {
                query.sort = match sort {
                    "page" => Order::Page,
                    "due" => Order::Due,
                    _ => {
                        return Err(format!(
                            "Unknown order \"{sort}\". Use sort:page or sort:due."
                        ));
                    }
                };
            } else if let Some(limit) = lower.strip_prefix("limit:").filter(|_| !negated) {
                let value = js_number(limit);
                if !value.is_finite() || value.fract() != 0.0 || value < 1.0 {
                    return Err(format!(
                        "The limit \"{limit}\" is not a whole number above 0."
                    ));
                }
                query.limit = value as usize;
            } else if UNKNOWN_TERM.is_match(body) {
                return Err(format!("Unknown term \"{body}\". Use {TERMS}."));
            } else {
                pick(&mut query.words, &mut query.not_words, lower);
            }
        }
        Ok(query)
    }

    /// Whether any term needs project membership.
    pub fn uses_projects(&self) -> bool {
        !self.projects.is_empty()
            || !self.not_projects.is_empty()
            || self.by == Some(Grouping::Project)
    }

    /// Every page name the query mentions other than `here`.
    pub fn page_names(&self) -> impl Iterator<Item = &str> {
        self.pages
            .iter()
            .chain(&self.not_pages)
            .map(String::as_str)
            .filter(|name| *name != HERE)
    }
}

/// One task as a query reads it.
#[derive(Clone, Copy, Debug)]
pub struct Listed<'a> {
    /// The identity of the page the task sits on.
    pub page: &'a str,
    pub title: &'a str,
    pub text: &'a str,
    /// Lowercase, with `#`.
    pub tags: &'a [String],
    /// Page links of the task and its parent blocks.
    pub links: &'a [String],
    pub state: TaskState,
    pub due: Option<&'a str>,
    pub completed: Option<&'a str>,
}

/// A project page by identity and title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    pub page: String,
    pub title: String,
}

/// What a query reads besides the tasks.
pub struct Notebook<'a> {
    /// The page holding the query.
    pub here: &'a str,
    pub today: &'a str,
    pub projects: &'a [Project],
    /// For each task, the indices of the projects it belongs to.
    pub members: &'a [Vec<usize>],
    /// Resolves a page name to a page identity, as links resolve.
    pub find_page: &'a dyn Fn(&str) -> Option<String>,
    /// Orders page and project titles, as the reference `localeCompare` does.
    pub compare: &'a dyn Fn(&str, &str) -> Ordering,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskGroup {
    /// Empty without grouping.
    pub label: String,
    /// The page a page or project group names.
    pub page: Option<String>,
    /// Indices into the listed tasks.
    pub tasks: Vec<usize>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TaskListing {
    pub groups: Vec<TaskGroup>,
    /// How many tasks match, beyond the limit too.
    pub total: usize,
}

impl TaskListing {
    pub fn shown(&self) -> usize {
        self.groups.iter().map(|group| group.tasks.len()).sum()
    }
}

fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Inbox => "Inbox",
        TaskState::Next => "Next",
        TaskState::Waiting => "Waiting",
        TaskState::Someday => "Someday",
        TaskState::Done => "Done",
    }
}

fn shows_in(task: &Listed, view: View, today: &str, until: Option<&str>) -> bool {
    let open = task.state != TaskState::Done;
    match view {
        View::All => true,
        View::Open => open,
        View::Due => open && task.due.is_some_and(|due| due <= today),
        View::Upcoming => {
            open && task
                .due
                .is_some_and(|due| due > today && until.is_some_and(|until| due <= until))
        }
        View::State(state) => task.state == state,
    }
}

/// The reference `runTaskQuery`: the matching tasks, ordered by page title then position (or
/// earliest due date first with `sort:due`), the first `limit` of them, grouped when asked.
pub fn run(query: &TaskQuery, tasks: &[Listed], notebook: &Notebook) -> TaskListing {
    let resolved: HashMap<&str, Option<String>> = query
        .pages
        .iter()
        .chain(&query.not_pages)
        .map(|name| {
            let page = match name.as_str() {
                HERE => Some(notebook.here.to_owned()),
                name => (notebook.find_page)(name),
            };
            (name.as_str(), page)
        })
        .collect();
    let mut links: HashMap<&str, Option<String>> = HashMap::new();
    for task in tasks {
        for link in task.links {
            links
                .entry(link.as_str())
                .or_insert_with(|| (notebook.find_page)(link));
        }
    }
    let touches = |task: &Listed, name: &String| {
        resolved
            .get(name.as_str())
            .cloned()
            .flatten()
            .is_some_and(|page| {
                task.page == page
                    || task.links.iter().any(|link| {
                        links.get(link.as_str()).cloned().flatten().as_deref() == Some(&page)
                    })
            })
    };
    let has_tag = |task: &Listed, tag: &String| {
        task.tags.iter().any(|own| {
            own == tag
                || own
                    .strip_prefix(tag.as_str())
                    .is_some_and(|rest| rest.starts_with('/'))
        })
    };
    let named = |name: &str| -> Vec<usize> {
        notebook
            .projects
            .iter()
            .enumerate()
            .filter(|(_, project)| {
                let title = project.title.to_lowercase();
                title == name || title.strip_prefix("projects/") == Some(name)
            })
            .map(|(index, _)| index)
            .collect()
    };
    let project_terms: HashMap<&str, Vec<usize>> = query
        .projects
        .iter()
        .chain(&query.not_projects)
        .map(|name| (name.as_str(), named(name)))
        .collect();
    let in_project = |index: usize, name: &String| {
        project_terms[name.as_str()].iter().any(|project| {
            notebook
                .members
                .get(index)
                .is_some_and(|of| of.contains(project))
        })
    };
    let asks_done = query
        .dates
        .iter()
        .any(|term| matches!(term, DateTerm::Done(_)));
    let views: &[View] = match (query.views.is_empty(), asks_done) {
        (false, _) => &query.views,
        (true, true) => &[View::All],
        (true, false) => &[View::Open],
    };
    let place = |terms: &[DateTerm]| -> Vec<Option<DateTest>> {
        terms
            .iter()
            .map(|term| term.resolve(notebook.today).ok())
            .collect()
    };
    let (dates, not_dates) = (place(&query.dates), place(&query.not_dates));
    let dated = |task: &Listed, test: &Option<DateTest>| {
        test.as_ref()
            .is_some_and(|test| test.matches(task.state, task.due, task.completed))
    };
    let until = add_days(notebook.today, UPCOMING_DAYS);
    let mut matched: Vec<usize> = tasks
        .iter()
        .enumerate()
        .filter(|(index, task)| {
            let haystack = format!("{} {}", task.text, task.title).to_lowercase();
            views
                .iter()
                .any(|view| shows_in(task, *view, notebook.today, until.as_deref()))
                && query.tags.iter().all(|tag| has_tag(task, tag))
                && !query.not_tags.iter().any(|tag| has_tag(task, tag))
                && query.pages.iter().all(|page| touches(task, page))
                && !query.not_pages.iter().any(|page| touches(task, page))
                && query.projects.iter().all(|name| in_project(*index, name))
                && !query
                    .not_projects
                    .iter()
                    .any(|name| in_project(*index, name))
                && dates.iter().all(|test| dated(task, test))
                && !not_dates.iter().any(|test| dated(task, test))
                && query
                    .words
                    .iter()
                    .all(|word| haystack.contains(word.as_str()))
                && !query
                    .not_words
                    .iter()
                    .any(|word| haystack.contains(word.as_str()))
        })
        .map(|(index, _)| index)
        .collect();
    let by_page =
        |a: &usize, b: &usize| (notebook.compare)(tasks[*a].title, tasks[*b].title).then(a.cmp(b));
    match query.sort {
        Order::Page => matched.sort_by(by_page),
        Order::Due => matched.sort_by(|a, b| {
            let (left, right) = (tasks[*a].due, tasks[*b].due);
            left.is_none()
                .cmp(&right.is_none())
                .then(left.unwrap_or("").cmp(right.unwrap_or("")))
                .then_with(|| by_page(a, b))
        }),
    }
    let total = matched.len();
    matched.truncate(query.limit);
    let Some(by) = query.by else {
        return TaskListing {
            groups: vec![TaskGroup {
                label: String::new(),
                page: None,
                tasks: matched,
            }],
            total,
        };
    };
    const NONE: &str = "\0none";
    let mut groups: Vec<(String, TaskGroup)> = Vec::new();
    let mut add = |key: &str, label: &str, page: Option<&str>, task: usize| match groups
        .iter_mut()
        .find(|(own, _)| own == key)
    {
        Some((_, group)) => group.tasks.push(task),
        None => groups.push((
            key.to_owned(),
            TaskGroup {
                label: label.to_owned(),
                page: page.map(str::to_owned),
                tasks: vec![task],
            },
        )),
    };
    for index in matched {
        let task = &tasks[index];
        match by {
            Grouping::Page => add(task.page, task.title, Some(task.page), index),
            Grouping::State => add(task.state.as_str(), state_label(task.state), None, index),
            Grouping::Project => {
                let of = notebook.members.get(index).map_or(&[][..], Vec::as_slice);
                if of.is_empty() {
                    add(NONE, "No project", None, index);
                }
                for project in of {
                    let project = &notebook.projects[*project];
                    add(&project.page, &project.title, Some(&project.page), index);
                }
            }
        }
    }
    match by {
        Grouping::State => groups.sort_by_key(|(key, _)| {
            TaskState::ALL
                .iter()
                .position(|state| state.as_str() == key)
        }),
        Grouping::Project => groups.sort_by(|(a, left), (b, right)| match (a == NONE, b == NONE) {
            (true, true) => Ordering::Equal,
            (true, false) => Ordering::Greater,
            (false, true) => Ordering::Less,
            (false, false) => (notebook.compare)(&left.label, &right.label),
        }),
        Grouping::Page => {}
    }
    TaskListing {
        groups: groups.into_iter().map(|(_, group)| group).collect(),
        total,
    }
}
