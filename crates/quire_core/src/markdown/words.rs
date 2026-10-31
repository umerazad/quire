//! The words a reader sees on a page, for its header and status bar.

use regex::Regex;
use std::sync::LazyLock;

static STAMPS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[ \t]*(?:➕|📅|✅)[ \t]*(?:\[\[[^\]\n]*\]\]|[0-9]{4}-[0-9]{2}-[0-9]{2})").unwrap()
});
static BLOCK_IDS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)(?:^|[ \t])\^[A-Za-z0-9_-]+[ \t]*$").unwrap());
static LIST_MARKERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^[ \t]*(?:[-+*]|[0-9]+[.)])[ \t]+").unwrap());
static CHECKBOXES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\[[ xX]\]|\{\{\[\[(?:TODO|DONE)\]\]\}\}").unwrap());
static LINE_MARKS: LazyLock<Regex> = LazyLock::new(|| {
    let word = format!("[^{}*", &super::JS_SPACE[1..]);
    Regex::new(&format!(
        r"(?m)^[ \t]*(?:#{{1,6}}|>+|```{word}|~~~{word})[ \t]*"
    ))
    .unwrap()
});
static LINK_TARGETS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\]\([^)\n]*\)").unwrap());
static PUNCTUATION: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\[\](){}*_`~=^|]").unwrap());
static WORDLIKE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[\p{L}\p{N}]").unwrap());

/// Counts the words a reader sees: front matter, Markdown syntax, list markers, checkboxes,
/// block IDs, and task date stamps do not count, as the reference `countWords` decides.
pub fn count_words(text: &str) -> usize {
    let visible = &text[super::frontmatter_length(text)..];
    let visible = [
        &*STAMPS,
        &*BLOCK_IDS,
        &*LIST_MARKERS,
        &*CHECKBOXES,
        &*LINE_MARKS,
        &*LINK_TARGETS,
        &*PUNCTUATION,
    ]
    .into_iter()
    .fold(visible.to_owned(), |text, pattern| {
        pattern.replace_all(&text, " ").into_owned()
    });
    visible
        .split(super::js_space)
        .filter(|word| WORDLIKE.is_match(word))
        .count()
}

/// "1 word", "0 words": a count with its noun, plural unless the count is one.
pub fn plural(count: usize, noun: &str) -> String {
    if count == 1 {
        format!("{count} {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

/// Minutes to read `words` at 200 words a minute, rounded up, as the reference header shows.
pub fn reading_minutes(words: usize) -> usize {
    words.div_ceil(200)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_the_words_a_reader_sees_not_the_markdown_around_them() {
        assert_eq!(count_words("- "), 0);
        assert_eq!(count_words("- hello"), 1);
        assert_eq!(
            count_words(
                "---\ntitle: Notes\n---\n# Plan\n- [ ] Call [[Ada Lovelace]] about #ideas ➕ [[2026-10-29]] ^b-1\n  - {{[[TODO]]}} read [the doc](https://example.com) **now**\n> quoted line\n",
            ),
            12
        );
        assert_eq!(count_words("اردو متن اور English"), 4);
        assert_eq!(count_words("1. first\n2) second ✅ 2026-10-31"), 2);
    }

    #[test]
    fn pluralizes_counts_and_rounds_reading_time_up() {
        assert_eq!(plural(1, "word"), "1 word");
        assert_eq!(plural(0, "word"), "0 words");
        assert_eq!(plural(3, "block"), "3 blocks");
        assert_eq!(reading_minutes(0), 0);
        assert_eq!(reading_minutes(1), 1);
        assert_eq!(reading_minutes(200), 1);
        assert_eq!(reading_minutes(201), 2);
    }
}
