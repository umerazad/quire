use quire_store::sync::json::Json;

#[test]
fn utf16_strings_are_lossless_and_keys_use_utf16_order() {
    for (input, expected) in [
        (r#""\ud800""#, r#""\ud800""#),
        (r#""\udc00""#, r#""\udc00""#),
        (r#""\ud800\ud800x\udfff""#, r#""\ud800\ud800x\udfff""#),
        (r#""\ud83d\ude42""#, "\"🙂\""),
        (
            r#"{"\ue000":1,"\ud800":2,"𐀀":3}"#,
            r#"{"\ud800":2,"𐀀":3,"":1}"#,
        ),
    ] {
        let value = Json::parse(input.as_bytes()).unwrap();
        assert_eq!(value.canonical().unwrap(), expected);
        assert_eq!(value.clone().canonical().unwrap(), expected);
    }
}

#[test]
fn deep_documents_parse_serialize_clone_and_drop_without_recursion() {
    let input = format!("{}0{}", "[".repeat(20_000), "]".repeat(20_000));
    let value = Json::parse(input.as_bytes()).unwrap();
    assert_eq!(value.canonical().unwrap(), input);
    assert_eq!(value.clone().canonical().unwrap(), input);
}

#[test]
fn grammar_validation_stays_with_serde_json() {
    for invalid in [
        "",
        "[1,]",
        r#"{"a":}"#,
        r#""\q""#,
        "true false",
        "01",
        "{\"x\":0",
    ] {
        assert!(Json::parse(invalid.as_bytes()).is_err(), "{invalid}");
    }
}

#[test]
fn duplicates_use_the_last_value_and_utf8_decoding_matches_node() {
    assert_eq!(
        Json::parse(br#"{"a":1,"a":2}"#)
            .unwrap()
            .canonical()
            .unwrap(),
        r#"{"a":2}"#
    );
    assert_eq!(
        Json::parse(b"\"\xff\"").unwrap().canonical().unwrap(),
        "\"�\""
    );
}

#[test]
fn byte_deserialization_rejects_invalid_wtf8() {
    use quire_store::sync::json::JsString;
    use serde::{
        Deserialize,
        de::value::{BorrowedBytesDeserializer, Error},
    };
    for bytes in [&[0xff][..], &[0xed, 0xa0][..], &[0xc0, 0x80][..]] {
        let decoder = BorrowedBytesDeserializer::<Error>::new(bytes);
        assert!(JsString::deserialize(decoder).is_err());
    }
}

#[test]
fn overflowing_numbers_parse_as_javascript_infinities() {
    let value = Json::parse(br#"{"future":1e400,"negative":-1e400}"#).unwrap();
    assert_eq!(value.root().field("future").as_f64(), Some(f64::INFINITY));
    assert_eq!(
        value.root().field("negative").as_f64(),
        Some(f64::NEG_INFINITY)
    );
    assert!(value.canonical().is_err());
}
