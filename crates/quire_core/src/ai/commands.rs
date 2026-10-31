//! `:ai` arguments, transform prompts, and repair of the block markers a model drops or copies
//! (reference `src/core/ai-commands.ts`).

use regex::Regex;
use std::{collections::HashSet, sync::LazyLock};

fn pattern(source: &str) -> Regex {
    Regex::new(source).expect("A built-in AI command pattern is invalid. Fix it and rebuild.")
}

static ID: LazyLock<Regex> = LazyLock::new(|| pattern(r"(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*$"));
static DRAFTING: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"(?i-u)^(draft|write|add|brainstorm|list|generate|suggest|propose)(?-u:\b)")
});
static BLANK: LazyLock<Regex> = LazyLock::new(|| {
    pattern(r"^(?-u:\s)*(?:(?:[-*+]|[0-9]+[.)])(?-u:\s)*)?(?:\[[ xX]\](?-u:\s)*)?$")
});
static FENCED: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"(?s)^(?-u:\s)*```[a-zA-Z]*\n(.*?)\n```(?-u:\s)*$"));
static LINE_MARKER: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^(?:(?:[-*+]|[0-9]+[.)])(?-u:\s)+)?(?:\[[ xX]\](?-u:\s)+)?"));
static LISTED: LazyLock<Regex> =
    LazyLock::new(|| pattern(r"^(?-u:\s)*(?:[-*+]|[0-9]+[.)])(?-u:\s)"));

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Verb {
    Tighten,
    Prose,
    Outline,
    Continue,
    Check,
    Ask,
}

impl Verb {
    pub const ALL: [Verb; 6] = [
        Self::Tighten,
        Self::Prose,
        Self::Outline,
        Self::Continue,
        Self::Check,
        Self::Ask,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Self::Tighten => "tighten",
            Self::Prose => "prose",
            Self::Outline => "outline",
            Self::Continue => "continue",
            Self::Check => "check",
            Self::Ask => "ask",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Tighten => "Tighten wording",
            Self::Prose => "Rewrite as prose",
            Self::Outline => "Rewrite as an outline",
            Self::Continue => "Continue writing",
            Self::Check => "Check facts and logic",
            Self::Ask => "Ask about this text",
        }
    }

    pub fn instruction(self) -> &'static str {
        match self {
            Self::Tighten => {
                "Tighten the text. Cut redundant words and keep every fact, link, tag, task marker and the existing structure."
            }
            Self::Prose => {
                "Rewrite the outline as connected prose paragraphs. Keep every fact, link and tag."
            }
            Self::Outline => {
                "Rewrite the text as a nested Markdown bullet outline. Keep every fact, link and tag."
            }
            Self::Continue => {
                "Continue the text with the next few blocks in the same voice and format. Output only the new blocks, not the existing text."
            }
            Self::Check => {
                "Check the text for factual, logical and internal inconsistencies. Return the text with corrections applied. If nothing needs to change, return it unchanged."
            }
            Self::Ask => "Answer the question about the text.",
        }
    }

    pub fn parse(word: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|verb| verb.key().eq_ignore_ascii_case(word))
    }
}

pub const FREE: &str = "Apply this instruction to the text:";
pub const DRAFT: &str = "Write new Markdown for this request, to go below the text:";
pub const TRANSFORM_RULES: [&str; 4] = [
    "You edit one range of a Markdown notebook.",
    "Reply with only the resulting Markdown. No preamble, explanation, code fences or commentary.",
    "Keep block identifier markers such as ^b-1a2b at the end of the lines they belong to.",
    "Keep [[Page]] links, ((block-id)) references, #tags and task markers like [ ] and [x] intact unless the instruction says otherwise.",
];
pub const CONVENTIONS: [&str; 2] = [
    "Notebook conventions: pages are Markdown files whose title is the file name. [[Title]] links a page, ((id)) and [[Title#^id]] reference a block marked with ^id at the end of its line.",
    "Tasks are list items starting with [ ] or [x]. #inbox, #next, #waiting and #someday are task states; @YYYY-MM-DD is a due date.",
];
pub const IGNORED_PATHS: [&str; 3] = [".quire/", "attachments/", "assets/"];

/// The system prompt of a transform run.
pub fn transform_system_prompt() -> String {
    TRANSFORM_RULES
        .iter()
        .chain(CONVENTIONS.iter())
        .map(|line| (*line).to_owned())
        .chain([format!(
            "Ignore these folders: {}.",
            IGNORED_PATHS.join(", ")
        )])
        .collect::<Vec<_>>()
        .join("\n")
}

/// Where a finished proposal goes: over the range, below it, onto the caret's blank line, or to
/// the answer pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    Replace,
    Insert,
    Here,
    Answer,
}

/// Questions that lean on the notebook tools: `:ai <key>` with no selection asks one in the
/// answer pane. `needs` is "page", "tag", "daily", or empty.
pub const PRESETS: [(&str, &str); 6] = [
    ("due", ""),
    ("week", ""),
    ("last", "page"),
    ("yesterday", ""),
    ("tidy", "daily"),
    ("tagged", "tag"),
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Opens the answer pane, with a preset and its argument when one was named.
    Pane {
        preset: Option<String>,
        argument: String,
    },
    Run {
        verb: Option<Verb>,
        prompt: String,
        placement: Placement,
    },
}

/// `:ai` arguments: a verb with optional extra words, a preset question, or a free prompt.
/// `page` is the open page's title and `daily` whether it is a daily page.
pub fn parse_command(args: &str, has_range: bool, page: Option<&str>, daily: bool) -> Command {
    let text = args.trim();
    if text.is_empty() {
        return if has_range {
            Command::Run {
                verb: Some(Verb::Tighten),
                prompt: String::new(),
                placement: Placement::Replace,
            }
        } else {
            Command::Pane {
                preset: None,
                argument: String::new(),
            }
        };
    }
    let mut words = text.split_whitespace();
    let first = words.next().unwrap_or_default();
    let rest = words.collect::<Vec<_>>().join(" ");
    let preset = (!has_range)
        .then(|| {
            PRESETS
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(first))
        })
        .flatten();
    let pane = |key: &str, argument: String| Command::Pane {
        preset: Some(key.to_owned()),
        argument,
    };
    match preset {
        Some((key, "tag")) if !rest.is_empty() => return pane(key, rest),
        Some((key, "")) if rest.is_empty() => return pane(key, String::new()),
        Some((key, "page")) if rest.is_empty() && page.is_some() => {
            return pane(key, page.unwrap_or_default().to_owned());
        }
        Some((key, "daily")) if rest.is_empty() && page.is_some() && daily => {
            return pane(key, page.unwrap_or_default().to_owned());
        }
        _ => {}
    }
    if let Some(verb) = Verb::parse(first) {
        let placement = match verb {
            Verb::Ask => Placement::Answer,
            Verb::Continue => Placement::Insert,
            _ => Placement::Replace,
        };
        return Command::Run {
            verb: Some(verb),
            prompt: rest,
            placement,
        };
    }
    Command::Run {
        verb: None,
        prompt: text.to_owned(),
        placement: if DRAFTING.is_match(text) {
            Placement::Insert
        } else {
            Placement::Replace
        },
    }
}

/// The text a transform works on, with what surrounds it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AiRange {
    pub path: String,
    pub title: String,
    pub text: String,
    pub ancestors: Vec<String>,
    pub block_ids: Vec<String>,
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// What was sent, for the context preview.
pub fn context_summary(range: &AiRange) -> Vec<String> {
    let mut items = vec![format!("Page: {}", range.title)];
    if !range.ancestors.is_empty() {
        items.push(format!("Parent blocks: {}", range.ancestors.len()));
    }
    let lines = range.text.split('\n').count();
    let blocks = if range.block_ids.is_empty() {
        String::new()
    } else {
        format!(
            ", blocks {}",
            range
                .block_ids
                .iter()
                .map(|id| format!("^{id}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    items.push(format!("Selected text: {}{blocks}", plural(lines, "line")));
    items
}

/// The prompt for a command over a range. A question with no text attached is sent as written.
pub fn build_prompt(
    verb: Option<Verb>,
    prompt: &str,
    placement: Placement,
    range: Option<&AiRange>,
) -> String {
    let instruction = match verb {
        Some(Verb::Ask) if range.is_none() => prompt.to_owned(),
        Some(verb) => [verb.instruction(), prompt]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        None if placement == Placement::Insert => format!("{DRAFT} {prompt}"),
        None => format!("{FREE} {prompt}"),
    };
    let mut parts = vec![instruction];
    if let Some(range) = range {
        parts.push(format!("Page: {}", range.title));
        if !range.ancestors.is_empty() {
            parts.push(format!(
                "Parent blocks, for context only:\n{}",
                range
                    .ancestors
                    .iter()
                    .map(|text| format!("> {text}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            ));
        }
        parts.push(format!("Text:\n<text>\n{}\n</text>", range.text));
    }
    parts.join("\n\n")
}

/// A line with no text yet: empty, or only a bullet or task checkbox.
pub fn is_blank_line(text: &str) -> bool {
    BLANK.is_match(text)
}

/// Drafting on a blank line: the instruction plus the text above the caret.
pub fn build_draft_prompt(
    instruction: &str,
    title: &str,
    before: &str,
    continuing: bool,
) -> String {
    let lead = if continuing {
        Verb::Continue.instruction().to_owned()
    } else {
        format!("{DRAFT} {instruction}").trim().to_owned()
    };
    let context = if before.trim().is_empty() {
        "The page has no text above the cursor.".to_owned()
    } else {
        format!(
            "Text above the cursor, for context only:\n<context>\n{}\n</context>",
            before.trim_end()
        )
    };
    [lead, format!("Page: {title}"), context].join("\n\n")
}

/// Puts drafted text on the caret's blank line, keeping its indentation and any bullet or
/// checkbox already typed there.
pub fn place_here(line: &str, proposal: &str) -> String {
    let indent_len = line.len() - line.trim_start().len();
    let (indent, rest) = line.split_at(indent_len);
    let marker = LINE_MARKER.find(rest).map_or("", |found| found.as_str());
    let listed = LISTED.is_match(proposal);
    proposal
        .split('\n')
        .enumerate()
        .map(|(index, text)| match text {
            "" => String::new(),
            _ if index == 0 && !marker.is_empty() && !listed => format!("{indent}{marker}{text}"),
            _ => format!("{indent}{text}"),
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Models sometimes wrap a reply in a fence despite the rules.
pub fn clean_response(text: &str) -> String {
    let inner = FENCED
        .captures(text)
        .and_then(|captures| captures.get(1))
        .map_or(text, |inner| inner.as_str());
    inner.trim_end().to_owned()
}

/// The ID of the `^id` marker ending `line`.
pub fn marker_of(line: &str) -> Option<&str> {
    ID.captures(line)
        .and_then(|captures| captures.get(1))
        .map(|id| id.as_str())
}

fn without_marker(line: &str) -> String {
    ID.replace(line, "").trim_end().to_owned()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub text: String,
    pub restored: usize,
    pub lost: Vec<String>,
}

/// Puts back block markers the model dropped from lines whose text is otherwise unchanged,
/// matching by line text only, and lists the markers that are gone because their line changed.
pub fn restore_markers(original: &str, proposed: &str) -> Restored {
    let kept: HashSet<&str> = proposed.split('\n').filter_map(marker_of).collect();
    let mut missing: Vec<(String, String)> = Vec::new();
    for line in original.split('\n') {
        if let Some(id) = marker_of(line).filter(|id| !kept.contains(id)) {
            let key = without_marker(line).trim().to_owned();
            if !missing.iter().any(|(other, _)| *other == key) {
                missing.push((key, id.to_owned()));
            }
        }
    }
    let mut restored = 0;
    let text = proposed
        .split('\n')
        .map(|line| {
            let key = line.trim();
            if marker_of(line).is_some() || key.is_empty() {
                return line.to_owned();
            }
            match missing.iter().position(|(other, _)| other == key) {
                Some(index) => {
                    let (_, id) = missing.remove(index);
                    restored += 1;
                    format!("{} ^{id}", line.trim_end())
                }
                None => line.to_owned(),
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    Restored {
        text,
        restored,
        lost: missing.into_iter().map(|(_, id)| id).collect(),
    }
}

/// Removes block markers the model copied: any marker that was not in the original range and
/// already names a block, or that repeats in the result. Returns the text and how many went.
pub fn drop_foreign_markers(
    text: &str,
    original: &str,
    taken: impl Fn(&str) -> bool,
) -> (String, usize) {
    let own: HashSet<&str> = original.split('\n').filter_map(marker_of).collect();
    let mut seen = HashSet::new();
    let mut dropped = 0;
    let result = text
        .split('\n')
        .map(|line| {
            let Some(id) = marker_of(line) else {
                return line.to_owned();
            };
            let foreign = (!own.contains(id) && taken(id)) || seen.contains(id);
            seen.insert(id.to_owned());
            if foreign {
                dropped += 1;
                without_marker(line)
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    (result, dropped)
}

/// Indents inserted lines to sit beside the range's first line.
pub fn place_below(range_text: &str, proposal: &str) -> String {
    let indent: String = range_text
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect();
    proposal
        .split('\n')
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{indent}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The block IDs marked anywhere in `text`.
pub fn markers(text: &str) -> Vec<String> {
    text.split('\n')
        .filter_map(marker_of)
        .map(str::to_owned)
        .collect()
}
