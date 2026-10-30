use quire_core::markdown::{frontmatter_tags, page_title};
use serde_json::Value;

#[test]
fn title_and_tag_metadata_match_the_frozen_markdown_oracle_losslessly() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/markdown/frontmatter-vectors.json"
    ))
    .unwrap();
    assert!(cases.len() >= 700);
    for case in cases {
        let name = case["name"].as_str().unwrap();
        let source = case["text"].as_str().unwrap();
        let title = page_title(case["path"].as_str().unwrap(), source);
        let expected: Vec<u16> = case["titleUtf16"]
            .as_array()
            .unwrap()
            .iter()
            .map(|unit| u16::try_from(unit.as_u64().unwrap()).unwrap())
            .collect();
        assert_eq!(
            title.utf16_units().collect::<Vec<_>>(),
            expected,
            "title {name}"
        );
        assert_eq!(
            title.as_str(),
            String::from_utf16(&expected).ok().as_deref(),
            "native title representation {name}"
        );
        let expected: Vec<_> = case["tags"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tag| tag.as_str().unwrap())
            .collect();
        assert_eq!(frontmatter_tags(source), expected, "tags {name}");
    }
}
