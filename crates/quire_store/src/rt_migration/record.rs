//! `rt-migration.json` in Quire's data folder: which notebooks came from `rt`, the writer
//! identity each Quire notebook got, and the steps already done, so a second run, or one after
//! an interruption, repeats nothing.

use crate::durable_files::sync_directory;
use anyhow::{Context as _, Result};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::Write as _,
    path::{Path, PathBuf},
};

pub const RECORD_FILE: &str = "rt-migration.json";

/// A notebook that came from `rt`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Migrated {
    /// A folder notebook both apps edit in place; Quire's state for it is in `state`.
    Folder { root: PathBuf, state: PathBuf },
    /// A Quire notebook: `rt`'s synced change log in `folder`, Quire's copy in `database`,
    /// written by `writer`, a writer identity `rt` never used.
    Quire {
        folder: PathBuf,
        database: PathBuf,
        notebook: String,
        writer: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Record {
    pub source: Option<PathBuf>,
    pub migrated_at: Option<u64>,
    pub notebooks: Vec<Migrated>,
    /// Steps done once: profile choices and imported drafts, by key.
    pub completed: BTreeSet<String>,
}

impl Record {
    /// The record in `quire_data`, or an empty one before the first migration. A damaged record
    /// is an error, so it is never overwritten.
    pub fn load(quire_data: &Path) -> Result<Self> {
        let file = quire_data.join(RECORD_FILE);
        match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "Cannot read the migration record {}. Move it aside to migrate from the start; Quire's notebooks are unaffected.",
                    file.display()
                )
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| {
                format!(
                    "Cannot read the migration record {}. Check access to Quire's data folder, then run the migration again.",
                    file.display()
                )
            }),
        }
    }

    /// Replaces the record atomically and durably.
    pub fn save(&self, quire_data: &Path) -> Result<()> {
        let failed = || {
            format!(
                "Cannot save the migration record in {}. Check write access and free space, then run the migration again.",
                quire_data.display()
            )
        };
        fs::create_dir_all(quire_data).with_context(failed)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".rt-migration-")
            .tempfile_in(quire_data)
            .with_context(failed)?;
        temporary
            .write_all(&serde_json::to_vec_pretty(self)?)
            .and_then(|()| temporary.as_file().sync_all())
            .with_context(failed)?;
        temporary
            .persist(quire_data.join(RECORD_FILE))
            .map_err(|error| error.error)
            .with_context(failed)?;
        sync_directory(quire_data).with_context(failed)
    }

    /// The Quire notebook migrated from the synced folder `folder`, if any.
    pub fn quire(&self, folder: &Path) -> Option<&Migrated> {
        self.notebooks.iter().find(
            |migrated| matches!(migrated, Migrated::Quire { folder: known, .. } if known == folder),
        )
    }

    pub fn remember(&mut self, migrated: Migrated) {
        if !self.notebooks.contains(&migrated) {
            self.notebooks.push(migrated);
        }
    }
}
