use super::*;
use futures::executor::block_on;
use gpui::TestAppContext;
use quire_store::notebook::{Draft, DraftStamp, Edit, NotebookStore};
use std::time::{Duration, Instant};

fn edit(text: &str) -> Edit {
    Edit {
        page: "p".into(),
        base: vec![],
        path: "Daily/2026-10-30.md".into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

pub(crate) fn wait(page: &Entity<OpenPage>, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        cx.executor().run_until_parked();
        if page.read_with(cx, |page, _| !page.is_loading()) {
            break;
        }
        assert!(Instant::now() < deadline, "native page load did not finish");
        std::thread::yield_now();
    }
}

#[gpui::test]
fn duplicate_opens_share_one_buffer_and_analysis_even_while_loading(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&edit("- مرحبا #one ^b-one\n")).unwrap();
    drop(store);
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| session.open_page("p".into(), cx));
    let duplicate = cx.update(|cx| session.open_page("p".into(), cx));
    assert_eq!(page.entity_id(), duplicate.entity_id());
    wait(&page, cx);
    let source = cx.update(|cx| {
        let loaded = page.read(cx).loaded().unwrap();
        assert_eq!(loaded.heads, [saved.revision_id()]);
        assert_eq!(loaded.path.as_deref(), Some("Daily/2026-10-30.md"));
        assert_eq!(loaded.loaded_revision_bodies, 1);
        loaded.analysis.read(cx).source.clone()
    });
    let again = cx.update(|cx| session.open_page("p".into(), cx));
    assert_eq!(page.entity_id(), again.entity_id());
    source.update(cx, |source, cx| {
        source.set_capability(language::Capability::ReadWrite, cx);
        source.edit([(2..12, "اردو")], None, cx);
    });
    cx.update(|cx| {
        assert_eq!(
            again
                .read(cx)
                .loaded()
                .unwrap()
                .analysis
                .read(cx)
                .source
                .entity_id(),
            source.entity_id()
        )
    });
    page.update(cx, |page, cx| page.retry(cx));
    cx.update(|cx| {
        assert_eq!(
            page.read(cx)
                .loaded()
                .unwrap()
                .analysis
                .read(cx)
                .source
                .read(cx)
                .snapshot()
                .text(),
            "- اردو #one ^b-one\n"
        )
    });
    assert_eq!(
        NotebookStore::open(&path, "nb", "local")
            .unwrap()
            .get(saved.revision_id())
            .unwrap()
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("- مرحبا #one ^b-one\n")
    );
    session.client.close().unwrap();
}

#[gpui::test]
fn notebook_sessions_isolate_equal_page_identities_and_restore_recovery(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let first_path = root.path().join("first.sqlite");
    let store = NotebookStore::open(&first_path, "one", "local").unwrap();
    let saved = store.save(&edit("committed")).unwrap();
    let draft = Draft {
        page: "p".into(),
        path: "Moved.md".into(),
        text: "recovered اردو".into(),
        base: vec![saved.revision_id().into()],
        stamp: DraftStamp {
            session: "previous".into(),
            version: 9,
        },
        created: 2,
    };
    store.put_draft(&draft, None).unwrap();
    drop(store);
    let mut first =
        NotebookSession::open(first_path.clone(), "one".into(), "local".into()).unwrap();
    let second_path = root.path().join("second.sqlite");
    let store = NotebookStore::open(&second_path, "two", "local").unwrap();
    store.save(&edit("other notebook")).unwrap();
    drop(store);
    let mut second = NotebookSession::open(second_path, "two".into(), "local".into()).unwrap();
    let a = cx.update(|cx| first.open_page("p".into(), cx));
    let b = cx.update(|cx| second.open_page("p".into(), cx));
    wait(&a, cx);
    wait(&b, cx);
    cx.update(|cx| {
        let a = a.read(cx).loaded().unwrap();
        let b = b.read(cx).loaded().unwrap();
        assert_ne!(a.analysis.entity_id(), b.analysis.entity_id());
        assert!(a.recovered);
        assert_eq!(a.path.as_deref(), Some("Moved.md"));
        assert_eq!(
            a.analysis.read(cx).source.read(cx).snapshot().text(),
            draft.text
        );
        assert_eq!(
            b.analysis.read(cx).source.read(cx).snapshot().text(),
            "other notebook"
        );
        assert!(!b.recovered);
    });
    assert_eq!(
        NotebookStore::open(&first_path, "one", "local")
            .unwrap()
            .draft("p")
            .unwrap(),
        Some(draft)
    );
    first.client.close().unwrap();
    second.client.close().unwrap();
}

#[gpui::test]
fn startup_failures_are_visible_and_retry_preserves_the_page_identity(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let mut session = NotebookSession::open(
        root.path().join("missing/local.sqlite"),
        "nb".into(),
        "local".into(),
    )
    .unwrap();
    let page = cx.update(|cx| session.open_page("p".into(), cx));
    wait(&page, cx);
    cx.update(|cx| {
        assert!(page.read(cx).loaded().is_none());
        assert!(page.read(cx).status(cx).contains("Cannot open"));
    });
    page.update(cx, |page, cx| page.retry(cx));
    wait(&page, cx);
    let again = cx.update(|cx| session.open_page("p".into(), cx));
    assert_eq!(page.entity_id(), again.entity_id());
    session.client.close().unwrap();
}

#[gpui::test]
fn retry_after_closed_storage_never_manufactures_an_editable_empty_page(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let worker = StorageWorker::spawn(
        root.path().join("local.sqlite"),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
    let mut session = NotebookSession::from_worker(
        worker.client,
        futures::future::ready(Ok(())).boxed().shared(),
    );
    let page = cx.update(|cx| session.open_page("p".into(), cx));
    wait(&page, cx);
    cx.update(|cx| assert!(page.read(cx).status(cx).contains("stopped")));
    page.update(cx, |page, cx| page.retry(cx));
    wait(&page, cx);
    cx.update(|cx| assert!(page.read(cx).loaded().is_none()));
}

#[gpui::test]
fn a_failed_page_read_can_retry_after_repair_without_replacing_its_registry_identity(
    cx: &mut TestAppContext,
) {
    use sqlez::connection::Connection;
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let saved = store.save(&edit("preserved source")).unwrap();
    let id = saved.revision_id().to_owned();
    drop(store);
    let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    let original = connection
        .select_row_bound::<_, Vec<u8>>("SELECT body FROM revisions WHERE id=?")
        .unwrap()(id.as_str())
    .unwrap()
    .unwrap();
    connection
        .exec_bound("UPDATE revisions SET body=? WHERE id=?")
        .unwrap()((vec![0u8], id.as_str()))
    .unwrap();
    let mut session = NotebookSession::open(path, "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| session.open_page("p".into(), cx));
    wait(&page, cx);
    cx.update(|cx| {
        assert!(page.read(cx).loaded().is_none());
        assert!(page.read(cx).can_retry());
        assert!(page.read(cx).status(cx).contains("damaged"));
    });
    connection
        .exec_bound("UPDATE revisions SET body=? WHERE id=?")
        .unwrap()((original, id.as_str()))
    .unwrap();
    page.update(cx, |page, cx| page.retry(cx));
    wait(&page, cx);
    let again = cx.update(|cx| session.open_page("p".into(), cx));
    assert_eq!(page.entity_id(), again.entity_id());
    cx.update(|cx| {
        assert_eq!(
            page.read(cx)
                .loaded()
                .unwrap()
                .analysis
                .read(cx)
                .source
                .read(cx)
                .snapshot()
                .text(),
            "preserved source"
        )
    });
    session.client.close().unwrap();
}

#[gpui::test]
fn missing_pages_load_no_revision_body_and_remain_unsaved(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| session.open_page("missing".into(), cx));
    wait(&page, cx);
    cx.update(|cx| {
        let loaded = page.read(cx).loaded().unwrap();
        assert!(loaded.path.is_none());
        assert!(loaded.heads.is_empty());
        assert_eq!(loaded.loaded_revision_bodies, 0);
        assert!(loaded.analysis.read(cx).source.read(cx).read_only());
        assert!(page.read(cx).status(cx).contains("page not saved"));
    });
    assert!(
        NotebookStore::open(&path, "nb", "local")
            .unwrap()
            .outbox()
            .unwrap()
            .is_empty()
    );
    session.client.close().unwrap();
}

#[gpui::test]
fn unrepresentable_page_strings_fail_without_a_lossy_buffer_or_storage_changes(
    cx: &mut TestAppContext,
) {
    use quire_store::{
        notebook::BatchImport,
        sync::{format, json::Json},
    };
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    for (index, (text, path)) in [
        (r#""raw \ud800""#, r#""Note.md""#),
        (r#""valid text""#, r#""A\udfff.md""#),
    ]
    .into_iter()
    .enumerate()
    {
        let database = root.path().join(format!("{index}.sqlite"));
        let fields = Json::parse(format!(r#"{{"page":"p","path":{path},"text":{text},"parents":[],"generation":0,"deleted":false,"created":1,"device":"peer","future":{{"s":"\udfff"}}}}"#).as_bytes()).unwrap();
        let id = format::revision_id(&fields).unwrap();
        let raw = fields.canonical().unwrap();
        let record = format!("{{\"id\":\"{id}\",{}", &raw[1..]);
        let batch = Json::parse(format!("{{\"notebook\":\"nb\",\"device\":\"peer\",\"seq\":1,\"previous\":null,\"created\":1,\"revisions\":[{record}],\"attachments\":[]}}").as_bytes()).unwrap();
        let encoded = format::encode(&batch, format::Kind::Batch).unwrap();
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        assert!(matches!(
            store
                .import_batch("peer", &encoded.name, &encoded.data)
                .unwrap(),
            BatchImport::Applied(_)
        ));
        drop(store);
        let mut session =
            NotebookSession::open(database.clone(), "nb".into(), "local".into()).unwrap();
        let page = cx.update(|cx| session.open_page("p".into(), cx));
        wait(&page, cx);
        cx.update(|cx| {
            assert!(page.read(cx).loaded().is_none());
            assert!(page.read(cx).status(cx).contains("UTF-16"));
        });
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        assert_eq!(
            store.get(&id).unwrap().unwrap().canonical().unwrap(),
            Json::parse(record.as_bytes()).unwrap().canonical().unwrap()
        );
        assert!(store.outbox().unwrap().is_empty());
        session.client.close().unwrap();
    }
}

fn draft(page: &str, path: &str, text: &str, base: Vec<String>) -> Draft {
    Draft {
        page: page.into(),
        path: path.into(),
        text: text.into(),
        base,
        stamp: DraftStamp {
            session: "previous".into(),
            version: 3,
        },
        created: 2,
    }
}

fn source_text(page: &Entity<OpenPage>, cx: &mut TestAppContext) -> String {
    cx.update(|cx| {
        page.read(cx)
            .loaded()
            .unwrap()
            .analysis
            .read(cx)
            .source
            .read(cx)
            .text()
    })
}

#[gpui::test]
fn a_restored_draft_typed_into_an_older_version_merges_when_editing_starts(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let base = store.save(&edit("- one\n- two\n")).unwrap();
    let base = vec![base.revision_id().to_owned()];
    let mut newer = edit("- one\n- two\n- three\n");
    newer.base = base.clone();
    newer.created = 2;
    store.save(&newer).unwrap();
    store
        .put_draft(
            &draft("p", "Daily/2026-10-30.md", "- one!\n- two\n", base),
            None,
        )
        .unwrap();
    drop(store);
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| session.open_page("p".into(), cx));
    wait(&page, cx);
    assert_eq!(source_text(&page, cx), "- one!\n- two\n");
    let persistence = cx.update(|cx| page.read(cx).persistence().unwrap().clone());
    cx.update(|cx| session.enable_editing(cx));
    crate::page_persistence::tests::wait(&persistence, cx);
    assert_eq!(source_text(&page, cx), "- one!\n- two\n- three\n");
    cx.update(|cx| assert!(!persistence.read(cx).is_dirty(cx)));
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let heads = store.heads("p").unwrap();
    assert_eq!(heads.len(), 1);
    assert_eq!(
        store
            .get(&heads[0])
            .unwrap()
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("- one!\n- two\n- three\n")
    );
    assert!(store.draft("p").unwrap().is_none());
    session.client.close().unwrap();
}

#[gpui::test]
fn drafts_with_an_unknown_base_or_a_moved_page_stay_for_review(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let mut drafts = vec![];
    for (page, draft_path, known) in [
        ("unknown", "unknown.md", false),
        ("moved", "Elsewhere.md", true),
    ] {
        let mut first = edit("first\n");
        first.page = page.into();
        first.path = format!("{page}.md");
        let saved = store.save(&first).unwrap();
        let mut newer = first.clone();
        newer.base = vec![saved.revision_id().into()];
        newer.text = "newer\n".into();
        store.save(&newer).unwrap();
        let base = if known {
            vec![saved.revision_id().to_owned()]
        } else {
            vec!["0".repeat(64)]
        };
        let restored = draft(page, draft_path, "draft\n", base);
        store.put_draft(&restored, None).unwrap();
        drafts.push(restored);
    }
    drop(store);
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    cx.update(|cx| session.enable_editing(cx));
    for restored in &drafts {
        let page = cx.update(|cx| session.open_page(restored.page.clone(), cx));
        wait(&page, cx);
        assert_eq!(source_text(&page, cx), "draft\n");
        cx.update(|cx| {
            assert!(page.read(cx).persistence().is_none());
            assert!(
                page.read(cx)
                    .status(cx)
                    .contains("recovered unsaved changes kept for review")
            );
        });
    }
    cx.executor().run_until_parked();
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    for restored in drafts {
        assert_eq!(store.heads(&restored.page).unwrap().len(), 1);
        assert_eq!(store.draft(&restored.page).unwrap(), Some(restored));
    }
    session.client.close().unwrap();
}

#[gpui::test]
fn a_second_session_on_an_open_notebook_is_refused_until_the_first_closes(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    store.save(&edit("committed")).unwrap();
    drop(store);
    let mut first = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let page = cx.update(|cx| first.open_page("p".into(), cx));
    wait(&page, cx);
    let refused = NotebookSession::open(path.clone(), "nb".into(), "local".into())
        .err()
        .unwrap();
    assert_eq!(refused, quire_store::notebook_lock::HELD);
    first.client.close().unwrap();
    drop(first);
    NotebookSession::open(path, "nb".into(), "local".into())
        .unwrap()
        .client
        .close()
        .unwrap();

    let notes = root.path().join("notes");
    let state = root.path().join("state");
    std::fs::create_dir(&notes).unwrap();
    std::fs::write(notes.join("Note.md"), "- hello\n").unwrap();
    let folder = NotebookSession::open_folder(notes.clone(), state.clone(), None).unwrap();
    let refused = NotebookSession::open_folder(notes.clone(), state, None)
        .err()
        .unwrap();
    assert_eq!(refused, quire_store::notebook_lock::HELD);
    assert!(!notes.join(".open.lock").exists());
    folder.client.close().unwrap();
}
