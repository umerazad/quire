use anyhow::Result;
use quire_store::{
    notebook::{BatchImport, Edit, NotebookStore},
    sync::format::{self, Kind},
};
use serde_json::{Value, json};

fn revision(page: &str, text: &str, device: &str, parents: &[&Value]) -> Value {
    let mut value = json!({"page":page,"text":text,"path":"A.md","deleted":false,"device":device,"created":1,
        "parents":parents.iter().map(|parent| parent["id"].as_str().unwrap()).collect::<Vec<_>>(),
        "generation":parents.iter().map(|parent| parent["generation"].as_u64().unwrap() + 1).max().unwrap_or(0)});
    value["id"] = format::revision_id(&value).unwrap().into();
    value
}

fn batch(device: &str, seq: u32, previous: Option<&str>, revisions: &[Value]) -> format::Encoded {
    format::encode(&json!({"notebook":"nb","device":device,"seq":seq,"previous":previous,"created":1,"revisions":revisions,"attachments":[]}), Kind::Batch).unwrap()
}

#[test]
fn importing_remote_branches_retains_conflicts_until_an_explicit_merge() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let base = revision("p", "مرحبا 🙂", "office", &[]);
    let left = revision("p", "office", "office", &[&base]);
    let right = revision("p", "home", "home", &[&base]);
    let a = batch("office", 1, None, &[base.clone(), left.clone()]);
    let b = batch("home", 1, None, std::slice::from_ref(&right));
    let BatchImport::Applied(first) = store.import_batch("office", &a.name, &a.data)? else {
        panic!("first batch must apply");
    };
    assert_eq!(first.applied, 2);
    assert!(matches!(
        store.import_batch("home", &b.name, &b.data)?,
        BatchImport::Applied(_)
    ));
    let heads = store.heads("p")?;
    assert_eq!(heads.len(), 2);
    assert!(store.outbox()?.is_empty());
    assert_eq!(store.cursor("office")?.unwrap().hash, a.hash);
    let merged = store.save(&Edit {
        page: "p".into(),
        base: heads,
        path: "A.md".into(),
        text: "merged".into(),
        deleted: false,
        created: 2,
        recovery: None,
    })?;
    assert!(!merged.conflict);
    assert_eq!(store.heads("p")?, [merged.revision_id()]);
    assert_eq!(store.outbox()?, [merged.revision_id()]);
    Ok(())
}

#[test]
fn missing_parents_gaps_and_duplicates_do_not_partially_advance_the_graph() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let first = revision("p", "one", "office", &[]);
    let second = revision("p", "two", "office", &[&first]);
    let one = batch("office", 1, None, std::slice::from_ref(&first));
    let two = batch("office", 2, Some(&one.hash), std::slice::from_ref(&second));
    assert!(matches!(
        store.import_batch("office", &two.name, &two.data)?,
        BatchImport::WaitingForSequence { expected: 1 }
    ));
    let missing = batch(
        "other",
        1,
        None,
        &[revision("p", "child", "other", &[&second])],
    );
    assert!(matches!(
        store.import_batch("other", &missing.name, &missing.data)?,
        BatchImport::MissingParents { .. }
    ));
    assert!(store.cursor("office")?.is_none());
    assert!(store.heads("p")?.is_empty());
    assert!(matches!(
        store.import_batch("office", &one.name, &one.data)?,
        BatchImport::Applied(_)
    ));
    assert!(matches!(
        store.import_batch("office", &one.name, &one.data)?,
        BatchImport::Duplicate
    ));
    assert!(matches!(
        store.import_batch("office", &two.name, &two.data)?,
        BatchImport::Applied(_)
    ));
    assert!(matches!(
        store.import_batch("other", &missing.name, &missing.data)?,
        BatchImport::Applied(_)
    ));
    Ok(())
}

#[test]
fn malformed_batches_and_invalid_graphs_leave_no_cursor_or_revisions() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let parent = revision("p", "one", "office", &[]);
    let invalid = revision("other-page", "bad", "office", &[&parent]);
    let encoded = batch("office", 1, None, &[parent.clone(), invalid]);
    assert!(matches!(
        store.import_batch("office", &encoded.name, b"partial")?,
        BatchImport::Rejected { .. }
    ));
    assert!(matches!(
        store.import_batch("office", &encoded.name, &encoded.data)?,
        BatchImport::Rejected { .. }
    ));
    assert!(store.get(parent["id"].as_str().unwrap())?.is_none());
    assert!(store.cursor("office")?.is_none());
    assert!(store.outbox()?.is_empty());
    Ok(())
}

#[test]
fn stale_unchanged_local_save_and_recovery_survive_a_peer_update() -> Result<()> {
    use quire_store::notebook::{Draft, DraftStamp};
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let base = revision("p", "one", "office", &[]);
    let remote = revision("p", "peer update", "office", &[&base]);
    let first = batch("office", 1, None, std::slice::from_ref(&base));
    store.import_batch("office", &first.name, &first.data)?;
    let draft = Draft {
        page: "p".into(),
        path: "A.md".into(),
        text: "unsaved local".into(),
        base: vec![base["id"].as_str().unwrap().into()],
        created: 2,
        stamp: DraftStamp {
            session: "editor".into(),
            version: 1,
        },
    };
    store.put_draft(&draft, None)?;
    let second = batch(
        "office",
        2,
        Some(&first.hash),
        std::slice::from_ref(&remote),
    );
    store.import_batch("office", &second.name, &second.data)?;
    let stale = store.save(&Edit {
        page: "p".into(),
        base: draft.base.clone(),
        path: "A.md".into(),
        text: "one".into(),
        created: 3,
        deleted: false,
        recovery: None,
    })?;
    assert!(!stale.changed);
    assert_eq!(store.heads("p")?, [remote["id"].as_str().unwrap()]);
    assert_eq!(store.draft("p")?, Some(draft));
    assert!(store.outbox()?.is_empty());
    Ok(())
}

#[test]
fn own_pending_batch_is_left_for_durable_publication_and_unrelated_saves_are_retained() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let edit = Edit {
        page: "p".into(),
        base: vec![],
        path: "A.md".into(),
        text: "one".into(),
        created: 1,
        deleted: false,
        recovery: None,
    };
    let one = store.save(&edit)?;
    store.mark_reconciled(None)?;
    let pending = store.prepare_publication(100)?.unwrap();
    let two = store.save(&Edit {
        base: vec![one.revision_id().into()],
        text: "two".into(),
        ..edit
    })?;
    assert_eq!(
        store.import_batch("local", &pending.name, &pending.data)?,
        BatchImport::OwnPublicationPending
    );
    assert_eq!(store.pending_publication()?, Some(pending));
    assert!(store.cursor("local")?.is_none());
    assert_eq!(store.outbox()?, [one.revision_id(), two.revision_id()]);
    let foreign = batch(
        "local",
        1,
        None,
        &[revision("p", "other computer", "local", &[])],
    );
    assert!(matches!(
        store.import_batch("local", &foreign.name, &foreign.data)?,
        BatchImport::Rejected { .. }
    ));
    Ok(())
}

#[test]
fn rebuilding_own_log_requires_no_conflicting_local_outbox_and_does_not_acknowledge_a_folder()
-> Result<()> {
    let root = tempfile::tempdir()?;
    let incoming = batch(
        "local",
        1,
        None,
        &[revision("p", "prior work", "local", &[])],
    );
    let store = NotebookStore::open(&root.path().join("empty.sqlite"), "nb", "local")?;
    assert!(matches!(
        store.import_batch("local", &incoming.name, &incoming.data)?,
        BatchImport::Applied(_)
    ));
    assert!(store.outbox()?.is_empty());
    assert!(
        store.prepare_publication(100).is_err(),
        "memory import must not bypass folder reconciliation"
    );
    let copy = NotebookStore::open(&root.path().join("copy.sqlite"), "nb", "local")?;
    let saved = copy.save(&Edit {
        page: "p-local".into(),
        base: vec![],
        path: "B.md".into(),
        text: "local work".into(),
        created: 1,
        deleted: false,
        recovery: None,
    })?;
    assert!(matches!(
        copy.import_batch("local", &incoming.name, &incoming.data)?,
        BatchImport::Rejected { .. }
    ));
    assert!(copy.cursor("local")?.is_none());
    assert_eq!(copy.outbox()?, [saved.revision_id()]);
    Ok(())
}

#[test]
fn conflicting_sequence_or_previous_hash_cannot_advance_the_peer_cursor() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let first = batch("office", 1, None, &[revision("p", "one", "office", &[])]);
    store.import_batch("office", &first.name, &first.data)?;
    for fork in [
        batch("office", 1, None, &[revision("p", "fork", "office", &[])]),
        batch("office", 2, Some(&"a".repeat(64)), &[]),
    ] {
        assert!(matches!(
            store.import_batch("office", &fork.name, &fork.data)?,
            BatchImport::Rejected { .. }
        ));
        assert_eq!(store.cursor("office")?.unwrap().hash, first.hash);
    }
    Ok(())
}

#[test]
fn import_preserves_lone_surrogates_and_unknown_fields_across_reopen() -> Result<()> {
    use quire_store::sync::json::Json;
    let fields = Json::parse(br#"{"page":"p-\ud800","path":"A\udfff.md","text":"raw \ud800","parents":[],"generation":0,"deleted":false,"created":1,"device":"office","future":{"s":"\udfff"}}"#)?;
    let id = format::revision_id(&fields)?;
    let raw = fields.canonical()?;
    let record = format!("{{\"id\":\"{id}\",{}", &raw[1..]);
    let value = Json::parse(format!("{{\"notebook\":\"nb\",\"device\":\"office\",\"seq\":1,\"previous\":null,\"created\":1,\"revisions\":[{record}],\"attachments\":[]}}").as_bytes())?;
    let encoded = format::encode(&value, Kind::Batch)?;
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let BatchImport::Applied(result) =
        store.import_batch("office", &encoded.name, &encoded.data)?
    else {
        panic!("lossless revision must import");
    };
    assert_eq!(result.applied, 1);
    assert!(result.pages[0].page.as_str().is_none());
    assert_eq!(result.pages[0].heads, std::slice::from_ref(&id));
    drop(store);
    let store = NotebookStore::open(&path, "nb", "local")?;
    assert_eq!(
        store.get(&id)?.unwrap().canonical()?,
        Json::parse(record.as_bytes())?.canonical()?
    );
    Ok(())
}

#[test]
fn absent_attachments_are_reported_without_discarding_the_revision() -> Result<()> {
    let name = format::attachment_file_name(b"late image", "a.png");
    let record = revision("p", &format!("![](attachments/{name})"), "office", &[]);
    let encoded = format::encode(
        &json!({"notebook":"nb","device":"office","seq":1,"previous":null,"created":1,"revisions":[record],"attachments":[name]}),
        Kind::Batch,
    )?;
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let BatchImport::Applied(result) =
        store.import_batch("office", &encoded.name, &encoded.data)?
    else {
        panic!("missing attachment must not block the graph");
    };
    assert_eq!(result.attachments, [name]);
    assert_eq!(store.heads("p")?.len(), 1);
    assert!(!root.path().join("attachments").exists());
    Ok(())
}

#[test]
fn worker_imports_before_a_queued_head_query() -> Result<()> {
    use futures::executor::block_on;
    use quire_store::worker::{QueueLimits, StorageWorker};
    let root = tempfile::tempdir()?;
    let incoming = batch("office", 1, None, &[revision("p", "one", "office", &[])]);
    let worker = StorageWorker::spawn(
        root.path().join("local.sqlite"),
        "nb".into(),
        "local".into(),
        QueueLimits::default(),
    )?;
    block_on(worker.ready)?;
    let imported = worker
        .client
        .import_batch("office".into(), incoming.name, incoming.data)?;
    let heads = worker.client.heads("p".into())?;
    let requests = worker.client.pending_attachments(None, 100)?;
    let history = worker.client.batch_hash("office".into(), 1)?;
    assert!(matches!(block_on(imported)?, BatchImport::Applied(_)));
    assert_eq!(block_on(heads)?.len(), 1);
    assert!(block_on(requests)?.is_empty());
    assert_eq!(block_on(history)?, Some(incoming.hash));
    worker.client.close()?;
    block_on(worker.stopped)?;
    Ok(())
}

#[test]
fn newer_format_is_reported_without_advancing_any_local_state() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let value = json!({"format":2,"notebook":"nb","device":"office","seq":1,"previous":null,"created":1,"revisions":[],"attachments":[]});
    let raw = format::canonical(&value)?;
    let name = format!("000000001-{}.qlog.zst", format::sha256(&raw));
    let data = zstd::stream::encode_all(raw.as_bytes(), 0)?;
    assert!(matches!(
        store.import_batch("office", &name, &data)?,
        BatchImport::Rejected {
            newer_format: Some(2.),
            ..
        }
    ));
    assert!(store.cursor("office")?.is_none());
    assert!(store.outbox()?.is_empty());
    Ok(())
}

#[test]
fn pending_attachment_requests_survive_import_acknowledgment_and_reopen() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let name = format::attachment_file_name(b"late", "image.png");
    let encoded = format::encode(
        &json!({"notebook":"nb","device":"office","seq":1,"previous":null,"created":1,"revisions":[],"attachments":[name]}),
        Kind::Batch,
    )?;
    let store = NotebookStore::open(&path, "nb", "local")?;
    assert!(matches!(
        store.import_batch("office", &encoded.name, &encoded.data)?,
        BatchImport::Applied(_)
    ));
    assert_eq!(
        store.pending_attachments(None, 100)?,
        std::slice::from_ref(&name)
    );
    assert_eq!(store.batch_hash("office", 1)?, Some(encoded.hash));
    drop(store);
    let store = NotebookStore::open(&path, "nb", "local")?;
    assert_eq!(
        store.pending_attachments(None, 100)?,
        std::slice::from_ref(&name)
    );
    store.cache_attachment(&name, b"late")?;
    assert!(store.pending_attachments(None, 100)?.is_empty());
    Ok(())
}

#[test]
fn known_historical_batch_hash_rejects_a_single_old_fork_after_later_imports() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let one = batch("office", 1, None, &[]);
    let two = batch("office", 2, Some(&one.hash), &[]);
    let store = NotebookStore::open(&path, "nb", "local")?;
    store.import_batch("office", &one.name, &one.data)?;
    store.import_batch("office", &two.name, &two.data)?;
    drop(store);
    let store = NotebookStore::open(&path, "nb", "local")?;
    let fork = batch("office", 1, None, &[revision("p", "fork", "office", &[])]);
    assert!(matches!(
        store.import_batch("office", &fork.name, &fork.data)?,
        BatchImport::Rejected { .. }
    ));
    assert_eq!(store.cursor("office")?.unwrap().hash, two.hash);
    assert_eq!(store.batch_hash("office", 1)?, Some(one.hash));
    assert!(store.heads("p")?.is_empty());
    Ok(())
}

#[test]
fn a_duplicate_revision_from_a_peer_cannot_consume_the_local_frozen_outbox() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let saved = store.save(&Edit {
        page: "p".into(),
        base: vec![],
        path: "A.md".into(),
        text: "same content".into(),
        created: 1,
        deleted: false,
        recovery: None,
    })?;
    store.mark_reconciled(None)?;
    let pending = store.prepare_publication(100)?.unwrap();
    let mut peer: Value = serde_json::from_str(&saved.revision.canonical()?)?;
    peer["device"] = "office".into();
    let incoming = batch("office", 1, None, &[peer]);
    let BatchImport::Applied(result) =
        store.import_batch("office", &incoming.name, &incoming.data)?
    else {
        panic!("the peer's cursor must advance for a duplicate revision");
    };
    assert_eq!(result.applied, 0);
    assert!(result.pages.is_empty());
    assert_eq!(store.outbox()?, [saved.revision_id()]);
    assert_eq!(store.pending_publication()?, Some(pending));
    assert_eq!(
        store
            .get(saved.revision_id())?
            .unwrap()
            .root()
            .field("device")
            .as_str(),
        Some("local")
    );
    Ok(())
}

#[test]
fn a_peer_observed_revision_cannot_be_coalesced_out_of_the_local_graph() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    store.mark_reconciled(None)?;
    let first = store.save(&Edit {
        page: "p".into(),
        base: vec![],
        path: "A.md".into(),
        text: "same content".into(),
        created: 1,
        deleted: false,
        recovery: None,
    })?;
    let mut observed: Value = serde_json::from_str(&first.revision.canonical()?)?;
    observed["device"] = "peer".into();
    let incoming = batch("peer", 1, None, std::slice::from_ref(&observed));
    assert!(matches!(
        store.import_batch("peer", &incoming.name, &incoming.data)?,
        BatchImport::Applied(_)
    ));
    assert_eq!(store.outbox()?, [first.revision_id()]);
    assert_eq!(
        store.get(first.revision_id())?.unwrap().canonical()?,
        first.revision.canonical()?
    );
    drop(store);
    let store = NotebookStore::open(&path, "nb", "local")?;
    store.mark_reconciled(None)?;
    let second = store.save(&Edit {
        page: "p".into(),
        base: vec![first.revision_id().into()],
        path: "A.md".into(),
        text: "new local content".into(),
        created: 2,
        deleted: false,
        recovery: None,
    })?;
    assert!(
        second.replaced.is_none(),
        "a revision already observed in a peer batch must remain available to later descendants"
    );
    assert!(store.get(first.revision_id())?.is_some());
    assert_eq!(store.outbox()?, [first.revision_id(), second.revision_id()]);
    let descendant = revision("p", "later peer edit", "peer", &[&observed]);
    let next = batch(
        "peer",
        2,
        Some(&incoming.hash),
        std::slice::from_ref(&descendant),
    );
    assert!(matches!(
        store.import_batch("peer", &next.name, &next.data)?,
        BatchImport::Applied(_)
    ));
    let mut heads = store.heads("p")?;
    heads.sort();
    let mut expected = vec![
        second.revision_id().to_owned(),
        descendant["id"].as_str().unwrap().to_owned(),
    ];
    expected.sort();
    assert_eq!(heads, expected);
    Ok(())
}

#[test]
fn seeded_native_import_schedules_match_the_frozen_graph_and_chain_oracle() -> Result<()> {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let mut seed = 0x173b_9a24_a33e_1881u64;
    let mut random = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    let mut graphs = Vec::new();
    for case in 0..64 {
        let page = format!("p-{case}-مرحبا🙂");
        let mut records = Vec::new();
        for index in 0..9 {
            let mut parents = Vec::new();
            if index > 0 {
                parents.push(&records[random() % index]);
                if index > 1 && random() % 2 == 0 {
                    let other = &records[random() % index];
                    if !parents.contains(&other) {
                        parents.push(other);
                    }
                }
            }
            let mut record = revision(
                &page,
                &format!("edit {index}"),
                &format!("peer-{case}-{index}"),
                &parents,
            );
            if index == 8 {
                record["deleted"] = true.into();
                record["text"] = "".into();
                record["id"] = format::revision_id(&record)?.into();
            }
            records.push(record);
        }
        let mut bad_generation =
            revision(&page, "bad generation", &format!("peer-{case}-bad"), &[]);
        bad_generation["generation"] = 100.into();
        bad_generation["id"] = format::revision_id(&bad_generation)?.into();
        records.push(bad_generation);
        records.push(revision(
            &format!("other-{case}"),
            "cross-page parent",
            &format!("peer-{case}-cross"),
            &[&records[0]],
        ));
        let mut schedule: Vec<_> = (0..records.len()).collect();
        for index in (1..schedule.len()).rev() {
            schedule.swap(index, random() % (index + 1));
        }
        schedule.extend([random() % records.len(), random() % records.len()]);
        schedule.extend(0..records.len());
        let mut operations = Vec::new();
        for index in schedule {
            let record = &records[index];
            let device = record["device"].as_str().unwrap();
            let incoming = batch(device, 1, None, std::slice::from_ref(record));
            let outcome = store.import_batch(device, &incoming.name, &incoming.data)?;
            let kind = match outcome {
                BatchImport::Applied(result) if result.applied > 0 => "added",
                BatchImport::Applied(_) | BatchImport::Duplicate => "duplicate",
                BatchImport::MissingParents { .. } => "missing",
                BatchImport::Rejected { .. } => "invalid",
                other => panic!("unexpected graph result: {other:?}"),
            };
            operations.push(json!({"revision":record,"kind":kind,"heads":store.heads(&page)?}));
        }
        graphs.push(json!({"page":page,"operations":operations}));
    }
    let one = batch("chain", 1, None, &[]);
    let two = batch("chain", 2, Some(&one.hash), &[]);
    let fork_one = format::encode(
        &json!({"notebook":"nb","device":"chain","seq":1,"previous":null,"created":2,"revisions":[],"attachments":[]}),
        Kind::Batch,
    )?;
    let mut chains = Vec::new();
    let mut known = std::collections::BTreeMap::<u32, String>::new();
    let gap = batch("chain", 3, Some(&two.hash), &[]);
    let wrong_previous = batch("chain", 2, Some(&"a".repeat(64)), &[]);
    let fork_two = batch(
        "chain",
        2,
        Some(&one.hash),
        &[revision("p-chain", "fork", "chain", &[])],
    );
    for incoming in [
        &one,
        &gap,
        &fork_one,
        &wrong_previous,
        &two,
        &fork_one,
        &two,
        &fork_two,
    ] {
        let before = store.cursor("chain")?;
        let decoded = format::decode(
            &incoming.name,
            &incoming.data,
            Kind::Batch,
            format::Expected {
                notebook: "nb",
                device: "chain",
            },
        )?;
        let kind = match store.import_batch("chain", &incoming.name, &incoming.data)? {
            BatchImport::Applied(_) => "apply",
            BatchImport::Duplicate => "applied",
            BatchImport::WaitingForSequence { .. } => "gap",
            BatchImport::Rejected { .. } => "fork",
            other => panic!("unexpected chain result: {other:?}"),
        };
        chains.push(json!({"cursor":before,"known":known,"batch":{"seq":decoded.value.root().field("seq").as_f64(),"previous":decoded.value.root().field("previous").as_str()},"hash":incoming.hash,"kind":kind}));
        if kind == "apply" {
            known.insert(
                decoded.value.root().field("seq").as_f64().unwrap() as u32,
                incoming.hash.clone(),
            );
        }
    }
    let script =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/graph-oracle.ts");
    let mut child = Command::new("bun")
        .arg(script)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    child.stdin.take().unwrap().write_all(&serde_json::to_vec(
        &json!({"graphs":graphs,"chains":chains}),
    )?)?;
    let output = child.wait_with_output()?;
    assert!(
        output.status.success(),
        "Frozen graph oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    Ok(())
}
