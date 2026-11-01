//! The task shortcut and capture commands as plans over one page: what to insert or change,
//! and where the caret goes. Callers apply the edits as one undoable change.

use super::{Action, DailyNames, Edit, Page, PageBlock, Task, TaskState};
use crate::markdown::{TagToken, frontmatter_length, js_space};
use std::ops::Range;

pub const NOT_IN_CODE: &str =
    "A task cannot be added inside code. Move the cursor to a list block or paragraph.";
pub const NOT_HERE: &str =
    "A task cannot be added here. Place the cursor in a list block, paragraph or empty line.";
pub const NOT_IN_SELECTION: &str =
    "A task cannot be added in this selection. Select list blocks or paragraphs.";
pub const NO_TASK: &str =
    "No task here to date. Place the cursor on a task line, or select task lines, and try again.";
pub const CAPTURE_ONE: &str = "Capture one task at a time. Use its source note for longer details.";

/// The task keys: toggle done (Cmd-Enter) or cycle Inbox, Next, Waiting, Someday
/// (Cmd-Shift-Enter), which reopens a done task into Inbox.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shortcut {
    Toggle,
    Cycle,
}

impl Shortcut {
    pub fn action(self, task: &Task) -> Action {
        match self {
            Self::Toggle => Action::Toggle,
            Self::Cycle => Action::Set(match task.state {
                TaskState::Inbox => TaskState::Next,
                TaskState::Next => TaskState::Waiting,
                TaskState::Waiting => TaskState::Someday,
                TaskState::Someday | TaskState::Done => TaskState::Inbox,
            }),
        }
    }
}

/// Edits plus, when the command made a new task, where the caret's anchor and head go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskEdit {
    pub edits: Vec<Edit>,
    pub selection: Option<(usize, usize)>,
}

/// The stamp a new task gets, linking the day it was made. A task on today's daily page needs
/// none: the page already records the day.
pub fn created_stamp(title: &str, today: &str, days: &DailyNames) -> String {
    if days.date(title).as_deref() == Some(today) {
        String::new()
    } else {
        super::stamp_text(super::DateField::Created, today, days)
    }
}

fn inside_code(page: &Page, position: usize) -> bool {
    page.code_ranges
        .iter()
        .any(|range| range.start < position && position <= range.end)
}

fn line_at(text: &str, position: usize) -> Range<usize> {
    let from = text[..position].rfind('\n').map_or(0, |at| at + 1);
    let to = text[position..]
        .find('\n')
        .map_or(text.len(), |at| position + at);
    from..to
}

fn text_start(text: &str, line: &Range<usize>) -> usize {
    line.end - text[line.clone()].trim_start_matches(js_space).len()
}

/// The reference `blockAt`: the last block starting at or before `position`, if it reaches it.
fn block_at<'b>(blocks: &'b [PageBlock<'b>], position: usize) -> Option<&'b PageBlock<'b>> {
    let index = blocks.partition_point(|block| block.from <= position);
    let block = blocks.get(index.checked_sub(1)?)?;
    (position <= block.content.1).then_some(block)
}

fn insert(at: usize, text: impl Into<String>) -> Edit {
    Edit {
        range: at..at,
        replacement: text.into(),
    }
}

/// Toggles or cycles the task at `position`, or makes the list item, paragraph, or empty line
/// there a task stamped with today. `selection` is the caret's anchor and head.
pub fn task_at(
    page: &Page,
    tasks: &[Task],
    days: &DailyNames,
    today: &str,
    position: usize,
    selection: (usize, usize),
    shortcut: Shortcut,
) -> Result<TaskEdit, &'static str> {
    let text = page.text;
    let line = line_at(text, position);
    let position = position.max(text_start(text, &line));
    if inside_code(page, position) {
        return Err(NOT_IN_CODE);
    }
    let block = block_at(page.blocks, position);
    if let Some(task) = block.and_then(|block| {
        tasks
            .iter()
            .find(|task| task.content.start == block.content.0)
    }) {
        let current = task.current(tasks)?;
        return Ok(TaskEdit {
            edits: current.changes(shortcut.action(task), today),
            selection: None,
        });
    }
    if position < frontmatter_length(text)
        || block.is_some_and(|block| !matches!(block.kind, "list" | "paragraph"))
        || (block.is_none() && !text[line.clone()].trim_matches(js_space).is_empty())
    {
        return Err(NOT_HERE);
    }
    let (from, checkbox, end) = match block {
        Some(block) => (
            block.content.0,
            if block.is_list() { "[ ] " } else { "- [ ] " },
            block.content.0 + block.text(text).len(),
        ),
        None => (line.end, "- [ ] ", line.end),
    };
    let stamp = created_stamp(page.title, today, days);
    let shift = |value: usize| {
        value
            + if value > from { checkbox.len() } else { 0 }
            + if value > end { stamp.len() } else { 0 }
    };
    let map = |value: usize| {
        if value == from {
            shift(from) + checkbox.len()
        } else {
            shift(value)
        }
    };
    Ok(TaskEdit {
        selection: Some((map(selection.0), map(selection.1))),
        edits: vec![insert(from, checkbox), insert(end, stamp)],
    })
}

/// Makes each selected paragraph line and list item without a task a task. With none left to
/// make, toggles or cycles every selected task. A selection ending at a line's start leaves that
/// line out.
pub fn tasks_in(
    page: &Page,
    tasks: &[Task],
    days: &DailyNames,
    today: &str,
    selection: Range<usize>,
    shortcut: Shortcut,
) -> Result<Vec<Edit>, &'static str> {
    let text = page.text;
    let frontmatter = frontmatter_length(text);
    let stamp = created_stamp(page.title, today, days);
    let first = line_at(text, selection.start).start;
    let mut last = line_at(text, selection.end);
    if selection.end > selection.start && last.start == selection.end && last.start > 0 {
        last = line_at(text, last.start - 1);
    }
    let mut existing = Vec::new();
    let mut created = Vec::new();
    let mut from = first;
    while from <= last.start {
        let line = line_at(text, from);
        from = line.end + 1;
        let start = text_start(text, &line);
        if start == line.end || start < frontmatter || inside_code(page, start) {
            continue;
        }
        let block = block_at(page.blocks, start);
        if block.is_some_and(|block| block.kind == "paragraph") {
            let end = line.start + text[line.clone()].trim_end_matches(js_space).len();
            created.extend([insert(start, "- [ ] "), insert(end, stamp.as_str())]);
        }
        let Some(block) =
            block.filter(|block| block.is_list() && line_at(text, block.from).start == line.start)
        else {
            continue;
        };
        match tasks
            .iter()
            .find(|task| task.content.start == block.content.0)
        {
            Some(task) => existing.push(task),
            None => created.extend([
                insert(block.content.0, "[ ] "),
                insert(block.content.0 + block.text(text).len(), stamp.as_str()),
            ]),
        }
    }
    if existing.is_empty() && created.is_empty() {
        return Err(NOT_IN_SELECTION);
    }
    Ok(if created.is_empty() {
        existing
            .into_iter()
            .flat_map(|task| task.changes(shortcut.action(task), today))
            .collect()
    } else {
        created
    })
}

/// Sets the due date of the task at a caret, or of every task starting on a selected line, to
/// `date`, or with `None` removes their due stamps. A selection ending at a line's start leaves
/// that line out.
pub fn due_in(
    page: &Page,
    tasks: &[Task],
    days: &DailyNames,
    selection: Range<usize>,
    date: Option<&str>,
) -> Result<Vec<Edit>, &'static str> {
    let text = page.text;
    let first = line_at(text, selection.start);
    let chosen: Vec<&Task> = if selection.is_empty() {
        let position = selection.start.max(text_start(text, &first));
        block_at(page.blocks, position)
            .and_then(|block| {
                tasks
                    .iter()
                    .find(|task| task.content.start == block.content.0)
            })
            .into_iter()
            .collect()
    } else {
        let mut last = line_at(text, selection.end);
        if last.start == selection.end && last.start > 0 {
            last = line_at(text, last.start - 1);
        }
        tasks
            .iter()
            .filter(|task| (first.start..=last.end).contains(&task.content.start))
            .collect()
    };
    if chosen.is_empty() {
        return Err(NO_TASK);
    }
    Ok(chosen
        .into_iter()
        .flat_map(|task| task.date_change(super::DateField::Due, date, days))
        .collect())
}

/// Appends a captured task to a page's text, replacing a new daily page's lone empty bullet.
/// `value_tags` are the captured text's tags; with no state tag the task goes to Inbox.
pub fn capture(
    current: &str,
    value: &str,
    value_tags: &[TagToken],
    link: Option<&str>,
    stamp: &str,
) -> Result<Edit, &'static str> {
    let trimmed = value.trim_matches(js_space);
    if trimmed.is_empty() || trimmed.contains(['\r', '\n']) {
        return Err(CAPTURE_ONE);
    }
    let has_state = value_tags.iter().any(|tag| {
        matches!(
            tag.text.to_lowercase().as_str(),
            "#inbox" | "#next" | "#waiting" | "#someday" | "#done"
        )
    });
    let placeholder = {
        let body = current.strip_suffix('\n').unwrap_or(current);
        body.strip_prefix('-')
            .is_some_and(|rest| rest.bytes().all(|b| b == b' ' || b == b'\t'))
    };
    let separator = if !current.is_empty() && !placeholder && !current.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    Ok(Edit {
        range: if placeholder { 0 } else { current.len() }..current.len(),
        replacement: format!(
            "{separator}- [ ] {trimmed}{}{}{stamp}\n",
            if has_state { "" } else { " #inbox" },
            link.map_or_else(String::new, |link| format!(" [[{link}]]")),
        ),
    })
}
