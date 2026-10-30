//! Task semantics: checkbox and Roam markers, states, created/due/completed stamps, inherited
//! dates and links, state changes, and the cursor and selection commands that make or change
//! tasks. Ported from rt `5ea5c22310ff` `src/core/tasks.ts` and the task methods of
//! `src/core/store.ts`.
//!
//! Callers supply the page's blocks, tags, references, and syntax ranges from native Markdown
//! analysis, so this module never parses Markdown. Offsets are bytes in the editor's
//! line-break-normalized text. Dates are ISO strings; "today" comes from the caller's clock and
//! time zone through [`crate::daily::DailyCalendar::date_at`], never from the process clock.

mod commands;
mod dates;
mod filter;

pub use commands::{
    CAPTURE_ONE, NOT_HERE, NOT_IN_CODE, NOT_IN_SELECTION, Shortcut, TaskEdit, capture,
    created_stamp, task_at, tasks_in,
};
pub use dates::{add_days, date_phrase, is_due, is_upcoming, short_date, task_date_label};
pub use filter::{TaskFilter, project_membership};

use crate::daily::{DailyCalendar, DailyLayout, DailyParser, format_daily};
use crate::markdown::{Reference, ReferenceKind, TagToken, js_space};
use crate::source_text::SourceEdit;
use regex::Regex;
use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    sync::LazyLock,
};

pub type Edit = SourceEdit<String>;

pub const DUPLICATE_ID: &str = "Several tasks share this block ID. Give them distinct IDs in Source before changing their state.";
pub const CHANGED: &str = "This task changed. Review its current text and try again.";
pub const OPEN_COMPLETION: &str =
    "Only a done task has a completion date. Complete the task first.";
pub const UNTITLED: &str = "Untitled action";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TaskState {
    Inbox,
    Next,
    Waiting,
    Someday,
    Done,
}

impl TaskState {
    pub const ALL: [Self; 5] = [
        Self::Inbox,
        Self::Next,
        Self::Waiting,
        Self::Someday,
        Self::Done,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Inbox => "inbox",
            Self::Next => "next",
            Self::Waiting => "waiting",
            Self::Someday => "someday",
            Self::Done => "done",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|state| state.as_str() == name)
    }
}

/// What a state change does: toggle done, or set a state, which reopens a done task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Toggle,
    Set(TaskState),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DateField {
    Created,
    Due,
    Completed,
}

/// The longest marker, `{{[[TODO]]}}`, in characters.
pub const MARKER_CHAR_LIMIT: usize = 13;

/// A task marker at the start of a block's text: `[ ]`, `[x]`, `[X]`, or a Roam
/// `{{TODO}}`/`{{DONE}}`, optionally with either half of `[[...]]`, followed by blanks or the
/// end of the text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marker<'a> {
    pub source: &'a str,
    /// The marker and the blanks after it.
    pub prefix_len: usize,
    pub checked: bool,
}

impl Marker<'_> {
    pub fn toggled(&self) -> String {
        marker_for(self.source, !self.checked)
    }
}

/// The marker spelled as `source` is spelled, open or done.
fn marker_for(source: &str, done: bool) -> String {
    if source.starts_with('[') {
        if done { "[x]" } else { "[ ]" }.into()
    } else {
        let at = [source.find("TODO"), source.find("DONE")]
            .into_iter()
            .flatten()
            .min()
            .unwrap_or(0);
        let mut next = source.to_owned();
        next.replace_range(at..at + 4, if done { "DONE" } else { "TODO" });
        next
    }
}

pub fn read_marker(text: &str) -> Option<Marker<'_>> {
    let source = if text.starts_with('[') {
        text.get(..3)
            .filter(|box_| matches!(*box_, "[ ]" | "[x]" | "[X]"))?
    } else {
        let rest = text.strip_prefix("{{")?;
        let open = if rest.starts_with("[[") { 2 } else { 0 };
        let word = rest
            .get(open..open + 4)
            .filter(|w| matches!(*w, "TODO" | "DONE"))?;
        let after = &rest[open + word.len()..];
        let close = if after.starts_with("]]") { 2 } else { 0 };
        after[close..].strip_prefix("}}")?;
        &text[..2 + open + 4 + close + 2]
    };
    let rest = &text[source.len()..];
    let blanks = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    (blanks > 0 || rest.is_empty()).then(|| Marker {
        source,
        prefix_len: source.len() + blanks,
        checked: matches!(source, "[x]" | "[X]") || source.contains("DONE"),
    })
}

/// Maps calendar dates to daily-page titles and back with the notebook's daily layout.
pub struct DailyNames {
    layout: DailyLayout,
    calendar: DailyCalendar,
    parser: DailyParser,
}

impl DailyNames {
    pub fn new(layout: DailyLayout, calendar: DailyCalendar) -> Self {
        Self {
            parser: DailyParser::new(&layout, calendar.clone()),
            layout,
            calendar,
        }
    }

    /// The ISO date a page title or link target names, if it names one.
    pub fn date(&self, title: &str) -> Option<String> {
        self.parser.date(title)
    }

    /// The daily page title for an ISO date, which stamps link so other tools resolve them.
    pub fn title(&self, iso: &str) -> String {
        format_daily(&self.layout.title, iso, &self.calendar).unwrap_or_else(|_| iso.to_owned())
    }

    /// Whether `date` is a real calendar date written as YYYY-MM-DD.
    pub fn is_calendar_date(&self, date: &str) -> bool {
        self.date(date).as_deref() == Some(date)
    }
}

/// One block of the page, as native Markdown analysis reports it.
#[derive(Clone, Copy, Debug)]
pub struct PageBlock<'a> {
    pub key: &'a str,
    pub id: Option<&'a str>,
    /// `list`, `paragraph`, `heading`, `code`, or `quote`.
    pub kind: &'a str,
    pub from: usize,
    pub content: (usize, usize),
    pub parent: Option<&'a str>,
}

impl PageBlock<'_> {
    pub fn is_list(&self) -> bool {
        self.kind == "list"
    }

    /// The block's text: its content without a trailing block ID.
    pub fn text<'t>(&self, page: &'t str) -> &'t str {
        strip_id(&page[self.content.0..self.content.1])
    }

    fn content_range(&self) -> Range<usize> {
        self.content.0..self.content.1
    }
}

static ID_TAIL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[ \t])\^[a-zA-Z0-9_-]+[ \t]*\z").unwrap());

/// Content without a trailing `^id`, trimmed at the end.
pub fn strip_id(content: &str) -> &str {
    let end = ID_TAIL
        .find(content)
        .map_or(content.len(), |found| found.start());
    content[..end].trim_end_matches(js_space)
}

/// One page's analysis, the input to every task function.
#[derive(Clone, Copy)]
pub struct Page<'a> {
    pub text: &'a str,
    pub title: &'a str,
    pub blocks: &'a [PageBlock<'a>],
    pub tags: &'a [TagToken],
    pub references: &'a [Reference],
    /// Code, link, image, autolink, HTML, escape, and link-reference ranges, as tags exclude.
    pub syntax_ranges: &'a [Range<usize>],
    /// Fenced and indented code blocks, where no task can be made.
    pub code_ranges: &'a [Range<usize>],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Task {
    pub block_key: String,
    pub block_from: usize,
    pub id: Option<String>,
    /// The block's content range; the raw text starts at `content.start`.
    pub content: Range<usize>,
    /// The block's text, marker included and block ID excluded.
    pub raw: String,
    /// The text to show: no marker, state tags, or stamps.
    pub text: String,
    pub marker: String,
    pub checked: bool,
    pub state: TaskState,
    /// Lowercased tags, state tags included.
    pub tags: Vec<String>,
    /// Page links of the block and its ancestors.
    pub links: Vec<String>,
    pub created: Option<String>,
    pub due: Option<String>,
    pub completed: Option<String>,
    /// The block's tags with ranges relative to `content.start`, Roam `#[[state]]` tags included.
    pub tag_tokens: Vec<TagToken>,
}

fn stamp_pattern(emoji: &str, linked: bool) -> Regex {
    let date = "([0-9]{4}-[0-9]{2}-[0-9]{2})";
    Regex::new(&if linked {
        format!(r"[ \t]*{emoji}[ \t]*(?:\[\[([^\]\n]+)\]\]|{date})")
    } else {
        format!(r"[ \t]*{emoji}[ \t]*{date}")
    })
    .unwrap()
}

static COMPLETED: LazyLock<Regex> = LazyLock::new(|| stamp_pattern("✅", false));
static CREATED: LazyLock<Regex> = LazyLock::new(|| stamp_pattern("➕", true));
static DUE: LazyLock<Regex> = LazyLock::new(|| stamp_pattern("📅", true));

fn stamp(field: DateField) -> &'static Regex {
    match field {
        DateField::Created => &CREATED,
        DateField::Due => &DUE,
        DateField::Completed => &COMPLETED,
    }
}

/// The text a date stamp holds: a link target or an ISO date.
fn stamp_target(field: DateField, text: &str) -> Option<&str> {
    let captures = stamp(field).captures(text)?;
    captures.get(1).or(captures.get(2)).map(|m| m.as_str())
}

/// The stamp for a date: created and due stamps link the daily page.
pub fn stamp_text(field: DateField, date: &str, days: &DailyNames) -> String {
    match field {
        DateField::Created => format!(" ➕ [[{}]]", days.title(date)),
        DateField::Due => format!(" 📅 [[{}]]", days.title(date)),
        DateField::Completed => format!(" ✅ {date}"),
    }
}

/// A tag that sets a task's state. `#pending`, common in Obsidian vaults, reads as Waiting.
fn is_state_tag(tag: &str) -> bool {
    tag.strip_prefix('#').is_some_and(|name| {
        ["inbox", "next", "waiting", "someday", "done", "pending"]
            .iter()
            .any(|state| state.eq_ignore_ascii_case(name))
    })
}

fn push_unique<'s>(items: &mut Vec<&'s str>, item: &'s str) {
    if !items.contains(&item) {
        items.push(item);
    }
}

/// Items whose range lies inside `range`, from a list sorted by start.
fn within<T>(items: &[T], range: Range<usize>, span: impl Fn(&T) -> &Range<usize>) -> Vec<&T> {
    let first = items.partition_point(|item| span(item).start < range.start);
    items[first..]
        .iter()
        .take_while(|item| span(item).start < range.end)
        .filter(|item| span(item).end <= range.end)
        .collect()
}

impl<'a> Page<'a> {
    fn own_links(&self, block: &PageBlock) -> Vec<&'a str> {
        within(self.references, block.content_range(), |r| &r.range)
            .into_iter()
            .filter(|reference| reference.kind == ReferenceKind::Page)
            .map(|reference| reference.target.as_str())
            .collect()
    }

    /// A block's page links, without the link that records when it was created.
    fn date_links(&self, block: &PageBlock) -> Vec<&'a str> {
        let created = CREATED
            .captures(block.text(self.text))
            .and_then(|captures| captures.get(1))
            .map(|m| m.as_str());
        self.own_links(block)
            .into_iter()
            .filter(|link| Some(*link) != created)
            .collect()
    }

    /// Tags plus Roam `#[[state]]` tags outside code and links, sorted by start.
    fn task_tags(&self) -> Vec<TagToken> {
        let wiki: Vec<_> = self
            .references
            .iter()
            .filter(|reference| {
                let start = reference.range.start;
                reference.kind == ReferenceKind::Page
                    && start > 0
                    && self.text.as_bytes()[start - 1] == b'#'
                    && self.text[..start - 1].chars().next_back().is_none_or(|c| {
                        js_space(c) || matches!(c, '(' | '[' | '{' | '"' | '\'' | '*' | '~')
                    })
                    && TaskState::ALL
                        .iter()
                        .any(|state| reference.target.to_lowercase() == state.as_str())
            })
            .collect();
        if wiki.is_empty() {
            return self.tags.to_vec();
        }
        let frontmatter = crate::markdown::frontmatter_length(self.text);
        let ignored: Vec<Range<usize>> = self
            .references
            .iter()
            .map(|reference| reference.range.clone())
            .chain((frontmatter > 0).then_some(0..frontmatter))
            .chain(self.syntax_ranges.iter().cloned())
            .collect();
        let mut tags = self.tags.to_vec();
        tags.extend(
            wiki.into_iter()
                .filter(|reference| {
                    let range = &reference.range;
                    !ignored.iter().any(|other| {
                        !(other.start >= range.start && other.end <= range.end)
                            && range.start - 1 < other.end
                            && range.end > other.start
                    })
                })
                .map(|reference| TagToken {
                    range: reference.range.start - 1..reference.range.end,
                    text: format!("#{}", reference.target),
                }),
        );
        tags.sort_by_key(|tag| tag.range.start);
        tags
    }

    /// The block's text with continuation lines unindented to its first line, as shown.
    fn shown_text(&self, block: &PageBlock, text: &str) -> String {
        if !block.is_list() {
            return text.to_owned();
        }
        let line_from = self.text[..block.from].rfind('\n').map_or(0, |at| at + 1);
        let indent = block.content.0 - line_from;
        text.split('\n')
            .enumerate()
            .map(|(index, line)| {
                if index == 0 {
                    line
                } else {
                    let blanks = line.len() - line.trim_start_matches([' ', '\t']).len();
                    &line[indent.min(blanks)..]
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Every task on the page, in block order.
    pub fn tasks(&self, days: &DailyNames) -> Vec<Task> {
        let page_date = (!self.title.is_empty())
            .then(|| days.date(self.title))
            .flatten();
        self.undated_tasks()
            .into_iter()
            .map(|(task, sources)| sources.resolve(task, days, page_date.as_deref()))
            .collect()
    }

    /// Every task with its created and due dates left out, and the stamps and links they come
    /// from. Dates depend on the daily layout; these do not, so an index can keep them across
    /// layout changes.
    pub fn undated_tasks(&self) -> Vec<(Task, DateSources)> {
        let tokens = self.task_tags();
        let by_key: HashMap<&str, &PageBlock> =
            self.blocks.iter().map(|block| (block.key, block)).collect();
        self.blocks
            .iter()
            .filter(|block| block.is_list())
            .filter_map(|block| {
                let raw = block.text(self.text);
                read_marker(raw)?;
                let own = self.shown_text(block, raw);
                let marker = read_marker(&own)?;
                let own_tags: Vec<TagToken> = within(&tokens, block.content_range(), |t| &t.range)
                    .into_iter()
                    .map(|tag| TagToken {
                        range: tag.range.start - block.content.0..tag.range.end - block.content.0,
                        text: tag.text.clone(),
                    })
                    .collect();
                let names: Vec<String> =
                    own_tags.iter().map(|tag| tag.text.to_lowercase()).collect();
                let has = |name: &str| names.iter().any(|tag| tag == name);
                let state = if marker.checked || has("#done") {
                    TaskState::Done
                } else if has("#next") {
                    TaskState::Next
                } else if has("#waiting") || has("#pending") {
                    TaskState::Waiting
                } else if has("#someday") {
                    TaskState::Someday
                } else {
                    TaskState::Inbox
                };
                let mut links: Vec<&str> = Vec::new();
                self.own_links(block)
                    .into_iter()
                    .for_each(|link| push_unique(&mut links, link));
                let mut inherited: Vec<&str> = Vec::new();
                let mut visited = HashSet::from([block.key]);
                let mut parent = block.parent.and_then(|key| by_key.get(key));
                while let Some(ancestor) = parent.filter(|a| visited.insert(a.key)) {
                    self.own_links(ancestor)
                        .into_iter()
                        .for_each(|link| push_unique(&mut links, link));
                    self.date_links(ancestor)
                        .into_iter()
                        .for_each(|link| push_unique(&mut inherited, link));
                    parent = ancestor.parent.and_then(|key| by_key.get(key));
                }
                let mut display = raw.to_owned();
                for tag in own_tags.iter().rev().filter(|tag| is_state_tag(&tag.text)) {
                    display.replace_range(tag.range.clone(), "");
                }
                let shown = self.shown_text(block, &display);
                let mut body = shown
                    .get(marker.prefix_len..)
                    .unwrap_or_default()
                    .to_owned();
                for field in [DateField::Completed, DateField::Created, DateField::Due] {
                    if let Some(found) = stamp(field).find(&body) {
                        body.replace_range(found.range(), "");
                    }
                }
                let body = body.trim_matches(js_space);
                let sources = DateSources {
                    created: stamp_target(DateField::Created, raw).map(str::to_owned),
                    due: stamp_target(DateField::Due, raw).map(str::to_owned),
                    links: owned(self.date_links(block)),
                    inherited: owned(inherited),
                };
                let task = Task {
                    block_key: block.key.to_owned(),
                    block_from: block.from,
                    id: block.id.map(str::to_owned),
                    content: block.content_range(),
                    raw: raw.to_owned(),
                    text: if body.is_empty() { UNTITLED } else { body }.to_owned(),
                    marker: marker.source.to_owned(),
                    checked: marker.checked,
                    state,
                    tags: names,
                    links: owned(links),
                    created: None,
                    due: None,
                    completed: COMPLETED
                        .captures(raw)
                        .map(|captures| captures[1].to_owned()),
                    tag_tokens: own_tags,
                };
                Some((task, sources))
            })
            .collect()
    }
}

fn owned(items: Vec<&str>) -> Vec<String> {
    items.into_iter().map(str::to_owned).collect()
}

/// Where a task's created and due dates come from: its stamps' targets, its own page links
/// other than the created stamp, and its ancestors' links. The daily layout turns them into
/// dates; a task on a daily page falls back to that page's date.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DateSources {
    pub created: Option<String>,
    pub due: Option<String>,
    pub links: Vec<String>,
    pub inherited: Vec<String>,
}

impl DateSources {
    /// The created date: the created stamp's date.
    pub fn created(&self, days: &DailyNames) -> Option<String> {
        self.created.as_deref().and_then(|target| days.date(target))
    }

    /// The due date: the due stamp's date, else the earliest dated link of the task, else the
    /// earliest dated link of its ancestors.
    pub fn due(&self, days: &DailyNames) -> Option<String> {
        let earliest = |targets: &[String]| -> Option<String> {
            targets.iter().filter_map(|target| days.date(target)).min()
        };
        self.due
            .as_deref()
            .and_then(|target| days.date(target))
            .or_else(|| earliest(&self.links))
            .or_else(|| earliest(&self.inherited))
    }

    /// The task with its dates filled in, falling back to the page's daily date.
    pub fn resolve(&self, mut task: Task, days: &DailyNames, page_date: Option<&str>) -> Task {
        task.created = self.created(days).or_else(|| page_date.map(str::to_owned));
        task.due = self.due(days).or_else(|| page_date.map(str::to_owned));
        task
    }
}

impl Task {
    fn at(&self, range: Range<usize>, replacement: impl Into<String>) -> Edit {
        Edit {
            range: self.content.start + range.start..self.content.start + range.end,
            replacement: replacement.into(),
        }
    }

    /// The edits that apply a state change, stamping the completion date with `today`.
    /// Completing keeps state tags; reopening by toggle removes `#done`; setting a state
    /// replaces every state tag.
    pub fn changes(&self, action: Action, today: &str) -> Vec<Edit> {
        let done = match action {
            Action::Toggle => self.state != TaskState::Done,
            Action::Set(state) => state == TaskState::Done,
        };
        let mut changes = Vec::new();
        let next_marker = marker_for(&self.marker, done);
        if next_marker != self.marker {
            changes.push(self.at(0..self.marker.len(), next_marker));
        }
        let removed = self.tag_tokens.iter().filter(|tag| match action {
            Action::Toggle => !done && tag.text.to_lowercase() == "#done",
            Action::Set(_) => !done && is_state_tag(&tag.text),
        });
        for tag in removed {
            let blank = tag.range.start > 0
                && matches!(self.raw.as_bytes()[tag.range.start - 1], b' ' | b'\t');
            changes.push(self.at(tag.range.start - usize::from(blank)..tag.range.end, ""));
        }
        let end = self.raw.len();
        let completion = COMPLETED.find(&self.raw);
        if let Action::Set(state) = action
            && !done
        {
            changes.push(self.at(end..end, format!(" #{}", state.as_str())));
        }
        match completion {
            None if done => changes.push(self.at(end..end, format!(" ✅ {today}"))),
            Some(found) if !done => changes.push(self.at(found.range(), "")),
            _ => {}
        }
        changes.sort_by_key(|change| change.range.start);
        changes
    }

    /// Sets, replaces, or with no date removes one date stamp.
    pub fn date_change(
        &self,
        field: DateField,
        date: Option<&str>,
        days: &DailyNames,
    ) -> Vec<Edit> {
        let replacement = date.map_or_else(String::new, |date| stamp_text(field, date, days));
        match stamp(field).find(&self.raw) {
            Some(found) => vec![self.at(found.range(), replacement)],
            None if date.is_some() => vec![self.at(self.raw.len()..self.raw.len(), replacement)],
            None => Vec::new(),
        }
    }

    /// [`Task::date_change`] after checking the date and that only a done task gets a
    /// completion date.
    pub fn set_date(
        &self,
        field: DateField,
        date: Option<&str>,
        days: &DailyNames,
    ) -> Result<Vec<Edit>, String> {
        if let Some(date) = date
            && !days.is_calendar_date(date)
        {
            return Err(format!(
                "\"{date}\" is not a calendar date. Choose the date again from the picker."
            ));
        }
        if field == DateField::Completed && self.state != TaskState::Done {
            return Err(OPEN_COMPLETION.into());
        }
        Ok(self.date_change(field, date, days))
    }

    /// The same task in a fresh reading of its page, refusing when its text changed or when
    /// several tasks share its block ID.
    pub fn current<'t>(&self, tasks: &'t [Task]) -> Result<&'t Task, &'static str> {
        find_current(tasks, self.id.as_deref(), &self.block_key, &self.raw)
    }
}

/// The task a caller read earlier, identified by its block ID or else its block key, in a fresh
/// reading of its page. Refuses when its text is no longer `raw` or several tasks share the ID.
pub fn find_current<'t>(
    tasks: &'t [Task],
    id: Option<&str>,
    block_key: &str,
    raw: &str,
) -> Result<&'t Task, &'static str> {
    if let Some(id) = id
        && tasks
            .iter()
            .filter(|task| task.id.as_deref() == Some(id))
            .count()
            != 1
    {
        return Err(DUPLICATE_ID);
    }
    tasks
        .iter()
        .find(|task| match id {
            Some(id) => task.id.as_deref() == Some(id),
            None => task.block_key == block_key,
        })
        .filter(|task| task.raw == raw)
        .ok_or(CHANGED)
}
