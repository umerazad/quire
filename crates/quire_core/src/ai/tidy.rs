//! Tidy this day (reference `checkTidyChange` in `src/core/ai-changes.ts` and `tidyQuestion` in
//! `ai-commands.ts`): the request for one daily page, and the limits every proposed line must
//! meet. Tidy may only add links around words already there, a GTD state on an open task with
//! none, and a 📅 date on an open task with none.

use super::commands::marker_of;
use crate::merge::diff_regions;
use regex::Regex;
use std::sync::LazyLock;

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("A built-in Tidy pattern is invalid. Fix it and rebuild.")
}

static LINK: LazyLock<Regex> = LazyLock::new(|| pattern(r"\[\[([^\]|\n]+)(?:\|([^\]\n]+))?\]\]"));
static STATE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?i-u)(?:^|[ \t])#(next|waiting|someday|inbox|pending|done)\b"));
static OPEN_TASK: LazyLock<Regex> = LazyLock::new(|| pattern(r"^\[ \][ \t]"));
static DUE: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"[ \t]*📅[ \t]*(?:\[\[[^\]\n]+\]\]|[0-9]{4}-[0-9]{2}-[0-9]{2})"));
static SPACES: LazyLock<Regex> = LazyLock::new(|| pattern(r"[ \t]+"));
static PREFIX: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^[ \t]*(?:(?:[-*+]|[0-9]+[.)])[ \t]+)?"));
static SUFFIX: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?:^|[ \t]+)\^[A-Za-z0-9_-]+[ \t]*$"));

pub const OTHER_PAGE: &str =
    "Tidy only changes the day it was run on. Leave other pages as they are.";
pub const CHANGED_LINK: &str =
    "Tidy cannot change or remove an existing link. Keep the block's links as written.";
pub const HAS_STATE: &str =
    "This task already has a state. Tidy only adds a state to tasks without one.";
pub const ONE_STATE: &str =
    "Tidy adds one #next, #waiting or #someday to an open task without a state.";
pub const DATE_ON_TASK: &str = "Tidy adds a 📅 date only to an open task.";
pub const REWRITE: &str =
    "Tidy cannot rewrite, move or delete text. Only add links, a task state or a 📅 date.";
pub const CHANGED_ID: &str = "Tidy cannot change a block's ID. Keep each ^id as written.";
pub const ADDED_LINE: &str = "Tidy cannot add lines. Only add to lines already on the page.";
pub const REMOVED_LINE: &str = "Tidy cannot delete lines. Leave every line on the page.";

fn links(text: &str) -> Vec<&str> {
    LINK.find_iter(text).map(|found| found.as_str()).collect()
}

fn plain(text: &str) -> String {
    let unlinked = LINK.replace_all(text, |captures: &regex::Captures| {
        captures
            .get(2)
            .or(captures.get(1))
            .map_or("", |found| found.as_str())
            .trim()
            .to_owned()
    });
    SPACES.replace_all(&unlinked, " ").trim().to_owned()
}

/// Why changing one block's text from `before` to `after` is refused, or None when Tidy may
/// make it. `exists` says whether a page title exists and `is_daily` whether a title names a
/// day, whose page a due date may name before it exists.
pub fn check_change(
    before: &str,
    after: &str,
    exists: impl Fn(&str) -> bool,
    is_daily: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut added = links(after);
    for link in links(before) {
        match added.iter().position(|other| *other == link) {
            Some(at) => {
                added.remove(at);
            }
            None => return Some(CHANGED_LINK.into()),
        }
    }
    for link in added {
        let target = LINK
            .captures(link)
            .and_then(|captures| captures.get(1))
            .map_or("", |found| found.as_str())
            .trim();
        if !is_daily(target) && !exists(target) {
            return Some(format!(
                "[[{target}]] is not a page in this notebook. Link only to pages that exist."
            ));
        }
    }
    let mut rest = after.to_owned();
    let states: Vec<String> = STATE
        .captures_iter(after)
        .map(|captures| captures[1].to_lowercase())
        .collect();
    let had = STATE.find_iter(before).count();
    if states.len() > had {
        if had > 0 {
            return Some(HAS_STATE.into());
        }
        if states.len() != 1
            || !["next", "waiting", "someday"].contains(&states[0].as_str())
            || !OPEN_TASK.is_match(after)
        {
            return Some(ONE_STATE.into());
        }
        rest = STATE.replace_all(&rest, "").into_owned();
    }
    if DUE.is_match(after) && !DUE.is_match(before) {
        if !OPEN_TASK.is_match(after) {
            return Some(DATE_ON_TASK.into());
        }
        rest = DUE.replace(&rest, "").into_owned();
    }
    (plain(&rest) != plain(before)).then(|| REWRITE.into())
}

/// A line's indentation and list marker, its text, and its trailing `^id` marker.
fn parts(line: &str) -> (&str, &str, &str) {
    let prefix = PREFIX.find(line).map_or(0, |found| found.end());
    let suffix = SUFFIX
        .find(&line[prefix..])
        .map_or(line.len(), |found| prefix + found.start());
    (&line[..prefix], &line[prefix..suffix], &line[suffix..])
}

/// The line Tidy may write in place of `before`, or why `after` is refused. A marker the model
/// dropped is put back.
pub fn check_line(
    before: &str,
    after: &str,
    exists: impl Fn(&str) -> bool,
    is_daily: impl Fn(&str) -> bool,
) -> Result<String, String> {
    let (prefix, text, suffix) = parts(before);
    let (new_prefix, new_text, new_suffix) = parts(after);
    if new_prefix != prefix {
        return Err(REWRITE.into());
    }
    if !new_suffix.is_empty() && marker_of(new_suffix) != marker_of(suffix) {
        return Err(CHANGED_ID.into());
    }
    match check_change(text, new_text.trim_end(), exists, is_daily) {
        Some(reason) => Err(reason),
        None => Ok(format!("{prefix}{}{suffix}", new_text.trim_end())),
    }
}

/// A proposed line Tidy refused, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Refusal {
    pub line: String,
    pub reason: String,
}

/// What Tidy may apply: the page with only the allowed changes, how many lines they change,
/// and the refused lines.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tidied {
    pub text: String,
    pub changed: usize,
    pub refused: Vec<Refusal>,
}

/// Keeps the allowed changes of `proposed` to `original`, line by line. Lines added or deleted
/// are refused, and blank-line differences are ignored.
pub fn filter(
    original: &str,
    proposed: &str,
    exists: impl Fn(&str) -> bool,
    is_daily: impl Fn(&str) -> bool,
) -> Tidied {
    let before: Vec<&str> = original.split('\n').collect();
    let after: Vec<&str> = proposed.split('\n').collect();
    let mut lines: Vec<String> = before.iter().map(|line| (*line).to_owned()).collect();
    let mut changed = 0;
    let mut refused = Vec::new();
    let mut refuse = |line: &str, reason: &str| {
        if !line.trim().is_empty() {
            refused.push(Refusal {
                line: line.to_owned(),
                reason: reason.to_owned(),
            });
        }
    };
    for region in diff_regions(&before, &after) {
        let removed = region.a_start..region.a_end;
        let added = region.b_start..region.b_end;
        let paired = removed.len().min(added.len());
        for offset in 0..paired {
            let (at, new) = (region.a_start + offset, after[region.b_start + offset]);
            match check_line(before[at], new, &exists, &is_daily) {
                Ok(line) if line != before[at] => {
                    lines[at] = line;
                    changed += 1;
                }
                Ok(_) => {}
                Err(reason) => refuse(new, &reason),
            }
        }
        for at in removed.skip(paired) {
            refuse(before[at], REMOVED_LINE);
        }
        for at in added.skip(paired) {
            refuse(after[at], ADDED_LINE);
        }
    }
    Tidied {
        text: lines.join("\n"),
        changed,
        refused,
    }
}

/// The request for one daily page. `titles` are the existing pages its text may mention, and
/// `due_example` the title of a day a 📅 stamp could name.
pub fn prompt(page: &str, text: &str, titles: &[String], due_example: &str) -> String {
    let listed = if titles.is_empty() {
        "No existing page titles match words on this page, so add no links.".to_owned()
    } else {
        format!(
            "Existing pages it may link:\n{}",
            titles
                .iter()
                .map(|title| format!("- {title}"))
                .collect::<Vec<_>>()
                .join("\n")
        )
    };
    [
        format!(
            "Tidy the daily page [[{page}]]. Reply with the whole page, line for line, changed only by these additions:"
        ),
        "1. Links: turn plain-text mentions of the existing pages listed below into [[links]]. Use [[Page|text]] when the wording on the page differs from the title. Never link a page that is not listed.".into(),
        "2. States: give each open task with no state one of #next, #waiting or #someday.".into(),
        format!(
            "3. Dates: when a task's text names a date (such as \"by Thursday\"), add a due stamp such as 📅 [[{due_example}]], counting from the page's own date."
        ),
        "Do not change, move, add or delete any other text, and keep every block marker such as ^b-1a2b at the end of its line. Quire refuses anything else.".into(),
        String::new(),
        listed,
        String::new(),
        format!("Text:\n<text>\n{text}\n</text>"),
    ]
    .join("\n")
}

/// The titles worth offering Tidy: pages whose title, or a word of it, appears on the page.
pub fn mentioned_titles<'a>(
    text: &str,
    titles: impl IntoIterator<Item = &'a str>,
    limit: usize,
) -> Vec<String> {
    let words: std::collections::HashSet<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| word.chars().count() >= 3)
        .map(str::to_lowercase)
        .collect();
    let mut found: Vec<String> = titles
        .into_iter()
        .filter(|title| {
            title
                .split(|c: char| !c.is_alphanumeric())
                .filter(|word| word.chars().count() >= 3)
                .any(|word| words.contains(&word.to_lowercase()))
        })
        .map(str::to_owned)
        .collect();
    found.sort();
    found.dedup();
    found.truncate(limit);
    found
}
