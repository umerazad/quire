//! Changes across several notes, such as a rename that rewrites links, made as one journaled
//! transaction, as the reference `commitEdits` does. Every note is checked first, then a synced
//! journal records the whole change before any file is touched, then each note is written and a
//! moved note's old file is archived in Quire's state folder. A crash part way leaves the journal,
//! and the next open rolls the change forward; a note that changed meanwhile stops the recovery
//! and keeps the journal and every version for review.

use super::{FolderNotebook, files, history};
use crate::{
    notebook::{CheckedSaves, Edit, SaveAck},
    sync::format::{page_id_for_path, sha256},
};
use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// The reference app's cap on one change across notes.
pub(super) const MAX_EDITS: usize = 1000;
const JOURNALS: &str = "transactions";
const RETIRED: &str = "retired";

/// One step of a journaled change, in the reference `FileEdit` shape.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Step {
    path: String,
    text: String,
    /// The note's revision the step was planned from; None for a note that must not exist yet.
    expected_revision: Option<String>,
    /// Archive the note at `path` instead of writing it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    remove: bool,
    /// The archive name for a removed note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    retirement: Option<String>,
    /// The old path of a moved note, whose saved history the new path takes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    history_from: Option<String>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Journal {
    edits: Vec<Step>,
    created_at: i64,
}

fn interrupted(journal: &Path, error: &anyhow::Error) -> String {
    format!(
        "An interrupted change across notes needs review: {error:#}. Its original versions and pending edits are kept in {}. Compare those files with the notes before deleting it.",
        journal.display()
    )
}

impl FolderNotebook {
    fn journals(&self) -> PathBuf {
        self.state.join(JOURNALS)
    }

    /// Saves every edit when each note still holds its edit's base, or reports the first note
    /// that changed and writes nothing. An edit whose path belongs to another page moves that
    /// page's note there: the new file is created, never replacing one, and the old file is
    /// archived. Several notes, or any move, go through the journal.
    pub fn save_many_if_heads(&self, edits: &[Edit]) -> Result<CheckedSaves> {
        let mut pages: Vec<&str> = edits.iter().map(|edit| edit.page.as_str()).collect();
        pages.sort_unstable();
        ensure!(
            pages.windows(2).all(|pair| pair[0] != pair[1]),
            "A change across pages must name each page once. Retry the change."
        );
        ensure!(
            edits.len() <= MAX_EDITS,
            "A change across pages may update at most {MAX_EDITS} notes. Change fewer pages at a time and retry."
        );
        let listing = self.listing();
        let mut writes = Vec::new();
        let mut removals = Vec::new();
        for edit in edits {
            files::validate(&edit.path)?;
            let heads = self.heads(&edit.page)?;
            if heads != edit.base {
                return Ok(CheckedSaves::HeadsChanged {
                    page: edit.page.clone(),
                    heads,
                });
            }
            self.preflight(edit)?;
            let moved = page_id_for_path(&edit.path) != edit.page;
            if !moved {
                writes.push(self.step(edit, edit.base.first().cloned(), None));
                continue;
            }
            let source = self.path_of(&edit.page).with_context(|| {
                format!(
                    "The note moving to {} has no file. Reopen the page and try again.",
                    edit.path
                )
            })?;
            if let Some(taken) = listing
                .files
                .iter()
                .map(|file| &file.path)
                .chain(&listing.others)
                .chain(&listing.folders)
                .find(|path| path.to_lowercase() == edit.path.to_lowercase())
            {
                bail!("{taken} already exists. Nothing was changed; choose another name.");
            }
            writes.insert(0, self.step(edit, None, Some(source.clone())));
            removals.push(Step {
                path: source,
                text: String::new(),
                expected_revision: edit.base.first().cloned(),
                remove: true,
                retirement: Some(
                    sha256(format!("{}:{}", edit.page, (self.clock)()))[..32].to_owned(),
                ),
                history_from: None,
            });
        }
        if writes.len() == 1 && removals.is_empty() {
            return Ok(CheckedSaves::Saved(vec![self.save(&edits[0])?]));
        }
        let steps: Vec<Step> = writes.into_iter().chain(removals).collect();
        let journal = self.write_journal(&steps)?;
        let mut acks: std::collections::HashMap<String, SaveAck> =
            self.apply(&steps)?.into_iter().collect();
        fs::remove_file(&journal).with_context(|| {
            format!("The change was saved, but its journal {} could not be removed. Delete it after checking the notes.", journal.display())
        })?;
        Ok(CheckedSaves::Saved(
            edits
                .iter()
                .map(|edit| {
                    acks.remove(&edit.path)
                        .context("A journaled save lost its acknowledgment. Reopen the notebook to check the change.")
                })
                .collect::<Result<_>>()?,
        ))
    }

    fn step(&self, edit: &Edit, expected: Option<String>, from: Option<String>) -> Step {
        Step {
            path: edit.path.clone(),
            text: edit.text.clone(),
            expected_revision: expected,
            remove: false,
            retirement: None,
            history_from: from,
        }
    }

    fn write_journal(&self, steps: &[Step]) -> Result<PathBuf> {
        let folder = self.journals();
        fs::create_dir_all(&folder).with_context(|| {
            format!("Cannot create {}. Check write access to Quire's application data folder and retry.", folder.display())
        })?;
        let bytes = serde_json::to_vec(&Journal {
            edits: steps.to_vec(),
            created_at: (self.clock)(),
        })?;
        let path = folder.join(format!("{}.json", &sha256(&bytes)[..32]));
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .with_context(|| format!("Cannot create the change journal {}. Check write access to Quire's application data folder and retry.", path.display()))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .and_then(|()| files::sync_directory(&folder))
            .with_context(|| format!("Cannot write the change journal {}. Check free space and retry; nothing was changed.", path.display()))?;
        Ok(path)
    }

    /// Applies every step in order. Each is idempotent, so a recovery can repeat the steps a
    /// crash already finished.
    fn apply(&self, steps: &[Step]) -> Result<Vec<(String, SaveAck)>> {
        let mut acks = Vec::new();
        for (index, step) in steps.iter().enumerate() {
            #[cfg(test)]
            if self.interrupt_at.get() == Some(index) {
                bail!("Injected interruption before step {index}");
            }
            #[cfg(not(test))]
            let _ = index;
            if step.remove {
                self.retire(step)?;
                continue;
            }
            let edit = Edit {
                page: page_id_for_path(&step.path),
                base: step.expected_revision.iter().cloned().collect(),
                path: step.path.clone(),
                text: step.text.clone(),
                deleted: false,
                created: (self.clock)().max(0) as u64,
                recovery: None,
            };
            let ack = self.save(&edit).with_context(|| {
                format!(
                    "{} changed before the change across notes could finish",
                    step.path
                )
            })?;
            if let Some(from) = &step.history_from {
                history::copy(&self.connection, from, &step.path)?;
                if let Some(old) = self.read(from)?
                    && !history::has(&self.connection, &step.path, &old.revision)?
                {
                    history::add(
                        &self.connection,
                        &step.path,
                        &old.text,
                        &old.revision,
                        (self.clock)(),
                    )?;
                }
            }
            acks.push((step.path.clone(), ack));
        }
        Ok(acks)
    }

    /// Moves a note into Quire's archive of moved originals when it still holds the planned
    /// version. A note already archived is done; one that changed stops the change.
    fn retire(&self, step: &Step) -> Result<()> {
        let name = step
            .retirement
            .as_deref()
            .filter(|name| name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .context(
                "The change journal names an invalid archive. Keep the journal for recovery.",
            )?;
        let folder = self.state.join(RETIRED);
        let archived = folder.join(format!("{name}.md"));
        let Some(disk) = self.read(&step.path)? else {
            return Ok(());
        };
        if Some(&disk.revision) != step.expected_revision.as_ref() {
            bail!(
                "{} changed before its old file could be archived. Both versions are kept; compare them before deleting the journal.",
                step.path
            );
        }
        fs::create_dir_all(&folder)?;
        match fs::symlink_metadata(&archived) {
            Ok(_) => bail!(
                "An archived original already exists at {}. Keep both files and review the interrupted change.",
                archived.display()
            ),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        history::add(
            &self.connection,
            &step.path,
            &disk.text,
            &disk.revision,
            (self.clock)(),
        )?;
        let source = files::resolve(&self.root, &step.path, false)?;
        fs::rename(&source, &archived).with_context(|| {
            format!("Cannot archive the old file {}. Check write access and reopen the notebook to finish the change.", step.path)
        })?;
        files::sync_directory(&folder)?;
        if let Some(parent) = source.parent() {
            files::sync_directory(parent)?;
        }
        let page = page_id_for_path(&step.path);
        if self
            .draft(&page)?
            .is_some_and(|draft| draft.text == disk.text)
        {
            self.discard_draft(&page)?;
        }
        self.known.borrow_mut().remove(&step.path);
        self.reported.borrow_mut().remove(&step.path);
        self.pages.borrow_mut().remove(&page);
        Ok(())
    }

    /// Rolls interrupted changes forward, as the reference `resumeTransactions` does, and
    /// reports what happened to each.
    pub(super) fn resume(&self) -> Vec<String> {
        let folder = self.journals();
        let Ok(entries) = fs::read_dir(&folder) else {
            return Vec::new();
        };
        let mut journals: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "json")
            })
            .collect();
        journals.sort();
        journals
            .into_iter()
            .map(|journal| {
                let resumed = fs::read(&journal)
                    .context("Cannot read the journal")
                    .and_then(|bytes| {
                        serde_json::from_slice::<Journal>(&bytes).context("The journal is malformed")
                    })
                    .and_then(|recorded| {
                        ensure!(
                            !recorded.edits.is_empty() && recorded.edits.len() <= MAX_EDITS * 2,
                            "The journal is malformed"
                        );
                        self.apply(&recorded.edits)?;
                        fs::remove_file(&journal)?;
                        Ok(())
                    });
                match resumed {
                    Ok(()) => "Finished an interrupted change across notes, such as a rename. Its notes and links are now saved.".to_owned(),
                    Err(error) => interrupted(&journal, &error),
                }
            })
            .collect()
    }
}
