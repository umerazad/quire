//! Tidy this day (reference `checkTidyChange` in `src/core/ai-changes.ts`, `tidyQuestion` and
//! `tidyBlocked` in `ai-commands.ts`, and the Tidy limits of `notebook-host.ts`): the question
//! for one daily page, and the limits every change it proposes through `replace_block` must
//! meet. Tidy may only add links around words already there, a GTD state on an open task with
//! none, and a 📅 date on an open task with none.

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

/// Tidy's tool calls may only replace blocks.
pub const TIDY_ONLY: &str = "Tidy proposes only replace_block changes to blocks on the day's page. It cannot create pages, append blocks, capture tasks or change task states.";
pub const WRITES_OFF: &str = "AI writes are Off. Set them to Review or Apply in Settings > AI to use Tidy; Tidy always asks before changing anything.";

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

/// The question Tidy this day asks about the daily page `page` (reference `tidyQuestion`);
/// `due_example` is the title of the day after it, for the 📅 example.
pub fn question(page: &str, due_example: &str) -> String {
    [
        format!(
            "Tidy the daily page [[{page}]]. Read it with read_page, then propose replace_block changes, passing each block's key and the page, that only add to what is there:"
        ),
        "1. Links: turn plain-text mentions of existing people, meeting and project pages into [[links]]. Find each page with list_pages: contains matches a name anywhere in a title, and namespace matches a folder such as People. Check the exact titles before linking. Use [[Page|text]] when the wording on the page differs from the title. Never link a page that does not exist.".into(),
        "2. States: give each open task with no state one of #next, #waiting or #someday.".into(),
        format!(
            "3. Dates: when a task's text names a date (such as \"by Thursday\"), add a due stamp such as 📅 [[{due_example}]], counting from the page's own date."
        ),
        "Do not change, move or delete any other text, do not touch other pages and do not create pages. Quire refuses anything else. Finish with a short list of what you proposed.".into(),
    ]
    .join("\n")
}

fn links(text: &str) -> Vec<&str> {
    LINK.find_iter(text).map(|found| found.as_str()).collect()
}

/// The page titles the links in `text` name.
pub fn link_targets(text: &str) -> Vec<String> {
    LINK.captures_iter(text)
        .filter_map(|captures| Some(captures.get(1)?.as_str().trim().to_owned()))
        .collect()
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
