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

/// Whether nothing can redraw the window soon: every open page is saved with no save in flight,
/// the index has analyzed every save, and no index status line is shown above the sidebar.
fn quiet(notebook: &Notebook, cx: &mut TestAppContext) -> bool {
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    futures::executor::block_on(client.unanalyzed().unwrap()).unwrap() == 0
        && read(notebook, cx, |view, cx| {
            view.index_status(cx).is_none()
                && view.session.borrow().open_pages().iter().all(|page| {
                    page.read(cx).persistence().is_none_or(|persistence| {
                        let persistence = persistence.read(cx);
                        persistence.is_idle() && !persistence.is_dirty(cx)
                    })
                })
        })
}

/// Clicks once the window is quiet. A GPUI test click runs the executor between the press and
/// the release, so a save reply or an index status line appearing then would move the layout
/// under the pointer and lose the click.
pub(super) fn click(notebook: &Notebook, selector: &'static str, cx: &mut TestAppContext) {
    wait_until(cx, "a quiet window", |cx| quiet(notebook, cx));
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
    let state = |cx: &mut TestAppContext| {
        read(notebook, cx, |view, cx| {
            (
                view.page.read(cx).path(cx),
                view.nav.creating.is_some(),
                view.nav.error.clone(),
                view.nav.view,
            )
        })
    };
    crate::test_wait::until(
        cx,
        std::time::Duration::ZERO,
        |cx| wait_until(cx, "a step", |_| true),
        |cx| {
            let (path, creating, ..) = state(cx);
            path.as_deref()
                .is_some_and(quire_core::templates::is_templates_page)
                && shown_text(notebook, cx).is_some()
                && !creating
        },
        |cx| {
            format!(
                "the Templates page: (path, creating, error, browser) = {:?}",
                state(cx)
            )
        },
    );
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

/// Presses `key` once the window is quiet, so no reply moves focus between the press and the
/// release that clicks a focused control.
fn press(notebook: &Notebook, key: &str, cx: &mut TestAppContext) {
    wait_until(cx, "a quiet window", |cx| quiet(notebook, cx));
    crate::tab_order::press(notebook.window.into(), key, cx);
}

/// The FILES heading and Refresh files, the stops between the recents and the tree.
const FILES_HEADING_STOPS: usize = 2;

#[gpui::test]
fn the_keyboard_reaches_the_templates_entry_and_the_guides_add_buttons(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("w", "Alpha.md", "- \n")], "w", cx);
    let entries_after_templates = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, _| {
            let workspace = &view.nav.workspace;
            workspace.favorites.len()
                + 2 * workspace
                    .recent
                    .len()
                    .min(quire_core::workspace::SHOWN_RECENT)
                + FILES_HEADING_STOPS
        })
    };
    cx.dispatch_action(notebook.window.into(), crate::navigation::FocusFiles);
    for _ in 0..=entries_after_templates(cx) {
        press(&notebook, "shift-tab", cx);
    }
    press(&notebook, "enter", cx);
    wait_until(cx, "the Templates page from the keyboard", |cx| {
        shown_path(&notebook, cx).as_deref() == Some("Templates.md")
            && shown_text(&notebook, cx).is_some()
    });
    assert_eq!(shown_text(&notebook, cx), Some(starter_templates()));

    cx.dispatch_action(
        notebook.window.into(),
        crate::navigation::FocusTemplatesGuide,
    );
    press(&notebook, "enter", cx);
    let one_on_one = format!("{}{}", starter_templates(), definition(&EXAMPLES[1]));
    assert_eq!(
        shown_text(&notebook, cx),
        Some(one_on_one.clone()),
        "the guide's first Add button skips the added Meeting"
    );
    press(&notebook, "tab enter", cx);
    assert_eq!(
        shown_text(&notebook, cx),
        Some(one_on_one.clone()),
        "Tab reaches the added Project, whose button adds nothing"
    );
    let defined_now = defined(&notebook, cx);
    let next = EXAMPLES
        .iter()
        .position(|example| !defined_now.contains(&example.name.to_lowercase()))
        .unwrap();
    for _ in 2..next {
        press(&notebook, "tab", cx);
    }
    press(&notebook, "enter", cx);
    let next = &EXAMPLES[next];
    assert_eq!(
        shown_text(&notebook, cx),
        Some(format!("{one_on_one}{}", definition(next))),
        "Tab moves to the next Add button"
    );
}
