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

type ChoiceRow = (PageRow, (String, Option<String>, String));

impl QueryIndex {
    /// Non-code blocks whose text holds every word of `query`, ignoring case, by page path and
    /// then block order, at most `limit`. Unsaved text of open pages counts.
    pub fn block_choices(&self, query: &str, limit: usize) -> Result<Vec<BlockChoice>> {
        self.refresh()?;
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        let first = words.first().cloned().unwrap_or_default();
        let rows: Vec<ChoiceRow> = self.connection.select_bound(&format!(
            "SELECT {PAGE_COLUMNS},b.key,b.id,b.text FROM live_blocks b JOIN live_pages p ON p.revision=b.revision
             WHERE p.deleted=0 AND b.kind!='code' AND b.text!='' AND instr(lower(b.text),?)>0
             ORDER BY p.path,b.ordinal"
        ))?(first.as_str())?;
        let mut choices = Vec::new();
        for (page, (key, id, text)) in rows {
            if choices.len() == limit {
                break;
            }
            let lower = text.to_lowercase();
            if words.iter().all(|word| lower.contains(word.as_str())) {
                choices.push(BlockChoice {
                    source: page_ref(page)?,
                    key,
                    id,
                    text,
                });
            }
        }
        Ok(choices)
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
