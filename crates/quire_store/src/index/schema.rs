use super::APPLICATION_ID;
use anyhow::{Context as _, Result, ensure};
use sqlez::connection::Connection;
use std::path::Path;

/// Bump when the index tables change; a mismatch rebuilds the index.
const FORMAT: i32 = 4;

const TABLES: &[&str] = &[
    "CREATE TABLE meta (singleton INTEGER PRIMARY KEY CHECK(singleton=1), notebook TEXT NOT NULL, analyzer INTEGER NOT NULL, layout TEXT NOT NULL)",
    "CREATE TABLE pages (revision TEXT PRIMARY KEY, page TEXT NOT NULL, path TEXT NOT NULL, name_key TEXT NOT NULL, title TEXT NOT NULL, title_key TEXT NOT NULL, date TEXT, deleted INTEGER NOT NULL, analyzed INTEGER NOT NULL)",
    "CREATE INDEX pages_by_name ON pages(name_key)",
    "CREATE INDEX pages_by_title ON pages(title_key)",
    "CREATE INDEX pages_by_date ON pages(date) WHERE date IS NOT NULL",
    "CREATE INDEX pages_by_page ON pages(page)",
    "CREATE INDEX pages_pending ON pages(analyzed, deleted, revision)",
    "CREATE TABLE blocks (revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE, ordinal INTEGER NOT NULL, key TEXT NOT NULL, id TEXT, start INTEGER NOT NULL, finish INTEGER NOT NULL, content_from INTEGER NOT NULL, content_to INTEGER NOT NULL, subtree_to INTEGER NOT NULL, depth INTEGER NOT NULL, kind TEXT NOT NULL, parent TEXT, text TEXT NOT NULL, content TEXT NOT NULL, UNIQUE(revision, ordinal))",
    "CREATE INDEX blocks_by_id ON blocks(id) WHERE id IS NOT NULL",
    "CREATE TABLE links (revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, syntax TEXT NOT NULL, target TEXT NOT NULL, label TEXT, media TEXT, start INTEGER NOT NULL, finish INTEGER NOT NULL, text TEXT NOT NULL, page_key TEXT, block_key TEXT, date TEXT, UNIQUE(revision, ordinal))",
    "CREATE INDEX links_by_page ON links(page_key) WHERE page_key IS NOT NULL",
    "CREATE INDEX links_by_block ON links(block_key) WHERE block_key IS NOT NULL",
    "CREATE INDEX links_by_date ON links(date) WHERE date IS NOT NULL",
    "CREATE TABLE tagged (revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE, ordinal INTEGER NOT NULL, tag TEXT NOT NULL, tag_key TEXT NOT NULL, start INTEGER NOT NULL, block_id TEXT, snippet TEXT NOT NULL, UNIQUE(revision, ordinal))",
    "CREATE INDEX tagged_by_key ON tagged(tag_key)",
    "CREATE TABLE front_tags (revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE, ordinal INTEGER NOT NULL, tag TEXT NOT NULL, UNIQUE(revision, ordinal))",
    // Dates keep their sources; `created` and `due` are re-derived when the daily layout changes.
    "CREATE TABLE tasks (revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE, ordinal INTEGER NOT NULL, key TEXT NOT NULL, id TEXT, start INTEGER NOT NULL, raw TEXT NOT NULL, text TEXT NOT NULL, state TEXT NOT NULL, tags TEXT NOT NULL, links TEXT NOT NULL, dates TEXT NOT NULL, completed TEXT, created TEXT, due TEXT, UNIQUE(revision, ordinal))",
    // Search rows mirror the reference SearchIndex: a title row per page, then one row per block.
    "CREATE TABLE search_rows (rowid INTEGER PRIMARY KEY, revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE, title TEXT NOT NULL, text TEXT NOT NULL, block_id TEXT)",
    "CREATE INDEX search_rows_by_revision ON search_rows(revision)",
    "CREATE VIRTUAL TABLE search USING fts5(revision UNINDEXED, title, text, block_id UNINDEXED, content='search_rows', content_rowid='rowid', tokenize='unicode61')",
    "CREATE TRIGGER search_rows_insert AFTER INSERT ON search_rows BEGIN INSERT INTO search(rowid,revision,title,text,block_id) VALUES (new.rowid,new.revision,new.title,new.text,new.block_id); END",
    "CREATE TRIGGER search_rows_delete AFTER DELETE ON search_rows BEGIN INSERT INTO search(search,rowid,revision,title,text,block_id) VALUES ('delete',old.rowid,old.revision,old.title,old.text,old.block_id); END",
];

/// The selected head of every page, using the same order as `page_metadata`.
macro_rules! selected {
    () => {
        "WITH selected AS (SELECT (SELECT h.revision FROM nb.heads h JOIN nb.revisions r ON r.id=h.revision
           WHERE h.page=x.page ORDER BY r.generation DESC, h.revision DESC LIMIT 1) AS revision
         FROM (SELECT DISTINCT page FROM nb.heads) x) "
    };
}
pub(super) const STALE_PAGES: &str = concat!(
    selected!(),
    "SELECT p.revision FROM pages p WHERE p.revision NOT IN (SELECT revision FROM selected)"
);
pub(super) const MISSING_PAGES: &str = concat!(
    selected!(),
    "SELECT s.revision FROM selected s WHERE NOT EXISTS (SELECT 1 FROM pages p WHERE p.revision=s.revision)"
);

pub(super) fn open(path: &Path, notebook_database: &Path, analyzer: i64) -> Result<Connection> {
    let name = path
        .to_str()
        .context("The notebook folder path is not valid Unicode. Move the notebook and retry.")?;
    let database = notebook_database
        .to_str()
        .context("The notebook database path is not valid Unicode. Move the notebook and retry.")?;
    let connection = Connection::try_open_file(name)?;
    for pragma in [
        "PRAGMA journal_mode = WAL",
        "PRAGMA synchronous = NORMAL",
        "PRAGMA foreign_keys = ON",
        "PRAGMA busy_timeout = 5000",
    ] {
        connection.exec(pragma)?()?;
    }
    connection.exec_bound("ATTACH DATABASE ? AS nb")?(format!(
        "file:{}?mode=ro",
        uri_path(database)
    ))?;
    let notebook: String = connection.select_row("SELECT notebook FROM nb.meta")?()?.context(
        "The notebook database has no identity. Reopen the notebook so it can initialize.",
    )?;
    let application: Option<i32> = connection.select_row("PRAGMA application_id")?()?;
    let format: Option<i32> = connection.select_row("PRAGMA user_version")?()?;
    if application == Some(0) && format == Some(0) {
        let tables: Option<i64> =
            connection.select_row("SELECT count(*) FROM main.sqlite_schema")?()?;
        ensure!(
            tables == Some(0),
            "The query index file holds unknown tables."
        );
        connection.exec("BEGIN IMMEDIATE")?()?;
        for table in TABLES {
            connection.exec(table)?()?;
        }
        connection.exec_bound(
            "INSERT INTO meta(singleton,notebook,analyzer,layout) VALUES (1,?,?,'')",
        )?((notebook.as_str(), analyzer))?;
        connection.exec(&format!("PRAGMA application_id = {APPLICATION_ID}"))?()?;
        connection.exec(&format!("PRAGMA user_version = {FORMAT}"))?()?;
        connection.exec("COMMIT")?()?;
        return Ok(connection);
    }
    ensure!(
        application == Some(APPLICATION_ID) && format == Some(FORMAT),
        "The query index was written by another Quire version."
    );
    let check: Option<String> = connection.select_row("PRAGMA main.quick_check(1)")?()?;
    ensure!(
        check.as_deref() == Some("ok"),
        "The query index failed its integrity check."
    );
    let stored: Option<(String, i64)> =
        connection.select_row("SELECT notebook,analyzer FROM meta")?()?;
    ensure!(
        stored == Some((notebook, analyzer)),
        "The query index belongs to another notebook or analyzer version."
    );
    Ok(connection)
}

/// Percent-encodes the characters SQLite URI filenames treat specially.
fn uri_path(path: &str) -> String {
    path.chars()
        .map(|c| match c {
            '%' => "%25".into(),
            '?' => "%3f".into(),
            '#' => "%23".into(),
            c => c.to_string(),
        })
        .collect()
}

/// Deletes the index and its WAL files. Only derived data is lost.
pub(super) fn remove(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm"] {
        let mut file = path.as_os_str().to_owned();
        file.push(suffix);
        match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(error).with_context(|| {
                    format!(
                        "Cannot remove the damaged query index {}. Close other Quire windows for this notebook, delete the file, and reopen it.",
                        Path::new(&file).display()
                    )
                });
            }
            _ => {}
        }
    }
    Ok(())
}
