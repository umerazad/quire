use anyhow::{Context as _, Result, ensure};
use sqlez::connection::Connection;
use std::path::Path;

/// Opens a local writing connection for the storage worker, with no memory fallback.
/// The caller owns schema migration and must not weaken the durability pragmas.
pub fn open_writable(path: &Path) -> Result<Connection> {
    ensure!(
        path.is_absolute(),
        "Cannot open notebook database at {}. Choose an absolute local file path.",
        path.display()
    );
    let name = path.to_str().context("Cannot open a database path containing invalid Unicode. Move it to a Unicode-compatible path and retry.")?;
    let connection = Connection::try_open_file(name).with_context(|| format!(
        "Cannot open notebook database {}. Check that the file and parent folder exist and are writable, then retry.", path.display()
    ))?;
    configure(&connection).with_context(|| format!(
        "Cannot configure notebook database {} for durable saves. Preserve the database, WAL, and SHM files and check permissions or restore a verified backup.", path.display()
    ))?;
    Ok(connection)
}

fn configure(connection: &Connection) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        connection.exec("PRAGMA fullfsync = ON")?()?;
        ensure!(
            connection.select_row::<i32>("PRAGMA fullfsync")?()? == Some(1),
            "SQLite did not enable macOS fullfsync. Rebuild Quire with the locked SQLite dependency."
        );
    }
    for sql in [
        "PRAGMA journal_mode = WAL",
        "PRAGMA synchronous = FULL",
        "PRAGMA foreign_keys = ON",
        "PRAGMA trusted_schema = OFF",
    ] {
        connection.exec(sql)?()?;
    }
    let mode = connection.select_row::<String>("PRAGMA journal_mode")?()?;
    let synchronous = connection.select_row::<i32>("PRAGMA synchronous")?()?;
    let foreign_keys = connection.select_row::<i32>("PRAGMA foreign_keys")?()?;
    let trusted_schema = connection.select_row::<i32>("PRAGMA trusted_schema")?()?;
    let fts = connection.select_row::<i32>("SELECT sqlite_compileoption_used('ENABLE_FTS5')")?()?;
    ensure!(
        mode.as_deref() == Some("wal")
            && synchronous == Some(2)
            && foreign_keys == Some(1)
            && trusted_schema == Some(0),
        "SQLite did not enable WAL/FULL, foreign keys, and an untrusted schema. Use the locked SQLite build on a writable local filesystem and retry."
    );
    ensure!(
        fts == Some(1),
        "SQLite lacks FTS5 support. Rebuild Quire with the locked bundled SQLite dependency."
    );
    Ok(())
}
