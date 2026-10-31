#![cfg(unix)]

use quire_store::{
    notebook::{Edit, NotebookStore},
    sync::format,
};
use std::fs;

#[test]
fn streamed_cache_verifies_source_and_destination_and_preserves_bad_files() -> anyhow::Result<()> {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
    let data: Vec<_> = (0..8 * 1024 * 1024)
        .map(|index| (index % 251) as u8)
        .collect();
    let name = format::attachment_file_name(&data, "a.bin");
    let source = root.path().join("download");
    fs::write(&source, &data)?;
    let linked = root.path().join("linked");
    symlink(&source, &linked)?;
    assert!(store.cache_attachment_file(&name, &linked).is_err());
    assert!(!root.path().join("attachments").exists());
    store.cache_attachment_file(&name, &source)?;
    let cached = root.path().join("attachments").join(&name);
    assert_eq!(fs::read(&cached)?, data);
    fs::write(&source, b"bad download")?;
    assert!(store.cache_attachment_file(&name, &source).is_err());
    assert_eq!(fs::read(&source)?, b"bad download");
    assert_eq!(fs::read(&cached)?, data);
    fs::write(&source, &data)?;
    fs::write(&cached, b"damaged cache")?;
    assert!(store.cache_attachment_file(&name, &source).is_err());
    assert_eq!(fs::read(&cached)?, b"damaged cache");
    Ok(())
}

#[test]
fn attaching_a_file_streams_it_under_its_content_name() -> anyhow::Result<()> {
    use futures::executor::block_on;
    use quire_store::worker::{QueueLimits, StorageWorker};
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir()?;
    let data: Vec<_> = (0..3 * 1024 * 1024)
        .map(|index| (index % 241) as u8)
        .collect();
    let source = root.path().join("Scan.PDF");
    fs::write(&source, &data)?;
    let linked = root.path().join("linked.pdf");
    symlink(&source, &linked)?;
    let worker = StorageWorker::spawn(
        root.path().join("local.sqlite"),
        "nb".into(),
        "office".into(),
        QueueLimits {
            requests: 4,
            bytes: 64 * 1024,
        },
    )?;
    block_on(worker.ready)?;
    let reference = block_on(
        worker
            .client
            .attach_file(source.clone(), "Scan.PDF".into())
            .unwrap(),
    )?;
    assert_eq!(
        reference,
        format!(
            "attachments/{}",
            format::attachment_file_name(&data, "scan.pdf")
        )
    );
    assert_eq!(fs::read(root.path().join(&reference))?, data);
    assert_eq!(
        block_on(
            worker
                .client
                .attach_file(linked, "linked.pdf".into())
                .unwrap()
        )?,
        reference,
        "a link attaches the file it names"
    );
    let missing = block_on(
        worker
            .client
            .attach_file(root.path().join("gone.png"), "gone.png".into())
            .unwrap(),
    )
    .unwrap_err();
    assert!(
        missing.to_string().contains("attach it again"),
        "{missing:#}"
    );
    let folder = block_on(
        worker
            .client
            .attach_file(root.path().to_owned(), "folder".into())
            .unwrap(),
    )
    .unwrap_err();
    assert!(
        folder.to_string().contains("not an ordinary file"),
        "{folder:#}"
    );
    Ok(())
}

#[test]
fn attachment_is_content_named_and_survives_reopen_before_a_save_uses_it() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let data = b"image data";
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    let reference = store.attach(data, "picture.PNG").unwrap();
    assert_eq!(
        reference,
        format!(
            "attachments/{}",
            format::attachment_file_name(data, "picture.PNG")
        )
    );
    assert_eq!(store.attach(data, "picture.PNG").unwrap(), reference);
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    let edit = Edit {
        page: "p-image".into(),
        base: vec![],
        path: "A.md".into(),
        text: format!("![]({reference})"),
        deleted: false,
        created: 1,
        recovery: None,
    };
    let saved = store.save(&edit).unwrap();
    assert_eq!(
        store
            .get(saved.revision_id())
            .unwrap()
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some(edit.text.as_str())
    );
    fs::remove_file(root.path().join(reference)).unwrap();
    let changed = Edit {
        text: format!("{} edited", edit.text),
        base: vec![saved.revision_id().into()],
        ..edit
    };
    assert!(store.save(&changed).is_err());
    assert_eq!(store.heads("p-image").unwrap(), [saved.revision_id()]);
}

#[test]
fn cache_validates_downloaded_names_bytes_and_preserves_damaged_files() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let data = b"downloaded";
    let name = format::attachment_file_name(data, "file.bin");
    for invalid in [
        "../escape",
        "attachments/name",
        "/tmp/name",
        "",
        &format!("{}.EXE", format::sha256(data)),
    ] {
        assert!(store.cache_attachment(invalid, data).is_err());
    }
    assert!(store.cache_attachment(&name, b"wrong bytes").is_err());
    assert!(!root.path().join("attachments").exists());
    store.cache_attachment(&name, data).unwrap();
    let path = root.path().join("attachments").join(&name);
    fs::write(&path, b"corrupted").unwrap();
    assert!(store.cache_attachment(&name, data).is_err());
    assert_eq!(fs::read(&path).unwrap(), b"corrupted");
}

#[test]
fn cache_rejects_symlink_directories_and_files_without_following_them() {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    symlink(outside.path(), root.path().join("attachments")).unwrap();
    assert!(store.attach(b"secret", "x.bin").is_err());
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
    fs::remove_file(root.path().join("attachments")).unwrap();
    fs::create_dir(root.path().join("attachments")).unwrap();
    let data = b"same content";
    let name = format::attachment_file_name(data, "x.bin");
    fs::write(outside.path().join("target"), data).unwrap();
    symlink(
        outside.path().join("target"),
        root.path().join("attachments").join(&name),
    )
    .unwrap();
    assert!(store.cache_attachment(&name, data).is_err());
    assert!(
        fs::symlink_metadata(root.path().join("attachments").join(name))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn failed_catalog_commit_preserves_a_valid_file_for_retry() {
    use sqlez::connection::Connection;
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "office").unwrap();
    let connection = Connection::try_open_file(database.to_str().unwrap()).unwrap();
    connection.exec("CREATE TRIGGER reject_attachment BEFORE INSERT ON attachments BEGIN SELECT RAISE(ABORT, 'injected catalog failure'); END").unwrap()().unwrap();
    let data = b"retained source";
    let name = format::attachment_file_name(data, "file.pdf");
    let error = store.cache_attachment(&name, data).unwrap_err();
    assert!(format!("{error:#}").contains("injected catalog failure"));
    assert_eq!(
        fs::read(root.path().join("attachments").join(&name)).unwrap(),
        data
    );
    let edit = Edit {
        page: "p-image".into(),
        base: vec![],
        path: "A.md".into(),
        text: format!("![](attachments/{name})"),
        deleted: false,
        created: 1,
        recovery: None,
    };
    assert!(store.save(&edit).is_err());
    assert!(store.heads(&edit.page).unwrap().is_empty());
    connection.exec("DROP TRIGGER reject_attachment").unwrap()().unwrap();
    store.cache_attachment(&name, data).unwrap();
    store.save(&edit).unwrap();
    drop(store);
    assert!(NotebookStore::open(&database, "nb", "office").is_ok());
}

#[test]
fn worker_installs_attachment_before_acknowledging_its_reference() {
    use futures::executor::block_on;
    use quire_store::worker::{QueueLimits, StorageWorker};
    let root = tempfile::tempdir().unwrap();
    let worker = StorageWorker::spawn(
        root.path().join("local.sqlite"),
        "nb".into(),
        "office".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let reference = block_on(
        worker
            .client
            .attach(b"worker image".to_vec(), "x.jpg".into())
            .unwrap(),
    )
    .unwrap();
    let edit = Edit {
        page: "p-image".into(),
        base: vec![],
        path: "A.md".into(),
        text: format!("![]({reference})"),
        deleted: false,
        created: 1,
        recovery: None,
    };
    block_on(worker.client.save(edit).unwrap()).unwrap();
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
    assert_eq!(
        fs::read(root.path().join(reference)).unwrap(),
        b"worker image"
    );
}
