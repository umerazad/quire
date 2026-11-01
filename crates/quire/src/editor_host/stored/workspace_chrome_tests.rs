//! The workspace as the reference lays it out (E11 window chrome): one page editor per pane
//! with the Write and Source switch above it, the page actions menu, the header's controls,
//! and the sidebar under the traffic lights.

use super::menu_tests::{choose, menu};
use super::tests::{Notebook, draw, open_notebook, read, wait_until};
use gpui::{Bounds, Pixels, TestAppContext, point, px};

const PAGE: &str = "- first ^b1\n- second\n";

fn bounds(notebook: &Notebook, selector: &'static str, cx: &mut TestAppContext) -> Bounds<Pixels> {
    draw(notebook, cx);
    gpui::VisualTestContext::from_window(notebook.window.into(), cx)
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"))
}

fn click_at(notebook: &Notebook, at: gpui::Point<Pixels>, cx: &mut TestAppContext) {
    draw(notebook, cx);
    gpui::VisualTestContext::from_window(notebook.window.into(), cx)
        .simulate_click(at, Default::default());
    cx.run_until_parked();
}

fn opened(cx: &mut TestAppContext) -> Notebook {
    let notebook = open_notebook(&[("p", "Plan.md", PAGE)], "p", cx);
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
}

fn writing(notebook: &Notebook, cx: &mut TestAppContext) -> bool {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).writing(cx)
    })
}

/// The reference capture showed two "Source" columns side by side: every page drew the
/// harness's second editor next to its own. A page pane draws one editor, as wide as the pane.
#[gpui::test]
fn a_page_pane_draws_one_editor_across_the_pane(cx: &mut TestAppContext) {
    let notebook = opened(cx);
    draw(&notebook, cx);
    let (page, second) = read(&notebook, cx, |view, cx| {
        let editor = view.editor.as_ref().unwrap().read(cx);
        (
            editor.source.read(cx).last_bounds().copied(),
            editor.second.read(cx).last_bounds().copied(),
        )
    });
    assert!(second.is_none(), "the second editor is not drawn");
    let page = page.expect("the page editor is drawn");
    let toolbar = bounds(&notebook, "quire-view-switch", cx);
    let status = bounds(&notebook, "quire-page-status", cx);
    assert!(
        page.size.width > status.size.width * 0.75,
        "the page editor spans its pane: {page:?} in {status:?}"
    );
    assert!(toolbar.bottom() <= page.top() && page.bottom() <= status.top());
}

#[gpui::test]
fn the_write_and_source_switch_changes_the_page_editor(cx: &mut TestAppContext) {
    let notebook = opened(cx);
    assert!(!writing(&notebook, cx));
    let switch = bounds(&notebook, "quire-view-switch", cx);
    let middle = switch.center().y;
    click_at(
        &notebook,
        point(switch.left() + switch.size.width * 0.25, middle),
        cx,
    );
    assert!(writing(&notebook, cx), "Write shows the page as it reads");
    click_at(
        &notebook,
        point(switch.left() + switch.size.width * 0.75, middle),
        cx,
    );
    assert!(!writing(&notebook, cx), "Source shows the Markdown");
}

#[gpui::test]
fn the_page_actions_menu_lists_every_page_action(cx: &mut TestAppContext) {
    let notebook = opened(cx);
    let button = bounds(&notebook, "quire-page-actions", cx);
    click_at(&notebook, button.center(), cx);
    let (name, items) = menu(&notebook, cx).expect("the page actions menu opens");
    assert_eq!(name, "Page actions");
    let exports: Vec<_> = items
        .iter()
        .filter(|item| item.starts_with("Export as PDF") || item.starts_with("Export as Word"))
        .collect();
    assert_eq!(exports.len(), 2, "{items:?}");
    assert!(
        exports.iter().all(|item| item.ends_with("… (disabled)")),
        "PDF and Word wait for AI: {items:?}"
    );
    let rest: Vec<_> = items
        .iter()
        .filter(|item| !exports.contains(item))
        .map(String::as_str)
        .collect();
    assert_eq!(
        rest,
        [
            "Copy block reference",
            "Copy block embed",
            "Move block to page",
            "Attach…",
            "History…",
            "-",
            "Export as Markdown…",
            "-",
            "Rename…",
            "Move…",
            "Delete page…",
            "-",
            "Save locally",
            "Copy Markdown",
            "-",
            "Show notebook folder",
            "Backups…",
            "Sync…",
        ]
    );
}

#[gpui::test]
fn page_actions_do_what_their_old_buttons_did(cx: &mut TestAppContext) {
    let notebook = opened(cx);
    let open = |notebook: &Notebook, cx: &mut TestAppContext| {
        let button = bounds(notebook, "quire-page-actions", cx);
        click_at(notebook, button.center(), cx);
        assert!(menu(notebook, cx).is_some(), "the menu opens");
    };

    open(&notebook, cx);
    choose(&notebook, "Copy Markdown", cx);
    wait_until(cx, "the page on the clipboard", |cx| {
        cx.read_from_clipboard()
            .and_then(|item| item.text())
            .is_some_and(|text| text == PAGE)
    });

    open(&notebook, cx);
    choose(&notebook, "Backups…", cx);
    wait_until(cx, "the backups list", |cx| {
        read(&notebook, cx, |view, cx| {
            view.backups.read(cx).listed().is_some()
        })
    });

    open(&notebook, cx);
    choose(&notebook, "Sync…", cx);
    assert!(read(&notebook, cx, |view, cx| view
        .sync
        .read(cx)
        .is_shown()));

    open(&notebook, cx);
    choose(&notebook, "History…", cx);
    wait_until(cx, "the page history", |cx| {
        read(&notebook, cx, |view, _| view.nav.page_history.is_some())
    });
}

#[gpui::test]
fn the_header_moves_the_window_beside_the_sidebar_and_holds_its_controls(cx: &mut TestAppContext) {
    let notebook = opened(cx);
    let header = bounds(&notebook, "quire-workspace-header", cx);
    let crumbs = bounds(&notebook, "quire-breadcrumb", cx);
    assert_eq!(header.top(), px(0.));
    assert_eq!(header.size.height, crate::title_bar::HEIGHT);
    assert!(
        header.left() > px(0.),
        "the sidebar reaches the window's top"
    );
    assert!(crumbs.left() > header.left() && crumbs.right() < header.right());
    if crate::title_bar::custom() {
        let lights = bounds(&notebook, "quire-title-bar", cx);
        assert_eq!(lights.top(), px(0.));
        assert!(
            lights.left() <= px(21.),
            "the strip starts where the lights do"
        );
        assert!(
            lights.right() <= header.left(),
            "the lights sit over the sidebar"
        );
    }

    notebook
        .window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(crate::app_menus::ToggleSidebar), cx)
        })
        .unwrap();
    let hidden = bounds(&notebook, "quire-workspace-header", cx);
    assert_eq!(hidden.left(), px(0.), "the header takes the whole width");
    let crumbs = bounds(&notebook, "quire-breadcrumb", cx);
    if crate::title_bar::custom() {
        assert!(
            crumbs.left() > px(84.),
            "the header leaves the traffic lights room"
        );
    }
}

#[gpui::test]
fn the_header_toggles_the_second_pane_with_its_own_toolbar(cx: &mut TestAppContext) {
    let notebook = opened(cx);
    notebook
        .window
        .update(cx, |view, window, cx| view.toggle_side(window, cx))
        .unwrap();
    wait_until(cx, "the side pane's editor", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.editor.is_some())
        })
    });
    let side = bounds(&notebook, "quire-side-view-switch", cx);
    let main = bounds(&notebook, "quire-view-switch", cx);
    assert!(side.left() > main.right(), "each pane has its own switch");
    let second = read(&notebook, cx, |view, cx| {
        let side = view.nav.side.as_ref().unwrap();
        side.editor
            .as_ref()
            .unwrap()
            .read(cx)
            .second
            .read(cx)
            .last_bounds()
            .copied()
    });
    assert!(second.is_none(), "the side pane draws one editor too");
}

#[gpui::test]
fn pages_open_in_write_as_the_reference_opens_them(cx: &mut TestAppContext) {
    cx.update(|cx| {
        crate::editor_host::init(cx).unwrap();
        cx.remove_global::<crate::editor_host::PagesOpenInSource>();
    });
    let notebook = opened(cx);
    assert!(writing(&notebook, cx), "a page opens in Write");
    notebook
        .window
        .update(cx, |view, window, cx| view.toggle_side(window, cx))
        .unwrap();
    wait_until(cx, "the side pane's editor", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .side
                .as_ref()
                .is_some_and(|side| side.editor.is_some())
        })
    });
    let side = read(&notebook, cx, |view, cx| {
        let editor = view.nav.side.as_ref().unwrap().editor.as_ref().unwrap();
        editor.read(cx).writing(cx)
    });
    assert!(side, "the second pane opens in Write too");
}
