//! Folder notebooks: ordinary Markdown files in a folder the user owns, edited in place.
//!
//! The adapter answers the same page operations the editor asks of a Quire notebook store
//! (heads, checkout, drafts, saves), with folder semantics. A page's identity derives from its
//! path, and its single head is the SHA-256 of the file's text. A save names the version it was
//! made from and is refused when the file changed on disk since, so another application's edit
//! is never overwritten; the refusal carries the disk version for comparison. Saves record
//! recovery first, write a synced temporary file beside the note, check the disk again, keep the
//! replaced text in history, and rename. Recovery and history live in a state folder outside the
//! notebook, so opening a folder never changes it.

mod census;
mod deletion;
mod files;
mod history;
mod journal;
mod markdown_import;
mod reference;
mod settings;

pub use census::{FolderCensus, hashed_listing};
pub use deletion::{NO_TRASH, Trash};
pub use files::{
    DiskDocument, ListedFile, Listing, MAX_NOTE_BYTES, is_markdown, list, read, revision, validate,
};
pub(crate) use files::{barrier, flush, flush_directory};
pub use history::FolderRevision;
pub use journal::{StepState, StuckChange};
pub use reference::{RecoveredDraft, ReferenceTally};
pub(crate) use settings::METADATA;

use crate::{
    database,
    notebook::{
        CheckedSaves, Draft, DraftStamp, Edit, PageCheckout, SaveAck, SavedVersion,
        VERSION_UNAVAILABLE,
    },
    sync::{format::page_id_for_path, json::Json},
    worker::{Client, QueueError, QueueLimits, Reply, Worker},
};
use anyhow::{Context as _, Result, ensure};
use files::Published;
use serde_json::json;
use sqlez::connection::Connection;
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashMap},
    fmt,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

const STATE_FILE: &str = "folder.sqlite";

/// Quire's recovery, history, index, and workspace for the folder at `root`, kept in Quire's
/// data folder `data` so opening a folder never writes inside it.
pub fn state_folder_in(data: &Path, root: &Path) -> PathBuf {
    data.join("folder-notebooks")
        .join(&crate::sync::format::sha256(root.to_string_lossy().as_bytes())[..16])
}

/// A save refused because the file on disk is not the version the save was made from. Nothing
/// was written; the text is kept in recovery. `disk` is the file's current contents, or None
/// when it no longer exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderConflict {
    pub page: String,
    pub path: String,
    pub disk: Option<DiskDocument>,
    pub message: String,
}

impl fmt::Display for FolderConflict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for FolderConflict {}

/// A note whose disk contents differ from what this adapter last read or wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderChange {
    Changed {
        page: String,
        document: DiskDocument,
    },
    Removed {
        page: String,
        path: String,
    },
}

impl FolderChange {
    pub fn page(&self) -> &str {
        match self {
            Self::Changed { page, .. } | Self::Removed { page, .. } => page,
        }
    }
}

pub struct FolderNotebook {
    root: PathBuf,
    connection: Connection,
    /// Page identity to path, from the last listing and every read since.
    pages: RefCell<BTreeMap<String, String>>,
    /// The revision last read or written for each path.
    known: RefCell<HashMap<String, String>>,
    /// The revision last reported to the editor by `refresh`, or written here. Reads for other
    /// purposes never update it, so a change they see is still reported.
    reported: RefCell<HashMap<String, String>>,
    warnings: RefCell<Vec<String>>,
    /// What opening found and did, such as finishing an interrupted rename, for the user to read.
    notices: RefCell<Vec<String>>,
    /// Pages that gained a recovery draft from the reference app's records on opening.
    recovered: RefCell<Vec<RecoveredDraft>>,
    /// The reference app's history still to take in, by `import_reference_history`.
    backlog: RefCell<Vec<reference::Backlog>>,
    /// Quire's state folder for this notebook.
    state: PathBuf,
    clock: Box<dyn Fn() -> i64>,
    /// Where deletions go; None refuses them.
    trash: Option<Trash>,
    /// Stops a change across notes before the step with this index, or after every step at the
    /// step count.
    #[cfg(test)]
    interrupt_at: std::cell::Cell<Option<usize>>,
    /// Stops a change across notes once every new text is staged, before history is recorded.
    #[cfg(test)]
    interrupt_staged: std::cell::Cell<bool>,
}

fn system_clock() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

impl FolderNotebook {
    /// Opens `root` as a folder notebook, keeping recovery and history in `state`, a folder
    /// outside the notebook that belongs to this root alone.
    pub fn open(root: &Path, state: &Path) -> Result<Self> {
        Self::open_with_clock(root, state, Box::new(system_clock))
    }

    pub fn open_with_clock(root: &Path, state: &Path, clock: Box<dyn Fn() -> i64>) -> Result<Self> {
        let notebook = Self::open_deferring_history(root, state, clock)?;
        let failed = notebook.finish_reference_history();
        notebook.notices.borrow_mut().extend(failed);
        Ok(notebook)
    }

    /// Opens like `open_with_clock` but leaves the reference app's history queued for
    /// `import_reference_history`, so the first page loads without waiting on years of saved
    /// versions.
    pub fn open_deferring_history(
        root: &Path,
        state: &Path,
        clock: Box<dyn Fn() -> i64>,
    ) -> Result<Self> {
        ensure!(
            root.is_absolute() && state.is_absolute(),
            "Cannot open notebook folder {}. Choose an absolute folder path.",
            root.display()
        );
        let metadata = std::fs::metadata(root).with_context(|| {
            format!(
                "Cannot open notebook folder {}. Make it available locally and retry.",
                root.display()
            )
        })?;
        ensure!(
            metadata.is_dir(),
            "{} is not a folder. Choose the folder that holds the notebook's Markdown files.",
            root.display()
        );
        let real_root = std::fs::canonicalize(root)?;
        ensure!(
            !state.starts_with(root) && !state.starts_with(&real_root),
            "Quire's state folder {} is inside the notebook. Choose a state folder outside the notebook so opening it changes nothing there.",
            state.display()
        );
        let real_state = std::fs::create_dir_all(state)
            .and_then(|()| std::fs::canonicalize(state))
            .with_context(|| format!("Cannot create Quire's state folder {}. Check write access to the application data folder and retry.", state.display()))?;
        ensure!(
            !real_state.starts_with(&real_root),
            "Quire's state folder {} is inside the notebook. Choose a state folder outside the notebook so opening it changes nothing there.",
            state.display()
        );
        let connection = database::open_writable(&state.join(STATE_FILE))?;
        let root_key = real_root.to_string_lossy().into_owned();
        connection.exec("BEGIN IMMEDIATE")?()?;
        let prepared = (|| -> Result<()> {
            for table in [
                "CREATE TABLE IF NOT EXISTS meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), root TEXT NOT NULL)",
                "CREATE TABLE IF NOT EXISTS drafts (page TEXT PRIMARY KEY, path TEXT NOT NULL, session TEXT NOT NULL, version INTEGER NOT NULL, body TEXT NOT NULL)",
            ]
            .iter()
            .chain(history::TABLES)
            .chain(reference::TABLES)
            {
                connection.exec(table)?()?;
            }
            let stored: Option<String> =
                connection.select_row("SELECT root FROM meta WHERE singleton=1")?()?;
            match stored {
                None => connection.exec_bound("INSERT INTO meta(singleton,root) VALUES (1,?)")?(
                    root_key.as_str(),
                )?,
                Some(stored) => ensure!(
                    stored == root_key,
                    "Quire's state folder {} belongs to the notebook at {stored}. Open that notebook, or choose a fresh state folder for this one.",
                    state.display()
                ),
            }
            Ok(())
        })();
        match prepared {
            Ok(()) => connection.exec("COMMIT")?()?,
            Err(error) => {
                connection.exec("ROLLBACK")?()?;
                return Err(error);
            }
        }
        let notebook = Self {
            root: root.to_owned(),
            connection,
            pages: RefCell::default(),
            known: RefCell::default(),
            reported: RefCell::default(),
            warnings: RefCell::default(),
            notices: RefCell::default(),
            recovered: RefCell::default(),
            backlog: RefCell::default(),
            state: state.to_owned(),
            clock,
            trash: None,
            #[cfg(test)]
            interrupt_at: std::cell::Cell::default(),
            #[cfg(test)]
            interrupt_staged: std::cell::Cell::default(),
        };
        let legacy = settings::adopt_legacy_metadata(&real_root);
        notebook.listing();
        let mut notices: Vec<String> = legacy.into_iter().collect();
        notices.extend(notebook.adopt_reference_journals());
        notices.extend(notebook.resume());
        let (recovered, imported) = notebook.import_reference();
        notices.extend(imported);
        *notebook.notices.borrow_mut() = notices;
        *notebook.recovered.borrow_mut() = recovered;
        notebook.listing();
        Ok(notebook)
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Lists the notebook again and refreshes which page each path belongs to. Two paths that
    /// differ only in case share a page identity; the first in path order keeps it, with a
    /// warning naming the other.
    pub fn listing(&self) -> Listing {
        let mut listing = list(&self.root);
        let mut pages = BTreeMap::new();
        for file in &listing.files {
            let page = page_id_for_path(&file.path);
            if let Some(first) = pages.get(&page) {
                listing.warnings.push(format!(
                    "{} and {first} differ only in letter case, so Quire opens {first}. Rename one of them to open both.",
                    file.path
                ));
            } else {
                pages.insert(page, file.path.clone());
            }
        }
        *self.pages.borrow_mut() = pages;
        *self.warnings.borrow_mut() = listing.warnings.clone();
        listing
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.borrow().clone()
    }

    /// What opening found and did, such as finishing an interrupted rename.
    pub fn notices(&self) -> Vec<String> {
        self.notices.borrow().clone()
    }

    /// Pages that gained a recovery draft from the reference app's records on opening.
    pub fn recovered_drafts(&self) -> Vec<RecoveredDraft> {
        self.recovered.borrow().clone()
    }

    /// The path a page lives at, when the notebook has one.
    pub fn path_of(&self, page: &str) -> Option<String> {
        self.pages.borrow().get(page).cloned()
    }

    fn read(&self, path: &str) -> Result<Option<DiskDocument>> {
        let document = files::read(&self.root, path)?;
        let mut known = self.known.borrow_mut();
        match &document {
            Some(document) => {
                known.insert(path.to_owned(), document.revision.clone());
                self.pages
                    .borrow_mut()
                    .insert(page_id_for_path(path), path.to_owned());
            }
            None => {
                known.remove(path);
            }
        }
        Ok(document)
    }

    fn record(page: &str, document: &DiskDocument) -> Json {
        Json::from(&json!({
            "id": document.revision,
            "page": page,
            "path": document.path,
            "text": document.text,
            "deleted": false,
            "parents": [],
        }))
    }

    /// The page's file now. A page not seen yet, such as a note another application just
    /// created, is looked for by listing the notebook again.
    fn current(&self, page: &str) -> Result<Option<DiskDocument>> {
        let path = self.path_of(page).or_else(|| {
            self.listing();
            self.path_of(page)
        });
        path.map_or(Ok(None), |path| self.read(&path))
    }

    /// The page's file revision, or nothing when it has no file.
    pub fn heads(&self, page: &str) -> Result<Vec<String>> {
        Ok(self
            .current(page)?
            .map(|document| vec![document.revision])
            .unwrap_or_default())
    }

    /// The page's file and any recovery draft. `selected` must be the file's current revision.
    pub fn checkout(&self, page: &str, selected: Option<&str>) -> Result<PageCheckout> {
        let current = self.current(page)?;
        if let Some(selected) = selected {
            ensure!(
                current
                    .as_ref()
                    .is_some_and(|document| document.revision == selected),
                "The selected version is no longer this note's contents on disk. Keep the buffer and reopen the page."
            );
        }
        Ok(PageCheckout {
            heads: current
                .as_ref()
                .map(|document| vec![document.revision.clone()])
                .unwrap_or_default(),
            selected: current
                .as_ref()
                .map(|document| Self::record(page, document)),
            draft: self.draft(page)?,
        })
    }

    /// A version by revision: a note's current text, or a replaced text kept in history.
    pub fn get(&self, revision: &str) -> Result<Option<Json>> {
        let candidates: Vec<String> = self
            .known
            .borrow()
            .iter()
            .filter(|(_, known)| known.as_str() == revision)
            .map(|(path, _)| path.clone())
            .collect();
        for path in candidates {
            if let Some(document) = self.read(&path)?
                && document.revision == revision
            {
                return Ok(Some(Self::record(&page_id_for_path(&path), &document)));
            }
        }
        Ok(
            history::by_revision(&self.connection, revision)?.map(|(path, text)| {
                Self::record(&page_id_for_path(&path), &DiskDocument::new(path, text))
            }),
        )
    }

    pub fn draft(&self, page: &str) -> Result<Option<Draft>> {
        let body: Option<String> =
            self.connection
                .select_row_bound("SELECT body FROM drafts WHERE page=?")?(page)?;
        body.map(|body| {
            serde_json::from_str(&body).context(
                "A recovery draft is damaged. Keep Quire's state folder and reopen the notebook.",
            )
        })
        .transpose()
    }

    /// Records unsaved text durably before any save touches the file. A draft from another
    /// editor session is replaced only by a writer that saw it.
    pub fn put_draft(&self, draft: &Draft, expected: Option<&DraftStamp>) -> Result<DraftStamp> {
        ensure!(
            !draft.page.is_empty() && !draft.stamp.session.is_empty(),
            "Recovery snapshot has no page or editor session. Reopen the page and retry."
        );
        files::validate(&draft.path)?;
        let current = self.draft(&draft.page)?;
        if current.as_ref() == Some(draft) {
            return Ok(draft.stamp.clone());
        }
        if let Some(current) = &current {
            if current.stamp.session == draft.stamp.session {
                ensure!(
                    draft.stamp.version > current.stamp.version,
                    "A stale recovery snapshot cannot replace newer typing. Record the current buffer instead."
                );
            } else {
                ensure!(
                    expected == Some(&current.stamp),
                    "Recovery changed while recording this snapshot. Keep the buffer and retry from the latest recovery version."
                );
            }
        }
        self.write_draft(draft)?;
        Ok(draft.stamp.clone())
    }

    fn write_draft(&self, draft: &Draft) -> Result<()> {
        self.connection.exec_bound(
            "INSERT INTO drafts(page,path,session,version,body) VALUES (?,?,?,?,?) ON CONFLICT(page) DO UPDATE SET path=excluded.path,session=excluded.session,version=excluded.version,body=excluded.body",
        )?((
            draft.page.as_str(),
            draft.path.as_str(),
            draft.stamp.session.as_str(),
            i64::try_from(draft.stamp.version)?,
            serde_json::to_string(draft)?.as_str(),
        ))
        .context("Cannot record recovery. Check free space in Quire's application data folder and retry before closing.")
    }

    /// Removes a page's draft when it holds `text` or was superseded by the save stamped `stamp`.
    fn settle_draft(&self, page: &str, text: &str, stamp: Option<&DraftStamp>) -> Result<()> {
        if let Some(current) = self.draft(page)?
            && (current.text == text
                || stamp.is_some_and(|stamp| {
                    stamp.session == current.stamp.session && stamp.version >= current.stamp.version
                }))
        {
            self.connection
                .exec_bound("DELETE FROM drafts WHERE page=?")?(page)?;
        }
        Ok(())
    }

    /// Discards a page's draft when it still holds `text`, after the user chose another version.
    pub fn clear_draft(&self, page: &str, text: &str) -> Result<()> {
        self.settle_draft(page, text, None)
    }

    /// Discards a page's draft whatever it holds, after the user chose the disk version.
    pub fn discard_draft(&self, page: &str) -> Result<()> {
        self.connection
            .exec_bound("DELETE FROM drafts WHERE page=?")?(page)
            .context("Cannot remove the recovery copy. Check write access to Quire's application data folder and retry.")
    }

    /// Writes `edit.text` to the page's file when the file still holds `edit.base`: one revision,
    /// or none for a new note. Any other disk state refuses with a [`FolderConflict`].
    pub fn save(&self, edit: &Edit) -> Result<SaveAck> {
        self.save_with(edit, &|| Ok(()))
    }

    fn save_with(&self, edit: &Edit, before_publish: &dyn Fn() -> Result<()>) -> Result<SaveAck> {
        ensure!(
            !edit.deleted,
            "A save cannot delete a note. Use Delete, which moves the note to the system Trash."
        );
        ensure!(
            edit.base.len() <= 1,
            "This save does not name one version of the note. Keep the buffer and reopen the page."
        );
        ensure!(
            edit.text.len() as u64 <= MAX_NOTE_BYTES,
            "The note could not be saved because it exceeds 10 MB. Split the note and retry."
        );
        files::validate(&edit.path)?;
        ensure!(
            page_id_for_path(&edit.path) == edit.page,
            "{} belongs to another page in this folder notebook. Keep the buffer and reopen the page.",
            edit.path
        );
        self.preflight(edit)?;
        let now = (self.clock)();
        let stamp = edit.recovery.clone().unwrap_or_else(|| DraftStamp {
            session: "folder-save".into(),
            version: now.max(0) as u64,
        });
        let draft = Draft {
            page: edit.page.clone(),
            path: edit.path.clone(),
            text: edit.text.clone(),
            base: edit.base.clone(),
            stamp,
            created: now.max(0) as u64,
        };
        if self.draft(&edit.page)?.as_ref() != Some(&draft) {
            self.write_draft(&draft)?;
        }
        let destination = files::resolve(&self.root, &edit.path, true)?;
        let disk = self.read(&edit.path)?;
        let expected = edit.base.first();
        if let Some(disk) = &disk
            && disk.text == edit.text
        {
            self.settle_draft(&edit.page, &edit.text, edit.recovery.as_ref())?;
            return Ok(self.ack(edit, disk.clone(), false));
        }
        if disk.as_ref().map(|disk| &disk.revision) != expected {
            return Err(self.conflict(edit, disk, "This note changed on disk. Your edits are in recovery. Compare the versions or save a separate copy.").into());
        }
        let staged = files::stage(&destination, &edit.text)?;
        before_publish()?;
        let latest = files::read_at(&edit.path, &destination)?;
        if latest.as_ref().map(|latest| &latest.revision) != expected {
            return Err(self.conflict(edit, latest, "Another application changed the note during saving. Compare the versions before retrying.").into());
        }
        if let Some(disk) = &disk {
            history::add(
                &self.connection,
                &edit.path,
                &disk.text,
                &disk.revision,
                now,
            )?;
        }
        if staged.publish(disk.is_some())? == Published::Exists {
            let found = self.read(&edit.path)?;
            return Err(self.conflict(edit, found, "A file with this name was created elsewhere. Choose another name or compare its content.").into());
        }
        let document = DiskDocument::new(edit.path.clone(), edit.text.clone());
        self.known
            .borrow_mut()
            .insert(edit.path.clone(), document.revision.clone());
        self.reported
            .borrow_mut()
            .insert(edit.path.clone(), document.revision.clone());
        self.pages
            .borrow_mut()
            .insert(edit.page.clone(), edit.path.clone());
        self.settle_draft(&edit.page, &edit.text, edit.recovery.as_ref())?;
        Ok(self.ack(edit, document, true))
    }

    /// Refuses a save before anything is written when the note holds unsaved work from another
    /// editor session, such as a draft left by a crash that no editor has opened, or when the
    /// note is read-only. A rename replaces a read-only file whenever its folder is writable.
    fn preflight(&self, edit: &Edit) -> Result<()> {
        if let Some(current) = self.draft(&edit.page)?
            && current.text != edit.text
            && edit
                .recovery
                .as_ref()
                .is_none_or(|stamp| stamp.session != current.stamp.session)
        {
            let disk = self.read(&edit.path)?;
            return Err(self.conflict(edit, disk, "This note has unsaved work recovered from an earlier session. Open the page and choose a version before changing it here.").into());
        }
        ensure!(
            !std::fs::metadata(self.root.join(&edit.path))
                .is_ok_and(|metadata| metadata.permissions().readonly()),
            "{} is read-only. Make it writable in Finder or Explorer, then save again; your text is kept in recovery.",
            edit.path
        );
        Ok(())
    }

    fn conflict(&self, edit: &Edit, disk: Option<DiskDocument>, message: &str) -> FolderConflict {
        FolderConflict {
            page: edit.page.clone(),
            path: edit.path.clone(),
            disk,
            message: message.into(),
        }
    }

    fn ack(&self, edit: &Edit, document: DiskDocument, changed: bool) -> SaveAck {
        SaveAck {
            heads: vec![document.revision.clone()],
            revision: Self::record(&edit.page, &document),
            conflict: false,
            changed,
            replaced: None,
            stamp: edit.recovery.clone(),
            merged: None,
            overlaps: 0,
        }
    }

    /// Rereads `paths` and reports each note whose contents differ from what was last read or
    /// written here, including notes that disappeared. Paths outside the notebook's Markdown
    /// files are ignored.
    pub fn refresh(&self, paths: &[String]) -> Result<Vec<FolderChange>> {
        let mut changes = Vec::new();
        for path in paths {
            if files::validate(path).is_err() {
                continue;
            }
            let before = self.reported.borrow().get(path).cloned();
            let page = page_id_for_path(path);
            match self.read(path) {
                Ok(Some(document)) => {
                    if before.as_ref() != Some(&document.revision) {
                        self.reported
                            .borrow_mut()
                            .insert(path.clone(), document.revision.clone());
                        changes.push(FolderChange::Changed { page, document });
                    }
                }
                Ok(None) => {
                    if before.is_some() || self.path_of(&page).as_deref() == Some(path.as_str()) {
                        self.reported.borrow_mut().remove(path);
                        self.pages.borrow_mut().remove(&page);
                        changes.push(FolderChange::Removed {
                            page,
                            path: path.clone(),
                        });
                    }
                }
                Err(error) => self.warnings.borrow_mut().push(format!("{error:#}")),
            }
        }
        Ok(changes)
    }

    /// Lists the notebook and rereads every note this adapter has read, catching changes a
    /// watcher missed.
    pub fn rescan(&self) -> Result<Vec<FolderChange>> {
        self.listing();
        let mut tracked: Vec<String> = self
            .known
            .borrow()
            .keys()
            .chain(self.reported.borrow().keys())
            .cloned()
            .collect();
        tracked.sort();
        tracked.dedup();
        self.refresh(&tracked)
    }

    /// The reference app's notebook settings, `.quire/notebook.json`, with its revision.
    pub fn settings(&self) -> Result<Option<DiskDocument>> {
        settings::read(&self.root)
    }

    /// Replaces `.quire/notebook.json` when it still has revision `base`. Returns the new revision.
    pub fn save_settings(&self, text: &str, base: Option<&str>) -> Result<String> {
        settings::write(&self.root, text, base)
    }

    /// Saved versions of `path`, newest first.
    pub fn revisions(&self, path: &str) -> Result<Vec<FolderRevision>> {
        history::list(&self.connection, path)
    }

    pub fn revision_text(&self, path: &str, id: i64) -> Result<String> {
        history::text(&self.connection, path, id)?.context(
            "That saved revision is unavailable. Refresh page history and choose another version.",
        )
    }

    /// `page`'s saved versions in the store-shaped form page history lists, newest first.
    pub fn history(&self, page: &str) -> Result<Vec<SavedVersion>> {
        let Some(path) = self.path_of(page) else {
            return Ok(Vec::new());
        };
        Ok(self
            .revisions(&path)?
            .into_iter()
            .map(|revision| SavedVersion {
                id: revision.id.to_string(),
                saved_at: revision.saved_at,
            })
            .collect())
    }

    pub fn version_text(&self, page: &str, id: &str) -> Result<String> {
        let path = self.path_of(page).context(VERSION_UNAVAILABLE)?;
        self.revision_text(&path, id.parse().context(VERSION_UNAVAILABLE)?)
    }
}

pub type FolderClient = Client<FolderNotebook>;
pub type FolderWorker = Worker<FolderNotebook>;

impl Worker<FolderNotebook> {
    /// Starts the folder adapter on its own thread, without touching the disk on the caller's.
    /// Deletions go to `trash`, or are refused without one.
    pub fn spawn_folder(
        root: PathBuf,
        state: PathBuf,
        trash: Option<Trash>,
        limits: QueueLimits,
    ) -> Result<Self> {
        Self::spawn_with("quire-folder", limits, move || {
            let mut notebook = FolderNotebook::open_deferring_history(&root, &state, Box::new(system_clock))?;
            notebook.trash = trash;
            Ok(notebook)
        })
        .context("Cannot start the folder notebook worker. Keep the buffer and retry after freeing process resources.")
    }
}

type Answer<T> = std::result::Result<Reply<T>, QueueError>;

fn edit_bytes(edit: &Edit) -> usize {
    256 + edit.page.len() + edit.path.len() + edit.text.len()
}

impl Client<FolderNotebook> {
    pub fn heads(&self, page: String) -> Answer<Vec<String>> {
        self.request(128 + page.len(), move |notebook| notebook.heads(&page))
    }
    pub fn checkout(&self, page: String, selected: Option<String>) -> Answer<PageCheckout> {
        self.request(128 + page.len(), move |notebook| {
            notebook.checkout(&page, selected.as_deref())
        })
    }
    pub fn get(&self, id: String) -> Answer<Option<Json>> {
        self.request(128 + id.len(), move |notebook| notebook.get(&id))
    }
    pub fn draft(&self, page: String) -> Answer<Option<Draft>> {
        self.request(128 + page.len(), move |notebook| notebook.draft(&page))
    }
    pub fn put_draft(&self, draft: Draft, expected: Option<DraftStamp>) -> Answer<DraftStamp> {
        self.request(256 + draft.text.len() + draft.path.len(), move |notebook| {
            notebook.put_draft(&draft, expected.as_ref())
        })
    }
    pub fn clear_draft(&self, page: String, text: String) -> Answer<()> {
        self.request(128 + text.len(), move |notebook| {
            notebook.clear_draft(&page, &text)
        })
    }
    pub fn discard_draft(&self, page: String) -> Answer<()> {
        self.request(128 + page.len(), move |notebook| {
            notebook.discard_draft(&page)
        })
    }
    pub fn save(&self, edit: Edit) -> Answer<SaveAck> {
        self.request(edit_bytes(&edit), move |notebook| notebook.save(&edit))
    }
    pub fn save_many_if_heads(&self, edits: Vec<Edit>) -> Answer<CheckedSaves> {
        let bytes = edits.iter().map(edit_bytes).sum();
        self.request(bytes, move |notebook| notebook.save_many_if_heads(&edits))
    }
    pub fn delete_if_heads(
        &self,
        edits: Vec<Edit>,
        folder: Option<String>,
    ) -> Answer<CheckedSaves> {
        let bytes = edits.iter().map(edit_bytes).sum();
        self.request(bytes, move |notebook| {
            notebook.delete_if_heads(&edits, folder.as_deref())
        })
    }
    pub fn other_files(&self, folder: String) -> Answer<Vec<String>> {
        self.request(128 + folder.len(), move |notebook| {
            Ok(notebook.other_files(&folder))
        })
    }
    pub fn refresh(&self, paths: Vec<String>) -> Answer<Vec<FolderChange>> {
        let bytes = paths.iter().map(String::len).sum::<usize>() + 128;
        self.request(bytes, move |notebook| notebook.refresh(&paths))
    }
    pub fn rescan(&self) -> Answer<Vec<FolderChange>> {
        self.request(128, FolderNotebook::rescan)
    }
    pub fn recovered_drafts(&self) -> Answer<Vec<RecoveredDraft>> {
        self.request(128, |notebook| Ok(notebook.recovered_drafts()))
    }
    pub fn notices(&self) -> Answer<Vec<String>> {
        self.request(128, |notebook| Ok(notebook.notices()))
    }
    pub fn listing(&self) -> Answer<Listing> {
        self.request(128, |notebook| Ok(notebook.listing()))
    }
    pub fn settings(&self) -> Answer<Option<DiskDocument>> {
        self.request(128, FolderNotebook::settings)
    }
    pub fn save_settings(&self, text: String, base: Option<String>) -> Answer<String> {
        self.request(128 + text.len(), move |notebook| {
            notebook.save_settings(&text, base.as_deref())
        })
    }
    pub fn revisions(&self, path: String) -> Answer<Vec<FolderRevision>> {
        self.request(128 + path.len(), move |notebook| notebook.revisions(&path))
    }
    pub fn revision_text(&self, path: String, id: i64) -> Answer<String> {
        self.request(128 + path.len(), move |notebook| {
            notebook.revision_text(&path, id)
        })
    }
    pub fn history(&self, page: String) -> Answer<Vec<SavedVersion>> {
        self.request(128 + page.len(), move |notebook| notebook.history(&page))
    }
    pub fn stuck_changes(&self) -> Answer<Vec<StuckChange>> {
        self.request(128, |notebook| Ok(notebook.stuck_changes()))
    }
    pub fn retry_change(&self, name: String) -> Answer<()> {
        self.request(128 + name.len(), move |notebook| {
            notebook.retry_change(&name)
        })
    }
    pub fn set_aside_change(&self, name: String) -> Answer<PathBuf> {
        self.request(128 + name.len(), move |notebook| {
            notebook.set_aside_change(&name)
        })
    }
    pub fn version_text(&self, page: String, id: String) -> Answer<String> {
        self.request(128 + page.len() + id.len(), move |notebook| {
            notebook.version_text(&page, &id)
        })
    }
    /// Takes in another step of the reference app's history queued on opening. Returns how many
    /// saved versions remain, none once it is all in.
    pub fn import_reference_history(&self) -> Answer<usize> {
        self.request(64, |notebook| {
            notebook
                .import_reference_history(reference::HISTORY_STEP)
                .map(|more| {
                    if more {
                        notebook.reference_history_left()
                    } else {
                        0
                    }
                })
        })
    }
    /// How many of the reference app's saved versions are queued to take in.
    pub fn reference_history_left(&self) -> Answer<usize> {
        self.request(64, |notebook| Ok(notebook.reference_history_left()))
    }
}

#[cfg(test)]
mod rt_tests;
#[cfg(test)]
mod tests;
