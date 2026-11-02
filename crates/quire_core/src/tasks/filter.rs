use super::{DateTerm, DateTest, Task, TaskState};
use crate::markdown::js_space;

/// The Tasks filter: words must appear, as one phrase, in the task or its page; `#tag` keeps
/// tasks carrying the tag or a tag nested under it, and a state tag (`#inbox`, `#next`,
/// `#waiting`, `#someday`, `#done`) keeps tasks in that state; `done:PERIOD`, `due:PERIOD`,
/// `due:overdue`, and `due:none` keep tasks by date. A leading `-` turns a word, tag, or date
/// term into one that hides the tasks it would keep.
#[derive(Clone, Debug, Default)]
pub struct TaskFilter {
    needle: String,
    exclude_words: Vec<String>,
    tags: Vec<String>,
    exclude_tags: Vec<String>,
    /// Each date test and whether it keeps (true) or hides the tasks it matches.
    dates: Vec<(DateTest, bool)>,
    error: Option<String>,
}

/// One task as the filter reads it.
#[derive(Clone, Copy, Debug)]
pub struct Candidate<'a> {
    /// The shown text, without marker, state tags, or stamps.
    pub text: &'a str,
    /// Lowercased, with `#`.
    pub tags: &'a [String],
    pub page: &'a str,
    pub state: TaskState,
    pub due: Option<&'a str>,
    pub completed: Option<&'a str>,
}

/// Whether `tag` is `term` or nested under it.
fn tag_within(tag: &str, term: &str) -> bool {
    let tag = tag.to_lowercase();
    tag == term
        || tag
            .strip_prefix(term)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// The filter's terms: its whitespace-separated words.
fn terms(query: &str) -> impl Iterator<Item = &str> {
    query.split(js_space).filter(|word| !word.is_empty())
}

/// The key a term shares with the terms it replaces when toggled: `due:` or `done:`.
fn date_key(term: &str) -> Option<&'static str> {
    let lower = term.to_lowercase();
    ["due:", "done:"]
        .into_iter()
        .find(|key| lower.starts_with(key))
}

impl TaskFilter {
    /// Reads `query` around `today`, the caller's ISO date. A date term that cannot be read
    /// filters nothing and sets [`TaskFilter::error`].
    pub fn new(query: &str, today: &str) -> Self {
        let mut include = Vec::new();
        let mut filter = Self::default();
        let query = query.to_lowercase();
        for word in terms(&query) {
            let (keep, body) = match word.strip_prefix('-').filter(|rest| !rest.is_empty()) {
                Some(rest) => (false, rest),
                None => (true, word),
            };
            if let Some(term) = DateTerm::parse(body) {
                match term.and_then(|term| term.resolve(today)) {
                    Ok(test) => filter.dates.push((test, keep)),
                    Err(error) => {
                        filter.error.get_or_insert(error);
                    }
                }
            } else if body.starts_with('#') && body.len() > 1 {
                if keep {
                    &mut filter.tags
                } else {
                    &mut filter.exclude_tags
                }
                .push(body.to_owned());
            } else if keep {
                include.push(word);
            } else {
                filter.exclude_words.push(body.to_owned());
            }
        }
        filter.needle = include.join(" ");
        filter
    }

    /// Why a term filters nothing, saying what to type instead.
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether every task matches, so a caller can skip reading what the filter would test.
    pub fn is_empty(&self) -> bool {
        !self.tests_text() && self.dates.is_empty()
    }

    /// Whether the filter reads task text, tags, or page titles, which dates alone do not.
    pub fn tests_text(&self) -> bool {
        !self.needle.is_empty()
            || !self.exclude_words.is_empty()
            || !self.tags.is_empty()
            || !self.exclude_tags.is_empty()
    }

    pub fn matches(&self, task: &Task, page: &str) -> bool {
        self.matches_parts(&Candidate {
            text: &task.text,
            tags: &task.tags,
            page,
            state: task.state,
            due: task.due.as_deref(),
            completed: task.completed.as_deref(),
        })
    }

    /// [`TaskFilter::matches`] over the parts of a task the filter reads.
    pub fn matches_parts(&self, task: &Candidate) -> bool {
        let dated = self
            .dates
            .iter()
            .all(|(test, keep)| test.matches(task.state, task.due, task.completed) == *keep);
        if !dated {
            return false;
        }
        let holds = |term: &String| match TaskState::parse(&term[1..]) {
            Some(state) => task.state == state,
            None => task.tags.iter().any(|tag| tag_within(tag, term)),
        };
        if !self.tags.iter().all(holds) || self.exclude_tags.iter().any(holds) {
            return false;
        }
        if self.needle.is_empty() && self.exclude_words.is_empty() {
            return true;
        }
        let haystack = format!("{} {}", task.text, task.page).to_lowercase();
        (self.needle.is_empty() || haystack.contains(&self.needle))
            && !self
                .exclude_words
                .iter()
                .any(|word| haystack.contains(word))
    }

    /// Whether `query` holds `term`, ignoring case.
    pub fn has_term(query: &str, term: &str) -> bool {
        terms(query).any(|word| word.eq_ignore_ascii_case(term))
    }

    /// `query` with `term` taken out when it holds it, else added at the end. A `due:` or
    /// `done:` term replaces any other term of its kind, negated ones included, since two
    /// periods of one kind would rarely both hold.
    pub fn toggle(query: &str, term: &str) -> String {
        let present = Self::has_term(query, term);
        let key = date_key(term);
        let mut kept: Vec<&str> = terms(query)
            .filter(|word| !word.eq_ignore_ascii_case(term))
            .filter(|word| {
                present || key.is_none() || date_key(word.strip_prefix('-').unwrap_or(word)) != key
            })
            .collect();
        if !present {
            kept.push(term);
        }
        kept.join(" ")
    }
}

/// The projects a task belongs to: the project page it sits on, project pages it links, and
/// the project page whose front matter carries a `project/<name>` tag the task has. `projects`
/// pairs each project page with its front matter tags; `find_page` resolves a link to a page.
pub fn project_membership<'p>(
    page: &str,
    task: &Task,
    projects: &'p [(String, Vec<String>)],
    find_page: impl FnMut(&str) -> Option<String>,
) -> Vec<&'p str> {
    memberships(page, &task.links, &task.tags, projects, find_page)
}

/// [`project_membership`] over a task's page links and lowercased tags.
pub fn memberships<'p>(
    page: &str,
    links: &[String],
    tags: &[String],
    projects: &'p [(String, Vec<String>)],
    mut find_page: impl FnMut(&str) -> Option<String>,
) -> Vec<&'p str> {
    let mut members: Vec<&str> = Vec::new();
    let mut add = |path: &'p str| {
        if !members.contains(&path) {
            members.push(path);
        }
    };
    let project = |path: &str| projects.iter().find(|(project, _)| project == path);
    if let Some((path, _)) = project(page) {
        add(path);
    }
    for link in links {
        if let Some((path, _)) = find_page(link).and_then(|path| project(&path)) {
            add(path);
        }
    }
    for tag in tags {
        let name = tag.strip_prefix('#').unwrap_or(tag).to_lowercase();
        let owner = projects.iter().rev().find(|(_, tags)| {
            tags.iter().any(|front| {
                front.to_lowercase() == name
                    && front
                        .get(..8)
                        .is_some_and(|head| head.eq_ignore_ascii_case("project/"))
                    && front.len() > 8
            })
        });
        if let Some((path, _)) = owner {
            add(path);
        }
    }
    members
}
