use quire_core::{
    json::{Json, ValueRef},
    merge::merge_text,
};

fn units(value: ValueRef<'_>) -> Vec<u16> {
    value
        .as_js_string()
        .expect("fixture string")
        .utf16_units()
        .collect()
}

fn check(case: ValueRef<'_>) -> bool {
    let merged = merge_text(
        &units(case.field("base")),
        &units(case.field("first")),
        &units(case.field("second")),
    );
    assert_eq!(
        merged.text,
        units(case.field("text")),
        "{}",
        case.canonical().unwrap()
    );
    assert_eq!(
        merged.overlaps as f64,
        case.field("overlaps").as_f64().unwrap(),
        "{}",
        case.canonical().unwrap()
    );
    merged.overlaps > 0
}

#[test]
fn shared_merge_cases_match_the_typescript_and_go_fixtures() {
    let cases = Json::parse(include_bytes!("../../../tests/fixtures/merges.json")).unwrap();
    let cases: Vec<_> = cases.root().elements().unwrap().collect();
    assert!(cases.len() >= 20);
    cases.into_iter().for_each(|case| {
        check(case);
    });
}

#[test]
fn seeded_merges_match_the_frozen_typescript_merge() {
    let vectors =
        Json::parse(include_bytes!("../../../tests/fixtures/merge-vectors.json")).unwrap();
    let cases: Vec<_> = vectors.root().field("texts").elements().unwrap().collect();
    assert!(cases.len() >= 4000);
    let overlapping = cases.into_iter().filter(|case| check(*case)).count();
    assert!(overlapping > 200, "{overlapping}");
}
