//! Questions about the notebook (reference `AiPage.tsx` and the ask half of `ai-commands.ts`):
//! the presets, the prompt with the notebook excerpts gathered for it, the context summary,
//! citations turned into links, and the block an answer becomes when it is kept.

use super::commands::{AiRange, CONVENTIONS, Verb};
use crate::markdown::{ReferenceKind, decode_uri_component, references};
use regex::Regex;
use std::{collections::HashSet, sync::LazyLock};

pub const DEFAULT_SYNTHESIS: &str =
    "Synthesize these linked references: the main themes, how they connect, and open questions.";
/// The question box text the Tagged with… preset starts.
pub const TAGGED_DRAFT: &str = "Show me everything tagged #";
pub const NO_CONTEXT: &str = "Question only; no notebook excerpts matched it.";

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

/// Where the excerpts sent with a question came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Linked references to a page, for a synthesis.
    References(String),
    /// Open tasks due by a date.
    Due,
    /// Open tasks due within the week.
    Week,
    /// The latest pages that mention a page.
    Mentions(String),
    /// Yesterday's daily page and the tasks done or added that day.
    Yesterday,
    /// Blocks with a tag.
    Tagged(String),
    /// Blocks matching the question's words.
    Search,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Self::References(_) => "Linked references".into(),
            Self::Due => "Tasks due today or overdue".into(),
            Self::Week => "Tasks due this week".into(),
            Self::Mentions(page) => format!("Latest mentions of {page}"),
            Self::Yesterday => "Yesterday's page and tasks".into(),
            Self::Tagged(tag) => format!("Blocks tagged #{}", tag.trim_start_matches('#')),
            Self::Search => "Matching blocks".into(),
        }
    }
}

/// One block of notebook text sent with a question.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Excerpt {
    pub title: String,
    pub id: Option<String>,
    pub text: String,
}

/// The excerpts sent with a question, and how many were left out to stay under the limits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Gathered {
    pub source: Source,
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
    pub fn bounded(source: Source, excerpts: impl IntoIterator<Item = Excerpt>) -> Self {
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
            source,
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

/// The rules an ask run follows. Answers come from the excerpts sent; the run has no tools.
pub const ASK_RULES: [&str; 3] = [
    "You answer questions about the writer's Quire notebook. Use the notebook excerpts and the conversation given with the question; if they do not answer it, say what is missing rather than guessing.",
    "Answer in Markdown. Cite sources inline: a page as [[Page title]], a block with an id as ((block-id)), and a block without an id as [[Page title]] followed by its exact words in double quotes.",
    "Do not change the notebook; you cannot.",
];

pub fn ask_system_prompt(today: &str) -> String {
    ASK_RULES
        .iter()
        .map(|line| (*line).to_owned())
        .chain([format!("Today is {today}.")])
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
        parts.push(format!("{}:\n{blocks}", gathered.source.label()));
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
            format!("{}: none found", gathered.source.label())
        } else {
            format!(
                "{}: {} from {}",
                gathered.source.label(),
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

const STOP_WORDS: [&str; 40] = [
    "the", "and", "for", "are", "was", "were", "what", "when", "where", "which", "who", "whom",
    "why", "how", "this", "that", "these", "those", "with", "from", "about", "into", "have", "has",
    "had", "did", "does", "can", "could", "would", "should", "will", "any", "all", "my", "our",
    "your", "there", "their", "notes",
];

/// The words of a question worth searching for, longest first, at most six.
pub fn search_terms(question: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut words: Vec<String> = question
        .split(|c: char| !c.is_alphanumeric() && c != '-' && c != '\'')
        .map(|word| word.trim_matches(['-', '\'']).to_lowercase())
        .filter(|word| word.chars().count() >= 3 && !STOP_WORDS.contains(&word.as_str()))
        .filter(|word| seen.insert(word.clone()))
        .collect();
    words.sort_by_key(|word| std::cmp::Reverse(word.chars().count()));
    words.truncate(6);
    words
}
