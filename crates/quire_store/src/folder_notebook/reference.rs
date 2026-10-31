//! Unsaved work and history the reference app left in a folder's `.quire` folder, read so it is
//! recoverable here. The reference app keeps one SQLite database per computer in
//! `.quire/devices/<device>/durability.sqlite` (a `pending` table of drafts and a `history`
//! table), older drafts as `.quire/devices/<device>/recovery/*.json`, and portable history
//! snapshots in `.quire/history/<device>.sqlite`. Each database is copied, with its write-ahead
//! log, into Quire's state folder and read there, so nothing inside the notebook is ever written.
//!
//! A draft becomes a recovery draft for its page and follows the usual rules when the page opens:
//! it saves at once only when its base is the file's current version, and otherwise opens in
//! conflict for review. A draft for a page that already holds other unsaved work here becomes a
//! separate recovered note instead, as the reference app does for another computer's drafts. Each
//! record is imported once; the reference app's records stay as they are.

use super::{FolderNotebook, files, history, settings::METADATA};
use crate::{
    database,
    notebook::{Draft, DraftStamp},
    sync::format::{page_id_for_path, sha256},
};
use anyhow::{Context as _, Result, bail};
use quire_core::markdown::{copy_with_new_ids, page_name, page_title};
use regex::Regex;
use serde::Deserialize;
use sqlez::connection::Connection;
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::LazyLock,
    time::UNIX_EPOCH,
};

pub(super) const TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS reference_sources (source TEXT PRIMARY KEY, fingerprint TEXT NOT NULL)",
    "CREATE TABLE IF NOT EXISTS reference_imports (key TEXT PRIMARY KEY)",
];

/// One unsaved draft as the reference app records it.
#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Pending {
    path: String,
    text: String,
    expected_revision: Option<String>,
    saved_at: i64,
}

/// A database the reference app wrote, with the drafts kept beside it.
struct Source {
    key: String,
    device: String,
    database: PathBuf,
    recovery: Option<PathBuf>,
}

/// A page that gained a recovery draft from the reference app.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecoveredDraft {
    pub page: String,
    pub path: String,
}

static FRONT_MATTER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\x{FEFF}?---\r?\n((?s:.)*?)\r?\n---(?:\r?\n|$)").expect("valid front matter")
});
static TITLE_KEY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^title:").expect("valid title key"));

/// An ordinary folder below `parent`, or None when it is missing; links are refused.
fn folder(parent: &Path, name: &str) -> Result<Option<PathBuf>> {
    let path = parent.join(name);
    match fs::symlink_metadata(&path) {
        Ok(found) if found.is_dir() && !found.file_type().is_symlink() => Ok(Some(path)),
        Ok(_) => bail!("{} is a link or a file", path.display()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn stamp(path: &Path) -> String {
    fs::symlink_metadata(path)
        .ok()
        .map(|metadata| {
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |time| time.as_nanos());
            format!("{}:{modified}", metadata.len())
        })
        .unwrap_or_default()
}

fn sorted_entries(folder: &Path) -> Result<Vec<fs::DirEntry>> {
    let mut entries: Vec<_> = fs::read_dir(folder)?.collect::<io::Result<_>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    Ok(entries)
}

impl FolderNotebook {
    /// Every database the reference app left in this notebook, or a notice when the folders
    /// cannot be read.
    fn reference_sources(&self) -> Result<Vec<Source>> {
        let Some(metadata) = folder(&self.root, METADATA)? else {
            return Ok(Vec::new());
        };
        let mut sources = Vec::new();
        if let Some(devices) = folder(&metadata, "devices")? {
            for entry in sorted_entries(&devices)? {
                let kind = entry.file_type()?;
                let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if !kind.is_dir() || kind.is_symlink() {
                    continue;
                }
                sources.push(Source {
                    key: format!("device:{name}"),
                    database: entry.path().join("durability.sqlite"),
                    recovery: Some(entry.path().join("recovery")),
                    device: name,
                });
            }
        }
        if let Some(snapshots) = folder(&metadata, "history")? {
            for entry in sorted_entries(&snapshots)? {
                let kind = entry.file_type()?;
                let name = entry.file_name().to_string_lossy().into_owned();
                let Some(device) = name.strip_suffix(".sqlite") else {
                    continue;
                };
                if kind.is_file() && !kind.is_symlink() {
                    sources.push(Source {
                        key: format!("history:{device}"),
                        device: device.to_owned(),
                        database: entry.path(),
                        recovery: None,
                    });
                }
            }
        }
        Ok(sources)
    }

    /// Imports drafts and history the reference app left since the last open. Returns the pages
    /// that gained a draft and notices for the user.
    pub(super) fn import_reference(&self) -> (Vec<RecoveredDraft>, Vec<String>) {
        let sources = match self.reference_sources() {
            Ok(sources) => sources,
            Err(error) => {
                return (
                    Vec::new(),
                    vec![format!(
                        "The reference app's recovery folders in .quire could not be read: {error:#}. Make them ordinary local folders and reopen the notebook to retry."
                    )],
                );
            }
        };
        let mut recovered = Vec::new();
        let mut notices = Vec::new();
        for source in sources {
            if let Err(error) = self.import_source(&source, &mut recovered) {
                notices.push(format!(
                    "The reference app's recovery data in {} could not be read: {error:#}. It is kept unchanged; reopen the notebook to retry.",
                    source.database.display()
                ));
            }
        }
        if !recovered.is_empty() {
            let mut paths: Vec<&str> = recovered.iter().map(|draft| draft.path.as_str()).collect();
            paths.truncate(10);
            let more = recovered.len().saturating_sub(paths.len());
            notices.insert(
                0,
                format!(
                    "Recovered {} unsaved {} from the reference app: {}{}. Open each to review it before saving; the reference app's own records are unchanged.",
                    recovered.len(),
                    if recovered.len() == 1 { "draft" } else { "drafts" },
                    paths.join(", "),
                    if more > 0 { format!(" and {more} more") } else { String::new() },
                ),
            );
        }
        (recovered, notices)
    }

    fn import_source(&self, source: &Source, recovered: &mut Vec<RecoveredDraft>) -> Result<()> {
        let metadata = match fs::symlink_metadata(&source.database) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("the database is a link or not a file");
        }
        let wal = PathBuf::from(format!("{}-wal", source.database.display()));
        let fingerprint = [
            stamp(&source.database),
            stamp(&wal),
            source.recovery.as_deref().map(stamp).unwrap_or_default(),
        ]
        .join(":");
        let seen: Option<String> = self
            .connection
            .select_row_bound("SELECT fingerprint FROM reference_sources WHERE source=?")?(
            source.key.as_str(),
        )?;
        if seen.as_deref() == Some(fingerprint.as_str()) {
            return Ok(());
        }
        let copy = tempfile::tempdir_in(&self.state)
            .context("Cannot copy it into Quire's state folder; check free space")?;
        let database = copy.path().join("reference.sqlite");
        fs::copy(&source.database, &database)?;
        match fs::copy(&wal, copy.path().join("reference.sqlite-wal")) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let foreign = database::open_unconfigured(&database)?;
        let check: Option<String> = foreign.select_row("PRAGMA integrity_check")?()?;
        if check.as_deref() != Some("ok") {
            bail!(
                "the copied database failed its integrity check, perhaps because the reference app was writing it"
            );
        }
        let mut drafts = pending(&foreign)?;
        if let Some(folder) = source.recovery.as_deref() {
            for record in recovery_records(folder)? {
                if !drafts.iter().any(|draft| draft.path == record.path) {
                    drafts.push(record);
                }
            }
        }
        let rows = reference_history(&foreign)?;
        drop(foreign);
        self.connection.exec("BEGIN IMMEDIATE")?()?;
        let imported = (|| -> Result<Vec<RecoveredDraft>> {
            let now = (self.clock)();
            for (path, text, revision, saved_at) in &rows {
                if files::validate(path).is_ok() {
                    history::import(&self.connection, path, text, revision, *saved_at, now)?;
                }
            }
            let mut found = Vec::new();
            for draft in &drafts {
                if let Some(page) = self.import_draft(&source.device, draft)? {
                    found.push(page);
                }
            }
            self.connection.exec_bound(
                "INSERT INTO reference_sources(source,fingerprint) VALUES (?,?) ON CONFLICT(source) DO UPDATE SET fingerprint=excluded.fingerprint",
            )?((source.key.as_str(), fingerprint.as_str()))?;
            Ok(found)
        })();
        match imported {
            Ok(found) => {
                self.connection.exec("COMMIT")?()?;
                recovered.extend(found);
                Ok(())
            }
            Err(error) => {
                self.connection.exec("ROLLBACK")?()?;
                Err(error)
            }
        }
    }

    /// Records one reference draft as a recovery draft, once. None when it needs nothing: it was
    /// imported before, the file already holds its text, or the page has the same draft.
    fn import_draft(&self, device: &str, draft: &Pending) -> Result<Option<RecoveredDraft>> {
        let key = sha256(serde_json::to_string(&(
            device,
            &draft.path,
            &draft.text,
            &draft.expected_revision,
            draft.saved_at,
        ))?);
        let known: Option<i64> = self
            .connection
            .select_row_bound("SELECT 1 FROM reference_imports WHERE key=?")?(
            key.as_str()
        )?;
        if known.is_some() {
            return Ok(None);
        }
        self.connection
            .exec_bound("INSERT INTO reference_imports(key) VALUES (?)")?(key.as_str())?;
        if files::validate(&draft.path).is_err() {
            return Ok(None);
        }
        let disk = files::read(&self.root, &draft.path).ok().flatten();
        if disk.as_ref().is_some_and(|disk| disk.text == draft.text) {
            return Ok(None);
        }
        let page = page_id_for_path(&draft.path);
        let saved_at = u64::try_from(draft.saved_at).unwrap_or_default();
        let stamp = DraftStamp {
            session: format!("reference:{device}"),
            version: saved_at,
        };
        let (page, path, text, base) = match self.draft(&page)? {
            Some(current) if current.text == draft.text => return Ok(None),
            // An earlier draft from the same computer that nobody has edited here yet gives way
            // to a newer one, so a draft the reference app keeps updating stays one draft.
            Some(current)
                if current.stamp.session == stamp.session
                    && current.path == draft.path
                    && current.stamp.version <= stamp.version =>
            {
                (
                    page,
                    draft.path.clone(),
                    draft.text.clone(),
                    draft.expected_revision.iter().cloned().collect(),
                )
            }
            Some(_) => {
                let path = format!(
                    "Recovered drafts/Draft {} {}.md",
                    device
                        .chars()
                        .map(|c| if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                            c
                        } else {
                            '-'
                        })
                        .take(8)
                        .collect::<String>(),
                    &key[..12]
                );
                if self.draft(&page_id_for_path(&path))?.is_some()
                    || files::read(&self.root, &path).ok().flatten().is_some()
                {
                    return Ok(None);
                }
                let text = recovered_copy(draft, &path, &key);
                (page_id_for_path(&path), path, text, Vec::new())
            }
            None => (
                page,
                draft.path.clone(),
                draft.text.clone(),
                draft.expected_revision.iter().cloned().collect(),
            ),
        };
        self.write_draft(&Draft {
            page: page.clone(),
            path: path.clone(),
            text,
            base,
            stamp,
            created: saved_at,
        })?;
        Ok(Some(RecoveredDraft { page, path }))
    }
}

/// The reference app's foreign-draft page: the draft with fresh block IDs under front matter
/// titled "Recovered draft: <title>", keeping its own front matter.
fn recovered_copy(draft: &Pending, destination: &str, key: &str) -> String {
    let title = page_title(&draft.path, &draft.text).to_lossy_string();
    let mut next = 0;
    let copied = copy_with_new_ids(
        &draft.text,
        destination,
        &[page_name(&draft.path), &title],
        || {
            next += 1;
            format!("b-{}", &sha256(format!("{key}:{next}"))[..32])
        },
    );
    let (attributes, body) = match FRONT_MATTER.captures(&copied) {
        Some(found) => (
            TITLE_KEY
                .replace(&found[1], format!("recovered-source-title-{}:", &key[..8]))
                .into_owned(),
            &copied[found[0].len()..],
        ),
        None => (String::new(), copied.as_str()),
    };
    let heading = serde_json::to_string(&format!("Recovered draft: {title}"))
        .unwrap_or_else(|_| "\"Recovered draft\"".into());
    let attributes = if attributes.is_empty() {
        String::new()
    } else {
        format!("{attributes}\n")
    };
    format!("---\n{attributes}title: {heading}\n---\n\n{body}")
}

fn has_table(connection: &Connection, name: &str) -> Result<bool> {
    let found: Option<i64> = connection
        .select_row_bound("SELECT 1 FROM sqlite_master WHERE type='table' AND name=?")?(
        name
    )?;
    Ok(found.is_some())
}

fn pending(connection: &Connection) -> Result<Vec<Pending>> {
    if !has_table(connection, "pending")? {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, String, Option<String>, i64)> = connection
        .select("SELECT path,text,expected_revision,saved_at FROM pending ORDER BY path")?(
    )?;
    Ok(rows
        .into_iter()
        .map(|(path, text, expected_revision, saved_at)| Pending {
            path,
            text,
            expected_revision,
            saved_at,
        })
        .collect())
}

/// Every saved version, from the current `history` table or the older `revisions` table.
fn reference_history(connection: &Connection) -> Result<Vec<(String, String, String, i64)>> {
    if has_table(connection, "history")? {
        let rows: Vec<(String, String, Vec<u8>, String, i64)> = connection.select(
            "SELECT h.path,c.codec,c.data,h.revision,h.saved_at FROM history h JOIN contents c ON c.hash=h.content ORDER BY h.id",
        )?()?;
        return rows
            .into_iter()
            .map(|(path, codec, data, revision, saved_at)| {
                Ok((path, history::decode(&codec, &data)?, revision, saved_at))
            })
            .collect();
    }
    if has_table(connection, "revisions")? {
        return connection
            .select("SELECT path,text,revision,saved_at FROM revisions ORDER BY id")?(
        );
    }
    Ok(Vec::new())
}

/// The older JSON drafts in a device's `recovery` folder.
fn recovery_records(folder: &Path) -> Result<Vec<Pending>> {
    let entries = match sorted_entries(folder) {
        Ok(entries) => entries,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::NotFound) =>
        {
            return Ok(Vec::new());
        }
        Err(error) => return Err(error),
    };
    entries
        .into_iter()
        .filter(|entry| entry.file_name().to_string_lossy().ends_with(".json"))
        .map(|entry| {
            let kind = entry.file_type()?;
            if !kind.is_file() || kind.is_symlink() {
                bail!("a recovery record is a link or not a file");
            }
            serde_json::from_slice(&fs::read(entry.path())?)
                .context("a recovery record is malformed")
        })
        .collect()
}
