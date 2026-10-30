use quire_core::markdown::{references, tag_completion_range, tags};
use serde_json::{Value, json};
use std::ops::Range;

fn offset(text: &str, units: usize) -> usize {
    let mut current = 0;
    for (byte, character) in text.char_indices() {
        if current == units {
            return byte;
        }
        current += character.len_utf16();
    }
    assert_eq!(
        current, units,
        "oracle positions must be complete UTF-16 scalar boundaries"
    );
    text.len()
}

fn ranges(text: &str, value: &Value) -> Vec<Range<usize>> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|range| {
            offset(text, range[0].as_u64().unwrap() as usize)
                ..offset(text, range[1].as_u64().unwrap() as usize)
        })
        .collect()
}

#[test]
fn tag_tokens_and_completion_match_the_frozen_oracle_with_explicit_exclusions() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/markdown/tag-vectors.json"
    ))
    .unwrap();
    assert!(cases.len() >= 500);
    for case in cases {
        for mode in ["raw", "editor"] {
            let vector = &case[mode];
            let text = vector["text"].as_str().unwrap();
            let refs = references(text, &ranges(text, &vector["codeRanges"]));
            let mut excluded = ranges(text, &vector["syntaxRanges"]);
            excluded.reverse();
            excluded.extend(excluded.clone());
            let actual: Vec<_> = tags(text, &excluded, &refs)
                .iter()
                .map(|tag| {
                    json!({
                        "from": text[..tag.range.start].encode_utf16().count(),
                        "to": text[..tag.range.end].encode_utf16().count(),
                        "text": tag.text,
                    })
                })
                .collect();
            assert_eq!(json!(actual), vector["tags"], "{} {mode}", case["name"]);
            for completion in vector["completions"].as_array().unwrap() {
                let position = completion[0].as_u64().unwrap() as usize;
                let actual = tag_completion_range(text, offset(text, position), &excluded, &refs)
                    .map(|range| {
                        json!([
                            text[..range.start].encode_utf16().count(),
                            text[..range.end].encode_utf16().count()
                        ])
                    });
                assert_eq!(
                    actual.unwrap_or(Value::Null),
                    completion[1],
                    "{} {mode} cursor={position}",
                    case["name"]
                );
            }
        }
    }
}

#[test]
fn tokens_filter_overlap_while_completion_filters_only_the_tag_start() {
    let text = "#a #b";
    let excluded = std::slice::from_ref(&(1..2));
    assert_eq!(
        tags(text, excluded, &[])
            .iter()
            .map(|tag| tag.text.as_str())
            .collect::<Vec<_>>(),
        ["#b"]
    );
    assert_eq!(tag_completion_range(text, 2, excluded, &[]), Some(0..2));
    assert_eq!(
        tag_completion_range(text, 1, std::slice::from_ref(&(0..1)), &[]),
        None
    );
    assert_eq!(tag_completion_range("🙂 #tag", 1, &[], &[]), None);
    assert_eq!(tag_completion_range(text, usize::MAX, &[], &[]), None);
}
