use quire_core::markdown::{angle_autolink_end, bare_url_end};
use serde_json::Value;

fn offset(text: &str, units: usize) -> usize {
    let mut current = 0;
    for (byte, character) in text.char_indices() {
        if current == units {
            return byte;
        }
        current += character.len_utf16();
    }
    assert_eq!(current, units);
    text.len()
}

#[test]
fn bare_url_scanner_matches_frozen_inline_url_boundaries() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/markdown/bare-url-vectors.json"
    ))
    .unwrap();
    assert!(cases.len() > 700);
    for case in cases {
        let text = case["text"].as_str().unwrap();
        let start = offset(text, case["start"].as_u64().unwrap() as usize);
        let end = bare_url_end(text, start, case["inLink"].as_bool().unwrap());
        let actual = end
            .map(|end| Value::from(text[..end].encode_utf16().count()))
            .unwrap_or(Value::Null);
        assert_eq!(actual, case["end"], "{} text={text:?}", case["name"]);
        let angle = case["angleSource"].as_str().unwrap();
        assert_eq!(
            angle_autolink_end(angle, 0) == Some(angle.len()),
            case["angle"].as_bool().unwrap(),
            "{} angle={angle:?}",
            case["name"]
        );
    }
    assert_eq!(bare_url_end("🙂 https://a.b", 1, false), None);
    assert_eq!(bare_url_end("https://a.b", usize::MAX, false), None);
    assert_eq!(angle_autolink_end("🙂 <https://a.b>", 1), None);
    assert_eq!(angle_autolink_end("<https://a.b>", usize::MAX), None);
    let angle = "<https://a.b/مرحبا🙂>";
    let text = format!("prefix {angle}tail");
    assert_eq!(angle_autolink_end(&text, 7), Some(7 + angle.len()));
}

#[test]
fn a_long_unbalanced_parenthesis_tail_retains_only_the_url_body() {
    let prefix = "https://a.b/";
    let text = format!("{prefix}{} suffix", ")".repeat(100_000));
    assert_eq!(bare_url_end(&text, 0, false), Some(prefix.len()));
}
