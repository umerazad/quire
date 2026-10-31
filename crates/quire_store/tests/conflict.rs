#![cfg(unix)]

use anyhow::Result;
use quire_store::notebook::{NotebookStore, RescanLimits};
use sqlez::connection::Connection;
use std::{fs, path::Path};

fn notebook(root: &Path) -> Result<()> {
    fs::create_dir(root)?;
    fs::write(
        root.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )?;
    Ok(())
}

fn revision(
    text: &str,
    device: &str,
    created: u64,
    parents: &[&serde_json::Value],
) -> serde_json::Value {
    let mut value = serde_json::json!({"page":"p","text":text,"path":"A.md","deleted":false,"device":device,"created":created,
        "parents":parents.iter().map(|parent| parent["id"].as_str().unwrap()).collect::<Vec<_>>(),
        "generation":parents.iter().map(|parent| parent["generation"].as_u64().unwrap() + 1).max().unwrap_or(0)});
    value["id"] = quire_store::sync::format::revision_id(&value)
        .unwrap()
        .into();
    value
}

fn deliver(
    folder: &Path,
    device: &str,
    seq: u32,
    previous: Option<&str>,
    revision: &serde_json::Value,
) -> Result<String> {
    use quire_store::sync::format::{Kind, encode};
    let encoded = encode(
        &serde_json::json!({"notebook":"nb","device":device,"seq":seq,"previous":previous,"created":1,"revisions":[revision],"attachments":[]}),
        Kind::Batch,
    )?;
    let log = folder.join("devices").join(device).join("log");
    fs::create_dir_all(&log)?;
    fs::write(log.join(&encoded.name), &encoded.data)?;
    Ok(encoded.hash)
}

/// Makes every merge fail, as a merge that cannot be stored does, so competing versions stay.
fn refuse_merges(database: &Path) -> Result<Connection> {
    let connection = Connection::try_open_file(database.to_str().unwrap())?;
    connection.exec("CREATE TRIGGER fail_merge BEFORE INSERT ON parents WHEN (SELECT COUNT(*) FROM parents WHERE child=NEW.child)>0 BEGIN SELECT RAISE(ABORT,'injected merge failure'); END")?()?;
    Ok(connection)
}

fn allow_merges(connection: &Connection) -> Result<()> {
    connection.exec("DROP TRIGGER fail_merge")?()?;
    Ok(())
}

/// A shared folder where the office and home writers changed one page from the same version.
fn competing(folder: &Path) -> Result<(serde_json::Value, serde_json::Value)> {
    notebook(folder)?;
    let base = revision("base\n", "office", 1, &[]);
    let office = revision("office line\n", "office", 20, &[&base]);
    let home = revision("home line\n", "home", 10, &[&base]);
    let one = deliver(folder, "office", 1, None, &base)?;
    deliver(folder, "office", 2, Some(&one), &office)?;
    deliver(folder, "home", 1, None, &home)?;
    Ok((office, home))
}

#[test]
fn a_chosen_version_ends_the_conflict_on_both_writers() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    let (office, home) = competing(&folder)?;
    let laptop_db = root.path().join("laptop.sqlite");
    let desk_db = root.path().join("desk.sqlite");
    let laptop = NotebookStore::open(&laptop_db, "nb", "laptop")?;
    let desk = NotebookStore::open(&desk_db, "nb", "desk")?;
    let laptop_refusal = refuse_merges(&laptop_db)?;
    let desk_refusal = refuse_merges(&desk_db)?;
    laptop.rescan(&folder, RescanLimits::default(), None)?;
    desk.rescan(&folder, RescanLimits::default(), None)?;
    allow_merges(&laptop_refusal)?;
    allow_merges(&desk_refusal)?;

    let versions = laptop.conflict_versions("p")?;
    assert_eq!(versions.len(), 2);
    let heads: Vec<String> = versions.iter().map(|v| v.revision.clone()).collect();
    assert_eq!(heads, laptop.heads("p")?);
    let by_device = |device: &str| versions.iter().find(|v| v.device == device).unwrap();
    assert_eq!(by_device("office").text, "office line\n");
    assert_eq!(by_device("office").created, 20);
    assert_eq!(by_device("home").text, "home line\n");
    assert_eq!(by_device("home").path, "A.md");
    assert!(versions.iter().all(|v| !v.deleted));

    let ack = laptop.resolve_conflict("p", &heads, "home line\n".into(), 30)?;
    let id = ack.revision_id().to_owned();
    assert!(!ack.conflict);
    assert_eq!(laptop.heads("p")?, [id.as_str()]);
    assert!(laptop.conflict_versions("p")?.is_empty());
    let mut parents = vec![office["id"].as_str().unwrap(), home["id"].as_str().unwrap()];
    parents.sort();
    let record = laptop.get(&id)?.unwrap();
    let root_value = record.root();
    assert_eq!(
        root_value
            .field("parents")
            .elements()
            .unwrap()
            .map(|parent| parent.as_str().unwrap())
            .collect::<Vec<_>>(),
        parents
    );
    assert_eq!(root_value.field("device").as_str(), Some("laptop"));
    assert_eq!(laptop.outbox()?, [id.as_str()]);

    laptop.publish_to(&folder, 31)?;
    let report = desk.rescan(&folder, RescanLimits::default(), None)?;
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(report.merged.is_empty(), "the choice needs no merge");
    assert_eq!(desk.heads("p")?, [id.as_str()]);
    assert!(desk.outbox()?.is_empty());
    assert_eq!(
        desk.get(&id)?.unwrap().root().field("text").as_str(),
        Some("home line\n")
    );
    Ok(())
}

#[test]
fn a_choice_made_from_versions_that_changed_saves_nothing() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    competing(&folder)?;
    let database = root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "local")?;
    let refusal = refuse_merges(&database)?;
    store.rescan(&folder, RescanLimits::default(), None)?;
    allow_merges(&refusal)?;
    let heads = store.heads("p")?;

    let stale = store
        .resolve_conflict("p", &heads[..1], "mine".into(), 30)
        .unwrap_err();
    assert!(stale.to_string().contains("changed while you chose"));
    assert_eq!(store.heads("p")?, heads);
    assert!(store.outbox()?.is_empty());

    store.resolve_conflict("p", &heads, "both".into(), 30)?;
    let settled = store.heads("p")?;
    let again = store
        .resolve_conflict("p", &settled, "again".into(), 31)
        .unwrap_err();
    assert!(
        again
            .to_string()
            .contains("no longer has competing versions")
    );
    assert_eq!(store.heads("p")?, settled);
    Ok(())
}

#[test]
fn a_deletion_against_an_edit_resolves_to_a_live_page_at_the_edited_path() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let base = revision("base\n", "office", 1, &[]);
    let mut gone = serde_json::json!({"page":"p","text":"base\n","path":"A.md","deleted":true,"device":"office","created":5,"parents":[base["id"]],"generation":1});
    gone["id"] = quire_store::sync::format::revision_id(&gone)?.into();
    let mut moved = serde_json::json!({"page":"p","text":"kept\n","path":"Moved.md","deleted":false,"device":"home","created":4,"parents":[base["id"]],"generation":1});
    moved["id"] = quire_store::sync::format::revision_id(&moved)?.into();
    let one = deliver(&folder, "office", 1, None, &base)?;
    deliver(&folder, "office", 2, Some(&one), &gone)?;
    deliver(&folder, "home", 1, None, &moved)?;
    let database = root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "local")?;
    let refusal = refuse_merges(&database)?;
    store.rescan(&folder, RescanLimits::default(), None)?;
    allow_merges(&refusal)?;
    let versions = store.conflict_versions("p")?;
    assert_eq!(versions.iter().filter(|v| v.deleted).count(), 1);
    let heads = store.heads("p")?;
    let ack = store.resolve_conflict("p", &heads, "base\n".into(), 9)?;
    let record = ack.revision.root();
    assert_eq!(record.field("path").as_str(), Some("Moved.md"));
    assert_eq!(record.field("deleted").as_bool(), Some(false));
    Ok(())
}
