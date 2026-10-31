//! Closing, quitting, and switching profiles with slow, failed, and unavailable work; the native
//! menu; and a second launch handed to the running Quire.

use super::super::*;
use super::folder_tests::open_folder;
use super::tests::{Notebook, read, wait_until};
use crate::lifecycle::{
    Activity, CLOSE_ANYWAY, FAILED, KEEP_EDITING, KEEP_WAITING, PATIENCE, POLL, STILL_WORKING,
};
use gpui::{TestAppContext, WindowHandle};
use quire_store::profiles::{Notebook as Location, ProfileId};
use std::{cell::RefCell, fs, path::PathBuf, rc::Rc};

/// A fake service whose state the test sets.
fn fake_service(notebook: &Notebook, cx: &mut TestAppContext) -> Rc<RefCell<Activity>> {
    let state = Rc::new(RefCell::new(Activity::Idle));
    let shared = state.clone();
    notebook
        .window
        .update(cx, |view, _, _| {
            view.close
                .services
                .borrow_mut()
                .push(Rc::new(move |_| shared.borrow().clone()));
        })
        .unwrap();
    state
}

fn is_open(window: WindowHandle<StoredWindow>, cx: &mut TestAppContext) -> bool {
    window.update(cx, |_, _, _| ()).is_ok()
}

fn request_close(notebook: &Notebook, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), crate::app_menus::CloseWindow);
    cx.run_until_parked();
}

fn advance(by: std::time::Duration, cx: &mut TestAppContext) {
    cx.executor().advance_clock(by);
    cx.run_until_parked();
}

/// Waits for a profile switch one poll at a time. `wait_until` moves the clock 200 ms per step,
/// so under load the switch's 10 s patience could run out before the storage thread saves.
fn wait_for_switch(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    cx.executor().allow_parking();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        advance(POLL, cx);
        if done(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}; last notice: {}",
            notice(cx)
        );
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
}

fn type_into(notebook: &Notebook, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(notebook.window.into(), keys);
    cx.run_until_parked();
}

fn page_text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let persistence = read(notebook, cx, |view, cx| {
        view.page.read(cx).persistence().cloned().unwrap()
    });
    cx.update(|cx| persistence.read(cx).buffer().read(cx).text())
}

fn notice(cx: &mut TestAppContext) -> String {
    cx.update(|cx| crate::outline::notice(cx))
        .map(|notice| notice.to_string())
        .unwrap_or_default()
}

#[gpui::test]
fn a_close_waits_quietly_for_slow_work_and_then_finishes_on_its_own(cx: &mut TestAppContext) {
    let opened = open_folder(&[("A.md", "- a\n")], "A.md", cx);
    let notebook = &opened.notebook;
    let slow = fake_service(notebook, cx);
    *slow.borrow_mut() = Activity::Busy("A slow export is running.".into());
    request_close(notebook, cx);
    advance(PATIENCE / 2, cx);
    assert!(is_open(notebook.window, cx));
    assert!(!cx.has_pending_prompt());
    *slow.borrow_mut() = Activity::Idle;
    advance(POLL, cx);
    assert!(!is_open(notebook.window, cx));
}

#[gpui::test]
fn an_unavailable_service_is_explained_after_patience_and_close_anyway_closes(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(&[("A.md", "- a\n")], "A.md", cx);
    let notebook = &opened.notebook;
    let stalled = fake_service(notebook, cx);
    *stalled.borrow_mut() = Activity::Busy("The export service does not answer.".into());
    request_close(notebook, cx);
    advance(PATIENCE, cx);
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(message, STILL_WORKING);
    assert!(
        detail.starts_with("The export service does not answer."),
        "{detail}"
    );
    assert!(
        detail.contains("Typing not yet recorded is lost"),
        "{detail}"
    );
    cx.simulate_prompt_answer(KEEP_WAITING);
    cx.run_until_parked();
    assert!(is_open(notebook.window, cx));
    advance(PATIENCE / 2, cx);
    assert!(!cx.has_pending_prompt());
    advance(PATIENCE, cx);
    assert!(cx.has_pending_prompt());
    cx.simulate_prompt_answer(CLOSE_ANYWAY);
    cx.run_until_parked();
    assert!(!is_open(notebook.window, cx));
}

#[gpui::test]
fn a_failed_service_asks_at_once_and_keep_editing_keeps_the_window_and_its_text(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(&[("A.md", "- a\n")], "A.md", cx);
    let notebook = &opened.notebook;
    let failed = fake_service(notebook, cx);
    *failed.borrow_mut() = Activity::Failed("The export service failed.".into());
    request_close(notebook, cx);
    let (message, _) = cx.pending_prompt().unwrap();
    assert_eq!(message, FAILED);
    cx.simulate_prompt_answer(KEEP_EDITING);
    cx.run_until_parked();
    assert!(is_open(notebook.window, cx));
    let error = read(notebook, cx, |view, _| view.close_error.clone()).unwrap();
    assert!(error.starts_with("The export service failed."), "{error}");
    assert!(read(notebook, cx, |view, _| view.closing.get().is_none()));

    *failed.borrow_mut() = Activity::Busy("A backup is running.".into());
    assert!(!cx.simulate_native_quit_request());
    *failed.borrow_mut() = Activity::Failed("The backup drive was unplugged.".into());
    advance(POLL, cx);
    cx.simulate_prompt_answer(CLOSE_ANYWAY);
    cx.run_until_parked();
    assert_eq!(cx.platform_quit_count(), 1);
    assert!(cx.simulate_native_quit_request());
}

#[gpui::test]
fn closing_anyway_after_storage_stops_keeps_saved_text_for_the_next_launch(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(&[("A.md", "- saved\n")], "A.md", cx);
    let notebook = &opened.notebook;
    type_into(notebook, "i x escape", cx);
    wait_until(cx, "the edit to save", |_| {
        fs::read_to_string(opened.folder.path().join("A.md")).unwrap() == "x- saved\n"
    });
    let persistence = read(notebook, cx, |view, cx| {
        view.page.read(cx).persistence().cloned().unwrap()
    });
    crate::page_persistence::tests::stop_storage(&persistence, cx);
    type_into(notebook, "i y escape", cx);
    request_close(notebook, cx);
    wait_until(cx, "the failure question", |cx| cx.has_pending_prompt());
    assert_eq!(cx.pending_prompt().unwrap().0, FAILED);
    cx.simulate_prompt_answer(CLOSE_ANYWAY);
    cx.run_until_parked();
    assert!(!is_open(notebook.window, cx));

    let state = notebook.path.parent().unwrap().to_owned();
    let mut reopened = None;
    wait_until(cx, "the notebook lock to be released", |_| {
        reopened =
            NotebookSession::open_folder(opened.folder.path().into(), state.clone(), None).ok();
        reopened.is_some()
    });
    let mut session = reopened.unwrap();
    let page =
        cx.update(|cx| session.open_page(quire_store::sync::format::page_id_for_path("A.md"), cx));
    crate::notebook_session::tests::wait(&page, cx);
    cx.update(|cx| {
        let loaded = page.read(cx).loaded().unwrap();
        assert_eq!(
            loaded.analysis.read(cx).source.read(cx).snapshot().text(),
            "x- saved\n"
        );
    });
}

/// Opens folder notebooks the way profile switching does, with state kept in test folders.
fn open_folders_in_tests(cx: &mut TestAppContext) -> Rc<RefCell<Vec<tempfile::TempDir>>> {
    let states = Rc::new(RefCell::new(Vec::new()));
    let kept = states.clone();
    cx.update(|cx| {
        cx.set_global(super::folder::OpenFolder(Rc::new(
            move |root: PathBuf, notices: Vec<String>, cx: &mut App| {
                let state = tempfile::tempdir().unwrap();
                let path = state.path().to_owned();
                kept.borrow_mut().push(state);
                super::folder::FolderOptions::new(root, notices, Some(path)).open(cx)
            },
        )))
    });
    states
}

fn folder(files: &[(&str, &str)]) -> tempfile::TempDir {
    let folder = tempfile::tempdir().unwrap();
    for (path, text) in files {
        fs::write(folder.path().join(path), text).unwrap();
    }
    folder
}

fn location(folder: &std::path::Path) -> Location {
    Location::Folder {
        root: fs::canonicalize(folder).unwrap(),
    }
}

fn stored_windows(cx: &mut TestAppContext) -> Vec<WindowHandle<StoredWindow>> {
    cx.update(|cx| super::profiles::stored_windows(cx))
}

fn shown_root(window: WindowHandle<StoredWindow>, cx: &mut TestAppContext) -> PathBuf {
    window
        .update(cx, |view, _, _| {
            view.location.clone().unwrap().root().to_owned()
        })
        .unwrap()
}

#[gpui::test]
fn switching_profiles_saves_pending_edits_then_opens_the_profile_notebook(cx: &mut TestAppContext) {
    let opened = open_folder(&[("A.md", "- current\n")], "A.md", cx);
    let _states = open_folders_in_tests(cx);
    let work = folder(&[("W.md", "- work\n")]);
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(ProfileId::Work, location(work.path()))
        })
        .unwrap()
    });
    assert_eq!(
        cx.update(|cx| crate::profiles::active(cx)),
        ProfileId::Current
    );
    type_into(&opened.notebook, "i x escape", cx);
    cx.dispatch_action(
        opened.notebook.window.into(),
        crate::profiles::UseWorkProfile,
    );
    wait_for_switch(cx, "the Work notebook to open", |cx| {
        stored_windows(cx).len() == 1 && !is_open(opened.notebook.window, cx)
    });
    assert_eq!(
        fs::read_to_string(opened.folder.path().join("A.md")).unwrap(),
        "x- current\n"
    );
    assert_eq!(cx.update(|cx| crate::profiles::active(cx)), ProfileId::Work);
    let shown = stored_windows(cx)[0];
    assert_eq!(
        shown_root(shown, cx),
        fs::canonicalize(work.path()).unwrap()
    );
    assert_eq!(notice(cx), "Switched to the Work profile.");
    let menus = cx.update(|cx| crate::app_menus::menus(cx));
    let checked = profile_items(&menus);
    assert_eq!(
        checked,
        [
            ("Current".to_owned(), false),
            ("Work".to_owned(), true),
            ("Personal".to_owned(), false)
        ]
    );

    cx.dispatch_action(shown.into(), crate::profiles::UseCurrentProfile);
    wait_for_switch(cx, "the Current notebook to open again", |cx| {
        stored_windows(cx).len() == 1 && !is_open(shown, cx)
    });
    assert_eq!(
        shown_root(stored_windows(cx)[0], cx),
        fs::canonicalize(opened.folder.path()).unwrap()
    );
}

fn profile_items(menus: &[gpui::Menu]) -> Vec<(String, bool)> {
    menus
        .iter()
        .find(|menu| menu.name == "File")
        .and_then(|file| {
            file.items.iter().find_map(|item| match item {
                gpui::MenuItem::Submenu(menu) if menu.name == "Profile" => Some(menu),
                _ => None,
            })
        })
        .map(|menu| {
            menu.items
                .iter()
                .filter_map(|item| match item {
                    gpui::MenuItem::Action { name, checked, .. } => {
                        Some((name.to_string(), *checked))
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[gpui::test]
fn a_switch_that_cannot_record_pages_changes_nothing_and_keeps_the_edit(cx: &mut TestAppContext) {
    let opened = open_folder(&[("A.md", "- current\n")], "A.md", cx);
    let _states = open_folders_in_tests(cx);
    let work = folder(&[("W.md", "- work\n")]);
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(ProfileId::Work, location(work.path()))
        })
        .unwrap()
    });
    let persistence = read(&opened.notebook, cx, |view, cx| {
        view.page.read(cx).persistence().cloned().unwrap()
    });
    crate::page_persistence::tests::stop_storage(&persistence, cx);
    type_into(&opened.notebook, "i x escape", cx);
    cx.dispatch_action(
        opened.notebook.window.into(),
        crate::profiles::UseWorkProfile,
    );
    wait_until(cx, "the switch to give up", |cx| {
        notice(cx).contains("did not switch")
    });
    let message = notice(cx);
    assert!(
        message.starts_with(
            "Quire did not switch to the Work notebook: Some pages could not be recorded"
        ) && message.ends_with("Your edits stay open in this window."),
        "{message}"
    );
    assert!(is_open(opened.notebook.window, cx));
    assert_eq!(stored_windows(cx).len(), 1);
    assert_eq!(
        cx.update(|cx| crate::profiles::active(cx)),
        ProfileId::Current
    );
    assert_eq!(page_text(&opened.notebook, cx), "x- current\n");
}

#[gpui::test]
fn a_switch_into_an_overlapping_folder_or_while_a_backup_runs_is_refused(cx: &mut TestAppContext) {
    let opened = open_folder(&[("A.md", "- current\n")], "A.md", cx);
    let inside = opened.folder.path().join("nested");
    fs::create_dir(&inside).unwrap();
    cx.update(|cx| super::profiles::open_in_profile(ProfileId::Work, location(&inside), cx));
    assert_eq!(
        notice(cx),
        "Quire did not switch to the Work notebook: This folder overlaps the Current notebook. Choose a separate folder for Work."
    );
    let backups = tempfile::tempdir().unwrap();
    let refused = cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_backup_folder(ProfileId::Work, &opened.folder.path().join("backups"))
        })
    });
    assert!(refused.unwrap_err().contains("overlaps a live notebook"));
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_backup_folder(ProfileId::Current, backups.path())
        })
        .unwrap()
    });
    opened
        .notebook
        .window
        .update(cx, |view, _, cx| {
            view.backups
                .update(cx, |backups, cx| backups.back_up(false, cx))
        })
        .unwrap();
    let work = folder(&[("W.md", "- work\n")]);
    cx.update(|cx| super::profiles::open_in_profile(ProfileId::Work, location(work.path()), cx));
    assert!(
        notice(cx).contains("a backup or Git sync is still running"),
        "{}",
        notice(cx)
    );
    assert_eq!(
        cx.update(|cx| crate::profiles::active(cx)),
        ProfileId::Current
    );
    assert_eq!(
        cx.update(|cx| crate::profiles::get(ProfileId::Work, cx).notebook),
        None
    );
}

#[gpui::test]
fn reopening_a_profile_notebook_shows_the_page_it_last_showed(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let notes = folder(&[("A.md", "- a\n"), ("B.md", "- b\n")]);
    let state = tempfile::tempdir().unwrap();
    let kept = state.path().to_owned();
    cx.update(|cx| {
        cx.set_global(super::folder::OpenFolder(Rc::new(
            move |root: PathBuf, notices: Vec<String>, cx: &mut App| {
                super::folder::FolderOptions::new(root, notices, Some(kept.clone())).open(cx)
            },
        )))
    });
    let root = location(notes.path());
    cx.update(|cx| super::open_location(&root, cx)).unwrap();
    let first = Notebook {
        _root: tempfile::tempdir().unwrap(),
        path: state.path().join("folder.sqlite"),
        window: stored_windows(cx)[0],
    };
    super::tests::settle(&first, cx);
    first
        .window
        .update(cx, |view, window, cx| {
            view.visit(
                quire_core::workspace::Destination::page(
                    quire_store::sync::format::page_id_for_path("B.md"),
                ),
                window,
                cx,
            )
        })
        .unwrap();
    super::workspace_tests::saved(&first, cx);
    cx.update_window(first.window.into(), |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    wait_until(cx, "the notebook to reopen", |cx| {
        cx.update(|cx| super::open_location(&root, cx)).is_ok()
    });
    let window = stored_windows(cx)[0];
    let shown = window
        .update(cx, |view, _, cx| view.page.read(cx).id().to_owned())
        .unwrap();
    assert_eq!(shown, quire_store::sync::format::page_id_for_path("B.md"));
}

#[gpui::test]
fn a_second_launch_brings_an_open_notebook_forward_or_opens_the_one_it_names(
    cx: &mut TestAppContext,
) {
    use crate::single_instance::{Launch, Reply};
    let opened = open_folder(&[("A.md", "- a\n")], "A.md", cx);
    let _states = open_folders_in_tests(cx);
    let launch = |args: Vec<String>| Launch {
        version: "0.1.0".into(),
        executable: "quire".into(),
        args: [vec!["quire".to_owned()], args].concat(),
    };
    let root = opened.folder.path().display().to_string();
    cx.update(|cx| {
        super::handle_launch(
            launch(vec!["--notebook-folder".into(), root]),
            Reply::Focus,
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(stored_windows(cx).len(), 1);
    cx.update(|cx| super::handle_launch(launch(Vec::new()), Reply::Focus, cx));
    cx.run_until_parked();
    assert_eq!(stored_windows(cx).len(), 1);

    let other = folder(&[("B.md", "- b\n")]);
    let other_root = other.path().display().to_string();
    cx.update(|cx| {
        super::handle_launch(
            launch(vec!["--notebook-folder".into(), other_root]),
            Reply::Focus,
            cx,
        )
    });
    cx.run_until_parked();
    assert_eq!(stored_windows(cx).len(), 2);

    cx.update(|cx| super::handle_launch(launch(Vec::new()), Reply::Handover, cx));
    cx.run_until_parked();
    assert_eq!(cx.platform_quit_count(), 1);
}

#[gpui::test]
fn the_menu_offers_appearance_find_profiles_and_window_commands_that_dispatch(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(&[("A.md", "- a\n")], "A.md", cx);
    let menus = cx.update(|cx| crate::app_menus::menus(cx));
    let names: Vec<String> = menus.iter().map(|menu| menu.name.to_string()).collect();
    let expected: &[&str] = if cfg!(target_os = "macos") {
        &["Quire", "File", "Edit", "View", "Window", "Help"]
    } else {
        &["File", "Edit", "View", "Window", "Help"]
    };
    assert_eq!(names, expected);
    let actions = |menu: &gpui::Menu| -> Vec<(String, String)> {
        fn walk(items: &[gpui::MenuItem], out: &mut Vec<(String, String)>) {
            for item in items {
                match item {
                    gpui::MenuItem::Action { name, action, .. } => {
                        out.push((name.to_string(), action.name().to_owned()))
                    }
                    gpui::MenuItem::Submenu(menu) => walk(&menu.items, out),
                    _ => {}
                }
            }
        }
        let mut out = Vec::new();
        walk(&menu.items, &mut out);
        out
    };
    let all: Vec<(String, String)> = menus.iter().flat_map(actions).collect();
    for (name, action) in [
        ("Dark", "quire_preferences::UseDarkAppearance"),
        ("Find…", "buffer_search::Deploy"),
        ("Find in Notebook…", "quire_navigation::FindPage"),
        ("Work", "quire_profile::UseWorkProfile"),
        ("Minimize", "quire_window::Minimize"),
        ("Close Window", "quire_window::CloseWindow"),
        ("Help and Shortcuts", "quire_help::OpenHelp"),
    ] {
        assert!(
            all.contains(&(name.to_owned(), action.to_owned())),
            "{name} -> {action} missing from {all:?}"
        );
    }
    let appearance = |cx: &mut TestAppContext| {
        cx.update(|cx| crate::app_menus::menus(cx))
            .iter()
            .flat_map(actions)
            .count()
    };
    assert!(appearance(cx) > 0);

    cx.dispatch_action(opened.notebook.window.into(), crate::navigation::FindPage);
    cx.run_until_parked();
    assert!(read(&opened.notebook, cx, |view, _| view
        .nav
        .palette
        .is_some()));
    cx.dispatch_action(opened.notebook.window.into(), crate::app_menus::Minimize);
    cx.dispatch_action(opened.notebook.window.into(), crate::app_menus::Zoom);
    cx.run_until_parked();
    assert!(is_open(opened.notebook.window, cx));
    cx.dispatch_action(opened.notebook.window.into(), crate::app_menus::CloseWindow);
    cx.run_until_parked();
    assert!(!is_open(opened.notebook.window, cx));
}

fn appearance_checks(cx: &mut TestAppContext) -> Vec<(String, bool)> {
    cx.update(|cx| crate::app_menus::menus(cx))
        .iter()
        .find(|menu| menu.name == "View")
        .and_then(|view| {
            view.items.iter().find_map(|item| match item {
                gpui::MenuItem::Submenu(menu) if menu.name == "Appearance" => Some(
                    menu.items
                        .iter()
                        .filter_map(|item| match item {
                            gpui::MenuItem::Action { name, checked, .. } => {
                                Some((name.to_string(), *checked))
                            }
                            _ => None,
                        })
                        .collect(),
                ),
                _ => None,
            })
        })
        .unwrap_or_default()
}

#[gpui::test]
async fn the_appearance_menu_switches_the_theme_and_shows_the_choice(cx: &mut TestAppContext) {
    let _ai = crate::ai::proposal::tests::setup(false, cx).await;
    let expect = |system, light, dark| {
        vec![
            ("System".to_owned(), system),
            ("Light".to_owned(), light),
            ("Dark".to_owned(), dark),
        ]
    };
    assert_eq!(appearance_checks(cx), expect(true, false, false));
    cx.update(|cx| cx.dispatch_action(&crate::preferences::UseDarkAppearance));
    cx.run_until_parked();
    assert_eq!(appearance_checks(cx), expect(false, false, true));
    let installed = cx.update(|cx| cx.get_menus()).unwrap();
    let view = installed.iter().find(|menu| menu.name == "View").unwrap();
    let dark = view.items.iter().find_map(|item| match item {
        gpui::OwnedMenuItem::Submenu(menu) => menu.items.iter().find_map(|item| match item {
            gpui::OwnedMenuItem::Action { name, checked, .. } if name == "Dark" => Some(*checked),
            _ => None,
        }),
        _ => None,
    });
    assert_eq!(dark, Some(true));
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[gpui::test]
async fn switching_profiles_ends_a_running_ai_transform_and_keeps_the_page(
    cx: &mut TestAppContext,
) {
    let ai = crate::ai::proposal::tests::setup(false, cx).await;
    let _states = open_folders_in_tests(cx);
    let current = folder(&[("A.md", "- one\n")]);
    let work = folder(&[("W.md", "- work\n")]);
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(ProfileId::Work, location(work.path()))
        })
        .unwrap();
        super::open_location(&location(current.path()), cx).unwrap();
    });
    let window = stored_windows(cx)[0];
    wait_until(cx, "the page editor", |cx| {
        window
            .update(cx, |view, _, _| view.editor.is_some())
            .unwrap()
    });
    let pids = ai.bench.folder.path().join("pids");
    let fixture = crate::ai::process::tests::fixture("claude-drift.jsonl");
    ai.answer(
        "",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_FIXTURE", &fixture),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    let editor = window
        .update(cx, |view, _, cx| {
            view.editor.as_ref().unwrap().read(cx).source.clone()
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        crate::ai::proposal::start(
            &editor,
            crate::ai::proposal::Invocation {
                args: "tighten".into(),
                rows: None,
                verb: None,
            },
            window,
            cx,
        )
    })
    .unwrap();
    wait_until(cx, "the fake CLI to start", |_| pids.exists());
    let running: Vec<String> = fs::read_to_string(&pids)
        .unwrap()
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    assert!(!running.is_empty());
    cx.dispatch_action(window.into(), crate::profiles::UseWorkProfile);
    wait_for_switch(cx, "the Work notebook to open", |cx| {
        stored_windows(cx).len() == 1 && !is_open(window, cx)
    });
    for pid in &running {
        wait_until(cx, "the AI run to end", |_| !alive(pid));
    }
    assert_eq!(
        fs::read_to_string(current.path().join("A.md")).unwrap(),
        "- one\n"
    );
}
