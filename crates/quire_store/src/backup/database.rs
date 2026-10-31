//! SQLite work for backups: consistent snapshots of open databases with `VACUUM INTO`, and
//! read-only checks of a copy.

use anyhow::{Context as _, Result, bail, ensure};
use sqlez::connection::Connection;
use std::{collections::BTreeMap, fs::File, path::Path};

/// Rows in each table of a database, by table name.
pub type Counts = BTreeMap<String, u64>;

/// The schema name a checked database is attached under.
pub(super) const ATTACHED: &str = "backup";

fn uri(path: &Path) -> Result<String> {
    let path = path.to_str().context(
        "A notebook path is not valid Unicode. Move the notebook to a Unicode path and retry.",
    )?;
    Ok(format!("file:{}?mode=ro", crate::index::uri_path(path)))
}

/// A connection with the database at `path` attached read-only as [`ATTACHED`]. Nothing is ever
/// written through it, so checking a backup leaves its bytes as they were.
pub(super) fn read_only(path: &Path) -> Result<Connection> {
    let metadata = std::fs::symlink_metadata(path).with_context(|| {
        format!(
            "Cannot find the database {}. Check that it exists and retry.",
            path.display()
        )
    })?;
    ensure!(
        metadata.is_file(),
        "{} is not an ordinary database file. Replace it with the original file and retry.",
        path.display()
    );
    let connection = Connection::open_memory(None);
    connection.exec_bound(&format!("ATTACH DATABASE ? AS {ATTACHED}"))?(uri(path)?).with_context(
        || {
            format!(
                "Cannot open the database {} read-only. Check that it is readable and retry.",
                path.display()
            )
        },
    )?;
    Ok(connection)
}

/// Writes a consistent copy of the database at `source` to the new file `target` with
/// `VACUUM INTO`, which reads one snapshot of the database, including committed work still in its
/// write-ahead log, while other connections keep writing. The copy uses a rollback journal, is
/// synced to the drive, and passes an integrity check.
pub(super) fn snapshot(source: &Path, target: &Path) -> Result<Counts> {
    {
        let connection = read_only(source)?;
        let target_name = target.to_str().context(
            "The backup folder path is not valid Unicode. Choose a folder with a Unicode path and retry.",
        )?;
        connection.exec_bound(&format!("VACUUM {ATTACHED} INTO ?"))?(target_name).with_context(|| {
            format!(
                "Cannot copy the database {} into the backup. Check free space at the backup folder and retry.",
                source.display()
            )
        })?;
    }
    {
        let name = target
            .to_str()
            .context("The backup path is not valid Unicode.")?;
        let copy = Connection::try_open_file(name)?;
        copy.exec("PRAGMA journal_mode = DELETE")?()?;
        let mode = copy.select_row::<String>("PRAGMA journal_mode")?()?;
        ensure!(
            mode.as_deref() == Some("delete"),
            "Cannot finish the database copy {}. Check write access at the backup folder and retry.",
            target.display()
        );
    }
    File::open(target)?.sync_all().with_context(|| {
        format!(
            "Cannot flush {} to the drive. Check the backup drive and retry.",
            target.display()
        )
    })?;
    check(target)
}

/// Runs SQLite's integrity check on the database at `path`, read-only, and counts every table.
pub(super) fn check(path: &Path) -> Result<Counts> {
    let connection = read_only(path)?;
    check_attached(&connection, path)
}

pub(super) fn check_attached(connection: &Connection, path: &Path) -> Result<Counts> {
    let result = connection
        .select::<String>(&format!("PRAGMA {ATTACHED}.integrity_check"))
        .and_then(|mut check| check());
    match result {
        Ok(rows) if rows == ["ok"] => {}
        Ok(rows) => bail!(
            "The database {} failed its integrity check ({}). Choose another backup.",
            path.display(),
            rows.join("; ")
        ),
        Err(error) => bail!(
            "The database {} cannot be read ({error}). Choose another backup.",
            path.display()
        ),
    }
    let tables = connection.select::<String>(&format!(
        "SELECT name FROM {ATTACHED}.sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name"
    ))?()?;
    tables
        .into_iter()
        .map(|table| {
            let quoted = table.replace('"', "\"\"");
            let rows = connection
                .select_row::<i64>(&format!("SELECT COUNT(*) FROM {ATTACHED}.\"{quoted}\""))?(
            )?
            .unwrap_or(0);
            Ok((table, u64::try_from(rows)?))
        })
        .collect()
}
