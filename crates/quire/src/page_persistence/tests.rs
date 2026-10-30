use super::*;
use futures::executor::block_on;
use gpui::{AppContext, TestAppContext};
use quire_store::{
    notebook::{Draft, DraftStamp, Edit, NotebookStore},
    worker::{QueueLimits, StorageWorker},
};
use std::time::{Duration, Instant};

fn initial(text: &str) -> Edit {
    Edit {
        page: "p".into(),
        base: vec![],
        path: "Note.md".into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

pub(crate) fn wait(model: &Entity<PagePersistence>, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while cx.executor().tick() {}
        if model.read_with(cx, |model, _| {
            model.task.is_none() && model.debounce.is_none()
        }) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native persistence did not finish"
        );
        if model.read_with(cx, |model, _| model.debounce.is_some()) {
            cx.executor().advance_clock(Duration::from_millis(100));
        }
        std::thread::yield_now();
    }
}

pub(crate) fn stop_storage(model: &Entity<PagePersistence>, cx: &mut TestAppContext) {
    model.read_with(cx, |model, _| model.client.close().unwrap());
}

#[gpui::test]
fn typing_records_recovery_at_100_ms_and_autosaves_at_450_ms(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("saved")).unwrap();
    let base = vec![saved.revision_id().into()];
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "saved",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base: base.clone(),
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    cx.executor().allow_parking();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..5, "typed")], None, cx);
    });
    while cx.executor().tick() {}
    cx.executor().advance_clock(Duration::from_millis(99));
    while cx.executor().tick() {}
    assert!(store.draft("p").unwrap().is_none());
    assert_eq!(store.heads("p").unwrap(), base);
    cx.executor().advance_clock(Duration::from_millis(1));
    wait(&persistence, cx);
    assert_eq!(store.draft("p").unwrap().unwrap().text, "typed");
    assert_eq!(store.heads("p").unwrap(), base);
    cx.update(|cx| assert!(persistence.read(cx).is_dirty(cx)));
    cx.executor().advance_clock(Duration::from_millis(350));
    wait(&persistence, cx);
    cx.update(|cx| {
        assert!(!persistence.read(cx).is_dirty(cx));
        assert!(!buffer.read(cx).is_dirty());
    });
    assert!(store.draft("p").unwrap().is_none());
    assert_eq!(
        store
            .checkout("p", None)
            .unwrap()
            .selected
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("typed")
    );
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
}

#[gpui::test]
fn stale_heads_preserve_typing_and_ancestry_while_new_recovery_keeps_working(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("saved")).unwrap();
    let base = vec![saved.revision_id().into()];
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "saved",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base: base.clone(),
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    let mut peer = initial("other writer");
    peer.base = base.clone();
    let remote = store.save(&peer).unwrap();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..5, "my text")], None, cx);
    });
    persistence.update(cx, |model, cx| model.save(cx));
    wait(&persistence, cx);
    cx.update(|cx| {
        assert!(persistence.read(cx).blocked);
        assert_eq!(persistence.read(cx).opened.base, base);
        assert!(persistence.read(cx).is_dirty(cx));
        assert!(persistence.read(cx).is_secured(cx));
    });
    assert_eq!(store.heads("p").unwrap(), [remote.revision_id()]);
    assert_eq!(store.draft("p").unwrap().unwrap().text, "my text");
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(7..7, " and later")], None, cx);
    });
    persistence.update(cx, |model, cx| model.recover(cx));
    wait(&persistence, cx);
    let draft = store.draft("p").unwrap().unwrap();
    assert_eq!(draft.text, "my text and later");
    assert_eq!(draft.base, base);
    assert_eq!(store.heads("p").unwrap(), [remote.revision_id()]);
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
}

#[gpui::test]
fn busy_queue_retries_the_frozen_request_then_records_later_typing(cx: &mut TestAppContext) {
    use sqlez::connection::Connection;
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("saved")).unwrap();
    let base = vec![saved.revision_id().into()];
    drop(store);
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits {
            requests: 1,
            bytes: 1024 * 1024,
        },
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "saved",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base,
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    let lock = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    let mut filler = initial("first");
    filler.page = "other".into();
    let first = worker.client.save(filler.clone()).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !worker.client.stats().active {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    filler.text = "second".into();
    let second = worker.client.save(filler).unwrap();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..5, "keep")], None, cx);
    });
    persistence.update(cx, |model, cx| model.recover(cx));
    wait(&persistence, cx);
    let failed = persistence.read_with(cx, |model, cx| {
        assert!(model.status(cx).contains("busy"));
        assert!(!model.is_secured(cx));
        model.pending.as_ref().unwrap().stamp.clone()
    });
    lock.exec("ROLLBACK").unwrap()().unwrap();
    block_on(first).unwrap();
    block_on(second).unwrap();
    while worker.client.stats().pending_requests != 0 {
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(4..4, " later")], None, cx);
    });
    persistence.update(cx, |model, cx| model.recover(cx));
    wait(&persistence, cx);
    let draft = block_on(worker.client.draft("p".into()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(draft.text, "keep later");
    assert_eq!(draft.stamp.session, failed.session);
    assert!(draft.stamp.version > failed.version);
    cx.update(|cx| assert!(persistence.read(cx).is_secured(cx)));
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
}

fn model(
    text: &str,
    opened: OpenedPage,
    client: StorageClient,
    cx: &mut TestAppContext,
) -> (Entity<Buffer>, Entity<PagePersistence>) {
    let prepared = crate::stored_source::PreparedSource::new(text.into());
    let (buffer, source) = cx.update(|cx| prepared.install(cx));
    let persistence =
        cx.new(|cx| PagePersistence::new(buffer.clone(), Arc::new(source), client, opened, cx));
    persistence.update(cx, |model, cx| model.enable(cx));
    (buffer, persistence)
}

#[gpui::test]
fn saves_mixed_source_and_restores_recovery_without_marking_it_saved(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("A\r\nB\rC\n")).unwrap();
    let base = vec![saved.revision_id().into()];
    drop(store);
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "A\r\nB\rC\n",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base: base.clone(),
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(2..3, "اردو")], None, cx);
    });
    persistence.update(cx, |model, cx| model.recover(cx));
    wait(&persistence, cx);
    cx.update(|cx| {
        assert!(persistence.read(cx).is_dirty(cx));
        assert!(persistence.read(cx).is_secured(cx));
        assert!(persistence.read(cx).status(cx).contains("recovery"));
    });
    let draft = block_on(worker.client.draft("p".into()).unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(draft.text, "A\r\nاردو\rC\n");
    assert_eq!(draft.base, base);
    persistence.update(cx, |model, cx| model.save(cx));
    wait(&persistence, cx);
    cx.update(|cx| {
        assert!(!persistence.read(cx).is_dirty(cx));
        assert!(persistence.read(cx).is_secured(cx));
        assert_eq!(persistence.read(cx).status(cx), "Saved locally");
    });
    assert!(
        block_on(worker.client.draft("p".into()).unwrap())
            .unwrap()
            .is_none()
    );
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let current = store.checkout("p", None).unwrap();
    assert_eq!(
        current.selected.unwrap().root().field("text").as_str(),
        Some("A\r\nاردو\rC\n")
    );
}

#[gpui::test]
fn later_typing_remains_dirty_when_an_older_save_finishes(cx: &mut TestAppContext) {
    use sqlez::connection::Connection;
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("one")).unwrap();
    let base = vec![saved.revision_id().into()];
    drop(store);
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "one",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base,
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    let lock = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..3, "save me")], None, cx);
    });
    persistence.update(cx, |model, cx| model.save(cx));
    cx.executor().allow_parking();
    cx.executor().run_until_parked();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(7..7, " and this later")], None, cx);
    });
    cx.update(|cx| assert!(persistence.read(cx).is_dirty(cx)));
    lock.exec("ROLLBACK").unwrap()().unwrap();
    wait(&persistence, cx);
    cx.update(|cx| {
        assert!(persistence.read(cx).is_dirty(cx));
        assert!(persistence.read(cx).is_secured(cx));
    });
    let checkout = block_on(worker.client.checkout("p".into(), None).unwrap()).unwrap();
    assert_eq!(
        checkout.selected.unwrap().root().field("text").as_str(),
        Some("save me")
    );
    assert_eq!(checkout.draft.unwrap().text, "save me and this later");
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
}

#[gpui::test]
fn adopts_a_prior_session_draft_before_saving(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("saved")).unwrap();
    let base = vec![saved.revision_id().into()];
    let stamp = DraftStamp {
        session: "previous-run".into(),
        version: 9,
    };
    store
        .put_draft(
            &Draft {
                page: "p".into(),
                path: "Note.md".into(),
                text: "recovered\r\n".into(),
                base: base.clone(),
                stamp: stamp.clone(),
                created: 2,
            },
            None,
        )
        .unwrap();
    drop(store);
    let worker =
        StorageWorker::spawn(path, "nb".into(), "local".into(), QueueLimits::default()).unwrap();
    block_on(worker.ready).unwrap();
    let (_, persistence) = model(
        "recovered\r\n",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base,
            draft: Some(stamp),
        },
        worker.client.clone(),
        cx,
    );
    cx.update(|cx| {
        assert!(persistence.read(cx).is_dirty(cx));
        assert!(persistence.read(cx).is_secured(cx));
    });
    persistence.update(cx, |model, cx| model.save(cx));
    wait(&persistence, cx);
    cx.update(|cx| assert!(!persistence.read(cx).is_dirty(cx)));
    assert!(
        block_on(worker.client.draft("p".into()).unwrap())
            .unwrap()
            .is_none()
    );
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
}

#[gpui::test]
fn stopped_storage_keeps_the_current_buffer_unsecured_and_retryable(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("saved")).unwrap();
    let base = vec![saved.revision_id().into()];
    drop(store);
    let worker =
        StorageWorker::spawn(path, "nb".into(), "local".into(), QueueLimits::default()).unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "saved",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base,
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..5, "keep this")], None, cx);
    });
    persistence.update(cx, |model, cx| model.recover(cx));
    wait(&persistence, cx);
    cx.update(|cx| {
        assert!(persistence.read(cx).is_dirty(cx));
        assert!(!persistence.read(cx).is_secured(cx));
        assert!(persistence.read(cx).status(cx).contains("stopped"));
        assert_eq!(buffer.read(cx).snapshot().text(), "keep this");
    });
    persistence.update(cx, |model, cx| model.recover(cx));
    wait(&persistence, cx);
    cx.update(|cx| assert_eq!(buffer.read(cx).snapshot().text(), "keep this"));
}

#[gpui::test]
fn reverting_line_endings_during_a_save_requires_a_new_recovery_ack(cx: &mut TestAppContext) {
    use sqlez::connection::Connection;
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&initial("A\n")).unwrap();
    let base = vec![saved.revision_id().into()];
    drop(store);
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, persistence) = model(
        "A\n",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base,
            draft: None,
        },
        worker.client.clone(),
        cx,
    );
    let lock = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    buffer.update(cx, |buffer, cx| {
        buffer.set_line_ending(text::LineEnding::Windows, cx)
    });
    persistence.update(cx, |model, cx| model.save(cx));
    buffer.update(cx, |buffer, cx| {
        buffer.set_line_ending(text::LineEnding::Unix, cx)
    });
    let unsecured = cx.update(|cx| !persistence.read(cx).is_secured(cx));
    lock.exec("ROLLBACK").unwrap()().unwrap();
    assert!(
        unsecured,
        "an older in-flight format change can still overwrite the stored recovery"
    );
    wait(&persistence, cx);
    let checkout = block_on(worker.client.checkout("p".into(), None).unwrap()).unwrap();
    assert_eq!(
        checkout.selected.unwrap().root().field("text").as_str(),
        Some("A\r\n")
    );
    assert_eq!(checkout.draft.unwrap().text, "A\n");
    cx.update(|cx| {
        assert!(persistence.read(cx).is_dirty(cx));
        assert!(persistence.read(cx).is_secured(cx));
    });
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
}
