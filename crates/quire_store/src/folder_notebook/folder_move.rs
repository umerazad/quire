//! Moving a folder whole, as the reference app renames it: every file inside moves with it,
//! attachments and empty folders included. A folder that holds notes moves through the change
//! journal together with the link changes its move needs, so a crash part way finishes on the
//! next open; any clash found first refuses the move with nothing changed.

use super::{
    FolderNotebook, files,
    journal::{Step, check_edits, exists},
};
use crate::notebook::{CheckedSaves, Edit};
use crate::sync::format::page_id_for_path;
use anyhow::{Context as _, Result, anyhow, ensure};
use std::{
    collections::{HashMap, HashSet},
    fs,
};

fn valid_folder(folder: &str) -> Result<()> {
    ensure!(
        !folder.is_empty()
            && !folder.contains('\\')
            && !folder
                .split('/')
                .any(|part| part.is_empty() || part == ".." || part.starts_with('.')),
        "Invalid folder path {folder:?}. Choose a folder inside the open notebook."
    );
    Ok(())
}

impl FolderNotebook {
    /// Refuses moving `from` to `to` when either path is invalid, `to` lies inside `from`, `from`
    /// is not a folder here, or anything already uses `to`, in any letter case.
    fn check_folder_move(&self, from: &str, to: &str) -> Result<()> {
        valid_folder(from)?;
        valid_folder(to)?;
        ensure!(
            !to.to_lowercase()
                .starts_with(&format!("{}/", from.to_lowercase())),
            "A folder cannot move inside itself. Choose another folder."
        );
        ensure!(
            fs::symlink_metadata(self.root.join(from)).is_ok_and(|found| found.is_dir()),
            "{from} is no longer in the notebook. Refresh files and try again."
        );
        let listing = self.listing();
        let (lower, inside) = (to.to_lowercase(), format!("{}/", to.to_lowercase()));
        let taken = listing
            .files
            .iter()
            .map(|file| &file.path)
            .chain(&listing.others)
            .chain(&listing.folders)
            .any(|path| {
                let path = path.to_lowercase();
                path == lower || path.starts_with(&inside)
            });
        ensure!(
            !taken && !exists(&self.root.join(to)),
            "{to} already exists. Nothing was moved; choose another name."
        );
        Ok(())
    }

    /// Renames or moves `from`, a folder that holds no notes, to `to`, with every other file in
    /// it. This refuses, moving nothing, when a note appeared inside meanwhile, when `to` exists,
    /// or when `to` lies inside `from`.
    pub fn move_noteless_folder(&self, from: &str, to: &str) -> Result<()> {
        self.check_folder_move(from, to)?;
        let prefix = format!("{from}/");
        ensure!(
            !self
                .listing()
                .files
                .iter()
                .any(|file| file.path.starts_with(&prefix)),
            "A note appeared in {from}. Nothing was moved; try again so its links follow."
        );
        self.move_folder_step(&Step::folder(from, to))?;
        self.listing();
        Ok(())
    }

    /// Moves `from` to `to` whole with every file in it, and saves `edits`: one for each note
    /// inside, at its place under `to`, and one for each other note whose links change. Every
    /// note must still hold its edit's base (`HeadsChanged` otherwise), and every note inside
    /// must be among the edits (`Appeared` otherwise); either refusal, or `to` existing, changes
    /// nothing. The folder renames first, then each note is written in place, all journaled.
    pub fn move_folder_if_heads(
        &self,
        edits: &[Edit],
        from: &str,
        to: &str,
    ) -> Result<CheckedSaves> {
        let (from, to) = (from.trim_matches('/'), to.trim_matches('/'));
        check_edits(edits)?;
        self.check_folder_move(from, to)?;
        let prefix = format!("{from}/");
        let mut steps = vec![Step::folder(from, to)];
        let mut inside = HashSet::new();
        for edit in edits {
            files::validate(&edit.path)?;
            let heads = self.listed_heads(&edit.page)?;
            if heads != edit.base {
                return Ok(CheckedSaves::HeadsChanged {
                    page: edit.page.clone(),
                    heads,
                });
            }
            let source = self
                .path_of(&edit.page)
                .unwrap_or_else(|| edit.path.clone());
            match source.strip_prefix(&prefix) {
                Some(rest) => {
                    ensure!(
                        edit.path == format!("{to}/{rest}"),
                        "{source} must move to {to}/{rest} with its folder. Nothing was moved; preview the move again."
                    );
                    // The note is written in place after the folder moves, so it must be
                    // writable where it is now.
                    self.preflight(&Edit {
                        path: source.clone(),
                        ..edit.clone()
                    })?;
                    steps.push(Step::write(edit, Some(source.clone())));
                    inside.insert(source);
                }
                None => {
                    ensure!(
                        page_id_for_path(&edit.path) == edit.page,
                        "{} cannot move while {from} moves. Nothing was moved; preview the move again.",
                        edit.path
                    );
                    self.preflight(edit)?;
                    steps.push(Step::write(edit, None));
                }
            }
        }
        if let Some(path) = self
            .listing()
            .files
            .into_iter()
            .map(|file| file.path)
            .find(|path| path.starts_with(&prefix) && !inside.contains(path))
        {
            return Ok(CheckedSaves::Appeared { path });
        }
        let journal = self.write_journal(&steps)?;
        #[cfg(test)]
        if self.interrupt_folder.get() {
            anyhow::bail!("Injected interruption before the folder moves");
        }
        if let Err(error) = self.move_folder_step(&steps[0]) {
            if exists(&self.root.join(from)) && !exists(&self.root.join(to)) {
                fs::remove_file(&journal).with_context(|| {
                    format!("{error:#} Nothing was moved, but the change journal {} could not be removed. Delete it, then retry.", journal.display())
                })?;
                return Err(anyhow!("{error:#} Nothing was moved."));
            }
            return Err(error);
        }
        let mut acks: HashMap<String, _> = self.apply(&steps)?.into_iter().collect();
        for edit in edits {
            self.settle_draft(&edit.page, &edit.text, None)?;
        }
        fs::remove_file(&journal).with_context(|| {
            format!("The folder moved, but its journal {} could not be removed. Delete it after checking the notes.", journal.display())
        })?;
        self.listing();
        Ok(CheckedSaves::Saved(
            edits
                .iter()
                .map(|edit| {
                    acks.remove(&edit.path).context(
                        "A journaled save lost its acknowledgment. Reopen the notebook to check the change.",
                    )
                })
                .collect::<Result<_>>()?,
        ))
    }
}
