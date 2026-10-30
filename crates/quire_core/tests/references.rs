use quire_core::markdown::references;
use serde_json::{Value, json};

fn offset(text: &str, units: usize) -> usize {
    let mut current = 0;
    for (byte, c) in text.char_indices() {
        if current == units {
            return byte;
        }
        current += c.len_utf16();
    }
    assert_eq!(
        current, units,
        "oracle offset must be on a complete UTF-16 character boundary"
    );
    text.len()
}

#[test]
fn exclusions_merge_overlaps_and_filter_starts_without_discarding_crossing_matches() {
    let text = "[[one]] [[two]] [[three]]";
    let targets = |ranges: &[std::ops::Range<usize>]| {
        references(text, ranges)
            .into_iter()
            .map(|reference| reference.target)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        targets(std::slice::from_ref(&(2..5))),
        ["one", "two", "three"]
    );
    assert_eq!(targets(&[4..9, 0..6, 6..8, 0..0]), ["three"]);
    assert_eq!(
        targets(std::slice::from_ref(&(0..text.len()))),
        Vec::<String>::new()
    );
}

#[test]
fn reference_syntax_matches_the_frozen_oracle_with_explicit_code_exclusions() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/markdown/reference-vectors.json"
    ))
    .unwrap();
    assert!(cases.len() >= 500);
    for case in cases {
        for mode in ["raw", "editor"] {
            let vector = &case[mode];
            let text = vector["text"].as_str().unwrap();
            let mut excluded: Vec<_> = vector["codeRanges"]
                .as_array()
                .unwrap()
                .iter()
                .map(|range| {
                    offset(text, range[0].as_u64().unwrap() as usize)
                        ..offset(text, range[1].as_u64().unwrap() as usize)
                })
                .collect();
            excluded.reverse();
            excluded.extend(excluded.clone());
            let actual: Vec<_> = references(text, &excluded).iter().map(|reference| {
                let mut value = json!({"kind":reference.kind.as_str(),"syntax":reference.syntax.as_str(),"target":reference.target,"from":text[..reference.range.start].encode_utf16().count(),"to":text[..reference.range.end].encode_utf16().count()});
                if let Some(label) = &reference.label { value["label"] = label.clone().into(); }
                if let Some(media) = reference.media { value["media"] = media.as_str().into(); }
                value
            }).collect();
            assert_eq!(
                json!(actual),
                vector["references"],
                "{} {mode}",
                case["name"]
            );
        }
    }
}
