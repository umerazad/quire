//! Saved page history for folder notebooks, as in the reference `history-store`: each distinct
//! text is stored once, compressed when that is smaller, and older rows thin out by age.

use crate::{compression, sync::format::sha256};
use anyhow::{Context as _, Result};
use sqlez::connection::Connection;

const HOUR: i64 = 3_600_000;
const DAY: i64 = 24 * HOUR;
/// Every save from the last day, the newest per hour for 30 days, then the newest per UTC day.
const KEEP_ALL: i64 = DAY;
const KEEP_HOURLY: i64 = 30 * DAY;

pub(super) const TABLES: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS contents (hash TEXT PRIMARY KEY, codec TEXT NOT NULL, data BLOB NOT NULL)",
    "CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, path TEXT NOT NULL, content TEXT NOT NULL, revision TEXT NOT NULL, saved_at INTEGER NOT NULL)",
    "CREATE INDEX IF NOT EXISTS history_by_path ON history(path, id DESC)",
    "CREATE INDEX IF NOT EXISTS history_by_content ON history(content)",
    "CREATE INDEX IF NOT EXISTS history_by_revision ON history(revision)",
];

/// One saved version of a note, newest first in listings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderRevision {
    pub id: i64,
    pub revision: String,
    pub saved_at: i64,
}

fn encode(text: &str) -> Result<(&'static str, Vec<u8>)> {
    let packed = zstd::stream::encode_all(text.as_bytes(), 0)?;
    Ok(if packed.len() < text.len() {
        ("zstd", packed)
    } else {
        ("raw", text.as_bytes().to_vec())
    })
}

fn decode(codec: &str, data: &[u8]) -> Result<String> {
    let bytes = match codec {
        "raw" => data.to_vec(),
        "zstd" => compression::decompress(data, super::files::MAX_NOTE_BYTES as usize * 4)
            .context("A saved revision is damaged. Keep the notebook's history database and choose another version.")?,
        other => anyhow::bail!(
            "A saved revision uses the {other} format, which this version cannot read. Update Quire and reopen page history."
        ),
    };
    String::from_utf8(bytes)
        .context("A saved revision is not valid text. Choose another version from page history.")
}

/// Records `text`, the version a save is about to replace, and thins `path`'s older rows.
pub(super) fn add(
    connection: &Connection,
    path: &str,
    text: &str,
    revision: &str,
    now: i64,
) -> Result<()> {
    let hash = sha256(text);
    let known: Option<i64> =
        connection.select_row_bound("SELECT 1 FROM contents WHERE hash=?")?(hash.as_str())?;
    if known.is_none() {
        let (codec, data) = encode(text)?;
        connection.exec_bound("INSERT INTO contents(hash,codec,data) VALUES (?,?,?)")?((
            hash.as_str(),
            codec,
            data,
        ))?;
    }
    connection
        .exec_bound("INSERT INTO history(path,content,revision,saved_at) VALUES (?,?,?,?)")?((
        path,
        hash.as_str(),
        revision,
        now,
    ))?;
    prune(connection, path, now)
}

fn prune(connection: &Connection, path: &str, now: i64) -> Result<()> {
    let released: Vec<String> = connection.select_bound(&format!(
        "DELETE FROM history WHERE path=? AND id NOT IN (
           SELECT id FROM (
             SELECT id, ROW_NUMBER() OVER (PARTITION BY
               CASE WHEN saved_at >= ? THEN 'r' || id
                    WHEN saved_at >= ? THEN 'h' || (saved_at / {HOUR})
                    ELSE 'd' || (saved_at / {DAY}) END
               ORDER BY saved_at DESC, id DESC) AS rank
             FROM history WHERE path=?
           ) WHERE rank = 1
         ) RETURNING content"
    ))?((path, now - KEEP_ALL, now - KEEP_HOURLY, path))?;
    let mut released = released;
    released.sort();
    released.dedup();
    for hash in released {
        connection.exec_bound(
            "DELETE FROM contents WHERE hash=? AND NOT EXISTS (SELECT 1 FROM history WHERE content=?)",
        )?((hash.as_str(), hash.as_str()))?;
    }
    Ok(())
}

pub(super) fn list(connection: &Connection, path: &str) -> Result<Vec<FolderRevision>> {
    let rows: Vec<(i64, String, i64)> = connection
        .select_bound("SELECT id,revision,saved_at FROM history WHERE path=? ORDER BY id DESC")?(
        path,
    )?;
    Ok(rows
        .into_iter()
        .map(|(id, revision, saved_at)| FolderRevision {
            id,
            revision,
            saved_at,
        })
        .collect())
}

pub(super) fn text(connection: &Connection, path: &str, id: i64) -> Result<Option<String>> {
    let row: Option<(String, Vec<u8>)> = connection.select_row_bound(
        "SELECT c.codec,c.data FROM history h JOIN contents c ON c.hash=h.content WHERE h.path=? AND h.id=?",
    )?((path, id))?;
    row.map(|(codec, data)| decode(&codec, &data)).transpose()
}

/// The newest saved text with `revision`, from any path, for reading an older base version.
pub(super) fn by_revision(
    connection: &Connection,
    revision: &str,
) -> Result<Option<(String, String)>> {
    let row: Option<(String, String, Vec<u8>)> = connection.select_row_bound(
        "SELECT h.path,c.codec,c.data FROM history h JOIN contents c ON c.hash=h.content WHERE h.revision=? ORDER BY h.id DESC LIMIT 1",
    )?(revision)?;
    row.map(|(path, codec, data)| Ok((path, decode(&codec, &data)?)))
        .transpose()
}
