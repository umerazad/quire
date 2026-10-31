//! rt's format 1 sync vectors (`scripts/sync-vectors.ts`, copied unchanged with hashes in
//! `tests/reference/UPSTREAM.json`): two writers' change files, decoded and encoded again to the
//! same names, imported by a third writer to rt's heads, and rt's canonical JSON, ID, and head
//! order rules.

use quire_store::{
    notebook::{BatchImport, NotebookStore},
    sync::{
        format::{self, Expected, Kind},
        json::Json,
    },
};
use serde_json::Value;
use std::{collections::HashMap, fs, path::PathBuf};

const NOTEBOOK: &str = "vectors-notebook";

fn folder() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/sync-vectors")
}

fn expected() -> Value {
    serde_json::from_slice(&fs::read(folder().join("expected.json")).unwrap()).unwrap()
}

fn batches() -> Vec<(String, String, Vec<u8>)> {
    expected()["batches"]
        .as_array()
        .unwrap()
        .iter()
        .map(|batch| {
            let device = batch["device"].as_str().unwrap().to_owned();
            let name = batch["name"].as_str().unwrap().to_owned();
            let data = fs::read(
                folder()
                    .join("devices")
                    .join(&device)
                    .join("log")
                    .join(&name),
            )
            .unwrap();
            (device, name, data)
        })
        .collect()
}

fn canonical_round_trip(text: &str) -> String {
    Json::parse(text.as_bytes()).unwrap().canonical().unwrap()
}

#[test]
fn rt_change_files_decode_and_encode_to_the_same_names() {
    let expected = expected();
    let header: Value =
        serde_json::from_slice(&fs::read(folder().join("quire-notebook.json")).unwrap()).unwrap();
    assert_eq!(header["id"], NOTEBOOK);
    let mut revisions = HashMap::new();
    for ((device, name, data), batch) in batches()
        .iter()
        .zip(expected["batches"].as_array().unwrap())
    {
        let decoded = format::decode(
            name,
            data,
            Kind::Batch,
            Expected {
                notebook: NOTEBOOK,
                device,
            },
        )
        .unwrap_or_else(|error| panic!("{device}/{name}: {error}"));
        assert_eq!(decoded.hash, batch["hash"].as_str().unwrap(), "{name}");
        assert_eq!(
            format::parse_name(name, Kind::Batch).unwrap().seq as u64,
            batch["seq"].as_u64().unwrap()
        );
        let encoded = format::encode(&decoded.value, Kind::Batch).unwrap();
        assert_eq!(encoded.name, *name, "{name}: same name");
        // The name carries the hash of the uncompressed content, the format's identity; the
        // compressed frames differ between rt's zstd and the native one.
        let again = format::decode(
            name,
            &encoded.data,
            Kind::Batch,
            Expected {
                notebook: NOTEBOOK,
                device,
            },
        )
        .unwrap();
        assert_eq!(again.hash, decoded.hash, "{name}: same content");
        for revision in decoded.value.root().field("revisions").elements().unwrap() {
            revisions.insert(
                revision.field("id").as_str().unwrap().to_owned(),
                revision.canonical().unwrap(),
            );
        }
    }
    let listed = expected["revisions"].as_array().unwrap();
    assert_eq!(revisions.len(), listed.len());
    for revision in listed {
        let id = revision["id"].as_str().unwrap();
        let canonical = revision["canonical"].as_str().unwrap();
        assert_eq!(
            revisions.get(id).map(String::as_str),
            Some(canonical),
            "{id}"
        );
        let parsed = Json::parse(canonical.as_bytes()).unwrap();
        assert_eq!(format::revision_id(&parsed).unwrap(), id, "{id}");
        assert_eq!(
            canonical_round_trip(revision["idInput"].as_str().unwrap()),
            revision["idInput"].as_str().unwrap()
        );
    }
}

#[test]
fn a_third_writer_imports_rt_change_files_to_rt_heads_and_order() {
    let expected = expected();
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), NOTEBOOK, "reader").unwrap();
    for (device, name, data) in batches() {
        let imported = store.import_batch(&device, &name, &data).unwrap();
        assert!(
            !matches!(imported, BatchImport::Rejected { .. }),
            "{device}/{name}: {imported:?}"
        );
    }
    for (page, heads) in expected["heads"].as_object().unwrap() {
        let heads: Vec<String> = serde_json::from_value(heads.clone()).unwrap();
        assert_eq!(store.heads(page).unwrap(), heads, "{page}");
    }

    let generation: HashMap<String, f64> = expected["revisions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|revision| {
            let parsed = Json::parse(revision["canonical"].as_str().unwrap().as_bytes()).unwrap();
            let generation = parsed.root().field("generation").as_f64().unwrap();
            (revision["id"].as_str().unwrap().to_owned(), generation)
        })
        .collect();
    let mut sorted: Vec<&String> = generation.keys().collect();
    sorted.sort_by(|a, b| {
        generation[*b]
            .total_cmp(&generation[*a])
            .then_with(|| b.cmp(a))
    });
    let wanted: Vec<String> = serde_json::from_value(expected["sortedHeads"].clone()).unwrap();
    assert_eq!(
        sorted.into_iter().cloned().collect::<Vec<_>>(),
        wanted,
        "heads sort by generation, then ID, both descending"
    );
}

#[test]
fn rt_page_ids_canonical_strings_key_order_numbers_and_frozen_id_match() {
    let expected = expected();
    for case in expected["pageIds"].as_array().unwrap() {
        assert_eq!(
            format::page_id_for_path(case["path"].as_str().unwrap()),
            case["id"].as_str().unwrap(),
            "{}",
            case["path"]
        );
    }
    for case in expected["strings"].as_array().unwrap() {
        let canonical = case["canonical"].as_str().unwrap();
        assert_eq!(canonical_round_trip(canonical), canonical);
        assert_eq!(format::sha256(canonical), case["sha256"].as_str().unwrap());
    }
    let key_order = expected["keyOrder"].as_str().unwrap();
    assert_eq!(canonical_round_trip(key_order), key_order);
    let numbers = expected["numbers"].as_str().unwrap();
    assert_eq!(
        format::canonical(&serde_json::json!([
            0,
            -0.0,
            1,
            -1,
            2.5,
            1e21,
            1e-7,
            123_456_789_012_345_u64,
            0.1 + 0.2
        ]))
        .unwrap(),
        numbers
    );
    assert_eq!(
        format::revision_id(&serde_json::json!({
            "page": "p-1",
            "parents": [],
            "generation": 0,
            "path": "A.md",
            "deleted": false,
            "text": "- hello\r\n- مرحبا\n",
        }))
        .unwrap(),
        expected["frozenId"].as_str().unwrap()
    );
}
