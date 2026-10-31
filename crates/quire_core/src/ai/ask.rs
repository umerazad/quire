//! Questions about the notebook (reference `AiPage.tsx` and the ask half of `ai-commands.ts`):
//! the presets, the tool rules, the prompt with the range or linked references sent with it, the
//! context summary, citations turned into links, and the block an answer becomes when it is kept.

use super::commands::{AiRange, CONVENTIONS, Verb};
use super::tools::Writes;
use crate::markdown::{ReferenceKind, decode_uri_component, references};
use regex::Regex;
use std::{collections::HashSet, sync::LazyLock};

pub const DEFAULT_SYNTHESIS: &str =
    "Synthesize these linked references: the main themes, how they connect, and open questions.";
/// The question box text the Tagged with… preset starts.
pub const TAGGED_DRAFT: &str = "Show me everything tagged #";

/// What a preset needs before it can be asked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    Nothing,
    /// The open page's title.
    Page,
    /// A tag, typed after the preset.
    Tag,
    /// The open page, which must be a daily page.
    Daily,
}

/// A suggested question. `:ai <key>` asks one with no selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    pub key: &'static str,
    pub label: &'static str,
    pub needs: Needs,
}

pub const PRESETS: [Preset; 6] = [
    Preset {
        key: "due",
        label: "Due today",
        needs: Needs::Nothing,
    },
    Preset {
        key: "week",
        label: "This week",
        needs: Needs::Nothing,
    },
    Preset {
        key: "last",
        label: "Summarize the last occurrence",
        needs: Needs::Page,
    },
    Preset {
        key: "yesterday",
        label: "What changed yesterday",
        needs: Needs::Nothing,
    },
    Preset {
        key: "tidy",
        label: "Tidy this day",
        needs: Needs::Daily,
    },
    Preset {
        key: "tagged",
        label: "Tagged with…",
        needs: Needs::Tag,
    },
];

impl Preset {
    pub fn find(key: &str) -> Option<&'static Preset> {
        PRESETS
            .iter()
            .find(|preset| preset.key.eq_ignore_ascii_case(key))
    }

    /// The question the preset asks about `value`: the page title or the tag.
    pub fn question(&self, value: &str) -> String {
        match self.key {
            "due" => "What is due today? List overdue tasks separately, with their pages.".into(),
            "week" => "What is due this week, day by day?".into(),
            "last" => format!(
                "Find the last occurrence of [[{value}]] and summarize it: decisions, follow-ups and open tasks."
            ),
            "yesterday" => "What changed in the notebook yesterday? Summarize by page, with new and completed tasks.".into(),
            "tidy" => format!("Tidy the daily page [[{value}]]."),
            _ => format!(
                "Show me everything tagged #{} from the last month, grouped by page.",
                value.trim_start_matches('#')
            ),
        }
    }

    pub fn label_for(&self, page: Option<&str>) -> String {
        match (self.needs, page) {
            (Needs::Page, Some(page)) => format!("{} of {page}", self.label),
            _ => self.label.to_owned(),
        }
    }

    /// Whether the preset is offered for the open page.
    pub fn offered(&self, page: Option<&str>, daily: bool) -> bool {
        match self.needs {
            Needs::Page => page.is_some(),
            Needs::Daily => page.is_some() && daily,
            Needs::Nothing | Needs::Tag => true,
        }
    }
}

/// What a synthesis sends with its instruction (reference `scope`); questions read the rest of
/// the notebook through the tools.
pub const REFERENCES: &str = "Linked references";

/// One linked reference sent with a synthesis.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub title: String,
    pub id: Option<String>,
    pub text: String,
}

/// The linked references sent with a synthesis, and how many were left out to stay under the
/// limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gathered {
    pub excerpts: Vec<Excerpt>,
    pub omitted: usize,
}

pub const MAX_EXCERPTS: usize = 60;
pub const MAX_EXCERPT_CHARS: usize = 2_000;
pub const MAX_CONTEXT_CHARS: usize = 60_000;
const EARLIER_TURNS: usize = 4;
const EARLIER_CHARS: usize = 12_000;

fn clip(text: &str, limit: usize) -> String {
    match text.char_indices().nth(limit) {
        Some((end, _)) => format!("{}…", &text[..end]),
        None => text.to_owned(),
    }
}

impl Gathered {
    /// Keeps the first excerpts that fit the count and size limits, each clipped, and counts the
    /// rest. Repeated blocks are sent once.
    pub fn bounded(excerpts: impl IntoIterator<Item = Excerpt>) -> Self {
        let mut seen = HashSet::new();
        let mut kept = Vec::new();
        let mut omitted = 0;
        let mut size = 0;
        for excerpt in excerpts {
            let text = clip(excerpt.text.trim(), MAX_EXCERPT_CHARS);
            if text.is_empty() || !seen.insert((excerpt.title.clone(), text.clone())) {
                continue;
            }
            if kept.len() == MAX_EXCERPTS || size + text.len() > MAX_CONTEXT_CHARS {
                omitted += 1;
                continue;
            }
            size += text.len();
            kept.push(Excerpt { text, ..excerpt });
        }
        Self {
            excerpts: kept,
            omitted,
        }
    }

    fn pages(&self) -> usize {
        self.excerpts
            .iter()
            .map(|excerpt| excerpt.title.as_str())
            .collect::<HashSet<_>>()
            .len()
    }
}

/// The rules of an ask run, which reads the notebook through the quire tools (reference
/// `aiPrompts.toolRules`).
pub const TOOL_RULES: [&str; 4] = [
    "You answer questions about the writer's Quire notebook through the quire tools. They read the notebook as it is open in the app, including unsaved edits. Do not look for files, and do not read the .quire folder or attachment files.",
    "Choose the structured tool first: tasks for what is due or open, tagged_blocks for #tags, mentions for the last time a page or meeting came up, daily_pages and list_pages for dates, read_page and read_block to read, linked_references for what links to a page. Use search_notebook to find words.",
    "Pages are named by title. \"today\" names today's daily page.",
    "Answer in Markdown. Cite sources inline: a page as [[Page title]], a block with a marker as ((block-id)), and a block without a marker as [[Page title]] followed by its exact words in double quotes.",
];

/// What an ask run may change, by the AI writes setting (reference `aiPrompts.writeRules`).
pub fn write_rule(writes: Writes) -> &'static str {
    match writes {
        Writes::Off => {
            "You cannot change the notebook. If the writer asks for a change, say that AI writes are off in Settings > AI."
        }
        Writes::Review => {
            "Change the notebook only when the writer asks. Write tools stage each change for the writer to review in Quire; say what you proposed."
        }
        Writes::Apply => {
            "Change the notebook only when the writer asks. Write tools apply at once and the writer can undo each change; say what you changed."
        }
    }
}

/// The system prompt of an ask run (reference `systemPrompt("ask", writes)`).
pub fn ask_system_prompt(writes: Writes, today: &str) -> String {
    TOOL_RULES
        .iter()
        .map(|line| (*line).to_owned())
        .chain([write_rule(writes).to_owned(), format!("Today is {today}.")])
        .chain(CONVENTIONS.iter().map(|line| (*line).to_owned()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A finished question and answer, sent with later questions in the same pane.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Turn {
    pub question: String,
    pub answer: String,
}

fn attribute(value: &str) -> String {
    value.replace('&', "&amp;").replace('"', "&quot;")
}

/// The prompt for a question: the question as written when nothing goes with it, otherwise the
/// question with the range, the excerpts, and the latest earlier turns.
pub fn build_ask_prompt(
    question: &str,
    range: Option<&AiRange>,
    gathered: Option<&Gathered>,
    earlier: &[Turn],
) -> String {
    let excerpts = gathered.filter(|gathered| !gathered.excerpts.is_empty());
    let mut parts = Vec::new();
    let recent = &earlier[earlier.len().saturating_sub(EARLIER_TURNS)..];
    if !recent.is_empty() {
        let mut size = 0;
        let mut turns: Vec<String> = recent
            .iter()
            .rev()
            .map(|turn| {
                format!(
                    "<question>\n{}\n</question>\n<answer>\n{}\n</answer>",
                    turn.question.trim(),
                    turn.answer.trim()
                )
            })
            .take_while(|text| {
                size += text.len();
                size <= EARLIER_CHARS
            })
            .collect();
        turns.reverse();
        if !turns.is_empty() {
            parts.push(format!(
                "Earlier in this conversation:\n{}",
                turns.join("\n")
            ));
        }
    }
    parts.push(match range {
        Some(_) => format!("{} {}", Verb::Ask.instruction(), question.trim())
            .trim()
            .to_owned(),
        None => question.trim().to_owned(),
    });
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
    if let Some(gathered) = excerpts {
        let blocks = gathered
            .excerpts
            .iter()
            .map(|excerpt| {
                let id = excerpt
                    .id
                    .as_ref()
                    .map(|id| format!(" id=\"{}\"", attribute(id)))
                    .unwrap_or_default();
                format!(
                    "<excerpt page=\"{}\"{id}>\n{}\n</excerpt>",
                    attribute(&excerpt.title),
                    excerpt.text
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        parts.push(format!("{}:\n{blocks}", REFERENCES));
    }
    parts.join("\n\n")
}

fn plural(count: usize, word: &str) -> String {
    format!("{count} {word}{}", if count == 1 { "" } else { "s" })
}

/// What was sent with a question, for the context list under it.
pub fn ask_context_summary(range: Option<&AiRange>, gathered: Option<&Gathered>) -> Vec<String> {
    let mut items = range
        .map(super::commands::context_summary)
        .unwrap_or_default();
    if let Some(gathered) = gathered {
        items.push(if gathered.excerpts.is_empty() {
            format!("{}: none found", REFERENCES)
        } else {
            format!(
                "{}: {} from {}",
                REFERENCES,
                plural(gathered.excerpts.len(), "block"),
                plural(gathered.pages(), "page")
            )
        });
        if gathered.omitted > 0 {
            items.push(format!(
                "{} left out to stay under the size limit",
                plural(gathered.omitted, "more block")
            ));
        }
    }
    items
}

/// What a citation link opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cited {
    Page(String),
    Block(String),
}

const PAGE_URL: &str = "quire-page:";
const BLOCK_URL: &str = "quire-block:";

fn encode(target: &str) -> String {
    target
        .bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

fn escape_label(label: &str) -> String {
    label
        .chars()
        .flat_map(|c| match c {
            '[' | ']' | '\\' | '*' | '_' | '`' => vec!['\\', c],
            c => vec![c],
        })
        .collect()
}

/// Turns `[[Page]]`, `[[Page|label]]`, `((id))`, and `[[Page#^id]]` citations into Markdown
/// links the answer pane follows. `label` names a block for its link text.
pub fn link_citations(markdown: &str, label: impl Fn(&str) -> Option<String>) -> String {
    let found = references(markdown, &[]);
    let mut result = String::with_capacity(markdown.len());
    let mut at = 0;
    for reference in found {
        let (text, url) = match reference.kind {
            ReferenceKind::Page => (
                reference
                    .label
                    .clone()
                    .unwrap_or_else(|| reference.target.clone()),
                format!("{PAGE_URL}{}", encode(&reference.target)),
            ),
            ReferenceKind::Block => {
                let id = reference
                    .target
                    .rsplit_once("#^")
                    .map_or(reference.target.as_str(), |(_, id)| id);
                let shown = reference
                    .label
                    .clone()
                    .or_else(|| label(id))
                    .unwrap_or_else(|| "block".into());
                (
                    format!("↗ {}", clip(&shown, 60)),
                    format!("{BLOCK_URL}{}", encode(&reference.target)),
                )
            }
            ReferenceKind::Embed => continue,
        };
        result.push_str(&markdown[at..reference.range.start]);
        result.push_str(&format!("[{}]({url})", escape_label(&text)));
        at = reference.range.end;
    }
    result.push_str(&markdown[at..]);
    result
}

/// The block IDs an answer cites, once each, in order.
pub fn cited_blocks(markdown: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    references(markdown, &[])
        .into_iter()
        .filter(|reference| reference.kind == ReferenceKind::Block)
        .map(|reference| {
            reference
                .target
                .rsplit_once("#^")
                .map_or(reference.target.as_str(), |(_, id)| id)
                .to_owned()
        })
        .filter(|id| seen.insert(id.clone()))
        .collect()
}

/// What a link in an answer opens, when it is a citation.
pub fn parse_citation(url: &str) -> Option<Cited> {
    if let Some(page) = url.strip_prefix(PAGE_URL) {
        return decode_uri_component(page).map(Cited::Page);
    }
    url.strip_prefix(BLOCK_URL)
        .and_then(decode_uri_component)
        .map(Cited::Block)
}

static QUOTE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"\[\[([^\]|\n]+)\]\][ \t]*"([^"\n]{3,})""#)
        .expect("The quoted citation pattern is invalid. Fix it and rebuild.")
});

/// Turns `[[Page]] "exact words"` citations into block references through `mint`, which names
/// the quoted block's ID. Quotes it cannot place stay as written.
pub fn resolve_quotes(
    markdown: &str,
    mut mint: impl FnMut(&str, &str) -> Option<String>,
) -> String {
    QUOTE
        .replace_all(markdown, |captures: &regex::Captures| {
            let (title, quote) = (&captures[1], &captures[2]);
            match mint(title, quote) {
                Some(id) => format!("\"{quote}\" (({id}))"),
                None => captures[0].to_owned(),
            }
        })
        .into_owned()
}

/// An answer as one block for a page: its first line a bullet, the rest nested under it.
pub fn answer_block(answer: &str) -> String {
    let mut lines = answer.trim().split('\n');
    let first = lines.next().unwrap_or_default();
    std::iter::once(format!("- {first}"))
        .chain(lines.map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("  {line}")
            }
        }))
        .collect::<Vec<_>>()
        .join("\n")
}
