//! A restored notebook answers the notebook's queries exactly as the original does.

use super::*;
use crate::index::{IndexConfig, IndexedBlock, IndexedLink, IndexedTag, PageIndex, QueryIndex};

fn config() -> IndexConfig {
    IndexConfig {
        analyzer: 1,
        layout: quire_core::daily::DailyLayout {
            folder: "daily".into(),
            title: "{YYYY}-{MM}-{DD}".into(),
        },
        calendar: quire_core::daily::DailyCalendar::utc(),
    }
}

/// The analysis a page gets here: one block per line, every `#word` a tag, and every
/// `[[Title]]` a page link.
fn analysis(text: &str) -> PageIndex {
    let mut index = PageIndex::default();
    let mut offset = 0;
    for line in text.split('\n') {
        let end = offset + line.len() as u32;
        index.blocks.push(IndexedBlock {
            key: format!("at-{offset}"),
            from: offset,
            to: end,
            content_from: offset,
            content_to: end,
            subtree_to: end,
            kind: "list".into(),
            text: line.into(),
            content: line.into(),
            ..Default::default()
        });
        let mut position = 0;
        for word in line.split(' ') {
            let from = offset + position;
            if word.starts_with('#') {
                index.tagged.push(IndexedTag {
                    tag: word.into(),
                    from,
                    block_id: None,
                    snippet: line.into(),
                });
            }
            if let Some(target) = word.strip_prefix("[[").and_then(|w| w.strip_suffix("]]")) {
                index.links.push(IndexedLink {
                    kind: "page".into(),
                    syntax: "wiki".into(),
                    target: target.into(),
                    from,
                    to: from + word.len() as u32,
                    text: line.into(),
                    ..Default::default()
                });
            }
            position += word.len() as u32 + 1;
        }
        offset = end + 1;
    }
    index
}

fn analyze(index: &QueryIndex) {
    loop {
        let pending = index.pending(64).unwrap();
        if pending.is_empty() {
            break;
        }
        for page in pending {
            let text = page.text.as_str().unwrap().to_owned();
            assert!(index.put(&page.revision, &analysis(&text)).unwrap());
        }
    }
    assert_eq!(index.unanalyzed().unwrap(), 0);
}

/// What the queries answer: the pages, two searches, the tags, the blocks tagged #plan, and every
/// page's backlinks.
fn answers(index: &QueryIndex) -> Vec<String> {
    let pages = index.pages().unwrap();
    let mut answers = vec![
        format!("{pages:?}"),
        format!("{:?}", index.search("second").unwrap()),
        format!("{:?}", index.search("plan").unwrap()),
        format!("{:?}", index.tag_names().unwrap()),
        format!("{:?}", index.tagged_blocks("#plan").unwrap()),
    ];
    answers.extend(
        pages
            .iter()
            .map(|listed| format!("{:?}", index.backlinks(&listed.page.page).unwrap())),
    );
    answers
}

#[test]
fn a_restored_notebook_answers_queries_as_the_original_does() {
    let fixture = StoreFixture::new();
    let store = fixture.open();
    save(
        &store,
        "p-c",
        "Plans.md",
        "- second plan #plan links [[B]]\n- more #plan",
    );
    save(&store, "p-b", "B.md", "- other [[Plans]] #idea");
    drop(store);
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let restored = restore(
        &RestoreRequest {
            backup: &backup.path,
            parent: parent.path(),
            current: None,
            state_for: &no_state,
            created: 2,
        },
        &mut |_| Ok(()),
    )
    .unwrap();
    let Location::Store { database, .. } = restored.location else {
        panic!("a Quire notebook restores as a Quire notebook");
    };
    let original = QueryIndex::open(&fixture.database, config()).unwrap();
    let copy = QueryIndex::open(&database, config()).unwrap();
    analyze(&original);
    analyze(&copy);
    let expected = answers(&original);
    assert!(expected[2].contains("Plans.md"), "{}", expected[2]);
    assert_eq!(
        expected[4].matches("tag: \"#plan\"").count(),
        2,
        "{}",
        expected[4]
    );
    assert!(
        expected
            .iter()
            .any(|answer| answer.contains("Plans.md") && answer.contains("\"B\""))
    );
    assert_eq!(answers(&copy), expected);

    let folder = FolderFixture::new();
    fs::write(
        folder.root.path().join("Plans.md"),
        "- second plan #plan [[A]]",
    )
    .unwrap();
    let backup = create(&folder.source(), folder.destination.path(), 3).unwrap();
    let state_for = folder.state_for();
    let restored = restore(
        &RestoreRequest {
            backup: &backup.path,
            parent: parent.path(),
            current: None,
            state_for: &state_for,
            created: 4,
        },
        &mut |_| Ok(()),
    )
    .unwrap();
    let Location::Folder { root, state } = restored.location else {
        panic!("a folder notebook restores as a folder notebook");
    };
    let original =
        QueryIndex::open_folder(folder.root.path(), folder.state.path(), config()).unwrap();
    let copy = QueryIndex::open_folder(&root, &state, config()).unwrap();
    analyze(&original);
    analyze(&copy);
    let expected = answers(&original);
    assert!(expected[4].contains("Plans.md"), "{}", expected[4]);
    assert_eq!(answers(&copy), expected);
}
