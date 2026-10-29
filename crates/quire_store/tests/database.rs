use quire_store::database::open_writable;
use std::{fs, path::Path};

#[test]
fn unavailable_files_and_memory_paths_fail() {
    let folder = tempfile::tempdir().unwrap();
    assert!(open_writable(&folder.path().join("missing/notebook.sqlite")).is_err());
    for name in ["", ":memory:", "relative.sqlite"] {
        assert!(open_writable(Path::new(name)).is_err());
    }
    assert!(sqlez::connection::Connection::try_open_file(":memory:").is_err());
    assert!(sqlez::connection::Connection::try_open_file("").is_err());
}

#[test]
fn writing_connections_use_full_wal_foreign_keys_and_patched_sqlite() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("notebook.sqlite");
    for _ in 0..2 {
        let connection = open_writable(&path).unwrap();
        assert!(connection.persistent());
        assert_eq!(
            connection
                .select_row::<String>("PRAGMA journal_mode")
                .unwrap()()
            .unwrap()
            .as_deref(),
            Some("wal")
        );
        assert_eq!(
            connection.select_row::<i32>("PRAGMA synchronous").unwrap()().unwrap(),
            Some(2)
        );
        assert_eq!(
            connection.select_row::<i32>("PRAGMA foreign_keys").unwrap()().unwrap(),
            Some(1)
        );
        assert_eq!(
            connection
                .select_row::<i32>("PRAGMA trusted_schema")
                .unwrap()()
            .unwrap(),
            Some(0)
        );
        assert_eq!(
            connection
                .select_row::<String>("SELECT sqlite_version()")
                .unwrap()()
            .unwrap()
            .as_deref(),
            Some("3.54.1")
        );
        connection
            .exec("CREATE VIRTUAL TABLE temp.probe USING fts5(text)")
            .unwrap()()
        .unwrap();
        connection
            .exec_bound("INSERT INTO temp.probe(text) VALUES (?)")
            .unwrap()("مرحبا world")
        .unwrap();
        assert_eq!(
            connection
                .select_row_bound::<_, String>("SELECT text FROM temp.probe WHERE probe MATCH ?")
                .unwrap()("مرحبا")
            .unwrap()
            .as_deref(),
            Some("مرحبا world")
        );
    }
}

#[test]
fn corruption_is_reported_without_overwriting_the_file() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("notebook.sqlite");
    let bytes = b"This file is not a SQLite database.";
    fs::write(&path, bytes).unwrap();
    assert!(open_writable(&path).is_err());
    assert_eq!(fs::read(&path).unwrap(), bytes);
}

#[cfg(unix)]
#[test]
fn read_only_files_fail_instead_of_becoming_memory_databases() {
    use std::os::unix::fs::PermissionsExt;
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("notebook.sqlite");
    drop(open_writable(&path).unwrap());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o444)).unwrap();
    let result = open_writable(&path);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert!(result.is_err());
}

#[test]
fn committed_rows_reopen_and_failed_savepoints_roll_back() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("notebook.sqlite");
    let connection = open_writable(&path).unwrap();
    connection
        .exec("CREATE TABLE pages (text TEXT NOT NULL)")
        .unwrap()()
    .unwrap();
    let text = "'; DROP TABLE pages; -- میٹنگ 🙂";
    connection
        .exec_bound("INSERT INTO pages(text) VALUES (?)")
        .unwrap()(text)
    .unwrap();
    let result: anyhow::Result<()> = connection.with_savepoint("save", || {
        connection.exec_bound("INSERT INTO pages(text) VALUES (?)")?("uncommitted")?;
        anyhow::bail!("injected failure");
    });
    assert!(result.is_err());
    drop(connection);
    let reopened = open_writable(&path).unwrap();
    assert_eq!(
        reopened.select::<String>("SELECT text FROM pages").unwrap()().unwrap(),
        vec![text]
    );
}

#[cfg(target_os = "macos")]
#[test]
fn macos_connections_enable_fullfsync() {
    let folder = tempfile::tempdir().unwrap();
    for _ in 0..2 {
        let connection = open_writable(&folder.path().join("notebook.sqlite")).unwrap();
        assert_eq!(
            connection.select_row::<i32>("PRAGMA fullfsync").unwrap()().unwrap(),
            Some(1)
        );
    }
}
