//! Outline edits computed over a page's source text and its block model.
//!
//! Each command returns `None` when the outline rules do not apply, so the editor's ordinary
//! command runs instead. Edits are given in the original text's byte offsets, sorted and
//! non-overlapping; the cursor is a byte offset in the edited text.

use crate::markdown_blocks::{Block, id_tail};
use std::ops::Range;

pub type Edit = (Range<usize>, String);

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Plan {
    pub edits: Vec<Edit>,
    pub cursor: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Apply(Plan),
    Refuse(&'static str),
}

pub const REFERENCED: &str = "Cannot merge a referenced block in Write view. Use Shift-Tab to outdent, move the block, or edit its structure in Source.";

impl Plan {
    fn new(edits: Vec<Edit>, cursor: usize) -> Self {
        Self {
            edits: normalize(edits),
            cursor,
        }
    }

    fn stay(cursor: usize) -> Self {
        Self {
            edits: Vec::new(),
            cursor,
        }
    }
}

fn apply(plan: Plan) -> Option<Outcome> {
    Some(Outcome::Apply(plan))
}

/// Sorts edits and joins those sharing a boundary, so the buffer applies them in their given order.
pub fn normalize(mut edits: Vec<Edit>) -> Vec<Edit> {
    edits.sort_by_key(|(range, _)| (range.start, range.end));
    edits
        .into_iter()
        .fold(Vec::new(), |mut merged: Vec<Edit>, (range, text)| {
            match merged.last_mut() {
                Some((last, inserted)) if range.start <= last.end => {
                    last.end = last.end.max(range.end);
                    inserted.push_str(&text);
                }
                _ => merged.push((range, text)),
            }
            merged
        })
}

/// The edits without the text each one would replace with itself at its start and end.
pub fn trim_unchanged(text: &str, edits: Vec<Edit>) -> Vec<Edit> {
    edits
        .into_iter()
        .map(|(range, new)| {
            let old = &text[range.clone()];
            let prefix: usize = old
                .chars()
                .zip(new.chars())
                .take_while(|(a, b)| a == b)
                .map(|(c, _)| c.len_utf8())
                .sum();
            let suffix: usize = old[prefix..]
                .chars()
                .rev()
                .zip(new[prefix..].chars().rev())
                .take_while(|(a, b)| a == b)
                .map(|(c, _)| c.len_utf8())
                .sum();
            let kept = new[prefix..new.len() - suffix].to_owned();
            (range.start + prefix..range.end - suffix, kept)
        })
        .collect()
}

/// Maps a position in the original text through normalized edits, as CodeMirror maps it: a
/// position at an insertion or inside a replaced range goes after the new text when `after` is
/// true; the start of a replaced range stays before it and its end follows it.
pub fn map_position(edits: &[Edit], position: usize, after: bool) -> usize {
    let mut delta = 0isize;
    for (range, text) in edits {
        let start = range.start.saturating_add_signed(delta);
        if range.is_empty() {
            if position < range.start || (position == range.start && !after) {
                break;
            }
            if position == range.start {
                return start + text.len();
            }
        } else {
            if position <= range.start {
                break;
            }
            if position < range.end {
                return if after { start + text.len() } else { start };
            }
        }
        delta += text.len() as isize - range.len() as isize;
    }
    position.saturating_add_signed(delta)
}

pub fn apply_edits(text: &str, edits: &[Edit]) -> String {
    let mut result = String::with_capacity(text.len());
    let mut at = 0;
    for (range, inserted) in edits {
        result.push_str(&text[at..range.start]);
        result.push_str(inserted);
        at = range.end;
    }
    result.push_str(&text[at..]);
    result
}

pub fn line_start(text: &str, position: usize) -> usize {
    text[..position].rfind('\n').map_or(0, |index| index + 1)
}

pub fn line_end(text: &str, position: usize) -> usize {
    text[position..]
        .find('\n')
        .map_or(text.len(), |index| position + index)
}

/// The zero-based line holding `position`.
pub fn row_of(text: &str, position: usize) -> usize {
    text[..position].matches('\n').count()
}

/// Where line `row` starts, or the text's end past its last line.
pub fn row_start(text: &str, row: usize) -> usize {
    match row.checked_sub(1) {
        None => 0,
        Some(before) => text
            .match_indices('\n')
            .nth(before)
            .map_or(text.len(), |(index, _)| index + 1),
    }
}

fn blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// The text start and end a writer sees in Write view, past the marker and checkbox and before
/// a hidden block ID.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub line_from: usize,
    pub text_start: usize,
    pub text_end: usize,
}

pub struct Task<'a> {
    pub source: &'a str,
    pub prefix: usize,
}

/// Content without a trailing ID, trimmed at its end.
fn stripped<'a>(text: &'a str, block: &Block) -> &'a str {
    let content = &text[block.content_from..block.content_to];
    id_tail(content)
        .map_or(content, |(range, _)| &content[..range.start])
        .trim_end()
}

pub fn task<'a>(text: &'a str, block: &Block) -> Option<Task<'a>> {
    if block.kind != "list" {
        return None;
    }
    let body = stripped(text, block);
    let marker = crate::task_marker::read(body)?;
    let rest = &body[marker.source.len()..];
    let gap = rest.len() - rest.trim_start_matches([' ', '\t']).len();
    Some(Task {
        source: marker.source,
        prefix: marker.source.len() + gap,
    })
}

fn task_prefix(text: &str, block: &Block) -> usize {
    task(text, block).map_or(0, |task| task.prefix)
}

pub fn bounds(text: &str, block: &Block) -> Bounds {
    let content = &text[block.content_from..block.content_to];
    Bounds {
        line_from: line_start(text, block.from),
        text_start: block.content_from + task_prefix(text, block),
        text_end: block
            .id
            .as_ref()
            .and_then(|_| id_tail(content))
            .map_or(block.content_to, |(range, _)| {
                block.content_from + range.start
            }),
    }
}

fn first_line_end(text: &str, block: &Block) -> usize {
    line_end(text, block.content_from).min(block.content_to)
}

/// The list marker at the start of `text`: `-`, `+`, `*`, or a number followed by `.` or `)`.
pub fn list_mark(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    match bytes.first()? {
        b'-' | b'+' | b'*' => Some(&text[..1]),
        b'0'..=b'9' => {
            let digits = bytes.iter().take_while(|c| c.is_ascii_digit()).count();
            matches!(bytes.get(digits), Some(b'.' | b')')).then(|| &text[..=digits])
        }
        _ => None,
    }
}

fn numbered(mark: &str) -> bool {
    mark.starts_with(|c: char| c.is_ascii_digit())
}

fn mark_number(mark: &str) -> u64 {
    mark[..mark.len() - 1].parse().unwrap_or(u64::MAX)
}

fn mark_suffix(mark: &str) -> &str {
    &mark[mark.len() - 1..]
}

fn list_at<'a>(blocks: &'a [Block], key: &str) -> Option<&'a Block> {
    blocks.iter().find(|block| block.key == key)
}

fn contains_list(block: &Block, position: usize) -> bool {
    block.kind == "list" && block.content_from <= position && position <= block.content_to
}

/// Where a trailing ID starts for splitting: the space before `^`, or the content start.
fn id_split_point(text: &str, block: &Block) -> usize {
    let content = &text[block.content_from..block.content_to];
    match id_tail(content) {
        Some((range, _)) => {
            let caret = range.start + content[range.start..].find('^').unwrap_or(0);
            block.content_from + caret.saturating_sub(1)
        }
        None => block.content_to,
    }
}

fn continued_task(task: &Option<Task<'_>>) -> String {
    task.as_ref().map_or_else(String::new, |task| {
        format!(
            "{} ",
            task.source
                .replace("DONE", "TODO")
                .replacen("[x]", "[ ]", 1)
                .replacen("[X]", "[ ]", 1)
        )
    })
}

/// Enter in a list item: continue the list, split the text, or end an empty item.
/// `zoom` is the ID of a zoomed block; `folded` reports whether the children after a block's
/// content are collapsed.
pub fn split(
    text: &str,
    blocks: &[Block],
    head: usize,
    zoom: Option<&str>,
    folded: impl Fn(usize) -> bool,
) -> Option<Outcome> {
    let block = blocks.iter().find(|block| contains_list(block, head))?;
    let task = task(text, block);
    let prefix = task.as_ref().map_or(0, |task| task.prefix);
    let body = stripped(text, block);
    if block.id.is_none()
        && blank(body.get(prefix..).unwrap_or(""))
        && block.subtree_to == block.content_to
    {
        return match &block.parent {
            Some(parent) if Some(parent.as_str()) != zoom => indent(text, blocks, head, true, zoom),
            Some(_) => apply(Plan::stay(head)),
            None => {
                let line = line_start(text, block.from);
                apply(Plan::new(
                    vec![(line..block.content_to, String::new())],
                    line,
                ))
            }
        };
    }
    let marker_from = id_split_point(text, block);
    let text_start = block.content_from + prefix;
    let cursor = head.min(marker_from).max(marker_from.min(text_start));
    let tail = text[cursor..marker_from].trim_start();
    let line = line_start(text, block.from);
    let indent = &text[line..block.from];
    let retained = block
        .id
        .as_ref()
        .map_or_else(String::new, |id| format!(" ^{id}"));
    let mark = list_mark(&text[block.from..]).unwrap_or("-");
    let next_mark = if numbered(mark) {
        format!(
            "{}{}",
            mark_number(mark).saturating_add(1),
            mark_suffix(mark)
        )
    } else {
        mark.to_owned()
    };
    let task_text = continued_task(&task);
    if cursor <= text_start && !blank(&text[cursor..marker_from]) {
        if zoom.is_some() && block.id.as_deref() == zoom {
            return apply(Plan::stay(head));
        }
        let above = format!("{indent}{mark} {task_text}\n");
        let cursor = head + above.len();
        return apply(Plan::new(vec![(line..line, above)], cursor));
    }
    let child = blocks
        .iter()
        .find(|item| item.parent.as_ref() == Some(&block.key));
    let zoom_root = zoom.is_some() && block.id.as_deref() == zoom;
    if (child.is_some() && !folded(block.content_to)) || zoom_root {
        let child_indent = child.map_or_else(
            || format!("{indent}{}", " ".repeat(block.content_from - block.from)),
            |child| text[line_start(text, child.from)..child.from].to_owned(),
        );
        let child_mark = child
            .and_then(|child| list_mark(&text[child.from..]))
            .unwrap_or(if numbered(mark) { mark } else { "-" });
        let first_mark = if numbered(child_mark) {
            format!("1{}", mark_suffix(child_mark))
        } else {
            child_mark.to_owned()
        };
        let insert = format!("{retained}\n{child_indent}{first_mark} {task_text}");
        let cursor_after = cursor + insert.len();
        return apply(Plan::new(
            vec![(cursor..block.content_to, format!("{insert}{tail}"))],
            cursor_after,
        ));
    }
    let prefix_text = format!("\n{indent}{next_mark} {task_text}");
    if block.subtree_to == block.content_to {
        let after = cursor + retained.len() + prefix_text.len();
        return apply(Plan::new(
            vec![(
                cursor..block.content_to,
                format!("{retained}{prefix_text}{tail}"),
            )],
            after,
        ));
    }
    let after = block.subtree_to - (block.content_to - cursor) + retained.len() + prefix_text.len();
    apply(Plan::new(
        vec![
            (cursor..block.content_to, retained),
            (
                block.subtree_to..block.subtree_to,
                format!("{prefix_text}{tail}"),
            ),
        ],
        after,
    ))
}

/// Enter at the text end of a paragraph or heading with a hidden ID breaks the line after the ID.
pub fn newline_after_id(text: &str, blocks: &[Block], head: usize) -> Option<Outcome> {
    let block = blocks.iter().find(|block| {
        block.id.is_some() && block.kind != "list" && bounds(text, block).text_end == head
    })?;
    if block.content_to == head {
        return None;
    }
    apply(Plan::new(
        vec![(block.content_to..block.content_to, "\n".into())],
        block.content_to + 1,
    ))
}

/// A line holding only a bullet without its space becomes a bullet before indenting.
pub fn bare_bullet(text: &str, head: usize) -> Option<Plan> {
    let from = line_start(text, head);
    let to = line_end(text, head);
    let line = &text[from..to];
    let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
    let rest = &line[indent..];
    let mark = rest
        .chars()
        .next()
        .filter(|c| matches!(c, '-' | '*' | '+'))?;
    if !rest[1..].chars().all(|c| matches!(c, ' ' | '\t')) {
        return None;
    }
    let bullet = format!("{}{mark} ", &line[..indent]);
    (line != bullet).then(|| Plan::new(vec![(from..to, bullet.clone())], from + bullet.len()))
}

/// The lines a block and its children occupy, including the final line break.
fn subtree_range(text: &str, block: &Block) -> Range<usize> {
    let from = line_start(text, block.from);
    let last = line_start(text, block.from.max(block.subtree_to.saturating_sub(1)));
    let end = line_end(text, last);
    from..end + usize::from(end < text.len())
}

/// Tab or Shift-Tab in a list item moves it, with its children, one level in or out. Outdenting
/// places the block after its former parent's remaining children and never leaves `zoom`.
pub fn indent(
    text: &str,
    blocks: &[Block],
    head: usize,
    outdent: bool,
    zoom: Option<&str>,
) -> Option<Outcome> {
    let block = blocks
        .iter()
        .find(|item| item.kind == "list" && item.from <= head && head <= item.content_to)?;
    indent_group(text, blocks, &[block], head, outdent, zoom)
}

/// Vim `>` and `<` over several lines, from a count or a visual selection, move the list items
/// starting on those lines one level together, each with its children. They must be siblings.
pub fn indent_lines(
    text: &str,
    blocks: &[Block],
    lines: Range<usize>,
    outdent: bool,
    zoom: Option<&str>,
) -> Option<Outcome> {
    let from = line_start(text, lines.start);
    let to = line_end(text, lines.end);
    let selected: Vec<&Block> = blocks
        .iter()
        .filter(|item| item.kind == "list" && (from..=to).contains(&line_start(text, item.from)))
        .collect();
    let group: Vec<&Block> = selected
        .iter()
        .copied()
        .filter(|item| {
            !selected
                .iter()
                .any(|other| item.parent.as_deref() == Some(other.key.as_str()))
        })
        .collect();
    let first = group.first()?;
    if group.iter().any(|item| item.parent != first.parent) {
        return Some(Outcome::Refuse(NOT_SIBLINGS));
    }
    let head = bounds(text, first).text_start;
    indent_group(text, blocks, &group, head, outdent, zoom)
}

pub const NOT_SIBLINGS: &str = "Those list items sit at different levels. Select items under one parent to move them together.";

fn indent_group(
    text: &str,
    blocks: &[Block],
    group: &[&Block],
    head: usize,
    outdent: bool,
    zoom: Option<&str>,
) -> Option<Outcome> {
    let (block, last) = (*group.first()?, *group.last()?);
    let range = subtree_range(text, block).start..subtree_range(text, last).end;
    let old_indent = &text[range.start..block.from];
    let target = if outdent {
        block
            .parent
            .as_deref()
            .and_then(|parent| list_at(blocks, parent))
    } else {
        blocks.iter().rfind(|item| {
            item.kind == "list" && item.parent == block.parent && item.from < block.from
        })
    };
    let Some(target) = target else {
        return apply(Plan::stay(head));
    };
    if outdent
        && zoom.is_some()
        && (target.id.as_deref() == zoom || group.iter().any(|item| item.id.as_deref() == zoom))
    {
        return apply(Plan::stay(head));
    }
    if !outdent && !blank(text.get(target.subtree_to..range.start).unwrap_or("")) {
        return apply(Plan::stay(head));
    }
    let target_line = line_start(text, target.from);
    let next_indent = format!(
        "{}{}",
        &text[target_line..target.from],
        if outdent {
            String::new()
        } else {
            " ".repeat(target.content_from - target.from)
        }
    );
    let body = &text[range.clone()];
    let mut offset = 0;
    let mut local = Vec::new();
    for line in body.split('\n') {
        if offset < body.len() && line.starts_with(old_indent) {
            local.push((offset..offset + old_indent.len(), next_indent.clone()));
        }
        offset += line.len() + 1;
    }
    let local = normalize(local);
    let local_head = map_position(&local, head - range.start, true);
    let destination = if outdent {
        subtree_range(text, target).end
    } else {
        range.end
    };
    if destination == range.end {
        let edits = local
            .into_iter()
            .map(|(edit, inserted)| (edit.start + range.start..edit.end + range.start, inserted))
            .collect();
        return apply(Plan::new(edits, range.start + local_head));
    }
    let moved = apply_edits(body, &local);
    let separator = if text[..destination].ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let cursor = destination - range.len() + separator.len() + local_head;
    apply(Plan::new(
        vec![
            (range.clone(), String::new()),
            (destination..destination, format!("{separator}{moved}")),
        ],
        cursor,
    ))
}

/// The lines a block and its children occupy, with the final line break.
fn subtree_lines(text: &str, block: &Block) -> Range<usize> {
    let from = line_start(text, block.from);
    let end = text[block.subtree_to..]
        .find('\n')
        .map_or(text.len(), |index| block.subtree_to + index + 1);
    from..end
}

/// Swaps a list item, with its children, and its neighbouring sibling.
pub fn move_block(text: &str, blocks: &[Block], head: usize, up: bool) -> Option<Outcome> {
    let block = blocks.iter().rfind(|item| {
        item.kind == "list" && line_start(text, item.from) <= head && head <= item.content_to
    })?;
    let siblings: Vec<_> = blocks
        .iter()
        .filter(|item| item.kind == "list" && item.parent == block.parent)
        .collect();
    let index = siblings.iter().position(|item| item.key == block.key)?;
    let other = if up {
        index.checked_sub(1).and_then(|index| siblings.get(index))
    } else {
        siblings.get(index + 1)
    };
    let Some(other) = other else {
        return apply(Plan::stay(head));
    };
    let (first, second) = if up { (*other, block) } else { (block, *other) };
    let a = subtree_lines(text, first);
    let b = subtree_lines(text, second);
    if a.end != b.start {
        return apply(Plan::stay(head));
    }
    let mut upper = text[a.clone()].to_owned();
    let mut lower = text[b.clone()].to_owned();
    if !lower.ends_with('\n') {
        lower.push('\n');
        upper.pop();
    }
    let moved = if up { &b } else { &a };
    let offset = head - moved.start;
    let cursor = if up {
        a.start + offset
    } else {
        a.start + lower.len() + offset
    };
    apply(Plan::new(vec![(a.start..b.end, lower + &upper)], cursor))
}

fn mergeable(block: &Block) -> bool {
    matches!(block.kind, "list" | "paragraph" | "heading")
}

fn content_column(text: &str, block: &Block) -> usize {
    block.content_from - line_start(text, block.from)
}

/// Shifts a merged block's child lines so they stay nested under the block it joined.
fn shift_children(text: &str, block: &Block, delta: isize) -> Vec<Edit> {
    let mut edits = Vec::new();
    if delta == 0 {
        return edits;
    }
    let mut line = text[block.content_to..]
        .find('\n')
        .map(|index| block.content_to + index + 1);
    while let Some(start) = line.filter(|start| *start < block.subtree_to) {
        let spaces = text[start..].bytes().take_while(|c| *c == b' ').count();
        if text.as_bytes().get(start) != Some(&b'\n') {
            if delta > 0 {
                edits.push((start..start, " ".repeat(delta as usize)));
            } else {
                edits.push((
                    start..start + spaces.min(delta.unsigned_abs()),
                    String::new(),
                ));
            }
        }
        line = text[start..].find('\n').map(|index| start + index + 1);
    }
    edits
}

/// Joined text before an empty block's ID gets a space, so the ID stays an ID.
fn before_id(text: &str, at: usize, insert: &str) -> String {
    if !insert.is_empty()
        && text.as_bytes().get(at) == Some(&b'^')
        && !insert.ends_with(char::is_whitespace)
    {
        format!("{insert} ")
    } else {
        insert.to_owned()
    }
}

/// An empty bullet with a block after it is removed, its children move up a level, and the
/// caret goes to the next block's text.
fn remove_empty(text: &str, blocks: &[Block], index: usize) -> Option<Plan> {
    let block = &blocks[index];
    let current = bounds(text, block);
    let next = blocks.get(index + 1)?;
    if block.kind != "list" || block.id.is_some() || current.text_start < current.text_end {
        return None;
    }
    let end = line_end(text, block.content_to);
    let mut edits = vec![(current.line_from..(end + 1).min(text.len()), String::new())];
    edits.extend(shift_children(
        text,
        block,
        block.from as isize - block.content_from as isize,
    ));
    let edits = normalize(edits);
    let cursor = map_position(&edits, bounds(text, next).text_start, true);
    Some(Plan { edits, cursor })
}

/// Backspace at a block's text start: drop its checkbox first, then join its text to the end of
/// the block above. An empty first bullet is removed instead.
pub fn merge_backward(text: &str, blocks: &[Block], position: usize) -> Option<Outcome> {
    let index = blocks.iter().position(|block| {
        mergeable(block)
            && block.content_from <= position
            && position <= bounds(text, block).text_start
    })?;
    let block = &blocks[index];
    let current = bounds(text, block);
    if position == current.text_start && current.text_start > block.content_from {
        return apply(Plan::new(
            vec![(block.content_from..current.text_start, String::new())],
            block.content_from,
        ));
    }
    if position != block.content_from {
        return None;
    }
    let previous = index.checked_sub(1).map(|index| &blocks[index]);
    let Some(previous) = previous.filter(|previous| mergeable(previous)) else {
        return (block.kind == "list").then(|| {
            Outcome::Apply(
                remove_empty(text, blocks, index).unwrap_or_else(|| Plan::stay(position)),
            )
        });
    };
    if block.id.is_some() {
        return Some(Outcome::Refuse(REFERENCED));
    }
    let above = bounds(text, previous);
    let mut edits = vec![
        (
            above.text_end..above.text_end,
            before_id(
                text,
                above.text_end,
                &text[current.text_start..block.content_to],
            ),
        ),
        (previous.content_to..block.content_to, String::new()),
    ];
    edits.extend(shift_children(
        text,
        block,
        content_column(text, previous) as isize - content_column(text, block) as isize,
    ));
    let edits = normalize(edits);
    let cursor = map_position(&edits, above.text_end, false);
    apply(Plan { edits, cursor })
}

/// Delete at a block's text end pulls the next block's text in.
pub fn merge_forward(text: &str, blocks: &[Block], position: usize) -> Option<Outcome> {
    let index = blocks
        .iter()
        .position(|block| mergeable(block) && bounds(text, block).text_end == position)?;
    let block = &blocks[index];
    let current = bounds(text, block);
    if let Some(plan) = remove_empty(text, blocks, index) {
        return apply(plan);
    }
    let Some(next) = blocks.get(index + 1).filter(|next| mergeable(next)) else {
        return block
            .id
            .is_some()
            .then(|| Outcome::Apply(Plan::stay(position)));
    };
    if next.id.is_some() {
        return Some(Outcome::Refuse(REFERENCED));
    }
    let below = bounds(text, next);
    let mut edits = vec![
        (
            current.text_end..current.text_end,
            before_id(
                text,
                current.text_end,
                &text[below.text_start..next.content_to],
            ),
        ),
        (block.content_to..next.content_to, String::new()),
    ];
    edits.extend(shift_children(
        text,
        next,
        content_column(text, block) as isize - content_column(text, next) as isize,
    ));
    let edits = normalize(edits);
    let cursor = map_position(&edits, current.text_end, false);
    apply(Plan { edits, cursor })
}

/// Vim's `J`: the next block's text joins this one after a space.
pub fn join(text: &str, blocks: &[Block], position: usize) -> Option<Outcome> {
    let index = blocks.iter().position(|block| {
        mergeable(block) && line_start(text, block.from) <= position && position <= block.content_to
    })?;
    let block = &blocks[index];
    let current = bounds(text, block);
    let next = blocks.get(index + 1).filter(|next| mergeable(next))?;
    if next.id.is_some() {
        return Some(Outcome::Refuse(REFERENCED));
    }
    let below = bounds(text, next);
    let joined = text[below.text_start..next.content_to].trim_start();
    let space = if !joined.is_empty() && !text[..current.text_end].ends_with(char::is_whitespace) {
        " "
    } else {
        ""
    };
    let mut edits = vec![
        (
            current.text_end..current.text_end,
            before_id(text, current.text_end, &format!("{space}{joined}")),
        ),
        (block.content_to..next.content_to, String::new()),
    ];
    edits.extend(shift_children(
        text,
        next,
        content_column(text, block) as isize - content_column(text, next) as isize,
    ));
    let edits = normalize(edits);
    let cursor = map_position(&edits, current.text_end, false);
    apply(Plan { edits, cursor })
}

/// The list item holding the line at `position`, for line commands.
pub fn list_line<'a>(
    text: &str,
    blocks: &'a [Block],
    position: usize,
) -> Option<(&'a Block, Bounds)> {
    blocks
        .iter()
        .find(|item| {
            item.kind == "list"
                && line_start(text, item.from) <= position
                && position <= first_line_end(text, item)
        })
        .map(|block| (block, bounds(text, block)))
}

/// Vim's `o` and `O` in Write view: open a sibling block below or above the line's block.
/// Returns `None` outside a list, where Vim's own command applies.
pub fn open_line(
    text: &str,
    blocks: &[Block],
    head: usize,
    below: bool,
    zoom: Option<&str>,
    folded: impl Fn(usize) -> bool,
) -> Option<Plan> {
    let (block, bounds) = list_line(text, blocks, head)?;
    let marker = &text[bounds.line_from..block.content_from];
    if !below {
        return Some(Plan::new(
            vec![(bounds.line_from..bounds.line_from, format!("{marker}\n"))],
            bounds.line_from + marker.len(),
        ));
    }
    if bounds.text_start == bounds.text_end {
        let at = block.subtree_to;
        return Some(Plan::new(
            vec![(at..at, format!("\n{marker}"))],
            at + 1 + marker.len(),
        ));
    }
    match split(text, blocks, bounds.text_end, zoom, folded)? {
        Outcome::Apply(plan) => Some(plan),
        Outcome::Refuse(_) => None,
    }
}

/// Pasting several lines into a list block makes each line a block: the first joins the caret's
/// block, the rest follow with their relative nesting, and the text after the caret ends the
/// last one. IDs stay at the ends of their blocks; fenced code pastes as it is.
pub fn paste(text: &str, blocks: &[Block], head: usize, pasted: &str) -> Option<Plan> {
    let normalized = pasted.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.trim_end_matches('\n').split('\n').collect();
    if lines.len() < 2
        || normalized.lines().any(|line| {
            let line = line.trim_start_matches([' ', '\t']);
            line.starts_with("```") || line.starts_with("~~~")
        })
    {
        return None;
    }
    let (block, bounds) = list_line(text, blocks, head)?;
    if head < bounds.text_start || head > bounds.text_end {
        return None;
    }
    struct Item<'a> {
        indent: usize,
        marker: Option<&'a str>,
        body: &'a str,
        id: Option<&'a str>,
    }
    let items: Vec<Item> = lines
        .iter()
        .filter(|line| !blank(line))
        .map(|line| {
            let body_start = line.len() - line.trim_start_matches([' ', '\t']).len();
            let indent = line[..body_start].replace('\t', "  ").len();
            let rest = &line[body_start..];
            let (marker, body) = match list_mark(rest) {
                Some(mark) if rest[mark.len()..].starts_with([' ', '\t']) => {
                    let body = rest[mark.len()..].trim_start_matches([' ', '\t']);
                    (Some(mark), body)
                }
                _ => (None, rest),
            };
            let (body, id) = match id_tail(body) {
                Some((range, id)) => (&body[..range.start], Some(id)),
                None => (body, None),
            };
            Item {
                indent,
                marker,
                body,
                id,
            }
        })
        .collect();
    if items.len() < 2 {
        return None;
    }
    let line_indent = &text[bounds.line_from..block.from];
    let own_mark = list_mark(&text[block.from..]).unwrap_or("-");
    let tail = &text[head..bounds.text_end];
    let suffix = |id: Option<&str>| id.map_or_else(String::new, |id| format!(" ^{id}"));
    let first = &items[0];
    let current = format!(
        "{}{}",
        before_id(text, bounds.text_end, first.body),
        if block.id.is_some() {
            text[bounds.text_end..block.content_to].to_owned()
        } else {
            suffix(first.id)
        }
    );
    let mut levels = vec![(first.indent, content_column(text, block))];
    let count = items.len() - 1;
    let rest = items[1..]
        .iter()
        .enumerate()
        .map(|(index, item)| {
            while levels
                .last()
                .is_some_and(|(indent, _)| *indent >= item.indent)
            {
                levels.pop();
            }
            let parent = levels.last().copied();
            let indent =
                parent.map_or_else(|| line_indent.to_owned(), |(_, column)| " ".repeat(column));
            let marker = item
                .marker
                .unwrap_or(if parent.is_some() { "-" } else { own_mark });
            let line = format!("{indent}{marker} {}", item.body);
            levels.push((item.indent, indent.len() + marker.len() + 1));
            if index + 1 < count {
                format!("{line}{}", suffix(item.id))
            } else {
                format!("{line}{tail}{}", suffix(item.id))
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    let tail_at = rest.len() - tail.len() - suffix(items[count].id).len();
    if block.subtree_to > block.content_to {
        let edits = normalize(vec![
            (head..block.content_to, current),
            (block.subtree_to..block.subtree_to, format!("\n{rest}")),
        ]);
        let cursor = map_position(&edits, block.subtree_to, false) + 1 + tail_at;
        return Some(Plan { edits, cursor });
    }
    let cursor = head + current.len() + 1 + tail_at;
    Some(Plan::new(
        vec![(head..block.content_to, format!("{current}\n{rest}"))],
        cursor,
    ))
}

/// Cmd-Backspace in a list item deletes back to its text start and keeps the marker, the
/// checkbox, and the ID, as deleting to the line start would not.
pub fn delete_to_text_start(text: &str, blocks: &[Block], head: usize) -> Option<Plan> {
    let (_, bounds) = list_line(text, blocks, head)?;
    (head > bounds.text_start && head <= bounds.text_end).then(|| {
        Plan::new(
            vec![(bounds.text_start..head, String::new())],
            bounds.text_start,
        )
    })
}

/// Backspace inside an empty `[[]]` or `(())` removes both pairs at once.
pub fn delete_empty_link(text: &str, head: usize) -> Option<Plan> {
    let around = text.get(head.checked_sub(2)?..head + 2)?;
    matches!(around, "[[]]" | "(())")
        .then(|| Plan::new(vec![(head - 2..head + 2, String::new())], head - 2))
}

/// Enter right before the `]]` or `))` closing a link on the caret's line steps past them
/// instead of splitting the link: the caret position after the brackets.
pub fn step_out_of_link(text: &str, head: usize) -> Option<usize> {
    let before = &text[line_start(text, head)..head];
    let after = &text[head..];
    let open = |opening: &str, closing: char| {
        before
            .rfind(opening)
            .is_some_and(|at| !before[at + 2..].contains(closing))
    };
    let closes = if after.starts_with("]]") {
        open("[[", ']')
    } else {
        after.starts_with("))") && open("((", ')')
    };
    closes.then_some(head + 2)
}

#[cfg(test)]
#[path = "model_tests.rs"]
mod tests;
