//! Roam JSON import and export, ported from the reference `importRoamJSON` and `exportRoamJSON`:
//! Roam pages become Markdown outlines whose blocks keep their Roam UIDs as `^id` markers, the
//! original page and block attributes are kept as notebook metadata, and export rebuilds Roam pages
//! from the notebook's Markdown, reusing that metadata for pages and blocks left unchanged.
//!
//! Markdown analysis stays with the caller through [`RoamMarkdown`], and identifiers come from a
//! caller's generator, so the rules here are deterministic.

use super::js_json::{self, entries, quote};
use crate::markdown::{
    Reference, ReferenceKind, ReferenceSyntax, js_space, page_name, page_title, safe_page_path,
};
use regex::Regex;
use serde_json::{Map, Value};
use std::{
    collections::{HashMap, HashSet},
    sync::LazyLock,
};
use unicode_normalization::UnicodeNormalization as _;

/// Roam blocks nest at most this deep.
const MAX_DEPTH: usize = 128;
/// The largest page an import writes.
const MAX_NOTE_BYTES: usize = 10 * 1024 * 1024;

pub const NOT_ROAM: &str = "This is not a Roam JSON page export. Export the graph as JSON from Roam, then select that file.";
const INVALID_UID: &str = "A Roam UID is invalid. Export the graph again and retry.";
const CHILDREN: &str = "Roam block children must be an array with at most 128 nesting levels. Export the graph again and retry.";
const MALFORMED_TEXT: &str =
    "A Roam block is malformed or missing its text. Export the graph again and retry.";
const NO_TITLE: &str = "A Roam page has no title. Export the graph again and retry.";
const NESTING: &str =
    "Roam block nesting is invalid or exceeds 128 levels. Split the graph and retry.";
const MALFORMED: &str = "A Roam block is malformed. Export the graph again and retry.";
const INVALID_METADATA: &str =
    "The Roam metadata file is invalid. Restore .quire/roam.json from a backup before exporting.";
const INVALID_METADATA_PAGE: &str = "The Roam metadata file contains an invalid page. Restore .quire/roam.json from a backup before exporting.";

/// A block as the notebook's Markdown parser finds it, with byte offsets into the page text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextBlock {
    /// The block's `^id`, or a key unique within the page.
    pub key: String,
    pub id: Option<String>,
    pub parent: Option<String>,
    pub from: usize,
    pub subtree_to: usize,
    /// The block's own text, as the reference `blockText` gives it.
    pub text: String,
}

/// The Markdown analysis Roam exchange reads.
pub trait RoamMarkdown {
    /// References outside code in `text`.
    fn references(&self, text: &str) -> Vec<Reference>;
    /// Every block of a page's text, in document order.
    fn blocks(&self, text: &str) -> Vec<TextBlock>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Document {
    pub path: String,
    pub text: String,
}

/// An imported graph: the pages to write, the report lines, and the metadata to keep.
#[derive(Debug)]
pub struct Imported {
    pub documents: Vec<Document>,
    pub report: Vec<String>,
    /// `{ version: 1, pages: { <path>: <Roam page> } }`.
    pub metadata: Value,
}

#[derive(Debug)]
pub struct Exported {
    pub pages: Vec<Value>,
    pub report: Vec<String>,
}

static MACRO_ALLOWED: [&str; 8] = [
    "pdf",
    "[[pdf]]",
    "embed",
    "[[embed]]",
    "TODO",
    "DONE",
    "[[TODO]]",
    "[[DONE]]",
];
static REMOTE_IMAGE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!\[[^\]]*\]\(https?:").unwrap());
static TITLE_CHARACTERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"[<>:"/\\|?*\p{Cc}]"#).unwrap());
static FRONTMATTER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x{feff}?---\r?\n((?s:.*?))\r?\n---(?:\r?\n|$)").unwrap());
static FENCE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}(`{3,}|~{3,})([^\n\r\u{2028}\u{2029}]*)$").unwrap());
static LIST_MARK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( {0,3})([-+*]|[0-9]+[.)])").unwrap());
static ESCAPED_BULLET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( {0,3})\\([-+*])").unwrap());
static ESCAPED_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^( {0,3}[0-9]+)\\([.)])").unwrap());
static ROAM_UID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap());
static MEDIA: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u)\.(png|jpe?g|gif|webp|svg|avif|pdf|mp3|mp4|wav)$").unwrap()
});
static LOCAL_LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[[^\]]*\]\(([^)]+)\)").unwrap());

fn js_trim(text: &str) -> &str {
    text.trim_matches(js_space)
}

fn js_trim_end(text: &str) -> &str {
    text.trim_end_matches(js_space)
}

/// JavaScript truthiness for a JSON value.
fn truthy(value: Option<&Value>) -> bool {
    match value {
        None | Some(Value::Null) => false,
        Some(Value::Bool(value)) => *value,
        Some(Value::Number(value)) => value.as_f64().is_some_and(|value| value != 0.0),
        Some(Value::String(value)) => !value.is_empty(),
        Some(Value::Array(_) | Value::Object(_)) => true,
    }
}

fn dedupe(report: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    report
        .into_iter()
        .filter(|line| seen.insert(line.clone()))
        .collect()
}

/// The reference `portableTitle`: a file name stem at most 130 UTF-16 units and 200 bytes long.
fn portable_title(original: &str) -> String {
    let normalized: String = original.nfc().collect();
    let cleaned = TITLE_CHARACTERS.replace_all(js_trim(&normalized), "-");
    let mut title = String::new();
    let mut units = 0;
    for character in cleaned.chars() {
        if units + character.len_utf16() > 130 || title.len() + character.len_utf8() > 200 {
            break;
        }
        units += character.len_utf16();
        title.push(character);
    }
    let title = match title.trim_end_matches(['.', ' ']) {
        "" => "Untitled".to_owned(),
        trimmed => trimmed.to_owned(),
    };
    match safe_page_path(&title) {
        Ok(_) => title,
        Err(_) => format!("Page {title}"),
    }
}

/// A Roam heading level, when `heading` is an integer from 1 to 3.
fn heading(node: &Map<String, Value>) -> usize {
    match node.get("heading").and_then(Value::as_f64) {
        Some(level) if level.fract() == 0.0 && (1.0..=3.0).contains(&level) => level as usize,
        _ => 0,
    }
}

/// The reference `listText`: escapes (or with `escape` false, unescapes) list markers that start a
/// line outside fenced code, so a block's own lines never read as nested blocks.
fn list_text(text: &str, escape: bool) -> String {
    let mut fence: Option<String> = None;
    text.split('\n')
        .map(|line| {
            if let Some(delimiter) = FENCE.captures(line) {
                match &fence {
                    None => fence = Some(delimiter[1].to_owned()),
                    Some(open)
                        if delimiter[1].as_bytes()[0] == open.as_bytes()[0]
                            && delimiter[1].len() >= open.len()
                            && js_trim(&delimiter[2]).is_empty() =>
                    {
                        fence = None
                    }
                    Some(_) => {}
                }
                return line.to_owned();
            }
            if fence.is_some() {
                return line.to_owned();
            }
            let followed = |end: usize| line[end..].starts_with([' ', '\t']);
            if escape {
                match LIST_MARK.captures(line) {
                    Some(found) if followed(found.get(0).unwrap().end()) => {
                        let (space, mark) = (&found[1], &found[2]);
                        let escaped = if mark.len() == 1 {
                            format!("\\{mark}")
                        } else {
                            format!("{}\\{}", &mark[..mark.len() - 1], &mark[mark.len() - 1..])
                        };
                        format!("{space}{escaped}{}", &line[found.get(0).unwrap().end()..])
                    }
                    _ => line.to_owned(),
                }
            } else {
                let unescape = |line: String, pattern: &Regex| match pattern.captures(&line) {
                    Some(found) if line[found.get(0).unwrap().end()..].starts_with([' ', '\t']) => {
                        format!(
                            "{}{}{}",
                            &found[1],
                            &found[2],
                            &line[found.get(0).unwrap().end()..]
                        )
                    }
                    _ => line,
                };
                unescape(unescape(line.to_owned(), &ESCAPED_BULLET), &ESCAPED_NUMBER)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The reference `markdownText`: a Roam block's text as Markdown, with its heading and task marks.
fn markdown_text(node: &Map<String, Value>) -> String {
    let level = heading(node);
    let string = node
        .get("string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let task = |text: String, marks: [&str; 2], task: &str| match marks
        .iter()
        .find_map(|mark| text.strip_prefix(mark))
    {
        Some(rest) => format!("{task}{rest}"),
        None => text,
    };
    let string = task(string.to_owned(), ["{{TODO}}", "{{[[TODO]]}}"], "[ ]");
    let string = task(string, ["{{DONE}}", "{{[[DONE]]}}"], "[x]");
    let prefix = if level > 0 {
        format!("{} ", "#".repeat(level))
    } else {
        String::new()
    };
    list_text(&format!("{prefix}{string}"), true)
}

/// The reference `renderNodes`: Roam blocks as a Markdown outline with `^uid` markers.
fn render_nodes(nodes: Option<&Value>, depth: usize) -> Result<String, &'static str> {
    if !truthy(nodes) {
        return Ok(String::new());
    }
    let Some(Value::Array(nodes)) = nodes.filter(|_| depth <= MAX_DEPTH) else {
        return Err(NESTING);
    };
    let mut output = String::new();
    for node in nodes {
        let Some(node) = node.as_object().filter(|node| {
            node.get("string").is_some_and(Value::is_string)
                && node.get("uid").is_some_and(Value::is_string)
        }) else {
            return Err(MALFORMED);
        };
        let indent = "  ".repeat(depth);
        let markdown = markdown_text(node);
        let mut lines: Vec<&str> = markdown.split('\n').collect();
        let last = lines.len() - 1;
        for line in &mut lines[..last] {
            *line = line.strip_suffix('\r').unwrap_or(line);
        }
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                output.push('\n');
            }
            output.push_str(&indent);
            output.push_str(if index > 0 { "  " } else { "- " });
            output.push_str(line);
        }
        if lines.len() > 1 {
            output.push('\n');
            output.push_str(&indent);
            output.push_str("  ");
        } else if !lines[0].is_empty() {
            output.push(' ');
        }
        output.push('^');
        output.push_str(node["uid"].as_str().unwrap_or_default());
        output.push('\n');
        output.push_str(&render_nodes(node.get("children"), depth + 1)?);
    }
    Ok(output)
}

/// Whether a block's text holds a `{{` macro other than the embeds, PDFs, and tasks Quire reads.
fn unsupported_macro(text: &str) -> bool {
    text.match_indices("{{").any(|(index, _)| {
        let rest = &text[index + 2..];
        !MACRO_ALLOWED
            .iter()
            .any(|allowed| rest.starts_with(allowed))
    })
}

/// `haystack.replace(pattern, replacement)` for a string pattern: the first occurrence, with
/// JavaScript's `$$`, `$&`, `` $` `` and `$'` replacement patterns.
fn js_replace_first(haystack: &str, pattern: &str, replacement: &str) -> String {
    let Some(start) = haystack.find(pattern) else {
        return haystack.to_owned();
    };
    let end = start + pattern.len();
    let mut expanded = String::with_capacity(replacement.len());
    let mut rest = replacement;
    while let Some(dollar) = rest.find('$') {
        expanded.push_str(&rest[..dollar]);
        let after = &rest[dollar + 1..];
        let (inserted, skip) = match after.as_bytes().first() {
            Some(b'$') => ("$", 1),
            Some(b'&') => (pattern, 1),
            Some(b'`') => (&haystack[..start], 1),
            Some(b'\'') => (&haystack[end..], 1),
            _ => ("$", 0),
        };
        expanded.push_str(inserted);
        rest = &after[skip..];
    }
    expanded.push_str(rest);
    format!("{}{expanded}{}", &haystack[..start], &haystack[end..])
}

/// Every block text a Roam import reads references in: the texts holding `((`, the only ones whose
/// Roam block references it rewrites. Callers parse these before importing.
pub fn reference_texts(pages: &Value) -> Vec<String> {
    let mut texts = Vec::new();
    let mut pending: Vec<&Value> = pages
        .as_array()
        .map(|pages| {
            pages
                .iter()
                .filter_map(|page| page.get("children"))
                .collect()
        })
        .unwrap_or_default();
    while let Some(children) = pending.pop() {
        for node in children.as_array().into_iter().flatten() {
            if let Some(text) = node.get("string").and_then(Value::as_str)
                && text.contains("((")
            {
                texts.push(text.to_owned());
            }
            if let Some(children) = node.get("children") {
                pending.push(children);
            }
        }
    }
    texts
}

struct Registry {
    ids: HashMap<String, String>,
    assigned: Vec<String>,
    report: Vec<String>,
    blocks: usize,
}

impl Registry {
    fn register(
        &mut self,
        node: &Map<String, Value>,
        new_id: &mut dyn FnMut() -> String,
    ) -> Result<(), String> {
        match node.get("uid") {
            None => self.assigned.push(new_id()),
            Some(Value::String(uid)) if !uid.is_empty() => {
                if self.ids.contains_key(uid) {
                    return Err(format!(
                        "Roam export has duplicate ID {uid}. Fix the export before importing."
                    ));
                }
                let local = if ROAM_UID.is_match(uid) {
                    uid.clone()
                } else {
                    new_id()
                };
                if &local != uid {
                    self.report.push(format!(
                        "Remapped block ID {uid} to {local} for Markdown compatibility."
                    ));
                }
                self.ids.insert(uid.clone(), local.clone());
                self.assigned.push(local);
            }
            Some(_) => return Err(INVALID_UID.to_owned()),
        }
        Ok(())
    }

    fn walk(
        &mut self,
        nodes: Option<&Value>,
        depth: usize,
        new_id: &mut dyn FnMut() -> String,
    ) -> Result<(), String> {
        let Some(nodes) = nodes else {
            return Ok(());
        };
        let Some(nodes) = nodes.as_array().filter(|_| depth <= MAX_DEPTH) else {
            return Err(CHILDREN.to_owned());
        };
        for node in nodes {
            let Some(node) = node
                .as_object()
                .filter(|node| node.get("string").is_some_and(Value::is_string))
            else {
                return Err(MALFORMED_TEXT.to_owned());
            };
            self.register(node, new_id)?;
            self.blocks += 1;
            self.walk(node.get("children"), depth + 1, new_id)?;
        }
        Ok(())
    }

    /// The reference import `rewrite`: Roam block references point at their local identifiers.
    fn rewrite(&mut self, text: &str, markdown: &dyn RoamMarkdown) -> String {
        let mut text = text.to_owned();
        for reference in markdown.references(&text).into_iter().rev() {
            if reference.kind == ReferenceKind::Page || reference.syntax != ReferenceSyntax::Roam {
                continue;
            }
            let id = self.ids.get(&reference.target).cloned();
            if id.is_none() {
                self.report
                    .push(format!("Unresolved reference: {}", reference.target));
            }
            let replaced = js_replace_first(
                &text[reference.range.clone()],
                &reference.target,
                id.as_deref().unwrap_or(&reference.target),
            );
            text.replace_range(reference.range.clone(), &replaced);
        }
        text
    }

    /// The reference `normalize`, consuming identifiers in registration order.
    fn normalize(
        &mut self,
        nodes: &mut [Value],
        next: &mut std::vec::IntoIter<String>,
        markdown: &dyn RoamMarkdown,
    ) {
        for node in nodes {
            let Some(node) = node.as_object_mut() else {
                continue;
            };
            let string = node["string"].as_str().unwrap_or_default().to_owned();
            let named = match node.get("uid").and_then(Value::as_str) {
                Some(uid) => uid.to_owned(),
                None => "a block".to_owned(),
            };
            if unsupported_macro(&string) {
                self.report
                    .push(format!("Preserved an unsupported Roam macro in {named}."));
            }
            if REMOTE_IMAGE.is_match(&string) {
                self.report.push(format!(
                    "Remote image in {named} still requires its original URL."
                ));
            }
            let uid = next.next().unwrap_or_default();
            node.insert("uid".into(), Value::String(uid));
            let rewritten = self.rewrite(&string, markdown);
            node.insert("string".into(), Value::String(rewritten));
            if let Some(Value::Array(children)) = node.get_mut("children") {
                self.normalize(children, next, markdown);
            }
        }
    }
}

/// The reference `importRoamJSON`: Roam pages as Markdown documents, with a report and the
/// metadata that lets an export restore what Markdown cannot hold.
pub fn import_roam_json(
    value: Value,
    markdown: &dyn RoamMarkdown,
    new_id: &mut dyn FnMut() -> String,
) -> Result<Imported, String> {
    let Value::Array(mut pages) = value else {
        return Err(NOT_ROAM.to_owned());
    };
    let mut registry = Registry {
        ids: HashMap::new(),
        assigned: Vec::new(),
        report: Vec::new(),
        blocks: 0,
    };
    let mut titles = HashSet::new();
    for page in &pages {
        let Some(page) = page.as_object().filter(|page| {
            page.get("title")
                .and_then(Value::as_str)
                .is_some_and(|title| !js_trim(title).is_empty())
        }) else {
            return Err(NO_TITLE.to_owned());
        };
        let title = page["title"].as_str().unwrap_or_default();
        if !titles.insert(title.to_owned()) {
            return Err(format!(
                "Roam export repeats page title {title}. Resolve the duplicate before importing."
            ));
        }
        if page.contains_key("uid") {
            registry.register(page, new_id)?;
        }
        registry.walk(page.get("children"), 0, new_id)?;
    }
    let mut next = std::mem::take(&mut registry.assigned).into_iter();
    let mut filenames = HashSet::new();
    let mut documents = Vec::with_capacity(pages.len());
    let mut metadata = Map::new();
    for page in &mut pages {
        let source = page.as_object_mut().expect("validated Roam page");
        if source.contains_key("uid") {
            let uid = next.next().unwrap_or_default();
            source.insert("uid".into(), Value::String(uid));
        }
        if let Some(Value::Array(children)) = source.get_mut("children") {
            registry.normalize(children, &mut next, markdown);
        }
        let original = source["title"].as_str().unwrap_or_default().to_owned();
        let title = portable_title(&original);
        let mut candidate = title.clone();
        let mut index = 2;
        let path = loop {
            let path = safe_page_path(&candidate).map_err(|error| error.to_string())?;
            if filenames.insert(path.to_lowercase()) {
                break path;
            }
            candidate = format!("{title} {index}");
            index += 1;
        };
        if path != format!("{original}.md") {
            registry.report.push(format!(
                "Stored {original} as {path}; retained the original title in front matter."
            ));
        }
        let text = format!(
            "---\ntitle: {}\n---\n\n{}",
            quote(&original),
            render_nodes(source.get("children"), 0)?
        );
        if text.len() > MAX_NOTE_BYTES {
            return Err(format!(
                "{original} exceeds the 10 MB note limit. Split that page in Roam and export again."
            ));
        }
        documents.push(Document {
            path: path.clone(),
            text,
        });
        metadata.insert(path, std::mem::take(page));
    }
    let mut report = vec![
        format!(
            "Imported {} pages and {} blocks. Original export unchanged.",
            documents.len(),
            registry.blocks
        ),
        "Original Roam page and block metadata are retained in .quire/roam.json. Executable macros are preserved as text; remote attachments are not downloaded.".to_owned(),
    ];
    report.append(&mut registry.report);
    let mut root = Map::new();
    root.insert("version".into(), Value::from(1));
    root.insert("pages".into(), Value::Object(metadata));
    Ok(Imported {
        documents,
        report: dedupe(report),
        metadata: Value::Object(root),
    })
}

/// A Roam page and the attributes of its blocks, checked as the reference does before export.
fn valid_page(page: &Value) -> bool {
    page.get("title").is_some_and(Value::is_string)
        && page.is_object()
        && render_nodes(page.get("children"), 0).is_ok()
}

struct Page<'a> {
    path: &'a str,
    text: String,
    title: String,
    source: Option<&'a Map<String, Value>>,
    unchanged: bool,
    blocks: Vec<TextBlock>,
    ids: HashMap<String, String>,
}

struct Identity {
    uid: String,
    path: String,
}

/// A heading prefix: one to three `#` and a space, and its length in bytes.
fn heading_prefix(text: &str) -> Option<(usize, usize)> {
    let hashes = text.len() - text.trim_start_matches('#').len();
    let space = text[hashes..].chars().next().filter(|c| js_space(*c))?;
    (1..=3)
        .contains(&hashes)
        .then_some((hashes, hashes + space.len_utf8()))
}

/// Replaces a leading `[ ]` or `[x]` task box with Roam's task macro.
fn roam_task(text: &str) -> String {
    let boxed = |mark: &str| {
        text.strip_prefix(mark)
            .filter(|rest| rest.chars().next().is_none_or(js_space))
    };
    if let Some(rest) = boxed("[ ]") {
        return format!("{{{{[[TODO]]}}}}{rest}");
    }
    for mark in ["[x]", "[X]"] {
        if let Some(rest) = boxed(mark) {
            return format!("{{{{[[DONE]]}}}}{rest}");
        }
    }
    text.to_owned()
}

/// Whether text holds a Markdown link or image to a local file or relative path.
fn local_link(text: &str) -> bool {
    text.match_indices('[').any(|(index, _)| {
        LOCAL_LINK.captures(&text[index..]).is_some_and(|found| {
            !["http:", "https:", "mailto:", "data:", "#"]
                .iter()
                .any(|prefix| found[1].starts_with(prefix))
        })
    })
}

struct Exporter<'a> {
    pages: Vec<Page<'a>>,
    identity: HashMap<String, Identity>,
    used: HashSet<String>,
    report: Vec<String>,
    generated: usize,
    markdown: &'a dyn RoamMarkdown,
}

impl Exporter<'_> {
    fn register(&mut self, uid: &str, path: &str) -> Result<String, String> {
        if !self.used.insert(uid.to_owned()) {
            return Err(format!(
                "Duplicate block or page ID {uid} prevents a reliable Roam export. Give each block a unique identifier in Source."
            ));
        }
        self.identity.insert(
            uid.to_owned(),
            Identity {
                uid: uid.to_owned(),
                path: path.to_owned(),
            },
        );
        Ok(uid.to_owned())
    }

    fn find_page(&self, target: &str) -> Option<&Page<'_>> {
        let bare = page_name(target);
        let lower = bare.to_lowercase();
        self.pages
            .iter()
            .find(|page| page.title == target)
            .or_else(|| self.pages.iter().find(|page| page_name(page.path) == bare))
            .or_else(|| {
                self.pages.iter().find(|page| {
                    [page_name(page.path), page.title.as_str()]
                        .iter()
                        .any(|name| name.to_lowercase() == lower)
                })
            })
    }

    /// The reference export `rewrite`: Quire links and embeds in Roam's forms.
    fn rewrite(&mut self, text: &str, path: &str) -> String {
        let mut text = text.to_owned();
        let mut report = Vec::new();
        for reference in self.markdown.references(&text).into_iter().rev() {
            if reference.media.is_some() {
                continue;
            }
            let replacement =
                match reference.kind {
                    ReferenceKind::Page => self.find_page(&reference.target).map(|page| {
                        match reference.label.as_deref().filter(|label| !label.is_empty()) {
                            Some(label) => format!("[{label}]([[{}]])", page.title),
                            None => format!("[[{}]]", page.title),
                        }
                    }),
                    ReferenceKind::Embed
                        if reference.syntax == ReferenceSyntax::Wiki
                            && !reference.target.contains("#^") =>
                    {
                        match self.find_page(&reference.target) {
                            Some(page) => Some(format!("{{{{[[embed]]: [[{}]]}}}}", page.title)),
                            None if MEDIA.is_match(&reference.target) => {
                                Some(format!("![]({})", reference.target))
                            }
                            None => {
                                report.push(format!(
                                    "Unresolved page embed in {path}: {}",
                                    reference.target
                                ));
                                None
                            }
                        }
                    }
                    _ => {
                        let (page, id) = match reference.target.split_once("#^") {
                            Some((page, id)) => (Some(page), id),
                            None => (None, reference.target.as_str()),
                        };
                        match self.identity.get(id).filter(|target| {
                            page.is_none_or(|page| {
                                self.find_page(page)
                                    .is_some_and(|found| found.path == target.path)
                            })
                        }) {
                            Some(target) if reference.kind == ReferenceKind::Embed => {
                                Some(format!("{{{{[[embed]]: (({}))}}}}", target.uid))
                            }
                            Some(target) => Some(format!("(({}))", target.uid)),
                            None => {
                                report.push(format!(
                                    "Unresolved reference in {path}: {}",
                                    reference.target
                                ));
                                None
                            }
                        }
                    }
                };
            if let Some(replacement) = replacement.filter(|replacement| !replacement.is_empty()) {
                text.replace_range(reference.range.clone(), &replacement);
            }
        }
        if local_link(&text) {
            report.push(format!(
                "Local attachment or relative link in {path} remains a dependency. Copy its files separately; JSON does not contain attachment bytes."
            ));
        }
        self.report.append(&mut report);
        text
    }

    fn copy_original(&mut self, nodes: &[Value], path: &str) -> Vec<Value> {
        nodes
            .iter()
            .map(|node| {
                let mut result = node.as_object().cloned().unwrap_or_default();
                let string = result
                    .get("string")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                result.insert("string".into(), Value::String(self.rewrite(&string, path)));
                if truthy(node.get("children"))
                    && let Some(children) = node.get("children").and_then(Value::as_array)
                {
                    let copied = self.copy_original(children, path);
                    result.insert("children".into(), Value::Array(copied));
                }
                Value::Object(result)
            })
            .collect()
    }
}

/// The reference `collect`: every metadata block by UID, registered when its page is unchanged.
fn collect<'a>(
    nodes: Option<&'a Value>,
    page: &Page<'_>,
    exporter: &mut Exporter<'_>,
    original_nodes: &mut HashMap<String, &'a Map<String, Value>>,
) -> Result<(), String> {
    for node in nodes.and_then(Value::as_array).into_iter().flatten() {
        let Some(node) = node.as_object() else {
            continue;
        };
        let uid = node["uid"].as_str().unwrap_or_default();
        original_nodes.insert(uid.to_owned(), node);
        if page.unchanged {
            exporter.register(uid, page.path)?;
        }
        collect(node.get("children"), page, exporter, original_nodes)?;
    }
    Ok(())
}

/// One built node of an exported page, with its children by index.
struct Built {
    node: Map<String, Value>,
    children: Vec<usize>,
}

fn assemble(index: usize, built: &mut Vec<Option<Built>>) -> Value {
    let Built { mut node, children } = built[index].take().expect("each node is placed once");
    if !children.is_empty() {
        let children = children
            .into_iter()
            .map(|child| assemble(child, built))
            .collect();
        node.insert("children".into(), Value::Array(children));
    }
    Value::Object(node)
}

/// The metadata an export reads, or None when the file holds a value JavaScript treats as absent.
fn metadata_pages(metadata: Option<&Value>) -> Result<Option<&Map<String, Value>>, String> {
    let Some(metadata) = metadata.filter(|metadata| truthy(Some(metadata))) else {
        return Ok(None);
    };
    let pages = metadata
        .get("version")
        .and_then(Value::as_f64)
        .filter(|version| *version == 1.0)
        .and_then(|_| metadata.get("pages"))
        .and_then(Value::as_object)
        .ok_or_else(|| INVALID_METADATA.to_owned())?;
    if !pages.values().all(valid_page) {
        return Err(INVALID_METADATA_PAGE.to_owned());
    }
    Ok(Some(pages))
}

/// The reference `exportRoamJSON`: every document as a Roam page. Pages and blocks whose Markdown
/// still matches the imported metadata keep their Roam attributes.
pub fn export_roam_json(
    documents: &[Document],
    metadata: Option<&Value>,
    markdown: &dyn RoamMarkdown,
    new_id: &mut dyn FnMut() -> String,
) -> Result<Exported, String> {
    let sources = metadata_pages(metadata)?;
    let mut titles = HashSet::new();
    let mut pages = Vec::with_capacity(documents.len());
    for document in documents {
        let text = document.text.replace("\r\n", "\n");
        let title = page_title(&document.path, &text).to_lossy_string();
        if !titles.insert(title.clone()) {
            return Err(format!(
                "Multiple pages are titled {title}. Give them distinct titles before exporting to Roam."
            ));
        }
        let source = sources.and_then(|sources| {
            sources.get(&document.path).or_else(|| {
                entries(sources)
                    .into_iter()
                    .map(|(_, page)| page)
                    .find(|page| page.get("title").and_then(Value::as_str) == Some(&title))
            })
        });
        let source = source.and_then(Value::as_object);
        let frontmatter = FRONTMATTER.find(&text).map_or(0, |found| found.end());
        let body = &text[frontmatter..];
        let body = body.strip_prefix('\n').unwrap_or(body);
        let unchanged = source.is_some_and(|source| {
            render_nodes(source.get("children"), 0).is_ok_and(|rendered| rendered == body)
        });
        let blocks = markdown.blocks(&text);
        pages.push(Page {
            path: &document.path,
            text,
            title,
            source,
            unchanged,
            blocks,
            ids: HashMap::new(),
        });
    }
    let mut exporter = Exporter {
        pages: Vec::new(),
        identity: HashMap::new(),
        used: HashSet::new(),
        report: Vec::new(),
        generated: 0,
        markdown,
    };
    let mut original_nodes: HashMap<String, &Map<String, Value>> = HashMap::new();
    for page in &mut pages {
        if let Some(uid) = page
            .source
            .and_then(|source| source.get("uid"))
            .and_then(Value::as_str)
        {
            if !exporter.used.insert(uid.to_owned()) {
                return Err(format!(
                    "Duplicate page ID {uid}. Resolve it before exporting."
                ));
            }
            exporter.identity.insert(
                uid.to_owned(),
                Identity {
                    uid: uid.to_owned(),
                    path: page.path.to_owned(),
                },
            );
        }
        collect(
            page.source.and_then(|source| source.get("children")),
            page,
            &mut exporter,
            &mut original_nodes,
        )?;
        if !page.unchanged {
            for block in &page.blocks {
                let uid = match &block.id {
                    Some(id) => id.clone(),
                    None => {
                        exporter.generated += 1;
                        new_id()
                    }
                };
                let uid = exporter.register(&uid, page.path)?;
                page.ids.insert(block.key.clone(), uid);
            }
        }
    }
    exporter.pages = pages;
    let mut output = Vec::with_capacity(exporter.pages.len());
    for index in 0..exporter.pages.len() {
        let page = &exporter.pages[index];
        let (path, title, source) = (page.path.to_owned(), page.title.clone(), page.source);
        if page.unchanged {
            let source = source.expect("an unchanged page has metadata");
            let mut result = source.clone();
            result.insert("title".into(), Value::String(title));
            if truthy(source.get("children"))
                && let Some(children) = source.get("children").and_then(Value::as_array)
            {
                let copied = exporter.copy_original(children, &path);
                result.insert("children".into(), Value::Array(copied));
            }
            output.push(Value::Object(result));
            continue;
        }
        let text = page.text.clone();
        let blocks = page.blocks.clone();
        let ids = page.ids.clone();
        let frontmatter = FRONTMATTER.captures(&text);
        let mut cursor = frontmatter.as_ref().map_or(0, |found| found[0].len());
        let mut built: Vec<Option<Built>> = Vec::new();
        let mut roots: Vec<usize> = Vec::new();
        let mut keys: HashMap<String, usize> = HashMap::new();
        let mut gap = |exporter: &mut Exporter,
                       cursor: usize,
                       to: usize,
                       built: &mut Vec<Option<Built>>,
                       roots: &mut Vec<usize>| {
            let between = js_trim(&text[cursor.min(to)..to]);
            if !between.is_empty() {
                let uid = new_id();
                exporter.generated += 1;
                let mut node = Map::new();
                node.insert("uid".into(), Value::String(uid));
                node.insert(
                    "string".into(),
                    Value::String(exporter.rewrite(between, &path)),
                );
                built.push(Some(Built {
                    node,
                    children: Vec::new(),
                }));
                roots.push(built.len() - 1);
            }
        };
        for block in &blocks {
            if block.parent.is_none() {
                gap(&mut exporter, cursor, block.from, &mut built, &mut roots);
                cursor = block.subtree_to;
            }
            let uid = ids.get(&block.key).cloned().unwrap_or_default();
            let original = original_nodes.get(&uid).copied();
            let content = &block.text;
            let unchanged = original.is_some_and(|original| {
                js_trim_end(content) == js_trim_end(&markdown_text(original))
            });
            let mut attributes = original.cloned().unwrap_or_default();
            for key in ["children", "string", "uid", "heading"] {
                attributes.shift_remove(key);
            }
            if !unchanged {
                for key in ["edit-time", "edit-user", "edit-email"] {
                    attributes.shift_remove(key);
                }
            }
            let level = heading_prefix(content);
            let current = if original.is_some() {
                list_text(content, false)
            } else {
                content.clone()
            };
            let string = match (unchanged, original) {
                (true, Some(original)) => original
                    .get("string")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                _ => roam_task(match heading_prefix(&current) {
                    Some((_, length)) => &current[length..],
                    None => &current,
                }),
            };
            let mut node = attributes;
            node.insert("uid".into(), Value::String(uid));
            node.insert(
                "string".into(),
                Value::String(exporter.rewrite(&string, &path)),
            );
            match (
                unchanged,
                original.and_then(|original| original.get("heading")),
            ) {
                (true, Some(original)) => {
                    node.insert("heading".into(), original.clone());
                }
                _ => {
                    if let Some((hashes, _)) = level {
                        node.insert("heading".into(), Value::from(hashes));
                    }
                }
            }
            built.push(Some(Built {
                node,
                children: Vec::new(),
            }));
            let placed = built.len() - 1;
            keys.insert(block.key.clone(), placed);
            match block.parent.as_ref().and_then(|parent| keys.get(parent)) {
                Some(&parent) => built[parent]
                    .as_mut()
                    .expect("parents precede children")
                    .children
                    .push(placed),
                None => roots.push(placed),
            }
        }
        gap(&mut exporter, cursor, text.len(), &mut built, &mut roots);
        let custom = frontmatter.as_ref().map(|found| found[1].to_owned());
        let mut children: Vec<Value> = roots
            .into_iter()
            .map(|root| assemble(root, &mut built))
            .collect();
        if source.is_none()
            && let Some(custom) = custom.filter(|custom| {
                custom
                    .split('\n')
                    .any(|line| !js_trim(line).is_empty() && !line.starts_with("title:"))
            })
        {
            let mut node = Map::new();
            node.insert("uid".into(), Value::String(new_id()));
            node.insert(
                "string".into(),
                Value::String(format!("```yaml\n{custom}\n```")),
            );
            children.insert(0, Value::Object(node));
            exporter.generated += 1;
            exporter.report.push(format!(
                "Preserved custom Markdown front matter from {path} as a YAML code block."
            ));
        }
        let mut result = source.cloned().unwrap_or_default();
        result.insert("title".into(), Value::String(title));
        result.insert("children".into(), Value::Array(children));
        output.push(Value::Object(result));
    }
    let mut report = vec![format!(
        "Exported {} pages to Roam JSON. Existing block IDs are preserved; {} new export-only IDs were generated.",
        output.len(),
        exporter.generated
    )];
    report.append(&mut exporter.report);
    Ok(Exported {
        pages: output,
        report: dedupe(report),
    })
}

/// The JSON file an export writes: `JSON.stringify(pages, null, 2)` and a final newline.
pub fn export_file_text(pages: Vec<Value>) -> String {
    js_json::stringify_pretty(&Value::Array(pages)) + "\n"
}
