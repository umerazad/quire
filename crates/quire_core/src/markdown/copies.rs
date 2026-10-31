//! Copies of a page's text that keep its blocks addressable without duplicating block IDs: the
//! reference `copyWithNewIds`, used when a conflicting version is saved as a separate page.

use super::{ReferenceKind, page_name, references};
use regex::Regex;
use std::{collections::HashMap, ops::Range, sync::LazyLock};

/// The reference `idPattern`: a `^id` marker ending a line.
static ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*$")
        .expect("Quire's block ID pattern is invalid. Fix the pattern and rebuild.")
});

/// Byte ranges of fenced code blocks, where markers and references are literal text.
fn fenced_code(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut open: Option<(usize, &str)> = None;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let fence = ["```", "~~~"]
            .into_iter()
            .find(|fence| trimmed.starts_with(fence));
        match (open, fence) {
            (None, Some(fence)) => open = Some((offset, fence)),
            (Some((start, opened)), Some(fence)) if fence == opened => {
                ranges.push(start..offset + line.len());
                open = None;
            }
            _ => {}
        }
        offset += line.len();
    }
    if let Some((start, _)) = open {
        ranges.push(start..text.len());
    }
    ranges
}

/// Gives every block ID in `text` a fresh one from `new_id`, and points references to those
/// blocks at the new IDs. A `Page#^id` reference follows only when `Page` is one of
/// `source_names`, and then names `destination`'s page.
pub fn copy_with_new_ids(
    text: &str,
    destination: &str,
    source_names: &[&str],
    mut new_id: impl FnMut() -> String,
) -> String {
    let code = fenced_code(text);
    let in_code = |at: usize| code.iter().any(|range| range.contains(&at));
    let mut ids = HashMap::new();
    let mut edits: Vec<(Range<usize>, String)> = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        if !in_code(offset)
            && let Some(id) = ID.captures(content).and_then(|captures| captures.get(1))
        {
            let fresh = ids
                .entry(id.as_str().to_owned())
                .or_insert_with(&mut new_id)
                .clone();
            edits.push((offset + id.start()..offset + id.end(), fresh));
        }
        offset += line.len();
    }
    for reference in references(text, &code) {
        if reference.kind == ReferenceKind::Page {
            continue;
        }
        let (page, id) = match reference.target.find("#^") {
            Some(separator) => (
                Some(&reference.target[..separator]),
                &reference.target[separator + 2..],
            ),
            None => (None, reference.target.as_str()),
        };
        let Some(fresh) = ids.get(id) else {
            continue;
        };
        let target = match page {
            Some(page) if !source_names.contains(&page) => continue,
            Some(_) => format!("{}#^{fresh}", page_name(destination)),
            None => fresh.clone(),
        };
        let original = &text[reference.range.clone()];
        edits.push((
            reference.range.clone(),
            original.replacen(&reference.target, &target, 1),
        ));
    }
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut copy = text.to_owned();
    for (range, replacement) in edits {
        copy.replace_range(range, &replacement);
    }
    copy
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copies_renumber_blocks_and_follow_their_references() {
        let mut next = 0;
        let new_id = || {
            next += 1;
            format!("b-new{next}")
        };
        let text = "- first ^b-one\n- see ((b-one)) and [[Note#^b-one]] and [[Other#^b-one]]\n```\n- code ^b-two\n```\n- second ^b-two\r\n";
        assert_eq!(
            copy_with_new_ids(text, "Note (recovered).md", &["Note"], new_id),
            "- first ^b-new1\n- see ((b-new1)) and [[Note (recovered)#^b-new1]] and [[Other#^b-one]]\n```\n- code ^b-two\n```\n- second ^b-new2\r\n"
        );
    }
}
