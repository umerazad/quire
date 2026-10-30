use anyhow::Result;
use quire_store::{
    notebook::{BatchImport, NotebookStore},
    sync::format::{self, Kind},
};
use serde_json::json;

/// Imports one revision as another device's first batch. Imports keep competing heads
/// until the next rescan, save, or open merges them, so tests use this to build conflicts.
pub fn import_branch(
    store: &NotebookStore,
    device: &str,
    page: &str,
    parents: &[&str],
    text: &str,
    deleted: bool,
) -> Result<String> {
    let generation = parents
        .iter()
        .map(|id| {
            Ok(store
                .get(id)?
                .unwrap()
                .root()
                .field("generation")
                .as_f64()
                .unwrap() as u64
                + 1)
        })
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .max()
        .unwrap_or(0);
    let mut revision = json!({"page":page,"parents":parents,"generation":generation,"path":format!("{page}.md"),"text":text,"deleted":deleted,"created":1,"device":device});
    revision["id"] = format::revision_id(&revision)?.into();
    let encoded = format::encode(
        &json!({"notebook":store.notebook_id(),"device":device,"seq":1,"previous":null,"created":1,"revisions":[revision],"attachments":[]}),
        Kind::Batch,
    )?;
    assert!(matches!(
        store.import_batch(device, &encoded.name, &encoded.data)?,
        BatchImport::Applied(_)
    ));
    Ok(revision["id"].as_str().unwrap().to_owned())
}
