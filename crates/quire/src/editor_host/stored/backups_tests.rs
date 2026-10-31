use super::super::*;
use super::tests::{Notebook, open_notebook, read, save, wait_until};
use crate::backups::{
    BackUpNow, Backups, ChooseBackupFolder, INTERVAL, NO_FOLDER, RETRY, ShowBackups, UNCHANGED,
};
use gpui::TestAppContext;
use quire_store::{
    backup::{self, BackupInfo, Source},
    notebook::NotebookStore,
};
use std::{fs, path::Path, time::Duration};

fn dispatch(notebook: &Notebook, action: impl gpui::Action, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), action);
}

fn backups(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<Backups> {
    read(notebook, cx, |view, _| view.backups.clone())
}

/// The message shown once no backup work runs.
fn settled_message(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let backups = backups(notebook, cx);
    wait_until(cx, "backup work to finish", |cx| {
        backups.read_with(cx, |backups, _| !backups.is_running())
    });
    backups.read_with(cx, |backups, _| {
        backups
            .message()
            .map(|message| message.to_string())
            .unwrap_or_default()
    })
}

fn choose_folder(folder: &Path, cx: &mut TestAppContext) {
    wait_until(cx, "the folder dialog", |cx| cx.did_prompt_for_paths());
    let folder = folder.to_owned();
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.directories && !options.files);
        Some(vec![folder])
    });
}

fn completed(notebook: &Notebook, destination: &Path) -> Vec<BackupInfo> {
    let source = Source::Store {
        database: notebook.path.clone(),
    };
    backup::list(&source, destination).unwrap()
}

fn page(id: &str, path: &str, text: &str) -> (String, String, String) {
    (id.into(), path.into(), text.into())
}

fn open(pages: &[(String, String, String)], cx: &mut TestAppContext) -> Notebook {
    let pages: Vec<(&str, &str, &str)> = pages
        .iter()
        .map(|(id, path, text)| (id.as_str(), path.as_str(), text.as_str()))
        .collect();
    open_notebook(&pages, "p-a", cx)
}

/// Chooses `folder` for backups through the command and its dialog.
fn use_folder(notebook: &Notebook, folder: &Path, cx: &mut TestAppContext) {
    dispatch(notebook, ChooseBackupFolder, cx);
    choose_folder(folder, cx);
    wait_until(cx, "the backup folder", |cx| {
        backups(notebook, cx).read_with(cx, |backups, _| backups.destination().is_some())
    });
}

#[gpui::test]
fn back_up_now_needs_a_folder_then_writes_a_verified_backup_that_the_list_shows(
    cx: &mut TestAppContext,
) {
    let notebook = open(&[page("p-a", "A.md", "- alpha")], cx);
    dispatch(&notebook, BackUpNow, cx);
    assert_eq!(settled_message(&notebook, cx), NO_FOLDER);

    let inside = notebook.path.parent().unwrap().join("inside");
    fs::create_dir(&inside).unwrap();
    dispatch(&notebook, ChooseBackupFolder, cx);
    choose_folder(&inside, cx);
    let message = settled_message(&notebook, cx);
    assert!(
        message.starts_with("Cannot use that backup folder:")
            && message.contains("overlaps the notebook's own data"),
        "{message}"
    );

    let destination = tempfile::tempdir().unwrap();
    use_folder(&notebook, destination.path(), cx);
    dispatch(&notebook, BackUpNow, cx);
    let message = settled_message(&notebook, cx);
    assert!(message.starts_with("Backed up at "), "{message}");
    let made = completed(&notebook, destination.path());
    assert!(!made.is_empty());
    assert!(
        message.contains(&made[0].path.display().to_string()),
        "{message}"
    );
    backup::verify(&made[0].path).unwrap();

    dispatch(&notebook, ShowBackups, cx);
    let entity = backups(&notebook, cx);
    wait_until(cx, "the backup list", |cx| {
        entity.read_with(cx, |backups, _| {
            backups.listed().is_some_and(|listed| !listed.is_empty())
        })
    });
    let newest = entity.read_with(cx, |backups, _| backups.listed().unwrap()[0].clone());
    assert_eq!(newest, made[0]);
    entity.update(cx, |backups, cx| backups.verify(newest.clone(), cx));
    let message = settled_message(&notebook, cx);
    assert!(
        message.contains("is complete") && message.contains("files match their recorded hashes"),
        "{message}"
    );

    fs::remove_file(newest.path.join("local.sqlite")).unwrap();
    entity.update(cx, |backups, cx| backups.verify(newest.clone(), cx));
    let message = settled_message(&notebook, cx);
    assert!(
        message.contains("failed verification") && message.contains("local.sqlite is missing"),
        "{message}"
    );
}

#[gpui::test]
fn backups_run_every_hour_while_open_and_wait_fifteen_minutes_after_a_failure(
    cx: &mut TestAppContext,
) {
    let notebook = open(&[page("p-a", "A.md", "- alpha")], cx);
    let destination = tempfile::tempdir().unwrap();
    let folder = destination.path().join("drive");
    fs::create_dir(&folder).unwrap();
    use_folder(&notebook, &folder, cx);
    let count = |cx: &mut TestAppContext| {
        settled_message(&notebook, cx);
        completed(&notebook, &folder).len()
    };
    cx.executor().advance_clock(Duration::from_secs(61));
    assert_eq!(count(cx), 1, "the first backup runs within a minute");
    edit(&notebook, "- edited once");
    cx.executor().advance_clock(INTERVAL / 2);
    assert_eq!(count(cx), 1, "no second backup within the hour");
    cx.executor()
        .advance_clock(INTERVAL / 2 + Duration::from_secs(61));
    assert_eq!(count(cx), 2, "the next backup runs an hour later");

    edit(&notebook, "- edited twice");
    fs::rename(&folder, destination.path().join("unplugged")).unwrap();
    cx.executor()
        .advance_clock(INTERVAL + Duration::from_secs(61));
    let message = settled_message(&notebook, cx);
    assert!(
        message.starts_with("Automatic backup failed:")
            && message.ends_with("Open Backups and back up now to retry."),
        "{message}"
    );
    fs::rename(destination.path().join("unplugged"), &folder).unwrap();
    cx.executor().advance_clock(RETRY / 3);
    assert_eq!(count(cx), 2, "a failed backup is not retried at once");
    cx.executor().advance_clock(RETRY);
    assert_eq!(count(cx), 3, "it is retried fifteen minutes later");
}

/// Saves `text` as page A through a second connection, as another window or device would.
fn edit(notebook: &Notebook, text: &str) {
    let live = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&live, "p-a", "A.md", text);
}

#[gpui::test]
fn the_hourly_backup_skips_an_unchanged_notebook_but_back_up_now_does_not(cx: &mut TestAppContext) {
    let notebook = open(&[page("p-a", "A.md", "- alpha")], cx);
    let destination = tempfile::tempdir().unwrap();
    use_folder(&notebook, destination.path(), cx);
    let count = || completed(&notebook, destination.path()).len();
    let hour = |cx: &mut TestAppContext| {
        cx.executor()
            .advance_clock(INTERVAL + Duration::from_secs(61));
        settled_message(&notebook, cx)
    };
    cx.executor().advance_clock(Duration::from_secs(61));
    settled_message(&notebook, cx);
    assert_eq!(count(), 1, "the first backup runs within a minute");
    assert_eq!(hour(cx), UNCHANGED);
    assert_eq!(count(), 1, "an unchanged notebook is not backed up again");

    edit(&notebook, "- one edit");
    assert!(hour(cx).starts_with("Backed up at "));
    assert_eq!(count(), 2, "one edit is backed up on the next hour");
    assert_eq!(hour(cx), UNCHANGED);

    let live = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    live.attach(b"new image bytes", "photo.png").unwrap();
    drop(live);
    assert!(hour(cx).starts_with("Backed up at "));
    assert_eq!(count(), 3, "an added attachment is backed up");
    let newest = completed(&notebook, destination.path()).remove(0);
    assert!(
        backup::verify(&newest.path)
            .unwrap()
            .files
            .iter()
            .any(|file| file.path.starts_with("attachments/") && file.path.ends_with(".png")),
        "the backup holds the attachment"
    );
    assert_eq!(hour(cx), UNCHANGED);
    assert_eq!(count(), 3);

    dispatch(&notebook, BackUpNow, cx);
    assert!(settled_message(&notebook, cx).starts_with("Backed up at "));
    assert_eq!(count(), 4, "Back up now backs up an unchanged notebook");
}

fn restore_newest(notebook: &Notebook, cx: &mut TestAppContext) -> BackupInfo {
    let entity = backups(notebook, cx);
    if entity.read_with(cx, |backups, _| backups.listed().is_none()) {
        dispatch(notebook, ShowBackups, cx);
    }
    wait_until(cx, "the backup list", |cx| {
        entity.read_with(cx, |backups, _| {
            backups.listed().is_some_and(|listed| !listed.is_empty())
        })
    });
    let newest = entity.read_with(cx, |backups, _| backups.listed().unwrap()[0].clone());
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.backups.update(cx, |backups, cx| {
                backups.restore(newest.clone(), window, cx)
            })
        })
        .unwrap();
    wait_until(cx, "the restore question", |cx| cx.has_pending_prompt());
    newest
}

fn stored_windows(cx: &mut TestAppContext) -> Vec<gpui::WindowHandle<StoredWindow>> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<StoredWindow>())
        .collect()
}

#[gpui::test]
fn restoring_backs_up_first_and_opens_the_restored_notebook_in_place_of_this_one(
    cx: &mut TestAppContext,
) {
    let notebook = open(&[page("p-a", "A.md", "- at backup time")], cx);
    let destination = tempfile::tempdir().unwrap();
    use_folder(&notebook, destination.path(), cx);
    dispatch(&notebook, BackUpNow, cx);
    settled_message(&notebook, cx);
    let live = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    save(&live, "p-a", "A.md", "- changed after the backup");
    drop(live);

    let parent = tempfile::tempdir().unwrap();
    let chosen = restore_newest(&notebook, cx);
    let (question, detail) = cx.pending_prompt().unwrap();
    assert!(
        question.starts_with("Restore the backup from ")
            && question.ends_with(" as a new notebook?"),
        "{question}"
    );
    assert!(
        detail.contains("first backs up this notebook")
            && detail.contains("This notebook stays unchanged."),
        "{detail}"
    );
    cx.simulate_prompt_answer("Cancel");
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_paths(), "Cancel asks nothing more");
    assert_eq!(
        completed(&notebook, destination.path()),
        vec![chosen.clone()]
    );

    restore_newest(&notebook, cx);
    cx.simulate_prompt_answer("Restore as a new notebook");
    choose_folder(parent.path(), cx);
    wait_until(cx, "the restored notebook's window", |cx| {
        let windows = stored_windows(cx);
        windows.len() == 1 && windows[0] != notebook.window
    });
    let restored = stored_windows(cx)[0];
    // Read before the restored notebook's own hourly schedule can tick: a wait under load
    // advances the test clock past its first minute, when it backs itself up.
    let message = restored
        .update(cx, |view, _, cx| view.backups.read(cx).message())
        .unwrap();
    assert_eq!(
        message, None,
        "the restored window starts without the old window's messages"
    );
    let database = restored
        .update(cx, |view, _, _| view.nav.database.clone())
        .unwrap();
    assert!(
        database.starts_with(fs::canonicalize(parent.path()).unwrap()),
        "{}",
        database.display()
    );
    wait_until(cx, "the restored page", |cx| {
        restored
            .update(cx, |view, _, cx| view.page.read(cx).loaded().is_some())
            .unwrap()
    });
    let text = restored
        .update(cx, |view, _, cx| {
            view.page
                .read(cx)
                .loaded()
                .unwrap()
                .analysis
                .read(cx)
                .source
                .read(cx)
                .text()
        })
        .unwrap();
    assert_eq!(text, "- at backup time");
    // A loaded run reaches the restored notebook's first scheduled backup; it is its own and
    // leaves this notebook's backups alone.
    cx.executor().advance_clock(Duration::from_secs(61));
    cx.run_until_parked();
    let after = completed(&notebook, destination.path());
    assert_eq!(after.len(), 2, "the safety backup joins the first");
    assert_eq!(after[1], chosen);
    let original = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let head = original.heads("p-a").unwrap().remove(0);
    assert_eq!(
        original
            .get(&head)
            .unwrap()
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("- changed after the backup"),
        "the original notebook is unchanged"
    );
}

#[gpui::test]
fn a_damaged_backup_is_not_restored_and_the_window_stays(cx: &mut TestAppContext) {
    let notebook = open(&[page("p-a", "A.md", "- alpha")], cx);
    let destination = tempfile::tempdir().unwrap();
    use_folder(&notebook, destination.path(), cx);
    dispatch(&notebook, BackUpNow, cx);
    settled_message(&notebook, cx);
    let made = completed(&notebook, destination.path());
    fs::write(made[0].path.join("local.sqlite"), b"not a database").unwrap();
    let parent = tempfile::tempdir().unwrap();
    restore_newest(&notebook, cx);
    cx.simulate_prompt_answer("Restore as a new notebook");
    choose_folder(parent.path(), cx);
    let message = settled_message(&notebook, cx);
    assert!(
        message.starts_with("Restore failed: Backup verification failed: local.sqlite"),
        "{message}"
    );
    assert_eq!(stored_windows(cx), vec![notebook.window]);
    assert!(fs::read_dir(parent.path()).unwrap().next().is_none());
    assert_eq!(
        completed(&notebook, destination.path()).len(),
        1,
        "no safety backup for a refused restore"
    );
}

/// The tabs and favorites a window shows.
fn workspace_pages(
    window: gpui::WindowHandle<StoredWindow>,
    cx: &mut TestAppContext,
) -> (Vec<String>, Vec<String>) {
    let ids = |destinations: &[quire_core::workspace::Destination]| {
        destinations
            .iter()
            .filter_map(|destination| destination.page_id().map(str::to_owned))
            .collect()
    };
    window
        .update(cx, |view, _, _| {
            (
                ids(&view.nav.workspace.tabs),
                ids(&view.nav.workspace.favorites),
            )
        })
        .unwrap()
}

#[gpui::test]
fn a_folder_notebook_backs_up_and_restores_through_the_window_with_its_tabs(
    cx: &mut TestAppContext,
) {
    use quire_core::workspace::Destination;
    use quire_store::sync::format::page_id_for_path;
    let opened = super::folder_tests::open_folder(
        &[("A.md", "- at backup time\n"), ("People/B.md", "- bee\n")],
        "A.md",
        cx,
    );
    let notebook = &opened.notebook;
    let root = opened.folder.path().to_owned();
    let states = tempfile::tempdir().unwrap();
    let state_root = states.path().to_owned();
    backups(notebook, cx).update(cx, |backups, _| {
        backups.keep_restored_state(std::sync::Arc::new(move |root: &Path| {
            state_root.join(quire_store::sync::format::sha256(
                root.to_string_lossy().as_bytes(),
            ))
        }))
    });
    let destination = tempfile::tempdir().unwrap();
    use_folder(notebook, destination.path(), cx);
    dispatch(notebook, BackUpNow, cx);
    let message = settled_message(notebook, cx);
    let source = Source::Folder {
        root: root.clone(),
        state: notebook.path.parent().unwrap().to_owned(),
    };
    let made = backup::list(&source, destination.path()).unwrap();
    assert_eq!(made.len(), 1, "{message}");
    backup::verify(&made[0].path).unwrap();
    fs::write(root.join("A.md"), "- changed after the backup\n").unwrap();
    fs::write(root.join("Later.md"), "- written after the backup\n").unwrap();

    let (a, b, later) = (
        page_id_for_path("A.md"),
        page_id_for_path("People/B.md"),
        page_id_for_path("Later.md"),
    );
    restore_newest(notebook, cx);
    notebook
        .window
        .update(cx, |view, _, _| {
            view.nav.workspace.tabs = [&a, &b, &later]
                .map(|id| Destination::page(id.clone()))
                .into();
            view.nav.workspace.favorites = vec![Destination::page(b.clone())];
        })
        .unwrap();
    cx.simulate_prompt_answer("Restore as a new notebook");
    let parent = tempfile::tempdir().unwrap();
    choose_folder(parent.path(), cx);
    wait_until(cx, "the restored notebook's window", |cx| {
        let windows = stored_windows(cx);
        windows.len() == 1 && windows[0] != notebook.window
    });
    let restored = stored_windows(cx)[0];
    let restored_root = fs::read_dir(parent.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect::<Vec<_>>();
    assert_eq!(restored_root.len(), 1, "{restored_root:?}");
    assert_eq!(
        fs::read_to_string(restored_root[0].join("A.md")).unwrap(),
        "- at backup time\n"
    );
    assert!(!restored_root[0].join("Later.md").exists());
    assert_eq!(
        fs::read_to_string(root.join("A.md")).unwrap(),
        "- changed after the backup\n",
        "the original folder is unchanged"
    );
    wait_until(cx, "the restored page", |cx| {
        restored
            .update(cx, |view, _, cx| view.page.read(cx).loaded().is_some())
            .unwrap()
    });
    wait_until(
        cx,
        "the carried tabs without the page the backup lacks",
        |cx| workspace_pages(restored, cx) == (vec![a.clone(), b.clone()], vec![b.clone()]),
    );
    assert_eq!(
        backup::list(&source, destination.path()).unwrap().len(),
        2,
        "the safety backup joins the first"
    );
    assert!(
        fs::read_dir(states.path()).unwrap().count() == 1,
        "the restored state stays in the test's folder"
    );
}
