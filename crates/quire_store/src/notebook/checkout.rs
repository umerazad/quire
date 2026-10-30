use super::{Draft, NotebookStore};
use crate::sync::json::Json;
use anyhow::{Context as _, Result, ensure};

#[derive(Debug)]
pub struct PageCheckout {
    /// Includes tombstones and conflicts, in the store's stable head order.
    pub heads: Vec<String>,
    /// At most one revision body; its strings and unknown fields remain lossless.
    pub selected: Option<Json>,
    pub draft: Option<Draft>,
}

impl NotebookStore {
    /// Reads heads, one chosen body, and recovery from the same SQLite snapshot.
    /// Without an explicit selection, chooses the first head in store order.
    pub fn checkout(&self, page: &str, selected: Option<&str>) -> Result<PageCheckout> {
        self.transaction_with("BEGIN", || {
            let heads = self.heads(page)?;
            let selected = selected.or_else(|| heads.first().map(String::as_str));
            let selected = selected
                .map(|id| {
                    ensure!(
                        heads.iter().any(|head| head == id),
                        "The selected revision is no longer a head of this page. Keep the buffer and refresh the page's versions."
                    );
                    self.get(id)?.context(
                        "The selected page revision is missing. Preserve the database and restore a verified backup."
                    )
                })
                .transpose()?;
            Ok(PageCheckout {
                heads,
                selected,
                draft: self.draft(page)?,
            })
        })
    }
}
