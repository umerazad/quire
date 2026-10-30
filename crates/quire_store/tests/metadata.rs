use anyhow::Result;
use futures::executor::block_on;
use quire_store::{
    notebook::{Edit, MetadataLimits, NotebookStore},
    sync::json::JsString,
    worker::{QueueLimits, StorageWorker},
};
use sqlez::connection::Connection;

mod common;

fn edit(page: &str, text: &str, base: Vec<String>) -> Edit {
    Edit {
        page: page.into(),
        path: format!("{page}.md"),
        text: text.into(),
        base,
        deleted: false,
        created: 1,
        recovery: None,
    }
}

#[test]
fn metadata_reads_one_selected_head_without_loading_any_revision_body() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let first = store.save(&edit("a", "---\ntitle: اردو 🙂\n---\nbody", vec![]))?;
    store.save(&edit("b", "unrelated", vec![]))?;
    let connection = Connection::try_open_file(path.to_str().unwrap())?;
    connection.exec("UPDATE revisions SET body=X'00'")?()?;
    let pages = store.page_metadata(None, MetadataLimits::default())?;
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].page, JsString::from("a"));
    assert_eq!(pages[0].revision, first.revision_id());
    assert_eq!(pages[0].title.as_str(), Some("اردو 🙂"));
    assert_eq!(pages[0].heads, 1);
    assert!(store.get(first.revision_id()).is_err());
    assert!(store.rebuild_metadata().is_err());
    assert_eq!(store.page_metadata(None, MetadataLimits::default())?, pages);
    drop(store);
    let worker = StorageWorker::spawn(path, "nb".into(), "local".into(), QueueLimits::default())?;
    block_on(worker.ready)?;
    assert_eq!(
        block_on(
            worker
                .client
                .page_metadata(None, MetadataLimits::default())?
        )?
        .len(),
        2
    );
    worker.client.close()?;
    block_on(worker.stopped)?;
    Ok(())
}

#[test]
fn indexed_titles_follow_editor_line_breaks_without_rewriting_stored_source() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let text = "---\rtitle: daily name\r---\rbody\r";
    let saved = store.save(&edit("p", text, vec![]))?;
    assert_eq!(
        store.page_metadata(None, MetadataLimits::default())?[0]
            .title
            .as_str(),
        Some("daily name")
    );
    assert_eq!(
        store
            .get(saved.revision_id())?
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some(text)
    );
    Ok(())
}

#[test]
fn malformed_numeric_head_columns_cannot_change_the_selected_version_silently() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let first = store.save(&edit("p", "first", vec![]))?;
    let connection = Connection::try_open_file(path.to_str().unwrap())?;
    connection.exec("UPDATE revisions SET generation='invalid'")?()?;
    assert!(
        store.get(first.revision_id()).is_ok(),
        "the authoritative body remains intact"
    );
    assert!(
        store
            .page_metadata(None, MetadataLimits::default())
            .is_err()
    );
    Ok(())
}

#[test]
fn metadata_tracks_coalescing_conflicts_and_tombstones() -> Result<()> {
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    store.mark_reconciled(None)?;
    let root_version = store.save(&edit("p", "---\ntitle: root\n---\n", vec![]))?;
    let first = store.save(&edit(
        "p",
        "---\ntitle: first\n---\n",
        vec![root_version.revision_id().into()],
    ))?;
    assert!(first.replaced.is_none());
    let second = store.save(&edit(
        "p",
        "---\ntitle: second\n---\n",
        vec![first.revision_id().into()],
    ))?;
    assert_eq!(second.replaced.as_deref(), Some(first.revision_id()));
    let connection = Connection::try_open_file(path.to_str().unwrap())?;
    assert_eq!(
        connection.select_row::<i64>("SELECT COUNT(*) FROM revision_metadata")?()?,
        Some(2)
    );
    assert_eq!(
        store.page_metadata(None, MetadataLimits::default())?[0]
            .title
            .as_str(),
        Some("second")
    );
    common::import_branch(&store, "peer", "p", &[], "---\ntitle: branch\n---\n", false)?;
    let heads = store.heads("p")?;
    let pages = store.page_metadata(None, MetadataLimits::default())?;
    assert_eq!(pages[0].heads, 2);
    assert_eq!(pages[0].revision, heads[0]);
    let selected = store.get(&heads[0])?.unwrap();
    assert_eq!(
        pages[0].title,
        quire_core::markdown::page_title("p.md", selected.root().field("text").as_str().unwrap())
    );
    let mut deletion = edit("p", "", heads);
    deletion.deleted = true;
    let deleted = store.save(&deletion)?;
    let pages = store.page_metadata(None, MetadataLimits::default())?;
    assert_eq!(pages[0].revision, deleted.revision_id());
    assert!(pages[0].deleted);
    assert_eq!(pages[0].heads, 1);
    Ok(())
}

#[test]
fn imported_metadata_preserves_raw_utf16_and_rolls_back_with_a_failed_import() -> Result<()> {
    use quire_store::{
        notebook::BatchImport,
        sync::{
            format::{self, Kind},
            json::Json,
        },
    };
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let fields = Json::parse(br#"{"page":"p-\ud800","path":"Folder/\udfff.md","text":"---\ntitle: \"\ud800\"\n---\nbody \udfff","parents":[],"generation":0,"deleted":false,"created":1,"device":"office","future":{"s":"\udfff"}}"#)?;
    let id = format::revision_id(&fields)?;
    let canonical = fields.canonical()?;
    let record = format!("{{\"id\":\"{id}\",{}", &canonical[1..]);
    let value = Json::parse(format!("{{\"notebook\":\"nb\",\"device\":\"office\",\"seq\":1,\"previous\":null,\"created\":1,\"revisions\":[{record}],\"attachments\":[]}}").as_bytes())?;
    let batch = format::encode(&value, Kind::Batch)?;
    let connection = Connection::try_open_file(path.to_str().unwrap())?;
    connection.exec("CREATE TRIGGER fail_metadata BEFORE INSERT ON revision_metadata BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END")?()?;
    assert!(
        store
            .import_batch("office", &batch.name, &batch.data)
            .is_err()
    );
    assert!(store.cursor("office")?.is_none());
    assert!(
        store
            .page_metadata(None, MetadataLimits::default())?
            .is_empty()
    );
    connection.exec("DROP TRIGGER fail_metadata")?()?;
    assert!(matches!(
        store.import_batch("office", &batch.name, &batch.data)?,
        BatchImport::Applied(_)
    ));
    let pages = store.page_metadata(None, MetadataLimits::default())?;
    assert_eq!(
        pages[0].page,
        fields.root().field("page").as_js_string().unwrap().clone()
    );
    assert_eq!(
        pages[0].path,
        fields.root().field("path").as_js_string().unwrap().clone()
    );
    assert_eq!(pages[0].title.utf16_units().collect::<Vec<_>>(), [0xd800]);
    assert_eq!(
        store.get(&id)?.unwrap().canonical()?,
        Json::parse(record.as_bytes())?.canonical()?
    );
    Ok(())
}

#[test]
fn metadata_pages_obey_row_and_byte_limits_without_losing_the_cursor() -> Result<()> {
    let root = tempfile::tempdir()?;
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
    let title = "x".repeat(32_768);
    for page in ["a", "b", "c"] {
        store.save(&edit(page, &format!("---\ntitle: {title}\n---\n"), vec![]))?;
    }
    let limit = MetadataLimits {
        pages: 2,
        bytes: 1024,
    };
    let first = store.page_metadata(None, limit)?;
    assert_eq!(
        first.len(),
        1,
        "one oversized metadata row must progress alone"
    );
    let second = store.page_metadata(Some(&first[0].page), limit)?;
    let third = store.page_metadata(Some(&second[0].page), limit)?;
    assert_eq!(
        [
            first[0].page.clone(),
            second[0].page.clone(),
            third[0].page.clone()
        ],
        ["a".into(), "b".into(), "c".into()]
    );
    assert!(store.page_metadata(Some(&third[0].page), limit)?.is_empty());
    assert_eq!(
        store
            .page_metadata(
                None,
                MetadataLimits {
                    pages: 2,
                    bytes: 1_000_000
                }
            )?
            .len(),
        2
    );
    assert!(
        store
            .page_metadata(
                None,
                MetadataLimits {
                    pages: 0,
                    bytes: 1024
                }
            )
            .is_err()
    );
    Ok(())
}

#[test]
fn metadata_corruption_requires_rebuild_and_failed_writes_leave_the_graph_unchanged() -> Result<()>
{
    let root = tempfile::tempdir()?;
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local")?;
    let first = store.save(&edit("p", "---\ntitle: first\n---\n", vec![]))?;
    let connection = Connection::try_open_file(path.to_str().unwrap())?;
    connection.exec("UPDATE revision_metadata SET title='\"corrupt\"'")?()?;
    assert!(
        store
            .page_metadata(None, MetadataLimits::default())
            .unwrap_err()
            .to_string()
            .contains("metadata index")
    );
    assert_eq!(store.rebuild_metadata()?, 1);
    assert_eq!(
        store.page_metadata(None, MetadataLimits::default())?[0]
            .title
            .as_str(),
        Some("first")
    );
    connection.exec("CREATE TRIGGER fail_metadata BEFORE INSERT ON revision_metadata BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END")?()?;
    assert!(
        store
            .save(&edit("p", "changed", vec![first.revision_id().into()]))
            .is_err()
    );
    assert_eq!(store.heads("p")?, [first.revision_id()]);
    assert_eq!(store.outbox()?, [first.revision_id()]);
    connection.exec("DROP TRIGGER fail_metadata")?()?;
    assert_eq!(
        store.page_metadata(None, MetadataLimits::default())?[0]
            .title
            .as_str(),
        Some("first")
    );
    Ok(())
}
