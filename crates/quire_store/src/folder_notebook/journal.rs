//! Changes across several notes, such as a rename that rewrites links, made as one journaled
//! transaction, as the reference `commitEdits` does. Every note is checked first, then a synced
//! journal records the whole change before any file is touched, then each note is written and a
//! moved note's old file is archived in Quire's state folder. A crash part way leaves the journal,
//! and the next open rolls the change forward; a note that changed meanwhile stops the recovery
//! and keeps the journal and every version for review. The journal is removed only after every
//! note is durable. Syncs are batched: on macOS each file is `fsync`ed and one `F_FULLFSYNC` per
//! drive and phase flushes the drive's cache, since a full sync per note costs milliseconds.

use super::{DiskDocument, FolderNotebook, Published, files, history};
use crate::{
    notebook::{CheckedSaves, Edit, SaveAck},
    sync::format::{page_id_for_path, sha256},
};
use anyhow::{Context as _, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeSet, HashMap},
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
        let taken: HashMap<String, &String> = listing
            .files
            .iter()
            .map(|file| &file.path)
            .chain(&listing.others)
            .chain(&listing.folders)
            .map(|path| (path.to_lowercase(), path))
            .collect();
        let mut writes = Vec::new();
        let mut removals = Vec::new();
        for edit in edits {
            files::validate(&edit.path)?;
            let heads = self.listed_heads(&edit.page)?;
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
            if let Some(taken) = taken.get(&edit.path.to_lowercase()) {
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
        let mut acks: HashMap<String, SaveAck> = self.apply(&steps)?.into_iter().collect();
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

    /// The page's file revision as of the last listing, which a change across notes takes once
    /// instead of listing again for every page that has no file.
    fn listed_heads(&self, page: &str) -> Result<Vec<String>> {
        let Some(path) = self.path_of(page) else {
            return Ok(Vec::new());
        };
        Ok(self
            .read(&path)?
            .map(|document| vec![document.revision])
            .unwrap_or_default())
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
            .and_then(|()| files::full_sync(&file))
            .and_then(|()| files::sync_directory(&folder))
            .with_context(|| format!("Cannot write the change journal {}. Check free space and retry; nothing was changed.", path.display()))?;
        Ok(path)
    }

    /// Applies every step in order. Each is idempotent, so a recovery can repeat the steps a
    /// crash already finished. Every step is checked first; then each new text is written beside
    /// its note and all of them are made durable together, the replaced versions go into history
    /// in one transaction, the notes are renamed into place and the old ones archived in step
    /// order, and finally every folder touched is made durable together. A step that fails its
    /// check stops the change there, after the steps before it are applied.
    fn apply(&self, steps: &[Step]) -> Result<Vec<(String, SaveAck)>> {
        let mut planned = Vec::with_capacity(steps.len());
        let mut failure = None;
        for step in steps {
            match self.plan(step) {
                Ok(step) => planned.push(step),
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
        let acks = self.apply_planned(planned)?;
        failure.map_or(Ok(acks), Err)
    }

    fn apply_planned(&self, planned: Vec<Planned>) -> Result<Vec<(String, SaveAck)>> {
        let staged: Vec<Option<files::Staged>> = planned
            .iter()
            .map(|planned| match planned {
                Planned::Write {
                    edit, destination, ..
                } => files::stage_flushed(destination, &edit.text)
                    .map(Some)
                    .with_context(|| changed_before(&edit.path)),
                _ => Ok(None),
            })
            .collect::<Result<_>>()?;
        files::barrier(planned.iter().filter_map(Planned::folder)).context(
            "Cannot make the new note texts durable. Check the notebook's disk and reopen the notebook to finish the change.",
        )?;
        #[cfg(test)]
        if self.interrupt_staged.get() {
            bail!("Injected interruption once every new text is staged");
        }
        self.transaction(|| {
            planned
                .iter()
                .try_for_each(|planned| self.keep_history(planned))
        })?;
        let mut folders = BTreeSet::new();
        for (index, (planned, staged)) in planned.iter().zip(staged).enumerate() {
            #[cfg(test)]
            if self.interrupt_at.get() == Some(index) {
                bail!("Injected interruption before step {index}");
            }
            #[cfg(not(test))]
            let _ = index;
            match planned {
                Planned::Write {
                    edit,
                    destination,
                    replaced,
                    ..
                } => {
                    let staged = staged.context(
                        "A planned note lost its staged text. Reopen the notebook to finish the change.",
                    )?;
                    self.place(edit, destination, replaced.is_some(), staged)
                        .with_context(|| changed_before(&edit.path))?;
                    folders.extend(destination.parent().map(Path::to_owned));
                }
                Planned::Retire {
                    step,
                    source,
                    archived,
                    ..
                } => {
                    archive(step, source, archived)?;
                    folders.extend(source.parent().map(Path::to_owned));
                    folders.extend(archived.parent().map(Path::to_owned));
                    folders.insert(self.state.clone());
                }
                Planned::Written { .. } | Planned::Retired => {}
            }
        }
        #[cfg(test)]
        if self.interrupt_at.get() == Some(planned.len()) {
            bail!("Injected interruption after every step");
        }
        folders
            .iter()
            .try_for_each(|folder| files::flush_directory(folder))
            .and_then(|()| files::barrier(folders.iter().map(PathBuf::as_path)))
            .context("Cannot make the renamed notes durable. Check the notebook's disk and reopen the notebook to finish the change.")?;
        self.transaction(|| {
            planned
                .into_iter()
                .filter_map(|planned| self.settle(planned).transpose())
                .collect()
        })
    }

    /// Checks one step against the disk before anything is written.
    fn plan<'a>(&self, step: &'a Step) -> Result<Planned<'a>> {
        if step.remove {
            return self.plan_retirement(step);
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
        (|| {
            ensure!(
                edit.text.len() as u64 <= files::MAX_NOTE_BYTES,
                "The note could not be saved because it exceeds 10 MB. Split the note and retry."
            );
            self.preflight(&edit)?;
            let destination = files::resolve(&self.root, &edit.path, true)?;
            let disk = self.read(&edit.path)?;
            if let Some(disk) = disk.as_ref().filter(|disk| disk.text == edit.text) {
                return Ok(Planned::Written {
                    step,
                    disk: disk.clone(),
                    edit: edit.clone(),
                });
            }
            if disk.as_ref().map(|disk| &disk.revision) != step.expected_revision.as_ref() {
                return Err(self.conflict(&edit, disk, "This note changed on disk. Your edits are in recovery. Compare the versions or save a separate copy.").into());
            }
            Ok(Planned::Write {
                step,
                destination,
                replaced: disk,
                edit: edit.clone(),
            })
        })()
        .with_context(|| changed_before(&step.path))
    }

    fn plan_retirement<'a>(&self, step: &'a Step) -> Result<Planned<'a>> {
        let name = step
            .retirement
            .as_deref()
            .filter(|name| name.len() == 32 && name.bytes().all(|byte| byte.is_ascii_hexdigit()))
            .context(
                "The change journal names an invalid archive. Keep the journal for recovery.",
            )?;
        let archived = self.state.join(RETIRED).join(format!("{name}.md"));
        let Some(disk) = self.read(&step.path)? else {
            return Ok(Planned::Retired);
        };
        if Some(&disk.revision) != step.expected_revision.as_ref() {
            bail!(
                "{} changed before its old file could be archived. Both versions are kept; compare them before deleting the journal.",
                step.path
            );
        }
        ensure_unarchived(&archived)?;
        Ok(Planned::Retire {
            source: files::resolve(&self.root, &step.path, false)?,
            archived,
            disk,
            step,
        })
    }

    /// Records the versions a step replaces, before any file is renamed: the text a write
    /// replaces, an archived note's text, and a moved note's old history under its new path. A
    /// version a crash already recorded is not recorded again.
    fn keep_history(&self, planned: &Planned) -> Result<()> {
        let now = (self.clock)();
        let (step, replaced) = match planned {
            Planned::Write { step, replaced, .. } => (step, replaced.as_ref()),
            Planned::Written { step, .. } => (step, None),
            Planned::Retire { step, disk, .. } => (step, Some(disk)),
            Planned::Retired => return Ok(()),
        };
        let path = &step.path;
        if let Some(disk) = replaced {
            history::add_once(&self.connection, path, &disk.text, &disk.revision, now)?;
        }
        let Some(from) = &step.history_from else {
            return Ok(());
        };
        history::copy(&self.connection, from, path)?;
        if let Some(old) = self.read(from)?
            && !history::has(&self.connection, path, &old.revision)?
        {
            history::add(&self.connection, path, &old.text, &old.revision, now)?;
        }
        Ok(())
    }

    /// Renames a staged note into place once the disk still holds the version it was planned
    /// from. A new note never replaces a file created meanwhile.
    fn place(
        &self,
        edit: &Edit,
        destination: &Path,
        replace: bool,
        staged: files::Staged,
    ) -> Result<()> {
        let latest = files::read_at(&edit.path, destination)?;
        if latest
            .as_ref()
            .is_some_and(|latest| latest.text == edit.text)
        {
            return Ok(());
        }
        if latest.as_ref().map(|latest| &latest.revision) != edit.base.first() {
            return Err(self.conflict(edit, latest, "Another application changed the note during saving. Compare the versions before retrying.").into());
        }
        if staged.place(replace)? == Published::Exists {
            let found = self.read(&edit.path)?;
            return Err(self.conflict(edit, found, "A file with this name was created elsewhere. Choose another name or compare its content.").into());
        }
        Ok(())
    }

    /// Updates drafts and what this notebook knows of each note once the change is durable,
    /// and acknowledges each written note.
    fn settle(&self, planned: Planned) -> Result<Option<(String, SaveAck)>> {
        match planned {
            Planned::Write { edit, .. } => {
                let document = DiskDocument::new(edit.path.clone(), edit.text.clone());
                for seen in [&self.known, &self.reported] {
                    seen.borrow_mut()
                        .insert(edit.path.clone(), document.revision.clone());
                }
                self.pages
                    .borrow_mut()
                    .insert(edit.page.clone(), edit.path.clone());
                self.settle_draft(&edit.page, &edit.text, None)?;
                Ok(Some((edit.path.clone(), self.ack(&edit, document, true))))
            }
            Planned::Written { edit, disk, .. } => {
                self.settle_draft(&edit.page, &edit.text, None)?;
                Ok(Some((edit.path.clone(), self.ack(&edit, disk, false))))
            }
            Planned::Retire { step, disk, .. } => {
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
                Ok(None)
            }
            Planned::Retired => Ok(None),
        }
    }

    pub(super) fn transaction<T>(&self, work: impl FnOnce() -> Result<T>) -> Result<T> {
        self.connection.exec("BEGIN IMMEDIATE")?()?;
        match work() {
            Ok(value) => {
                self.connection.exec("COMMIT")?()?;
                Ok(value)
            }
            Err(error) => {
                self.connection.exec("ROLLBACK")?()?;
                Err(error)
            }
        }
    }

    /// The journals waiting in the state folder, in name order.
    fn waiting(&self) -> Vec<PathBuf> {
        let Ok(entries) = fs::read_dir(self.journals()) else {
            return Vec::new();
        };
        let mut journals: Vec<PathBuf> = entries
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.is_file()
                    && path
                        .extension()
                        .is_some_and(|extension| extension == "json")
            })
            .collect();
        journals.sort();
        journals
    }

    fn load(journal: &Path) -> Result<Journal> {
        let recorded: Journal = fs::read(journal)
            .context("Cannot read the journal")
            .and_then(|bytes| serde_json::from_slice(&bytes).context("The journal is malformed"))?;
        ensure!(
            !recorded.edits.is_empty() && recorded.edits.len() <= MAX_EDITS * 2,
            "The journal is malformed"
        );
        Ok(recorded)
    }

    /// Applies one journal's steps again and removes it once every step is done.
    fn finish(&self, journal: &Path) -> Result<()> {
        self.apply(&Self::load(journal)?.edits)?;
        fs::remove_file(journal)?;
        Ok(())
    }

    /// Rolls interrupted changes forward, as the reference `resumeTransactions` does, and
    /// reports what happened to each.
    pub(super) fn resume(&self) -> Vec<String> {
        self.waiting()
            .into_iter()
            .map(|journal| match self.finish(&journal) {
                Ok(()) => FINISHED.to_owned(),
                Err(error) => interrupted(&journal, &error),
            })
            .collect()
    }

    /// The journal named `name` in the state folder; a name with a path in it is refused.
    fn named(&self, name: &str) -> Result<PathBuf> {
        ensure!(
            !name.is_empty()
                && name.ends_with(".json")
                && !name.contains(['/', '\\'])
                && !name.starts_with('.'),
            "That change journal name is not valid. Reopen the notebook to list interrupted changes again."
        );
        let path = self.journals().join(name);
        ensure!(
            path.is_file(),
            "That interrupted change is gone; it may already be finished. Reopen the notebook to check."
        );
        Ok(path)
    }

    /// Interrupted changes that opening could not finish, each with where its notes stand.
    pub fn stuck_changes(&self) -> Vec<StuckChange> {
        self.waiting()
            .into_iter()
            .map(|journal| {
                let name = journal
                    .file_name()
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match Self::load(&journal) {
                    Ok(recorded) => StuckChange {
                        name,
                        created_at: recorded.created_at,
                        steps: recorded
                            .edits
                            .iter()
                            .map(|step| (step.path.clone(), self.state_of(step)))
                            .collect(),
                    },
                    Err(_) => StuckChange {
                        name,
                        created_at: 0,
                        steps: Vec::new(),
                    },
                }
            })
            .collect()
    }

    fn state_of(&self, step: &Step) -> StepState {
        let Ok(disk) = files::read(&self.root, &step.path) else {
            return StepState::Unreadable;
        };
        let revision = disk.as_ref().map(|disk| &disk.revision);
        match (step.remove, &disk) {
            (true, None) => StepState::Done,
            (false, Some(disk)) if disk.text == step.text => StepState::Done,
            _ if revision == step.expected_revision.as_ref() => StepState::Waiting,
            _ => StepState::ChangedElsewhere,
        }
    }

    /// Tries an interrupted change again, after its notes were put back or reviewed.
    pub fn retry_change(&self, name: &str) -> Result<()> {
        let journal = self.named(name)?;
        self.finish(&journal)
            .map_err(|error| anyhow::anyhow!(interrupted(&journal, &error)))
    }

    /// Stops trying an interrupted change: its notes stay as they are now, and its journal moves
    /// to `transactions/set-aside`, keeping every planned version. Returns where it went.
    pub fn set_aside_change(&self, name: &str) -> Result<PathBuf> {
        let journal = self.named(name)?;
        let folder = self.journals().join(SET_ASIDE);
        fs::create_dir_all(&folder).with_context(|| {
            format!("Cannot create {}. Check write access to Quire's application data folder and retry.", folder.display())
        })?;
        let target = folder.join(name);
        ensure!(
            !target.exists(),
            "{} already holds a set-aside change with this name. Move that file elsewhere and retry.",
            folder.display()
        );
        fs::rename(&journal, &target).with_context(|| {
            format!(
                "Cannot move the change journal to {}. Check write access and retry.",
                target.display()
            )
        })?;
        files::sync_directory(&folder)?;
        files::sync_directory(&self.journals())?;
        Ok(target)
    }
}

const FINISHED: &str = "Finished an interrupted change across notes, such as a rename. Its notes and links are now saved.";
const SET_ASIDE: &str = "set-aside";

/// Where one note of an interrupted change stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepState {
    /// The note already holds what the change planned.
    Done,
    /// The note still holds the version the change was planned from.
    Waiting,
    /// The note changed after the change was planned, so finishing would overwrite that.
    ChangedElsewhere,
    Unreadable,
}

/// An interrupted change across notes that opening could not finish. A journal that cannot be
/// read lists no steps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StuckChange {
    /// The journal's file name in the state folder.
    pub name: String,
    pub created_at: i64,
    pub steps: Vec<(String, StepState)>,
}

impl StuckChange {
    /// One sentence per kind of state, naming the notes.
    pub fn summary(&self) -> String {
        if self.steps.is_empty() {
            return "This interrupted change cannot be read. Set it aside to keep the file for review.".into();
        }
        let named = |state: StepState| {
            let mut paths: Vec<&str> = self
                .steps
                .iter()
                .filter(|(_, step)| *step == state)
                .map(|(path, _)| path.as_str())
                .collect();
            paths.dedup();
            paths.join(", ")
        };
        [
            (
                StepState::ChangedElsewhere,
                "Changed since the change was planned",
            ),
            (StepState::Unreadable, "Cannot be read"),
            (StepState::Waiting, "Still to update"),
            (StepState::Done, "Already updated"),
        ]
        .into_iter()
        .filter_map(|(state, label)| {
            let paths = named(state);
            (!paths.is_empty()).then(|| format!("{label}: {paths}."))
        })
        .collect::<Vec<_>>()
        .join(" ")
    }
}
/// One step of a change, checked against the disk before anything is written.
enum Planned<'a> {
    /// A note to write; `replaced` is the version it replaces, None for a new note.
    Write {
        step: &'a Step,
        edit: Edit,
        destination: PathBuf,
        replaced: Option<DiskDocument>,
    },
    /// A note that already holds its step's text.
    Written {
        step: &'a Step,
        edit: Edit,
        disk: DiskDocument,
    },
    /// A note to archive, still holding its planned version.
    Retire {
        step: &'a Step,
        source: PathBuf,
        archived: PathBuf,
        disk: DiskDocument,
    },
    /// A note already archived.
    Retired,
}

impl Planned<'_> {
    /// The folder a write stages its text in.
    fn folder(&self) -> Option<&Path> {
        match self {
            Self::Write { destination, .. } => destination.parent(),
            _ => None,
        }
    }
}

fn changed_before(path: &str) -> String {
    format!("{path} changed before the change across notes could finish")
}

fn ensure_unarchived(archived: &Path) -> Result<()> {
    match fs::symlink_metadata(archived) {
        Ok(_) => bail!(
            "An archived original already exists at {}. Keep both files and review the interrupted change.",
            archived.display()
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

/// Moves a note into Quire's archive of moved originals when it still holds the planned
/// version. A note already archived is done; one that changed stops the change.
fn archive(step: &Step, source: &Path, archived: &Path) -> Result<()> {
    let Some(disk) = files::read_at(&step.path, source)? else {
        return Ok(());
    };
    if Some(&disk.revision) != step.expected_revision.as_ref() {
        bail!(
            "{} changed before its old file could be archived. Both versions are kept; compare them before deleting the journal.",
            step.path
        );
    }
    if let Some(folder) = archived.parent() {
        fs::create_dir_all(folder)?;
    }
    ensure_unarchived(archived)?;
    fs::rename(source, archived).with_context(|| {
        format!("Cannot archive the old file {}. Check write access and reopen the notebook to finish the change.", step.path)
    })
}
