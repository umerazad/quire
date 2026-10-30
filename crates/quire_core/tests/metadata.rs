use quire_core::markdown::{
    frontmatter_length, frontmatter_length_chars, page_name, relative_vault_path, safe_page_path,
};
use serde_json::Value;

#[test]
fn metadata_helpers_match_the_frozen_markdown_oracle() {
    let cases: Vec<Value> = serde_json::from_str(include_str!(
        "../../../tests/fixtures/markdown/metadata-vectors.json"
    ))
    .unwrap();
    assert!(cases.len() >= 500);
    for (index, case) in cases.iter().enumerate() {
        let title = case["title"].as_str().unwrap();
        let actual = safe_page_path(title);
        match case["path"].as_str() {
            Some(expected) => assert_eq!(actual.unwrap(), expected, "path fixture {index}"),
            None => assert_eq!(
                actual.unwrap_err().to_string(),
                case["error"].as_str().unwrap(),
                "path error fixture {index}"
            ),
        }
        let target = case["target"].as_str().unwrap();
        assert_eq!(
            relative_vault_path(case["from"].as_str(), target).as_deref(),
            case["relative"].as_str(),
            "link fixture {index}"
        );
        let text = case["markdown"].as_str().unwrap();
        let offset = frontmatter_length(text);
        assert!(
            text.is_char_boundary(offset),
            "metadata boundary fixture {index}"
        );
        assert_eq!(
            text[..offset].encode_utf16().count(),
            case["frontmatterUtf16"].as_u64().unwrap() as usize,
            "metadata fixture {index}"
        );
        assert_eq!(
            page_name(case["pagePath"].as_str().unwrap()),
            case["pageName"].as_str().unwrap(),
            "page name fixture {index}"
        );
    }
}

#[test]
fn frontmatter_scanning_stops_at_the_boundary_or_an_invalid_opener() {
    let prefix = "\u{feff}---\r\ntitle: میٹنگ 🙂\r\n---\r\n";
    let characters = prefix.chars().chain(std::iter::once_with(|| {
        panic!("The metadata scanner read the page body")
    }));
    assert_eq!(frontmatter_length_chars(characters), prefix.len());
    let characters = std::iter::once('p').chain(std::iter::once_with(|| {
        panic!("The metadata scanner read past an invalid opener")
    }));
    assert_eq!(frontmatter_length_chars(characters), 0);
}
