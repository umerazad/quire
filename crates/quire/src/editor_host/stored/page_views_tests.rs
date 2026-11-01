//! Window chrome around the page (batch J): the header, status bar figures, details panel, save
//! indicator and tab dots, toolbar toggles, zoom, the README pane, the Daily notes month, and
//! the Files heading, each against the reference behavior it ports.

use super::super::page_views::{SaveState, subtitle, tidy_blocked};
use super::tests::{Notebook, caret_text, draw, open_notebook, read, wait_until};
use crate::app_menus::{
    ActualSize, ShowNotebookFolder, TogglePageDetails, ToggleSidebar, ToggleVim, ZoomIn, ZoomOut,
};
use crate::page_details::PageFacts;
use gpui::{AppContext as _, TestAppContext};
use quire_core::workspace::{BrowserView, Destination};
use std::{cell::RefCell, rc::Rc};

const PLAN: &str = "# Plan\n- one idea\n- two ^b1\n## Later\n- three\n";

fn facts(notebook: &Notebook, cx: &mut TestAppContext) -> PageFacts {
    wait_until(cx, "the page's figures", |cx| {
        read(notebook, cx, |view, cx| {
            view.page_facts(&view.page, cx).is_some()
        })
    });
    read(notebook, cx, |view, cx| {
        view.page_facts(&view.page, cx).unwrap()
    })
}

fn dispatch(notebook: &Notebook, action: impl gpui::Action, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(action), cx)
        })
        .unwrap();
    cx.run_until_parked();
}

fn drawn(notebook: &Notebook, selector: &'static str, cx: &mut TestAppContext) -> bool {
    draw(notebook, cx);
    gpui::VisualTestContext::from_window(notebook.window.into(), cx)
        .debug_bounds(selector)
        .is_some()
}

#[gpui::test]
fn the_header_names_the_kind_of_page_and_counts_blocks_and_reading_time(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("plan", "Plan.md", PLAN),
            ("day", "2026-10-31.md", "- a day\n"),
        ],
        "plan",
        cx,
    );
    let facts = facts(&notebook, cx);
    assert_eq!(facts.blocks, 3);
    assert_eq!(facts.ids, 1);
    assert_eq!(facts.words, 6);
    assert_eq!(subtitle(&facts), "3 blocks · 1 min read");
    assert_eq!(
        subtitle(&PageFacts::default()),
        "0 blocks",
        "an empty page has no reading time"
    );
    let source = read(&notebook, cx, |view, cx| {
        assert_eq!(view.page_kind("Plan"), "A PAGE IN YOUR NOTEBOOK");
        assert_eq!(view.page_kind("2026-10-31"), "DAILY NOTE");
        view.editor.as_ref().unwrap().read(cx).source.clone()
    });
    assert!(
        drawn(&notebook, "quire-page-status", cx),
        "the status bar shows the page's path and words"
    );
    assert_eq!(
        source.update(cx, |editor, cx| editor.placeholder_text(cx)),
        Some(super::super::page_views::EMPTY_PAGE.to_owned())
    );
    let buffer = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    });
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..buffer.len(), "- one more word here")], None, cx)
    });
    wait_until(cx, "the figures to follow the edit", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page_facts(&view.page, cx)
                .is_some_and(|facts| facts.words == 4 && facts.blocks == 1)
        })
    });
}

#[gpui::test]
fn details_list_headings_that_jump_and_count_references_and_ids(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("plan", "Plan.md", PLAN),
            ("other", "Other.md", "- see [[Plan]]\n"),
        ],
        "plan",
        cx,
    );
    let facts = facts(&notebook, cx);
    let headings: Vec<_> = facts.headings.iter().map(|h| h.text.as_str()).collect();
    assert_eq!(headings, ["Plan", "Later"]);
    assert!(!drawn(&notebook, "quire-page-details", cx));
    dispatch(&notebook, TogglePageDetails, cx);
    assert!(drawn(&notebook, "quire-page-details", cx));
    wait_until(cx, "the linked reference count", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .references
                .get("plan")
                .is_some_and(|references| references.read(cx).results.value.total == 1)
        })
    });
    let later = facts.headings[1].clone();
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.reveal_in_shown(
                crate::navigation::Jump::at(later.block.clone(), later.from),
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(caret_text(&notebook, false, cx), "## Later");
    dispatch(&notebook, TogglePageDetails, cx);
    assert!(!drawn(&notebook, "quire-page-details", cx));
}

/// rt's second pane is a whole page pane, with its own save icon and details panel.
#[gpui::test]
fn the_side_pane_has_its_own_save_icon_and_details(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("other", "Other.md", "- see [[Plan]]\n"),
            ("plan", "Plan.md", PLAN),
        ],
        "other",
        cx,
    );
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_beside("plan".into(), window, cx)
        })
        .unwrap();
    wait_until(cx, "the side page's figures", |cx| {
        read(&notebook, cx, |view, cx| {
            view.nav.side.as_ref().is_some_and(|side| {
                side.editor.is_some() && view.page_facts(&side.page, cx).is_some()
            })
        })
    });
    assert!(drawn(&notebook, "quire-side-save-indicator", cx));
    assert!(!drawn(&notebook, "quire-side-page-details", cx));
    super::template_tests::click(&notebook, "quire-side-details-toggle", cx);
    assert!(drawn(&notebook, "quire-side-page-details", cx));
    assert!(
        !drawn(&notebook, "quire-page-details", cx),
        "the main page's panel stays closed"
    );
    let later = read(&notebook, cx, |view, cx| {
        let side = view.nav.side.as_ref().unwrap();
        view.page_facts(&side.page, cx).unwrap().headings[1].clone()
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.reveal_in_side(
                crate::navigation::Jump::at(later.block.clone(), later.from),
                window,
                cx,
            )
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(caret_text(&notebook, true, cx), "## Later");
    super::template_tests::click(&notebook, "quire-side-details-toggle", cx);
    assert!(!drawn(&notebook, "quire-side-page-details", cx));
}

#[gpui::test]
fn the_save_icon_and_tab_dot_follow_saving(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let state = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, cx| {
            let tab = Destination::page("plan");
            let shown = SaveState::of(view.page.read(cx), cx);
            assert_eq!(view.tab_save_state(&tab, cx), shown);
            shown
        })
    };
    wait_until(cx, "the page to be saved", |cx| {
        state(cx) == Some(SaveState::Saved)
    });
    assert!(drawn(&notebook, "quire-save-indicator", cx));
    assert!(!drawn(&notebook, "quire-tab-dot-0", cx));
    let buffer = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    });
    buffer.update(cx, |buffer, cx| buffer.edit([(0..0, "Edited. ")], None, cx));
    assert_eq!(state(cx), Some(SaveState::Pending));
    assert!(drawn(&notebook, "quire-tab-dot-0", cx));
    assert_eq!(SaveState::Pending.dot(), Some(ui::Color::Warning));
    assert_eq!(SaveState::Failed.dot(), Some(ui::Color::Error));
    assert_eq!(SaveState::Saved.dot(), None);
    notebook
        .window
        .update(cx, |view, _, cx| view.save_page(cx))
        .unwrap();
    wait_until(cx, "the edit to be saved", |cx| {
        state(cx) == Some(SaveState::Saved)
    });
    assert!(!drawn(&notebook, "quire-tab-dot-0", cx));
}

#[gpui::test]
async fn the_vim_button_and_sidebar_toggle_change_what_they_name(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(paths::config_dir(), serde_json::json!({}))
        .await;
    cx.update(|cx| cx.set_global(crate::preferences::SettingsFs(fs.clone())));
    let vim =
        |cx: &mut TestAppContext| cx.update(|cx| crate::preferences::Preferences::current(cx).vim);
    assert!(vim(cx));
    dispatch(&notebook, ToggleVim, cx);
    assert!(!vim(cx));
    dispatch(&notebook, ToggleVim, cx);
    assert!(vim(cx));

    assert!(drawn(&notebook, "quire-templates-entry", cx));
    dispatch(&notebook, ToggleSidebar, cx);
    assert!(read(&notebook, cx, |view, _| view
        .nav
        .workspace
        .chrome
        .sidebar_hidden));
    assert!(!drawn(&notebook, "quire-templates-entry", cx));
    dispatch(&notebook, ToggleSidebar, cx);
    assert!(drawn(&notebook, "quire-templates-entry", cx));
}

#[gpui::test]
fn zoom_scales_editor_and_interface_text_until_actual_size(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let sizes = |cx: &mut TestAppContext| {
        draw(&notebook, cx);
        notebook
            .window
            .update(cx, |_, window, cx| {
                let theme = <theme_settings::ThemeSettings as settings::Settings>::get_global(cx);
                (
                    f32::from(theme.buffer_font_size(cx)),
                    f32::from(theme.ui_font_size(cx)),
                    f32::from(window.rem_size()),
                )
            })
            .unwrap()
    };
    let (buffer, ui, rem) = sizes(cx);
    assert_eq!(rem, ui, "the interface follows the UI font size");
    dispatch(&notebook, ZoomIn, cx);
    dispatch(&notebook, ZoomIn, cx);
    assert_eq!(sizes(cx), (buffer + 2., ui + 2., ui + 2.));
    dispatch(&notebook, ZoomOut, cx);
    assert_eq!(sizes(cx), (buffer + 1., ui + 1., ui + 1.));
    dispatch(&notebook, ActualSize, cx);
    assert_eq!(sizes(cx), (buffer, ui, rem));
}

#[gpui::test]
fn zoom_reaches_the_settings_help_migration_and_readme_windows(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let data = tempfile::tempdir().unwrap();
    let windows: [gpui::AnyWindowHandle; 4] = [
        cx.add_window(crate::preferences::view::PreferencesWindow::new)
            .into(),
        cx.add_window(|window, cx| crate::help::view::HelpWindow::new(false, window, cx))
            .into(),
        cx.add_window(|window, cx| {
            crate::rt_migration::view::MigrationWindow::new(
                data.path().to_owned(),
                None,
                window,
                cx,
            )
        })
        .into(),
        cx.add_window(|window, cx| {
            let page = crate::showcase::sample(cx);
            crate::showcase::ShowcaseWindow::new(page, false, window, cx)
        })
        .into(),
    ];
    let rems = |cx: &mut TestAppContext| {
        windows.map(|handle| {
            cx.update_window(handle, |_, window, cx| {
                window.draw(cx).clear(cx);
                f32::from(window.rem_size())
            })
            .unwrap()
        })
    };
    let ui = read(&notebook, cx, |_, cx| {
        f32::from(
            <theme_settings::ThemeSettings as settings::Settings>::get_global(cx).ui_font_size(cx),
        )
    });
    assert_eq!(rems(cx), [ui; 4]);
    cx.dispatch_action(windows[1], ZoomIn);
    cx.run_until_parked();
    assert_eq!(
        rems(cx),
        [ui + 1.; 4],
        "zoom from Help reaches every window"
    );
    cx.dispatch_action(windows[0], ActualSize);
    cx.run_until_parked();
    assert_eq!(rems(cx), [ui; 4]);
}

#[gpui::test]
fn show_notebook_folder_reveals_where_the_notebook_lives(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let revealed = Rc::new(RefCell::new(Vec::new()));
    let recorder = revealed.clone();
    cx.update(|cx| {
        cx.set_global(super::menu_views::RevealFiles(Rc::new(move |path| {
            recorder.borrow_mut().push(path.to_owned())
        })))
    });
    dispatch(&notebook, ShowNotebookFolder, cx);
    assert_eq!(revealed.borrow()[0], notebook.path);

    let opened = super::folder_tests::open_folder(&[("A.md", "- alpha\n")], "A.md", cx);
    dispatch(&opened.notebook, ShowNotebookFolder, cx);
    assert_eq!(revealed.borrow()[1], opened.folder.path());
}

#[gpui::test]
fn tidy_says_why_it_cannot_run(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("day", "2026-10-31.md", "- a day\n")], "day", cx);
    let reason = read(&notebook, cx, |view, cx| {
        tidy_blocked(view.page.read(cx), cx)
    });
    assert_eq!(reason, Some(crate::ai::settings::UNTESTED));
}

#[gpui::test]
fn readme_opens_beside_the_page_in_place_of_help(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let showcase_windows = |cx: &mut TestAppContext| {
        cx.update(|cx| {
            cx.windows()
                .into_iter()
                .filter(|window| {
                    window
                        .downcast::<crate::showcase::ShowcaseWindow>()
                        .is_some()
                })
                .count()
        })
    };
    dispatch(&notebook, crate::showcase::OpenShowcase, cx);
    assert_eq!(showcase_windows(cx), 0, "README opens beside the page");
    assert!(read(&notebook, cx, |view, _| view
        .chrome
        .showcase
        .is_some()));
    dispatch(&notebook, crate::help::OpenHelp, cx);
    read(&notebook, cx, |view, _| {
        assert!(view.help.is_some());
        assert!(view.chrome.showcase.is_none(), "Help takes the pane");
    });
    dispatch(&notebook, crate::showcase::OpenShowcase, cx);
    let showcase = read(&notebook, cx, |view, _| {
        assert!(view.help.is_none(), "README takes the pane back");
        view.chrome.showcase.clone().unwrap()
    });
    showcase.update(cx, |_, cx| cx.emit(crate::showcase::CloseShowcase));
    cx.run_until_parked();
    assert!(read(&notebook, cx, |view, _| view
        .chrome
        .showcase
        .is_none()));
}

#[gpui::test]
fn the_readme_pane_resolves_in_its_own_notebook_never_this_one(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    dispatch(&notebook, crate::showcase::OpenShowcase, cx);
    let (pane_hosts, notebook_host, client) = read(&notebook, cx, |view, cx| {
        let showcase = view.chrome.showcase.clone().unwrap();
        let hosts = showcase.read(cx).notebook.read(cx).editors().map(|editor| {
            crate::references::host(editor.read(cx))
                .unwrap()
                .entity_id()
        });
        (
            hosts,
            view.links.host.entity_id(),
            view.index_client(cx).unwrap(),
        )
    });
    assert!(
        pane_hosts.iter().all(|host| *host != notebook_host),
        "the sample's links never resolve in the notebook"
    );
    let search = |query: &str| {
        futures::executor::block_on(crate::query_index::ask(client.search(query.into())))
            .unwrap()
            .into_iter()
            .map(|hit| hit.source.path)
            .collect::<Vec<_>>()
    };
    wait_until(cx, "the notebook's own page in its search", |_| {
        search("idea") == ["Plan.md"]
    });
    assert!(
        search("Rendering").is_empty(),
        "the sample stays out of the notebook's queries"
    );
}

#[gpui::test]
fn daily_notes_show_one_remembered_month(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("sep", "2026-09-30.md", "- end of september\n"),
            ("oct1", "2026-10-01.md", "- first\n"),
            ("oct31", "2026-10-31.md", "- last\n"),
            ("plan", "Plan.md", PLAN),
        ],
        "plan",
        cx,
    );
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(
                Destination::Browser {
                    view: BrowserView::Daily,
                },
                window,
                cx,
            )
        })
        .unwrap();
    let titles = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, cx| {
            view.browser_rows(cx)
                .into_iter()
                .map(|row| row.title)
                .collect::<Vec<_>>()
        })
    };
    wait_until(cx, "the daily pages", |cx| titles(cx).len() == 3);
    notebook
        .window
        .update(cx, |view, _, cx| view.set_daily_month("2026-10".into(), cx))
        .unwrap();
    assert_eq!(titles(cx), ["2026-10-31", "2026-10-01"]);
    read(&notebook, cx, |view, _| {
        assert_eq!(view.nav.workspace.chrome.daily_month, "2026-10")
    });
    draw(&notebook, cx);
    notebook
        .window
        .update(cx, |view, _, cx| view.set_daily_month(String::new(), cx))
        .unwrap();
    assert_eq!(titles(cx).len(), 3);
}

#[gpui::test]
fn the_files_heading_collapses_and_refresh_lists_new_files(cx: &mut TestAppContext) {
    let opened = super::folder_tests::open_folder(&[("A.md", "- alpha\n")], "A.md", cx);
    let notebook = &opened.notebook;
    let listed = |name: &str, cx: &mut TestAppContext| {
        read(notebook, cx, |view, _| {
            view.file_rows().iter().any(|row| row.path == name)
        })
    };
    wait_until(cx, "Files to list A.md", |cx| listed("A.md", cx));
    std::fs::write(opened.folder.path().join("Fresh.md"), "- new\n").unwrap();
    notebook
        .window
        .update(cx, |view, _, cx| view.refresh_files(cx))
        .unwrap();
    wait_until(cx, "Refresh to list Fresh.md", |cx| listed("Fresh.md", cx));

    notebook
        .window
        .update(cx, |view, _, cx| view.toggle_files(cx))
        .unwrap();
    assert!(read(notebook, cx, |view, _| view
        .nav
        .workspace
        .chrome
        .files_collapsed));
    draw(notebook, cx);
    notebook
        .window
        .update(cx, |view, _, cx| view.toggle_files(cx))
        .unwrap();
    assert!(!read(notebook, cx, |view, _| view
        .nav
        .workspace
        .chrome
        .files_collapsed));
}

#[gpui::test]
fn the_view_and_file_menus_offer_the_window_chrome(cx: &mut TestAppContext) {
    let _notebook = open_notebook(&[("plan", "Plan.md", PLAN)], "plan", cx);
    let all: Vec<(String, String, String, bool)> = cx
        .update(|cx| crate::app_menus::menus(cx))
        .into_iter()
        .flat_map(|menu| {
            let menu_name = menu.name.to_string();
            menu.items.into_iter().filter_map(move |item| match item {
                gpui::MenuItem::Action {
                    name,
                    action,
                    checked,
                    ..
                } => Some((
                    menu_name.clone(),
                    name.to_string(),
                    action.name().to_owned(),
                    checked,
                )),
                _ => None,
            })
        })
        .collect();
    for (menu, name, action) in [
        ("View", "Actual Size", "quire_window::ActualSize"),
        ("View", "Zoom In", "quire_window::ZoomIn"),
        ("View", "Zoom Out", "quire_window::ZoomOut"),
        ("View", "Sidebar", "quire_window::ToggleSidebar"),
        ("View", "Page Details", "quire_window::TogglePageDetails"),
        ("View", "Vim Bindings", "quire_window::ToggleVim"),
        (
            "File",
            "Show Notebook Folder",
            "quire_window::ShowNotebookFolder",
        ),
    ] {
        assert!(
            all.iter()
                .any(|(m, n, a, _)| m == menu && n == name && a == action),
            "{menu} > {name} -> {action} missing from {all:?}"
        );
    }
    let vim = all.iter().find(|(_, name, _, _)| name == "Vim Bindings");
    assert!(vim.is_some_and(|(_, _, _, checked)| *checked));
}
