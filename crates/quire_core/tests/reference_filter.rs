use quire_core::reference_filter::{
    ReferenceContext, ReferenceFilter, UNCLOSED_QUOTE, query_tokens,
};

#[test]
fn tokens_keep_pages_and_quotes_whole_and_split_on_javascript_whitespace() {
    assert_eq!(
        query_tokens("-[[Book A]] \"two words\"\u{feff}#tag\u{a0}-x").collect::<Vec<_>>(),
        ["-[[Book A]]", "\"two words\"", "#tag", "-x"]
    );
}

#[test]
fn edge_terms_follow_the_reference_rules() {
    let filter =
        ReferenceFilter::parse("- # project: PROJECT:Alpha [[ Spaced ]] -#Tag/Sub").unwrap();
    assert_eq!(filter.words, ["-", "#", "project:"]);
    assert_eq!(filter.projects, ["alpha"]);
    assert_eq!(
        filter.pages,
        ["Spaced"],
        "page names keep their case and lose outer space"
    );
    assert_eq!(filter.not_tags, ["#tag/sub"]);
    assert_eq!(ReferenceFilter::parse("\"").unwrap_err(), UNCLOSED_QUOTE);
    assert!(ReferenceFilter::parse("  ").unwrap().is_empty());
}

#[test]
fn tags_match_their_nested_children_but_not_prefixes() {
    let context = ReferenceContext {
        tags: ["#work/project".to_owned()].into(),
        ..Default::default()
    };
    let names = Default::default();
    assert!(
        ReferenceFilter::parse("#work")
            .unwrap()
            .matches(&names, &context)
    );
    assert!(
        !ReferenceFilter::parse("#wor")
            .unwrap()
            .matches(&names, &context)
    );
    assert!(
        !ReferenceFilter::parse("-#work")
            .unwrap()
            .matches(&names, &context)
    );
}
