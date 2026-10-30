#![cfg(unix)]

use anyhow::Result;
use quire_store::{
    notebook::{Edit, NotebookStore},
    sync::format::{self, Expected, Kind},
};
use std::{fs, path::Path};

fn notebook(root: &Path) -> Result<()> {
    fs::create_dir(root)?;
    fs::write(
        root.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )?;
    Ok(())
}

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

#[test]
fn folder_publication_delivers_attachments_and_advances_only_its_frozen_selection() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office")?;
    let reference = store.attach(b"image", "a.png")?;
    let first = store.save(&edit(vec![], &format!("مرحبا 🙂 ![]({reference})")))?;
    let ack = store.publish_to(&folder, 100)?.unwrap();
    assert_eq!(ack.revisions, 1);
    assert!(!ack.adopted);
    assert_eq!(fs::read(folder.join(&reference))?, b"image");
    let data = fs::read(folder.join("devices/office/log").join(&ack.name))?;
    let decoded = format::decode(
        &ack.name,
        &data,
        Kind::Batch,
        Expected {
            notebook: "nb",
            device: "office",
        },
    )?;
    assert_eq!(decoded.hash, ack.cursor.hash);
    assert_eq!(store.cursor("office")?, Some(ack.cursor.clone()));
    assert!(store.outbox()?.is_empty());
    assert!(store.publish_to(&folder, 101)?.is_none());
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office")?;
    let second = store.save(&edit(vec![first.revision_id().into()], "second"))?;
    let next = store.publish_to(&folder, 102)?.unwrap();
    assert_eq!(next.cursor.seq, 2);
    assert!(store.get(first.revision_id())?.is_some());
    assert_eq!(store.heads("p-one")?, [second.revision_id()]);
    Ok(())
}

#[test]
fn interrupted_folder_write_adopts_exact_pending_bytes_and_retains_later_saves() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office")?;
    let first = store.save(&edit(vec![], "one"))?;
    store.mark_reconciled(None)?;
    let pending = store.prepare_publication(100)?.unwrap();
    let log = folder.join("devices/office/log");
    fs::create_dir_all(&log)?;
    fs::write(log.join(&pending.name), &pending.data)?;
    let second = store.save(&edit(vec![first.revision_id().into()], "two"))?;
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office")?;
    let ack = store.publish_to(&folder, 200)?.unwrap();
    assert!(ack.adopted);
    assert_eq!(ack.name, pending.name);
    assert_eq!(fs::read(log.join(&ack.name))?, pending.data);
    assert_eq!(store.outbox()?, [second.revision_id()]);
    assert_eq!(store.publish_to(&folder, 300)?.unwrap().cursor.seq, 2);
    Ok(())
}

#[test]
fn damaged_existing_batch_is_preserved_and_never_acknowledged() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
    let saved = store.save(&edit(vec![], "one"))?;
    store.mark_reconciled(None)?;
    let pending = store.prepare_publication(100)?.unwrap();
    let log = folder.join("devices/office/log");
    fs::create_dir_all(&log)?;
    fs::write(log.join(&pending.name), b"partial")?;
    assert!(store.publish_to(&folder, 200).is_err());
    assert_eq!(fs::read(log.join(&pending.name))?, b"partial");
    assert!(store.cursor("office")?.is_none());
    assert_eq!(store.outbox()?, [saved.revision_id()]);
    assert_eq!(store.pending_publication()?, Some(pending));
    Ok(())
}

#[test]
fn changed_or_newer_identity_and_unsafe_writer_paths_never_publish() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    for header in [
        br#"{"format":1,"id":"other"}"#.as_slice(),
        br#"{"format":2,"id":"nb"}"#,
        b"partial",
    ] {
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let saved = store.save(&edit(store.heads("p-one")?, "one"))?;
        fs::write(folder.join("quire-notebook.json"), header)?;
        assert!(store.publish_to(&folder, 100).is_err());
        assert!(!folder.join("devices").exists());
        assert!(store.pending_publication()?.is_none());
        assert!(store.get(saved.revision_id())?.is_some());
    }
    for (index, device) in ["../outside", "..", "x/y", "x\\y"].iter().enumerate() {
        let store = NotebookStore::open(
            &root.path().join(format!("unsafe-{index}.sqlite")),
            "nb",
            device,
        )?;
        assert!(store.publish_to(&folder, 100).is_err());
        assert!(!folder.join("devices").exists());
    }
    Ok(())
}

#[test]
fn conflicting_or_unrecorded_writer_sequences_preserve_local_and_folder_copies() -> Result<()> {
    for duplicate in [false, true] {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("notebook");
        notebook(&folder)?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let saved = store.save(&edit(vec![], "one"))?;
        store.mark_reconciled(None)?;
        let pending = store.prepare_publication(100)?.unwrap();
        let log = folder.join("devices/office/log");
        fs::create_dir_all(&log)?;
        let name = format!("000000001-{}.qlog.zst", "a".repeat(64));
        fs::write(log.join(&name), b"foreign copy")?;
        if duplicate {
            fs::write(log.join(&pending.name), &pending.data)?;
        }
        assert!(store.publish_to(&folder, 200).is_err());
        assert_eq!(fs::read(log.join(&name))?, b"foreign copy");
        assert_eq!(store.outbox()?, [saved.revision_id()]);
        assert!(store.cursor("office")?.is_none());
        assert_eq!(store.pending_publication()?, Some(pending));
    }
    Ok(())
}

#[test]
fn missing_acknowledged_log_and_failed_scan_disable_coalescing_until_reconciled() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
    let first = store.save(&edit(vec![], "one"))?;
    let ack = store.publish_to(&folder, 100)?.unwrap();
    let path = folder.join("devices/office/log").join(&ack.name);
    let bytes = fs::read(&path)?;
    fs::remove_file(&path)?;
    let second = store.save(&edit(vec![first.revision_id().into()], "two"))?;
    assert!(store.publish_to(&folder, 101).is_err());
    let third = store.save(&edit(vec![second.revision_id().into()], "three"))?;
    assert!(
        third.replaced.is_none(),
        "a failed folder scan must leave publication uncertain"
    );
    assert!(store.get(second.revision_id())?.is_some());
    fs::write(&path, bytes)?;
    assert_eq!(store.publish_to(&folder, 102)?.unwrap().revisions, 2);
    assert!(store.outbox()?.is_empty());
    Ok(())
}

#[test]
fn corrupt_attachment_blocks_its_batch_and_keeps_both_local_saves_and_bad_file() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
    let reference = store.attach(b"original", "a.bin")?;
    let saved = store.save(&edit(vec![], &format!("![]({reference})")))?;
    fs::create_dir(folder.join("attachments"))?;
    fs::write(folder.join(&reference), b"damaged")?;
    assert!(store.publish_to(&folder, 100).is_err());
    assert_eq!(fs::read(folder.join(&reference))?, b"damaged");
    assert_eq!(store.outbox()?, [saved.revision_id()]);
    assert!(store.pending_publication()?.is_some());
    assert_eq!(fs::read_dir(folder.join("devices/office/log"))?.count(), 0);
    fs::remove_file(folder.join(&reference))?;
    assert!(store.publish_to(&folder, 200)?.is_some());
    assert_eq!(fs::read(folder.join(&reference))?, b"original");
    Ok(())
}

#[test]
fn publisher_rejects_symlinks_at_each_folder_and_file_boundary() -> Result<()> {
    use std::os::unix::fs::symlink;
    for target in [
        "quire-notebook.json",
        "devices",
        "devices/office",
        "devices/office/log",
        "attachments",
    ] {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("notebook");
        notebook(&folder)?;
        let outside = root.path().join("outside");
        fs::create_dir(&outside)?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let reference = store.attach(b"data", "a.bin")?;
        store.save(&edit(vec![], &format!("![]({reference})")))?;
        let path = folder.join(target);
        fs::create_dir_all(path.parent().unwrap())?;
        if path.is_file() {
            fs::remove_file(&path)?;
        }
        symlink(&outside, &path)?;
        assert!(store.publish_to(&folder, 100).is_err(), "{target}");
        assert_eq!(fs::read_dir(&outside)?.count(), 0, "{target}");
        assert!(store.cursor("office")?.is_none());
        assert_eq!(store.outbox()?.len(), 1);
    }
    Ok(())
}

#[test]
fn worker_orders_save_and_publication_without_blocking_the_caller_on_folder_io() -> Result<()> {
    use futures::executor::block_on;
    use quire_store::worker::{QueueLimits, StorageWorker};
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let worker = StorageWorker::spawn(
        root.path().join("local.sqlite"),
        "nb".into(),
        "office".into(),
        QueueLimits::default(),
    )?;
    block_on(worker.ready)?;
    let save = worker.client.save(edit(vec![], "one"))?;
    let publication = worker.client.publish_to(folder.clone(), 100)?;
    let saved = block_on(save)?;
    let ack = block_on(publication)?.unwrap();
    assert_eq!(ack.revisions, 1);
    assert!(folder.join("devices/office/log").join(&ack.name).is_file());
    assert!(block_on(worker.client.get(saved.revision_id().into())?)?.is_some());
    worker.client.close()?;
    block_on(worker.stopped)?;
    Ok(())
}

#[test]
fn copied_writer_identity_cannot_adopt_another_databases_unrecorded_batch() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let original = NotebookStore::open(&root.path().join("original.sqlite"), "nb", "office")?;
    original.save(&edit(vec![], "original computer"))?;
    let ack = original.publish_to(&folder, 100)?.unwrap();
    let copy = NotebookStore::open(&root.path().join("copy.sqlite"), "nb", "office")?;
    let local = copy.save(&edit(vec![], "copied identity"))?;
    assert!(copy.publish_to(&folder, 200).is_err());
    assert!(copy.pending_publication()?.is_none());
    assert!(copy.cursor("office")?.is_none());
    let later = copy.save(&edit(vec![local.revision_id().into()], "later local work"))?;
    assert!(later.replaced.is_none());
    assert_eq!(copy.outbox()?.len(), 2);
    assert_eq!(fs::read_dir(folder.join("devices/office/log"))?.count(), 1);
    assert!(folder.join("devices/office/log").join(&ack.name).is_file());
    Ok(())
}

#[test]
fn readonly_publication_folder_retains_the_pending_batch_for_retry() -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let log = folder.join("devices/office/log");
    fs::create_dir_all(&log)?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
    let saved = store.save(&edit(vec![], "saved locally"))?;
    fs::set_permissions(&log, fs::Permissions::from_mode(0o555))?;
    let result = store.publish_to(&folder, 100);
    fs::set_permissions(&log, fs::Permissions::from_mode(0o755))?;
    assert!(result.is_err());
    assert_eq!(store.outbox()?, [saved.revision_id()]);
    let pending = store.pending_publication()?.unwrap();
    assert_eq!(store.publish_to(&folder, 200)?.unwrap().name, pending.name);
    Ok(())
}

#[test]
fn linked_notebook_roots_and_batch_files_are_never_followed() -> Result<()> {
    use std::os::unix::fs::symlink;
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let alias = root.path().join("linked-root");
    symlink(&folder, &alias)?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
    let saved = store.save(&edit(vec![], "one"))?;
    assert!(store.publish_to(&alias, 100).is_err());
    assert!(!folder.join("devices").exists());
    store.mark_reconciled(None)?;
    let pending = store.prepare_publication(100)?.unwrap();
    let log = folder.join("devices/office/log");
    fs::create_dir_all(&log)?;
    let outside = root.path().join("outside");
    fs::write(&outside, &pending.data)?;
    symlink(&outside, log.join(&pending.name))?;
    assert!(store.publish_to(&folder, 200).is_err());
    assert_eq!(fs::read(&outside)?, pending.data);
    assert!(store.cursor("office")?.is_none());
    assert_eq!(store.outbox()?, [saved.revision_id()]);
    Ok(())
}
