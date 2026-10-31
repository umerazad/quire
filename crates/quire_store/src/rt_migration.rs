//! Moving from the Electron app (`rt`) to Quire without changing anything `rt` keeps.
//!
//! `rt` keeps its machine state in its application data folder (`profiles.sqlite`, a legacy
//! `settings.json`, appearance and AI settings, its device identity, and one local database per
//! Quire notebook) and its notebook state in the notebooks themselves (a folder notebook's
//! `.quire` folder, a Quire notebook's synced change log). Migration reads all of it and writes
//! only to Quire's data folder: every `rt` database is copied, with its write-ahead log, into a
//! scratch folder there and read from the copy, as folder notebooks already do (NR13), so `rt`
//! can be used again at any point with nothing of its own changed.
//!
//! Folder notebooks are read in place, so both apps keep editing the same files. A Quire
//! notebook gets a Quire database built from its published change log under a new writer
//! identity; `rt`'s unpublished saves stop its migration until `rt` publishes them. Each step is
//! recorded in `rt-migration.json` in Quire's data folder, so running the migration again, or
//! after an interruption, finishes what is missing and changes nothing else.

mod backups;
mod folder;
mod inventory;
mod quire_notebook;
mod record;
mod report;
mod rollback;
mod rt_profiles;
#[cfg(test)]
mod tests;

pub use backups::{RestoredBackup, RtBackup, RtBackupKind, list_backups, restore_backup};
pub use inventory::{Disposition, Item, inventory};
pub use record::{Migrated, RECORD_FILE, Record};
pub use report::{Check, NotebookKind, NotebookReport, Report};
pub use rollback::{RollbackNotebook, RollbackReport, prepare_rollback};
pub use rt_profiles::{RtProfile, RtProfiles, read_profiles};

use crate::{
    database,
    profiles::{Notebook, ProfileId, Profiles},
};
use anyhow::{Context as _, Result, bail};
use sqlez::connection::Connection;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The scratch folder for copies of `rt`'s databases, inside Quire's data folder.
const SCRATCH: &str = ".rt-migration";

/// A copy of one of `rt`'s SQLite databases, open for reading; removed when dropped.
pub(crate) struct CopiedDatabase {
    pub connection: Connection,
    _folder: tempfile::TempDir,
}

/// Copies `source` and its write-ahead log into a new folder below `scratch` and opens the copy
/// after an integrity check. `rt`'s files are only read.
pub(crate) fn copy_database(source: &Path, scratch: &Path) -> Result<CopiedDatabase> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("Cannot read rt's database {}", source.display()))?;
    if !metadata.is_file() {
        bail!(
            "rt's database {} is a link or not a file. Replace it with the original file, then run the migration again.",
            source.display()
        );
    }
    fs::create_dir_all(scratch).with_context(|| {
        format!(
            "Cannot create {}. Check write access to Quire's data folder, then run the migration again.",
            scratch.display()
        )
    })?;
    let wal = PathBuf::from(format!("{}-wal", source.display()));
    // The database and its log are copied separately; a copy taken while either changed could
    // pair a checkpointed file with a stale log, so it is taken again.
    let stamp = || {
        [source, wal.as_path()].map(|path| {
            fs::metadata(path)
                .ok()
                .map(|metadata| (metadata.len(), metadata.modified().ok()))
        })
    };
    let mut attempts = 0;
    let (folder, copy) = loop {
        let before = stamp();
        let folder = tempfile::tempdir_in(scratch)
            .context("Cannot make a scratch folder in Quire's data folder; check free space")?;
        let copy = folder.path().join("copy.sqlite");
        fs::copy(source, &copy).with_context(|| {
            format!(
                "Cannot copy rt's database {}; check free space",
                source.display()
            )
        })?;
        match fs::copy(&wal, folder.path().join("copy.sqlite-wal")) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("Cannot copy rt's write-ahead log {}", wal.display())
                });
            }
        }
        if stamp() == before {
            break (folder, copy);
        }
        attempts += 1;
        if attempts == 3 {
            bail!(
                "rt's database {} kept changing while it was copied. Quit rt, then run the migration again.",
                source.display()
            );
        }
    };
    let connection = database::open_unconfigured(&copy)?;
    let check: Option<String> = connection
        .select_row("PRAGMA integrity_check")
        .and_then(|mut check| check())
        .with_context(|| format!("rt's database {} cannot be read", source.display()))?;
    if check.as_deref() != Some("ok") {
        bail!(
            "rt's database {} failed its integrity check, perhaps because rt was writing it. Quit rt, then run the migration again.",
            source.display()
        );
    }
    Ok(CopiedDatabase {
        connection,
        _folder: folder,
    })
}

/// Where Quire keeps the Quire notebook with identity `notebook` that migration builds.
pub fn store_database(quire_data: &Path, notebook: &str) -> PathBuf {
    quire_data
        .join("notebooks")
        .join(notebook)
        .join("local.sqlite")
}

/// The names `rt` may use for the migrated notebook at `root`: `root` itself and every `rt`
/// profile root that resolves to it. `rt` keys renderer state by the root as its profile spells it.
pub fn rt_names_for(rt_data: &Path, quire_data: &Path, root: &Path) -> Vec<PathBuf> {
    let profile_roots = read_profiles(rt_data, &quire_data.join(SCRATCH))
        .map(|profiles| profiles.roots())
        .unwrap_or_default();
    std::iter::once(root.to_owned())
        .chain(profile_roots.into_iter().filter(|candidate| {
            candidate != root && fs::canonicalize(candidate).is_ok_and(|real| real == root)
        }))
        .collect()
}

/// What to migrate and where.
#[derive(Clone, Debug)]
pub struct Options {
    /// `rt`'s application data folder, such as `~/Library/Application Support/Quire`.
    pub rt_data: PathBuf,
    /// Quire's data folder.
    pub quire_data: PathBuf,
    /// Milliseconds since 1970, for the record.
    pub now: u64,
}

/// Migrates every notebook `rt`'s profiles name and the profiles themselves, then verifies the
/// result. Problems with one notebook are reported and do not stop the others.
pub fn migrate(options: &Options) -> Result<Report> {
    let Options {
        rt_data,
        quire_data,
        now,
    } = options;
    if !rt_data.is_dir() {
        bail!(
            "{} is not rt's data folder. Pass the folder that holds profiles.sqlite, usually ~/Library/Application Support/Quire on macOS.",
            rt_data.display()
        );
    }
    // Electron keeps this link while rt runs; a running rt could save between our copies.
    if fs::symlink_metadata(rt_data.join("SingletonLock")).is_ok() {
        bail!(
            "rt appears to be running: {} exists. Quit rt, then run the migration again. If rt is not running, open and quit it once to clear the lock.",
            rt_data.join("SingletonLock").display()
        );
    }
    fs::create_dir_all(quire_data).with_context(|| {
        format!(
            "Cannot create Quire's data folder {}. Check write access, then run the migration again.",
            quire_data.display()
        )
    })?;
    let scratch = quire_data.join(SCRATCH);
    let mut record = Record::load(quire_data)?;
    record.source = Some(rt_data.clone());
    let mut report = Report {
        source: rt_data.clone(),
        ..Report::default()
    };
    let profiles = match read_profiles(rt_data, &scratch) {
        Ok(profiles) => profiles,
        Err(error) => {
            report.problems.push(format!(
                "rt's profiles could not be read: {error:#}. Its notebooks are not migrated; repair or restore profiles.sqlite, then run the migration again."
            ));
            RtProfiles::default()
        }
    };
    report.problems.extend(profiles.problems.iter().cloned());
    report.inventory = inventory(rt_data, &profiles.roots());
    report.inventory.extend(
        profiles
            .profiles
            .iter()
            .filter_map(|profile| profile.backup_folder.as_deref())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .flat_map(inventory::backup_items),
    );
    let mut notebooks: Vec<(ProfileId, Option<Notebook>)> = Vec::new();
    for profile in &profiles.profiles {
        let Some(root) = &profile.root else {
            notebooks.push((profile.id, None));
            continue;
        };
        let migrated = if root.join("quire-notebook.json").is_file() {
            quire_notebook::Source::installed(root, rt_data).and_then(|source| {
                quire_notebook::migrate(&source, quire_data, &scratch, &mut record)
            })
        } else if root.is_dir() {
            folder::adopt(root, quire_data, &mut record)
        } else {
            Err(anyhow::anyhow!(
                "{} is missing. Reconnect the drive or folder, then run the migration again; rt's record of it is unchanged.",
                root.display()
            ))
        };
        match migrated {
            Ok((notebook, notebook_report)) => {
                report.notebooks.push(notebook_report);
                notebooks.push((profile.id, notebook));
            }
            Err(error) => {
                report.problems.push(format!(
                    "The {} notebook at {} was not migrated: {error:#}",
                    profile.id.label(),
                    root.display()
                ));
                notebooks.push((profile.id, None));
            }
        }
    }
    record.save(quire_data)?;
    let (lines, problems) = import_profiles(&profiles, &notebooks, quire_data, &mut record)?;
    report.profiles = lines;
    report.problems.extend(problems);
    record.migrated_at.get_or_insert(*now);
    record.save(quire_data)?;
    let _ = fs::remove_dir(&scratch);
    Ok(report)
}

/// Gives each Quire profile the notebook, backup folder, and Git remote of its `rt` profile.
/// Each choice is made once and recorded, so later runs keep what was chosen in Quire since, and
/// a notebook migrated only on a later run still joins its profile then. A Quire profile that
/// already has a value keeps it, and a backup folder or Git remote joins a profile only when it
/// holds the very notebook they served in `rt`. Returns report lines and problems.
fn import_profiles(
    rt: &RtProfiles,
    notebooks: &[(ProfileId, Option<Notebook>)],
    quire_data: &Path,
    record: &mut Record,
) -> Result<(Vec<String>, Vec<String>)> {
    let mut profiles = Profiles::load(quire_data.join("profiles.json"))?;
    let mut lines = Vec::new();
    let mut problems = Vec::new();
    let mut step = |record: &mut Record, key: String, outcome: Result<String>| match outcome {
        Ok(line) => {
            record.completed.insert(key);
            lines.push(line);
        }
        Err(error) => problems.push(format!(
            "{error:#} Run the migration again after fixing that; the step is retried."
        )),
    };
    if !record.completed.contains("profile:active") {
        let fresh = ProfileId::ALL
            .iter()
            .all(|id| profiles.get(*id).notebook.is_none());
        let outcome = if fresh {
            profiles
                .activate(rt.active)
                .map(|()| format!("Active profile: {}.", rt.active.label()))
        } else {
            Ok("Kept Quire's active profile.".into())
        };
        step(record, "profile:active".into(), outcome);
    }
    for profile in &rt.profiles {
        let label = profile.id.label();
        let id = profile.id;
        let key = |what: &str| format!("profile:{}:{what}", label.to_lowercase());
        let notebook = notebooks
            .iter()
            .find(|(held, _)| *held == id)
            .and_then(|(_, notebook)| notebook.clone());
        if !record.completed.contains(&key("notebook"))
            && let Some(notebook) = &notebook
        {
            let outcome = if profiles.get(id).notebook.is_some() {
                Ok(format!(
                    "{label}: kept Quire's notebook; rt's is {}.",
                    notebook.describe()
                ))
            } else {
                profiles
                    .set_notebook(id, notebook.clone())
                    .map(|()| format!("{label}: notebook {}.", notebook.describe()))
                    .with_context(|| format!("{label}: notebook {} not set.", notebook.describe()))
            };
            step(record, key("notebook"), outcome);
        }
        // Until rt's notebook is migrated nothing is decided, since setting a notebook later
        // clears the Git remote. Afterwards the folder and remote follow only that notebook.
        let held = profiles.get(id).notebook;
        let belongs = match (&profile.root, &notebook) {
            (None, _) => Some(held.is_none()),
            (Some(_), Some(notebook)) => {
                Some(held.as_ref().is_some_and(|held| held.same(notebook)))
            }
            (Some(_), None) => None,
        };
        let Some(belongs) = belongs else { continue };
        if !record.completed.contains(&key("backup"))
            && let Some(folder) = &profile.backup_folder
        {
            let outcome = if !belongs {
                Ok(format!(
                    "{label}: rt's backup folder {} was not set, since Quire's {label} holds another notebook.",
                    folder.display()
                ))
            } else if profiles.get(id).backup_folder.is_some() {
                Ok(format!("{label}: kept Quire's backup folder."))
            } else {
                profiles
                    .set_backup_folder(id, folder)
                    .map(|()| {
                        format!(
                            "{label}: backup folder {}. rt's backups there stay rt's; restore one with --restore-rt-backup.",
                            folder.display()
                        )
                    })
                    .with_context(|| format!("{label}: backup folder {} not set.", folder.display()))
            };
            step(record, key("backup"), outcome);
        }
        if !record.completed.contains(&key("git"))
            && let Some(remote) = &profile.git_remote
        {
            let outcome = if !belongs {
                Ok(format!(
                    "{label}: rt's Git remote {remote} was not set, since Quire's {label} holds another notebook."
                ))
            } else if profiles.get(id).git_remote.is_some() {
                Ok(format!("{label}: kept Quire's Git remote."))
            } else {
                profiles
                    .set_git_remote(id, remote)
                    .map(|_| format!("{label}: Git remote {remote}."))
                    .with_context(|| format!("{label}: Git remote {remote} not set."))
            };
            step(record, key("git"), outcome);
        }
    }
    if lines.is_empty() && problems.is_empty() {
        lines.push(
            "Profiles were imported by an earlier run; Quire's profiles are unchanged.".into(),
        );
    }
    Ok((lines, problems))
}
