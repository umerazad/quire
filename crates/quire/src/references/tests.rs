use super::*;

fn parsed(text: &str) -> Vec<Reference> {
    quire_core::markdown::references(text, &[])
}

#[test]
fn targets_follow_the_reference_kinds() {
    let text =
        "[[Page]] [[Page#^b]] ((b)) {{embed: ((e))}} {{embed: [[Whole]]}} ![[Also]] {{pdf: a.pdf}}";
    let targets: Vec<_> = parsed(text).iter().map(Target::of).collect();
    assert_eq!(
        targets,
        [
            Some(Target::Page("Page".into())),
            Some(Target::Block("Page#^b".into())),
            Some(Target::Block("b".into())),
            Some(Target::Block("e".into())),
            Some(Target::Page("Whole".into())),
            Some(Target::Page("Also".into())),
            None,
        ]
    );
}

#[test]
fn the_reference_at_a_caret_includes_its_end() {
    let text = "go [[Page]] now";
    let references = parsed(text);
    assert!(reference_at(&references, 2).is_none());
    assert_eq!(reference_at(&references, 3).unwrap().target, "Page");
    assert_eq!(reference_at(&references, 11).unwrap().target, "Page");
    assert!(reference_at(&references, 12).is_none());
}
