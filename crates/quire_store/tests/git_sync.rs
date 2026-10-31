//! Personal Git between two simulated computers: two clones and a bare remote in a temporary
//! folder. Git runs with a private global configuration, so no test reads the user's identity,
//! credential helpers, or signing settings, and no test touches the network.
#![cfg(unix)]

use anyhow::Result;
use quire_store::{
    git_sync::{self, Git, Profile, Pushed, Target},
    notebook::{Edit, NotebookStore, RescanLimits},
};
use std::{
    fs,
    os::unix::fs::PermissionsExt as _,
    path::{Path, PathBuf},
    process::Command,
};

struct Lab {
    directory: tempfile::TempDir,
    git: Git,
    remote: String,
}

impl Lab {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let directory_path = fs::canonicalize(directory.path()).unwrap();
        let config = directory_path.join("gitconfig");
        fs::write(
            &config,
            "[user]\n\tname = Test Writer\n\temail = test@example.invalid\n[init]\n\tdefaultBranch = main\n[commit]\n\tgpgsign = false\n[advice]\n\tdetachedHead = false\n",
        )
        .unwrap();
        let git = Git::default()
            .env("GIT_CONFIG_GLOBAL", &config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", &directory_path);
        let lab = Self {
            directory,
            git,
            remote: directory_path.join("remote.git").display().to_string(),
        };
        lab.run(
            &directory_path,
            &["init", "--bare", "-b", "main", &lab.remote],
        );
        lab
    }

    fn path(&self, name: &str) -> PathBuf {
        fs::canonicalize(self.directory.path()).unwrap().join(name)
    }

    fn command(&self, root: &Path, args: &[&str]) -> std::process::Output {
        let mut command = Command::new("git");
        command.current_dir(root).args(args);
        for key in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"] {
            command.env_remove(key);
        }
        command
            .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("HOME", self.path(""))
            .output()
            .unwrap()
    }

    fn run(&self, root: &Path, args: &[&str]) -> String {
        let output = self.command(root, args);
        assert!(
            output.status.success(),
            "git {args:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).trim().to_owned()
    }

    fn remote_head(&self) -> String {
        self.run(Path::new(&self.remote), &["rev-parse", "refs/heads/main"])
    }

    fn folder(&self, name: &str) -> PathBuf {
        let root = self.path(name);
        fs::create_dir(&root).unwrap();
        self.run(&root, &["init", "-b", "main"]);
        root
    }

    fn clone(&self, name: &str) -> PathBuf {
        let root = self.path(name);
        self.run(
            &self.path(""),
            &["clone", "-q", "-b", "main", &self.remote, name],
        );
        root
    }

    fn target<'a>(&'a self, root: &'a Path) -> Target<'a> {
        Target {
            profile: Profile::Personal,
            root,
            remote: Some(&self.remote),
        }
    }

    fn sync_folder(&self, root: &Path) -> Result<Pushed> {
        git_sync::sync_folder(&self.git, &self.target(root), 1_000)
    }

    fn sync_quire(&self, root: &Path) -> Result<Pushed> {
        git_sync::sync_quire(&self.git, &self.target(root), 1_000)
    }

    fn reject_pushes(&self, reject: bool) {
        let hook = Path::new(&self.remote).join("hooks/pre-receive");
        if reject {
            fs::write(&hook, "#!/bin/sh\necho 'remote refused' >&2\nexit 1\n").unwrap();
            fs::set_permissions(&hook, fs::Permissions::from_mode(0o755)).unwrap();
        } else {
            fs::remove_file(hook).unwrap();
        }
    }
}

fn write(root: &Path, path: &str, text: &str) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, text).unwrap();
}

fn read(root: &Path, path: &str) -> String {
    fs::read_to_string(root.join(path)).unwrap()
}

fn message(result: Result<Pushed>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn a_folder_notebook_hands_over_between_two_computers_by_fast_forward() -> Result<()> {
    let lab = Lab::new();
    let first = lab.folder("first");
    write(&first, "Note.md", "From first");
    let pushed = lab.sync_folder(&first)?;
    assert_eq!(lab.remote_head(), pushed.commit);
    assert_eq!(pushed.at, 1_000);

    let second = lab.clone("second");
    write(&second, "Note.md", "From second");
    lab.sync_folder(&second)?;
    lab.sync_folder(&first)?;
    assert_eq!(read(&first, "Note.md"), "From second");
    assert_eq!(lab.run(&first, &["status", "--porcelain"]), "");
    assert_eq!(lab.run(&first, &["rev-parse", "HEAD"]), lab.remote_head());
    let subject = lab.run(&first, &["log", "-1", "--format=%s"]);
    assert_eq!(subject, "Save personal notes 1970-01-01T00:00:01.000Z");
    Ok(())
}

#[test]
fn local_edits_are_kept_when_the_remote_changed_meanwhile() -> Result<()> {
    let lab = Lab::new();
    let first = lab.folder("first");
    write(&first, "Note.md", "Original");
    lab.sync_folder(&first)?;
    let second = lab.clone("second");
    write(&second, "Note.md", "Remote edit");
    lab.sync_folder(&second)?;
    write(&first, "Note.md", "Offline local edit");
    assert!(message(lab.sync_folder(&first)).contains("also has local edits"));
    assert_eq!(read(&first, "Note.md"), "Offline local edit");
    assert_eq!(lab.run(&first, &["show", "HEAD:Note.md"]), "Original");
    Ok(())
}

#[test]
fn divergent_commits_stop_without_resetting_either_side() -> Result<()> {
    let lab = Lab::new();
    let first = lab.folder("first");
    write(&first, "Note.md", "Original");
    lab.sync_folder(&first)?;
    let second = lab.clone("second");
    write(&second, "Note.md", "Remote");
    let pushed = lab.sync_folder(&second)?;
    write(&first, "Note.md", "Local commit");
    lab.run(&first, &["commit", "-qam", "Offline edit"]);
    let local = lab.run(&first, &["rev-parse", "HEAD"]);
    let error = message(lab.sync_folder(&first));
    assert!(
        error.contains("diverged") && error.contains("Both versions are retained"),
        "{error}"
    );
    assert_eq!(lab.run(&first, &["rev-parse", "HEAD"]), local);
    assert_eq!(lab.remote_head(), pushed.commit);
    assert_eq!(read(&first, "Note.md"), "Local commit");
    Ok(())
}

#[test]
fn a_rejected_push_keeps_the_local_commit_and_the_next_sync_delivers_it() -> Result<()> {
    let lab = Lab::new();
    let first = lab.folder("first");
    write(&first, "Note.md", "Original");
    let original = lab.sync_folder(&first)?;
    lab.reject_pushes(true);
    write(&first, "Note.md", "Retain locally");
    let error = message(lab.sync_folder(&first));
    assert!(
        error.contains("Git push failed") && error.contains("remote refused"),
        "{error}"
    );
    assert!(error.contains("Local notes are retained"), "{error}");
    assert_eq!(lab.run(&first, &["show", "HEAD:Note.md"]), "Retain locally");
    let kept = lab.run(&first, &["rev-parse", "HEAD"]);
    assert_eq!(lab.remote_head(), original.commit);
    assert_eq!(lab.run(&first, &["log", "--format=%s"]).lines().count(), 2);

    lab.reject_pushes(false);
    let delivered = lab.sync_folder(&first)?;
    assert_eq!(delivered.commit, kept);
    assert_eq!(lab.remote_head(), kept);
    // The remote only ever moved forward.
    lab.run(
        Path::new(&lab.remote),
        &["merge-base", "--is-ancestor", &original.commit, &kept],
    );
    Ok(())
}

#[test]
fn an_unreachable_remote_fails_the_fetch_and_changes_nothing_local() -> Result<()> {
    let lab = Lab::new();
    let first = lab.folder("first");
    write(&first, "Note.md", "Original");
    lab.sync_folder(&first)?;
    let away = lab.path("away.git");
    fs::rename(&lab.remote, &away)?;
    write(&first, "Note.md", "Written offline");
    let head = lab.run(&first, &["rev-parse", "HEAD"]);
    let error = message(lab.sync_folder(&first));
    assert!(
        error.starts_with("Git fetch failed") && error.contains("credentials, then retry"),
        "{error}"
    );
    assert_eq!(lab.run(&first, &["rev-parse", "HEAD"]), head);
    assert_eq!(read(&first, "Note.md"), "Written offline");
    fs::rename(&away, &lab.remote)?;
    assert_eq!(lab.sync_folder(&first)?.commit, lab.remote_head());
    Ok(())
}

#[test]
fn work_notebooks_and_folders_inside_a_larger_repository_are_refused() -> Result<()> {
    let lab = Lab::new();
    let root = lab.folder("first");
    let work = Target {
        profile: Profile::Work,
        ..lab.target(&root)
    };
    let error = git_sync::sync_folder(&lab.git, &work, 1).unwrap_err();
    assert!(error.to_string().contains("only for Personal"));
    let nested = root.join("nested");
    write(&nested, "Private.md", "Do not stage the parent");
    assert!(message(lab.sync_folder(&nested)).contains("larger Git repository"));
    assert_eq!(lab.run(&root, &["remote"]), "");
    let unset = Target {
        remote: None,
        ..lab.target(&root)
    };
    let error = git_sync::sync_folder(&lab.git, &unset, 1).unwrap_err();
    assert!(error.to_string().contains("configure its Git remote"));
    Ok(())
}

#[test]
fn a_different_origin_or_branch_on_the_remote_is_never_changed() -> Result<()> {
    let lab = Lab::new();
    let root = lab.folder("first");
    lab.run(&root, &["remote", "add", "origin", "/elsewhere.git"]);
    assert!(message(lab.sync_folder(&root)).contains("origin differs"));
    assert_eq!(
        lab.run(&root, &["remote", "get-url", "origin"]),
        "/elsewhere.git"
    );

    let other = lab.folder("other");
    lab.run(&other, &["checkout", "-q", "-b", "drafts"]);
    write(&other, "Note.md", "On drafts");
    lab.run(&other, &["add", "."]);
    lab.run(&other, &["commit", "-qm", "drafts"]);
    lab.run(&other, &["remote", "add", "origin", &lab.remote]);
    lab.run(&other, &["push", "-q", "origin", "drafts"]);
    let fresh = lab.folder("fresh");
    write(&fresh, "Mine.md", "Here");
    assert!(message(lab.sync_folder(&fresh)).contains("different branch"));
    assert!(
        lab.command(
            Path::new(&lab.remote),
            &["rev-parse", "--verify", "refs/heads/main"]
        )
        .status
        .code()
            != Some(0)
    );
    Ok(())
}

#[test]
fn live_databases_and_ai_transcripts_are_never_committed() -> Result<()> {
    let lab = Lab::new();
    let first = lab.folder("first");
    write(&first, ".quire/devices/device/durability.sqlite", "live");
    write(
        &first,
        ".quire/devices/device/durability.sqlite-wal",
        "live",
    );
    write(&first, ".quire/ai/runs/transcript.jsonl", "private prompt");
    write(&first, ".quire/history/device.sqlite", "snapshot");
    write(&first, "Note.md", "Saved");
    lab.sync_folder(&first)?;
    let tracked = lab.run(&first, &["ls-files"]);
    assert!(tracked.contains(".quire/history/device.sqlite"));
    assert!(
        !tracked.contains(".quire/devices") && !tracked.contains(".quire/ai"),
        "{tracked}"
    );
    let remote = lab.run(
        Path::new(&lab.remote),
        &["ls-tree", "-r", "--name-only", "main"],
    );
    assert!(
        !remote.contains(".quire/devices") && !remote.contains(".quire/ai"),
        "{remote}"
    );

    // The second computer has its own live data, which a fast-forward never touches.
    let second = lab.clone("second");
    write(
        &second,
        ".quire/devices/other/durability.sqlite",
        "second's live data",
    );
    write(
        &second,
        ".quire/ai/runs/second.jsonl",
        "second's transcript",
    );
    write(&first, "Note.md", "Edited on first");
    lab.sync_folder(&first)?;
    lab.sync_folder(&second)?;
    assert_eq!(read(&second, "Note.md"), "Edited on first");
    assert_eq!(
        read(&second, ".quire/ai/runs/second.jsonl"),
        "second's transcript"
    );

    // A remote that tracks live data is refused rather than replacing this computer's.
    let third = lab.clone("third");
    write(
        &third,
        ".quire/devices/third/durability.sqlite",
        "committed by hand",
    );
    lab.run(&third, &["add", "-f", ".quire/devices"]);
    lab.run(&third, &["commit", "-qm", "hand commit"]);
    lab.run(&third, &["push", "-q", "origin", "main"]);
    assert!(message(lab.sync_folder(&second)).contains("remote tracks live device databases"));
    assert!(!second.join(".quire/devices/third").exists());
    // And a folder that tracks it itself must untrack it first.
    assert!(message(lab.sync_folder(&third)).contains("Live device databases are tracked"));
    Ok(())
}

#[test]
fn opening_a_repository_notebook_excludes_live_data_once_and_reports_tracked_databases() {
    let lab = Lab::new();
    let root = lab.folder("first");
    let exclude = root.join(".git/info/exclude");
    fs::write(&exclude, "*.tmp").unwrap();
    assert!(git_sync::protect_notebook(&lab.git, &root).is_empty());
    let expected = "*.tmp\n/.quire/devices/\n/.quire/ai/\n";
    assert_eq!(fs::read_to_string(&exclude).unwrap(), expected);
    git_sync::protect_notebook(&lab.git, &root);
    assert_eq!(fs::read_to_string(&exclude).unwrap(), expected);
    assert_eq!(lab.run(&root, &["status", "--porcelain"]), "");

    write(&root, ".quire/devices/a.db", "x");
    lab.run(&root, &["add", "-f", ".quire/devices/a.db"]);
    let warnings = git_sync::protect_notebook(&lab.git, &root);
    assert!(warnings[0].contains("git rm -r --cached .quire/devices .quire/ai"));

    let nested = root.join("Nested");
    fs::create_dir(&nested).unwrap();
    assert!(git_sync::protect_notebook(&lab.git, &nested).is_empty());
    let plain = lab.path("plain");
    fs::create_dir(&plain).unwrap();
    assert!(git_sync::protect_notebook(&lab.git, &plain).is_empty());
    assert!(!plain.join(".git").exists());
    assert!(git_sync::protect_notebook(&Git::new("/nonexistent/git"), &root).is_empty());
}

#[test]
fn notes_left_only_in_the_cloud_stop_the_sync_before_any_commit() -> Result<()> {
    let lab = Lab::new();
    let root = lab.path("Library/Mobile Documents/com~apple~CloudDocs/Notes");
    fs::create_dir_all(&root)?;
    lab.run(&root, &["init", "-q", "-b", "main"]);
    write(&root, "Here.md", "Downloaded");
    write(&root, "Daily/.2026-10-30.md.icloud", "stub");
    let error = message(lab.sync_folder(&root));
    assert!(
        error.contains("1 notes, including Daily/2026-10-30.md, are only in iCloud Drive"),
        "{error}"
    );
    assert!(error.contains("Keep Downloaded"), "{error}");
    assert!(
        lab.command(&root, &["rev-parse", "--verify", "HEAD"])
            .status
            .code()
            != Some(0)
    );
    fs::remove_file(root.join("Daily/.2026-10-30.md.icloud"))?;
    write(&root, "Daily/2026-10-30.md", "Downloaded later");
    lab.sync_folder(&root)?;
    assert_eq!(
        lab.run(&root, &["ls-files"]),
        "Daily/2026-10-30.md\nHere.md"
    );
    Ok(())
}

#[test]
fn a_repository_is_created_on_first_sync_and_an_unfinished_merge_is_refused() -> Result<()> {
    let lab = Lab::new();
    let root = lab.path("new");
    fs::create_dir(&root)?;
    write(&root, "Note.md", "First");
    lab.sync_folder(&root)?;
    assert_eq!(lab.run(&root, &["symbolic-ref", "--short", "HEAD"]), "main");
    assert_eq!(lab.run(&root, &["remote", "get-url", "origin"]), lab.remote);

    let broken = lab.path("broken");
    fs::create_dir(&broken)?;
    fs::write(broken.join(".git"), "gitdir: /nowhere")?;
    assert!(message(lab.sync_folder(&broken)).contains("Repair its .git metadata"));

    let second = lab.clone("second");
    write(&second, "Note.md", "Second");
    lab.sync_folder(&second)?;
    write(&root, "Note.md", "Conflicting");
    lab.run(&root, &["commit", "-qam", "local"]);
    lab.run(&root, &["fetch", "-q", "origin"]);
    assert!(lab.command(&root, &["merge", "origin/main"]).status.code() != Some(0));
    assert!(message(lab.sync_folder(&root)).contains("unresolved conflicts"));
    Ok(())
}

fn quire_notebook(root: &Path) {
    write(
        root,
        "quire-notebook.json",
        r#"{"format":1,"id":"nb","created":1}"#,
    );
}

fn edit(page: &str, base: Vec<String>, text: &str) -> Edit {
    Edit {
        page: page.into(),
        base,
        path: format!("{page}.md"),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

fn text(store: &NotebookStore, page: &str) -> Result<Option<String>> {
    let heads = store.heads(page)?;
    let [head] = &heads[..] else {
        return Ok(None);
    };
    Ok(store
        .get(head)?
        .and_then(|revision| revision.root().field("text").as_str().map(str::to_owned)))
}

fn rescan(store: &NotebookStore, root: &Path) -> Result<quire_store::notebook::RescanReport> {
    store.rescan(root, RescanLimits::default(), None)
}

#[test]
fn quire_notebooks_merge_two_computers_histories_through_git() -> Result<()> {
    let lab = Lab::new();
    let office = lab.folder("office");
    quire_notebook(&office);
    let office_store = NotebookStore::open(&lab.path("office.sqlite"), "nb", "office")?;
    office_store.save(&edit("plan", vec![], "- start"))?;
    office_store.publish_to(&office, 10)?;
    lab.sync_quire(&office)?;

    let home = lab.clone("home");
    let home_store = NotebookStore::open(&lab.path("home.sqlite"), "nb", "home")?;
    assert_eq!(rescan(&home_store, &home)?.applied, 1);
    assert_eq!(text(&home_store, "plan")?.as_deref(), Some("- start"));

    // Both computers save, publish, and commit before either pulls: the histories diverge.
    home_store.save(&edit("home", vec![], "- home"))?;
    office_store.save(&edit("office", vec![], "- office"))?;
    home_store.publish_to(&home, 11)?;
    office_store.publish_to(&office, 12)?;
    lab.sync_quire(&home)?;
    lab.sync_quire(&office)?;
    lab.sync_quire(&home)?;
    for (store, root) in [(&home_store, &home), (&office_store, &office)] {
        let report = rescan(store, root)?;
        assert!(report.problems.is_empty(), "{:?}", report.problems);
        assert_eq!(text(store, "home")?.as_deref(), Some("- home"));
        assert_eq!(text(store, "office")?.as_deref(), Some("- office"));
        assert_eq!(lab.run(root, &["status", "--porcelain"]), "");
    }
    assert_eq!(lab.run(&home, &["rev-parse", "HEAD"]), lab.remote_head());
    assert_eq!(lab.run(&office, &["rev-parse", "HEAD"]), lab.remote_head());

    // Immutable names do not prevent every conflict: a hand edit to a shared file stops the sync
    // with no half-finished merge, and this computer's commit stays.
    write(&home, "quire-notebook.json", "{}\n");
    write(&office, "quire-notebook.json", "[]\n");
    lab.sync_quire(&home)?;
    let error = message(lab.sync_quire(&office));
    assert!(error.contains("outside Quire"), "{error}");
    assert!(!office.join(".git/MERGE_HEAD").exists());
    assert_eq!(read(&office, "quire-notebook.json"), "[]\n");
    assert_eq!(
        lab.run(&office, &["show", "HEAD:quire-notebook.json"]),
        "[]"
    );
    Ok(())
}

#[test]
fn a_copied_device_identity_merges_in_git_but_the_rescan_refuses_it_and_keeps_both_files()
-> Result<()> {
    let lab = Lab::new();
    let office = lab.folder("office");
    quire_notebook(&office);
    let original = lab.path("office.sqlite");
    let store = NotebookStore::open(&original, "nb", "office")?;
    let first = store.save(&edit("plan", vec![], "- start"))?;
    store.publish_to(&office, 10)?;
    lab.sync_quire(&office)?;
    drop(store);
    // The application data is copied to a second computer along with the notebook.
    let copied = lab.path("copied.sqlite");
    for suffix in ["", "-wal", "-shm"] {
        let from = PathBuf::from(format!("{}{suffix}", original.display()));
        if from.exists() {
            fs::copy(&from, format!("{}{suffix}", copied.display()))?;
        }
    }
    let home = lab.clone("home");
    let office_store = NotebookStore::open(&original, "nb", "office")?;
    let copy_store = NotebookStore::open(&copied, "nb", "office")?;
    let base = vec![first.revision_id().to_owned()];
    office_store.save(&edit("plan", base.clone(), "- office"))?;
    copy_store.save(&edit("plan", base, "- copy"))?;
    office_store.publish_to(&office, 11)?;
    copy_store.publish_to(&home, 12)?;
    lab.sync_quire(&office)?;
    lab.sync_quire(&home)?;
    lab.sync_quire(&office)?;
    let logs: Vec<_> = fs::read_dir(office.join("devices/office/log"))?
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("000000002-"))
        .collect();
    assert_eq!(
        logs.len(),
        2,
        "both computers' second change files are kept: {logs:?}"
    );
    let report = rescan(&office_store, &office)?;
    assert!(
        report
            .problems
            .iter()
            .any(|problem| problem.reason.contains("distinct writer identity")),
        "{:?}",
        report.problems
    );
    assert_eq!(text(&office_store, "plan")?.as_deref(), Some("- office"));
    Ok(())
}

#[test]
fn a_failed_push_keeps_the_published_change_files_and_a_later_sync_delivers_them() -> Result<()> {
    let lab = Lab::new();
    let office = lab.folder("office");
    quire_notebook(&office);
    let office_store = NotebookStore::open(&lab.path("office.sqlite"), "nb", "office")?;
    office_store.save(&edit("plan", vec![], "- start"))?;
    office_store.publish_to(&office, 10)?;
    let delivered = lab.sync_quire(&office)?;
    let home = lab.clone("home");
    let home_store = NotebookStore::open(&lab.path("home.sqlite"), "nb", "home")?;
    rescan(&home_store, &home)?;

    lab.reject_pushes(true);
    let saved = office_store.save(&edit(
        "plan",
        vec![office_store.heads("plan")?[0].clone()],
        "- later",
    ))?;
    let ack = office_store.publish_to(&office, 11)?.unwrap();
    let error = message(lab.sync_quire(&office));
    assert!(error.contains("Git push failed"), "{error}");
    let log = format!("devices/office/log/{}", ack.name);
    assert!(office.join(&log).is_file());
    assert_eq!(lab.run(&office, &["ls-files", "--", &log]), log);
    assert_eq!(lab.remote_head(), delivered.commit);
    // Publication to the folder is not delivery: the other computer still has the old text.
    lab.sync_quire(&home).ok();
    rescan(&home_store, &home)?;
    assert_eq!(text(&home_store, "plan")?.as_deref(), Some("- start"));

    lab.reject_pushes(false);
    lab.sync_quire(&office)?;
    lab.sync_quire(&home)?;
    rescan(&home_store, &home)?;
    assert_eq!(home_store.heads("plan")?, [saved.revision_id()]);
    Ok(())
}
