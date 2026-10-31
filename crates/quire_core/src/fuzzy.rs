//! Fuzzy matching for short names, ported from the reference `fuzzy.ts`: the query's characters
//! must appear in order. Matches at the start, at word starts, and in runs score higher; gaps
//! cost a little. Positions index the name's characters, for highlighting.

use crate::markdown::js_space;

#[derive(Clone, Debug, PartialEq)]
pub struct FuzzyMatch {
    pub score: f64,
    pub positions: Vec<usize>,
}

fn lower(c: char) -> String {
    c.to_lowercase().collect()
}

fn is_boundary(c: char) -> bool {
    js_space(c) || matches!(c, '-' | '_' | '/' | '.' | ':' | '(' | '[')
}

fn bonus(chars: &[char], index: usize) -> f64 {
    if index == 0 {
        return 10.;
    }
    let (before, here) = (chars[index - 1], chars[index]);
    if is_boundary(before) {
        8.
    } else if lower(before) == before.to_string() && lower(here) != here.to_string() {
        6.
    } else {
        0.
    }
}

pub fn fuzzy_match(query: &str, text: &str) -> Option<FuzzyMatch> {
    let raw: Vec<char> = query.chars().filter(|c| !js_space(*c)).collect();
    let q: Vec<String> = raw.iter().map(|c| lower(*c)).collect();
    let chars: Vec<char> = text.chars().collect();
    let lowered: Vec<String> = chars.iter().map(|c| lower(*c)).collect();
    let n = chars.len();
    if q.is_empty() {
        return Some(FuzzyMatch {
            score: 0.,
            positions: Vec::new(),
        });
    }
    if q.len() > n {
        return None;
    }
    let mut best = vec![vec![f64::NEG_INFINITY; n]; q.len()];
    let mut from = vec![vec![usize::MAX; n]; q.len()];
    for i in 0..q.len() {
        for j in i..n {
            if lowered[j] != q[i] {
                continue;
            }
            let own = 1. + bonus(&chars, j) + if chars[j] == raw[i] { 1. } else { 0. };
            if i == 0 {
                best[i][j] = own - j.min(5) as f64;
                continue;
            }
            for k in i - 1..j {
                if best[i - 1][k] == f64::NEG_INFINITY {
                    continue;
                }
                let run = if k + 1 == j {
                    5.
                } else {
                    -((j - k - 1).min(3) as f64)
                };
                let value = best[i - 1][k] + own + run;
                if value > best[i][j] {
                    best[i][j] = value;
                    from[i][j] = k;
                }
            }
        }
    }
    let last = q.len() - 1;
    let end = (0..n).filter(|j| best[last][*j] > f64::NEG_INFINITY).fold(
        None,
        |end: Option<usize>, j| match end {
            Some(end) if best[last][j] <= best[last][end] => Some(end),
            _ => Some(j),
        },
    )?;
    let mut positions = vec![0; q.len()];
    let mut j = end;
    for i in (0..=last).rev() {
        positions[i] = j;
        j = from[i][j];
    }
    Some(FuzzyMatch {
        score: best[last][end] - (n - positions.len()) as f64 * 0.05,
        positions,
    })
}

/// Indexes of the items whose name matches with their matches, best first; ties keep the given
/// order.
pub fn fuzzy_filter<T>(
    items: &[T],
    query: &str,
    name: impl Fn(&T) -> &str,
) -> Vec<(usize, FuzzyMatch)> {
    let mut matched: Vec<(usize, FuzzyMatch)> = items
        .iter()
        .enumerate()
        .filter_map(|(order, item)| Some((order, fuzzy_match(query, name(item))?)))
        .collect();
    matched.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0)));
    matched
}
