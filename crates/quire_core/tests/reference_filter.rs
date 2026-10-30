use quire_core::reference_filter::{
    ReferenceContext, ReferenceFilter, TermState, UNCLOSED_QUOTE, query_tokens, term_state,
    toggle_term,
};

#[test]
fn chips_toggle_terms_the_reference_way() {
    assert_eq!(toggle_term("", "#idea", false), "#idea");
    assert_eq!(toggle_term("#idea", "#idea", false), "");
    assert_eq!(toggle_term("#idea words", "#idea", true), "words -#idea");
    assert_eq!(toggle_term("-#Idea", "#idea", true), "");
    assert_eq!(toggle_term("x", "[[Book A]]", false), "x [[Book A]]");
    assert_eq!(term_state("x -#Idea", "#idea"), Some(TermState::Exclude));
    assert_eq!(
        term_state("[[book a]]", "[[Book A]]"),
        Some(TermState::Include)
    );
    assert_eq!(term_state("#ideas", "#idea"), None);
}

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
