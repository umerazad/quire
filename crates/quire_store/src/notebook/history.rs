//! A page's earlier saved versions, as the reference engine's `revisions` and `revisionText` list
//! them: every stored revision of the page except its current heads, newest first. Revisions
//! from other computers and from the reference app are listed with their own times.

use super::{NotebookStore, recovery::page_key};
use anyhow::{Context as _, Result};

/// One earlier saved version of a page. `id` names it to `version_text`: a revision ID in a
/// Quire notebook, a history row in a folder notebook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedVersion {
    pub id: String,
    /// Milliseconds since the Unix epoch.
    pub saved_at: i64,
}

pub const VERSION_UNAVAILABLE: &str =
    "That version of the page is not available. Reopen page history and choose another.";

impl NotebookStore {
    /// Every stored revision of `page` except its current heads, newest first.
    pub fn history(&self, page: &str) -> Result<Vec<SavedVersion>> {
        let key = page_key(page);
        let rows: Vec<(String, i64)> = self.connection.select_bound(
            "SELECT id,created FROM revisions WHERE page=?1 AND id NOT IN (SELECT revision FROM heads WHERE page=?1) ORDER BY created DESC, generation DESC, id DESC",
        )?(key.as_str())?;
        Ok(rows
            .into_iter()
            .map(|(id, saved_at)| SavedVersion { id, saved_at })
            .collect())
    }

    /// The text of one of `page`'s saved versions.
    pub fn version_text(&self, page: &str, id: &str) -> Result<String> {
        let record = self.get(id)?.context(VERSION_UNAVAILABLE)?;
        let root = record.root();
        anyhow::ensure!(
            root.field("page").as_str() == Some(page),
            VERSION_UNAVAILABLE
        );
        root.field("text")
            .as_str()
            .map(str::to_owned)
            .context(VERSION_UNAVAILABLE)
    }
}
