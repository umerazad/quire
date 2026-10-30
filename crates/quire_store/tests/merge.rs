use anyhow::Result;
use quire_store::{
    notebook::{BatchImport, Edit, NotebookStore},
    sync::{
        format::{self, Kind},
        json::Json,
    },
};
use std::collections::BTreeMap;

mod common;

const FIELDS: [&str; 7] = [
    "id",
    "page",
    "parents",
    "generation",
    "path",
    "deleted",
    "text",
];

#[test]
fn head_merges_match_the_frozen_typescript_graph() -> Result<()> {
    let vectors = Json::parse(include_bytes!("../../../tests/fixtures/merge-vectors.json"))?;
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open_with_clock(
        &root.path().join("local.sqlite"),
        "nb",
        "local",
        Box::new(|| 5000),
    )?;
    let mut chains = BTreeMap::<String, (u32, Option<String>)>::new();
    let mut expected = BTreeMap::new();
    for case in vectors.root().field("heads").elements().unwrap() {
        // Batches hold only their writer's revisions, so each revision travels on its device's chain.
        for revision in case.field("revisions").elements().unwrap() {
            let device = revision.field("device").as_str().unwrap();
            let (seq, previous) = chains.entry(device.to_owned()).or_default();
            *seq += 1;
            let batch = format!(
                "{{\"notebook\":\"nb\",\"device\":{},\"seq\":{seq},\"previous\":{},\"created\":1,\"revisions\":[{}],\"attachments\":[]}}",
                serde_json::to_string(device)?,
                serde_json::to_string(previous)?,
                revision.canonical()?
            );
            let encoded = format::encode(Json::parse(batch.as_bytes())?, Kind::Batch)?;
            assert!(matches!(
                store.import_batch(device, &encoded.name, &encoded.data)?,
                BatchImport::Applied(_)
            ));
            *previous = Some(encoded.hash);
        }
        let page = case.field("expected").field("page").as_str().unwrap();
        assert!(store.heads(page)?.len() >= 2);
        expected.insert(page.to_owned(), case);
    }
    let merges = store.merge_conflicts()?;
    assert!(merges.failed.is_empty(), "{:?}", merges.failed);
    assert_eq!(merges.merged.len(), expected.len());
    for page in merges.merged {
        let case = expected[page.revision.root().field("page").as_str().unwrap()];
        for field in FIELDS {
            assert_eq!(
                page.revision.root().field(field).canonical()?,
                case.field("expected").field(field).canonical()?,
                "{field}: {}",
                case.canonical()?
            );
        }
        assert_eq!(
            page.overlaps as f64,
            case.field("overlaps").as_f64().unwrap()
        );
        let record = page.revision.root();
        assert_eq!(record.field("device").as_str(), Some("local"));
        assert_eq!(record.field("created").as_f64(), Some(5000.0));
        let id = record.field("id").as_str().unwrap();
        assert_eq!(store.heads(record.field("page").as_str().unwrap())?, [id]);
        assert!(store.outbox()?.iter().any(|pending| pending == id));
    }
    Ok(())
}

fn edit(base: Vec<String>, text: &str, deleted: bool) -> Edit {
    Edit {
        page: "p".into(),
        base,
        path: "p.md".into(),
        text: text.into(),
        deleted,
        created: 7,
        recovery: None,
    }
}

#[test]
fn an_edit_outlives_a_deletion_and_two_deletions_stay_deleted() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let base = store.save(&edit(vec![], "- one\n", false))?;
    common::import_branch(&store, "peer", "p", &[base.revision_id()], "", true)?;
    let revived = store.save(&edit(
        vec![base.revision_id().into()],
        "- one\n- two\n",
        false,
    ))?;
    let merged = revived.merged.as_ref().expect("a stale save merges");
    assert_eq!(merged.root().field("deleted").as_bool(), Some(false));
    assert_eq!(merged.root().field("text").as_str(), Some("- one\n- two\n"));
    assert_eq!(revived.overlaps, 0);

    let tombstone = |text: &str| edit(vec![], text, true);
    let mut other = tombstone("");
    other.page = "q".into();
    let first = store.save(&other)?;
    common::import_branch(&store, "home", "q", &[first.revision_id()], "", true)?;
    other.base = vec![first.revision_id().into()];
    other.text = "removed here too".into();
    let both = store.save(&other)?;
    let merged = both.merged.as_ref().expect("competing deletions merge");
    assert_eq!(merged.root().field("deleted").as_bool(), Some(true));
    Ok(())
}

#[test]
fn the_newer_of_two_moves_wins_and_overlaps_are_reported() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open_with_clock(&path, "nb", "local", Box::new(|| 9000))?;
    let base = store.save(&edit(vec![], "- call Sam ^b-1\n", false))?;
    let mut moved = edit(vec![base.revision_id().into()], "- call Alex ^b-1\n", false);
    moved.path = "Moved.md".into();
    moved.created = 5;
    store.save(&moved)?;
    let peer = Json::parse(
        format!(
            "{{\"page\":\"p\",\"parents\":[\"{}\"],\"generation\":1,\"path\":\"Elsewhere.md\",\"text\":\"- call Robin ^b-1\\n\",\"deleted\":false,\"created\":6,\"device\":\"peer\"}}",
            base.revision_id()
        )
        .as_bytes(),
    )?;
    let id = format::revision_id(&peer)?;
    let record = format!("{{\"id\":\"{id}\",{}", &peer.canonical()?[1..]);
    let batch = Json::parse(format!("{{\"notebook\":\"nb\",\"device\":\"peer\",\"seq\":1,\"previous\":null,\"created\":1,\"revisions\":[{record}],\"attachments\":[]}}").as_bytes())?;
    let encoded = format::encode(batch, Kind::Batch)?;
    store.import_batch("peer", &encoded.name, &encoded.data)?;
    drop(store);
    let store = NotebookStore::open_with_clock(&path, "nb", "local", Box::new(|| 9001))?;
    let merges = store.merge_conflicts()?;
    let [merged] = &merges.merged[..] else {
        panic!("the competing heads must merge");
    };
    let record = merged.revision.root();
    assert_eq!(record.field("path").as_str(), Some("Elsewhere.md"));
    assert_eq!(
        record.field("text").as_str(),
        Some("- call Alex ^b-1\n- call Robin\n")
    );
    assert_eq!(record.field("created").as_f64(), Some(9001.0));
    assert_eq!(merged.overlaps, 1);
    Ok(())
}

#[test]
fn a_merge_that_cannot_be_stored_rolls_back_with_its_save() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let base = store.save(&edit(vec![], "one", false))?;
    let peer = common::import_branch(&store, "peer", "p", &[base.revision_id()], "peer", false)?;
    let heads = store.heads("p")?;
    let outbox = store.outbox()?;
    let connection = sqlez::connection::Connection::try_open_file(path.to_str().unwrap())?;
    connection.exec("CREATE TRIGGER fail_merge BEFORE INSERT ON parents WHEN (SELECT COUNT(*) FROM parents WHERE child=NEW.child)>0 BEGIN SELECT RAISE(ABORT,'injected merge failure'); END")?()?;
    assert!(
        store
            .save(&edit(vec![base.revision_id().into()], "local", false))
            .is_err()
    );
    assert_eq!(store.heads("p")?, heads);
    assert_eq!(store.outbox()?, outbox);
    assert!(store.heads("p")?.contains(&peer));
    connection.exec("DROP TRIGGER fail_merge")?()?;
    let saved = store.save(&edit(vec![base.revision_id().into()], "local", false))?;
    assert!(saved.merged.is_some());
    assert_eq!(store.heads("p")?.len(), 1);
    Ok(())
}
