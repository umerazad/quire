use super::{frontmatter_length, js_space};
use regex::Regex;
use std::{ops::Range, sync::LazyLock};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceKind {
    Page,
    Block,
    Embed,
}
impl ReferenceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Block => "block",
            Self::Embed => "embed",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceSyntax {
    Wiki,
    Roam,
}
impl ReferenceSyntax {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Wiki => "wiki",
            Self::Roam => "roam",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReferenceMedia {
    Pdf,
}
impl ReferenceMedia {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Reference {
    pub kind: ReferenceKind,
    pub syntax: ReferenceSyntax,
    pub range: Range<usize>,
    pub target: String,
    pub label: Option<String>,
    pub media: Option<ReferenceMedia>,
}

const JS_SPACE: &str = r"[\t\n\x0b\x0c\r \u{00a0}\u{1680}\u{2000}-\u{200a}\u{2028}\u{2029}\u{202f}\u{205f}\u{3000}\u{feff}]";

fn expression(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("SPACE", JS_SPACE))
        .expect("Quire's static reference grammar is invalid. Fix the pattern and rebuild.")
}

static PDF: LazyLock<Regex> =
    LazyLock::new(|| expression(r"\{\{(?:pdf|\[\[pdf\]\]):SPACE*([^}\n]+?)SPACE*\}\}"));
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    expression(
        r"\{\{(?:embed|\[\[embed\]\]):SPACE*(?:\(\(([^)\n]+)\)\)|\[\[([^\]\n]+)\]\])SPACE*\}\}|!\[\[([^\]\n]+)\]\]|\(\(([^)\n]+)\)\)|\[\[([^\]\n]+)\]\]",
    )
});

fn merged_ranges(ranges: impl Iterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let mut ranges: Vec<_> = ranges.filter(|range| range.start < range.end).collect();
    ranges.sort_unstable_by_key(|range| range.start);
    let mut merged: Vec<Range<usize>> = Vec::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn contains(ranges: &[Range<usize>], position: usize) -> bool {
    let index = ranges.partition_point(|range| range.start <= position);
    index > 0 && position < ranges[index - 1].end
}

/// Extracts Quire reference forms with native byte ranges, preserving raw targets and labels.
/// The caller supplies current Markdown inline/fenced/indented code ranges. This function adds
/// frontmatter exclusion and tests each match's start, as the legacy parser does.
pub fn references(text: &str, code_ranges: &[Range<usize>]) -> Vec<Reference> {
    if !text.contains("((") && !text.contains("[[") && !text.contains("{{pdf:") {
        return Vec::new();
    }
    let excluded = merged_ranges(
        code_ranges
            .iter()
            .cloned()
            .chain(std::iter::once(0..frontmatter_length(text))),
    );
    let mut result = Vec::new();
    for capture in PDF.captures_iter(text) {
        let whole = capture.get(0).unwrap();
        if contains(&excluded, whole.start()) {
            continue;
        }
        result.push(Reference {
            kind: ReferenceKind::Embed,
            syntax: ReferenceSyntax::Wiki,
            range: whole.range(),
            target: capture[1].trim_matches(js_space).to_owned(),
            label: None,
            media: Some(ReferenceMedia::Pdf),
        });
    }
    let pdf_ranges: Vec<_> = result
        .iter()
        .map(|reference| reference.range.clone())
        .collect();
    for capture in REFERENCE.captures_iter(text) {
        let whole = capture.get(0).unwrap();
        if contains(&excluded, whole.start()) || contains(&pdf_ranges, whole.start()) {
            continue;
        }
        let (group, raw) = (1..=5)
            .find_map(|group| capture.get(group).map(|value| (group, value.as_str())))
            .unwrap();
        let mut parts = raw.split('|');
        let target = parts.next().unwrap();
        let label = parts.next().map(str::to_owned);
        let kind = if group <= 3 {
            ReferenceKind::Embed
        } else if group == 4 || target.contains("#^") {
            ReferenceKind::Block
        } else {
            ReferenceKind::Page
        };
        result.push(Reference {
            kind,
            syntax: if matches!(group, 2 | 3 | 5) {
                ReferenceSyntax::Wiki
            } else {
                ReferenceSyntax::Roam
            },
            range: whole.range(),
            target: target.to_owned(),
            label,
            media: None,
        });
    }
    result.sort_by_key(|reference| reference.range.start);
    result
}
