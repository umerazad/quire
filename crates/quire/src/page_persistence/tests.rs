use super::*;
use futures::executor::block_on;
use gpui::{AppContext, TestAppContext};
use quire_store::{
    notebook::{Draft, DraftStamp, Edit, NotebookStore},
    worker::{QueueLimits, StorageClient, StorageWorker},
};
use std::time::{Duration, Instant};
use text::ToOffset as _;

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
            model.task.is_none() && model.merge.is_none() && model.debounce.is_none()
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

/// Waits for storage work in flight without moving simulated time, so no new timer fires.
fn settle(model: &Entity<PagePersistence>, cx: &mut TestAppContext) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        while cx.executor().tick() {}
        if model.read_with(cx, |model, _| model.task.is_none() && model.merge.is_none()) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native persistence did not finish"
        );
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
            base_text: None,
            settle: false,
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

struct Fixture {
    _root: tempfile::TempDir,
    path: std::path::PathBuf,
    store: NotebookStore,
    client: StorageClient,
    base: String,
    buffer: Entity<Buffer>,
    persistence: Entity<PagePersistence>,
}

impl Fixture {
    fn new(text: &str, cx: &mut TestAppContext) -> Self {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.sqlite");
        let store = NotebookStore::open(&path, "nb", "local").unwrap();
        let base = store.save(&initial(text)).unwrap().revision_id().to_owned();
        let worker = StorageWorker::spawn(
            path.clone(),
            "nb".into(),
            "local".into(),
            QueueLimits::default(),
        )
        .unwrap();
        block_on(worker.ready).unwrap();
        let (buffer, persistence) = model(
            text,
            OpenedPage {
                page: "p".into(),
                path: "Note.md".into(),
                base: vec![base.clone()],
                draft: None,
                base_text: Some(text.into()),
                settle: false,
            },
            worker.client.clone(),
            cx,
        );
        Self {
            _root: root,
            path,
            store,
            client: worker.client,
            base,
            buffer,
            persistence,
        }
    }

    /// Another writer's version, stored directly as an import or bulk edit would.
    fn peer(&self, text: &str, deleted: bool) -> String {
        let mut edit = initial(text);
        edit.base = vec![self.base.clone()];
        edit.deleted = deleted;
        edit.created = 2;
        self.store.save(&edit).unwrap().revision_id().to_owned()
    }

    fn head(&self) -> (String, bool) {
        let heads = self.store.heads("p").unwrap();
        assert_eq!(heads.len(), 1, "the page keeps one head");
        let record = self.store.get(&heads[0]).unwrap().unwrap();
        (
            record.root().field("text").as_str().unwrap().to_owned(),
            record.root().field("deleted").as_bool().unwrap(),
        )
    }

    fn text(&self, cx: &mut TestAppContext) -> String {
        self.buffer.read_with(cx, |buffer, _| buffer.text())
    }

    fn edit(&self, range: std::ops::Range<usize>, text: &str, cx: &mut TestAppContext) {
        self.buffer.update(cx, |buffer, cx| {
            buffer.edit([(range, text)], None, cx);
        });
    }

    fn autosave(&self, cx: &mut TestAppContext) {
        wait(&self.persistence, cx);
        cx.executor().advance_clock(Duration::from_millis(450));
        wait(&self.persistence, cx);
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.client.close().ok();
    }
}

#[gpui::test]
fn merges_another_writers_version_into_a_page_being_typed_in(cx: &mut TestAppContext) {
    let page = Fixture::new("- one\n- two\n", cx);
    page.edit(5..5, " and a half", cx);
    let peer = page.peer("- [x] one\n- two\n- three\n", false);
    page.persistence.update(cx, |model, cx| {
        model.absorb(peer, "- [x] one\n- two\n- three\n", cx)
    });
    wait(&page.persistence, cx);
    assert_eq!(page.text(cx), "- [x] one and a half\n- two\n- three\n");
    cx.update(|cx| assert!(page.persistence.read(cx).is_dirty(cx)));
    page.autosave(cx);
    cx.update(|cx| assert!(!page.persistence.read(cx).is_dirty(cx)));
    assert_eq!(
        page.head(),
        ("- [x] one and a half\n- two\n- three\n".into(), false)
    );
}

#[gpui::test]
fn absorbing_into_a_dirty_page_keeps_anchors_and_stays_out_of_undo(cx: &mut TestAppContext) {
    let page = Fixture::new("alpha\nbeta\n", cx);
    page.edit(10..10, "!", cx);
    let anchor = page
        .buffer
        .read_with(cx, |buffer, _| buffer.anchor_before(6));
    let peer = page.peer("alpha more\nbeta\n", false);
    page.persistence.update(cx, |model, cx| {
        model.absorb(peer.clone(), "alpha more\nbeta\n", cx)
    });
    wait(&page.persistence, cx);
    page.buffer.read_with(cx, |buffer, _| {
        assert_eq!(buffer.text(), "alpha more\nbeta!\n");
        assert_eq!(anchor.to_offset(&buffer.snapshot()), 11);
        assert!(buffer.is_dirty());
    });
    cx.update(|cx| {
        let model = page.persistence.read(cx);
        assert_eq!(model.opened.base, [peer]);
        assert!(model.is_dirty(cx));
    });
    page.buffer.update(cx, |buffer, cx| buffer.undo(cx));
    assert_eq!(page.text(cx), "alpha more\nbeta\n");
    page.buffer.update(cx, |buffer, cx| buffer.undo(cx));
    assert_eq!(page.text(cx), "alpha more\nbeta\n");
}

#[gpui::test]
fn absorbing_into_a_clean_page_shows_the_new_version_with_its_line_endings(
    cx: &mut TestAppContext,
) {
    let page = Fixture::new("one\r\n", cx);
    let peer = page.peer("one\r\ntwo\r\n", false);
    page.persistence.update(cx, |model, cx| {
        model.absorb(peer.clone(), "one\r\ntwo\r\n", cx)
    });
    wait(&page.persistence, cx);
    assert_eq!(page.text(cx), "one\ntwo\n");
    cx.update(|cx| {
        let model = page.persistence.read(cx);
        assert!(!model.is_dirty(cx));
        assert!(!page.buffer.read(cx).is_dirty());
        assert_eq!(model.opened.base, std::slice::from_ref(&peer));
        let snapshot = page.buffer.read(cx).snapshot();
        assert_eq!(
            &*model.source().render(&snapshot).unwrap(),
            "one\r\ntwo\r\n"
        );
    });
    assert!(page.store.draft("p").unwrap().is_none());
    assert_eq!(page.store.heads("p").unwrap(), std::slice::from_ref(&peer));
    page.persistence
        .update(cx, |model, cx| model.absorb(peer.clone(), "ignored", cx));
    wait(&page.persistence, cx);
    assert_eq!(page.text(cx), "one\ntwo\n");
    page.edit(8..8, "x", cx);
    page.persistence.update(cx, |model, cx| model.save(cx));
    wait(&page.persistence, cx);
    assert_eq!(page.head(), ("one\r\ntwo\r\nx".into(), false));
}

#[gpui::test]
fn shows_the_merge_a_stale_save_returns_while_typing_continues(cx: &mut TestAppContext) {
    use sqlez::connection::Connection;
    let page = Fixture::new("- one\n- two\n", cx);
    page.edit(5..5, "!", cx);
    let peer = page.peer("- one\n- two\n- from elsewhere\n", false);
    let lock = Connection::try_open_file(page.path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    page.persistence.update(cx, |model, cx| model.save(cx));
    cx.executor().allow_parking();
    cx.executor().run_until_parked();
    page.persistence.update(cx, |model, cx| {
        model.absorb(peer, "- one\n- two\n- from elsewhere\n", cx)
    });
    cx.executor().run_until_parked();
    assert_eq!(page.text(cx), "- one!\n- two\n");
    page.edit(6..6, "?", cx);
    lock.exec("ROLLBACK").unwrap()().unwrap();
    wait(&page.persistence, cx);
    assert_eq!(page.text(cx), "- one!?\n- two\n- from elsewhere\n");
    assert_eq!(
        page.head(),
        ("- one!\n- two\n- from elsewhere\n".into(), false)
    );
    page.autosave(cx);
    assert_eq!(
        page.head(),
        ("- one!?\n- two\n- from elsewhere\n".into(), false)
    );
    cx.update(|cx| {
        let model = page.persistence.read(cx);
        assert!(!model.is_dirty(cx));
        assert_eq!(model.status(cx), "Saved locally");
    });
}

#[gpui::test]
fn keeps_both_versions_of_a_line_changed_in_both_places_and_warns(cx: &mut TestAppContext) {
    let page = Fixture::new("- call Sam\n", cx);
    page.edit(7..10, "Alex", cx);
    page.peer("- call Robin\n", false);
    page.persistence.update(cx, |model, cx| model.save(cx));
    wait(&page.persistence, cx);
    assert_eq!(page.text(cx), "- call Robin\n- call Alex\n");
    assert_eq!(page.head(), ("- call Robin\n- call Alex\n".into(), false));
    cx.update(|cx| {
        let model = page.persistence.read(cx);
        assert!(!model.is_dirty(cx));
        assert!(model.status(cx).contains(
            "Note.md was edited in two places at once and some of the same lines changed both ways"
        ));
    });
}

#[gpui::test]
fn saves_while_typing_goes_on_without_a_pause(cx: &mut TestAppContext) {
    let page = Fixture::new("- one\n- two\n", cx);
    cx.executor().allow_parking();
    for _ in 0..12 {
        page.edit(5..5, "!", cx);
        while cx.executor().tick() {}
        cx.executor().advance_clock(Duration::from_millis(200));
        while cx.executor().tick() {}
    }
    settle(&page.persistence, cx);
    assert_eq!(
        page.head(),
        (format!("- one{}\n- two\n", "!".repeat(10)), false)
    );
}

#[gpui::test]
fn two_editors_saving_one_page_from_one_version_end_with_one_head_and_the_same_text(
    cx: &mut TestAppContext,
) {
    let first = Fixture::new("- one\n- two\n", cx);
    let worker = StorageWorker::spawn(
        first.path.clone(),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let (buffer, second) = model(
        "- one\n- two\n",
        OpenedPage {
            page: "p".into(),
            path: "Note.md".into(),
            base: vec![first.base.clone()],
            draft: None,
            base_text: Some("- one\n- two\n".into()),
            settle: false,
        },
        worker.client.clone(),
        cx,
    );
    // The second editor saves from the version both opened, after the first saved, so its save
    // is stale and merges. Their recordings are sequential: one page has one recovery slot, and
    // concurrent recordings from two processes refuse each other (see NR11.4 Remaining).
    first.edit(5..5, " first", cx);
    first.persistence.update(cx, |model, cx| model.save(cx));
    wait(&first.persistence, cx);
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(11..11, " second")], None, cx);
    });
    second.update(cx, |model, cx| model.save(cx));
    wait(&second, cx);
    let (text, _) = first.head();
    assert_eq!(text, "- one first\n- two second\n");
    let head = first.store.heads("p").unwrap().remove(0);
    for model in [&first.persistence, &second] {
        model.update(cx, |model, cx| model.absorb(head.clone(), text.clone(), cx));
        wait(model, cx);
    }
    assert_eq!(first.text(cx), text);
    assert_eq!(buffer.read_with(cx, |buffer, _| buffer.text()), text);
    cx.update(|cx| {
        assert!(!first.persistence.read(cx).is_dirty(cx));
        assert!(!second.read(cx).is_dirty(cx));
    });
    assert_eq!(first.store.heads("p").unwrap(), [head]);
    worker.client.close().unwrap();
}

#[gpui::test]
fn saving_to_a_page_deleted_elsewhere_revives_it(cx: &mut TestAppContext) {
    let page = Fixture::new("keep", cx);
    page.peer("", true);
    page.edit(4..4, " more", cx);
    page.persistence.update(cx, |model, cx| model.save(cx));
    wait(&page.persistence, cx);
    assert_eq!(page.head(), ("keep more".into(), false));
    cx.update(|cx| assert!(!page.persistence.read(cx).is_dirty(cx)));
}

#[gpui::test]
fn unchanged_text_saved_from_a_deleted_version_stays_deleted(cx: &mut TestAppContext) {
    let page = Fixture::new("keep", cx);
    page.peer("", true);
    page.edit(4..4, "x", cx);
    page.edit(4..5, "", cx);
    page.persistence.update(cx, |model, cx| model.save(cx));
    wait(&page.persistence, cx);
    assert_eq!(page.head(), ("".into(), true));
    assert_eq!(page.text(cx), "keep");
    cx.update(|cx| {
        assert!(page.persistence.read(cx).status(cx).contains(
            "deleted elsewhere. Your text stays open here; edit and save it to restore the page"
        ))
    });
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
            base_text: None,
            settle: false,
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
            base_text: None,
            settle: false,
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
            base_text: None,
            settle: false,
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
            base_text: None,
            settle: false,
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
            base_text: None,
            settle: false,
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
            base_text: None,
            settle: false,
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
