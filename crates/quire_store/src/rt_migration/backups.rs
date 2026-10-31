//! Restoring `rt`'s backups (rt `src/desktop/backups.ts` and `engine/backup.ts`) as new
//! notebooks. A backup is verified against its manifest first and only ever read.
//!
//! A folder backup's `notebook/` becomes a new folder, with every draft in its `.quire`
//! databases set to review (no known base), then is adopted like any folder notebook. A Quire
//! notebook backup becomes a new synced folder as `rt` restores it: each device's change files
//! are rewritten under a new notebook identity, so the copy never mixes with the live notebook,
//! and revision IDs, and so history, are unchanged; then it is migrated like any `rt` Quire
//! notebook, with the backup's drafts for review. Either result is a notebook `rt` opens too.

use super::{
    NotebookReport, Record, copy_database, folder,
    quire_notebook::{self, Source},
};
use crate::{
    database,
    profiles::{Notebook, overlaps},
    sync::{
        format::{self, Expected, Kind, sha256},
        json::Json,
    },
};
use anyhow::{Context as _, Result, bail, ensure};
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RtBackupKind {
    Folder,
    Quire,
}

/// One completed `rt` backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RtBackup {
    pub path: PathBuf,
    pub profile: String,
    pub created_at: u64,
    pub kind: RtBackupKind,
    pub source_name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    path: String,
    hash: String,
    size: u64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Manifest {
    version: u32,
    kind: Option<String>,
    profile: String,
    created_at: f64,
    files: Vec<Entry>,
    source_name: Option<String>,
}

fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// The manifest, checked by `rt`'s rules (`Backups.manifest`), except that any profile is
/// accepted.
fn manifest(backup: &Path) -> Result<(Manifest, RtBackupKind)> {
    let file = backup.join("manifest.json");
    let manifest: Manifest = serde_json::from_slice(&fs::read(&file).with_context(|| {
        format!(
            "Cannot read the backup manifest {}. Choose another completed backup",
            file.display()
        )
    })?)
    .with_context(|| {
        format!(
            "The backup manifest {} is damaged. Choose another completed backup",
            file.display()
        )
    })?;
    let kind = match manifest.kind.as_deref() {
        None => RtBackupKind::Folder,
        Some("quire") => RtBackupKind::Quire,
        Some(other) => bail!(
            "The backup at {} is of kind {other:?}, which this Quire does not know. Choose another backup",
            backup.display()
        ),
    };
    let database = match kind {
        RtBackupKind::Folder => "recovery.sqlite",
        RtBackupKind::Quire => "local.sqlite",
    };
    let allowed = |path: &str| match kind {
        RtBackupKind::Quire => path == "local.sqlite" || path.starts_with("notebook/"),
        RtBackupKind::Folder => {
            path == "recovery.sqlite"
                || ["notebook/", "transactions/", "recovery/"]
                    .iter()
                    .any(|prefix| path.starts_with(prefix))
        }
    };
    let mut seen = std::collections::BTreeSet::new();
    let valid = manifest.version == 1
        && manifest.created_at.is_finite()
        && manifest.files.iter().any(|entry| entry.path == database)
        && manifest.files.iter().all(|entry| {
            valid_path(&entry.path)
                && allowed(&entry.path)
                && !entry.path.split('/').any(|part| part == ".git")
                && entry.hash.len() == 64
                && entry.hash.bytes().all(|b| b.is_ascii_hexdigit())
                && seen.insert(entry.path.clone())
        });
    ensure!(
        valid,
        "The backup manifest in {} is invalid. Choose another completed backup",
        backup.display()
    );
    Ok((manifest, kind))
}

fn walk(root: &Path, relative: &str, found: &mut Vec<String>) -> Result<()> {
    for entry in fs::read_dir(root.join(relative))? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if relative.is_empty() {
            name
        } else {
            format!("{relative}/{name}")
        };
        let kind = entry.file_type()?;
        ensure!(
            !kind.is_symlink(),
            "The backup holds a link at {child}. Choose another backup"
        );
        if kind.is_dir() {
            walk(root, &child, found)?;
        } else {
            found.push(child);
        }
    }
    Ok(())
}

/// Checks every file against the manifest, reading only. Fails on a missing, extra, or changed
/// file, as `rt`'s restore does.
fn verify(backup: &Path, manifest: &Manifest) -> Result<()> {
    let mut found = Vec::new();
    walk(backup, "", &mut found)?;
    found.retain(|path| path != "manifest.json");
    let expected: BTreeMap<&str, &Entry> = manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    ensure!(
        found.len() == expected.len(),
        "Backup verification failed: files are missing or extra. Choose another completed backup"
    );
    for path in found {
        let entry = expected.get(path.as_str()).with_context(|| {
            format!("Backup verification failed: {path} is not in the manifest. Choose another completed backup")
        })?;
        let bytes = fs::read(backup.join(&path))?;
        ensure!(
            bytes.len() as u64 == entry.size && sha256(&bytes) == entry.hash,
            "Backup verification failed: {path} changed. Choose another completed backup"
        );
    }
    Ok(())
}

/// The completed `rt` backups below the backup folder `destination`, newest first. Folders
/// without a valid manifest are left out, as `rt` lists them.
pub fn list_backups(destination: &Path) -> Vec<RtBackup> {
    let mut backups = Vec::new();
    let quire = destination.join("Quire");
    let children = |path: &Path| -> Vec<PathBuf> {
        fs::read_dir(path)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
            .map(|entry| entry.path())
            .collect()
    };
    for profile in children(&quire) {
        for notebook in children(&profile) {
            for backup in children(&notebook) {
                if let Ok((manifest, kind)) = manifest(&backup) {
                    backups.push(RtBackup {
                        path: backup,
                        profile: manifest.profile,
                        created_at: manifest.created_at as u64,
                        kind,
                        source_name: manifest.source_name,
                    });
                }
            }
        }
    }
    backups.sort_by_key(|backup| std::cmp::Reverse(backup.created_at));
    backups
}

/// A restored backup: the new notebook and how it compares with the backup.
#[derive(Debug)]
pub struct RestoredBackup {
    pub root: PathBuf,
    pub notebook: Notebook,
    pub report: NotebookReport,
}

/// Copies every file of `from` into `to`, keeping relative paths.
fn copy_tree(from: &Path, to: &Path, skip: impl Fn(&str) -> bool) -> Result<()> {
    let mut found = Vec::new();
    walk(from, "", &mut found)?;
    for path in found.into_iter().filter(|path| !skip(path)) {
        let target = to.join(&path);
        fs::create_dir_all(target.parent().unwrap_or(to))?;
        fs::copy(from.join(&path), &target)?;
        fs::File::open(&target)?.sync_all()?;
    }
    Ok(())
}

/// Sets every draft in the copied `.quire` databases and JSON records to review: `rt` and Quire
/// then offer it for comparison and never save it on their own.
fn drafts_for_review(root: &Path) -> Result<()> {
    for device in fs::read_dir(root.join(".quire/devices"))
        .into_iter()
        .flatten()
    {
        let device = device?.path();
        let database = device.join("durability.sqlite");
        if database.is_file() {
            let connection = database::open_unconfigured(&database)?;
            let has_pending: Option<i64> = connection
                .select_row("SELECT 1 FROM sqlite_master WHERE type='table' AND name='pending'")?(
            )?;
            if has_pending.is_some() {
                connection.exec("UPDATE pending SET expected_revision=NULL")?()?;
            }
        }
        for record in fs::read_dir(device.join("recovery")).into_iter().flatten() {
            let record = record?.path();
            if record
                .extension()
                .is_some_and(|extension| extension == "json")
            {
                let mut value: serde_json::Value = serde_json::from_slice(&fs::read(&record)?)?;
                if let Some(object) = value.as_object_mut() {
                    object.insert("expectedRevision".into(), serde_json::Value::Null);
                }
                fs::write(&record, serde_json::to_vec(&value)?)?;
            }
        }
    }
    Ok(())
}

/// Rewrites each device's change files from `from` into `to` under `notebook`, chaining each
/// file to the one before, as `restoreQuireBackup` does.
fn rewrite_log(from: &Path, to: &Path, old: &str, notebook: &str) -> Result<()> {
    for device in fs::read_dir(from.join("devices")).into_iter().flatten() {
        let device = device?;
        if !device.file_type()?.is_dir() {
            continue;
        }
        let name = device.file_name().to_string_lossy().into_owned();
        let mut files: Vec<(u32, String)> = fs::read_dir(device.path().join("log"))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let file = entry.file_name().to_string_lossy().into_owned();
                let seq = format::parse_name(&file, Kind::Batch)?.seq;
                Some((seq, file))
            })
            .collect();
        files.sort();
        let log = to.join("devices").join(&name).join("log");
        fs::create_dir_all(&log)?;
        let mut previous: Option<String> = None;
        for (index, (seq, file)) in files.into_iter().enumerate() {
            ensure!(
                seq as usize == index + 1,
                "The backup is missing change {} from computer {name}. Choose another backup",
                index + 1
            );
            let data = fs::read(device.path().join("log").join(&file))?;
            let decoded = format::decode(
                &file,
                &data,
                Kind::Batch,
                Expected {
                    notebook: old,
                    device: &name,
                },
            )
            .map_err(|error| {
                anyhow::anyhow!(
                    "A change file in the backup is damaged ({}). Choose another backup",
                    error.reason()
                )
            })?;
            let mut value: Json = decoded.value;
            value.set_string("notebook", notebook);
            match &previous {
                Some(hash) => value.set_string("previous", hash),
                None => value.set_null("previous"),
            };
            let encoded = format::encode(value, Kind::Batch).map_err(|error| {
                anyhow::anyhow!("Cannot rewrite a change file: {}", error.reason())
            })?;
            fs::write(log.join(&encoded.name), &encoded.data)?;
            previous = Some(encoded.hash);
        }
    }
    Ok(())
}

/// Restores the `rt` backup at `backup` as a new notebook in a new folder below `parent`, and
/// brings it into Quire. The backup is only read. The copy is built in a hidden folder that a
/// failure removes; once complete it is renamed into place, so a later failure to bring it into
/// Quire leaves a whole restored notebook that the migration can adopt again.
pub fn restore_backup(
    backup: &Path,
    parent: &Path,
    quire_data: &Path,
    now: u64,
) -> Result<RestoredBackup> {
    let (manifest, kind) = manifest(backup)?;
    verify(backup, &manifest)?;
    let parent = fs::canonicalize(parent).with_context(|| {
        format!(
            "Cannot use {} for the restored notebook. Choose an existing folder",
            parent.display()
        )
    })?;
    let backup = fs::canonicalize(backup)?;
    ensure!(
        !overlaps(&parent, &backup),
        "Choose a folder outside the backup for the restored notebook"
    );
    let scratch = quire_data.join(super::SCRATCH);
    let database = backup.join(match kind {
        RtBackupKind::Folder => "recovery.sqlite",
        RtBackupKind::Quire => "local.sqlite",
    });
    drop(copy_database(&database, &scratch)?);
    let id = &uuid::Uuid::new_v4().simple().to_string()[..8];
    let name = format!("Restored {}-{id}", crate::backup::stamp(now));
    let staging = parent.join(format!(".restoring-{id}"));
    let root = parent.join(&name);
    let result = (|| -> Result<RestoredBackup> {
        fs::create_dir(&staging)?;
        let source = backup.join("notebook");
        match kind {
            RtBackupKind::Folder => {
                copy_tree(&source, &staging, |_| false)?;
                drafts_for_review(&staging)?;
            }
            RtBackupKind::Quire => {
                let old = quire_notebook::notebook_identity(&source)?;
                let notebook = uuid::Uuid::new_v4().to_string();
                rewrite_log(&source, &staging, &old, &notebook)?;
                copy_tree(&source, &staging, |path| {
                    path == "quire-notebook.json"
                        || path.starts_with("devices/")
                        || path.split('/').any(|part| part.starts_with('.'))
                })?;
                fs::write(
                    staging.join("quire-notebook.json"),
                    format!(
                        "{{\n  \"format\": 1,\n  \"id\": \"{notebook}\",\n  \"created\": {now}\n}}\n"
                    ),
                )?;
            }
        }
        crate::durable_files::sync_directory(&staging)?;
        fs::rename(&staging, &root)?;
        crate::durable_files::sync_directory(&parent)?;
        let mut record = Record::load(quire_data)?;
        let (notebook, report) = match kind {
            RtBackupKind::Folder => folder::adopt(&root, quire_data, &mut record)?,
            RtBackupKind::Quire => quire_notebook::migrate(
                &Source {
                    folder: &root,
                    local: Some(database.clone()),
                    rt_device: String::new(),
                    review_only: true,
                },
                quire_data,
                &scratch,
                &mut record,
            )?,
        };
        record.save(quire_data)?;
        Ok(RestoredBackup {
            notebook: notebook.context("The restored notebook could not be opened")?,
            root: root.clone(),
            report,
        })
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    let _ = fs::remove_dir(&scratch);
    result
}
