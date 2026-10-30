use super::super::*;
use super::tests::{Notebook, active, open_notebook, read, save, wait_until};
use crate::navigation::{
    ShowTags,
    context_rows::ContextRow,
    tags::{TagSort, visible},
};
use gpui::TestAppContext;
use quire_core::workspace::{BrowserView, Destination};
use quire_store::{index::TagCount, notebook::NotebookStore};

pub(super) const TAGGED: [(&str, &str, &str); 3] = [
    ("a", "Alpha.md", "- first #Plan\n  - nested #plan/sub\n"),
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

pub(super) fn page_rows(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<ContextRow> {
    read(notebook, cx, |view, cx| {
        view.shown_tag_page()
            .map(|page| page.read(cx).rows(cx))
            .unwrap_or_default()
    })
}

fn texts(rows: &[ContextRow]) -> Vec<String> {
    rows.iter()
        .map(|row| match row {
            ContextRow::Page { title, matches, .. } => format!("{title} ({matches})"),
            ContextRow::Block { text, .. } => format!("  {text}"),
            ContextRow::Parents { text, .. } => format!("  ^ {text}"),
            ContextRow::Child { text, .. } => format!("    {text}"),
            ContextRow::More { count, .. } => format!("    +{count}"),
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
        ["Alpha (1)", "  ^ first #Plan", "  nested #plan/sub"]
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
        [
            "Alpha (1)",
            "  first #Plan",
            "    nested #plan/sub",
            "Beta (1)",
            "  third #plan"
        ],
        "a tag page matches its tag in any case, not the tags nested under it, and shows nested blocks"
    );
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(
        &store,
        "b",
        "Beta.md",
        "- third #plan\n- fourth #Plan too\n",
    );
    wait_until(cx, "the saved page to reach the tag page", |cx| {
        page_rows(&notebook, cx).len() == 6
    });
    assert_eq!(
        texts(&page_rows(&notebook, cx)),
        [
            "Alpha (1)",
            "  first #Plan",
            "    nested #plan/sub",
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

fn edits(
    notebook: &Notebook,
    cx: &mut TestAppContext,
) -> Entity<crate::navigation::tag_edits::TagEdits> {
    read(notebook, cx, |view, cx| {
        view.nav.tags.list.as_ref().unwrap().read(cx).edits.clone()
    })
}

fn focus_name(notebook: &Notebook, cx: &mut TestAppContext) {
    let edits = edits(notebook, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            let focus = edits.read(cx).name.focus_handle(cx);
            window.focus(&focus, cx);
        })
        .unwrap();
}

fn settled_edits(notebook: &Notebook, cx: &mut TestAppContext) {
    let edits = edits(notebook, cx);
    wait_until(cx, "the tag edit", |cx| {
        cx.read(|cx| !edits.read(cx).is_busy())
    });
}

fn open_buffer(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<language::Buffer> {
    wait_until(cx, "the first page", |cx| {
        read(notebook, cx, |view, cx| {
            view.page.read(cx).persistence().is_some()
        })
    });
    read(notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    })
}

fn stored(notebook: &Notebook, page: &str) -> String {
    NotebookStore::open(&notebook.path, "nb", "local")
        .unwrap()
        .checkout(page, None)
        .unwrap()
        .selected
        .unwrap()
        .root()
        .field("text")
        .as_str()
        .unwrap()
        .to_owned()
}

#[gpui::test]
fn renaming_selected_tags_previews_every_change_then_rewrites_open_and_closed_pages(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&TAGGED, "a", cx);
    let buffer = open_buffer(&notebook, cx);
    buffer.update(cx, |buffer, cx| {
        let end = buffer.len();
        buffer.edit([(end..end, "- typed #plan\n")], None, cx)
    });
    show_tags(&notebook, cx);
    cx.simulate_keystrokes(
        notebook.window.into(),
        &format!("down {}", super::tests::key("enter")),
    );
    let list = read(&notebook, cx, |view, _| view.nav.tags.list.clone().unwrap());
    list.update(cx, |list, cx| list.toggle("#other", cx));
    assert_eq!(
        cx.read(|cx| list.read(cx).selected().to_vec()),
        ["#Plan", "#other"],
        "Command-Enter and the checkbox both choose tags"
    );
    focus_name(&notebook, cx);
    cx.simulate_input(notebook.window.into(), "#job");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    settled_edits(&notebook, cx);
    let edits = edits(&notebook, cx);
    cx.read(|cx| {
        let edits = edits.read(cx);
        assert_eq!(edits.error, None);
        let preview = edits.preview.as_ref().unwrap();
        assert_eq!(
            preview.summary(),
            "Rename #Plan, #other to #job on 2 pages?"
        );
        let changes: Vec<_> = preview
            .changes()
            .into_iter()
            .map(|change| format!("{}: {} -> {}", change.page, change.before, change.after))
            .collect();
        assert_eq!(
            changes,
            [
                "Alpha: #Plan -> #job",
                "Alpha: #plan/sub -> #job/sub",
                "Alpha: #plan -> #job",
                "Beta: #plan -> #job",
                "Beta: #other -> #job",
            ],
            "the preview reads unsaved typing and moves nested tags"
        );
    });
    notebook
        .window
        .update(cx, |_, window, cx| {
            edits.update(cx, |edits, cx| edits.apply(window, cx))
        })
        .unwrap();
    settled_edits(&notebook, cx);
    cx.read(|cx| {
        let edits = edits.read(cx);
        assert_eq!(edits.error, None);
        assert_eq!(edits.done.as_deref(), Some("Changed tags on 2 pages."));
        assert!(edits.preview.is_none() && edits.name.read(cx).text(cx).is_empty());
    });
    assert_eq!(stored(&notebook, "b"), "- third #job\n- #job note\n");
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "- first #job\n  - nested #job/sub\n- typed #job\n"
    );
    assert_eq!(stored(&notebook, "c"), "- `#plan` in code\n");
    assert!(cx.read(|cx| list.read(cx).selected().is_empty()));
    wait_until(cx, "the renamed tags in the list", |cx| {
        tag_rows(&notebook, cx) == [("#job".to_owned(), 4), ("#job/sub".to_owned(), 1)]
    });
}

#[gpui::test]
fn invalid_names_empty_selections_and_stale_previews_show_inline_errors(cx: &mut TestAppContext) {
    use quire_core::tag_edits::{INVALID_NAME, NOTHING_CHOSEN};
    let notebook = open_notebook(&TAGGED, "a", cx);
    let buffer = open_buffer(&notebook, cx);
    show_tags(&notebook, cx);
    let list = read(&notebook, cx, |view, _| view.nav.tags.list.clone().unwrap());
    list.update(cx, |list, cx| list.toggle("#Plan", cx));
    focus_name(&notebook, cx);
    cx.simulate_input(notebook.window.into(), "two words");
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    settled_edits(&notebook, cx);
    let edits = edits(&notebook, cx);
    assert_eq!(
        cx.read(|cx| edits.read(cx).error.clone()).as_deref(),
        Some(INVALID_NAME)
    );
    cx.simulate_input(notebook.window.into(), "s");
    assert_eq!(
        cx.read(|cx| edits.read(cx).error.clone()),
        None,
        "typing clears the error"
    );

    edits.update(cx, |edits, cx| edits.preview(false, cx));
    settled_edits(&notebook, cx);
    assert_eq!(
        cx.read(|cx| edits
            .read(cx)
            .preview
            .as_ref()
            .map(|preview| preview.summary())),
        Some("Delete #Plan on 2 pages?".into())
    );
    buffer.update(cx, |buffer, cx| buffer.edit([(0..0, "x")], None, cx));
    notebook
        .window
        .update(cx, |_, window, cx| {
            edits.update(cx, |edits, cx| edits.apply(window, cx))
        })
        .unwrap();
    settled_edits(&notebook, cx);
    assert_eq!(
        cx.read(|cx| edits.read(cx).error.clone()).as_deref(),
        Some(crate::tag_edit::STALE_PREVIEW)
    );
    assert_eq!(stored(&notebook, "b"), "- third #plan\n- #other note\n");
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "x- first #Plan\n  - nested #plan/sub\n"
    );

    list.update(cx, |list, cx| list.toggle("#Plan", cx));
    edits.update(cx, |edits, cx| edits.preview(false, cx));
    assert_eq!(
        cx.read(|cx| edits.read(cx).error.clone()).as_deref(),
        Some(NOTHING_CHOSEN)
    );
}

#[gpui::test]
fn long_previews_list_twenty_changes_until_asked_for_more(cx: &mut TestAppContext) {
    let many: String = (0..25)
        .map(|index| format!("- item {index} #many\n"))
        .collect();
    let notebook = open_notebook(&[("m", "Many.md", many.as_str())], "m", cx);
    show_tags(&notebook, cx);
    let list = read(&notebook, cx, |view, _| view.nav.tags.list.clone().unwrap());
    list.update(cx, |list, cx| list.toggle("#many", cx));
    let edits = edits(&notebook, cx);
    edits.update(cx, |edits, cx| edits.preview(false, cx));
    settled_edits(&notebook, cx);
    let shown = |cx: &mut TestAppContext| {
        cx.read(|cx| {
            let edits = edits.read(cx);
            let (rows, more) = edits.preview.as_ref().unwrap().shown(edits.limit);
            (rows.len(), more)
        })
    };
    assert_eq!(shown(cx), (20, 5));
    edits.update(cx, |edits, cx| edits.show_more(cx));
    assert_eq!(shown(cx), (25, 0));
}
