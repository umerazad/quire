//! Renaming and moving a page: the destination checks and the per-page rewrite of every link
//! that follows it. Ported from rt `35e673677291` `src/core/relocate.ts` (`planRelocation`) and
//! `src/core/file-paths.ts`.
//!
//! Callers supply each page's references and Markdown URL ranges from native analysis, so links
//! inside code stay, and resolve reference targets with the notebook's own lookup. Text is the
//! editor's line-break-normalized text; callers write the returned edits through
//! `SourceText::apply`, which keeps each page's own line breaks where the reference rewrites a
//! CRLF page entirely in CRLF.
//!
//! Two deliberate deviations. A replacement is literal, where JavaScript's `String.replace`
//! expands `$&` and similar patterns in a new name. A URL that is only a `#fragment` stays as it
//! is, where the reference rewrites it to the page's folder name when a page inside a folder
//! moves.

use crate::markdown::{
    Reference, ReferenceKind, ReferenceSyntax, page_name, relative_vault_path, safe_page_path,
};
use crate::source_text::SourceEdit;
use regex::Regex;
use std::{ops::Range, sync::LazyLock};
use unicode_normalization::UnicodeNormalization as _;

pub const INVALID_PATH: &str = "This path cannot be used on Windows and macOS. Use ordinary names separated by /, without dots at the beginning or reserved characters.";
pub const INVALID_NAME: &str = "This page name cannot be used on Windows and macOS. Use a shorter name without a leading dot, slashes, or reserved filename characters.";
pub const MISSING_SOURCE: &str = "This source page is missing. Reopen it before moving.";
pub const CASE_ONLY: &str = "Choose a different path. Case-only renames need an intermediate filename on macOS and Windows.";
pub const TAKEN: &str = "A file already uses that destination. Choose another path.";
pub const NAME_HAS_SLASH: &str =
    "A page name cannot contain / or \\. Use Move to… to change its folder.";

static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*:|^//").unwrap());
static WORD_SCHEME: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[A-Za-z0-9_]+:").unwrap());
static METADATA: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\x{feff}?---\r?\n((?s:.*?))\r?\n---(?:\r?\n|$)").unwrap());
static TITLE_LINE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?m)^title:").unwrap());
static OPENER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\x{feff}?---\r?\n").unwrap());

fn js_trim(value: &str) -> &str {
    value.trim_matches(crate::markdown::js_space)
}

/// The reference `notebookPath`: ordinary names separated by `/`, NFC, with `.md` added for a
/// page when `markdown` is set.
pub fn notebook_path(value: &str, markdown: bool) -> Result<String, &'static str> {
    let name: String = js_trim(value).nfc().collect();
    if name.is_empty() {
        return Err(INVALID_PATH);
    }
    // The first offending part decides the message: an unusable name refuses as a page name.
    for part in name.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(INVALID_PATH);
        }
        let path = safe_page_path(part).map_err(|_| INVALID_NAME)?;
        if path[..path.len() - 3] != *part {
            return Err(INVALID_PATH);
        }
    }
    let has_md = name.len() >= 3
        && name
            .get(name.len() - 3..)
            .is_some_and(|end| end.eq_ignore_ascii_case(".md"));
    Ok(if markdown && !has_md {
        format!("{name}.md")
    } else {
        name
    })
}

/// The reference `relativePath`: `target` as seen from the folder holding `from`.
pub fn relative_path(from: &str, target: &str) -> String {
    let mut a: Vec<&str> = from.split('/').collect();
    a.pop();
    let mut b: Vec<&str> = target.split('/').collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    a.drain(..common);
    b.drain(..common);
    let joined = std::iter::repeat_n("..", a.len())
        .chain(b)
        .collect::<Vec<_>>()
        .join("/");
    if joined.is_empty() {
        target.rsplit('/').next().unwrap_or_default().to_owned()
    } else {
        joined
    }
}

/// The path a rename in place gives `source`, as the reference `renamePage` builds it, or None
/// when nothing would change.
pub fn rename_target(source: &str, name: &str) -> Result<Option<String>, &'static str> {
    let title = js_trim(name);
    let folder = source.rsplit_once('/').map_or("", |(folder, _)| folder);
    let target = if folder.is_empty() {
        format!("{title}.md")
    } else {
        format!("{folder}/{title}.md")
    };
    if title.is_empty() || target == source {
        return Ok(None);
    }
    if title.contains(['/', '\\']) {
        return Err(NAME_HAS_SLASH);
    }
    Ok(Some(target))
}

/// One planned rename or move, checked against the notebook's paths.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Relocation {
    pub source: String,
    pub target: String,
    pub rename: bool,
    source_title: String,
    unique_title: bool,
}

impl Relocation {
    /// `title` is the source page's title and `unique_title` whether no other page shares it;
    /// `taken` says whether a page already uses a path, ignoring case.
    pub fn new(
        source: &str,
        target: &str,
        rename: bool,
        title: &str,
        unique_title: bool,
        source_exists: bool,
        taken: impl Fn(&str) -> bool,
    ) -> Result<Self, &'static str> {
        let target = notebook_path(target, true)?;
        if !source_exists {
            return Err(MISSING_SOURCE);
        }
        if target.to_lowercase() == source.to_lowercase() {
            return Err(CASE_ONLY);
        }
        if taken(&target) {
            return Err(TAKEN);
        }
        Ok(Self {
            source: source.to_owned(),
            target,
            rename,
            source_title: title.to_owned(),
            unique_title,
        })
    }
}

/// One link change listed in the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkChange {
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRelocation {
    /// Where the page lives afterwards.
    pub path: String,
    /// The rewritten normalized text.
    pub text: String,
    pub changes: Vec<LinkChange>,
    /// Ordered, non-overlapping edits on the normalized text.
    pub edits: Vec<SourceEdit<String>>,
}

fn replace_first(raw: &str, from: &str, to: &str) -> String {
    raw.replacen(from, to, 1)
}

/// Rewrites one page for `relocation`: links that resolve to the moved page follow it, and the
/// moved page itself keeps its relative links working and its title as the reference does.
/// Returns None for a page other than the source when nothing changes.
pub fn relocate_page(
    relocation: &Relocation,
    path: &str,
    text: &str,
    references: &[Reference],
    urls: &[Range<usize>],
    find_page: impl Fn(&str) -> Option<String>,
) -> Option<PageRelocation> {
    let moved = path == relocation.source;
    let destination = if moved {
        relocation.target.as_str()
    } else {
        path
    };
    let mut changes: Vec<SourceEdit<String>> = Vec::new();
    for reference in references {
        let (base, fragment) = match reference.target.find('#') {
            Some(hash) => reference.target.split_at(hash),
            None => (reference.target.as_str(), ""),
        };
        let raw = &text[reference.range.clone()];
        let resolved = find_page(base);
        if (reference.syntax == ReferenceSyntax::Wiki || !fragment.is_empty())
            && resolved.as_deref() == Some(relocation.source.as_str())
        {
            if !relocation.rename
                && relocation.unique_title
                && base.to_lowercase() == relocation.source_title.to_lowercase()
            {
                continue;
            }
            let replacement = format!("{}{fragment}", page_name(&relocation.target));
            changes.push(SourceEdit {
                range: reference.range.clone(),
                replacement: replace_first(raw, &reference.target, &replacement),
            });
        } else if moved
            && reference.kind == ReferenceKind::Embed
            && resolved.is_none()
            && !WORD_SCHEME.is_match(&reference.target)
            && let Some(absolute) = relative_vault_path(Some(&relocation.source), &reference.target)
                .filter(|absolute| !absolute.is_empty())
        {
            let replacement = format!("{}{fragment}", relative_path(&relocation.target, &absolute));
            changes.push(SourceEdit {
                range: reference.range.clone(),
                replacement: replace_first(raw, &reference.target, &replacement),
            });
        }
    }
    for range in urls {
        let raw = &text[range.clone()];
        let angle = raw.len() >= 2 && raw.starts_with('<') && raw.ends_with('>');
        let url = if angle { &raw[1..raw.len() - 1] } else { raw };
        if SCHEME.is_match(url) || url.split('#').next().unwrap_or_default().is_empty() {
            continue;
        }
        let Some(absolute) =
            relative_vault_path(Some(path), url).filter(|absolute| !absolute.is_empty())
        else {
            continue;
        };
        let fragment = url.find('#').map_or("", |hash| &url[hash..]);
        if !moved && absolute != relocation.source {
            continue;
        }
        let resolved = if absolute == relocation.source {
            relocation.target.as_str()
        } else {
            absolute.as_str()
        };
        let replacement = format!("{}{fragment}", relative_path(destination, resolved));
        changes.push(SourceEdit {
            range: range.clone(),
            replacement: if angle {
                format!("<{replacement}>")
            } else {
                replacement.replace(' ', "%20")
            },
        });
    }
    let listed: Vec<LinkChange> = changes
        .iter()
        .filter(|change| text[change.range.clone()] != change.replacement)
        .map(|change| LinkChange {
            before: text[change.range.clone()].to_owned(),
            after: change.replacement.clone(),
        })
        .collect();
    if moved {
        changes.extend(title_edit(relocation, text));
    }
    changes.sort_by_key(|change| (change.range.start, change.range.end));
    let mut edits: Vec<SourceEdit<String>> = Vec::new();
    for change in changes {
        // A title written over its whole line replaces any link edit on that line, and a link
        // inside another link's range keeps only the outer edit.
        if let Some(last) = edits.last_mut()
            && change.range.start < last.range.end
        {
            if change.range.end >= last.range.end && change.range.start <= last.range.start {
                *last = change;
            }
            continue;
        }
        edits.push(change);
    }
    if !moved
        && edits
            .iter()
            .all(|edit| text[edit.range.clone()] == edit.replacement)
    {
        return None;
    }
    let mut next = text.to_owned();
    for edit in edits.iter().rev() {
        next.replace_range(edit.range.clone(), &edit.replacement);
    }
    if !moved && next == text {
        return None;
    }
    Some(PageRelocation {
        path: destination.to_owned(),
        text: next,
        changes: listed,
        edits,
    })
}

fn quoted(value: &str) -> String {
    serde_json::to_string(value).expect("a page name serializes")
}

/// A rename writes the new name into a front matter `title`; a move records the old title in
/// front matter so links by title keep resolving.
fn title_edit(relocation: &Relocation, text: &str) -> Option<SourceEdit<String>> {
    if relocation.rename {
        let range = title_line(text)?;
        return Some(SourceEdit {
            range,
            replacement: format!("title: {}", quoted(page_name(&relocation.target))),
        });
    }
    let title = crate::markdown::page_title(&relocation.source, text).to_lossy_string();
    match METADATA.captures(text) {
        None => Some(SourceEdit {
            range: 0..0,
            replacement: format!("---\ntitle: {}\n---\n\n", quoted(&title)),
        }),
        Some(metadata) if !TITLE_LINE.is_match(&metadata[1]) => {
            let at = OPENER.find(text)?.end();
            Some(SourceEdit {
                range: at..at,
                replacement: format!("title: {}\n", quoted(&title)),
            })
        }
        Some(_) => None,
    }
}

/// The reference pattern `^(﻿?---\r?\n(?:(?!---)[^\n]*\n)*?)title:[^\n]*`: the first line
/// starting `title:` after an opening `---`, before any line starting `---`.
fn title_line(text: &str) -> Option<Range<usize>> {
    let mut at = OPENER.find(text)?.end();
    loop {
        let rest = &text[at..];
        let line_end = rest.find('\n');
        if rest.starts_with("title:") {
            return Some(at..at + line_end.unwrap_or(rest.len()));
        }
        if rest.starts_with("---") {
            return None;
        }
        at += line_end? + 1;
    }
}
