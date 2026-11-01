//! Copies of a page's text that keep its blocks addressable without duplicating block IDs: the
//! reference `copyWithNewIds`, used when a template is inserted and when a conflicting version is
//! saved as a separate page.

use super::{Reference, ReferenceKind, page_name, references};
use regex::Regex;
use std::{collections::HashMap, ops::Range, sync::LazyLock};

/// The reference `idPattern`: a `^id` marker ending a block's content.
static ID: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*$")
        .expect("Quire's block ID pattern is invalid. Fix the pattern and rebuild.")
});

/// Where a text's blocks can be addressed: the byte range of each block ID after its `^`, in
/// text order, and the references that may point at them.
#[derive(Clone, Debug, Default)]
pub struct Addresses {
    pub ids: Vec<Range<usize>>,
    pub references: Vec<Reference>,
}

/// The range of the ID in a `^id` marker ending `text[content]`, if there is one.
pub fn block_id_range(text: &str, content: Range<usize>) -> Option<Range<usize>> {
    let id = ID.captures(&text[content.clone()])?.get(1)?;
    Some(content.start + id.start()..content.start + id.end())
}

/// Byte ranges of fenced code blocks, where markers and references are literal text.
pub(crate) fn fenced_code(text: &str) -> Vec<Range<usize>> {
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

impl Addresses {
    /// Finds addresses line by line, for callers without a Markdown block parser: every line
    /// outside fenced code that ends in a `^id` marker counts as a block. It differs from the
    /// parser only where a block spans lines, such as a paragraph whose inner line ends in a
    /// marker, which the parser leaves alone.
    pub fn scan(text: &str) -> Self {
        let code = fenced_code(text);
        let in_code = |at: usize| code.iter().any(|range| range.contains(&at));
        let ids = text
            .split_inclusive('\n')
            .scan(0, |offset, line| {
                let start = *offset;
                *offset += line.len();
                Some(start..start + line.trim_end_matches(['\n', '\r']).len())
            })
            .filter(|line| !in_code(line.start))
            .filter_map(|line| block_id_range(text, line))
            .collect();
        Self {
            ids,
            references: references(text, &code),
        }
    }
}

/// Gives every block ID in `text` a fresh one from `new_id`, and points references to those
/// blocks at the new IDs. A `Page#^id` reference follows only when `Page` is one of
/// `source_names`, and then names `destination`'s page. As in the reference, `new_id` runs once
/// per marker, and every copy of a repeated ID takes the last fresh one.
pub fn copy_with_new_ids(
    text: &str,
    addresses: &Addresses,
    destination: &str,
    source_names: &[&str],
    new_id: impl FnMut() -> String,
) -> String {
    let fresh = fresh_ids(text, addresses, new_id);
    renamed_ids(text, addresses, &fresh, destination, source_names)
}

/// A fresh ID from `new_id` for each block ID in `text`, run once per marker, so every copy of
/// a repeated ID takes the last one.
pub fn fresh_ids(
    text: &str,
    addresses: &Addresses,
    mut new_id: impl FnMut() -> String,
) -> HashMap<String, String> {
    addresses
        .ids
        .iter()
        .map(|range| (text[range.clone()].to_owned(), new_id()))
        .collect()
}

/// Renames the block IDs in `text` that `ids` maps, and points references to those blocks at
/// the new IDs as `copy_with_new_ids` does. IDs that `ids` leaves out keep their names.
pub fn renamed_ids(
    text: &str,
    addresses: &Addresses,
    ids: &HashMap<String, String>,
    destination: &str,
    source_names: &[&str],
) -> String {
    let markers = addresses.ids.iter().filter_map(|range| {
        let fresh = ids.get(&text[range.clone()])?;
        Some((range.clone(), fresh.clone()))
    });
    let followed = addresses.references.iter().filter_map(|reference| {
        if reference.kind == ReferenceKind::Page {
            return None;
        }
        let (page, id) = match reference.target.find("#^") {
            Some(separator) => (
                Some(&reference.target[..separator]),
                &reference.target[separator + 2..],
            ),
            None => (None, reference.target.as_str()),
        };
        let fresh = ids.get(id)?;
        let target = match page {
            Some(page) if !source_names.contains(&page) => return None,
            Some(_) => format!("{}#^{fresh}", page_name(destination)),
            None => fresh.clone(),
        };
        Some((
            reference.range.clone(),
            text[reference.range.clone()].replacen(&reference.target, &target, 1),
        ))
    });
    let mut edits: Vec<_> = markers.chain(followed).collect();
    edits.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    edits
        .into_iter()
        .fold(text.to_owned(), |mut copy, (range, replacement)| {
            copy.replace_range(range, &replacement);
            copy
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn numbered() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("b-new{next}")
        }
    }

    #[test]
    fn copies_renumber_blocks_and_follow_their_references() {
        let text = "- first ^b-one\n- see ((b-one)) and [[Note#^b-one]] and [[Other#^b-one]]\n```\n- code ^b-two\n```\n- second ^b-two\r\n";
        assert_eq!(
            copy_with_new_ids(
                text,
                &Addresses::scan(text),
                "Note (recovered).md",
                &["Note"],
                numbered()
            ),
            "- first ^b-new1\n- see ((b-new1)) and [[Note (recovered)#^b-new1]] and [[Other#^b-one]]\n```\n- code ^b-two\n```\n- second ^b-new2\r\n"
        );
    }

    #[test]
    fn renaming_leaves_unmapped_ids_and_their_references_alone() {
        let text = "- a ^one\n- b ^two\n- ((one)) ((two)) [[Note#^one]]\n";
        let ids = HashMap::from([("one".to_owned(), "b-new".to_owned())]);
        assert_eq!(
            renamed_ids(text, &Addresses::scan(text), &ids, "Copy.md", &["Note"]),
            "- a ^b-new\n- b ^two\n- ((b-new)) ((two)) [[Copy#^b-new]]\n"
        );
    }

    #[test]
    fn repeated_ids_take_the_last_fresh_id_as_the_reference_does() {
        let text = "- a ^dup\n- b ^dup\n- ((dup))\n";
        assert_eq!(
            copy_with_new_ids(text, &Addresses::scan(text), "Note.md", &[], numbered()),
            "- a ^b-new2\n- b ^b-new2\n- ((b-new2))\n"
        );
    }
}
