//! Numbered list markers: converting a list between bullets and numbers, and renumbering the
//! numbered runs an edit touched.

use super::model::{Edit, list_mark, normalize};
use crate::markdown_blocks::Block;
use std::collections::BTreeMap;

pub const NO_LIST: &str =
    "There is no list here. Place the cursor in a list item, or select list items, to number them.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Style {
    Bullet,
    Numbered,
}

/// The items a numbering command applies to: with a cursor, the item's run of sibling items;
/// with a selection, every item it touches.
fn target_items(blocks: &[Block], from: usize, to: usize) -> Vec<&Block> {
    if from != to {
        return blocks
            .iter()
            .filter(|block| block.kind == "list" && block.from < to && block.content_to > from)
            .collect();
    }
    let Some(item) = blocks
        .iter()
        .find(|block| block.kind == "list" && block.from <= from && from <= block.content_to)
    else {
        return Vec::new();
    };
    let siblings: Vec<_> = blocks
        .iter()
        .filter(|block| block.parent == item.parent)
        .collect();
    let index = siblings
        .iter()
        .position(|block| block.key == item.key)
        .unwrap_or_default();
    let start = siblings[..index]
        .iter()
        .rposition(|block| block.kind != "list")
        .map_or(0, |position| position + 1);
    let end = siblings[index..]
        .iter()
        .position(|block| block.kind != "list")
        .map_or(siblings.len(), |position| index + position);
    siblings[start..end].to_vec()
}

/// Numbers the list items, or turns them back into bullets when they are all numbered already.
/// An explicit style converts without toggling.
pub fn numbered_list_changes(
    text: &str,
    blocks: &[Block],
    from: usize,
    to: usize,
    style: Option<Style>,
) -> Option<Vec<Edit>> {
    let items = target_items(blocks, from, to);
    if items.is_empty() {
        return None;
    }
    let to_bullets = match style {
        Some(style) => style == Style::Bullet,
        None => items
            .iter()
            .all(|item| text[item.from..].starts_with(|c: char| c.is_ascii_digit())),
    };
    let mut counters = BTreeMap::<&str, usize>::new();
    let entries = items
        .into_iter()
        .map(|item| {
            let count = counters
                .entry(item.parent.as_deref().unwrap_or(""))
                .and_modify(|count| *count += 1)
                .or_insert(1);
            let mark = if to_bullets {
                "- ".to_owned()
            } else {
                format!("{count}. ")
            };
            (item, mark)
        })
        .collect::<Vec<_>>();
    Some(marker_changes(text, &entries))
}

/// Gives each item a new marker; its child lines move by the change in marker width, so they
/// stay nested under the new marker.
fn marker_changes(text: &str, entries: &[(&Block, String)]) -> Vec<Edit> {
    let mut shifts = BTreeMap::<usize, isize>::new();
    let mut edits = Vec::new();
    for (item, mark) in entries {
        let shift = mark.len() as isize - (item.content_from - item.from) as isize;
        edits.push((item.from..item.content_from, mark.clone()));
        if shift == 0 {
            continue;
        }
        let mut line = text[item.from..]
            .find('\n')
            .map(|index| item.from + index + 1);
        while let Some(start) = line.filter(|start| *start < item.subtree_to) {
            *shifts.entry(start).or_default() += shift;
            line = text[start..].find('\n').map(|index| start + index + 1);
        }
    }
    for (line, shift) in shifts {
        let end = text[line..]
            .find('\n')
            .map_or(text.len(), |index| line + index);
        let content = &text[line..end];
        if content.trim().is_empty() || shift == 0 {
            continue;
        }
        let spaces = content.bytes().take_while(|c| *c == b' ').count();
        edits.push(if shift > 0 {
            (line..line, " ".repeat(shift as usize))
        } else {
            (line..line + spaces.min(shift.unsigned_abs()), String::new())
        });
    }
    normalize(edits)
}

fn numbered_marker<'a>(text: &'a str, block: &Block) -> Option<&'a str> {
    if block.kind != "list" {
        return None;
    }
    let marker = &text[block.from..block.content_from];
    list_mark(marker).filter(|mark| mark.starts_with(|c: char| c.is_ascii_digit()))
}

/// Renumbers the runs of numbered sibling items that `touched` reports as edited. A nested run
/// starts at 1; a top-level run keeps its first number, since a list may continue after a
/// paragraph. Runs nobody edited keep their numbers, such as a deliberate "1. 1. 1." list.
pub fn renumber_changes(
    text: &str,
    blocks: &[Block],
    touched: impl Fn(usize, usize) -> bool,
) -> Vec<Edit> {
    let mut runs: Vec<Vec<&Block>> = Vec::new();
    let mut open = BTreeMap::<&str, usize>::new();
    for block in blocks {
        let parent = block.parent.as_deref().unwrap_or("");
        if numbered_marker(text, block).is_none() {
            open.remove(parent);
            continue;
        }
        let run = *open.entry(parent).or_insert_with(|| {
            runs.push(Vec::new());
            runs.len() - 1
        });
        runs[run].push(block);
    }
    let mut entries = Vec::new();
    for run in runs {
        let (Some(first), Some(last)) = (run.first(), run.last()) else {
            continue;
        };
        if !touched(first.from, last.subtree_to) {
            continue;
        }
        let start = if first.parent.is_some() {
            1
        } else {
            numbered_marker(text, first)
                .and_then(|mark| mark[..mark.len() - 1].parse::<u64>().ok())
                .unwrap_or(1)
        };
        for (index, item) in run.into_iter().enumerate() {
            let marker = &text[item.from..item.content_from];
            let Some(mark) = numbered_marker(text, item) else {
                continue;
            };
            let next = format!(
                "{}{} ",
                start.saturating_add(index as u64),
                &mark[mark.len() - 1..]
            );
            if marker != next {
                entries.push((item, next));
            }
        }
    }
    marker_changes(text, &entries)
}
