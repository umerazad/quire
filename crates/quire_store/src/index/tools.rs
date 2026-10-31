//! Queries the AI notebook tools (NR29) need beyond the reference store's lookups: every live
//! block with its page, one page's blocks, and when each page last changed. Unsaved text of open
//! pages counts, as everywhere in the index.

use super::{
    IndexedBlock, QueryIndex,
    query::{PAGE_COLUMNS, PageRef, PageRow, page_ref},
};
use crate::sync::json::JsString;
use anyhow::Result;
use std::collections::HashMap;

type BlockRow = (
    PageRow,
    (String, Option<String>, u32, u32, u32, u32),
    (u32, u32, String, Option<String>, String, String),
);

const BLOCK_COLUMNS: &str = "b.key,b.id,b.start,b.finish,b.content_from,b.content_to,b.subtree_to,b.depth,b.kind,b.parent,b.text,b.content";

fn block(
    (key, id, from, to, content_from, content_to): (String, Option<String>, u32, u32, u32, u32),
    (subtree_to, depth, kind, parent, text, content): (
        u32,
        u32,
        String,
        Option<String>,
        String,
        String,
    ),
) -> IndexedBlock {
    IndexedBlock {
        key,
        id,
        from,
        to,
        content_from,
        content_to,
        subtree_to,
        depth,
        kind,
        parent,
        text,
        content,
    }
}

impl QueryIndex {
    /// Every live block with its page, by page path and then position.
    pub fn live_blocks(&self) -> Result<Vec<(PageRef, IndexedBlock)>> {
        self.refresh()?;
        let rows: Vec<(BlockRow, i64)> = self.connection.select(&format!(
            "SELECT {PAGE_COLUMNS},{BLOCK_COLUMNS},b.ordinal FROM live_blocks b JOIN live_pages p ON p.revision=b.revision WHERE p.deleted=0"
        ))?()?;
        let mut blocks = rows
            .into_iter()
            .map(|((page, first, second), ordinal)| {
                Ok((page_ref(page)?, block(first, second), ordinal))
            })
            .collect::<Result<Vec<_>>>()?;
        blocks.sort_by(|a, b| self.compare_paths(&a.0.path, &b.0.path).then(a.2.cmp(&b.2)));
        Ok(blocks
            .into_iter()
            .map(|(page, block, _)| (page, block))
            .collect())
    }

    /// The blocks of `page`'s live version, in order; empty when the page is not live.
    pub fn page_blocks(&self, page: &JsString) -> Result<Vec<IndexedBlock>> {
        self.refresh()?;
        let rows: Vec<BlockRow> = self.connection.select_bound(&format!(
            "SELECT {PAGE_COLUMNS},{BLOCK_COLUMNS} FROM live_blocks b JOIN live_pages p ON p.revision=b.revision WHERE p.deleted=0 AND p.page=? ORDER BY b.ordinal"
        ))?(page.to_json())?;
        Ok(rows
            .into_iter()
            .map(|(_, first, second)| block(first, second))
            .collect())
    }

    /// When each live page's saved version was written, in milliseconds since the Unix epoch, by
    /// page identity: the revision time in a Quire notebook, the file's modification time in a
    /// folder notebook.
    pub fn modified_times(&self) -> Result<HashMap<String, i64>> {
        self.refresh()?;
        let rows: Vec<(String, i64)> = if self.folder.is_some() {
            let stamps: Vec<(String, String)> = self.connection.select(
                "SELECT p.page,f.stamp FROM folder_files f JOIN pages p ON p.revision=f.revision WHERE p.deleted=0",
            )?()?;
            stamps
                .into_iter()
                .filter_map(|(page, stamp)| {
                    let nanos: i128 = stamp.split(':').nth(1)?.parse().ok()?;
                    Some((page, i64::try_from(nanos / 1_000_000).ok()?))
                })
                .collect()
        } else {
            self.connection.select(
                "SELECT p.page,r.created FROM pages p JOIN nb.revisions r ON r.id=p.revision WHERE p.deleted=0",
            )?()?
        };
        rows.into_iter()
            .map(|(page, time)| {
                let page = crate::sync::json::Json::parse(page.as_bytes())
                    .ok()
                    .and_then(|json| json.root().as_js_string().map(JsString::to_lossy_string))
                    .ok_or_else(|| anyhow::anyhow!(super::REBUILD))?;
                Ok((page, time))
            })
            .collect()
    }
}
