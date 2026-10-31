use super::{CheckedSaves, Edit, NotebookStore};
use crate::{
    markdown_import::{PagesImported, SourcePage},
    sync::format::{page_id_for_path, sha256},
};
use anyhow::{Result, bail};
use quire_core::exchange::import::{path_key, plan_import};

impl NotebookStore {
    /// Creates every source page whose path no live page uses, ignoring case, in one checked
    /// save: all of them or none. Pages already here are left unchanged and reported as skipped.
    /// Refuses the whole import when a source path cannot be a notebook path.
    pub fn import_pages(&self, pages: &[SourcePage], created: u64) -> Result<PagesImported> {
        let paths: Vec<String> = pages.iter().map(|page| page.path.clone()).collect();
        let mut live = self.connection.select_bound::<String, i64>(
            "SELECT count(*) FROM heads h JOIN revisions r ON r.id=h.revision WHERE r.deleted=0 AND r.path_key=?",
        )?;
        let mut failure = None;
        let plan = plan_import(&paths, |key| {
            live(key.to_owned()).map_or_else(
                |error| {
                    failure.get_or_insert(error);
                    true
                },
                |rows| rows.first().is_some_and(|count| *count > 0),
            )
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        let texts: std::collections::HashMap<&str, &str> = pages
            .iter()
            .map(|page| (page.path.as_str(), page.text.as_str()))
            .collect();
        let mut edits = Vec::with_capacity(plan.pages.len());
        for (source, path) in &plan.pages {
            let derived = page_id_for_path(path);
            let page = if self.heads(&derived)?.is_empty() {
                derived
            } else {
                let seed = format!("{}\0{}\0{created}", path_key(path), self.device_id());
                format!("p-{}", &sha256(seed)[..32])
            };
            edits.push(Edit {
                page,
                base: Vec::new(),
                path: path.clone(),
                text: texts[source.as_str()].to_owned(),
                deleted: false,
                created,
                recovery: None,
            });
        }
        match self.save_many_if_heads(&edits)? {
            CheckedSaves::Saved(saved) => Ok(PagesImported {
                saved,
                skipped: plan.skipped,
            }),
            CheckedSaves::HeadsChanged { .. } | CheckedSaves::Appeared { .. } => bail!(
                "Another change created one of the imported pages first. Nothing was imported; import again to skip the pages already here."
            ),
        }
    }
}

/// The queue bytes an import of `pages` reserves.
pub(crate) fn pages_size(pages: &[SourcePage]) -> usize {
    pages
        .iter()
        .map(|page| 256 + page.path.len() + page.text.len())
        .sum()
}

impl crate::worker::Client<NotebookStore> {
    pub fn import_pages(
        &self,
        pages: Vec<SourcePage>,
        created: u64,
    ) -> std::result::Result<crate::worker::Reply<PagesImported>, crate::worker::QueueError> {
        self.request(pages_size(&pages), move |store| {
            store.import_pages(&pages, created)
        })
    }
}
