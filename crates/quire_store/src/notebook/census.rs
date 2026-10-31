//! What a Quire notebook holds, counted and hashed for a migration report: each live page's path
//! and text hash, and how many revisions, drafts, unpublished saves, and attachments it keeps.

use super::{NotebookStore, recovery::page_key};
use crate::sync::format::sha256;
use anyhow::{Context as _, Result};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreCensus {
    /// SHA-256 of each live page's text by path, from the first head in store order.
    pub pages: BTreeMap<String, String>,
    /// Paths of live pages with more than one head.
    pub conflicted: Vec<String>,
    pub revisions: usize,
    /// Each draft's path and the editor session that recorded it.
    pub drafts: Vec<(String, String)>,
    pub outbox: usize,
    pub attachments: usize,
}

impl NotebookStore {
    pub fn census(&self) -> Result<StoreCensus> {
        self.transaction_with("BEGIN", || {
            let count = |sql: &str| -> Result<usize> {
                Ok(self.connection.select_row::<i64>(sql)?()?.unwrap_or(0) as usize)
            };
            let mut census = StoreCensus {
                revisions: count("SELECT COUNT(*) FROM revisions")?,
                outbox: count("SELECT COUNT(*) FROM outbox")?,
                attachments: count("SELECT COUNT(*) FROM attachments")?,
                ..StoreCensus::default()
            };
            let drafted: Vec<String> =
                self.connection
                    .select("SELECT page FROM drafts ORDER BY page")?()?;
            for key in drafted {
                let page: String = serde_json::from_str(&key)?;
                if let Some(draft) = self.draft(&page)? {
                    census.drafts.push((draft.path, draft.stamp.session));
                }
            }
            let pages: Vec<String> =
                self.connection
                    .select("SELECT DISTINCT page FROM heads ORDER BY page")?()?;
            for key in pages {
                let heads = self.heads_key(&key)?;
                let Some(first) = heads.first() else { continue };
                let record = self.get(first)?.context(
                    "A page head is missing. Preserve the database and restore a verified backup.",
                )?;
                let root = record.root();
                if root.field("deleted").as_bool() == Some(true) {
                    continue;
                }
                let path = root.field("path").as_str().unwrap_or_default().to_owned();
                let text = root
                    .field("text")
                    .as_js_string()
                    .map(|text| sha256(text.as_bytes()))
                    .unwrap_or_default();
                if heads.len() > 1 {
                    census.conflicted.push(path.clone());
                }
                census.pages.insert(path, text);
            }
            Ok(census)
        })
    }

    /// The live page at `path`, ignoring case and Unicode normalization as page lookups do, with
    /// its heads.
    pub fn page_at(&self, path: &str) -> Result<Option<(String, Vec<String>)>> {
        let rows = self.connection.select::<(String, String)>(
            "SELECT DISTINCT r.page,r.path FROM heads h JOIN revisions r ON r.id=h.revision WHERE r.deleted=0",
        )?()?;
        let wanted = crate::sync::format::page_id_for_path(path);
        for (page, stored) in rows {
            let page: String = serde_json::from_str(&page)?;
            let stored: String = serde_json::from_str(&stored)?;
            if stored == path || crate::sync::format::page_id_for_path(&stored) == wanted {
                let heads = self.heads_key(&page_key(&page))?;
                return Ok(Some((page, heads)));
            }
        }
        Ok(None)
    }
}
