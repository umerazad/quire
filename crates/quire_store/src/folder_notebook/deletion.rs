//! Deleting notes and folders from a folder notebook by moving them to the system Trash, as the
//! reference `VaultFiles.remove` does: every chosen note must still hold the version chosen, and a
//! folder goes whole only when no note appeared in it since.

use super::{FolderNotebook, files, history};
use crate::{
    notebook::{CheckedSaves, Edit},
    sync::format::page_id_for_path,
};
use anyhow::{Context as _, Result, ensure};
use std::{fs, path::Path, sync::Arc};

/// Moves a file or folder to the system Trash. Tests pass a stand-in so they never fill it.
pub type Trash = Arc<dyn Fn(&Path) -> Result<()> + Send + Sync>;

pub const NO_TRASH: &str = "The system Trash is unavailable here, so nothing was deleted. Delete the file in Finder or Explorer; Quire follows the change.";

impl FolderNotebook {
    pub fn set_trash(&mut self, trash: Trash) {
        self.trash = Some(trash);
    }

    /// Ordinary files below `folder` that are not notes, which a folder deletion also moves.
    pub fn other_files(&self, folder: &str) -> Vec<String> {
        let prefix = format!("{folder}/");
        self.listing()
            .others
            .into_iter()
            .filter(|path| path.starts_with(&prefix))
            .collect()
    }

    /// Moves every chosen note to the Trash, or `folder` with everything in it, when each note
    /// still holds its edit's base. Refuses before moving anything when a note changed
    /// (`HeadsChanged`), a note appeared in the folder (`Appeared`), or a note has unsaved work
    /// from another session. Notes already gone are skipped. The deleted text joins history.
    pub fn delete_if_heads(&self, edits: &[Edit], folder: Option<&str>) -> Result<CheckedSaves> {
        let trash = self.trash.clone().context(NO_TRASH)?;
        let folder = folder.map(|folder| folder.trim_matches('/'));
        let mut present = Vec::new();
        for edit in edits {
            files::validate(&edit.path)?;
            ensure!(
                page_id_for_path(&edit.path) == edit.page,
                "{} belongs to another page in this folder notebook. Nothing was deleted; try again.",
                edit.path
            );
            if let Some(folder) = folder {
                ensure!(
                    edit.path.starts_with(&format!("{folder}/")),
                    "{} is not in {folder}. Nothing was deleted; try again.",
                    edit.path
                );
            }
            let disk = self.read(&edit.path)?;
            if disk.as_ref().map(|disk| &disk.revision) != edit.base.first() {
                return Ok(CheckedSaves::HeadsChanged {
                    page: edit.page.clone(),
                    heads: disk.map(|disk| vec![disk.revision]).unwrap_or_default(),
                });
            }
            if let Some(draft) = self.draft(&edit.page)?
                && draft.text != edit.text
                && Some(&draft.text) != disk.as_ref().map(|disk| &disk.text)
            {
                anyhow::bail!(
                    "{} has unsaved work recovered from an earlier session. Nothing was deleted; open the page and choose a version first.",
                    edit.path
                );
            }
            if let Some(disk) = disk {
                present.push((edit, disk));
            }
        }
        if let Some(folder) = folder {
            let chosen: Vec<&str> = edits.iter().map(|edit| edit.path.as_str()).collect();
            let prefix = format!("{folder}/");
            if let Some(path) = self
                .listing()
                .files
                .into_iter()
                .map(|file| file.path)
                .find(|path| path.starts_with(&prefix) && !chosen.contains(&path.as_str()))
            {
                return Ok(CheckedSaves::Appeared { path });
            }
        }
        // Text typed into an open page that has not reached its file yet goes with it, as a
        // Quire notebook keeps it in the tombstone.
        for (edit, disk) in &present {
            if edit.text != disk.text {
                self.save(&Edit {
                    deleted: false,
                    ..(*edit).clone()
                })?;
            }
        }
        let now = (self.clock)();
        for (edit, disk) in &present {
            let text = &edit.text;
            history::add(
                &self.connection,
                &disk.path,
                text,
                &files::revision(text),
                now,
            )?;
        }
        match folder {
            Some(folder) => trash(&self.folder_path(folder)?)?,
            None => {
                for (_, disk) in &present {
                    trash(&files::resolve(&self.root, &disk.path, false)?)?;
                }
            }
        }
        let acks = edits
            .iter()
            .map(|edit| {
                let page = &edit.page;
                self.connection
                    .exec_bound("DELETE FROM drafts WHERE page=?")?(page.as_str())?;
                self.known.borrow_mut().remove(&edit.path);
                self.reported.borrow_mut().remove(&edit.path);
                self.pages.borrow_mut().remove(page);
                let document = files::DiskDocument::new(edit.path.clone(), edit.text.clone());
                Ok(self.ack(edit, document, true))
            })
            .collect::<Result<_>>()?;
        Ok(CheckedSaves::Saved(acks))
    }

    /// A folder inside the notebook, refusing links and paths that leave it.
    fn folder_path(&self, folder: &str) -> Result<std::path::PathBuf> {
        ensure!(
            !folder.is_empty()
                && !folder.contains('\\')
                && !folder
                    .split('/')
                    .any(|part| part.is_empty() || part == ".." || part.starts_with('.')),
            "Invalid folder {folder:?}. Choose a folder inside the open notebook."
        );
        let path = self.root.join(folder);
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("Cannot find {folder}. Rescan the notebook and try again."))?;
        ensure!(
            metadata.is_dir() && !metadata.file_type().is_symlink(),
            "{folder} is a link or not a folder. Nothing was deleted; delete it in Finder or Explorer."
        );
        let real_root = fs::canonicalize(&self.root)?;
        ensure!(
            fs::canonicalize(&path)?.starts_with(&real_root),
            "{folder} resolves outside this notebook. Nothing was deleted."
        );
        Ok(path)
    }
}
