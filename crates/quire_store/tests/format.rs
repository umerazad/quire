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

fn next_random(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

#[test]
fn canonical_json_matches_javascript() {
    for case in vectors()["values"].as_array().unwrap() {
        assert_eq!(
            canonical(&case["value"]).unwrap(),
            case["canonical"].as_str().unwrap()
        );
    }
    assert_eq!(canonical(&serde_json::json!(-0.0)).unwrap(), "0");
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
        assert_eq!(
            decoded.value.canonical().unwrap(),
            canonical(&case["value"]).unwrap()
        );
        assert_eq!(decoded.hash, case["hash"]);
        let encoded = encode(&decoded.value, kind).unwrap();
        assert_eq!(encoded.name, name);
        assert_eq!(encoded.hash, decoded.hash);
        assert_eq!(
            decode(&encoded.name, &encoded.data, kind, expected())
                .unwrap()
                .value
                .canonical()
                .unwrap(),
            decoded.value.canonical().unwrap()
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
fn node_checks_native_envelopes_and_seeded_canonical_json() {
    use quire_store::sync::json::Json;
    use serde_json::json;
    use std::{
        io::Write,
        process::{Command, Stdio},
    };

    let mut seed = 0xbac1_3475_9dde_6921_u64;
    let mut values = vec![];
    for _ in 0..4096 {
        let number = f64::from_bits(next_random(&mut seed));
        if number.is_finite() {
            let value = json!(number);
            values.push(json!({"canonical": canonical(&value).unwrap(), "value": value}));
        }
    }
    let mut lossless = vec![];
    for raw in [
        "{\"n\":9007199254740993}",
        "{\"n\":333333333.33333329}",
        "{\"n\":2.2250738585072011e-308}",
        "{\"n\":1e-999999999999999999999}",
    ] {
        let wire = Json::parse(raw.as_bytes()).unwrap();
        lossless.push(json!({"json": raw, "canonical": wire.canonical().unwrap()}));
    }
    for _ in 0..1024 {
        let mut string = || {
            let mut text = String::from("\"");
            for _ in 0..(next_random(&mut seed) % 24) {
                text.push_str(&format!("\\u{:04x}", next_random(&mut seed) as u16));
            }
            text.push('"');
            text
        };
        let key = string();
        let text = string();
        let raw = format!(
            "{{{key}:[{text},{{\"settings\":{{\"vimMode\":true,\"customCss\":{text}}}}}],\"\\ue000\":1,\"𐀀\":2}}"
        );
        let wire = Json::parse(raw.as_bytes()).unwrap();
        lossless.push(json!({"json": raw, "canonical": wire.canonical().unwrap()}));
    }
    let vectors = vectors();
    let mut records = vec![];
    for (key, kind) in [("batch", Kind::Batch), ("checkpoint", Kind::Checkpoint)] {
        let value = &vectors[key]["value"];
        let encoded = encode(value, kind).unwrap();
        records.push(json!({"kind": key, "value": value, "name": encoded.name, "hash": encoded.hash, "data": encoded.data, "error": null}));
    }
    for label in ["unpaired-surrogate", "surrogate-revision", "deep-json"] {
        let metadata: Value =
            serde_json::from_slice(&fs::read(fixtures().join(format!("{label}.json"))).unwrap())
                .unwrap();
        let data = fs::read(fixtures().join(format!("{label}.zst"))).unwrap();
        let decoded = decode(
            metadata["name"].as_str().unwrap(),
            &data,
            Kind::Batch,
            expected(),
        )
        .unwrap();
        let encoded = encode(&decoded.value, Kind::Batch).unwrap();
        records.push(json!({"kind": "batch", "canonical": decoded.value.canonical().unwrap(), "name": encoded.name, "hash": encoded.hash, "data": encoded.data, "error": null}));
    }
    let base = vectors["batch"]["value"].clone();
    let mut invalid = vec![];
    for (field, replacement) in [
        ("parents", json!(["a".repeat(64), "a".repeat(64)])),
        ("parents", json!(vec!["a".repeat(64); 17])),
        ("parents", json!(["A".repeat(64)])),
        ("generation", json!(-1)),
        ("generation", json!(0.5)),
        ("created", json!(null)),
        ("device", json!(false)),
        ("deleted", json!(1)),
        ("page", json!("")),
    ] {
        let mut value = base.clone();
        value["revisions"][0][field] = replacement;
        invalid.push(value);
    }
    let mut array_revision = base.clone();
    array_revision["revisions"] = json!([[]]);
    invalid.push(array_revision);
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
        let text = canonical(&value).unwrap();
        let name = format!("000000001-{}.qlog.zst", sha256(&text));
        let data = zstd::stream::encode_all(text.as_bytes(), 0).unwrap();
        let error = decode(&name, &data, Kind::Batch, expected()).unwrap_err();
        records.push(json!({"kind": "batch", "name": name, "data": data, "error": error.reason()}));
    }
    for (field, replacement) in [
        ("cursors", json!(null)),
        ("cursors", json!([])),
        (
            "cursors",
            json!({"office": {"seq": -1, "hash": "a".repeat(64)}}),
        ),
        (
            "cursors",
            json!({"office": {"seq": 1, "hash": "A".repeat(64)}}),
        ),
        ("heads", json!(null)),
        ("heads", json!([{"page": "p-missing"}])),
    ] {
        let mut value = vectors["checkpoint"]["value"].clone();
        value[field] = replacement;
        let text = canonical(&value).unwrap();
        let name = format!("000000001-{}.qsnap.zst", sha256(&text));
        let data = zstd::stream::encode_all(text.as_bytes(), 0).unwrap();
        let error = decode(&name, &data, Kind::Checkpoint, expected()).unwrap_err();
        records.push(
            json!({"kind": "checkpoint", "name": name, "data": data, "error": error.reason()}),
        );
    }
    let mut checkpoint = vectors["checkpoint"]["value"].clone();
    checkpoint["heads"][0]["device"] = json!("home");
    let encoded = encode(&checkpoint, Kind::Checkpoint).unwrap();
    records.push(json!({"kind": "checkpoint", "name": encoded.name, "hash": encoded.hash, "data": encoded.data, "value": checkpoint, "error": null}));
    for (future, format) in [("1e400", "1"), ("\"\u{fffd}\"", "1"), ("null", "1e400")] {
        let mut value = base.clone();
        value.as_object_mut().unwrap().remove("future");
        value.as_object_mut().unwrap().remove("format");
        let text = canonical(&value).unwrap();
        let mut bytes = format!(
            "{},\"future\":{future},\"format\":{format}}}",
            &text[..text.len() - 1]
        )
        .into_bytes();
        if future.contains('\u{fffd}') {
            let offset = bytes
                .windows(3)
                .position(|window| window == [0xef, 0xbf, 0xbd])
                .unwrap();
            bytes.splice(offset..offset + 3, [0xff]);
        }
        let hash = sha256(&bytes);
        let name = format!("000000001-{hash}.qlog.zst");
        let data = zstd::stream::encode_all(bytes.as_slice(), 0).unwrap();
        match decode(&name, &data, Kind::Batch, expected()) {
            Ok(decoded) if future == "1e400" => {
                assert!(encode(&decoded.value, Kind::Batch).is_err());
                records.push(json!({"kind": "batch", "name": name, "data": data, "hash": hash, "error": null, "overflow": true}));
            }
            Ok(decoded) => records.push(json!({"kind": "batch", "name": name, "data": data, "hash": hash, "error": null, "canonical": decoded.value.canonical().unwrap()})),
            Err(error) => records.push(json!({"kind": "batch", "name": name, "data": data, "error": error.reason()})),
        }
    }
    let script = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/format-oracle.ts");
    let mut child = Command::new("node").arg(script).stdin(Stdio::piped()).stderr(Stdio::piped()).spawn()
        .expect("Cross-language verification requires Node 26. Install it and rerun cargo test -p quire_store.");
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            &serde_json::to_vec(
                &json!({"values": values, "records": records, "lossless": lossless}),
            )
            .unwrap(),
        )
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

/// rt's Go terminal app writes this batch (see `tests/reference/UPSTREAM.json`); rt's
/// `tests/sync-go-batch.test.ts` reads it the same way.
#[test]
fn a_batch_written_by_the_go_terminal_app_decodes_byte_for_byte() {
    let folder = fixtures().join("../sync-vectors/go-written");
    let expected: Value =
        serde_json::from_slice(&fs::read(folder.join("expected.json")).unwrap()).unwrap();
    let name = expected["name"].as_str().unwrap();
    let data = fs::read(folder.join(name)).unwrap();
    let decoded = decode(
        name,
        &data,
        Kind::Batch,
        Expected {
            notebook: "vectors-notebook",
            device: "go",
        },
    )
    .unwrap();
    let revisions: Vec<_> = decoded
        .value
        .root()
        .field("revisions")
        .elements()
        .unwrap()
        .collect();
    assert_eq!(revisions.len(), 1);
    let revision = revisions[0];
    assert_eq!(revision.field("id").as_str(), expected["id"].as_str());
    let text: Vec<u16> = revision
        .field("text")
        .as_js_string()
        .unwrap()
        .utf16_units()
        .collect();
    let mut wanted: Vec<u16> = "- written by Go \"q\" \\ \u{2028} 😀 "
        .encode_utf16()
        .collect();
    wanted.extend([0xd800, u16::from(b'\n')]);
    assert_eq!(text, wanted, "the lone surrogate survives");
    let encoded = encode(&decoded.value, Kind::Batch).unwrap();
    assert_eq!(
        encoded.name, name,
        "the native encoder writes the same bytes"
    );
}

#[test]
fn surrogate_and_deep_json_envelopes_round_trip_losslessly() {
    for label in ["unpaired-surrogate", "surrogate-revision", "deep-json"] {
        let metadata: Value =
            serde_json::from_slice(&fs::read(fixtures().join(format!("{label}.json"))).unwrap())
                .unwrap();
        let data = fs::read(fixtures().join(format!("{label}.zst"))).unwrap();
        let original = data.clone();
        assert_eq!(metadata["sourceAccepts"], true);
        let decoded = decode(
            metadata["name"].as_str().unwrap(),
            &data,
            Kind::Batch,
            expected(),
        )
        .unwrap();
        let encoded = encode(&decoded.value, Kind::Batch).unwrap();
        assert_eq!(encoded.name, metadata["name"]);
        assert_eq!(encoded.hash, decoded.hash);
        assert_eq!(
            decode(&encoded.name, &encoded.data, Kind::Batch, expected())
                .unwrap()
                .value
                .canonical()
                .unwrap(),
            decoded.value.canonical().unwrap()
        );
        assert_eq!(data, original);
    }
}
