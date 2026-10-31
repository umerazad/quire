//! Pointer menus for Files, the sidebar, and tabs (NR16.2): their names, items, order, and
//! disabled entries match the reference, and choosing an item does what its keys do.

use super::file_tests::summary;
use super::move_tests::select_file;
use super::tests::{Notebook, draw, open_notebook, read, wait_until};
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};
use quire_core::workspace::{BrowserView, Destination};

const PAGES: [(&str, &str, &str); 4] = [
    ("o", "Old.md", "- Original\n"),
    ("k", "Keep.md", "- keep\n"),
    ("pa", "Projects/A.md", "- a\n"),
    ("pb", "Projects/B.md", "- b\n"),
];

pub(super) fn menu(notebook: &Notebook, cx: &mut TestAppContext) -> Option<(String, Vec<String>)> {
    read(notebook, cx, |view, _| {
        view.nav
            .menu
            .as_ref()
            .map(|menu| (menu.name.to_string(), menu.summary.clone()))
    })
}

/// Picks `label` from the open menu with the keyboard: Down to it past disabled items, then
/// Enter.
pub(super) fn choose(notebook: &Notebook, label: &str, cx: &mut TestAppContext) {
    let (_, items) = menu(notebook, cx).expect("an open menu");
    let enabled: Vec<&String> = items
        .iter()
        .filter(|item| *item != "-" && !item.ends_with("(disabled)"))
        .collect();
    let at = enabled
        .iter()
        .position(|item| item.trim_start_matches("[x] ").trim_start_matches("[ ] ") == label)
        .unwrap_or_else(|| panic!("{label} is not an enabled item of {items:?}"));
    draw(notebook, cx);
    for _ in 0..=at {
        cx.simulate_keystrokes(notebook.window.into(), "down");
    }
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    cx.run_until_parked();
    draw(notebook, cx);
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

fn files_focused(notebook: &Notebook, cx: &mut TestAppContext) -> bool {
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.nav
                .browsers
                .files_focus
                .as_ref()
                .is_some_and(|focus| focus.contains_focused(window, cx))
        })
        .unwrap()
}

pub(super) fn right_click_file(notebook: &Notebook, path: &str, cx: &mut TestAppContext) {
    draw(notebook, cx);
    let position = notebook
        .window
        .update(cx, |view, window, _| {
            let index = view
                .file_rows()
                .iter()
                .position(|row| row.path == path)
                .unwrap_or_else(|| panic!("{path} is not in Files"));
            view.file_row_position(index, window) - point(px(0.), px(4.))
        })
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    visual.simulate_mouse_down(position, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(position, MouseButton::Right, Modifiers::default());
    draw(notebook, cx);
}

fn right_click_tab(notebook: &Notebook, index: usize, cx: &mut TestAppContext) {
    draw(notebook, cx);
    let selector: &'static str = Box::leak(format!("quire-tab-{index}").into_boxed_str());
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("tab {index} is not drawn"));
    visual.simulate_mouse_down(bounds.center(), MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(bounds.center(), MouseButton::Right, Modifiers::default());
    draw(notebook, cx);
}

fn dialog_name(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
    read(notebook, cx, |view, cx| {
        view.nav
            .dialog
            .as_ref()
            .and_then(|dialog| dialog.read(cx).name(cx))
    })
}

fn escape(notebook: &Notebook, cx: &mut TestAppContext) {
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    draw(notebook, cx);
}

#[gpui::test]
fn files_menus_open_from_shift_f10_and_close_on_escape_back_to_files(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    select_file(&notebook, "Old.md", cx);
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "shift-f10");
    assert_eq!(
        menu(&notebook, cx),
        Some((
            "Page actions for Old".into(),
            strings(&[
                "Open",
                "Open beside",
                "Copy link",
                "Add to favorites",
                "-",
                "New note here",
                "-",
                "Rename",
                "Move to…",
                "-",
                "Delete page…",
            ])
        ))
    );
    escape(&notebook, cx);
    assert!(menu(&notebook, cx).is_none());
    assert!(files_focused(&notebook, cx));

    select_file(&notebook, "Projects", cx);
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "shift-f10");
    assert_eq!(
        menu(&notebook, cx),
        Some((
            "Folder actions for Projects".into(),
            strings(&[
                "New note here",
                "-",
                "Rename or move…",
                "-",
                "Delete folder…",
            ])
        ))
    );
    choose(&notebook, "Rename or move…", cx);
    assert!(menu(&notebook, cx).is_none());
    wait_until(cx, "the folder dialog", |cx| {
        dialog_name(&notebook, cx) == Some("Projects".into())
    });
}

#[gpui::test]
fn files_page_menu_items_do_what_their_keys_do(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    right_click_file(&notebook, "Old.md", cx);
    assert_eq!(
        menu(&notebook, cx).map(|(name, _)| name),
        Some("Page actions for Old".into())
    );
    assert!(read(&notebook, cx, |view, _| {
        view.selected_file() == Some(crate::file_actions::Target::Page("o".into()))
    }));
    choose(&notebook, "Copy link", cx);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("[[Old]]".into())
    );

    right_click_file(&notebook, "Old.md", cx);
    choose(&notebook, "Add to favorites", cx);
    assert!(read(&notebook, cx, |view, _| {
        view.nav.workspace.favorites == [Destination::page("o")]
    }));
    right_click_file(&notebook, "Old.md", cx);
    assert!(
        menu(&notebook, cx)
            .unwrap()
            .1
            .contains(&"Remove from favorites".to_owned())
    );
    escape(&notebook, cx);

    // Rename and Move open the dialogs F2 and Shift-F2 open.
    select_file(&notebook, "Old.md", cx);
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "f2");
    wait_until(cx, "the F2 dialog", |cx| {
        dialog_name(&notebook, cx).is_some()
    });
    let renamed_by_key = dialog_name(&notebook, cx);
    escape(&notebook, cx);
    right_click_file(&notebook, "Old.md", cx);
    choose(&notebook, "Rename", cx);
    wait_until(cx, "the menu's rename dialog", |cx| {
        dialog_name(&notebook, cx).is_some()
    });
    assert_eq!(dialog_name(&notebook, cx), renamed_by_key);
    assert_eq!(renamed_by_key, Some("Old".into()));
    escape(&notebook, cx);
    right_click_file(&notebook, "Old.md", cx);
    choose(&notebook, "Move to…", cx);
    wait_until(cx, "the move dialog", |cx| {
        dialog_name(&notebook, cx) == Some("Old.md".into())
    });
    escape(&notebook, cx);

    right_click_file(&notebook, "Old.md", cx);
    choose(&notebook, "Delete page…", cx);
    assert_eq!(summary(&notebook, cx).0, "Delete Old?");
    escape(&notebook, cx);

    right_click_file(&notebook, "Old.md", cx);
    choose(&notebook, "Open", cx);
    wait_until(cx, "Old to open", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav.workspace.active == Some(Destination::page("o"))
        })
    });
}

#[gpui::test]
fn new_note_here_creates_the_page_in_the_clicked_folder(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    right_click_file(&notebook, "Projects", cx);
    choose(&notebook, "New note here", cx);
    assert!(read(&notebook, cx, |view, _| view.nav.palette.is_some()));
    cx.simulate_input(notebook.window.into(), "Fresh");
    wait_until(cx, "the create row", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav.palette.as_ref().is_some_and(|picker| {
                let picker = picker.read(cx);
                picker.query(cx) == "Fresh" && picker.delegate.settled()
            })
        })
    });
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the new page in Projects", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav
                .pages
                .iter()
                .any(|listed| listed.page.path == "Projects/Fresh.md")
        })
    });
}

#[gpui::test]
fn tab_menus_close_around_the_tab_and_disable_what_cannot_close(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    for page in ["o", "pa"] {
        super::file_tests::visit(&notebook, Destination::page(page), cx);
    }
    let tabs =
        |cx: &mut TestAppContext| read(&notebook, cx, |view, _| view.nav.workspace.tabs.clone());
    let pages = |ids: &[&str]| {
        ids.iter()
            .map(|id| Destination::page(*id))
            .collect::<Vec<_>>()
    };
    assert_eq!(tabs(cx), pages(&["k", "o", "pa"]));
    right_click_tab(&notebook, 1, cx);
    assert_eq!(
        menu(&notebook, cx),
        Some((
            "Tab actions for Old".into(),
            strings(&[
                "Close",
                "Close others",
                "Close tabs to the left",
                "Close tabs to the right",
                "Close all",
                "-",
                "Add to favorites",
                "Open beside",
                "Copy link",
                "-",
                "Rename or move…",
                "Delete page…",
            ])
        ))
    );
    choose(&notebook, "Close tabs to the right", cx);
    assert_eq!(tabs(cx), pages(&["k", "o"]));
    // The active tab closed, so the tab the menu came from shows.
    assert!(read(&notebook, cx, |view, _| {
        view.nav.workspace.active == Some(Destination::page("o"))
    }));
    right_click_tab(&notebook, 1, cx);
    choose(&notebook, "Close tabs to the left", cx);
    assert_eq!(tabs(cx), pages(&["o"]));
    right_click_tab(&notebook, 0, cx);
    let (_, items) = menu(&notebook, cx).unwrap();
    assert_eq!(
        items[..5],
        strings(&[
            "Close",
            "Close others (disabled)",
            "Close tabs to the left (disabled)",
            "Close tabs to the right (disabled)",
            "Close all",
        ])
    );
    choose(&notebook, "Add to favorites", cx);
    assert!(read(&notebook, cx, |view, _| {
        view.nav.workspace.favorites == [Destination::page("o")]
    }));
    right_click_tab(&notebook, 0, cx);
    choose(&notebook, "Close all", cx);
    assert_eq!(
        tabs(cx),
        [Destination::Browser {
            view: BrowserView::Pages
        }]
    );
    // A browser tab has no page actions.
    right_click_tab(&notebook, 0, cx);
    let (name, items) = menu(&notebook, cx).unwrap();
    assert_eq!(name, "Tab actions for Pages");
    assert_eq!(items.last().map(String::as_str), Some("Add to favorites"));
    escape(&notebook, cx);
    assert!(menu(&notebook, cx).is_none());
}

#[gpui::test]
fn shift_f10_outside_files_opens_the_active_tab_menu(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.nav
                .browsers
                .list_focus
                .get_or_insert_with(|| cx.focus_handle());
            let focus = view.nav.browsers.list_focus.clone().unwrap();
            window.focus(&focus, cx);
        })
        .unwrap();
    super::file_tests::visit(
        &notebook,
        Destination::Browser {
            view: BrowserView::Pages,
        },
        cx,
    );
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "shift-f10");
    assert_eq!(
        menu(&notebook, cx).map(|(name, _)| name),
        Some("Tab actions for Pages".into())
    );
}

#[gpui::test]
fn recent_and_favorite_menus_act_on_their_page(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "k", cx);
    super::file_tests::visit(&notebook, Destination::page("o"), cx);
    super::file_tests::visit(&notebook, Destination::page("k"), cx);
    let open = |sidebar, cx: &mut TestAppContext| {
        notebook
            .window
            .update(cx, |view, window, cx| {
                view.deploy_sidebar_menu("o".into(), sidebar, point(px(10.), px(10.)), window, cx)
            })
            .unwrap();
        draw(&notebook, cx);
    };
    assert!(read(&notebook, cx, |view, _| {
        view.nav.workspace.recent.contains(&Destination::page("o"))
    }));
    open(super::menu_views::Sidebar::Recent, cx);
    assert_eq!(
        menu(&notebook, cx),
        Some((
            "Page actions for Old".into(),
            strings(&[
                "Open",
                "Open beside",
                "Copy link",
                "Add to favorites",
                "-",
                "Rename or move…",
                "Remove from recent",
                "-",
                "Delete page…",
            ])
        ))
    );
    choose(&notebook, "Remove from recent", cx);
    assert!(read(&notebook, cx, |view, _| {
        !view.nav.workspace.recent.contains(&Destination::page("o"))
    }));
    open(super::menu_views::Sidebar::Favorite, cx);
    assert!(
        !menu(&notebook, cx)
            .unwrap()
            .1
            .contains(&"Remove from recent".to_owned())
    );
    choose(&notebook, "Open", cx);
    wait_until(cx, "Old to open", |cx| {
        read(&notebook, cx, |view, _| {
            view.nav.workspace.active == Some(Destination::page("o"))
        })
    });
    open(super::menu_views::Sidebar::Recent, cx);
    choose(&notebook, "Delete page…", cx);
    assert_eq!(summary(&notebook, cx).0, "Delete Old?");
}
