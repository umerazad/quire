use anyhow::Result;
use futures::executor::block_on;
use quire_store::{
    notebook::{CheckedSave, Draft, DraftStamp, Edit, NotebookStore},
    sync::{format, json::Json},
    worker::{QueueError, QueueLimits, StorageWorker},
};
use sqlez::connection::Connection;
use std::{sync::mpsc, time::Duration};

fn edit(page: &str, base: Vec<String>, text: &str) -> Edit {
    Edit {
        page: page.into(),
        base,
        path: format!("{page}.md"),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

#[test]
fn checkout_selects_one_head_and_keeps_tombstones_and_recovery_on_reopen() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let empty = store.checkout("p", None)?;
    assert!(empty.heads.is_empty());
    assert!(empty.selected.is_none());
    assert!(empty.draft.is_none());
    let base = store.save(&edit("p", vec![], "مرحبا 🙂"))?;
    let left = store.save(&edit("p", vec![base.revision_id().into()], "left"))?;
    let mut deletion = edit("p", vec![base.revision_id().into()], "");
    deletion.deleted = true;
    let right = store.save(&deletion)?;
    let heads = store.heads("p")?;
    assert_eq!(heads.len(), 2);
    let draft = Draft {
        page: "p".into(),
        path: "p.md".into(),
        text: "unsaved اردو".into(),
        base: vec![left.revision_id().into()],
        stamp: DraftStamp {
            session: "previous-run".into(),
            version: 4,
        },
        created: 2,
    };
    store.put_draft(&draft, None)?;
    drop(store);
    let store = NotebookStore::open(&path, "nb", "local")?;
    let checkout = store.checkout("p", None)?;
    assert_eq!(checkout.heads, heads);
    assert_eq!(
        checkout.selected.unwrap().root().field("id").as_str(),
        Some(heads[0].as_str())
    );
    assert_eq!(checkout.draft, Some(draft));
    let deleted = store.checkout("p", Some(right.revision_id()))?;
    assert_eq!(
        deleted.selected.unwrap().root().field("deleted").as_bool(),
        Some(true)
    );
    let selected = store.checkout("p", Some(left.revision_id()))?;
    assert_eq!(
        selected.selected.unwrap().root().field("text").as_str(),
        Some("left")
    );
    assert!(store.checkout("p", Some(base.revision_id())).is_err());
    assert!(store.checkout("absent", Some(left.revision_id())).is_err());
    Ok(())
}

#[test]
fn checked_save_refuses_changed_heads_without_consuming_recovery_or_making_a_branch() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let CheckedSave::Saved(base) = store.save_if_heads(&edit("p", vec![], "one"), &[])? else {
        panic!("new page must save");
    };
    let opened = store.checkout("p", None)?;
    let draft = Draft {
        page: "p".into(),
        path: "p.md".into(),
        text: "my typing".into(),
        base: opened.heads.clone(),
        stamp: DraftStamp {
            session: "editor".into(),
            version: 1,
        },
        created: 2,
    };
    store.put_draft(&draft, None)?;
    let peer = store.save(&edit("p", vec![base.revision_id().into()], "peer update"))?;
    let outbox = store.outbox()?;
    let mut request = edit("p", opened.heads.clone(), &draft.text);
    request.recovery = Some(draft.stamp.clone());
    let CheckedSave::HeadsChanged { heads } = store.save_if_heads(&request, &opened.heads)? else {
        panic!("stale save must be refused");
    };
    assert_eq!(heads, [peer.revision_id()]);
    assert_eq!(store.heads("p")?, heads);
    assert_eq!(store.outbox()?, outbox);
    assert_eq!(store.draft("p")?, Some(draft));
    // Refreshing the expected heads alone must not turn old text into a blind overwrite.
    assert!(store.save_if_heads(&request, &heads).is_err());
    assert_eq!(store.heads("p")?, heads);
    assert_eq!(store.outbox()?, outbox);
    Ok(())
}

#[test]
fn checked_save_cannot_implicitly_merge_conflicting_heads() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    store.save(&edit("p", vec![], "left"))?;
    store.save(&edit("p", vec![], "right"))?;
    let heads = store.heads("p")?;
    let outbox = store.outbox()?;
    assert!(
        store
            .save_if_heads(&edit("p", heads.clone(), "merge"), &heads)
            .is_err()
    );
    assert_eq!(store.heads("p")?, heads);
    assert_eq!(store.outbox()?, outbox);
    let merged = store.save(&edit("p", heads, "explicit resolution"))?;
    assert!(!merged.conflict);
    Ok(())
}

#[test]
fn checkout_does_not_read_unselected_or_unrelated_revision_bodies() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let selected = store.save(&edit("p", vec![], "selected"))?;
    let other = store.save(&edit("p", vec![], "other conflict"))?;
    let unrelated = store.save(&edit("unrelated", vec![], "large unrelated body"))?;
    let connection = Connection::try_open_file(path.to_str().unwrap())?;
    for id in [other.revision_id(), unrelated.revision_id()] {
        connection.exec_bound("UPDATE revisions SET body=? WHERE id=?")?((vec![0u8], id))?;
    }
    let checkout = store.checkout("p", Some(selected.revision_id()))?;
    assert_eq!(checkout.heads.len(), 2);
    assert_eq!(
        checkout.selected.unwrap().root().field("text").as_str(),
        Some("selected")
    );
    assert!(store.checkout("p", Some(other.revision_id())).is_err());
    assert!(store.checkout("unrelated", None).is_err());
    // A failed read rolls back its snapshot and leaves later checkouts usable.
    assert!(
        store
            .checkout("p", Some(selected.revision_id()))?
            .selected
            .is_some()
    );
    Ok(())
}

#[test]
fn checkout_retains_unrepresentable_text_and_unknown_revision_fields() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let fields = Json::parse(br#"{"page":"p","path":"A\udfff.md","text":"raw \ud800","parents":[],"generation":0,"deleted":false,"created":1,"device":"peer","future":{"s":"\udfff"}}"#)?;
    let id = format::revision_id(&fields)?;
    let raw = fields.canonical()?;
    let record = format!("{{\"id\":\"{id}\",{}", &raw[1..]);
    let batch = Json::parse(format!("{{\"notebook\":\"nb\",\"device\":\"peer\",\"seq\":1,\"previous\":null,\"created\":1,\"revisions\":[{record}],\"attachments\":[]}}").as_bytes())?;
    let encoded = format::encode(&batch, format::Kind::Batch)?;
    store.import_batch("peer", &encoded.name, &encoded.data)?;
    drop(store);
    let store = NotebookStore::open(&path, "nb", "local")?;
    let selected = store.checkout("p", None)?.selected.unwrap();
    assert!(selected.root().field("text").as_str().is_none());
    assert_eq!(
        selected.canonical()?,
        Json::parse(record.as_bytes())?.canonical()?
    );
    Ok(())
}

#[test]
fn worker_checkout_and_checked_save_observe_queue_order() -> Result<()> {
    let root = tempfile::tempdir()?;
    let worker = StorageWorker::spawn(
        root.path().join("local.sqlite"),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )?;
    block_on(worker.ready)?;
    let saved = worker
        .client
        .save_if_heads(edit("p", vec![], "saved"), vec![])?;
    let opened = worker.client.checkout("p".into(), None)?;
    let stale = worker
        .client
        .save_if_heads(edit("p", vec![], "stale"), vec![])?;
    let CheckedSave::Saved(saved) = block_on(saved)? else {
        panic!("initial save must succeed")
    };
    let opened = block_on(opened)?;
    assert_eq!(opened.heads, [saved.revision_id()]);
    assert_eq!(
        opened.selected.unwrap().root().field("text").as_str(),
        Some("saved")
    );
    assert!(
        matches!(block_on(stale)?, CheckedSave::HeadsChanged { heads } if heads == [saved.revision_id()])
    );
    worker.client.close()?;
    assert!(matches!(
        worker.client.checkout("p".into(), None),
        Err(QueueError::Stopped)
    ));
    assert!(matches!(
        worker
            .client
            .save_if_heads(edit("p", vec![], "late"), vec![]),
        Err(QueueError::Stopped)
    ));
    block_on(worker.stopped)?;
    Ok(())
}

#[test]
fn checkout_uses_a_read_snapshot_while_another_connection_holds_the_writer_lock() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )?;
    block_on(worker.ready)?;
    let saved = block_on(worker.client.save(edit("p", vec![], "committed"))?)?;
    let lock = Connection::try_open_file(path.to_str().unwrap())?;
    lock.exec("BEGIN IMMEDIATE")?()?;
    lock.exec_bound("DELETE FROM heads WHERE revision=?")?(saved.revision_id())?;
    let reply = worker.client.checkout("p".into(), None)?;
    let (sender, receiver) = mpsc::channel();
    let thread = std::thread::spawn(move || {
        let _ = sender.send(block_on(reply));
    });
    let checked_out = receiver.recv_timeout(Duration::from_secs(5));
    lock.exec("ROLLBACK")?()?;
    thread.join().unwrap();
    let checked_out =
        checked_out.expect("page checkout must not wait for the SQLite writer lock")?;
    assert_eq!(checked_out.heads, [saved.revision_id()]);
    assert_eq!(
        checked_out.selected.unwrap().root().field("text").as_str(),
        Some("committed")
    );
    worker.client.close()?;
    block_on(worker.stopped)?;
    Ok(())
}

#[test]
fn checked_save_does_not_allow_an_older_editor_version_even_with_current_heads() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let mut current = edit("p", vec![], "new typing");
    current.recovery = Some(DraftStamp {
        session: "editor".into(),
        version: 5,
    });
    let CheckedSave::Saved(saved) = store.save_if_heads(&current, &[])? else {
        panic!("current version must save")
    };
    let heads = vec![saved.revision_id().into()];
    let mut stale = edit("p", heads.clone(), "old typing");
    stale.recovery = Some(DraftStamp {
        session: "editor".into(),
        version: 4,
    });
    let outbox = store.outbox()?;
    assert!(store.save_if_heads(&stale, &heads).is_err());
    assert_eq!(store.heads("p")?, heads);
    assert_eq!(store.outbox()?, outbox);
    assert_eq!(
        store
            .checkout("p", None)?
            .selected
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("new typing")
    );
    Ok(())
}
