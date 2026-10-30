//! Bulk tag rename and delete, per page. Ported from rt `957da33d4766` `src/core/tag-edits.ts`.
//! Callers supply the page's tag tokens from native Markdown analysis, so tags inside code and
//! links are already excluded. Text is the editor's line-break-normalized text.

use crate::markdown::{TagToken, frontmatter_length, js_space};
use regex::Regex;
use std::sync::LazyLock;
use unicode_normalization::UnicodeNormalization;

pub const INVALID_NAME: &str =
    "Use a tag name of letters, digits, _ and -, with / between nested names, such as work/home.";
pub const NOTHING_CHOSEN: &str = "Select one or more tags first.";
pub const PAGE_LIMIT: usize = 1000;

/// JavaScript's `.` outside the `s` flag.
const DOT: &str = r"[^\n\r\u{2028}\u{2029}]";

static TAG_NAME: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\p{L}\p{N}_][\p{L}\p{N}_-]*(?:/[\p{L}\p{N}_][\p{L}\p{N}_-]*)*$").unwrap()
});
static MARKER_PREFIX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"^[ \t]*(?:[-*+]|[0-9]+[.)])?[ \t]*(?:\[{DOT}\][ \t]*)?$"
    ))
    .unwrap()
});
static FIELD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"(?m)^([tT][aA][gG][sS]?)[ \t]*:[ \t]*({DOT}*)((?:\r?\n[ \t]*-[ \t]+{DOT}*)*)"
    ))
    .unwrap()
});
static LIST_ITEM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[ \t]*-[ \t]+").unwrap());
static INLINE_SPLIT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(
        r"[,{}]+",
        &crate::markdown::JS_SPACE[1..crate::markdown::JS_SPACE.len() - 1]
    ))
    .unwrap()
});

/// The reference `tagKey`: no leading `#`, NFC, lowercase.
pub fn tag_key(tag: &str) -> String {
    tag.strip_prefix('#')
        .unwrap_or(tag)
        .nfc()
        .collect::<String>()
        .to_lowercase()
}

/// Which tags change and what they become.
#[derive(Clone, Debug)]
pub struct Retag {
    keys: Vec<String>,
    rename: Option<String>,
}

impl Retag {
    /// Renames `chosen` (and the tags nested under them) to `rename`, or deletes exactly the
    /// chosen tags when `rename` is None.
    pub fn new(chosen: &[impl AsRef<str>], rename: Option<&str>) -> Result<Self, &'static str> {
        if chosen.is_empty() {
            return Err(NOTHING_CHOSEN);
        }
        let rename = match rename {
            Some(name) => {
                let name = name.trim_matches(js_space);
                let name = name.strip_prefix('#').unwrap_or(name);
                if !TAG_NAME.is_match(name) {
                    return Err(INVALID_NAME);
                }
                Some(name.to_owned())
            }
            None => None,
        };
        let mut keys: Vec<String> = chosen.iter().map(|tag| tag_key(tag.as_ref())).collect();
        keys.sort_by_key(|key| std::cmp::Reverse(key.encode_utf16().count()));
        Ok(Self { keys, rename })
    }

    /// The new name without `#`: `Some(None)` deletes, `None` leaves the tag alone.
    pub fn change(&self, name: &str) -> Option<Option<String>> {
        let key = tag_key(name);
        let Some(rename) = &self.rename else {
            return self.keys.contains(&key).then_some(None);
        };
        let parent = self.keys.iter().find(|item| nested(&key, item))?;
        let bare: Vec<u16> = name
            .strip_prefix('#')
            .unwrap_or(name)
            .encode_utf16()
            .collect();
        let skip = parent.encode_utf16().count().min(bare.len());
        Some(Some(format!(
            "{rename}{}",
            String::from_utf16_lossy(&bare[skip..])
        )))
    }

    /// Whether a tag key could change, for choosing candidate pages from an index.
    pub fn touches(&self, key: &str) -> bool {
        self.keys.iter().any(|item| {
            if self.rename.is_some() {
                nested(key, item)
            } else {
                key == item
            }
        })
    }
}

/// Whether `key` is `parent` or a tag nested under it.
fn nested(key: &str, parent: &str) -> bool {
    key.strip_prefix(parent)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

/// One replacement shown in the preview.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagChange {
    pub before: String,
    pub after: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageTagEdit {
    pub text: String,
    pub changes: Vec<TagChange>,
}

struct Edit {
    range: std::ops::Range<usize>,
    insert: String,
}

/// Rewrites one page, or returns None when nothing changes.
pub fn edit_page(text: &str, tags: &[TagToken], retag: &Retag) -> Option<PageTagEdit> {
    let mut edits = Vec::new();
    for token in tags {
        let Some(renamed) = retag.change(&token.text) else {
            continue;
        };
        if let Some(renamed) = renamed {
            edits.push(Edit {
                range: token.range.clone(),
                insert: format!("#{renamed}"),
            });
            continue;
        }
        // A deleted tag takes the space after it, or else the space before it unless that space
        // follows a list marker or checkbox.
        let line_start = text[..token.range.start]
            .rfind('\n')
            .map_or(0, |index| index + 1);
        let prefix = &text[line_start..token.range.start];
        let after = matches!(text.as_bytes().get(token.range.end), Some(b' ' | b'\t'));
        let before = !after && prefix.ends_with([' ', '\t']) && !MARKER_PREFIX.is_match(prefix);
        edits.push(Edit {
            range: token.range.start - usize::from(before)..token.range.end + usize::from(after),
            insert: String::new(),
        });
    }
    edits.extend(frontmatter_change(text, retag));
    if edits.is_empty() {
        return None;
    }
    edits.sort_by_key(|edit| edit.range.start);
    let changes = edits
        .iter()
        .map(|edit| TagChange {
            before: text[edit.range.clone()].trim_matches(js_space).to_owned(),
            after: edit.insert.trim_matches(js_space).to_owned(),
        })
        .collect();
    let mut next = text.to_owned();
    for edit in edits.iter().rev() {
        next.replace_range(edit.range.clone(), &edit.insert);
    }
    Some(PageTagEdit {
        text: next,
        changes,
    })
}

/// Front matter `tags` as Obsidian writes them: `[a, b]`, `a, b`, or a `- a` list, rewritten in
/// the same form.
fn frontmatter_change(text: &str, retag: &Retag) -> Option<Edit> {
    let length = frontmatter_length(text);
    if length == 0 {
        return None;
    }
    let field = FIELD.captures(&text[..length])?;
    let inline = field[2].trim_matches(js_space);
    let values: Vec<&str> = if inline.is_empty() {
        field[3]
            .split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line))
            .filter(|line| !line.is_empty())
            .map(|line| {
                line[LIST_ITEM.find(line).map_or(0, |marker| marker.end())..].trim_matches(js_space)
            })
            .collect()
    } else {
        let inner = inline.strip_prefix('[').unwrap_or(inline);
        let inner = inner.strip_suffix(']').unwrap_or(inner);
        INLINE_SPLIT
            .split(inner)
            .filter(|value| !value.is_empty())
            .collect()
    };
    let mut changed = false;
    let mut next: Vec<String> = Vec::new();
    for value in values {
        let bare = value.strip_prefix(['"', '\'']).unwrap_or(value);
        let bare = bare.strip_suffix(['"', '\'']).unwrap_or(bare);
        let bare = bare.strip_prefix('#').unwrap_or(bare);
        match retag.change(bare) {
            None => next.push(value.to_owned()),
            Some(renamed) => {
                changed = true;
                next.extend(renamed);
            }
        }
    }
    if !changed {
        return None;
    }
    let mut unique: Vec<String> = Vec::new();
    for value in next {
        if !unique.contains(&value) {
            unique.push(value);
        }
    }
    let newline = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let key = &field[1];
    let written = if unique.is_empty() {
        format!("{key}: []")
    } else if !inline.is_empty() {
        if inline.starts_with('[') {
            format!("{key}: [{}]", unique.join(", "))
        } else {
            format!("{key}: {}", unique.join(", "))
        }
    } else {
        format!(
            "{key}:{}",
            unique
                .iter()
                .map(|value| format!("{newline}  - {value}"))
                .collect::<String>()
        )
    };
    let whole = field.get(0)?;
    Some(Edit {
        range: whole.range(),
        insert: written,
    })
}

/// Refuses an edit that changes more pages than one apply may write.
pub fn check_page_count(pages: usize) -> Result<(), String> {
    if pages > PAGE_LIMIT {
        return Err(format!(
            "This changes {pages} pages; at most {PAGE_LIMIT} can change at once. Select fewer tags and repeat."
        ));
    }
    Ok(())
}
