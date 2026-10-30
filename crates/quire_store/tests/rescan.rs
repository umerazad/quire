#![cfg(unix)]

use anyhow::Result;
use quire_store::notebook::{Edit, NotebookStore, RescanLimits, RescanWait};
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
        page: "p".into(),
        base,
        path: "A.md".into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

#[test]
fn two_folder_replicas_deliver_attachments_keep_conflicts_and_merge() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let a_dir = root.path().join("a");
    let b_dir = root.path().join("b");
    fs::create_dir(&a_dir)?;
    fs::create_dir(&b_dir)?;
    let a = NotebookStore::open(&a_dir.join("local.sqlite"), "nb", "office")?;
    let b = NotebookStore::open(&b_dir.join("local.sqlite"), "nb", "home")?;
    let reference = a.attach(b"image", "a.png")?;
    let base = a.save(&edit(vec![], &format!("مرحبا 🙂 ![]({reference})")))?;
    a.publish_to(&folder, 100)?;
    let report = b.rescan(&folder, RescanLimits::default(), None)?;
    assert_eq!(report.applied, 1);
    assert_eq!(report.attachments_completed, 1);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert!(report.waiting.is_empty(), "{:?}", report.waiting);
    assert_eq!(fs::read(b_dir.join(&reference))?, b"image");
    let office = a.save(&edit(vec![base.revision_id().into()], "office edit"))?;
    let home = b.save(&edit(vec![base.revision_id().into()], "home edit"))?;
    a.publish_to(&folder, 101)?;
    b.publish_to(&folder, 102)?;
    a.rescan(&folder, RescanLimits::default(), None)?;
    b.rescan(&folder, RescanLimits::default(), None)?;
    assert_eq!(a.heads("p")?, b.heads("p")?);
    assert_eq!(a.heads("p")?.len(), 2);
    assert!(a.get(office.revision_id())?.is_some());
    assert!(b.get(home.revision_id())?.is_some());
    let merged = b.save(&edit(b.heads("p")?, "merged"))?;
    b.publish_to(&folder, 103)?;
    a.rescan(&folder, RescanLimits::default(), None)?;
    assert_eq!(a.heads("p")?, [merged.revision_id()]);
    assert_eq!(a.heads("p")?, b.heads("p")?);
    Ok(())
}

#[test]
fn one_rescan_retries_parents_from_a_later_sorted_device() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let parent = NotebookStore::open(&root.path().join("parent.sqlite"), "nb", "zzz")?;
    let child = NotebookStore::open(&root.path().join("child.sqlite"), "nb", "aaa")?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let base = parent.save(&edit(vec![], "base"))?;
    let published = parent.publish_to(&folder, 100)?.unwrap();
    child.import_batch(
        "zzz",
        &published.name,
        &fs::read(folder.join("devices/zzz/log").join(&published.name))?,
    )?;
    let next = child.save(&edit(vec![base.revision_id().into()], "child"))?;
    child.publish_to(&folder, 101)?;
    let report = receiver.rescan(&folder, RescanLimits::default(), None)?;
    assert_eq!(report.applied, 2);
    assert_eq!(report.batches, 2);
    assert!(report.waiting.is_empty(), "{:?}", report.waiting);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(receiver.heads("p")?, [next.revision_id()]);
    Ok(())
}

#[test]
fn partial_delivery_preserves_the_file_and_retries_after_completion() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    let target = root.path().join("target");
    notebook(&source)?;
    notebook(&target)?;
    let writer = NotebookStore::open(&root.path().join("writer.sqlite"), "nb", "writer")?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let saved = writer.save(&edit(vec![], "delivered later"))?;
    let publication = writer.publish_to(&source, 100)?.unwrap();
    let directory = target.join("devices/writer/log");
    fs::create_dir_all(&directory)?;
    let file = directory.join(&publication.name);
    for partial in [b"".as_slice(), b"partial"] {
        fs::write(&file, partial)?;
        let report = receiver.rescan(&target, RescanLimits::default(), None)?;
        assert_eq!(report.applied, 0);
        assert_eq!(report.problems.len(), 1);
        assert!(report.continuation.is_none());
        assert_eq!(fs::read(&file)?, partial);
        assert!(receiver.cursor("writer")?.is_none());
    }
    fs::copy(
        source.join("devices/writer/log").join(&publication.name),
        &file,
    )?;
    let report = receiver.rescan(&target, RescanLimits::default(), None)?;
    assert_eq!(report.applied, 1);
    assert_eq!(receiver.heads("p")?, [saved.revision_id()]);
    assert!(
        report
            .waiting
            .iter()
            .all(|wait| !matches!(wait, RescanWait::Parents { .. }))
    );
    Ok(())
}

#[test]
fn bounded_rescan_continues_at_the_next_device_even_when_an_earlier_device_keeps_writing()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let a = NotebookStore::open(&root.path().join("a.sqlite"), "nb", "aaa")?;
    let b = NotebookStore::open(&root.path().join("b.sqlite"), "nb", "bbb")?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let first = a.save(&edit(vec![], "a one"))?;
    a.publish_to(&folder, 100)?;
    b.save(&edit(vec![], "b one"))?;
    b.publish_to(&folder, 101)?;
    let limits = RescanLimits {
        batches: 1,
        ..Default::default()
    };
    let first_report = receiver.rescan(&folder, limits, None)?;
    let resume = first_report.continuation.unwrap();
    assert_eq!(resume.after_device.as_deref(), Some("aaa"));
    a.save(&edit(vec![first.revision_id().into()], "a two"))?;
    a.publish_to(&folder, 102)?;
    let second = receiver.rescan(&folder, limits, Some(&resume))?;
    assert_eq!(second.batches, 1);
    assert_eq!(receiver.cursor("bbb")?.unwrap().seq, 1);
    assert_eq!(receiver.cursor("aaa")?.unwrap().seq, 1);
    Ok(())
}

#[test]
fn missing_sequence_waits_then_delivers_the_chain_and_its_tombstone() -> Result<()> {
    let root = tempfile::tempdir()?;
    let source = root.path().join("source");
    let target = root.path().join("target");
    notebook(&source)?;
    notebook(&target)?;
    let writer = NotebookStore::open(&root.path().join("writer.sqlite"), "nb", "writer")?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let first = writer.save(&edit(vec![], "one"))?;
    let one = writer.publish_to(&source, 100)?.unwrap();
    let deleted = writer.save(&Edit {
        deleted: true,
        ..edit(vec![first.revision_id().into()], "")
    })?;
    let two = writer.publish_to(&source, 101)?.unwrap();
    let log = target.join("devices/writer/log");
    fs::create_dir_all(&log)?;
    fs::copy(
        source.join("devices/writer/log").join(&two.name),
        log.join(&two.name),
    )?;
    let report = receiver.rescan(&target, RescanLimits::default(), None)?;
    assert!(report.waiting.contains(&RescanWait::Sequence {
        device: "writer".into(),
        expected: 1
    }));
    assert!(receiver.heads("p")?.is_empty());
    fs::copy(
        source.join("devices/writer/log").join(&one.name),
        log.join(&one.name),
    )?;
    let report = receiver.rescan(&target, RescanLimits::default(), None)?;
    assert_eq!(report.batches, 2);
    assert!(report.waiting.is_empty());
    assert_eq!(receiver.heads("p")?, [deleted.revision_id()]);
    assert_eq!(
        receiver
            .get(deleted.revision_id())?
            .unwrap()
            .root()
            .field("deleted")
            .as_bool(),
        Some(true)
    );
    Ok(())
}

#[test]
fn unavailable_early_attachment_does_not_starve_a_later_delivered_file() -> Result<()> {
    use quire_store::sync::format::{self, Kind};
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let mut files = [b"first".as_slice(), b"second"]
        .map(|data| (format::attachment_file_name(data, "a.bin"), data));
    files.sort_by(|a, b| a.0.cmp(&b.0));
    let incoming = format::encode(
        &serde_json::json!({"notebook":"nb","device":"writer","seq":1,"previous":null,"created":1,"revisions":[],"attachments":files.iter().map(|(name,_)|name).collect::<Vec<_>>()}),
        Kind::Batch,
    )?;
    receiver.import_batch("writer", &incoming.name, &incoming.data)?;
    fs::create_dir(folder.join("attachments"))?;
    fs::write(folder.join("attachments").join(&files[1].0), files[1].1)?;
    let limits = RescanLimits {
        attachments: 1,
        ..Default::default()
    };
    let first = receiver.rescan(&folder, limits, None)?;
    assert!(first.waiting.contains(&RescanWait::Attachment {
        name: files[0].0.clone()
    }));
    let resume = first.continuation.unwrap();
    assert_eq!(
        resume.after_attachment.as_deref(),
        Some(files[0].0.as_str())
    );
    let second = receiver.rescan(&folder, limits, Some(&resume))?;
    assert_eq!(second.attachments_completed, 1);
    assert!(second.continuation.is_none());
    assert_eq!(
        receiver.pending_attachments(None, 100)?,
        std::slice::from_ref(&files[0].0)
    );
    assert_eq!(
        fs::read(root.path().join("attachments").join(&files[1].0))?,
        files[1].1
    );
    Ok(())
}

#[test]
fn attachment_byte_limits_resume_after_one_oversized_file() -> Result<()> {
    use quire_store::sync::format::{self, Kind};
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    fs::create_dir(folder.join("attachments"))?;
    let files = [b"first".as_slice(), b"second"]
        .map(|data| (format::attachment_file_name(data, "a.bin"), data));
    for (name, data) in &files {
        fs::write(folder.join("attachments").join(name), data)?;
    }
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "receiver")?;
    let incoming = format::encode(
        &serde_json::json!({"notebook":"nb","device":"writer","seq":1,"previous":null,"created":1,"revisions":[],"attachments":files.iter().map(|(name,_)| name).collect::<Vec<_>>()}),
        Kind::Batch,
    )?;
    store.import_batch("writer", &incoming.name, &incoming.data)?;
    let limits = RescanLimits {
        attachment_bytes: 1,
        ..Default::default()
    };
    let first = store.rescan(&folder, limits, None)?;
    assert_eq!(first.attachments_completed, 1);
    assert!(first.attachment_bytes > 1);
    let next = store.rescan(&folder, limits, first.continuation.as_ref())?;
    assert_eq!(next.attachments_completed, 1);
    assert!(next.continuation.is_none());
    assert!(store.pending_attachments(None, 100)?.is_empty());
    Ok(())
}

#[test]
fn byte_limits_allow_one_oversized_atomic_batch_and_resume_the_rest() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let writer = NotebookStore::open(&root.path().join("writer.sqlite"), "nb", "writer")?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let first = writer.save(&edit(vec![], "one"))?;
    writer.publish_to(&folder, 100)?;
    let second = writer.save(&edit(vec![first.revision_id().into()], "two"))?;
    writer.publish_to(&folder, 101)?;
    let limits = RescanLimits {
        batch_bytes: 1,
        ..Default::default()
    };
    let report = receiver.rescan(&folder, limits, None)?;
    assert_eq!(report.batches, 1);
    assert!(report.batch_bytes > 1);
    let next = receiver.rescan(&folder, limits, report.continuation.as_ref())?;
    assert_eq!(next.batches, 1);
    assert!(next.continuation.is_none());
    assert_eq!(receiver.heads("p")?, [second.revision_id()]);
    Ok(())
}

#[test]
fn rescan_adopts_only_the_written_frozen_batch_and_retains_a_later_local_save() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office")?;
    let first = store.save(&edit(vec![], "one"))?;
    store.mark_reconciled(None)?;
    let pending = store.prepare_publication(100)?.unwrap();
    let second = store.save(&edit(vec![first.revision_id().into()], "two"))?;
    let log = folder.join("devices/office/log");
    fs::create_dir_all(&log)?;
    fs::write(log.join(&pending.name), &pending.data)?;
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office")?;
    let report = store.rescan(&folder, RescanLimits::default(), None)?;
    assert_eq!(report.adopted, 1);
    assert_eq!(report.batches, 1);
    assert_eq!(store.outbox()?, [second.revision_id()]);
    assert!(store.pending_publication()?.is_none());
    assert_eq!(fs::read_dir(&log)?.count(), 1);
    assert_eq!(fs::read(log.join(&pending.name))?, pending.data);
    Ok(())
}

#[test]
fn historical_fork_and_newer_format_are_reported_without_mutating_the_files() -> Result<()> {
    use quire_store::sync::format::{self, Kind};
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let writer = NotebookStore::open(&root.path().join("writer.sqlite"), "nb", "writer")?;
    let receiver = NotebookStore::open(&root.path().join("receiver.sqlite"), "nb", "receiver")?;
    let saved = writer.save(&edit(vec![], "one"))?;
    writer.publish_to(&folder, 100)?;
    receiver.rescan(&folder, RescanLimits::default(), None)?;
    let fork = format::encode(
        &serde_json::json!({"notebook":"nb","device":"writer","seq":1,"previous":null,"created":200,"revisions":[],"attachments":[]}),
        Kind::Batch,
    )?;
    let file = folder.join("devices/writer/log").join(&fork.name);
    fs::write(&file, &fork.data)?;
    let raw = format::canonical(
        &serde_json::json!({"format":2,"notebook":"nb","device":"future","seq":1,"previous":null,"created":1,"revisions":[],"attachments":[]}),
    )?;
    let name = format!("000000001-{}.qlog.zst", format::sha256(&raw));
    let future = folder.join("devices/future/log");
    fs::create_dir_all(&future)?;
    fs::write(
        future.join(&name),
        zstd::stream::encode_all(raw.as_bytes(), 0)?,
    )?;
    let report = receiver.rescan(&folder, RescanLimits::default(), None)?;
    assert_eq!(report.problems.len(), 2);
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.newer_format == Some(2.))
    );
    assert_eq!(receiver.heads("p")?, [saved.revision_id()]);
    assert!(receiver.cursor("future")?.is_none());
    assert_eq!(fs::read(&file)?, fork.data);
    Ok(())
}

#[test]
fn linked_folders_and_batch_files_are_rejected_without_following_them() -> Result<()> {
    use std::os::unix::fs::symlink;
    for target in [
        "devices",
        "devices/writer",
        "devices/writer/log",
        "attachments",
    ] {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("notebook");
        notebook(&folder)?;
        let outside = root.path().join("outside");
        fs::create_dir(&outside)?;
        let path = folder.join(target);
        fs::create_dir_all(path.parent().unwrap())?;
        symlink(&outside, &path)?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "receiver")?;
        match store.rescan(&folder, RescanLimits::default(), None) {
            Ok(report) => assert!(!report.problems.is_empty(), "{target}"),
            Err(_) => assert_eq!(target, "devices"),
        }
        assert_eq!(fs::read_dir(&outside)?.count(), 0);
    }
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let log = folder.join("devices/writer/log");
    fs::create_dir_all(&log)?;
    let outside = root.path().join("outside");
    fs::write(&outside, b"preserved")?;
    symlink(
        &outside,
        log.join(format!("000000001-{}.qlog.zst", "a".repeat(64))),
    )?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "receiver")?;
    assert_eq!(
        store
            .rescan(&folder, RescanLimits::default(), None)?
            .problems
            .len(),
        1
    );
    assert_eq!(fs::read(&outside)?, b"preserved");
    Ok(())
}

#[test]
fn worker_rescan_keeps_file_io_off_the_caller() -> Result<()> {
    use futures::executor::block_on;
    use quire_store::worker::{QueueLimits, StorageWorker};
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let writer = NotebookStore::open(&root.path().join("writer.sqlite"), "nb", "writer")?;
    let saved = writer.save(&edit(vec![], "one"))?;
    writer.publish_to(&folder, 100)?;
    let worker = StorageWorker::spawn(
        root.path().join("receiver.sqlite"),
        "nb".into(),
        "receiver".into(),
        QueueLimits::default(),
    )?;
    block_on(worker.ready)?;
    let rescan = worker
        .client
        .rescan(folder, RescanLimits::default(), None)?;
    let heads = worker.client.heads("p".into())?;
    assert_eq!(block_on(rescan)?.applied, 1);
    assert_eq!(block_on(heads)?, [saved.revision_id()]);
    worker.client.close()?;
    block_on(worker.stopped)?;
    Ok(())
}
