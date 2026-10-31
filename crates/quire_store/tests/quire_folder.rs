#![cfg(unix)]

use anyhow::Result;
use quire_store::{
    notebook::{Edit, NotebookStore},
    profiles::Notebook,
    quire_folder::{self, IDENTITY_FILE},
};
use std::fs;

#[test]
fn a_new_notebook_needs_an_empty_folder_and_writes_its_identity_once() -> Result<()> {
    let root = tempfile::tempdir()?;
    let busy = root.path().join("busy");
    fs::create_dir(&busy)?;
    fs::write(busy.join("Note.md"), "keep")?;
    let error = quire_folder::create(&busy, 7).unwrap_err();
    assert!(error.to_string().contains("Choose an empty folder"));
    assert!(!busy.join(IDENTITY_FILE).exists());
    assert_eq!(fs::read_to_string(busy.join("Note.md"))?, "keep");

    let fresh = root.path().join("OneDrive").join("Notes");
    fs::create_dir_all(&fresh)?;
    fs::write(fresh.join(".DS_Store"), "")?;
    let id = quire_folder::create(&fresh, 7)?;
    assert!(quire_folder::is_quire_notebook(&fresh));
    assert_eq!(quire_folder::read_identity(&fresh)?, id);
    let written: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(fresh.join(IDENTITY_FILE))?)?;
    assert_eq!(written, serde_json::json!({"format":1,"id":id,"created":7}));
    assert!(
        quire_folder::create(&fresh, 8).is_err(),
        "a notebook's identity is never replaced"
    );
    assert_eq!(quire_folder::read_identity(&fresh)?, id);
    Ok(())
}

#[test]
fn damaged_or_newer_identities_are_refused_with_what_to_do() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path();
    assert!(!quire_folder::is_quire_notebook(folder));
    assert!(
        quire_folder::read_identity(folder)
            .unwrap_err()
            .to_string()
            .contains("has not finished syncing")
    );
    fs::write(folder.join(IDENTITY_FILE), "{\"format\":1,\"id\":\"x\"}")?;
    assert!(
        quire_folder::read_identity(folder)
            .unwrap_err()
            .to_string()
            .contains("is damaged")
    );
    fs::write(
        folder.join(IDENTITY_FILE),
        "{\"format\":2,\"id\":\"0f8fad5b-d9cb-469f-a165-70867728950e\"}",
    )?;
    assert!(
        quire_folder::read_identity(folder)
            .unwrap_err()
            .to_string()
            .contains("newer version of Quire")
    );
    Ok(())
}

#[test]
fn choosing_the_synced_folder_builds_this_computers_database_from_its_change_files() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("synced");
    let id = quire_folder::create(&folder, 1)?;
    let office = NotebookStore::open(&root.path().join("office.sqlite"), &id, "office")?;
    office.save(&Edit {
        page: "p".into(),
        base: vec![],
        path: "Projects/Plan.md".into(),
        text: "- from the office\n".into(),
        deleted: false,
        created: 1,
        recovery: None,
    })?;
    office.publish_to(&folder, 2)?;

    let data = root.path().join("quire-data");
    let (notebook, problems) = quire_folder::open(&folder, &data)?;
    assert!(problems.is_empty(), "{problems:?}");
    let Notebook::Store {
        database,
        notebook: opened,
        writer,
        folder: synced,
    } = notebook.clone()
    else {
        panic!("a Quire notebook opens as a store");
    };
    assert_eq!(opened, id);
    assert_eq!(synced, Some(fs::canonicalize(&folder)?));
    assert!(database.starts_with(&data));
    assert_ne!(writer, "office");
    let home = NotebookStore::open(&database, &id, &writer)?;
    let heads = home.heads("p")?;
    assert_eq!(heads.len(), 1);
    assert_eq!(
        home.get(&heads[0])?.unwrap().root().field("text").as_str(),
        Some("- from the office\n")
    );
    drop(home);

    let (again, _) = quire_folder::open(&folder, &data)?;
    assert_eq!(
        again, notebook,
        "a second open keeps the database and writer"
    );
    Ok(())
}

#[test]
fn a_database_of_another_notebook_at_the_expected_place_is_left_alone() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("synced");
    let id = quire_folder::create(&folder, 1)?;
    let data = root.path().join("quire-data");
    let database = data.join("notebooks").join(&id).join("local.sqlite");
    fs::create_dir_all(database.parent().unwrap())?;
    drop(NotebookStore::open(&database, "other", "w")?);
    let error = quire_folder::open(&folder, &data).unwrap_err();
    assert!(error.to_string().contains("belongs to another notebook"));
    assert_eq!(NotebookStore::identity(&database)?.notebook, "other");
    Ok(())
}
