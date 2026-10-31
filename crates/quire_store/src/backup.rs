//! Verified backups of a notebook while it is open, and restores into a new notebook.
//!
//! A backup is a folder `<destination>/Quire Native/<source key>/<id>/` holding consistent copies
//! of the notebook's databases (made with `VACUUM INTO`, never a raw copy of a live file), its
//! attachments or files, and `manifest.json` with each file's SHA-256 and size and each database's
//! row counts. It is written under a hidden `.partial-<id>` name, verified from the drive, and
//! only then renamed into place, so a listed backup is always complete. Older backups are never
//! changed or removed.
//!
//! A Quire notebook backup holds `local.sqlite` and `attachments/<name>` for every attachment its
//! catalog lists. A folder notebook backup holds `notebook/` (every file but `.git`, with the
//! reference app's `.quire` databases copied consistently) and `state/` (Quire's recovery,
//! history, journals, and archive from application data). Derived files such as the query index
//! and the window's tabs are left out and rebuilt.
//!
//! These folders are deliberately not the reference app's backup format: its Quire notebook
//! backups hold a synced change-log folder and its own database schema, which this store does not
//! write. Each app ignores the other's backups (the reference app lists only `Quire/<profile>`).

mod database;
mod files;
mod restore;
#[cfg(test)]
mod tests;

pub use database::Counts;
pub use restore::{Location, RestoreRequest, RestoreStep, Restored, restore};

use crate::{
    notebook::{APPLICATION_ID, Draft, SCHEMA_VERSION, read_revision, unpack},
    sync::format::{attachment_hash, sha256},
};
use anyhow::{Context as _, Result, bail, ensure};
use files::{below, copy_hashed, create_dir_durable, discard, hash_file, sync, walk};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

/// The folder inside a backup destination that holds Quire Native backups.
pub const BACKUPS_FOLDER: &str = "Quire Native";
const MANIFEST: &str = "manifest.json";
const FORMAT: &str = "quire-native-backup";
const VERSION: u32 = 1;
const STORE_DATABASE: &str = "local.sqlite";
const STATE_DATABASE: &str = "state/folder.sqlite";
const MAX_MANIFEST_BYTES: u64 = 256 * 1024 * 1024;

/// The notebook a backup copies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// A Quire notebook: its database and the attachment cache beside it.
    Store { database: PathBuf },
    /// A Markdown folder notebook and Quire's state folder for it in application data.
    Folder { root: PathBuf, state: PathBuf },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Store,
    Folder,
}

impl Source {
    pub fn kind(&self) -> Kind {
        match self {
            Self::Store { .. } => Kind::Store,
            Self::Folder { .. } => Kind::Folder,
        }
    }

    /// The folders that hold live notebook data, which a backup destination must stay outside.
    fn live(&self) -> Vec<PathBuf> {
        match self {
            Self::Store { database } => database
                .parent()
                .map(Path::to_path_buf)
                .into_iter()
                .collect(),
            Self::Folder { root, state } => vec![root.clone(), state.clone()],
        }
    }

    /// Identifies this notebook's backups in a destination, from its location on this computer.
    fn key(&self) -> Result<String> {
        let (kind, path) = match self {
            Self::Store { database } => ("store", database),
            Self::Folder { root, .. } => ("folder", root),
        };
        let path = fs::canonicalize(path).with_context(|| {
            format!(
                "Cannot find the notebook at {}. Reopen the notebook and retry.",
                path.display()
            )
        })?;
        Ok(sha256(format!("{kind}:{}", path.to_string_lossy())))
    }

    /// A name for people choosing among backups.
    fn name(&self) -> String {
        let path = match self {
            Self::Store { database } => database.parent().unwrap_or(database),
            Self::Folder { root, .. } => root,
        };
        path.file_name().map_or_else(
            || path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        )
    }

    /// Where this notebook's backups live inside `destination`.
    pub fn backups_in(&self, destination: &Path) -> Result<PathBuf> {
        Ok(destination.join(BACKUPS_FOLDER).join(self.key()?))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub path: String,
    pub hash: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub format: String,
    pub version: u32,
    pub kind: Kind,
    /// Milliseconds since the Unix epoch when the backup began.
    pub created: u64,
    /// The notebook's folder name when it was backed up.
    pub source: String,
    /// A Quire notebook's identity; None for a folder notebook.
    pub notebook: Option<String>,
    pub files: Vec<Entry>,
    /// Row counts of each database file, by its path in `files`.
    pub databases: BTreeMap<String, Counts>,
}

/// A completed backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BackupInfo {
    pub id: String,
    pub path: PathBuf,
    pub created: u64,
    pub source: String,
    pub kind: Kind,
}

impl BackupInfo {
    fn new(path: PathBuf, manifest: &Manifest) -> Self {
        Self {
            id: path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default(),
            path,
            created: manifest.created,
            source: manifest.source.clone(),
            kind: manifest.kind,
        }
    }
}

/// `created` as `2026-10-31T14-03-05-123Z`, sortable and safe in a file name.
pub(crate) fn stamp(created: u64) -> String {
    let days = (created / 86_400_000) as i64;
    let rest = created % 86_400_000;
    let (hour, minute, second, millis) = (
        rest / 3_600_000,
        rest / 60_000 % 60,
        rest / 1000 % 60,
        rest % 1000,
    );
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}-{minute:02}-{second:02}-{millis:03}Z")
}

pub(crate) fn unique(created: u64) -> String {
    format!(
        "{}-{}",
        stamp(created),
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

fn real(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Refuses a backup or restore folder that is inside live notebook data or holds it.
fn outside_live(source: &Source, folder: &Path, what: &str) -> Result<()> {
    let folder = real(folder);
    for live in source.live() {
        ensure!(
            !overlaps(&folder, &real(&live)),
            "The {what} {} overlaps the notebook's own data at {}. Choose a separate folder, such as one on another drive.",
            folder.display(),
            live.display()
        );
    }
    Ok(())
}

/// Backs up `source` into `destination`, returning the completed backup. On any failure the
/// unfinished copy is removed, earlier backups are untouched, and the error says what to do.
pub fn create(source: &Source, destination: &Path, created: u64) -> Result<BackupInfo> {
    let destination = fs::canonicalize(destination).with_context(|| {
        format!(
            "Cannot open the backup folder {}. Reconnect its drive or choose another backup folder.",
            destination.display()
        )
    })?;
    ensure!(
        destination.is_dir(),
        "The backup destination {} is not a folder. Choose a folder for backups.",
        destination.display()
    );
    outside_live(source, &destination, "backup folder")?;
    let parent = source.backups_in(&destination)?;
    create_dir_durable(&parent)?;
    let id = unique(created);
    let partial = parent.join(format!(".partial-{id}"));
    let result = (|| {
        fs::create_dir(&partial).with_context(|| {
            format!(
                "Cannot create a backup in {}. Check write access and free space there, then retry.",
                parent.display()
            )
        })?;
        let (notebook, databases) = match source {
            Source::Store { database } => stage_store(database, &partial)?,
            Source::Folder { root, state } => (None, stage_folder(root, state, &partial)?),
        };
        let files = walk(&partial, &|_| false)?
            .into_iter()
            .map(|path| {
                let (hash, size) = hash_file(&below(&partial, &path))?;
                Ok(Entry { path, hash, size })
            })
            .collect::<Result<Vec<_>>>()?;
        let manifest = Manifest {
            format: FORMAT.into(),
            version: VERSION,
            kind: source.kind(),
            created,
            source: source.name(),
            notebook,
            files,
            databases,
        };
        write_manifest(&partial, &manifest)?;
        files::sync_tree(&partial)?;
        verify(&partial)?;
        let complete = parent.join(&id);
        fs::rename(&partial, &complete).with_context(|| {
            format!(
                "Cannot complete the backup in {}. Check write access there and retry.",
                parent.display()
            )
        })?;
        sync(&parent)?;
        Ok(BackupInfo::new(complete, &manifest))
    })();
    result.map_err(|error: anyhow::Error| match discard(&partial) {
        Some(note) => error.context(format!("The backup did not complete.{note}")),
        None => error,
    })
}

fn write_manifest(folder: &Path, manifest: &Manifest) -> Result<()> {
    use std::io::Write as _;
    let path = folder.join(MANIFEST);
    let mut file = fs::File::options()
        .write(true)
        .create_new(true)
        .open(&path)
        .with_context(|| {
            format!(
                "Cannot write {}. Check free space and retry.",
                path.display()
            )
        })?;
    file.write_all(&serde_json::to_vec_pretty(manifest)?)?;
    file.sync_all()
        .with_context(|| format!("Cannot flush {} to the drive. Retry.", path.display()))?;
    Ok(())
}

/// Copies a Quire notebook: a snapshot of its database, then every attachment that snapshot's
/// catalog lists, each checked against the catalog's hash. Attachments are written before their
/// catalog rows commit and are immutable, so every listed attachment is on disk.
fn stage_store(
    database: &Path,
    partial: &Path,
) -> Result<(Option<String>, BTreeMap<String, Counts>)> {
    let copy = partial.join(STORE_DATABASE);
    let counts = database::snapshot(database, &copy)?;
    let connection = database::read_only(&copy)?;
    let notebook = connection
        .select_row::<String>(&format!("SELECT notebook FROM {}.meta", database::ATTACHED))?(
    )?
    .context("The notebook database has no identity. Reopen the notebook and retry.")?;
    let attachments = connection.select::<(String, String, i64)>(&format!(
        "SELECT name,hash,size FROM {}.attachments ORDER BY name",
        database::ATTACHED
    ))?()?;
    let cache = database
        .parent()
        .context("The notebook database has no folder.")?
        .join("attachments");
    for (name, hash, size) in attachments {
        ensure!(
            attachment_hash(&name).is_some(),
            "The attachment catalog lists an invalid name {name:?}. Preserve the notebook and contact support."
        );
        let copied = copy_hashed(&cache.join(&name), &partial.join("attachments").join(&name))
            .with_context(|| format!("Cannot back up attachment {name}. Check that the notebook's attachments folder is readable, then retry."))?;
        ensure!(
            copied == (hash, u64::try_from(size)?),
            "Attachment {name} differs from its catalog entry, so the backup stopped. Preserve the notebook and restore the attachment's original bytes, then retry."
        );
    }
    Ok((
        Some(notebook),
        BTreeMap::from([(STORE_DATABASE.to_owned(), counts)]),
    ))
}

fn named(relative: &str) -> &str {
    relative.rsplit('/').next().unwrap_or(relative)
}

fn journal_file(name: &str) -> bool {
    ["-wal", "-shm", "-journal"]
        .iter()
        .any(|suffix| name.ends_with(suffix))
}

fn in_reference_data(relative: &str) -> bool {
    relative.starts_with(".quire/")
}

/// What a folder notebook backup leaves out of the notebook: Git's data, and the write-ahead
/// files of the reference app's databases, whose committed contents the snapshots include.
fn skip_notebook(relative: &str) -> bool {
    named(relative) == ".git" || (in_reference_data(relative) && journal_file(named(relative)))
}

/// What a folder notebook backup leaves out of Quire's state folder: temporary files, the query
/// index, the window's tabs, and write-ahead files.
fn skip_state(relative: &str) -> bool {
    let name = named(relative);
    name.starts_with('.')
        || name.starts_with("query-index.sqlite")
        || name.starts_with("quire-workspace.json")
        || journal_file(name)
}

fn is_database(name: &str) -> bool {
    name.ends_with(".sqlite")
}

/// Files that must stay unchanged while they are copied, with their hashes.
type Inventory = BTreeMap<String, (String, u64)>;

fn inventory(root: &Path, files: &[String]) -> Result<Inventory> {
    files
        .iter()
        .map(|path| Ok((path.clone(), hash_file(&below(root, path))?)))
        .collect()
}

/// Copies a folder notebook and its state. Databases are snapshots; every other file is copied
/// and the notebook is listed and hashed again afterwards, so an edit made meanwhile, here or in
/// another application, fails the backup instead of mixing two versions.
fn stage_folder(root: &Path, state: &Path, partial: &Path) -> Result<BTreeMap<String, Counts>> {
    let mut databases = BTreeMap::new();
    let mut copy = |from: &Path,
                    prefix: &str,
                    skip: &dyn Fn(&str) -> bool,
                    snapshot: &dyn Fn(&str) -> bool|
     -> Result<(Vec<String>, Inventory)> {
        let listed = walk(from, skip)?;
        let mut copied = Inventory::new();
        for path in &listed {
            let target = below(partial, &format!("{prefix}/{path}"));
            if snapshot(path) {
                if let Some(parent) = target.parent() {
                    create_dir_durable(parent)?;
                }
                databases.insert(
                    format!("{prefix}/{path}"),
                    database::snapshot(&below(from, path), &target)?,
                );
            } else {
                copied.insert(path.clone(), copy_hashed(&below(from, path), &target)?);
            }
        }
        Ok((listed, copied))
    };
    let (notes, notes_copied) = copy(root, "notebook", &skip_notebook, &|path| {
        in_reference_data(path) && is_database(named(path))
    })?;
    let (states, states_copied) = copy(state, "state", &skip_state, &|path| {
        is_database(named(path))
    })?;
    ensure!(
        databases.contains_key(STATE_DATABASE),
        "Quire's state folder {} has no folder.sqlite. Open the notebook in Quire once, then retry.",
        state.display()
    );
    let unchanged = |from: &Path,
                     listed: &[String],
                     copied: &Inventory,
                     skip: &dyn Fn(&str) -> bool|
     -> Result<bool> {
        let now = walk(from, skip)?;
        Ok(now == listed
            && inventory(from, &copied.keys().cloned().collect::<Vec<_>>())? == *copied)
    };
    ensure!(
        unchanged(root, &notes, &notes_copied, &skip_notebook)?
            && unchanged(state, &states, &states_copied, &skip_state)?,
        "The notebook changed while it was being backed up. Wait for saves and other applications to finish, then back up again."
    );
    Ok(databases)
}

/// Lists this notebook's completed backups in `destination`, newest first. Unfinished copies and
/// folders without a readable manifest are left out; [`verify`] checks a backup fully.
pub fn list(source: &Source, destination: &Path) -> Result<Vec<BackupInfo>> {
    let parent = source.backups_in(destination)?;
    let entries = match fs::read_dir(&parent) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Cannot list backups in {}. Reconnect the backup drive and retry.",
                    parent.display()
                )
            });
        }
    };
    let mut backups: Vec<BackupInfo> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let name = entry.file_name();
            if name.to_string_lossy().starts_with('.') || !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let manifest = read_manifest(&entry.path()).ok()?;
            (manifest.kind == source.kind()).then(|| BackupInfo::new(entry.path(), &manifest))
        })
        .collect();
    backups.sort_by(|a, b| b.created.cmp(&a.created).then_with(|| b.id.cmp(&a.id)));
    Ok(backups)
}

fn read_manifest(folder: &Path) -> Result<Manifest> {
    let path = folder.join(MANIFEST);
    let metadata = fs::symlink_metadata(&path).with_context(|| {
        format!(
            "{} has no manifest, so it is not a completed backup. Choose another backup.",
            folder.display()
        )
    })?;
    ensure!(
        metadata.is_file() && metadata.len() <= MAX_MANIFEST_BYTES,
        "The backup manifest in {} is not an ordinary file. Choose another backup.",
        folder.display()
    );
    let manifest: Manifest = serde_json::from_slice(&fs::read(&path)?).with_context(|| {
        format!(
            "The backup manifest in {} cannot be read. Choose another completed backup.",
            folder.display()
        )
    })?;
    validate(&manifest).with_context(|| {
        format!(
            "The backup manifest in {} is invalid. Choose another completed backup.",
            folder.display()
        )
    })?;
    Ok(manifest)
}

fn validate(manifest: &Manifest) -> Result<()> {
    ensure!(
        manifest.format == FORMAT && manifest.version == VERSION,
        "This backup was made by another version of Quire. Update Quire before restoring it."
    );
    let mut paths = BTreeSet::new();
    for entry in &manifest.files {
        let allowed = match manifest.kind {
            Kind::Store => {
                entry.path == STORE_DATABASE
                    || entry
                        .path
                        .strip_prefix("attachments/")
                        .is_some_and(|name| attachment_hash(name).is_some())
            }
            Kind::Folder => entry.path.starts_with("notebook/") || entry.path.starts_with("state/"),
        };
        ensure!(
            files::valid_path(&entry.path)
                && allowed
                && entry.hash.len() == 64
                && entry
                    .hash
                    .bytes()
                    .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
                && paths.insert(entry.path.as_str()),
            "The manifest lists an invalid file {:?}.",
            entry.path
        );
    }
    let required = match manifest.kind {
        Kind::Store => STORE_DATABASE,
        Kind::Folder => STATE_DATABASE,
    };
    ensure!(
        paths.contains(required),
        "The manifest does not list {required}."
    );
    ensure!(
        manifest.databases.contains_key(required)
            && manifest
                .databases
                .keys()
                .all(|path| paths.contains(path.as_str())),
        "The manifest's database counts do not match its files."
    );
    ensure!(
        (manifest.kind == Kind::Store) == manifest.notebook.is_some(),
        "The manifest's notebook identity does not match its kind."
    );
    Ok(())
}

/// Checks a backup completely without changing it: the manifest, every file's presence, size, and
/// hash, no unexpected files, each database's integrity and row counts, and for a Quire notebook
/// the identity, every revision and draft, and every attachment the catalog lists.
pub fn verify(backup: &Path) -> Result<Manifest> {
    let manifest = read_manifest(backup)?;
    let actual: BTreeSet<String> = walk(backup, &|path| path == MANIFEST)?
        .into_iter()
        .collect();
    let expected: BTreeMap<&str, &Entry> = manifest
        .files
        .iter()
        .map(|entry| (entry.path.as_str(), entry))
        .collect();
    if let Some(missing) = expected.keys().find(|path| !actual.contains(**path)) {
        bail!(
            "Backup verification failed: {missing} is missing from {}. Choose another backup.",
            backup.display()
        );
    }
    if let Some(extra) = actual
        .iter()
        .find(|path| !expected.contains_key(path.as_str()))
    {
        bail!(
            "Backup verification failed: {} holds an unexpected file {extra}. Choose another backup.",
            backup.display()
        );
    }
    for (path, entry) in &expected {
        let (hash, size) = hash_file(&below(backup, path))?;
        ensure!(
            hash == entry.hash && size == entry.size,
            "Backup verification failed: {path} in {} changed since the backup was made. Choose another backup.",
            backup.display()
        );
    }
    for (path, counts) in &manifest.databases {
        let file = below(backup, path);
        let connection = database::read_only(&file)?;
        let found = database::check_attached(&connection, &file)?;
        ensure!(
            found == *counts,
            "Backup verification failed: the rows in {path} differ from the manifest. Choose another backup."
        );
        match (manifest.kind, path.as_str()) {
            (Kind::Store, STORE_DATABASE) => check_store(&connection, &manifest)?,
            (Kind::Folder, STATE_DATABASE) => check_state(&connection)?,
            _ => {}
        }
    }
    Ok(manifest)
}

fn check_store(connection: &sqlez::connection::Connection, manifest: &Manifest) -> Result<()> {
    let schema = database::ATTACHED;
    let application = connection.select_row::<i32>(&format!("PRAGMA {schema}.application_id"))?()?;
    let version =
        connection.select_row::<i32>(&format!("PRAGMA {schema}.user_version"))?()?.unwrap_or(0);
    ensure!(
        application == Some(APPLICATION_ID) && (1..=SCHEMA_VERSION).contains(&version),
        "Backup verification failed: local.sqlite is not a Quire notebook this version can read. Update Quire or choose another backup."
    );
    let notebook =
        connection.select_row::<String>(&format!("SELECT notebook FROM {schema}.meta"))?()?;
    ensure!(
        notebook == manifest.notebook,
        "Backup verification failed: local.sqlite belongs to another notebook than its manifest names. Choose another backup."
    );
    for (what, sql) in [
        (
            "page heads",
            format!(
                "SELECT COUNT(*) FROM {schema}.heads h LEFT JOIN {schema}.revisions r ON r.id=h.revision WHERE r.id IS NULL"
            ),
        ),
        (
            "revision parents",
            format!(
                "SELECT COUNT(*) FROM {schema}.parents p LEFT JOIN {schema}.revisions r ON r.id=p.parent WHERE r.id IS NULL"
            ),
        ),
        (
            "unpublished revisions",
            format!(
                "SELECT COUNT(*) FROM {schema}.outbox o LEFT JOIN {schema}.revisions r ON r.id=o.revision WHERE r.id IS NULL"
            ),
        ),
    ] {
        ensure!(
            connection.select_row::<i64>(&sql)?()? == Some(0),
            "Backup verification failed: {what} in local.sqlite name missing revisions. Choose another backup."
        );
    }
    for id in connection.select::<String>(&format!("SELECT id FROM {schema}.revisions"))?()? {
        read_revision(connection, schema, &id)
            .and_then(|record| record.context("missing"))
            .with_context(|| format!("Backup verification failed: revision {id} in local.sqlite is damaged. Choose another backup."))?;
    }
    for (page, hash, body) in connection.select::<(String, String, Vec<u8>)>(&format!(
        "SELECT page,body_hash,body FROM {schema}.drafts"
    ))?()? {
        unpack(&hash, &body)
            .and_then(|text| Ok(serde_json::from_slice::<Draft>(&text)?))
            .with_context(|| format!("Backup verification failed: the recovery draft for {page} is damaged. Choose another backup."))?;
    }
    let listed: BTreeMap<&str, &Entry> = manifest
        .files
        .iter()
        .filter_map(|entry| Some((entry.path.strip_prefix("attachments/")?, entry)))
        .collect();
    let catalog = connection.select::<(String, String, i64)>(&format!(
        "SELECT name,hash,size FROM {schema}.attachments"
    ))?()?;
    ensure!(
        catalog.len() == listed.len(),
        "Backup verification failed: the backup holds {} attachments but the notebook lists {}. Choose another backup.",
        listed.len(),
        catalog.len()
    );
    for (name, hash, size) in catalog {
        let entry = listed.get(name.as_str()).with_context(|| {
            format!(
                "Backup verification failed: attachment {name} is missing. Choose another backup."
            )
        })?;
        ensure!(
            entry.hash == hash && i64::try_from(entry.size)? == size,
            "Backup verification failed: attachment {name} differs from the notebook's catalog. Choose another backup."
        );
    }
    Ok(())
}

fn check_state(connection: &sqlez::connection::Connection) -> Result<()> {
    let schema = database::ATTACHED;
    ensure!(
        connection.select_row::<String>(&format!("SELECT root FROM {schema}.meta"))?()?.is_some(),
        "Backup verification failed: Quire's folder state has no notebook. Choose another backup."
    );
    for (page, body) in
        connection.select::<(String, String)>(&format!("SELECT page,body FROM {schema}.drafts"))?()?
    {
        serde_json::from_str::<Draft>(&body).with_context(|| {
            format!("Backup verification failed: the recovery draft for {page} is damaged. Choose another backup.")
        })?;
    }
    Ok(())
}
