use super::tests::{Notebook, active, draw, open_notebook, read, wait_until};
use gpui::TestAppContext;
use quire_core::templates::guide::{EXAMPLES, definition, starter_templates};

pub(super) fn stored_text(notebook: &Notebook, page: &str) -> Option<String> {
    let store = quire_store::notebook::NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    store
        .checkout(page, None)
        .ok()?
        .selected?
        .root()
        .field("text")
        .as_str()
        .map(str::to_owned)
}

pub(super) fn shown_text(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
    read(notebook, cx, |view, cx| {
        let loaded = view.page.read(cx).loaded()?;
        Some(loaded.analysis.read(cx).source.read(cx).text())
    })
}

fn shown_path(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
    read(notebook, cx, |view, cx| view.page.read(cx).path(cx))
}

/// Clicks once the index has analyzed every save, so no reply redraws the window between the
/// press and the release.
pub(super) fn click(notebook: &Notebook, selector: &'static str, cx: &mut TestAppContext) {
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    wait_until(cx, "a quiet index", |_| {
        futures::executor::block_on(client.unanalyzed().unwrap()).unwrap() == 0
    });
    draw(notebook, cx);
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("no element {selector}"));
    visual.simulate_click(bounds.center(), Default::default());
}

/// Clicks the pinned Templates entry and waits for the page it opens.
pub(super) fn open_templates(notebook: &Notebook, cx: &mut TestAppContext) {
    click(notebook, "quire-templates-entry", cx);
    wait_until(cx, "the Templates page", |cx| {
        shown_path(notebook, cx).is_some_and(|path| quire_core::templates::is_templates_page(&path))
            && shown_text(notebook, cx).is_some()
            && read(notebook, cx, |view, _| view.nav.creating.is_none())
    });
    draw(notebook, cx);
}

fn defined(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    read(notebook, cx, |view, cx| {
        let (_, guide) = view.nav.templates_guide.as_ref().unwrap();
        let mut names: Vec<_> = guide.read(cx).defined(cx).into_iter().collect();
        names.sort();
        names
    })
}

#[gpui::test]
fn the_sidebar_entry_creates_templates_with_starters_and_the_guide_adds_examples(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&[("w", "Alpha.md", "- \n")], "w", cx);
    assert!(!read(&notebook, cx, |view, cx| view.showing_templates(cx)));
    open_templates(&notebook, cx);
    let page = active(&notebook, cx);
    assert_eq!(shown_path(&notebook, cx).as_deref(), Some("Templates.md"));
    assert_eq!(shown_text(&notebook, cx), Some(starter_templates()));
    assert_eq!(stored_text(&notebook, &page), Some(starter_templates()));
    assert!(read(&notebook, cx, |view, cx| view.showing_templates(cx)));
    assert_eq!(
        defined(&notebook, cx),
        ["meeting", "my day", "person", "project"]
    );

    click(&notebook, "quire-add-template-Meeting", cx);
    cx.run_until_parked();
    assert_eq!(shown_text(&notebook, cx), Some(starter_templates()));

    let one_on_one = &EXAMPLES[1];
    click(&notebook, "quire-add-template-One-on-one", cx);
    let expected = format!("{}{}", starter_templates(), definition(one_on_one));
    assert_eq!(shown_text(&notebook, cx), Some(expected.clone()));
    assert!(defined(&notebook, cx).contains(&"one-on-one".to_owned()));
    wait_until(cx, "the added example to save", |_| {
        stored_text(&notebook, &page).as_deref() == Some(expected.as_str())
    });

    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("w".into(), false, window, cx)
        })
        .unwrap();
    wait_until(cx, "the first page", |cx| active(&notebook, cx) == "w");
    assert!(!read(&notebook, cx, |view, cx| view.showing_templates(cx)));
    open_templates(&notebook, cx);
    assert_eq!(active(&notebook, cx), page);
    assert_eq!(shown_text(&notebook, cx), Some(expected));
}

#[gpui::test]
fn the_sidebar_entry_reuses_a_templates_page_whatever_its_case(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("w", "Alpha.md", "- \n"),
            ("t", "templates.md", "- Mine\n  - body\n"),
            (
                "n",
                "Notes/Templates.md",
                "- Nested\n  - not a template page\n",
            ),
        ],
        "w",
        cx,
    );
    open_templates(&notebook, cx);
    assert_eq!(active(&notebook, cx), "t");
    assert_eq!(
        shown_text(&notebook, cx).as_deref(),
        Some("- Mine\n  - body\n")
    );
    assert_eq!(defined(&notebook, cx), ["mine"]);
}
