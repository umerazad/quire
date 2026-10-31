use super::{FolderNotebook, journal::MAX_EDITS};
use crate::{
    markdown_import::{PagesImported, SourcePage},
    notebook::{CheckedSaves, Edit},
    sync::format::page_id_for_path,
};
use anyhow::{Result, bail};
use quire_core::exchange::import::{path_key, plan_import};
use std::{collections::HashSet, fs};

impl FolderNotebook {
    /// Creates a note for every source page whose path no file or folder in the notebook uses,
    /// ignoring case. Notes are created through the journal, at most [`MAX_EDITS`] per journal;
    /// when a later group fails, the notes this import already created are removed again, so an
    /// import lands whole or not at all. An interrupted import leaves whole groups, and importing
    /// again skips them.
    pub fn import_pages(&self, pages: &[SourcePage], created: u64) -> Result<PagesImported> {
        self.import_in_groups(pages, created, MAX_EDITS)
    }

    fn import_in_groups(
        &self,
        pages: &[SourcePage],
        created: u64,
        group_size: usize,
    ) -> Result<PagesImported> {
        let listing = self.listing();
        let taken: HashSet<String> = listing
            .files
            .iter()
            .map(|file| file.path.as_str())
            .chain(listing.others.iter().map(String::as_str))
            .chain(listing.folders.iter().map(String::as_str))
            .map(path_key)
            .collect();
        let paths: Vec<String> = pages.iter().map(|page| page.path.clone()).collect();
        let plan = plan_import(&paths, |key| taken.contains(key))?;
        let texts: std::collections::HashMap<&str, &str> = pages
            .iter()
            .map(|page| (page.path.as_str(), page.text.as_str()))
            .collect();
        let edits: Vec<Edit> = plan
            .pages
            .iter()
            .map(|(source, path)| Edit {
                page: page_id_for_path(path),
                base: Vec::new(),
                path: path.clone(),
                text: texts[source.as_str()].to_owned(),
                deleted: false,
                created,
                recovery: None,
            })
            .collect();
        let mut saved = Vec::with_capacity(edits.len());
        for group in edits.chunks(group_size) {
            let failure = match self.save_many_if_heads(group) {
                Ok(CheckedSaves::Saved(acks)) => {
                    saved.extend(acks);
                    continue;
                }
                Ok(_) => anyhow::anyhow!(
                    "Another application created one of the imported pages first. Nothing was imported; import again to skip the pages already here."
                ),
                Err(error) => error,
            };
            self.remove_imported(&edits[..saved.len()]);
            bail!(failure);
        }
        Ok(PagesImported {
            saved,
            skipped: plan.skipped,
        })
    }

    /// Removes notes an import created, each only while it still holds the imported text.
    fn remove_imported(&self, edits: &[Edit]) {
        for edit in edits {
            let file = self.root.join(&edit.path);
            if fs::read_to_string(&file).is_ok_and(|text| text == edit.text) {
                fs::remove_file(file).ok();
            }
        }
        self.listing();
    }
}

impl crate::worker::Client<FolderNotebook> {
    pub fn import_pages(
        &self,
        pages: Vec<SourcePage>,
        created: u64,
    ) -> std::result::Result<crate::worker::Reply<PagesImported>, crate::worker::QueueError> {
        self.request(crate::notebook::pages_size(&pages), move |notebook| {
            notebook.import_pages(&pages, created)
        })
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn a_failing_group_removes_the_notes_the_import_created() -> Result<()> {
        let root = tempfile::tempdir()?;
        let state = tempfile::tempdir()?;
        let locked = root.path().join("Locked");
        fs::create_dir(&locked)?;
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o555))?;
        let page = |path: &str| SourcePage {
            path: path.into(),
            text: format!("- {path}\n"),
        };
        let pages = ["Bulk/A.md", "Bulk/B.md", "Locked/Late.md"].map(page);
        let notebook = FolderNotebook::open(root.path(), state.path())?;
        let failed = notebook.import_in_groups(&pages, 1, 2);
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o755))?;
        assert!(
            failed.is_err(),
            "the second group cannot write into a read-only folder"
        );
        let notes = || {
            crate::folder_notebook::list(root.path())
                .files
                .into_iter()
                .map(|file| file.path)
                .collect::<Vec<_>>()
        };
        assert!(
            notes().is_empty(),
            "the first group's notes were removed: {:?}",
            notes()
        );
        drop(notebook);
        let reopened = FolderNotebook::open(root.path(), state.path())?;
        assert!(notes().is_empty(), "reopening finishes no abandoned import");
        let imported = reopened.import_in_groups(&pages, 2, 2)?;
        assert_eq!(
            imported.saved.len(),
            3,
            "once the folder is writable the whole import lands"
        );
        assert_eq!(notes(), ["Bulk/A.md", "Bulk/B.md", "Locked/Late.md"]);
        Ok(())
    }
}
