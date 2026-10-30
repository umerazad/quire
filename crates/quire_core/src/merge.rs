//! Three-way page text merge, ported from `rt` `src/core/merge.ts` at 0.9.57.
//! Two computers merging the same versions must produce the same text, so every
//! step mirrors the TypeScript and Go implementations exactly. Text is a slice of
//! JavaScript UTF-16 code units, so lone surrogates survive unchanged.

use std::{borrow::Cow, collections::HashSet};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MergeResult {
    pub text: Vec<u16>,
    pub overlaps: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Region {
    pub a_start: usize,
    pub a_end: usize,
    pub b_start: usize,
    pub b_end: usize,
}

impl Region {
    fn inserts(&self) -> bool {
        self.a_start == self.a_end
    }
}

enum Chunk<T> {
    Same(Vec<T>),
    Both {
        base: Vec<T>,
        first: Vec<T>,
        second: Vec<T>,
    },
}

const MAX_EDITS: isize = 2000;
const NEWLINE: u16 = b'\n' as u16;
const NEWLINE_TOKEN: &[u16] = &[NEWLINE];

fn is_blank(unit: u16) -> bool {
    unit == u16::from(b' ') || unit == u16::from(b'\t')
}

/// Matched index pairs of a shortest edit script from a to b (Myers).
fn matches<T: PartialEq>(a: &[T], b: &[T]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len() as isize, b.len() as isize);
    let max = n + m;
    if max == 0 {
        return Vec::new();
    }
    let offset = max + 1;
    let mut v = vec![0isize; (2 * max + 3) as usize];
    let mut trace: Vec<Vec<isize>> = Vec::new();
    let mut found = None;
    for d in 0..=max.min(MAX_EDITS) {
        let mut k = -d;
        while k <= d {
            let at = |k: isize| v[(offset + k) as usize];
            let mut x = if k == -d || (k != d && at(k - 1) < at(k + 1)) {
                at(k + 1)
            } else {
                at(k - 1) + 1
            };
            let mut y = x - k;
            while x < n && y < m && a[x as usize] == b[y as usize] {
                x += 1;
                y += 1;
            }
            v[(offset + k) as usize] = x;
            if x >= n && y >= m {
                found = Some(d);
                break;
            }
            k += 2;
        }
        trace.push(v[(offset - d) as usize..=(offset + d) as usize].to_vec());
        if found.is_some() {
            break;
        }
    }
    let Some(found) = found else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    let (mut x, mut y) = (n, m);
    for d in (1..=found).rev() {
        let previous = &trace[(d - 1) as usize];
        let at = |k: isize| previous[(k + d - 1) as usize];
        let k = x - y;
        let down = k == -d || (k != d && at(k - 1) < at(k + 1));
        let previous_k = if down { k + 1 } else { k - 1 };
        let previous_x = at(previous_k);
        let previous_y = previous_x - previous_k;
        while x > previous_x && y > previous_y {
            x -= 1;
            y -= 1;
            pairs.push((x as usize, y as usize));
        }
        x = previous_x;
        y = previous_y;
    }
    while x > 0 && y > 0 {
        x -= 1;
        y -= 1;
        pairs.push((x as usize, y as usize));
    }
    pairs.reverse();
    pairs
}

/// The runs where a and b differ, in order.
pub fn diff_regions<T: PartialEq>(a: &[T], b: &[T]) -> Vec<Region> {
    let mut start = 0;
    while start < a.len() && start < b.len() && a[start] == b[start] {
        start += 1;
    }
    let (mut end_a, mut end_b) = (a.len(), b.len());
    while end_a > start && end_b > start && a[end_a - 1] == b[end_b - 1] {
        end_a -= 1;
        end_b -= 1;
    }
    let mut pairs: Vec<_> = matches(&a[start..end_a], &b[start..end_b])
        .into_iter()
        .map(|(i, j)| (i + start, j + start))
        .collect();
    pairs.push((end_a, end_b));
    let mut regions = Vec::new();
    let (mut i, mut j) = (start, start);
    for (pi, pj) in pairs {
        if pi > i || pj > j {
            regions.push(Region {
                a_start: i,
                a_end: pi,
                b_start: j,
                b_end: pj,
            });
        }
        i = pi + 1;
        j = pj + 1;
    }
    regions
}

/// `Array.prototype.slice` with JavaScript's negative and out-of-range handling.
fn js_slice<T: Clone>(items: &[T], begin: isize, end: isize) -> Vec<T> {
    let length = items.len() as isize;
    let clamp = |index: isize| {
        if index < 0 {
            (length + index).max(0)
        } else {
            index.min(length)
        }
    };
    let (begin, end) = (clamp(begin), clamp(end));
    if end <= begin {
        Vec::new()
    } else {
        items[begin as usize..end as usize].to_vec()
    }
}

/// Splits base into stretches both sides kept and stretches either changed.
fn merge3<T: PartialEq + Clone>(base: &[T], first: &[T], second: &[T]) -> Vec<Chunk<T>> {
    let mut hunks: Vec<(usize, Region)> = diff_regions(base, first)
        .into_iter()
        .map(|region| (0, region))
        .chain(
            diff_regions(base, second)
                .into_iter()
                .map(|region| (1, region)),
        )
        .collect();
    hunks.sort_by_key(|(side, region)| (region.a_start, !region.inserts(), *side));
    let sides = [first, second];
    let mut chunks = Vec::new();
    let mut shift = [0isize; 2];
    let mut at = 0;
    let mut i = 0;
    while i < hunks.len() {
        let group_start = hunks[i].1.a_start;
        let mut group_end = hunks[i].1.a_end;
        let mut group = vec![hunks[i]];
        i += 1;
        while i < hunks.len()
            && (hunks[i].1.a_start < group_end
                || (hunks[i].1.a_start == group_start && hunks[i].1.inserts()))
        {
            group_end = group_end.max(hunks[i].1.a_end);
            group.push(hunks[i]);
            i += 1;
        }
        if group_start > at {
            chunks.push(Chunk::Same(base[at..group_start].to_vec()));
        }
        let texts = [0, 1].map(|s| {
            let own: Vec<_> = group.iter().filter(|(side, _)| *side == s).collect();
            let (start, end) = (group_start as isize, group_end as isize);
            match (own.first(), own.last()) {
                (Some((_, first)), Some((_, last))) => {
                    let start_shift = first.b_start as isize - first.a_start as isize;
                    let end_shift = last.b_end as isize - last.a_end as isize;
                    shift[s] = end_shift;
                    js_slice(sides[s], start + start_shift, end + end_shift)
                }
                _ => {
                    let from = start + shift[s];
                    js_slice(sides[s], from, from + end - start)
                }
            }
        });
        let changed = [0, 1].map(|s| group.iter().any(|(side, _)| *side == s));
        let [first_text, second_text] = texts;
        chunks.push(if !changed[1] {
            Chunk::Same(first_text)
        } else if !changed[0] || first_text == second_text {
            Chunk::Same(if changed[0] { first_text } else { second_text })
        } else {
            Chunk::Both {
                base: base[group_start..group_end].to_vec(),
                first: first_text,
                second: second_text,
            }
        });
        at = group_end;
    }
    if at < base.len() {
        chunks.push(Chunk::Same(base[at..].to_vec()));
    }
    chunks
}

/// Words, runs of spaces and tabs, and line ends, so a merge inside a line works on whole words.
fn tokens<'a>(lines: &[&'a [u16]]) -> Vec<&'a [u16]> {
    let mut out = Vec::new();
    for line in lines {
        let mut start = 0;
        while start < line.len() {
            let blank = is_blank(line[start]);
            let end = line[start..]
                .iter()
                .position(|unit| is_blank(*unit) != blank)
                .map_or(line.len(), |length| start + length);
            out.push(&line[start..end]);
            start = end;
        }
        out.push(NEWLINE_TOKEN);
    }
    out
}

#[derive(Default)]
struct Lines<'a> {
    lines: Vec<Cow<'a, [u16]>>,
    repeats: Vec<bool>,
}

/// Both sides changed the same lines: merge word by word, or give up.
fn resolve<'a>(base: &[&'a [u16]], first: &[&'a [u16]], second: &[&'a [u16]]) -> Option<Lines<'a>> {
    let chunks = merge3(&tokens(base), &tokens(first), &tokens(second));
    let mut text = Vec::new();
    for chunk in chunks {
        match chunk {
            Chunk::Same(items) => items.iter().for_each(|item| text.extend_from_slice(item)),
            Chunk::Both { .. } => return None,
        }
    }
    if !(text.is_empty() || text.last() == Some(&NEWLINE)) {
        return None;
    }
    let mut lines: Vec<_> = text
        .split(|unit| *unit == NEWLINE)
        .map(|line| Cow::Owned(line.to_vec()))
        .collect();
    lines.pop();
    let repeats = vec![false; lines.len()];
    Some(Lines { lines, repeats })
}

fn keep_both<'a>(first: &[&'a [u16]], second: &[&'a [u16]]) -> Lines<'a> {
    let mut out = Lines::default();
    let mut push = |line: &'a [u16], repeat: bool| {
        out.lines.push(Cow::Borrowed(line));
        out.repeats.push(repeat);
    };
    let (mut i, mut j) = (0, 0);
    let end = Region {
        a_start: first.len(),
        a_end: first.len(),
        b_start: second.len(),
        b_end: second.len(),
    };
    for region in diff_regions(first, second).into_iter().chain([end]) {
        while i < region.a_start {
            push(first[i], false);
            i += 1;
            j += 1;
        }
        while i < region.a_end {
            push(first[i], false);
            i += 1;
        }
        while j < region.b_end {
            push(second[j], true);
            j += 1;
        }
    }
    out
}

/// Matches `/(?:^|[ \t])\^([a-zA-Z0-9_-]+)[ \t]*$/`: the match start and the block ID.
fn block_id(line: &[u16]) -> Option<(usize, &[u16])> {
    let caret = line.iter().rposition(|unit| *unit == u16::from(b'^'))?;
    let tail = &line[caret + 1..];
    let length = tail
        .iter()
        .position(|unit| {
            !u8::try_from(*unit)
                .is_ok_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
        })
        .unwrap_or(tail.len());
    if length == 0 || !tail[length..].iter().all(|unit| is_blank(*unit)) {
        return None;
    }
    let start = match caret {
        0 => 0,
        _ if is_blank(line[caret - 1]) => caret - 1,
        _ => return None,
    };
    Some((start, &tail[..length]))
}

fn lines(text: &[u16]) -> Vec<&[u16]> {
    text.split(|unit| *unit == NEWLINE).collect()
}

/// Merges the edits from base to first and from base to second.
pub fn merge_text(base: &[u16], first: &[u16], second: &[u16]) -> MergeResult {
    let unchanged = |text: &[u16]| MergeResult {
        text: text.to_vec(),
        overlaps: 0,
    };
    if first == second || base == second {
        return unchanged(first);
    }
    if base == first {
        return unchanged(second);
    }
    let (base_lines, first_lines, second_lines) = (lines(base), lines(first), lines(second));
    let mut merged = Lines::default();
    let mut overlaps = 0;
    for chunk in merge3(&base_lines, &first_lines, &second_lines) {
        let resolved = match chunk {
            Chunk::Same(items) => Lines {
                repeats: vec![false; items.len()],
                lines: items.into_iter().map(Cow::Borrowed).collect(),
            },
            Chunk::Both {
                base,
                first,
                second,
            } => resolve(&base, &first, &second).unwrap_or_else(|| {
                if !base.is_empty() {
                    overlaps += 1;
                }
                keep_both(&first, &second)
            }),
        };
        merged.lines.extend(resolved.lines);
        merged.repeats.extend(resolved.repeats);
    }
    let mut ids = HashSet::<Vec<u16>>::new();
    for (line, repeat) in merged.lines.iter().zip(&merged.repeats) {
        if !repeat && let Some((_, id)) = block_id(line) {
            ids.insert(id.to_vec());
        }
    }
    for (line, repeat) in merged.lines.iter_mut().zip(&merged.repeats) {
        if !repeat {
            continue;
        }
        let Some((start, id)) = block_id(line) else {
            continue;
        };
        if ids.contains(id) {
            let mut prefix = line[..start].to_vec();
            while prefix.last().is_some_and(|unit| is_blank(*unit)) {
                prefix.pop();
            }
            *line = Cow::Owned(prefix);
        } else {
            ids.insert(id.to_vec());
        }
    }
    MergeResult {
        text: merged.lines.join(&NEWLINE),
        overlaps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(text: &str) -> Vec<u16> {
        text.encode_utf16().collect()
    }

    #[test]
    fn finds_the_shortest_edit_between_two_line_lists() {
        assert_eq!(
            diff_regions(&["a", "b", "c"], &["a", "x", "c"]),
            [Region {
                a_start: 1,
                a_end: 2,
                b_start: 1,
                b_end: 2
            }]
        );
        assert!(diff_regions(&["a", "b"], &["a", "b"]).is_empty());
        assert_eq!(
            diff_regions::<&str>(&[], &["a"]),
            [Region {
                a_start: 0,
                a_end: 0,
                b_start: 0,
                b_end: 1
            }]
        );
    }

    #[test]
    fn block_ids_match_the_source_pattern() {
        for (line, expected) in [
            ("- call ^b-1", Some((6, "b-1"))),
            ("^id", Some((0, "id"))),
            (" ^id \t", Some((0, "id"))),
            ("- x^id", None),
            ("- x ^id ^", None),
            ("- x ^i.d", None),
            ("- x ^", None),
        ] {
            let line = units(line);
            assert_eq!(
                block_id(&line).map(|(start, id)| (start, String::from_utf16(id).unwrap())),
                expected.map(|(start, id)| (start, id.to_owned()))
            );
        }
    }

    #[test]
    fn lone_surrogates_merge_as_code_units() {
        let base = [0xd800, NEWLINE, u16::from(b'a')];
        let first = [0xd800, NEWLINE, u16::from(b'b')];
        let second = [0xdc00, NEWLINE, u16::from(b'a')];
        assert_eq!(
            merge_text(&base, &first, &second).text,
            [0xdc00, NEWLINE, u16::from(b'b')]
        );
    }
}
