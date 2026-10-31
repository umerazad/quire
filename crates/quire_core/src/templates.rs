//! Templates, ported from the reference `templates.ts`. They live on one page, `Templates.md` at
//! the notebook root: each top-level block there with indented blocks under it is a template
//! whose text is its name and whose indented blocks are inserted.
//!
//! Block parsing belongs to the caller, which hands each function the blocks and references of
//! the text it passes. Expansion is resumable: [`expand`] returns the first question it has no
//! answer for, so the caller can ask it, add the answer, and call again. Questions are asked in
//! the order the reference asks them, each once.

use crate::markdown::{Reference, ReferenceKind, js_space, page_name};
use crate::tasks::{DailyNames, add_days};
use regex::Regex;
use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    sync::LazyLock,
};

pub mod guide;

pub const TEMPLATES_TITLE: &str = "Templates";
pub const TEMPLATES_PATH: &str = "Templates.md";

const CURSOR: &str = "\u{fdd0}QUIRE_CURSOR\u{fdd1}";
const WEEKDAYS: [&str; 7] = [
    "sunday",
    "monday",
    "tuesday",
    "wednesday",
    "thursday",
    "friday",
    "saturday",
];

pub const CANCELLED: &str =
    "Template insertion cancelled. The block is unchanged; type jj again to retry.";
pub const CHANGED: &str =
    "The template changed while its list was open. Type jj again and pick its current version.";

pub fn is_templates_page(path: &str) -> bool {
    path.to_lowercase() == "templates.md"
}

/// One block of parsed text, as the caller's Markdown analysis reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Block {
    pub key: String,
    pub id: Option<String>,
    pub list: bool,
    pub from: usize,
    pub content_from: usize,
    pub content_to: usize,
    pub subtree_to: usize,
    pub parent: Option<String>,
    /// The content without a trailing block ID, trimmed at the end.
    pub text: String,
}

/// The blocks and references of one text.
#[derive(Clone, Debug, Default)]
pub struct Parsed {
    pub blocks: Vec<Block>,
    pub references: Vec<Reference>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Definition {
    pub path: String,
    pub key: String,
    pub name: String,
    pub body: String,
}

fn collapse_space(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut space = false;
    for c in text.chars() {
        if js_space(c) {
            space = true;
        } else {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        }
    }
    out
}

/// The templates on a page: none unless it is the Templates page.
pub fn templates_in_page(path: &str, text: &str, blocks: &[Block]) -> Vec<Definition> {
    if !is_templates_page(path) {
        return Vec::new();
    }
    blocks
        .iter()
        .filter(|block| block.parent.is_none())
        .filter_map(|block| {
            let first = blocks
                .iter()
                .find(|child| child.parent.as_deref() == Some(&block.key))?;
            let stripped: String = block
                .text
                .chars()
                .filter(|c| !matches!(c, '*' | '_' | '`'))
                .collect();
            let name = collapse_space(&stripped);
            if name.is_empty() {
                return None;
            }
            let line = text[..first.from].rfind('\n').map_or(0, |at| at + 1);
            let indent = &text[line..first.from];
            let body = text[line..block.subtree_to]
                .split('\n')
                .map(|value| value.strip_prefix(indent).unwrap_or(value))
                .collect::<Vec<_>>()
                .join("\n");
            Some(Definition {
                path: path.to_owned(),
                key: block.key.clone(),
                name,
                body,
            })
        })
        .collect()
}

static COLLATOR: LazyLock<icu_collator::CollatorBorrowed<'static>> = LazyLock::new(|| {
    icu_collator::Collator::try_new(Default::default(), Default::default())
        .expect("Quire's bundled collation data is missing. Rebuild with the compiled ICU data.")
});

/// Orders names as the reference `localeCompare` does.
pub fn collate(a: &str, b: &str) -> Ordering {
    COLLATOR.compare(a, b)
}

/// Every template in the given pages, ordered by name.
pub fn catalogue(mut definitions: Vec<Definition>) -> Vec<Definition> {
    definitions.sort_by(|a, b| collate(&a.name, &b.name));
    definitions
}

/// Refuses a listed template that no longer matches the page.
pub fn current<'a>(listed: &Definition, now: &'a [Definition]) -> Result<&'a Definition, String> {
    now.iter()
        .find(|value| value.path == listed.path && value.key == listed.key)
        .filter(|value| value.name == listed.name && value.body == listed.body)
        .ok_or_else(|| CHANGED.to_owned())
}

static SIGNED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[+-][0-9]{1,4}$").unwrap());
static ISO: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}$").unwrap());

fn weekday(iso: &str) -> Option<usize> {
    let date: jiff::civil::Date = iso.parse().ok()?;
    Some(date.weekday().to_sunday_zero_offset() as usize)
}

/// A date phrase: today, tomorrow, yesterday, +N or -N days, a weekday (the next one after
/// today), or YYYY-MM-DD.
pub fn template_date(phrase: &str, today: &str) -> Result<String, String> {
    let value = phrase.trim_matches(js_space).to_lowercase();
    let shift = |days: i64| add_days(today, days).ok_or_else(|| unclear(phrase));
    if value.is_empty() || value == "today" {
        return shift(0);
    }
    if value == "tomorrow" {
        return shift(1);
    }
    if value == "yesterday" {
        return shift(-1);
    }
    if SIGNED.is_match(&value) {
        return shift(
            value
                .trim_start_matches('+')
                .parse()
                .map_err(|_| unclear(phrase))?,
        );
    }
    if value.chars().count() >= 3
        && let Some(target) = WEEKDAYS.iter().position(|day| day.starts_with(&value))
    {
        let current = weekday(today).ok_or_else(|| unclear(phrase))?;
        return shift(((target + 6 - current) % 7 + 1) as i64);
    }
    if ISO.is_match(&value) {
        return Ok(value);
    }
    Err(unclear(phrase))
}

fn unclear(phrase: &str) -> String {
    format!(
        "The template date \"{phrase}\" is not understood. Use today, tomorrow, yesterday, +3, -1, a weekday or YYYY-MM-DD."
    )
}

static ID_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*\z").unwrap());

struct Change {
    from: usize,
    to: usize,
    insert: String,
}

fn apply(text: &str, mut changes: Vec<Change>) -> String {
    changes.sort_by_key(|change| std::cmp::Reverse(change.from));
    changes
        .into_iter()
        .fold(text.to_owned(), |mut current, change| {
            current.replace_range(change.from..change.to, &change.insert);
            current
        })
}

fn split_target(target: &str) -> (Option<&str>, &str) {
    match target.find("#^") {
        Some(at) => (Some(&target[..at]), &target[at + 2..]),
        None => (None, target),
    }
}

/// The reference `copyWithNewIds`: every block ID in `text` gets a fresh one from `new_id`, and
/// references to those blocks follow, including `Page#^id` forms that name a source page.
pub fn copy_with_new_ids(
    text: &str,
    parsed: &Parsed,
    destination: &str,
    source_names: &[&str],
    mut new_id: impl FnMut() -> String,
) -> String {
    let mut ids = HashMap::new();
    for block in &parsed.blocks {
        if let Some(id) = &block.id {
            ids.insert(id.clone(), new_id());
        }
    }
    let mut changes = Vec::new();
    for block in &parsed.blocks {
        let Some(id) = &block.id else { continue };
        let content = &text[block.content_from..block.content_to];
        let Some(marker) = ID_MARKER.find(content) else {
            continue;
        };
        let from = block.content_from + marker.start() + marker.as_str().find('^').unwrap_or(0) + 1;
        changes.push(Change {
            from,
            to: from + id.len(),
            insert: ids[id].clone(),
        });
    }
    for reference in &parsed.references {
        if reference.kind == ReferenceKind::Page {
            continue;
        }
        let (page, id) = split_target(&reference.target);
        let Some(fresh) = ids.get(id) else { continue };
        if page.is_some_and(|page| !source_names.contains(&page)) {
            continue;
        }
        let target = match page {
            Some(_) => format!("{}#^{fresh}", page_name(destination)),
            None => fresh.clone(),
        };
        changes.push(Change {
            from: reference.range.start,
            to: reference.range.end,
            insert: text[reference.range.clone()].replacen(&reference.target, &target, 1),
        });
    }
    apply(text, changes)
}

/// A value the user supplies while a template expands.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Question {
    /// `{ask: question}`, with `Value?` when no question is written.
    Ask(String),
    /// `{pick: #tag}`, naming the tag with its `#`.
    Pick(String),
}

impl Question {
    pub fn key(&self) -> String {
        match self {
            Self::Ask(question) => format!("ask:{question}"),
            Self::Pick(tag) => format!("pick:{tag}"),
        }
    }

    pub fn text(&self) -> &str {
        match self {
            Self::Ask(text) | Self::Pick(text) => text,
        }
    }
}

/// When and where a template expands.
pub struct Clock {
    /// Today, as YYYY-MM-DD.
    pub date: String,
    pub hour: u8,
    pub minute: u8,
}

pub struct Context<'a> {
    pub clock: &'a Clock,
    /// The title of the page the template is inserted into.
    pub page_title: &'a str,
    pub days: &'a DailyNames,
    /// Answers by [`Question::key`].
    pub answers: &'a HashMap<String, String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expanded {
    pub text: String,
    /// Where `{cursor}` was, or the end of the trimmed text.
    pub cursor: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Done(Expanded),
    Ask(Question),
}

enum Stop {
    Ask(Question),
    Error(String),
}

static PLACEHOLDER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\{(?i-u:(cursor|today|time|page|weekday|date|ask|pick))(?::([^{}\n]*))?\}")
        .unwrap()
});

fn weekday_name(iso: &str) -> String {
    let name = WEEKDAYS[weekday(iso).unwrap_or(0)];
    name[..1].to_uppercase() + &name[1..]
}

fn fill(text: &str, context: &Context) -> Result<String, Stop> {
    let mut output = String::with_capacity(text.len());
    let mut last = 0;
    for found in PLACEHOLDER.captures_iter(text) {
        let whole = found.get(0).unwrap();
        let name = found.get(1).unwrap().as_str();
        if text[..whole.start()].ends_with('{') || text[whole.end()..].starts_with('}') {
            continue;
        }
        let argument = found
            .get(2)
            .map_or("", |argument| argument.as_str().trim_matches(js_space));
        let day_link = |iso: &str| format!("[[{}]]", context.days.title(iso));
        let value = match name.to_ascii_lowercase().as_str() {
            "cursor" => CURSOR.to_owned(),
            "today" => day_link(&context.clock.date),
            "date" => day_link(&template_date(argument, &context.clock.date).map_err(Stop::Error)?),
            "time" => format!("{:02}:{:02}", context.clock.hour, context.clock.minute),
            "weekday" => weekday_name(&context.clock.date),
            "page" => context.page_title.to_owned(),
            kind => {
                let question = if kind == "pick" {
                    Question::Pick(format!(
                        "#{}",
                        argument.strip_prefix('#').unwrap_or(argument)
                    ))
                } else if argument.is_empty() {
                    Question::Ask("Value?".into())
                } else {
                    Question::Ask(argument.to_owned())
                };
                context
                    .answers
                    .get(&question.key())
                    .cloned()
                    .ok_or(Stop::Ask(question))?
            }
        };
        output.push_str(&text[last..whole.start()]);
        output.push_str(&value);
        last = whole.end();
    }
    output.push_str(&text[last..]);
    Ok(output)
}

/// The reference `blockText`: a list item's continuation lines lose its content indent.
fn block_text(text: &str, block: &Block) -> String {
    if !block.list {
        return block.text.clone();
    }
    let line = text[..block.from].rfind('\n').map_or(0, |at| at + 1);
    let indent = block.content_from - line;
    block
        .text
        .split('\n')
        .enumerate()
        .map(|(index, value)| {
            if index == 0 {
                return value;
            }
            let leading = value.len() - value.trim_start_matches([' ', '\t']).len();
            &value[indent.min(leading)..]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct Renderer<'a> {
    text: &'a str,
    context: &'a Context<'a>,
    referenced: HashSet<&'a str>,
    children: HashMap<Option<&'a str>, Vec<&'a Block>>,
}

impl Renderer<'_> {
    fn render(&self, block: &Block, depth: usize, output: &mut String) -> Result<(), Stop> {
        render_block(self, block, depth, output)
    }
}

/// Writes one block as a bullet at `depth`, then its children.
fn render_block(
    renderer: &Renderer,
    block: &Block,
    depth: usize,
    output: &mut String,
) -> Result<(), Stop> {
    let Renderer {
        text,
        context,
        referenced,
        children,
    } = renderer;
    let indent = "  ".repeat(depth);
    let filled = fill(&block_text(text, block), context)?;
    let lines: Vec<&str> = filled.split('\n').collect();
    let rows: Vec<String> = lines
        .iter()
        .enumerate()
        .map(|(index, line)| format!("{indent}{}{line}", if index == 0 { "- " } else { "  " }))
        .collect();
    output.push_str(&rows.join("\n"));
    if let Some(id) = block.id.as_deref().filter(|id| referenced.contains(id)) {
        if lines.len() > 1 {
            output.push_str(&format!("\n{indent}  "));
        } else if !lines[0].is_empty() {
            output.push(' ');
        }
        output.push('^');
        output.push_str(id);
    }
    output.push('\n');
    for child in children
        .get(&Some(block.key.as_str()))
        .into_iter()
        .flatten()
    {
        renderer.render(child, depth + 1, output)?;
    }
    Ok(())
}

/// Expands a copied template body: every block becomes a bullet at its depth, placeholders are
/// filled, and only IDs that other inserted blocks reference are kept.
pub fn expand(text: &str, parsed: &Parsed, context: &Context) -> Result<Step, String> {
    let mut renderer = Renderer {
        text,
        context,
        referenced: parsed
            .references
            .iter()
            .filter(|reference| reference.kind != ReferenceKind::Page)
            .map(|reference| {
                reference
                    .target
                    .rfind("#^")
                    .map_or(reference.target.as_str(), |at| &reference.target[at + 2..])
            })
            .collect(),
        children: HashMap::new(),
    };
    for block in &parsed.blocks {
        renderer
            .children
            .entry(block.parent.as_deref())
            .or_default()
            .push(block);
    }
    let mut output = String::new();
    for root in renderer.children.get(&None).into_iter().flatten() {
        match renderer.render(root, 0, &mut output) {
            Ok(()) => {}
            Err(Stop::Ask(question)) => return Ok(Step::Ask(question)),
            Err(Stop::Error(error)) => return Err(error),
        }
    }
    let cursor = output.find(CURSOR);
    let text = output.replace(CURSOR, "");
    let cursor = cursor.unwrap_or_else(|| text.trim_end_matches(js_space).len());
    Ok(Step::Done(Expanded { text, cursor }))
}

/// The reference `preserveTemplateRoot`: the first inserted block takes the trigger block's ID,
/// so references to the trigger block still resolve.
pub fn preserve_root(text: &str, id: &str, parsed: &Parsed) -> String {
    let Some(first) = parsed.blocks.first() else {
        return text.to_owned();
    };
    let Some(old) = &first.id else {
        let mut out = text.to_owned();
        out.insert_str(first.content_to, &format!(" ^{id}"));
        return out;
    };
    let needle = format!("^{old}");
    let window = (first.content_to + needle.len()).min(text.len());
    let Some(marker) = text[..window].rfind(&needle) else {
        return text.to_owned();
    };
    let mut out = format!("{}^{id}{}", &text[..marker], &text[marker + needle.len()..]);
    let shift = |offset: usize| {
        if offset > marker {
            offset + id.len() - old.len()
        } else {
            offset
        }
    };
    for reference in parsed.references.iter().rev() {
        let suffix = format!("#^{old}");
        if reference.target != *old && !reference.target.ends_with(&suffix) {
            continue;
        }
        let range = shift(reference.range.start)..shift(reference.range.end);
        let replaced = out[range.clone()].replacen(old.as_str(), id, 1);
        out.replace_range(range, &replaced);
    }
    out
}

/// The smallest single change turning `before` into `after`, on character boundaries.
fn minimal_change(before: &str, after: &str) -> (usize, usize, usize) {
    let from: usize = before
        .chars()
        .zip(after.chars())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    let common: usize = before[from..]
        .chars()
        .rev()
        .zip(after[from..].chars().rev())
        .take_while(|(a, b)| a == b)
        .map(|(a, _)| a.len_utf8())
        .sum();
    (from, before.len() - common, after.len() - common)
}

/// Where a caret before an edit lands after it, keeping to the left of inserted text.
fn map_position(before: &str, after: &str, position: usize) -> usize {
    let (from, to, inserted_to) = minimal_change(before, after);
    if position < from || (from == to && position == from) {
        position
    } else if position < to {
        from
    } else {
        position - to + inserted_to
    }
}

/// The text that replaces the trigger line and the caret offset within it: the expansion with
/// the trigger block's ID kept (`preserved`, when the trigger block had one), indented like the
/// trigger line, without trailing whitespace. Unlike the reference, which trims the space after a
/// final empty bullet and leaves the caret on `-`, whitespace up to the caret stays, so typing at
/// a `{cursor}` on the last line writes into that bullet.
pub fn placement(expanded: &Expanded, preserved: Option<&str>, indent: &str) -> (String, usize) {
    let text = preserved.unwrap_or(&expanded.text);
    let mapped = map_position(&expanded.text, text, expanded.cursor);
    let indented = |value: &str| {
        value
            .split('\n')
            .map(|line| format!("{indent}{line}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let trimmed = text.trim_end_matches(js_space).len();
    let kept = if mapped > trimmed && !text[trimmed..mapped].contains('\n') {
        mapped
    } else {
        trimmed
    };
    let out = indented(&text[..kept]);
    let cursor = indented(&text[..mapped]).len().min(out.len());
    (out, cursor)
}

/// One block tagged with the picked tag: its page and the page links it holds, in order.
pub struct TagUse {
    pub path: String,
    pub title: String,
    pub links: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PastName {
    pub name: String,
    pub uses: usize,
    /// The newest use's daily date, or empty.
    pub last: String,
}

/// The names used with a tag: in each tagged block, the first page link that is not a daily
/// page, else the page's own title when that is not a daily page. Most recently used first,
/// then most used, then by name.
pub fn past_names(uses: &[TagUse], days: &DailyNames) -> Vec<PastName> {
    let mut names: Vec<PastName> = Vec::new();
    let mut keys: HashMap<String, usize> = HashMap::new();
    for usage in uses {
        let page_date = days.date(&usage.title);
        let name = match usage
            .links
            .iter()
            .find(|target| days.date(target).is_none())
        {
            Some(target) => target
                .split(['#', '|'])
                .next()
                .unwrap_or_default()
                .trim_matches(js_space)
                .to_owned(),
            None if page_date.is_some() => String::new(),
            None => usage.title.clone(),
        };
        if name.is_empty() || is_templates_page(&usage.path) {
            continue;
        }
        let last = page_date.unwrap_or_default();
        match keys.get(&name.to_lowercase()) {
            Some(&at) => {
                names[at].uses += 1;
                if last > names[at].last {
                    names[at].last = last;
                }
            }
            None => {
                keys.insert(name.to_lowercase(), names.len());
                names.push(PastName {
                    name,
                    uses: 1,
                    last,
                });
            }
        }
    }
    names.sort_by(|a, b| {
        b.last
            .cmp(&a.last)
            .then(b.uses.cmp(&a.uses))
            .then_with(|| collate(&a.name, &b.name))
    });
    names
}
