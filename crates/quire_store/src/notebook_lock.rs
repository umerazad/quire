//! One process edits a notebook at a time.
//!
//! A notebook keeps one recovery slot per page. Two processes editing the same page would each
//! record recovery from their own buffer, and the store refuses the later of two concurrent
//! recordings, so one editor would stop protecting its typing. Rather than give each session its
//! own slot (a schema change), Quire holds an exclusive operating-system lock on the notebook for
//! as long as a session has it open. The lock file holds no data: the operating system releases
//! the lock when the process exits or crashes, so a stale file never blocks the next open.

use anyhow::{Context as _, Result, bail};
use std::{
    fs::{File, OpenOptions, TryLockError},
    path::{Path, PathBuf},
};

/// What opening a notebook another process holds says.
pub const HELD: &str = "This notebook is already open in another Quire window or process. Switch to that window, or quit the other Quire, then open the notebook again.";

/// The lock for a Quire notebook's database: a dot file beside it, which backups and their
/// change fingerprint leave out.
pub fn store_lock(database: &Path) -> PathBuf {
    let name = database
        .file_name()
        .map_or_else(|| "local.sqlite".into(), |name| name.to_string_lossy());
    database.with_file_name(format!(".{name}.quire-lock"))
}

/// The lock for a folder notebook: a dot file in Quire's state folder for it, never inside the
/// notebook folder itself.
pub fn folder_lock(state: &Path) -> PathBuf {
    state.join(".open.lock")
}

/// An exclusive hold on a notebook, released when dropped.
#[derive(Debug)]
pub struct NotebookLock {
    _file: File,
    path: PathBuf,
}

impl NotebookLock {
    /// Takes the lock at `path`, creating the file in its existing folder when missing. Fails with [`HELD`]
    /// while any other holder, in this process or another, keeps it.
    pub fn acquire(path: &Path) -> Result<Self> {
        let cannot = || {
            format!(
                "Cannot lock the notebook at {}. Check write access to that folder and open the notebook again.",
                path.display()
            )
        };
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(path)
            .with_context(cannot)?;
        match file.try_lock() {
            Ok(()) => Ok(Self {
                _file: file,
                path: path.to_owned(),
            }),
            Err(TryLockError::WouldBlock) => bail!(HELD),
            Err(TryLockError::Error(error)) => Err(error).with_context(cannot),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        backup::{self, Source},
        notebook::{Draft, DraftStamp, Edit, NotebookStore},
    };
    use std::fs;

    fn store(root: &Path) -> PathBuf {
        let database = root.join("notebook").join("local.sqlite");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let store = NotebookStore::open(&database, "nb", "office").unwrap();
        store
            .save(&Edit {
                page: "p".into(),
                base: vec![],
                path: "Note.md".into(),
                text: "hello".into(),
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
        database
    }

    fn draft(text: &str, version: u64) -> Draft {
        Draft {
            page: "p".into(),
            path: "Note.md".into(),
            text: text.into(),
            base: vec![],
            stamp: DraftStamp {
                session: "first".into(),
                version,
            },
            created: 2,
        }
    }

    #[test]
    fn a_second_session_on_one_notebook_is_refused_and_the_first_keeps_recording() {
        let root = tempfile::tempdir().unwrap();
        let database = store(root.path());
        let path = store_lock(&database);
        let first = NotebookLock::acquire(&path).unwrap();
        let refused = NotebookLock::acquire(&path).unwrap_err().to_string();
        assert_eq!(refused, HELD);
        let held = NotebookStore::open(&database, "nb", "office").unwrap();
        let stamp = held.put_draft(&draft("one", 1), None).unwrap();
        held.put_draft(&draft("two", 2), Some(&stamp)).unwrap();
        assert_eq!(held.draft("p").unwrap().unwrap().text, "two");
        drop(first);
        let second = NotebookLock::acquire(&path).unwrap();
        assert_eq!(second.path(), path);
    }

    #[test]
    fn different_notebooks_lock_independently_and_a_crashed_holder_leaves_nothing_held() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("one").join("local.sqlite");
        let second = root.path().join("two").join("local.sqlite");
        let state = root.path().join("state");
        for folder in ["one", "two", "state"] {
            fs::create_dir(root.path().join(folder)).unwrap();
        }
        let held = [
            NotebookLock::acquire(&store_lock(&first)).unwrap(),
            NotebookLock::acquire(&store_lock(&second)).unwrap(),
            NotebookLock::acquire(&folder_lock(&state)).unwrap(),
        ];
        assert!(NotebookLock::acquire(&folder_lock(&state)).is_err());
        drop(held);
        assert!(store_lock(&first).exists());
        NotebookLock::acquire(&store_lock(&first)).unwrap();
        NotebookLock::acquire(&folder_lock(&state)).unwrap();
    }

    #[test]
    fn locks_stay_out_of_backups_and_their_change_fingerprint() {
        let root = tempfile::tempdir().unwrap();
        let database = store(root.path());
        let destination = root.path().join("backups");
        fs::create_dir(&destination).unwrap();
        let source = Source::Store {
            database: database.clone(),
        };
        let made = backup::create(&source, &destination, 1).unwrap();
        let before = backup::fingerprint(&source).unwrap();
        let _lock = NotebookLock::acquire(&store_lock(&database)).unwrap();
        assert_eq!(backup::fingerprint(&source).unwrap(), before);
        backup::verify(&made.path).unwrap();

        let notes = root.path().join("notes");
        let state = root.path().join("folder-state");
        fs::create_dir_all(&notes).unwrap();
        fs::write(notes.join("Note.md"), "- hello\n").unwrap();
        crate::folder_notebook::FolderNotebook::open(&notes, &state).unwrap();
        let folder = Source::Folder {
            root: notes,
            state: state.clone(),
        };
        let made = backup::create(&folder, &destination, 3).unwrap();
        let before = backup::fingerprint(&folder).unwrap();
        let _lock = NotebookLock::acquire(&folder_lock(&state)).unwrap();
        assert_eq!(backup::fingerprint(&folder).unwrap(), before);
        assert!(!made.path.join("state").join(".open.lock").exists());
    }
}
