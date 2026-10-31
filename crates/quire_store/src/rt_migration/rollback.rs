//! Going back to `rt` after using Quire, and what `rt` then sees.
//!
//! Folder notebooks need nothing: `rt` reads the same files, so every note Quire saved is there.
//! What `rt` cannot see is Quire's own state: unsaved Quire drafts and the versions Quire kept in
//! its history. A Quire notebook's saves reach `rt` only through the synced change log, written
//! under Quire's own writer identity. That publication is the one one-way door here: `rt` and
//! every other computer keep a published change forever, so it happens only when the caller
//! confirms it (`publish`). Afterwards a second, independent replica reads the folder from
//! scratch, as `rt` would, and must arrive at the same pages Quire shows.

use super::{Migrated, NotebookKind, Record, report::summary};
use crate::{
    folder_notebook::{FolderNotebook, state_folder_in},
    notebook::NotebookStore,
    notebook_lock::{NotebookLock, folder_lock, store_lock},
    quire_folder::import_log,
};
use anyhow::{Context as _, Result};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RollbackNotebook {
    pub kind: NotebookKind,
    pub source: PathBuf,
    /// What `rt` will see, and anything that keeps it from seeing all of Quire's work.
    pub lines: Vec<String>,
    /// Whether `rt` sees every save made in Quire.
    pub ready: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RollbackReport {
    pub notebooks: Vec<RollbackNotebook>,
    /// Saves published to synced folders by this call.
    pub published: usize,
}

impl RollbackReport {
    pub fn ready(&self) -> bool {
        self.notebooks.iter().all(|notebook| notebook.ready)
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::from("# Returning to rt\n\n");
        out.push_str(if self.ready() {
            "rt sees every save made in Quire. Quit Quire, then open rt.\n\n"
        } else {
            "Some Quire work is not yet visible to rt; see below.\n\n"
        });
        for notebook in &self.notebooks {
            out.push_str(&format!("## {}\n\n", notebook.source.display()));
            for line in &notebook.lines {
                out.push_str(&format!("- {line}\n"));
            }
            out.push('\n');
        }
        out
    }
}

/// Paths of drafts recorded by Quire's editor rather than imported from `rt`.
fn quire_drafts(drafts: &[(String, String)]) -> Vec<String> {
    drafts
        .iter()
        .filter(|(_, session)| !session.starts_with("reference:") && session != "rt-migration")
        .map(|(path, _)| path.clone())
        .collect()
}

fn folder(root: &Path, state: &Path) -> Result<RollbackNotebook> {
    let _lock =
        NotebookLock::acquire(&folder_lock(state)).context("Close this notebook in Quire first")?;
    let notebook = FolderNotebook::open(root, state)?;
    let census = notebook.census()?;
    let unsaved = quire_drafts(&census.drafts);
    let mut lines = vec![
        format!(
            "rt reads the notes in place: {} (count · hash of path and bytes), {} other files.",
            summary(&census.pages),
            census.others.len()
        ),
        format!(
            "Quire's {} saved versions stay in Quire's history; rt shows its own history.",
            census.history
        ),
    ];
    if !unsaved.is_empty() {
        lines.push(format!(
            "Unsaved typing in Quire that rt cannot see: {}. Open each in Quire and save it, or it stays in Quire only.",
            unsaved.join(", ")
        ));
    }
    Ok(RollbackNotebook {
        kind: NotebookKind::Folder,
        source: root.to_owned(),
        lines,
        ready: unsaved.is_empty(),
    })
}

fn quire(
    folder: &Path,
    database: &Path,
    notebook: &str,
    writer: &str,
    publish: bool,
    now: u64,
    published: &mut usize,
) -> Result<RollbackNotebook> {
    let _lock = NotebookLock::acquire(&store_lock(database))
        .context("Close this notebook in Quire first")?;
    let store = NotebookStore::open(database, notebook, writer)?;
    let mut lines: Vec<String> = import_log(&store, folder)?
        .into_iter()
        .map(|problem| format!("Could not read part of the synced folder: {problem}"))
        .collect();
    let unreadable = !lines.is_empty();
    let before = store.census()?;
    if before.outbox > 0 && publish {
        while let Some(ack) = store.publish_to(folder, now)? {
            *published += ack.revisions;
        }
    }
    let after = store.census()?;
    if after.outbox > 0 {
        lines.push(format!(
            "{} saves made in Quire are not in the synced folder yet. Run the rollback again with --publish to write them under Quire's writer {writer}. That cannot be undone: rt and every other computer keep published changes.",
            after.outbox
        ));
    }
    let seen = what_rt_sees(folder, database, notebook)?;
    let agrees = seen.pages == after.pages;
    lines.push(format!(
        "A fresh replica reading the synced folder, as rt does, sees {}; Quire shows {}.",
        summary(&seen.pages),
        summary(&after.pages)
    ));
    let unsaved = quire_drafts(&after.drafts);
    if !unsaved.is_empty() {
        lines.push(format!(
            "Unsaved typing in Quire that rt cannot see: {}. Open each in Quire and save it, then run the rollback again.",
            unsaved.join(", ")
        ));
    }
    Ok(RollbackNotebook {
        kind: NotebookKind::Quire,
        source: folder.to_owned(),
        ready: !unreadable && after.outbox == 0 && agrees && unsaved.is_empty(),
        lines,
    })
}

/// The pages a replica that has never seen this notebook reads from the synced folder alone.
fn what_rt_sees(
    folder: &Path,
    database: &Path,
    notebook: &str,
) -> Result<crate::notebook::StoreCensus> {
    let scratch = tempfile::tempdir_in(database.parent().unwrap_or(Path::new(".")))?;
    let fresh = NotebookStore::open(
        &scratch.path().join("replica.sqlite"),
        notebook,
        &format!("verify-{}", uuid::Uuid::new_v4().simple()),
    )?;
    import_log(&fresh, folder)?;
    fresh.census()
}

/// Reports, for every notebook that came from `rt`, what `rt` will see, publishing Quire's
/// unpublished saves to synced folders only when `publish` is set.
pub fn prepare_rollback(quire_data: &Path, publish: bool, now: u64) -> Result<RollbackReport> {
    let record = Record::load(quire_data)?;
    let mut report = RollbackReport::default();
    for migrated in &record.notebooks {
        let notebook = match migrated {
            Migrated::Folder { root, state } => {
                let state = if state.exists() {
                    state.clone()
                } else {
                    state_folder_in(quire_data, root)
                };
                folder(root, &state)
            }
            Migrated::Quire {
                folder,
                database,
                notebook,
                writer,
            } => quire(
                folder,
                database,
                notebook,
                writer,
                publish,
                now,
                &mut report.published,
            ),
        };
        report.notebooks.push(notebook.unwrap_or_else(|error| {
            let (kind, source) = match migrated {
                Migrated::Folder { root, .. } => (NotebookKind::Folder, root.clone()),
                Migrated::Quire { folder, .. } => (NotebookKind::Quire, folder.clone()),
            };
            RollbackNotebook {
                kind,
                source,
                lines: vec![format!("Could not check this notebook: {error:#}")],
                ready: false,
            }
        }));
    }
    Ok(report)
}
