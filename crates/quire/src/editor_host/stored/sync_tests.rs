use super::super::*;
use super::folder_tests::{FolderNotebook, open_folder};
use super::tests::{Notebook, open_notebook, read, wait_until};
use crate::sync_status::{NO_FOLDER, NOT_PERSONAL, ShowSync, SyncStatus, SyncWithGit};
use gpui::TestAppContext;
use quire_store::git_sync::{self, Profile, Target};
use quire_store::profiles::ProfileId;
use std::{fs, os::unix::fs::PermissionsExt as _, path::Path, process::Command};

fn dispatch(notebook: &Notebook, action: impl gpui::Action, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), action);
}

fn status(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<SyncStatus> {
    read(notebook, cx, |view, _| view.sync.clone())
}

fn lines(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    status(notebook, cx).read_with(cx, |status, cx| status.lines(cx))
}

/// The message shown once Git no longer runs.
fn settled_message(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let status = status(notebook, cx);
    wait_until(cx, "Git sync to finish", |cx| {
        status.read_with(cx, |status, _| !status.is_running())
    });
    status.read_with(cx, |status, _| {
        status
            .message()
            .map(|message| message.to_string())
            .unwrap_or_default()
    })
}

fn wait_for_line(notebook: &Notebook, what: &str, cx: &mut TestAppContext) {
    let status = status(notebook, cx);
    wait_until(cx, what, |cx| {
        status.read_with(cx, |status, cx| {
            status.lines(cx).iter().any(|line| line.contains(what))
        })
    });
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .envs(GIT_ENV)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

const GIT_ENV: [(&str, &str); 6] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Other Computer"),
    ("GIT_AUTHOR_EMAIL", "other@example.invalid"),
    ("GIT_COMMITTER_NAME", "Other Computer"),
    ("GIT_COMMITTER_EMAIL", "other@example.invalid"),
];

fn bare(directory: &Path) -> String {
    let remote = fs::canonicalize(directory).unwrap().join("remote.git");
    git(
        directory,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    remote.display().to_string()
}

fn page_text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let persistence = read(notebook, cx, |view, cx| {
        view.page.read(cx).persistence().cloned().unwrap()
    });
    cx.update(|cx| persistence.read(cx).buffer().read(cx).text())
}

fn show(notebook: &Notebook, cx: &mut TestAppContext) {
    dispatch(notebook, ShowSync, cx);
    let status = status(notebook, cx);
    wait_until(cx, "the sync panel", |cx| {
        status.read_with(cx, |status, _| status.is_shown())
    });
}

/// Saves `remote` as the notebook's Git remote.
fn save_remote(opened: &FolderNotebook, remote: &str, cx: &mut TestAppContext) {
    let status = status(&opened.notebook, cx);
    let remote = remote.to_owned();
    status.update(cx, |status, cx| status.set_remote(remote, cx));
    cx.run_until_parked();
}

#[gpui::test]
fn a_notebook_without_a_folder_says_git_sync_needs_one(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("p-a", "A.md", "- alpha")], "p-a", cx);
    show(&notebook, cx);
    assert_eq!(lines(&notebook, cx), [NO_FOLDER]);
    dispatch(&notebook, SyncWithGit, cx);
    assert_eq!(settled_message(&notebook, cx), NO_FOLDER);
}

#[gpui::test]
fn the_panel_lists_cloud_only_notes_and_conflict_copies_until_they_are_resolved(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(
        &[
            ("A.md", "- alpha\n"),
            (".Plan.md.icloud", "stub"),
            ("Plan (conflicted copy 2026-10-31).md", "- copy\n"),
        ],
        "A.md",
        cx,
    );
    let notebook = &opened.notebook;
    show(notebook, cx);
    wait_for_line(
        notebook,
        "1 notes are only in the cloud, not on this computer: Plan.md",
        cx,
    );
    wait_for_line(
        notebook,
        "1 conflict copies: Plan (conflicted copy 2026-10-31).md",
        cx,
    );
    assert!(lines(notebook, cx)[0].contains("not a known OneDrive"));

    fs::remove_file(opened.folder.path().join(".Plan.md.icloud")).unwrap();
    fs::write(opened.folder.path().join("Plan.md"), "- downloaded\n").unwrap();
    fs::remove_file(
        opened
            .folder
            .path()
            .join("Plan (conflicted copy 2026-10-31).md"),
    )
    .unwrap();
    dispatch(notebook, crate::sync_status::CheckSyncFolder, cx);
    wait_for_line(notebook, "Every note is on this computer", cx);
}

/// Makes the notebook Personal's, as choosing it under File > Profile > Personal does.
fn make_personal(opened: &FolderNotebook, cx: &mut TestAppContext) {
    let root = fs::canonicalize(opened.folder.path()).unwrap();
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(
                ProfileId::Personal,
                quire_store::profiles::Notebook::Folder { root },
            )?;
            profiles.activate(ProfileId::Personal)
        })
        .unwrap()
    });
}

fn personal(cx: &mut TestAppContext) -> quire_store::profiles::Profile {
    cx.update(|cx| crate::profiles::get(ProfileId::Personal, cx))
}

#[gpui::test]
fn git_sync_is_personal_only_then_backs_up_pushes_and_rereads_pulled_notes(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(&[("A.md", "- from here\n")], "A.md", cx);
    let notebook = &opened.notebook;
    let elsewhere = tempfile::tempdir().unwrap();
    let remote = bare(elsewhere.path());
    show(notebook, cx);
    wait_for_line(notebook, NOT_PERSONAL, cx);
    save_remote(&opened, &remote, cx);
    assert_eq!(settled_message(notebook, cx), NOT_PERSONAL);
    dispatch(notebook, SyncWithGit, cx);
    assert_eq!(settled_message(notebook, cx), NOT_PERSONAL);
    assert_eq!(
        cx.update(|cx| crate::profiles::get(ProfileId::Current, cx).git_remote),
        None
    );

    make_personal(&opened, cx);
    wait_for_line(notebook, "Personal Git is off", cx);
    save_remote(&opened, "https://me:secret@example.com/notes.git", cx);
    assert!(settled_message(notebook, cx).contains("contains credentials"));
    save_remote(&opened, &format!(" {remote} "), cx);
    assert_eq!(personal(cx).git_remote.as_deref(), Some(remote.as_str()));
    let backups = tempfile::tempdir().unwrap();
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_backup_folder(ProfileId::Personal, backups.path())
        })
        .unwrap()
    });

    dispatch(notebook, SyncWithGit, cx);
    let message = settled_message(notebook, cx);
    assert!(
        message.starts_with("Pushed ") && message.contains("once they sync"),
        "{message}"
    );
    let profile = personal(cx);
    assert!(profile.last_push_at.is_some());
    assert!(profile.last_backup_at.is_some());
    let made = profile.last_backup_path.unwrap();
    assert!(made.starts_with(fs::canonicalize(backups.path()).unwrap()));
    quire_store::backup::verify(&made).unwrap();
    assert!(
        lines(notebook, cx)
            .iter()
            .any(|line| line.starts_with("Last successful push"))
    );
    assert_eq!(
        git(Path::new(&remote), &["show", "main:A.md"]),
        "- from here"
    );

    // Another computer edits the note and pushes; syncing here brings the edit into the page.
    let other = elsewhere.path().join("other");
    git(
        elsewhere.path(),
        &["clone", "-q", "-b", "main", &remote, "other"],
    );
    fs::write(other.join("A.md"), "- from the other computer\n").unwrap();
    let target = Target {
        profile: Profile::Personal,
        root: &other,
        remote: Some(&remote),
    };
    let isolated = GIT_ENV
        .into_iter()
        .fold(git_sync::Git::default(), |git, (key, value)| {
            git.env(key, value)
        });
    git_sync::sync_folder(&isolated, &target, 2).unwrap();
    dispatch(notebook, SyncWithGit, cx);
    assert!(settled_message(notebook, cx).starts_with("Pushed "));
    assert_eq!(
        fs::read_to_string(opened.folder.path().join("A.md")).unwrap(),
        "- from the other computer\n"
    );
    wait_until(cx, "the page to show the pulled note", |cx| {
        page_text(notebook, cx) == "- from the other computer\n"
    });
}

#[gpui::test]
fn a_failed_push_shows_its_reason_until_a_later_sync_succeeds(cx: &mut TestAppContext) {
    let opened = open_folder(&[("A.md", "- kept\n")], "A.md", cx);
    let notebook = &opened.notebook;
    let elsewhere = tempfile::tempdir().unwrap();
    let remote = bare(elsewhere.path());
    let hook = Path::new(&remote).join("hooks/pre-receive");
    fs::write(&hook, "#!/bin/sh\necho 'quota exceeded' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
    show(notebook, cx);
    make_personal(&opened, cx);
    save_remote(&opened, &remote, cx);

    dispatch(notebook, SyncWithGit, cx);
    let message = settled_message(notebook, cx);
    assert!(
        message.starts_with("Git sync failed: Git push failed")
            && message.contains("quota exceeded"),
        "{message}"
    );
    assert!(
        lines(notebook, cx)
            .iter()
            .any(|line| line.starts_with("Last Git error: Git push failed"))
    );
    assert!(lines(notebook, cx).contains(&"No successful push yet.".to_owned()));
    let committed = git(opened.folder.path(), &["show", "HEAD:A.md"]);
    assert_eq!(committed, "- kept");

    fs::remove_file(&hook).unwrap();
    dispatch(notebook, SyncWithGit, cx);
    assert!(settled_message(notebook, cx).starts_with("Pushed "));
    assert!(
        !lines(notebook, cx)
            .iter()
            .any(|line| line.starts_with("Last Git error"))
    );
    assert_eq!(git(Path::new(&remote), &["show", "main:A.md"]), "- kept");
}

#[gpui::test]
fn a_repository_notebook_keeps_live_data_out_of_git_and_reports_tracked_databases(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(
        &[
            ("A.md", "- a\n"),
            (".quire/devices/rt/durability.sqlite", "live"),
        ],
        "A.md",
        cx,
    );
    let root = opened.folder.path();
    git(root, &["init", "-q", "-b", "main"]);
    git(root, &["add", "-f", ".quire/devices/rt/durability.sqlite"]);
    dispatch(&opened.notebook, crate::sync_status::CheckSyncFolder, cx);
    wait_for_line(
        &opened.notebook,
        "git rm -r --cached .quire/devices .quire/ai",
        cx,
    );
    let exclude = fs::read_to_string(root.join(".git/info/exclude")).unwrap();
    assert!(
        exclude.ends_with("/.quire/devices/\n/.quire/ai/\n"),
        "{exclude}"
    );
}
