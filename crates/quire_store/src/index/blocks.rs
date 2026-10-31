//! Block lookups for references typed in the editor: choices for `((` completion and the
//! referenced block's text for inline previews and embeds.

use super::{
    QueryIndex, block_key,
    query::{PAGE_COLUMNS, PageRef, PageRow, page_ref},
};
use anyhow::Result;

/// A block offered after `((`. `key` names the block when it has no ID yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockChoice {
    pub source: PageRef,
    pub key: String,
    pub id: Option<String>,
    pub text: String,
}

/// The block a block reference resolves to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BlockPreview {
    pub source: PageRef,
    pub id: String,
    pub text: String,
    /// Whether the block has nested blocks, which a full embed shows.
    pub children: bool,
}

/// The page a `Page#Heading` link names, with the heading block when the page has one by that
/// text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeadingTarget {
    pub page: PageRef,
    /// The heading block's key and its start in the page's text.
    pub heading: Option<(String, u32)>,
}

/// The opening of the page a link names, as its hover preview shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PagePreview {
    pub page: PageRef,
    pub lines: Vec<String>,
}

/// The heading a link names after its first `#`, when it names one and not a block (`#^id`).
pub fn heading_of(target: &str) -> Option<&str> {
    let (_, heading) = target.split_once('#')?;
    let heading = heading.trim();
    (!heading.is_empty() && !heading.starts_with('^')).then_some(heading)
}

/// A heading block's text without its `#` marks, as rt's outline lists it.
fn heading_text(text: &str) -> &str {
    text.trim_start_matches('#').trim()
}

type ChoiceRow = (PageRow, (String, Option<String>, String));

/// How many matching blocks one query ranks before taking the best `limit`.
const RANKED_CANDIDATES: usize = 5000;

/// How well a block's lowercased text matches a typed query, lower first: the whole phrase
/// opening the text, the phrase at a word start, the phrase anywhere, every word at a word
/// start, then every word anywhere. Shorter texts break ties.
fn rank(text: &str, phrase: &str, words: &[String]) -> (u8, usize) {
    let at_word_start = |needle: &str| {
        text.match_indices(needle).any(|(at, _)| {
            text[..at]
                .chars()
                .next_back()
                .is_none_or(|before| !before.is_alphanumeric())
        })
    };
    if phrase.is_empty() {
        return (0, 0);
    }
    let tier = if text.starts_with(phrase) {
        0
    } else if at_word_start(phrase) {
        1
    } else if text.contains(phrase) {
        2
    } else if words.iter().all(|word| at_word_start(word)) {
        3
    } else {
        4
    };
    (tier, text.chars().count())
}

impl QueryIndex {
    /// Non-code blocks whose text holds every word of `query`, ignoring case, best match first
    /// (see `rank`), then by page path and block order, at most `limit`. Unsaved text of open
    /// pages counts. An empty query lists blocks in page order.
    pub fn block_choices(&self, query: &str, limit: usize) -> Result<Vec<BlockChoice>> {
        self.refresh()?;
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let phrase = words.join(" ");
        let first = words.first().cloned().unwrap_or_default();
        let rows: Vec<ChoiceRow> = self.connection.select_bound(&format!(
            "SELECT {PAGE_COLUMNS},b.key,b.id,b.text FROM live_blocks b JOIN live_pages p ON p.revision=b.revision
             WHERE p.deleted=0 AND b.kind!='code' AND b.text!='' AND instr(lower(b.text),?)>0
             ORDER BY p.path,b.ordinal"
        ))?(first.as_str())?;
        let candidates = if words.is_empty() {
            limit
        } else {
            RANKED_CANDIDATES
        };
        let mut ranked: Vec<_> = rows
            .into_iter()
            .filter_map(|row| {
                let lower = row.1.2.to_lowercase();
                words
                    .iter()
                    .all(|word| lower.contains(word.as_str()))
                    .then(|| (rank(&lower, &phrase, &words), row))
            })
            .take(candidates)
            .collect();
        ranked.sort_by_key(|(rank, _)| *rank);
        ranked
            .into_iter()
            .take(limit)
            .map(|(_, (page, (key, id, text)))| {
                Ok(BlockChoice {
                    source: page_ref(page)?,
                    key,
                    id,
                    text,
                })
            })
            .collect()
    }

    /// The page `target` names and, for `Page#Heading`, the first heading on it whose text
    /// matches ignoring case and `#` marks. Unsaved text of open pages counts.
    pub fn find_heading(&self, target: &str) -> Result<Option<HeadingTarget>> {
        self.refresh()?;
        let Some(page) = self.resolve_page(target)? else {
            return Ok(None);
        };
        let Some(wanted) = heading_of(target).map(str::to_lowercase) else {
            return Ok(Some(HeadingTarget {
                page,
                heading: None,
            }));
        };
        let rows: Vec<(String, u32, String)> = self.connection.select_bound(
            "SELECT key,start,text FROM live_blocks WHERE revision=? AND kind='heading' ORDER BY ordinal",
        )?(page.revision.as_str())?;
        let heading = rows
            .into_iter()
            .find(|(_, _, text)| heading_text(text).to_lowercase() == wanted)
            .map(|(key, start, _)| (key, start));
        Ok(Some(HeadingTarget { page, heading }))
    }

    /// The page `target` names with the text of its first `limit` non-empty, non-code blocks,
    /// heading marks removed, for a hover preview. For `Page#Heading` the preview starts at that
    /// heading when the page has it. Unsaved text of open pages counts.
    pub fn page_preview(&self, target: &str, limit: usize) -> Result<Option<PagePreview>> {
        let Some(found) = self.find_heading(target)? else {
            return Ok(None);
        };
        let from = found.heading.as_ref().map_or(0, |(_, start)| *start);
        let rows: Vec<(String, String)> = self.connection.select_bound(
            "SELECT kind,text FROM live_blocks WHERE revision=? AND start>=? AND kind!='code' AND text!='' ORDER BY ordinal",
        )?((found.page.revision.as_str(), from))?;
        let lines = rows
            .into_iter()
            .map(|(kind, text)| match kind.as_str() {
                "heading" => heading_text(&text).to_owned(),
                _ => text,
            })
            .take(limit)
            .collect();
        Ok(Some(PagePreview {
            page: found.page,
            lines,
        }))
    }

    /// The block `target` names under the reference `resolve` rules, with its text.
    pub fn block_preview(&self, target: &str) -> Result<Option<BlockPreview>> {
        self.refresh()?;
        let Some(source) = self.resolve_block_page(target)? else {
            return Ok(None);
        };
        let id = block_key(target).to_owned();
        let rows: Vec<(String, i64)> = self.connection.select_bound(
            "SELECT b.text,(SELECT count(*) FROM live_blocks c WHERE c.revision=b.revision AND c.parent=b.key)
             FROM live_blocks b WHERE b.revision=? AND b.id=?",
        )?((source.revision.as_str(), id.as_str()))?;
        Ok(rows
            .into_iter()
            .next()
            .map(|(text, children)| BlockPreview {
                source,
                id,
                text,
                children: children > 0,
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::rank;

    #[test]
    fn ranks_phrase_openings_then_word_starts_then_anywhere() {
        let words =
            |query: &str| -> Vec<String> { query.split_whitespace().map(str::to_owned).collect() };
        let tier = |text: &str, query: &str| rank(text, query, &words(query)).0;
        assert_eq!(tier("plan the launch", "plan the"), 0);
        assert_eq!(tier("we plan the launch", "plan the"), 1);
        assert_eq!(tier("(plan the) launch", "plan the"), 1);
        assert_eq!(tier("replan the launch", "plan the"), 2);
        assert_eq!(tier("the launch plan", "plan the"), 3);
        assert_eq!(tier("the replanning", "plan the"), 4);
        assert_eq!(rank("anything", "", &[]), (0, 0));
        assert!(rank("plan", "plan", &words("plan")) < rank("planet", "plan", &words("plan")));
    }
}
