use quire_core::tag_edits::{PAGE_LIMIT, Retag, check_page_count};

#[test]
fn renames_move_nested_tags_but_deletes_take_only_the_chosen_ones() {
    let rename = Retag::new(&["#Area"], Some(" #work ")).unwrap();
    assert_eq!(rename.change("#area"), Some(Some("work".into())));
    assert_eq!(rename.change("#AREA/Home"), Some(Some("work/Home".into())));
    assert_eq!(rename.change("#areas"), None);
    assert!(rename.touches("area/home") && !rename.touches("areas"));

    let delete = Retag::new(&["#area"], None).unwrap();
    assert_eq!(delete.change("#Area"), Some(None));
    assert_eq!(delete.change("#area/home"), None);
    assert!(delete.touches("area") && !delete.touches("area/home"));
}

#[test]
fn the_longest_chosen_parent_wins() {
    let retag = Retag::new(&["#a", "#a/b"], Some("x")).unwrap();
    assert_eq!(retag.change("#a/b/c"), Some(Some("x/c".into())));
    assert_eq!(retag.change("#a/c"), Some(Some("x/c".into())));
}

#[test]
fn more_than_the_page_limit_is_refused_with_the_count() {
    assert!(check_page_count(PAGE_LIMIT).is_ok());
    let error = check_page_count(PAGE_LIMIT + 1).unwrap_err();
    assert!(
        error.starts_with("This changes 1001 pages; at most 1000"),
        "{error}"
    );
}
