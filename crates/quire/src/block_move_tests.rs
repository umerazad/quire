use super::*;
use quire_core::markdown::references;

fn blocks(text: &str, cx: &mut gpui::TestAppContext) -> Vec<Block> {
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
    let snapshot = cx.update(|cx| crate::markdown::detached_snapshot(text, cx));
    let snapshot = cx.foreground_executor().block_test(snapshot);
    crate::markdown_blocks::current_blocks(&snapshot).expect("Markdown blocks")
}

#[gpui::test]
fn a_nested_block_moves_with_its_children_to_the_left_margin(cx: &mut gpui::TestAppContext) {
    let text = "- Top\n  - Move ^m\n    - Child ^c\n  - Stay\n";
    let cut = cut(text, &blocks(text, cx), "m", "# Inbox\n\nexisting").unwrap();
    assert_eq!(cut.source, "- Top\n  - Stay\n");
    assert_eq!(
        cut.destination,
        "# Inbox\n\nexisting\n\n- Move ^m\n  - Child ^c\n"
    );
    assert_eq!(cut.ids, BTreeSet::from(["m".to_owned(), "c".to_owned()]));
}

#[gpui::test]
fn the_destination_gets_one_blank_line_before_the_block(cx: &mut gpui::TestAppContext) {
    let text = "- Last ^l";
    let found = blocks(text, cx);
    for (destination, expected) in [
        ("", "- Last ^l\n"),
        ("a\n", "a\n\n- Last ^l\n"),
        ("a\n\n", "a\n\n- Last ^l\n"),
        ("a", "a\n\n- Last ^l\n"),
    ] {
        let cut = cut(text, &found, "l", destination).unwrap();
        assert_eq!(cut.destination, expected);
        assert_eq!(cut.source, "");
    }
    assert_eq!(cut(text, &found, "gone", ""), Err(MISSING_BLOCK));
}

#[test]
fn page_qualified_references_to_moved_blocks_follow_them() {
    let text = "See [[Old#^m]], [[Other#^m]], ((m)), [[Old#^kept]] and [[old#^c|label]].\n";
    let ids = BTreeSet::from(["m".to_owned(), "c".to_owned()]);
    let found = references(text, &[]);
    let edits = retarget(text, &found, &ids, "notes/New.md", |base| {
        base.eq_ignore_ascii_case("old")
    });
    assert_eq!(
        apply(text, edits),
        "See [[notes/New#^m]], [[Other#^m]], ((m)), [[Old#^kept]] and [[notes/New#^c|label]].\n"
    );
}
