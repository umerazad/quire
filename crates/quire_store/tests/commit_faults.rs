//! Fault injection through a save's database commit: every table write the save transaction
//! makes fails in turn, through a trigger that aborts it as a full disk or I/O error would. Each
//! failed save leaves every table exactly as it was, keeps the acknowledged saves and the recovery
//! snapshot, and the same save succeeds once the fault is gone. Publication's file steps are
//! covered in `notebook::commit_faults`; process kills between steps in `bun run
//! test:storage-crash`.

use quire_store::notebook::{Draft, DraftStamp, Edit, NotebookStore};
use sqlez::connection::Connection;

fn edit(base: Vec<String>, text: &str) -> Edit {
    Edit {
        page: "p-one".into(),
        base,
        path: "A.md".into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

fn draft(version: u64, base: Vec<String>, text: &str) -> Draft {
    Draft {
        page: "p-one".into(),
        path: "A.md".into(),
        text: text.into(),
        base,
        created: 1,
        stamp: DraftStamp {
            session: "editor-one".into(),
            version,
        },
    }
}

fn tables(connection: &Connection) -> Vec<String> {
    connection
        .select::<String>(
            "SELECT name FROM sqlite_schema WHERE type='table' AND name NOT LIKE 'sqlite_%' ORDER BY name",
        )
        .unwrap()()
    .unwrap()
}

/// Every row of every table, as SQL literals.
fn snapshot(connection: &Connection) -> Vec<(String, Vec<String>)> {
    tables(connection)
        .into_iter()
        .map(|table| {
            let columns = connection
                .select_bound::<&str, String>("SELECT name FROM pragma_table_info(?) ORDER BY cid")
                .unwrap()(&table)
            .unwrap();
            let row = columns
                .iter()
                .map(|column| format!("quote(\"{column}\")"))
                .collect::<Vec<_>>()
                .join("||','||");
            let rows = connection
                .select::<String>(&format!("SELECT {row} FROM \"{table}\" ORDER BY 1"))
                .unwrap()()
            .unwrap();
            (table, rows)
        })
        .collect()
}

/// The table writes `save` makes, as `EVENT table`, found by running it on a copy of the database
/// whose triggers log every write.
fn writes_of_save(connection: &Connection, copy: &std::path::Path, save: &Edit) -> Vec<String> {
    connection.exec_bound::<&str>("VACUUM INTO ?").unwrap()(copy.to_str().unwrap()).unwrap();
    let store = NotebookStore::open(copy, "nb", "office").unwrap();
    // Reopening leaves publication uncertain, which turns coalescing off; this copy has never
    // published, so it is reconciled at no cursor as the original is.
    store.mark_reconciled(None).unwrap();
    let probe = Connection::try_open_file(copy.to_str().unwrap()).unwrap();
    let names = tables(&probe);
    probe
        .exec("CREATE TABLE fault_log (position INTEGER PRIMARY KEY, write TEXT NOT NULL)")
        .unwrap()()
    .unwrap();
    for table in names {
        for event in ["INSERT", "UPDATE", "DELETE"] {
            probe
                .exec(&format!(
                    "CREATE TRIGGER \"log_{event}_{table}\" AFTER {event} ON \"{table}\" BEGIN INSERT INTO fault_log(write) VALUES ('{event} {table}'); END"
                ))
                .unwrap()()
            .unwrap();
        }
    }
    store.save(save).unwrap();
    let mut writes = probe
        .select::<String>("SELECT write FROM fault_log ORDER BY position")
        .unwrap()()
    .unwrap();
    let mut seen = std::collections::HashSet::new();
    writes.retain(|write| seen.insert(write.clone()));
    writes
}

#[test]
fn every_write_of_a_save_failing_rolls_back_everything_and_the_retry_succeeds() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    store.mark_reconciled(None).unwrap();
    let first = store.save(&edit(vec![], "- one")).unwrap();
    let mut second = edit(vec![first.revision_id().into()], "- two");
    let typed = draft(1, second.base.clone(), "- two");
    store.put_draft(&typed, None).unwrap();
    second.recovery = Some(typed.stamp.clone());
    let second = store.save(&second).unwrap();
    // The save under test coalesces the pending second revision and consumes a recovery snapshot,
    // so it inserts, updates, and deletes across the revision, head, outbox, and recovery tables.
    let pending = draft(2, vec![second.revision_id().into()], "- three");
    store.put_draft(&pending, None).unwrap();
    let mut save = edit(pending.base.clone(), &pending.text);
    save.recovery = Some(pending.stamp.clone());

    let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    let writes = writes_of_save(&connection, &root.path().join("probe.sqlite"), &save);
    for expected in [
        "INSERT revisions",
        "INSERT heads",
        "DELETE heads",
        "INSERT outbox",
        "DELETE outbox",
        "DELETE revisions",
        "DELETE drafts",
    ] {
        assert!(
            writes.iter().any(|write| write == expected),
            "{expected} in {writes:?}"
        );
    }
    let before = snapshot(&connection);
    for write in &writes {
        let (event, table) = write.split_once(' ').unwrap();
        connection
            .exec(&format!(
                "CREATE TRIGGER inject_fault BEFORE {event} ON \"{table}\" BEGIN SELECT RAISE(ABORT,'injected {event} failure on {table}'); END"
            ))
            .unwrap()()
        .unwrap();
        let result = store.save(&save);
        connection.exec("DROP TRIGGER inject_fault").unwrap()().unwrap();
        let error = result.expect_err(write);
        assert!(
            format!("{error:#}").contains("injected"),
            "{write}: {error:#}"
        );
        assert_eq!(snapshot(&connection), before, "{write}: no partial state");
        assert_eq!(store.heads("p-one").unwrap(), [second.revision_id()]);
        assert_eq!(store.draft("p-one").unwrap(), Some(pending.clone()));
    }
    let saved = store.save(&save).unwrap();
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    assert_eq!(store.heads("p-one").unwrap(), [saved.revision_id()]);
    assert!(store.draft("p-one").unwrap().is_none());
    assert!(store.get(first.revision_id()).unwrap().is_some());
}
