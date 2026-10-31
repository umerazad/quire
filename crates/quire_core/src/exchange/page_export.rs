//! A page as standalone Markdown for a document outside the notebook, ported from the reference
//! `exportMarkdown`: links read as their text, block references and embeds become what they
//! point to, and block IDs and created-date stamps are dropped. Local images become
//! `quire-vault:` links holding their notebook path, which [`localize_attachments`] turns into
//! checked file paths.

use super::uri::{decode_uri, decode_uri_component, encode_uri_component};
use crate::markdown::{JS_SPACE, Reference, ReferenceKind, js_space, relative_vault_path};
use regex::Regex;
use std::{collections::HashMap, sync::LazyLock};

pub const VAULT_SCHEME: &str = "quire-vault:";

/// Embeds and references nest at most this deep, which also stops cycles.
const MAX_DEPTH: usize = 3;

/// A block found by its reference target: its page and its byte offsets in that page's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FoundBlock {
    pub path: String,
    pub from: usize,
    pub content_from: usize,
    pub subtree_to: usize,
    /// The block's own text, as the block parser reports it.
    pub text: String,
}

/// What exporting reads from the notebook.
pub trait ExportNotebook {
    /// A page's editor text, or empty when the notebook has no such page.
    fn text(&self, path: &str) -> String;
    fn title(&self, path: &str) -> String;
    fn find_page(&self, target: &str) -> Option<String>;
    /// The single block a reference target names; None when missing or ambiguous.
    fn resolve(&self, target: &str) -> Option<FoundBlock>;
    /// References outside code, as the notebook's Markdown analysis finds them.
    fn references(&self, text: &str) -> Vec<Reference>;
}

/// Exporting stopped on a local image whose address has a malformed `%` escape.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MalformedLink {
    pub path: String,
    pub source: String,
}

impl std::fmt::Display for MalformedLink {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "The image link {} in {} has a malformed % escape. Fix the link in source view and export again.",
            self.source, self.path
        )
    }
}

impl std::error::Error for MalformedLink {}

static ID_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)[ \t]+\^[A-Za-z0-9_-]+[ \t]*$").unwrap());
static CREATED_STAMP: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ \t]*➕[ \t]*(?:\[\[[^\]\n]+\]\]|[0-9]{4}-[0-9]{2}-[0-9]{2})").unwrap()
});
static FILE_TARGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.[A-Za-z0-9]{2,5}$").unwrap());
static MARKDOWN_TARGET: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\.[mM][dD]$").unwrap());
static MARKDOWN_IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    let space = JS_SPACE;
    let inner = &JS_SPACE[1..JS_SPACE.len() - 1];
    Regex::new(&format!(
        r#"!\[([^\]\n]*)\]\(<?([^)>{inner}]+)>?({space}+"[^"]*")?\)"#
    ))
    .unwrap()
});
static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").unwrap());
static LIST_MARKER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[ \t]*(?:(?:[-+*]|[0-9]+[.)])[ \t]+)?").unwrap());
static PDF: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u:\.pdf)(?:[?#]|$)|^(?i-u:data:application/pdf)[;,]").unwrap()
});
static IMAGE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\.(?i-u:png|jpe?g|gif|webp|svg|avif)(?:[?#]|$)|^(?i-u:data:image/)").unwrap()
});
static VAULT_LINK: LazyLock<Regex> = LazyLock::new(|| {
    let inner = &JS_SPACE[1..JS_SPACE.len() - 1];
    Regex::new(&format!(r"\]\(quire-vault:([^){inner}]+)\)")).unwrap()
});

/// The reference `attachmentKind`: whether a link names an image or a PDF.
pub fn is_attachment(value: &str) -> bool {
    let decoded = decode_uri_component(value);
    let decoded = decoded.as_deref().unwrap_or(value);
    PDF.is_match(decoded) || IMAGE.is_match(decoded)
}

/// The reference `fileTarget`: a link ending in a file extension other than `.md`.
fn is_file_target(target: &str) -> bool {
    FILE_TARGET
        .find(target)
        .is_some_and(|found| !MARKDOWN_TARGET.is_match(&target[found.start()..]))
}

fn vault_link(from_path: &str, target: &str) -> Option<String> {
    let file = target.split(['?', '#']).next().unwrap_or_default();
    relative_vault_path(Some(from_path), file)
        .filter(|relative| !relative.is_empty())
        .map(|relative| format!("{VAULT_SCHEME}{}", encode_uri_component(&relative)))
}

fn local_images(text: &str, from_path: &str) -> Result<String, MalformedLink> {
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for captures in MARKDOWN_IMAGE.captures_iter(text) {
        let whole = captures.get(0).unwrap();
        let alt = &captures[1];
        let source = &captures[2];
        result.push_str(&text[last..whole.start()]);
        last = whole.end();
        if SCHEME.is_match(source) || source.starts_with("//") {
            result.push_str(whole.as_str());
            continue;
        }
        let decoded = decode_uri(source).ok_or_else(|| MalformedLink {
            path: from_path.to_owned(),
            source: source.to_owned(),
        })?;
        match vault_link(from_path, &decoded) {
            Some(link) => result.push_str(&format!("![{alt}]({link})")),
            None => result.push_str(whole.as_str()),
        }
    }
    result.push_str(&text[last..]);
    Ok(result)
}

fn clean(text: &str) -> String {
    let text = ID_MARKER.replace_all(text, "");
    CREATED_STAMP.replace_all(&text, "").into_owned()
}

fn line_start(text: &str, offset: usize) -> usize {
    text[..offset].rfind('\n').map_or(0, |index| index + 1)
}

/// The rest of `line` after its first `units` UTF-16 code units.
fn skip_units(line: &str, units: usize) -> &str {
    let mut seen = 0;
    for (index, character) in line.char_indices() {
        if seen >= units {
            return &line[index..];
        }
        seen += character.len_utf16();
    }
    ""
}

fn units(text: &str) -> usize {
    text.encode_utf16().count()
}

/// A referenced block and its children as Markdown, with the children re-indented to sit under
/// the column the embed was written at.
fn block_markdown(
    notebook: &impl ExportNotebook,
    target: &str,
    column: usize,
) -> Option<(String, String)> {
    let found = notebook.resolve(target)?;
    let text = notebook.text(&found.path);
    let indent = units(&text[line_start(&text, found.from)..found.from]);
    let body = text.get(found.content_from..found.subtree_to)?;
    let mut lines = body.split('\n');
    let first = lines.next().unwrap_or_default().to_owned();
    let markdown = std::iter::once(first)
        .chain(lines.map(|line| {
            if line.trim_matches(js_space).is_empty() {
                line.to_owned()
            } else {
                format!("{}{}", " ".repeat(column), skip_units(line, indent))
            }
        }))
        .collect::<Vec<_>>()
        .join("\n");
    Some((found.path, markdown))
}

struct Exporter<'a, N> {
    notebook: &'a N,
}

impl<N: ExportNotebook> Exporter<'_, N> {
    fn replacement(
        &self,
        text: &str,
        reference: &Reference,
        depth: usize,
        from_path: &str,
    ) -> Result<String, MalformedLink> {
        let original = &text[reference.range.clone()];
        let label_or_target = || {
            reference
                .label
                .clone()
                .unwrap_or_else(|| reference.target.clone())
        };
        match reference.kind {
            ReferenceKind::Page => return Ok(label_or_target()),
            ReferenceKind::Block => {
                if let Some(label) = &reference.label {
                    return Ok(label.clone());
                }
                return match self.notebook.resolve(&reference.target) {
                    None => Ok(original.to_owned()),
                    Some(found) => self.export_text(
                        clean(&found.text).trim_matches(js_space),
                        depth + 1,
                        &found.path,
                    ),
                };
            }
            ReferenceKind::Embed => {}
        }
        if reference.media.is_some() || is_attachment(&reference.target) {
            return Ok(match vault_link(from_path, &reference.target) {
                Some(link) => {
                    let label = reference.label.clone().unwrap_or_else(|| {
                        reference
                            .target
                            .rsplit('/')
                            .next()
                            .unwrap_or_default()
                            .to_owned()
                    });
                    format!("![{label}]({link})")
                }
                None => original.to_owned(),
            });
        }
        if is_file_target(&reference.target) {
            return Ok(original.to_owned());
        }
        let start = line_start(text, reference.range.start);
        let marker = LIST_MARKER
            .find(&text[start..reference.range.start])
            .map_or(0, |found| found.len());
        let column = if marker > 0 {
            text[start..]
                .char_indices()
                .find(|(_, character)| !js_space(*character))
                .map_or(0, |(index, _)| units(&text[start..start + index]))
        } else {
            0
        };
        if let Some((path, markdown)) = block_markdown(self.notebook, &reference.target, column) {
            return self.export_text(&markdown, depth + 1, &path);
        }
        let Some(page) = self.notebook.find_page(&reference.target) else {
            return Ok(label_or_target());
        };
        if depth >= MAX_DEPTH {
            return Ok(label_or_target());
        }
        let body = self.export_text(&self.notebook.text(&page), depth + 1, &page)?;
        let body = body.trim_end_matches(js_space);
        // Inside a list item, the page becomes an item titled with its name and its content
        // nested below.
        Ok(if marker > 0 {
            std::iter::once(self.notebook.title(&page))
                .chain(body.split('\n').map(|line| {
                    if line.trim_matches(js_space).is_empty() {
                        line.to_owned()
                    } else {
                        format!("{}{line}", " ".repeat(column + 2))
                    }
                }))
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            body.to_owned()
        })
    }

    fn export_text(
        &self,
        text: &str,
        depth: usize,
        from_path: &str,
    ) -> Result<String, MalformedLink> {
        if depth > MAX_DEPTH {
            return Ok(clean(text));
        }
        let mut references = self.notebook.references(text);
        references.sort_by_key(|reference| reference.range.start);
        let mut result = String::with_capacity(text.len());
        let mut last = 0;
        for reference in &references {
            if reference.range.start < last {
                continue;
            }
            result.push_str(&text[last..reference.range.start]);
            result.push_str(&self.replacement(text, reference, depth, from_path)?);
            last = reference.range.end;
        }
        result.push_str(&text[last..]);
        local_images(&clean(&result), from_path)
    }
}

/// The page at `path` as standalone Markdown.
pub fn export_markdown(
    notebook: &impl ExportNotebook,
    path: &str,
) -> Result<String, MalformedLink> {
    Exporter { notebook }.export_text(&notebook.text(path), 0, path)
}

/// The reference `localizeAttachments`: each `quire-vault:` link becomes `<file>` when `file`
/// finds an ordinary file inside the notebook for its decoded notebook path, else the decoded
/// notebook path as plain text. `file` runs once per distinct link.
pub fn localize_attachments(text: &str, mut file: impl FnMut(&str) -> Option<String>) -> String {
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    VAULT_LINK
        .replace_all(text, |captures: &regex::Captures| {
            let encoded = captures.get(1).unwrap().as_str();
            let decoded = decode_uri_component(encoded);
            let found = files
                .entry(encoded.to_owned())
                .or_insert_with(|| decoded.as_deref().and_then(&mut file));
            match (found, decoded) {
                (Some(path), _) => format!("](<{path}>)"),
                (None, Some(decoded)) => format!("]({decoded})"),
                (None, None) => format!("]({encoded})"),
            }
        })
        .into_owned()
}

/// The reference `exportName`: a file name for the save dialog from a page title.
pub fn export_name(title: &str, extension: &str) -> String {
    let name: String = title
        .chars()
        .map(|character| {
            if "\\/:*?\"<>|\n\r\t".contains(character) {
                '-'
            } else {
                character
            }
        })
        .collect();
    let name = name.trim_matches(js_space);
    format!(
        "{}.{extension}",
        if name.is_empty() { "Untitled" } else { name }
    )
}
