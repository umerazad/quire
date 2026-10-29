use quire_store::sync::format::*;
use serde_json::Value;
use std::{fs, path::PathBuf};

fn fixtures() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/protocol")
}
fn vectors() -> Value {
    serde_json::from_slice(&fs::read(fixtures().join("vectors.json")).unwrap()).unwrap()
}
fn expected() -> Expected<'static> {
    Expected {
        notebook: "nb-vectors",
        device: "office",
    }
}

#[test]
fn canonical_json_matches_javascript() {
    for case in vectors()["values"].as_array().unwrap() {
        assert_eq!(
            canonical(&case["value"]),
            case["canonical"].as_str().unwrap()
        );
    }
    assert_eq!(canonical(&serde_json::json!(-0.0)), "0");
}

#[test]
fn page_and_revision_ids_match_javascript() {
    let vectors = vectors();
    for case in vectors["paths"].as_array().unwrap() {
        assert_eq!(page_id_for_path(case["path"].as_str().unwrap()), case["id"]);
    }
    for case in vectors["revisions"].as_array().unwrap() {
        assert_eq!(revision_id(&case["fields"]).unwrap(), case["id"]);
    }
}

#[test]
fn filenames_and_attachments_are_strict() {
    let vectors = vectors();
    for case in vectors["names"].as_array().unwrap() {
        assert_eq!(
            parse_name(case["name"].as_str().unwrap(), Kind::Batch).is_some(),
            case["valid"].as_bool().unwrap()
        );
    }
    for case in vectors["attachments"].as_array().unwrap() {
        assert_eq!(
            attachment_hash(case["name"].as_str().unwrap()),
            case["hash"].as_str()
        );
    }
    assert_eq!(
        attachment_file_name(&[0, 1, 2, 255], "IMAGE.PnG"),
        vectors["attachment"]["name"]
    );
}

#[test]
fn node_envelopes_decode_and_reencode_without_losing_fields() {
    let vectors = vectors();
    for (key, kind) in [("batch", Kind::Batch), ("checkpoint", Kind::Checkpoint)] {
        let case = &vectors[key];
        let name = case["name"].as_str().unwrap();
        let data = fs::read(fixtures().join(name)).unwrap();
        let decoded = decode(name, &data, kind, expected()).unwrap();
        assert_eq!(decoded.value, case["value"]);
        assert_eq!(decoded.hash, case["hash"]);
        let encoded = encode(&decoded.value, kind).unwrap();
        assert_eq!(encoded.name, name);
        assert_eq!(encoded.hash, decoded.hash);
        assert_eq!(
            decode(&encoded.name, &encoded.data, kind, expected())
                .unwrap()
                .value,
            decoded.value
        );
    }
}

#[test]
fn damaged_and_newer_records_match_node_outcomes() {
    for case in vectors()["failures"].as_array().unwrap() {
        let data =
            fs::read(fixtures().join(format!("{}.zst", case["label"].as_str().unwrap()))).unwrap();
        let error = decode(
            case["name"].as_str().unwrap(),
            &data,
            Kind::Batch,
            expected(),
        )
        .unwrap_err();
        assert_eq!(error.reason(), case["outcome"]["reason"].as_str().unwrap());
        assert_eq!(error.newer_format(), case["outcome"]["newer"].as_f64());
    }
}

#[test]
fn rejects_corruption_and_partial_files() {
    let vectors = vectors();
    let name = vectors["batch"]["name"].as_str().unwrap();
    let data = fs::read(fixtures().join(name)).unwrap();
    assert!(decode(name, &data[..data.len() / 2], Kind::Batch, expected()).is_err());
    assert!(decode(name, b"plain text", Kind::Batch, expected()).is_err());
    let wrong_name = format!("000000001-{}.qlog.zst", "0".repeat(64));
    assert_eq!(
        decode(&wrong_name, &data, Kind::Batch, expected())
            .unwrap_err()
            .reason(),
        "the file's content does not match its name"
    );
}

#[test]
fn writer_rejects_unreadable_sequences_and_invalid_revisions() {
    let mut value = vectors()["batch"]["value"].clone();
    value["seq"] = serde_json::json!(1_000_000_000);
    assert!(encode(&value, Kind::Batch).is_err());
    value["seq"] = serde_json::json!(1);
    value["revisions"][0]["text"] = serde_json::json!("tampered");
    assert!(encode(&value, Kind::Batch).is_err());
}

#[test]
fn node_reads_native_records_and_checks_seeded_number_rendering() {
    use serde_json::json;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    let mut seed = 0xbac1_3475_9dde_6921_u64;
    let mut values = vec![];
    for _ in 0..4096 {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        let number = f64::from_bits(seed);
        if number.is_finite() {
            let value = json!(number);
            values.push(json!({"canonical": canonical(&value), "value": value}));
        }
    }
    let vectors = vectors();
    let mut records = vec![];
    for (key, kind) in [("batch", Kind::Batch), ("checkpoint", Kind::Checkpoint)] {
        let value = &vectors[key]["value"];
        let encoded = encode(value, kind).unwrap();
        records.push(json!({"kind": key, "value": value, "name": encoded.name, "hash": encoded.hash, "data": encoded.data, "error": null}));
    }
    let base = vectors["batch"]["value"].clone();
    let mut invalid = vec![];
    for field in [
        "format",
        "previous",
        "revisions",
        "attachments",
        "created",
        "seq",
        "device",
        "notebook",
    ] {
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove(field);
        invalid.push(value);
    }
    for (field, replacement) in [
        ("format", json!(-1)),
        ("format", json!(1e21)),
        ("created", json!(9_007_199_254_740_992_u64)),
        ("created", json!(0.5)),
        ("revisions", json!({})),
        ("attachments", json!(["a".repeat(64) + ".PDF"])),
    ] {
        let mut value = base.clone();
        value[field] = replacement;
        invalid.push(value);
    }
    for value in invalid {
        let text = canonical(&value);
        let name = format!("000000001-{}.qlog.zst", sha256(&text));
        let data = zstd::stream::encode_all(text.as_bytes(), 0).unwrap();
        let error = decode(&name, &data, Kind::Batch, expected()).unwrap_err();
        records.push(json!({"kind": "batch", "name": name, "data": data, "error": error.reason()}));
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/format-oracle.ts");
    let mut child = Command::new("node").arg(script).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .expect("Cross-language verification requires Node 26. Install it and rerun cargo test -p quire_store.");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&serde_json::to_vec(&json!({"values": values, "records": records})).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "Node protocol oracle failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn revisions_and_status_reject_invalid_fields() {
    use serde_json::json;
    let revision = vectors()["batch"]["value"]["revisions"][0].clone();
    validate_revision(&revision).unwrap();
    for replacement in [
        json!(["a".repeat(64), "a".repeat(64)]),
        json!(vec!["a".repeat(64); 17]),
        json!(["A".repeat(64)]),
        json!(null),
    ] {
        let mut value = revision.clone();
        value["parents"] = replacement;
        assert!(validate_revision(&value).is_err());
    }
    let status = json!({"format": 1, "app": "0.9.43", "device": "office", "label": "مكتب", "lastSeq": 1, "outbox": 0, "heartbeat": 1234, "future": true});
    validate_status(&status).unwrap();
    for field in [
        "format",
        "app",
        "device",
        "label",
        "lastSeq",
        "outbox",
        "heartbeat",
    ] {
        let mut value = status.clone();
        value.as_object_mut().unwrap().remove(field);
        assert!(validate_status(&value).is_err());
    }
}

// This records a release-blocking compatibility gap, not successful parity.
#[test]
fn source_accepted_json_outside_native_model_is_rejected_without_mutation() {
    for label in ["unpaired-surrogate", "deep-json"] {
        let metadata: Value =
            serde_json::from_slice(&fs::read(fixtures().join(format!("{label}.json"))).unwrap())
                .unwrap();
        let data = fs::read(fixtures().join(format!("{label}.zst"))).unwrap();
        let original = data.clone();
        assert_eq!(metadata["sourceAccepts"], true);
        let error = decode(
            metadata["name"].as_str().unwrap(),
            &data,
            Kind::Batch,
            expected(),
        )
        .unwrap_err();
        assert_eq!(error.reason(), "the file is not valid JSON");
        assert_eq!(data, original);
    }
}
