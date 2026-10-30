use quire_store::notebook::{APPLICATION_ID, NotebookStore, SCHEMA_VERSION};
use sqlez::connection::Connection;
use std::fs;

#[test]
fn notebook_schema_is_initialized_once_and_reopens() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    for _ in 0..2 {
        let store = NotebookStore::open(&path, "nb", "office").unwrap();
        assert_eq!(store.notebook_id(), "nb");
        assert_eq!(store.device_id(), "office");
        drop(store);
        let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
        assert_eq!(
            connection
                .select_row::<i32>("PRAGMA application_id")
                .unwrap()()
            .unwrap(),
            Some(APPLICATION_ID)
        );
        assert_eq!(
            connection.select_row::<i32>("PRAGMA user_version").unwrap()().unwrap(),
            Some(SCHEMA_VERSION)
        );
        assert_eq!(
            connection
                .select_row::<i32>("SELECT COUNT(*) FROM schema_history")
                .unwrap()()
            .unwrap(),
            Some(SCHEMA_VERSION)
        );
    }
}

#[test]
fn foreign_and_newer_databases_are_preserved_before_journal_changes() {
    let root = tempfile::tempdir().unwrap();
    for (name, sql) in [
        (
            "foreign.sqlite",
            "CREATE TABLE unrelated (value TEXT)".to_owned(),
        ),
        (
            "newer.sqlite",
            format!("PRAGMA application_id={APPLICATION_ID}; PRAGMA user_version=99"),
        ),
    ] {
        let path = root.path().join(name);
        let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
        connection.exec(&sql).unwrap()().unwrap();
        drop(connection);
        let original = fs::read(&path).unwrap();
        assert!(NotebookStore::open(&path, "nb", "office").is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }
}

#[test]
fn notebook_and_writer_identity_cannot_be_reassigned() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    drop(NotebookStore::open(&path, "nb", "office").unwrap());
    assert!(NotebookStore::open(&path, "other", "office").is_err());
    assert!(NotebookStore::open(&path, "nb", "home").is_err());
    assert!(NotebookStore::open(&path, "nb", "office").is_ok());
}

#[test]
fn cursor_history_disagreement_is_rejected_before_journal_changes() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    let encoded = quire_store::sync::format::encode(&serde_json::json!({"notebook":"nb","device":"peer","seq":1,"previous":null,"created":1,"revisions":[],"attachments":[]}), quire_store::sync::format::Kind::Batch).unwrap();
    store
        .import_batch("peer", &encoded.name, &encoded.data)
        .unwrap();
    drop(store);
    let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    connection
        .exec_bound("UPDATE batch_hashes SET hash=?")
        .unwrap()("a".repeat(64))
    .unwrap();
    connection.exec("PRAGMA journal_mode=DELETE").unwrap()().unwrap();
    drop(connection);
    let original = fs::read(&path).unwrap();
    assert!(NotebookStore::open(&path, "nb", "office").is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn changed_schema_history_is_rejected_without_rebuilding() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    drop(NotebookStore::open(&path, "nb", "office").unwrap());
    let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    connection
        .exec("UPDATE schema_history SET checksum='changed'")
        .unwrap()()
    .unwrap();
    drop(connection);
    let original = fs::read(&path).unwrap();
    assert!(NotebookStore::open(&path, "nb", "office").is_err());
    assert_eq!(fs::read(&path).unwrap(), original);
}

fn edit(base: Vec<String>, text: &str) -> quire_store::notebook::Edit {
    quire_store::notebook::Edit {
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
fn saves_revisions_heads_and_outbox_together_and_reopens() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    let ack = store
        .save(&edit(vec![], "- مرحبا\r\n- میٹنگ 🙂\n"))
        .unwrap();
    let id = ack.revision_id().to_owned();
    assert!(ack.changed);
    assert_eq!(store.heads("p-one").unwrap(), vec![id.clone()]);
    assert_eq!(store.outbox().unwrap(), vec![id.clone()]);
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    assert_eq!(
        store
            .get(&id)
            .unwrap()
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("- مرحبا\r\n- میٹنگ 🙂\n")
    );
    assert_eq!(store.heads("p-one").unwrap(), vec![id.clone()]);
    assert_eq!(store.outbox().unwrap(), vec![id]);
}

#[test]
fn idle_stale_buffers_do_not_reinstate_old_text_and_stale_edits_merge() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let first = store.save(&edit(vec![], "one")).unwrap();
    let second = store
        .save(&edit(vec![first.revision_id().into()], "two"))
        .unwrap();
    let idle = store
        .save(&edit(vec![first.revision_id().into()], "one"))
        .unwrap();
    assert!(!idle.changed);
    assert_eq!(idle.heads, vec![second.revision_id()]);
    let concurrent = store
        .save(&edit(vec![first.revision_id().into()], "peer edit"))
        .unwrap();
    assert!(!concurrent.conflict);
    let merged = concurrent.merged.as_ref().expect("a stale save merges");
    let merge = merged.root();
    let mut parents = [second.revision_id(), concurrent.revision_id()];
    parents.sort();
    assert_eq!(
        merge
            .field("parents")
            .elements()
            .unwrap()
            .map(|id| id.as_str().unwrap())
            .collect::<Vec<_>>(),
        parents
    );
    // Both changed the only line, so both versions stay, ordered by creation then ID.
    assert_eq!(concurrent.overlaps, 1);
    let expected = if parents[0] == second.revision_id() {
        "two\npeer edit"
    } else {
        "peer edit\ntwo"
    };
    assert_eq!(merge.field("text").as_str(), Some(expected));
    assert_eq!(concurrent.heads, [merge.field("id").as_str().unwrap()]);
    assert_eq!(
        store.outbox().unwrap(),
        [
            first.revision_id(),
            second.revision_id(),
            concurrent.revision_id(),
            merge.field("id").as_str().unwrap()
        ]
    );
}

#[test]
fn only_certain_unpublished_current_heads_are_coalesced() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let first = store.save(&edit(vec![], "one")).unwrap();
    let second = store
        .save(&edit(vec![first.revision_id().into()], "two"))
        .unwrap();
    assert!(second.replaced.is_none());
    store.mark_reconciled(None).unwrap();
    let third = store
        .save(&edit(vec![second.revision_id().into()], "three"))
        .unwrap();
    assert_eq!(third.replaced.as_deref(), Some(second.revision_id()));
    assert!(store.get(second.revision_id()).unwrap().is_none());
    assert!(store.get(first.revision_id()).unwrap().is_some());
    assert_eq!(
        store.outbox().unwrap(),
        vec![first.revision_id(), third.revision_id()]
    );
    assert_eq!(
        third.revision.root().field("generation").as_f64(),
        Some(1.0)
    );
}

#[test]
fn first_versions_and_merges_are_never_coalesced() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    store.mark_reconciled(None).unwrap();
    let first = store.save(&edit(vec![], "one")).unwrap();
    let second = store
        .save(&edit(vec![first.revision_id().into()], "two"))
        .unwrap();
    assert!(second.replaced.is_none(), "a page's first version stays");
    assert!(store.get(first.revision_id()).unwrap().is_some());
    let third = store
        .save(&edit(vec![second.revision_id().into()], "three"))
        .unwrap();
    assert_eq!(third.replaced.as_deref(), Some(second.revision_id()));

    let other = store.save(&edit(vec![], "unrelated root")).unwrap();
    let merge = other
        .merged
        .as_ref()
        .expect("a second first version merges");
    let merge_id = merge.root().field("id").as_str().unwrap();
    assert_eq!(merge.root().field("parents").elements().unwrap().count(), 2);
    let after = store
        .save(&edit(vec![merge_id.into()], "after merge"))
        .unwrap();
    assert!(after.replaced.is_none(), "a merge revision stays");
    assert!(store.get(merge_id).unwrap().is_some());
    assert_eq!(
        store.outbox().unwrap(),
        vec![
            first.revision_id(),
            third.revision_id(),
            other.revision_id(),
            merge_id,
            after.revision_id()
        ]
    );
}

#[test]
fn multi_page_save_rolls_back_every_page_when_one_parent_is_missing() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let first = edit(vec![], "first page");
    let mut second = edit(vec!["a".repeat(64)], "second page");
    second.page = "p-two".into();
    assert!(store.save_many(&[first, second]).is_err());
    assert!(store.heads("p-one").unwrap().is_empty());
    assert!(store.outbox().unwrap().is_empty());
}

#[test]
fn checked_multi_page_saves_write_nothing_when_any_page_moved() {
    use quire_store::notebook::CheckedSaves;
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let one = store
        .save(&edit(vec![], "one"))
        .unwrap()
        .revision_id()
        .to_owned();
    let mut other = edit(vec![], "two");
    other.page = "p-two".into();
    other.path = "B.md".into();
    let two = store.save(&other).unwrap().revision_id().to_owned();
    let moved = store
        .save(&edit(vec![one.clone()], "one, moved"))
        .unwrap()
        .revision_id()
        .to_owned();
    let outbox = store.outbox().unwrap().len();

    let mut second = edit(vec![two.clone()], "two, retagged");
    second.page = "p-two".into();
    second.path = "B.md".into();
    let stale = [second.clone(), edit(vec![one], "one, retagged")];
    match store.save_many_if_heads(&stale).unwrap() {
        CheckedSaves::HeadsChanged { page, heads } => {
            assert_eq!((page.as_str(), heads), ("p-one", vec![moved.clone()]));
        }
        CheckedSaves::Saved(_) => panic!("a moved page must refuse the whole save"),
    }
    assert_eq!(store.heads("p-two").unwrap(), [two]);
    assert_eq!(store.outbox().unwrap().len(), outbox);

    let current = [second, edit(vec![moved], "one, retagged")];
    let CheckedSaves::Saved(acks) = store.save_many_if_heads(&current).unwrap() else {
        panic!("current heads must save");
    };
    assert_eq!(acks.len(), 2);
    assert!(acks.iter().all(|ack| ack.changed && !ack.conflict));
}

fn draft(version: u64, base: Vec<String>, text: &str) -> quire_store::notebook::Draft {
    quire_store::notebook::Draft {
        page: "p-one".into(),
        path: "A.md".into(),
        text: text.into(),
        base,
        created: 1,
        stamp: quire_store::notebook::DraftStamp {
            session: "editor-one".into(),
            version,
        },
    }
}

#[test]
fn recovery_is_durable_and_only_the_matching_snapshot_is_cleared() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    let first = draft(1, vec![], "one");
    store.put_draft(&first, None).unwrap();
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    assert_eq!(store.draft("p-one").unwrap().unwrap(), first);
    let second = draft(2, vec![], "two");
    store.put_draft(&second, Some(&first.stamp)).unwrap();
    let mut old_save = edit(vec![], "one");
    old_save.recovery = Some(first.stamp);
    let saved = store.save(&old_save).unwrap();
    let pending = store.draft("p-one").unwrap().unwrap();
    assert_eq!(pending.text, "two");
    assert_eq!(pending.base, vec![saved.revision_id()]);
    let mut new_save = edit(vec![], "two");
    new_save.recovery = Some(second.stamp);
    let saved = store.save(&new_save).unwrap();
    assert!(!saved.conflict);
    assert!(store.draft("p-one").unwrap().is_none());
}

#[test]
fn stale_recovery_and_save_snapshots_cannot_replace_later_work() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let first = draft(1, vec![], "one");
    let second = draft(2, vec![], "two");
    store.put_draft(&first, None).unwrap();
    store.put_draft(&second, Some(&first.stamp)).unwrap();
    assert!(store.put_draft(&first, Some(&second.stamp)).is_err());
    let mut save = edit(vec![], "two");
    save.recovery = Some(second.stamp);
    let saved = store.save(&save).unwrap();
    save.text = "one".into();
    save.recovery = Some(first.stamp);
    assert!(store.save(&save).is_err());
    assert_eq!(store.heads("p-one").unwrap(), vec![saved.revision_id()]);
}

#[test]
fn unchanged_content_cannot_acknowledge_a_parent_from_another_page() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let saved = store.save(&edit(vec![], "one")).unwrap();
    let mut wrong = edit(vec![saved.revision_id().into()], "one");
    wrong.page = "p-two".into();
    assert!(store.save(&wrong).is_err());
    assert!(store.heads("p-two").unwrap().is_empty());
}

#[test]
fn delayed_recovery_does_not_resurrect_snapshots_already_saved() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let saved_draft = draft(2, vec![], "two");
    store.put_draft(&saved_draft, None).unwrap();
    let mut save = edit(vec![], "two");
    save.recovery = Some(saved_draft.stamp);
    let saved = store.save(&save).unwrap();
    assert!(store.put_draft(&draft(1, vec![], "one"), None).is_err());
    let mut next = draft(1, vec![saved.revision_id().into()], "new session");
    next.stamp.session = "editor-two".into();
    store.put_draft(&next, None).unwrap();
    let mut save = edit(next.base, &next.text);
    save.recovery = Some(next.stamp);
    store.save(&save).unwrap();
    assert!(
        store
            .put_draft(&draft(1, vec![], "late old session"), None)
            .is_err()
    );
    assert!(store.draft("p-one").unwrap().is_none());
}

#[test]
fn saving_a_newer_snapshot_consumes_older_recovery_in_the_same_session() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    store.put_draft(&draft(1, vec![], "one"), None).unwrap();
    let mut save = edit(vec![], "two");
    save.recovery = Some(draft(2, vec![], "two").stamp);
    store.save(&save).unwrap();
    assert!(store.draft("p-one").unwrap().is_none());
}

#[test]
fn selecting_a_publication_freezes_its_revisions_during_later_saves() {
    use quire_store::sync::format::{Expected, Kind, decode};
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    store.mark_reconciled(None).unwrap();
    let first = store.save(&edit(vec![], "one")).unwrap();
    let selected = store.prepare_publication(100).unwrap().unwrap();
    let second = store
        .save(&edit(vec![first.revision_id().into()], "two"))
        .unwrap();
    assert!(second.replaced.is_none());
    assert!(store.get(first.revision_id()).unwrap().is_some());
    assert_eq!(store.pending_publication().unwrap().unwrap(), selected);
    let batch = decode(
        &selected.name,
        &selected.data,
        Kind::Batch,
        Expected {
            notebook: "nb",
            device: "office",
        },
    )
    .unwrap();
    assert_eq!(
        batch
            .value
            .root()
            .field("revisions")
            .elements()
            .unwrap()
            .next()
            .unwrap()
            .field("text")
            .as_str(),
        Some("one")
    );
    store.complete_publication(&selected).unwrap();
    assert_eq!(store.outbox().unwrap(), vec![second.revision_id()]);
    assert_eq!(store.cursor("office").unwrap().unwrap().hash, selected.hash);
    let third = store
        .save(&edit(vec![second.revision_id().into()], "three"))
        .unwrap();
    assert_eq!(third.replaced.as_deref(), Some(second.revision_id()));
    assert_eq!(store.outbox().unwrap(), vec![third.revision_id()]);
}

#[test]
fn uncertain_publication_snapshots_survive_reopen_and_reject_stale_completion() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    store.save(&edit(vec![], "one")).unwrap();
    assert!(store.prepare_publication(100).is_err());
    store.mark_reconciled(None).unwrap();
    let selected = store.prepare_publication(100).unwrap().unwrap();
    store.publication_uncertain(&selected).unwrap();
    drop(store);
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    assert_eq!(store.prepare_publication(200).unwrap().unwrap(), selected);
    assert!(store.mark_reconciled(None).is_err());
    let mut wrong = selected.clone();
    wrong.hash = "a".repeat(64);
    assert!(store.complete_publication(&wrong).is_err());
    assert!(store.cursor("office").unwrap().is_none());
    assert_eq!(store.outbox().unwrap(), selected.revisions);
    store.complete_publication(&selected).unwrap();
    assert!(store.outbox().unwrap().is_empty());
}

#[test]
fn recovery_can_follow_a_save_that_consumed_its_expected_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let first = draft(1, vec![], "one");
    store.put_draft(&first, None).unwrap();
    let mut save = edit(vec![], "one");
    save.recovery = Some(first.stamp.clone());
    let saved = store.save(&save).unwrap();
    let second = draft(2, vec![], "two");
    store.put_draft(&second, Some(&first.stamp)).unwrap();
    assert_eq!(
        store.draft("p-one").unwrap().unwrap().base,
        vec![saved.revision_id()]
    );
    store.put_draft(&second, Some(&first.stamp)).unwrap();
}

#[test]
fn failure_while_writing_outbox_rolls_back_coalescing_and_recovery() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    store.mark_reconciled(None).unwrap();
    let first = store.save(&edit(vec![], "one")).unwrap();
    let pending = draft(1, vec![first.revision_id().into()], "two");
    store.put_draft(&pending, None).unwrap();
    let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    connection.exec("CREATE TRIGGER fail_outbox BEFORE INSERT ON outbox BEGIN SELECT RAISE(ABORT,'injected outbox failure'); END").unwrap()().unwrap();
    let mut save = edit(pending.base.clone(), &pending.text);
    save.recovery = Some(pending.stamp.clone());
    assert!(store.save(&save).is_err());
    assert_eq!(store.heads("p-one").unwrap(), vec![first.revision_id()]);
    assert_eq!(store.outbox().unwrap(), vec![first.revision_id()]);
    assert!(store.get(first.revision_id()).unwrap().is_some());
    assert_eq!(store.draft("p-one").unwrap().unwrap(), pending);
    connection.exec("DROP TRIGGER fail_outbox").unwrap()().unwrap();
    assert!(store.save(&save).is_ok());
}

#[test]
fn native_saved_revision_matches_the_frozen_typescript_identity() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let vectors: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../tests/fixtures/protocol/vectors.json"
    )))
    .unwrap();
    let case = &vectors["revisions"][0];
    let fields = &case["fields"];
    let mut input = edit(vec![], fields["text"].as_str().unwrap());
    input.page = fields["page"].as_str().unwrap().into();
    input.path = fields["path"].as_str().unwrap().into();
    let saved = store.save(&input).unwrap();
    assert_eq!(saved.revision_id(), case["id"]);
}

#[test]
fn damaged_revision_bodies_fail_without_replacing_the_saved_head() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "office").unwrap();
    let first = store.save(&edit(vec![], "one")).unwrap();
    let connection = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    connection
        .exec("UPDATE revisions SET body_hash='invalid'")
        .unwrap()()
    .unwrap();
    assert!(store.get(first.revision_id()).is_err());
    assert!(
        store
            .save(&edit(vec![first.revision_id().into()], "two"))
            .is_err()
    );
    assert_eq!(store.heads("p-one").unwrap(), vec![first.revision_id()]);
    assert_eq!(store.outbox().unwrap(), vec![first.revision_id()]);
}

#[test]
fn repeated_save_after_acknowledgment_keeps_the_same_editor_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office").unwrap();
    let mut input = edit(vec![], "one");
    input.recovery = Some(draft(1, vec![], "one").stamp);
    let saved = store.save(&input).unwrap();
    let original = input.clone();
    input.base = vec![saved.revision_id().into()];
    let repeated = store.save(&input).unwrap();
    assert!(!repeated.changed);
    assert_eq!(repeated.revision_id(), saved.revision_id());
    assert!(!store.save(&original).unwrap().changed);
    input.text = "different contents for the same editor version".into();
    assert!(store.save(&input).is_err());
    assert_eq!(store.outbox().unwrap(), vec![saved.revision_id()]);
}
