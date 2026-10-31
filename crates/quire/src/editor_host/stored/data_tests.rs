use super::super::*;
use super::tests::{open_notebook, read, save, wait_until};
use crate::{
    backups::{BackUpNow, ChooseBackupFolder, ShowBackups},
    preferences::{data::DataSection, view::PreferencesWindow},
    profiles::OpenNotebookDatabase,
};
use gpui::TestAppContext;
use quire_store::{
    notebook::NotebookStore,
    profiles::{Notebook as ProfileNotebook, ProfileId},
};
use std::{fs, path::Path};

fn stored_windows(cx: &mut TestAppContext) -> Vec<gpui::WindowHandle<StoredWindow>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<StoredWindow>())
        .collect()
}

/// Opens Settings and returns its Data section.
fn open_settings(
    cx: &mut TestAppContext,
) -> (gpui::WindowHandle<PreferencesWindow>, Entity<DataSection>) {
    cx.update(crate::preferences::view::open);
    cx.run_until_parked();
    let window = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<PreferencesWindow>())
        .expect("Settings opens");
    let data = window.update(cx, |view, _, _| view.data.clone()).unwrap();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    (window, data)
}

fn run(data: &Entity<DataSection>, action: impl gpui::Action, cx: &mut TestAppContext) {
    data.update(cx, |data, cx| data.in_notebook(action.boxed_clone(), cx));
    cx.run_until_parked();
}

fn answer_path(path: &Path, files: bool, cx: &mut TestAppContext) {
    wait_until(cx, "the file dialog", |cx| cx.did_prompt_for_paths());
    let path = path.to_owned();
    cx.simulate_path_prompt_response(move |options| {
        assert_eq!(options.files, files);
        Some(vec![path])
    });
}

fn profile(id: ProfileId, cx: &mut TestAppContext) -> quire_store::profiles::Profile {
    cx.update(|cx| crate::profiles::get(id, cx))
}

#[gpui::test]
fn the_data_section_runs_backups_in_the_notebook_window_and_saves_the_personal_remote(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    let (settings, data) = open_settings(cx);
    assert_eq!(
        profile(ProfileId::Current, cx).notebook,
        Some(ProfileNotebook::Store {
            database: notebook.path.clone(),
            notebook: "nb".into(),
            writer: "local".into(),
            folder: None,
        }),
        "the open notebook is the active profile's"
    );

    let destination = tempfile::tempdir().unwrap();
    run(&data, ChooseBackupFolder, cx);
    answer_path(destination.path(), false, cx);
    wait_until(cx, "the profile's backup folder", |cx| {
        profile(ProfileId::Current, cx).backup_folder.is_some()
    });
    run(&data, BackUpNow, cx);
    wait_until(cx, "the backup", |cx| {
        profile(ProfileId::Current, cx).last_backup_at.is_some()
    });
    let backed_up = profile(ProfileId::Current, cx);
    assert_eq!(backed_up.backup_error, None);
    assert!(
        backed_up
            .last_backup_path
            .unwrap()
            .starts_with(fs::canonicalize(destination.path()).unwrap())
    );
    run(&data, ShowBackups, cx);
    wait_until(cx, "the backup list in the notebook window", |cx| {
        read(&notebook, cx, |view, cx| {
            view.backups
                .read(cx)
                .listed()
                .is_some_and(|listed| listed.len() == 1)
        })
    });
    cx.update_window(settings.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();

    let personal = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(
                ProfileId::Personal,
                ProfileNotebook::Folder {
                    root: personal.path().to_owned(),
                },
            )?;
            profiles.activate(ProfileId::Personal)
        })
    })
    .unwrap();
    let remote = "git@example.invalid:me/notes.git";
    settings
        .update(cx, |_, window, cx| {
            data.update(cx, |data, cx| {
                data.remote_field()
                    .update(cx, |editor, cx| editor.set_text(remote, window, cx));
                data.save_remote(cx);
            })
        })
        .unwrap();
    assert_eq!(
        profile(ProfileId::Personal, cx).git_remote.as_deref(),
        Some(remote)
    );
    assert!(
        data.read_with(cx, |data, _| data.message().unwrap().contains(remote)),
        "the section confirms the remote"
    );
    cx.update_window(settings.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();

    notebook
        .window
        .update(cx, |view, _, _| view.end_close_request())
        .unwrap();
    cx.update_window(notebook.window.into(), |_, window, _| {
        window.remove_window()
    })
    .unwrap();
    cx.run_until_parked();
    run(&data, BackUpNow, cx);
    assert_eq!(
        data.read_with(cx, |data, _| data.error().cloned()).unwrap(),
        "No notebook is open. Open the profile's notebook from File > Profile, then try again."
    );
}

#[gpui::test]
fn choosing_a_quire_notebook_database_opens_it_as_the_active_profiles_notebook(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    let other = tempfile::tempdir().unwrap();
    let database = other.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "other-nb", "other-writer").unwrap();
    save(&store, "p-z", "Zed.md", "- zed");
    save(&store, "p-b", "Beta.md", "- beta");
    drop(store);
    let not_a_notebook = other.path().join("notes.txt");
    fs::write(&not_a_notebook, "plain text").unwrap();

    cx.dispatch_action(notebook.window.into(), OpenNotebookDatabase);
    answer_path(&not_a_notebook, true, cx);
    wait_until(cx, "the refusal", |cx| {
        cx.update(|cx| crate::outline::notice(cx))
            .is_some_and(|notice| notice.contains("Quire notebook database"))
    });
    assert_eq!(stored_windows(cx), vec![notebook.window]);

    cx.dispatch_action(notebook.window.into(), OpenNotebookDatabase);
    answer_path(&database, true, cx);
    wait_until(cx, "the chosen notebook's window", |cx| {
        let windows = stored_windows(cx);
        windows.len() == 1 && windows[0] != notebook.window
    });
    let opened = stored_windows(cx)[0];
    let shown = opened
        .update(cx, |view, _, cx| view.page.read(cx).id().to_owned())
        .unwrap();
    assert_eq!(
        shown, "p-b",
        "with no remembered page, the first page by path shows"
    );
    assert_eq!(
        profile(ProfileId::Current, cx).notebook,
        Some(ProfileNotebook::Store {
            database: fs::canonicalize(&database).unwrap(),
            notebook: "other-nb".into(),
            writer: "other-writer".into(),
            folder: None,
        })
    );
}

#[gpui::test]
fn a_restored_notebook_becomes_the_notebook_of_the_profile_that_held_the_original(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    let destination = tempfile::tempdir().unwrap();
    cx.dispatch_action(notebook.window.into(), ChooseBackupFolder);
    answer_path(destination.path(), false, cx);
    wait_until(cx, "the profile's backup folder", |cx| {
        profile(ProfileId::Current, cx).backup_folder.is_some()
    });
    cx.dispatch_action(notebook.window.into(), BackUpNow);
    wait_until(cx, "the backup", |cx| {
        profile(ProfileId::Current, cx).last_backup_at.is_some()
    });
    cx.dispatch_action(notebook.window.into(), ShowBackups);
    wait_until(cx, "the backup list", |cx| {
        read(&notebook, cx, |view, cx| {
            view.backups
                .read(cx)
                .listed()
                .is_some_and(|listed| !listed.is_empty())
        })
    });
    let newest = read(&notebook, cx, |view, cx| {
        view.backups.read(cx).listed().unwrap()[0].clone()
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.backups
                .update(cx, |backups, cx| backups.restore(newest, window, cx))
        })
        .unwrap();
    wait_until(cx, "the restore question", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Restore as a new notebook");
    let parent = tempfile::tempdir().unwrap();
    answer_path(parent.path(), false, cx);
    wait_until(cx, "the restored notebook's window", |cx| {
        let windows = stored_windows(cx);
        windows.len() == 1 && windows[0] != notebook.window
    });
    let held = profile(ProfileId::Current, cx);
    let Some(ProfileNotebook::Store { database, .. }) = held.notebook.clone() else {
        panic!("the profile holds a Quire notebook: {held:?}");
    };
    assert!(
        database.starts_with(fs::canonicalize(parent.path()).unwrap()),
        "{}",
        database.display()
    );
    assert_eq!(
        held.backup_folder.as_deref(),
        Some(fs::canonicalize(destination.path()).unwrap().as_path()),
        "the backup folder stays with the profile"
    );
    assert_eq!(
        cx.update(|cx| crate::profiles::active(cx)),
        ProfileId::Current
    );
}

#[gpui::test]
fn help_opens_beside_the_pages_closes_with_escape_and_captures_a_task(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    cx.dispatch_action(notebook.window.into(), crate::help::OpenHelp);
    cx.run_until_parked();
    let help = read(&notebook, cx, |view, _| view.help.clone()).expect("Help opens as a pane");
    assert!(
        cx.windows()
            .into_iter()
            .all(|window| window.downcast::<crate::help::view::HelpWindow>().is_none()),
        "no separate Help window opens over a notebook"
    );
    let focused = notebook
        .window
        .update(cx, |_, window, cx| {
            help.read(cx).search_focus(cx).is_focused(window)
        })
        .unwrap();
    assert!(focused, "the search field has focus");
    cx.update_window(notebook.window.into(), |_, window, cx| {
        window.draw(cx).clear(cx)
    })
    .unwrap();

    cx.dispatch_action(notebook.window.into(), menu::Cancel);
    cx.run_until_parked();
    assert!(read(&notebook, cx, |view, _| view.help.is_none()));

    cx.dispatch_action(notebook.window.into(), crate::help::OpenHelp);
    cx.run_until_parked();
    let help = read(&notebook, cx, |view, _| view.help.clone()).unwrap();
    notebook
        .window
        .update(cx, |_, window, cx| {
            help.update(cx, |help, cx| help.capture_task(window, cx))
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(
        read(&notebook, cx, |view, _| view.nav.view),
        Some(quire_core::workspace::BrowserView::Tasks),
        "Capture a task opens Tasks"
    );
}

#[gpui::test]
fn migrating_from_rt_waits_until_no_notebook_window_holds_a_notebook(cx: &mut TestAppContext) {
    let _notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    let data = tempfile::tempdir().unwrap();
    let rt = tempfile::tempdir().unwrap();
    let (quire_data, rt_data) = (data.path().to_owned(), rt.path().to_owned());
    let window = cx.add_window(|window, cx| {
        crate::rt_migration::view::MigrationWindow::new(quire_data, Some(rt_data), window, cx)
    });
    window.update(cx, |view, _, cx| view.migrate(cx)).unwrap();
    cx.run_until_parked();
    let (running, error) = window
        .update(cx, |view, _, _| (view.is_running(), view.error().cloned()))
        .unwrap();
    assert!(!running);
    assert_eq!(
        error.as_deref(),
        Some(crate::rt_migration::view::CLOSE_NOTEBOOKS)
    );
    assert_eq!(
        fs::read_dir(data.path()).unwrap().count(),
        0,
        "nothing is written to Quire's data folder"
    );
}

#[gpui::test]
fn reveal_shows_a_read_only_copy_of_a_quire_notebook_page(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("p-a", "Projects/Plan.md", "- alpha\r\n")], "p-a", cx);
    let copies = tempfile::tempdir().unwrap();
    let revealed = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
    let recorder = revealed.clone();
    let folder = copies.path().to_owned();
    cx.update(|cx| {
        cx.set_global(super::menu_views::RevealCopies(folder));
        cx.set_global(super::menu_views::RevealFiles(std::rc::Rc::new(
            move |path| recorder.borrow_mut().push(path.to_owned()),
        )));
    });
    cx.dispatch_action(
        notebook.window.into(),
        crate::navigation::RevealInFileManager,
    );
    wait_until(cx, "the revealed copy", |_| !revealed.borrow().is_empty());
    let copy = copies.path().join("Plan.md");
    assert_eq!(*revealed.borrow(), std::slice::from_ref(&copy));
    assert_eq!(fs::read_to_string(&copy).unwrap(), "- alpha\r\n");
    assert!(fs::metadata(&copy).unwrap().permissions().readonly());
    assert_eq!(read(&notebook, cx, |view, _| view.nav.error.clone()), None);
}

/// The single notebook window other than `previous`, once it opened.
fn other_window(
    previous: gpui::WindowHandle<StoredWindow>,
    cx: &mut TestAppContext,
) -> gpui::WindowHandle<StoredWindow> {
    wait_until(cx, "the chosen notebook's window", |cx| {
        let windows = stored_windows(cx);
        windows.len() == 1 && windows[0] != previous
    });
    stored_windows(cx)[0]
}

#[gpui::test]
fn choosing_a_quire_notebooks_synced_folder_opens_it_as_a_quire_notebook(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    let shared = tempfile::tempdir().unwrap();
    let synced = shared.path().join("OneDrive Notes");
    let id = quire_store::quire_folder::create(&synced, 1).unwrap();
    let office = NotebookStore::open(&shared.path().join("office.sqlite"), &id, "office").unwrap();
    save(&office, "p-o", "Office.md", "- from the office");
    office.publish_to(&synced, 2).unwrap();
    drop(office);
    let data = shared.path().join("quire-data");

    cx.update(|cx| {
        super::super::profiles::open_chosen_folder(
            ProfileId::Current,
            synced.clone(),
            data.clone(),
            cx,
        )
    });
    let opened = other_window(notebook.window, cx);
    wait_until(cx, "the office page", |cx| {
        opened
            .update(cx, |view, _, cx| {
                view.page.read(cx).id() == "p-o" && view.page.read(cx).loaded().is_some()
            })
            .unwrap()
    });
    let Some(ProfileNotebook::Store {
        database,
        notebook: held,
        writer,
        folder,
    }) = profile(ProfileId::Current, cx).notebook
    else {
        panic!("the synced folder opens as a Quire notebook");
    };
    assert_eq!(held, id);
    assert_eq!(folder, Some(fs::canonicalize(&synced).unwrap()));
    assert!(database.starts_with(&data));
    assert_ne!(writer, "office");
    assert!(
        !synced.join("Office.md").exists(),
        "a Quire notebook is never read as Markdown files"
    );
}

#[gpui::test]
fn a_new_quire_notebook_needs_an_empty_folder_then_opens_there(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    let parent = tempfile::tempdir().unwrap();
    let data = parent.path().join("quire-data");
    let busy = parent.path().join("busy");
    fs::create_dir(&busy).unwrap();
    fs::write(busy.join("Note.md"), "- keep").unwrap();

    cx.update(|cx| {
        super::super::profiles::create_quire_notebook(
            ProfileId::Current,
            busy.clone(),
            data.clone(),
            cx,
        )
    });
    wait_until(cx, "the refusal", |cx| {
        cx.update(|cx| crate::outline::notice(cx))
            .is_some_and(|notice| notice.contains("Choose an empty folder"))
    });
    assert_eq!(stored_windows(cx), vec![notebook.window]);
    assert!(!busy.join("quire-notebook.json").exists());

    let empty = parent.path().join("Notes");
    cx.update(|cx| {
        super::super::profiles::create_quire_notebook(
            ProfileId::Current,
            empty.clone(),
            data.clone(),
            cx,
        )
    });
    other_window(notebook.window, cx);
    let id = quire_store::quire_folder::read_identity(&empty).unwrap();
    let Some(ProfileNotebook::Store {
        notebook: held,
        folder,
        ..
    }) = profile(ProfileId::Current, cx).notebook
    else {
        panic!("the new notebook is a Quire notebook");
    };
    assert_eq!(held, id);
    assert_eq!(folder, Some(fs::canonicalize(&empty).unwrap()));
}
