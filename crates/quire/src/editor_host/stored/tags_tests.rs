use super::super::*;
use super::tests::{Notebook, active, open_notebook, read, save, wait_until};
use crate::navigation::{
    ShowTags,
    tag_page::TagRow,
    tags::{TagSort, visible},
};
use gpui::TestAppContext;
use quire_core::workspace::{BrowserView, Destination};
use quire_store::{index::TagCount, notebook::NotebookStore};

pub(super) const TAGGED: [(&str, &str, &str); 3] = [
    ("a", "Alpha.md", "- first #Plan\n- nested #plan/sub\n"),
    ("b", "Beta.md", "- third #plan\n- #other note\n"),
    ("c", "Code.md", "- `#plan` in code\n"),
];

pub(super) fn tag_rows(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<(String, usize)> {
    read(notebook, cx, |view, cx| {
        view.nav
            .tags
            .list
            .as_ref()
            .map(|list| {
                list.read(cx)
                    .rows(cx)
                    .into_iter()
                    .map(|tag| (tag.label, tag.count))
                    .collect()
            })
            .unwrap_or_default()
    })
}

pub(super) fn show_tags(notebook: &Notebook, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), ShowTags);
    wait_until(cx, "the tag list", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav
                .tags
                .list
                .as_ref()
                .is_some_and(|list| list.read(cx).tags.settled())
        })
    });
}

pub(super) fn page_rows(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<TagRow> {
    read(notebook, cx, |view, cx| {
        view.shown_tag_page()
            .map(|page| page.read(cx).rows(cx))
            .unwrap_or_default()
    })
}

fn texts(rows: &[TagRow]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            TagRow::Page { title, matches, .. } => format!("{title} ({matches})"),
            TagRow::Block { text, .. } => format!("  {text}"),
        })
        .collect()
}

#[gpui::test]
fn tags_list_counts_blocks_filters_and_opens_a_tag_page(cx: &mut TestAppContext) {
    let notebook = open_notebook(&TAGGED, "a", cx);
    show_tags(&notebook, cx);
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.view),
        Some(BrowserView::Tags)
    );
    assert_eq!(
        tag_rows(&notebook, cx),
        [
            ("#other".into(), 1),
            ("#Plan".into(), 2),
            ("#plan/sub".into(), 1)
        ],
        "one row per tag key, labelled by its first use; tags in code do not count"
    );
    cx.simulate_input(notebook.window.into(), "PLAN");
    assert_eq!(tag_rows(&notebook, cx).len(), 2, "the filter ignores case");
    cx.simulate_keystrokes(notebook.window.into(), "down enter");
    wait_until(cx, "the tag page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.shown_tag_page()
                .is_some_and(|page| page.read(cx).blocks.settled())
        })
    });
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.workspace.active.clone()),
        Some(Destination::Tag {
            tag: "#plan/sub".into()
        })
    );
    assert_eq!(
        texts(&page_rows(&notebook, cx)),
        ["Alpha (1)", "  nested #plan/sub"]
    );
    read(&notebook, cx, |view, cx| {
        assert_eq!(
            view.destination_label(view.nav.workspace.tabs.last().unwrap(), cx),
            "#plan/sub"
        );
    });
}

#[gpui::test]
fn tag_pages_group_by_page_follow_saves_and_open_sources(cx: &mut TestAppContext) {
    let notebook = open_notebook(&TAGGED, "c", cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(
                Destination::Tag {
                    tag: "#PLAN".into(),
                },
                window,
                cx,
            )
        })
        .unwrap();
    wait_until(cx, "the tag page", |cx| {
        !page_rows(&notebook, cx).is_empty()
    });
    assert_eq!(
        texts(&page_rows(&notebook, cx)),
        ["Alpha (1)", "  first #Plan", "Beta (1)", "  third #plan"],
        "a tag page matches its tag in any case, and not the tags nested under it"
    );
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(
        &store,
        "b",
        "Beta.md",
        "- third #plan\n- fourth #Plan too\n",
    );
    wait_until(cx, "the saved page to reach the tag page", |cx| {
        page_rows(&notebook, cx).len() == 5
    });
    assert_eq!(
        texts(&page_rows(&notebook, cx)),
        [
            "Alpha (1)",
            "  first #Plan",
            "Beta (2)",
            "  third #plan",
            "  fourth #Plan too"
        ]
    );
    cx.simulate_input(notebook.window.into(), "fourth");
    assert_eq!(
        texts(&page_rows(&notebook, cx)),
        ["Beta (1)", "  fourth #Plan too"]
    );
    cx.simulate_keystrokes(notebook.window.into(), "down shift-enter");
    wait_until(cx, "the page beside", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.page.read(cx).id() == "b")
        })
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            let focus = view.shown_tag_page().unwrap().focus_handle(cx);
            window.focus(&focus, cx);
        })
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    assert_eq!(active(&notebook, cx), "b");
}

#[test]
fn tag_sorts_order_by_count_with_ties_by_name() {
    let tags = [
        TagCount {
            label: "#a".into(),
            count: 1,
        },
        TagCount {
            label: "#b".into(),
            count: 3,
        },
        TagCount {
            label: "#c".into(),
            count: 1,
        },
    ];
    let labels = |sort| -> Vec<String> {
        visible(&tags, "", sort)
            .into_iter()
            .map(|tag| tag.label)
            .collect()
    };
    assert_eq!(labels(TagSort::Name), ["#a", "#b", "#c"]);
    assert_eq!(labels(TagSort::Most), ["#b", "#a", "#c"]);
    assert_eq!(labels(TagSort::Fewest), ["#a", "#c", "#b"]);
}
