use quire_core::{
    json::Json,
    markdown::{page_title_for_editor, page_title_lossless},
};

#[test]
fn raw_utf16_storage_titles_match_the_frozen_oracle() {
    let cases = Json::parse(include_bytes!(
        "../../../tests/fixtures/markdown/lossless-title-vectors.json"
    ))
    .unwrap();
    let cases: Vec<_> = cases.root().elements().unwrap().collect();
    assert!(cases.len() >= 800);
    for case in cases {
        let path = case.field("path").as_js_string().unwrap();
        let text = case.field("text").as_js_string().unwrap();
        let original = (path.clone(), text.clone());
        let title = page_title_lossless(path, text);
        let expected: Vec<_> = case
            .field("titleUtf16")
            .elements()
            .unwrap()
            .map(|unit| unit.as_f64().unwrap() as u16)
            .collect();
        assert_eq!(
            title.utf16_units().collect::<Vec<_>>(),
            expected,
            "{}",
            case.canonical().unwrap()
        );
        assert_eq!(original, (path.clone(), text.clone()));
        let title = page_title_for_editor(path, text);
        let expected: Vec<_> = case
            .field("editorTitleUtf16")
            .elements()
            .unwrap()
            .map(|unit| unit.as_f64().unwrap() as u16)
            .collect();
        assert_eq!(
            title.utf16_units().collect::<Vec<_>>(),
            expected,
            "editor {}",
            case.canonical().unwrap()
        );
    }
}
