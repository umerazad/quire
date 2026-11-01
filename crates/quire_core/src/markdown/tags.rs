use super::{
    JS_SPACE, Reference, frontmatter_length, js_space,
    ranges::{contains, merged_ranges, overlaps},
};
use regex::Regex;
use std::{ops::Range, sync::LazyLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagToken {
    pub range: Range<usize>,
    pub text: String,
}

fn expression(pattern: &str) -> Regex {
    Regex::new(&pattern.replace("SPACE", JS_SPACE))
        .expect("Quire's static tag grammar is invalid. Fix the pattern and rebuild.")
}

static TAG: LazyLock<Regex> = LazyLock::new(|| {
    expression(
        r#"(^|SPACE|[\(\[{"'*~])#([\p{L}\p{N}_][\p{L}\p{N}_-]*(?:/[\p{L}\p{N}_][\p{L}\p{N}_-]*)*)"#,
    )
});
static COMPLETION_BODY: LazyLock<Regex> = LazyLock::new(|| expression(r"^[\p{L}\p{N}_/-]*\z"));
static COMPLETION_TAIL: LazyLock<Regex> = LazyLock::new(|| expression(r"^[\p{L}\p{N}_/-]*"));

fn excluded(
    text: &str,
    syntax_ranges: &[Range<usize>],
    references: &[Reference],
) -> Vec<Range<usize>> {
    merged_ranges(
        syntax_ranges
            .iter()
            .cloned()
            .chain(references.iter().map(|reference| reference.range.clone()))
            .chain(std::iter::once(0..frontmatter_length(text))),
    )
}

/// Extracts inline tags with source byte ranges and the original spelling, including '#'.
/// The caller supplies current code, link, image, autolink, HTML, escape, and link-reference
/// syntax ranges. Metadata and supplied reference ranges are also excluded.
pub fn tags(text: &str, syntax_ranges: &[Range<usize>], references: &[Reference]) -> Vec<TagToken> {
    if !text.contains('#') {
        return Vec::new();
    }
    let excluded = excluded(text, syntax_ranges, references);
    TAG.captures_iter(text)
        .filter_map(|capture| {
            let value = capture.get(2).unwrap();
            let range = value.start() - 1..value.end();
            (!overlaps(&excluded, &range)).then(|| TagToken {
                text: text[range.clone()].to_owned(),
                range,
            })
        })
        .collect()
}

/// Roam-style tags written as `#[[name]]`: page references right after a `#` that starts a word,
/// as `#name` tags start, with the `#` in each token's range and `#name` as its text.
pub fn wiki_tags(text: &str, references: &[Reference]) -> Vec<TagToken> {
    references
        .iter()
        .filter(|reference| {
            let start = reference.range.start;
            reference.kind == super::ReferenceKind::Page
                && reference.label.is_none()
                && start > 0
                && text.as_bytes()[start - 1] == b'#'
                && text.get(start..start + 2) == Some("[[")
                && text[..start - 1].chars().next_back().is_none_or(|c| {
                    js_space(c) || matches!(c, '(' | '[' | '{' | '"' | '\'' | '*' | '~')
                })
        })
        .map(|reference| TagToken {
            range: reference.range.start - 1..reference.range.end,
            text: format!("#{}", reference.target),
        })
        .collect()
}

/// Returns the whole partial tag at a byte cursor position, including text after the cursor.
/// Invalid UTF-8 cursor boundaries return None. Exclusions use the tag's start, as in the
/// original completion helper; token extraction instead filters any overlap.
pub fn tag_completion_range(
    text: &str,
    position: usize,
    syntax_ranges: &[Range<usize>],
    references: &[Reference],
) -> Option<Range<usize>> {
    let prefix = text.get(..position)?;
    let (before, partial) = prefix.rsplit_once('#')?;
    if before.chars().next_back().is_some_and(|character| {
        !js_space(character) && !matches!(character, '(' | '[' | '{' | '"' | '\'' | '*' | '~')
    }) || !COMPLETION_BODY.is_match(partial)
    {
        return None;
    }
    let from = before.len();
    if contains(&excluded(text, syntax_ranges, references), from) {
        return None;
    }
    let tail = COMPLETION_TAIL.find(&text[position..]).unwrap();
    Some(from..position + tail.len())
}
