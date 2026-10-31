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
use quire_core::markdown::{Addresses, copy_with_new_ids, page_name, page_title};
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

impl Source {
    fn wal(&self) -> PathBuf {
        PathBuf::from(format!("{}-wal", self.database.display()))
    }
}

/// A draft the reference app recovered from another computer's records, as its
/// `foreign_drafts` table keeps it: the original draft, its computer, and the recovered note.
struct Origin {
    device: String,
    draft: Pending,
    recovered_path: String,
    recovered_text: String,
}

/// What one source holds.
struct Records {
    /// Each draft with the computer it was typed on. A draft the reference app recovered from
    /// another computer and has not changed since counts as that computer's original.
    drafts: Vec<(String, Pending)>,
    history: Vec<(String, String, String, i64)>,
    /// Import keys of drafts the reference app recovered from another computer and has since
    /// saved or discarded.
    settled: Vec<String>,
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
        let unreadable = |source: &Source, error: anyhow::Error| {
            format!(
                "The reference app's recovery data in {} could not be read: {error:#}. It is kept unchanged; reopen the notebook to retry.",
                source.database.display()
            )
        };
        let mut notices = Vec::new();
        let mut changed = Vec::new();
        for source in sources {
            match self.changed_source(&source) {
                Ok(Some(found)) => changed.push((source, found)),
                Ok(None) => {}
                Err(error) => notices.push(unreadable(&source, error)),
            }
        }
        // Drafts the reference app already recovered from another computer are settled before
        // any computer's own records, so that computer's copy is not imported a second time.
        let settled = self.transaction(|| {
            changed
                .iter()
                .flat_map(|(_, (_, records))| &records.settled)
                .try_for_each(|key| {
                    self.connection
                        .exec_bound("INSERT OR IGNORE INTO reference_imports(key) VALUES (?)")?(
                        key.as_str(),
                    )
                })
        });
        if let Err(error) = settled {
            notices.push(format!(
                "The reference app's recovered drafts could not be recorded: {error:#}. Check free space for Quire's application data and reopen the notebook to retry."
            ));
            return (Vec::new(), notices);
        }
        let mut recovered = Vec::new();
        for (source, (fingerprint, records)) in &changed {
            match self.import_source(source, fingerprint, records) {
                Ok(found) => recovered.extend(found),
                Err(error) => notices.push(unreadable(source, error)),
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

    /// The source's records with its fingerprint, or None when it is missing or unchanged since
    /// the last import.
    fn changed_source(&self, source: &Source) -> Result<Option<(String, Records)>> {
        let metadata = match fs::symlink_metadata(&source.database) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            bail!("the database is a link or not a file");
        }
        let fingerprint = [
            stamp(&source.database),
            stamp(&source.wal()),
            source.recovery.as_deref().map(stamp).unwrap_or_default(),
        ]
        .join(":");
        let seen: Option<String> = self
            .connection
            .select_row_bound("SELECT fingerprint FROM reference_sources WHERE source=?")?(
            source.key.as_str(),
        )?;
        if seen.as_deref() == Some(fingerprint.as_str()) {
            return Ok(None);
        }
        Ok(Some((fingerprint, self.read_source(source)?)))
    }

    /// Reads a source from a fresh copy in Quire's state folder, so the reference app's files
    /// are never opened for writing.
    fn read_source(&self, source: &Source) -> Result<Records> {
        let copy = tempfile::tempdir_in(&self.state)
            .context("Cannot copy it into Quire's state folder; check free space")?;
        let database = copy.path().join("reference.sqlite");
        fs::copy(&source.database, &database)?;
        match fs::copy(source.wal(), copy.path().join("reference.sqlite-wal")) {
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
        let origins = provenance(&foreign)?;
        let history = reference_history(&foreign)?;
        let mut used = vec![false; origins.len()];
        let drafts = drafts
            .into_iter()
            .map(|draft| {
                let origin = origins.iter().position(|origin| {
                    origin.recovered_path == draft.path && origin.recovered_text == draft.text
                });
                match origin {
                    Some(index) => {
                        used[index] = true;
                        (origins[index].device.clone(), origins[index].draft.clone())
                    }
                    None => (source.device.clone(), draft),
                }
            })
            .collect();
        let settled = origins
            .iter()
            .zip(used)
            .filter(|(_, used)| !used)
            .map(|(origin, _)| draft_key(&origin.device, &origin.draft))
            .collect::<Result<_>>()?;
        Ok(Records {
            drafts,
            history,
            settled,
        })
    }

    fn import_source(
        &self,
        source: &Source,
        fingerprint: &str,
        records: &Records,
    ) -> Result<Vec<RecoveredDraft>> {
        self.transaction(|| {
            let now = (self.clock)();
            for (path, text, revision, saved_at) in &records.history {
                if files::validate(path).is_ok() {
                    history::import(&self.connection, path, text, revision, *saved_at, now)?;
                }
            }
            let mut found = Vec::new();
            for (device, draft) in &records.drafts {
                if let Some(page) = self.import_draft(device, draft)? {
                    found.push(page);
                }
            }
            self.connection.exec_bound(
                "INSERT INTO reference_sources(source,fingerprint) VALUES (?,?) ON CONFLICT(source) DO UPDATE SET fingerprint=excluded.fingerprint",
            )?((source.key.as_str(), fingerprint))?;
            Ok(found)
        })
    }
    /// Records one reference draft as a recovery draft, once. None when it needs nothing: it was
    /// imported before, the file already holds its text, or the page has the same draft.
    fn import_draft(&self, device: &str, draft: &Pending) -> Result<Option<RecoveredDraft>> {
        let key = draft_key(device, draft)?;
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

    /// Counts every draft and saved version the reference app keeps for this notebook, read
    /// from fresh copies, and how many of them Quire's state already accounts for.
    pub fn reference_tally(&self) -> Result<ReferenceTally> {
        let mut tally = ReferenceTally::default();
        let mut versions = std::collections::BTreeSet::new();
        let mut keys = std::collections::BTreeSet::new();
        for source in self.reference_sources()? {
            if !source.database.is_file() {
                continue;
            }
            let records = self.read_source(&source)?;
            for (device, draft) in &records.drafts {
                // A computer's history snapshot repeats the drafts of its database, and a draft
                // recovered from another computer repeats that computer's own.
                let key = draft_key(device, draft)?;
                if !keys.insert(key.clone()) {
                    continue;
                }
                tally.drafts += 1;
                let known: Option<i64> = self
                    .connection
                    .select_row_bound("SELECT 1 FROM reference_imports WHERE key=?")?(
                    key.as_str(),
                )?;
                tally.drafts_accounted += usize::from(known.is_some());
            }
            for (path, _, revision, saved_at) in records.history {
                // A version can be in both a computer's database and its history snapshot.
                if files::validate(&path).is_err()
                    || !versions.insert((path.clone(), revision.clone(), saved_at))
                {
                    continue;
                }
                tally.history += 1;
                let known: Option<i64> =
                    self.connection.select_row_bound(
                        "SELECT 1 FROM history WHERE path=? AND revision=? AND saved_at=? LIMIT 1",
                    )?((path.as_str(), revision.as_str(), saved_at))?;
                tally.history_present += usize::from(known.is_some());
            }
        }
        Ok(tally)
    }
}

/// What the reference app keeps for a folder notebook beside what Quire's state accounts for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ReferenceTally {
    pub drafts: usize,
    /// Drafts imported, or found needing nothing (already the file's text).
    pub drafts_accounted: usize,
    pub history: usize,
    pub history_present: usize,
}

fn draft_key(device: &str, draft: &Pending) -> Result<String> {
    Ok(sha256(serde_json::to_string(&(
        device,
        &draft.path,
        &draft.text,
        &draft.expected_revision,
        draft.saved_at,
    ))?))
}

/// The reference app's foreign-draft page: the draft with fresh block IDs under front matter
/// titled "Recovered draft: <title>", keeping its own front matter.
fn recovered_copy(draft: &Pending, destination: &str, key: &str) -> String {
    let title = page_title(&draft.path, &draft.text).to_lossy_string();
    let mut next = 0;
    let copied = copy_with_new_ids(
        &draft.text,
        &Addresses::scan(&draft.text),
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

/// The drafts this computer's reference app recovered from other computers.
fn provenance(connection: &Connection) -> Result<Vec<Origin>> {
    if !has_table(connection, "foreign_drafts")? {
        return Ok(Vec::new());
    }
    type Row = (String, String, String, Option<String>, i64, String, String);
    let rows: Vec<Row> = connection.select(
        "SELECT device,path,text,expected_revision,saved_at,recovered_path,recovered_text FROM foreign_drafts ORDER BY fingerprint",
    )?()?;
    Ok(rows
        .into_iter()
        .map(
            |(device, path, text, expected_revision, saved_at, recovered_path, recovered_text)| {
                Origin {
                    device,
                    draft: Pending {
                        path,
                        text,
                        expected_revision,
                        saved_at,
                    },
                    recovered_path,
                    recovered_text,
                }
            },
        )
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
