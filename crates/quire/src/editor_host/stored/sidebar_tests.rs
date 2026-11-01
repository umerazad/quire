//! The sidebar laid out as the reference's (E11): the view list highlights what shows, Recent
//! folds, the notebook card opens the profile menu, and the footer holds New page, Open a
//! folder, and the Roam import and export.

use super::super::template_tests::{click, open_templates};
use super::super::tests::{Notebook, draw, open_notebook, read, wait_until};
use super::sidebar_views::NavEntry;
use crate::navigation::{palette::PaletteMode, pointer_menu};
use gpui::TestAppContext;
use quire_core::workspace::Destination;
use std::rc::Rc;

/// 2026-10-30 12:00 UTC: the 30th or 31st anywhere.
const NOW: i64 = 1_793_361_600_000;

fn current(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<NavEntry> {
    draw(notebook, cx);
    read(notebook, cx, |view, cx| view.current_nav(cx))
}

fn drawn(notebook: &Notebook, selector: &'static str, cx: &mut TestAppContext) -> bool {
    draw(notebook, cx);
    gpui::VisualTestContext::from_window(notebook.window.into(), cx)
        .debug_bounds(selector)
        .is_some()
}

fn open(notebook: &Notebook, entry: NavEntry, cx: &mut TestAppContext) {
    let selector: &'static str = Box::leak(format!("quire-nav-{}", entry.id()).into_boxed_str());
    click(notebook, selector, cx);
    cx.run_until_parked();
}

#[gpui::test]
fn the_view_list_lists_rt_views_in_order_and_highlights_the_one_showing(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            ("d30", "2026-10-30.md", "- a day\n"),
            ("d31", "2026-10-31.md", "- a day\n"),
        ],
        "a",
        cx,
    );
    notebook
        .window
        .update(cx, |view, _, _| view.nav.daily.now = Rc::new(|| NOW))
        .unwrap();
    assert_eq!(
        NavEntry::ALL.map(NavEntry::label),
        [
            "README.md",
            "Today",
            "Tasks",
            "Templates",
            "Pages",
            "Tags",
            "Daily notes",
            "Help & shortcuts"
        ]
    );
    for entry in NavEntry::ALL {
        let selector: &'static str =
            Box::leak(format!("quire-nav-{}", entry.id()).into_boxed_str());
        assert!(drawn(&notebook, selector, cx), "{entry:?} is drawn");
    }
    assert_eq!(current(&notebook, cx), []);

    for (entry, view) in [
        (NavEntry::Pages, quire_core::workspace::BrowserView::Pages),
        (NavEntry::Tags, quire_core::workspace::BrowserView::Tags),
        (NavEntry::Daily, quire_core::workspace::BrowserView::Daily),
        (NavEntry::Tasks, quire_core::workspace::BrowserView::Tasks),
    ] {
        open(&notebook, entry, cx);
        assert_eq!(read(&notebook, cx, |view, _| view.nav.view), Some(view));
        assert_eq!(current(&notebook, cx), [entry]);
    }
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.pages.len()),
        3,
        "Pages counts the notebook's pages"
    );

    let today = read(&notebook, cx, |view, _| view.today().unwrap());
    let page = if today == "2026-10-30" { "d30" } else { "d31" };
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(Destination::page(page), window, cx)
        })
        .unwrap();
    wait_until(cx, "today's page", |cx| {
        read(&notebook, cx, |view, cx| view.current_nav(cx)) == [NavEntry::Today]
    });

    open_templates(&notebook, cx);
    assert_eq!(current(&notebook, cx), [NavEntry::Templates]);
}

#[gpui::test]
fn recent_folds_until_opened_as_rt_does(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[("a", "Alpha.md", "- a\n"), ("b", "Beta.md", "- b\n")],
        "a",
        cx,
    );
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(Destination::page("b"), window, cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert!(!read(&notebook, cx, |view, _| view
        .nav
        .workspace
        .recent
        .is_empty()));
    assert!(drawn(&notebook, "quire-recent", cx));
    assert!(!read(&notebook, cx, |view, _| view
        .nav
        .browsers
        .recent_open));
    click(&notebook, "quire-recent-heading", cx);
    assert!(read(&notebook, cx, |view, _| view.nav.browsers.recent_open));
}

#[gpui::test]
fn the_notebook_card_opens_the_profile_menu_with_the_open_profile_ticked(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("a", "Alpha.md", "- a\n")], "a", cx);
    let entries = read(&notebook, cx, |view, cx| {
        pointer_menu::summary(&view.profile_menu_entries(cx))
    });
    assert_eq!(entries[..3], ["[x] Current", "[ ] Work", "[ ] Personal"]);
    assert!(
        entries[4].starts_with("Current profile · ") && entries[4].ends_with("(disabled)"),
        "{entries:?}"
    );
    assert!(!read(&notebook, cx, |view, _| view
        .nav
        .browsers
        .profile_menu
        .is_deployed()));
    click(&notebook, "quire-profile-card", cx);
    draw(&notebook, cx);
    assert!(read(&notebook, cx, |view, _| view
        .nav
        .browsers
        .profile_menu
        .is_deployed()));
}

#[gpui::test]
fn the_footer_offers_new_page_open_a_folder_and_the_roam_exchange(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("a", "Alpha.md", "- a\n")], "a", cx);
    for selector in [
        "quire-new-page",
        "quire-open-folder",
        "quire-import-roam-data",
        "quire-export-roam-json",
        "quire-find",
        "quire-profile-card",
    ] {
        assert!(drawn(&notebook, selector, cx), "{selector} is drawn");
    }
    let mode = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .palette
                .as_ref()
                .map(|palette| palette.read(cx).delegate.mode())
        })
    };
    click(&notebook, "quire-find", cx);
    draw(&notebook, cx);
    assert_eq!(mode(cx), Some(PaletteMode::Find));
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    draw(&notebook, cx);
    assert_eq!(mode(cx), None);
    click(&notebook, "quire-new-page", cx);
    draw(&notebook, cx);
    assert_eq!(mode(cx), Some(PaletteMode::Create));
}
