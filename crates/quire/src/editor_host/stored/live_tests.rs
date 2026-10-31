//! A Quire notebook window with a synced folder: saves publish, other writers' changes reach the
//! open page, and closing or quitting publishes first, asking when that is slow or fails.

use super::super::*;
use super::tests::{Notebook, read, save, settle, wait_until};
use crate::lifecycle::{
    Activity, CLOSE_ANYWAY, FAILED, KEEP_EDITING, KEEP_WAITING, PATIENCE, POLL, STILL_WORKING,
};
use crate::live_sync::LiveSync;
use futures::{FutureExt as _, channel::oneshot};
use gpui::TestAppContext;
use quire_store::{
    notebook::{Edit, NotebookStore},
    worker::{QueueError, Reply},
};
use std::{fs, os::unix::fs::PermissionsExt as _, path::Path};

pub(super) struct Synced {
    pub notebook: Notebook,
    pub folder: PathBuf,
}

/// Opens page `p-a` of a Quire notebook whose synced folder is `notebook/` beside its database.
pub(super) fn open_synced(text: &str, cx: &mut TestAppContext) -> Synced {
    let synced = open_synced_after(text, |_, _| {}, cx);
    let live = live(&synced, cx);
    wait_until(cx, "the first publication", |cx| {
        live.read_with(cx, |live, _| !live.is_publishing() && !live.is_importing())
    });
    synced
}

/// Like `open_synced`, with `prepare` given the store and the synced folder before the window
/// opens, and without waiting for the first publication.
fn open_synced_after(
    text: &str,
    prepare: impl FnOnce(&NotebookStore, &Path),
    cx: &mut TestAppContext,
) -> Synced {
    cx.update(|cx| init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(root.path()).unwrap();
    let folder = base.join("notebook");
    fs::create_dir(&folder).unwrap();
    fs::write(
        folder.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )
    .unwrap();
    let path = base.join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    save(&store, "p-a", "A.md", text);
    prepare(&store, &folder);
    drop(store);
    let mut session = NotebookSession::open_synced(
        path.clone(),
        "nb".into(),
        "local".into(),
        Some(folder.clone()),
    )
    .unwrap();
    let page = cx.update(|cx| session.open_page("p-a".into(), cx));
    let window = cx.add_window(|window, cx| {
        StoredWindow::new(session, page.clone(), path.clone(), "nb".into(), window, cx)
    });
    let notebook = Notebook {
        _root: root,
        path,
        window,
    };
    settle(&notebook, cx);
    Synced { notebook, folder }
}

fn live(synced: &Synced, cx: &mut TestAppContext) -> Entity<LiveSync> {
    read(&synced.notebook, cx, |view, _| view.live.clone().unwrap())
}

fn block<T>(reply: Result<Reply<T>, QueueError>) -> T {
    futures::executor::block_on(reply.unwrap()).unwrap()
}

fn client(synced: &Synced, cx: &mut TestAppContext) -> quire_store::worker::StorageClient {
    match read(&synced.notebook, cx, |view, _| {
        view.session.borrow().storage()
    }) {
        crate::notebook_backend::NotebookBackend::Store(client) => client,
        crate::notebook_backend::NotebookBackend::Folder(_) => unreachable!(),
    }
}

/// Saves page `p-b` through the window's storage, as another view of this notebook would.
fn save_elsewhere(synced: &Synced, text: &str, cx: &mut TestAppContext) {
    let client = client(synced, cx);
    block(client.save(Edit {
        page: "p-b".into(),
        base: block(client.heads("p-b".into())),
        path: "B.md".into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }));
    let live = live(synced, cx);
    wait_until_heard(&live, cx);
}

fn wait_until_heard(live: &Entity<LiveSync>, cx: &mut TestAppContext) {
    until_live(live, cx, "the save to be heard", |activity| {
        *activity != Activity::Idle
    });
}

/// Lets the storage thread answer without moving the clock, so a close never runs out of
/// patience while the machine is busy, then lets the close look again.
fn until_live(
    live: &Entity<LiveSync>,
    cx: &mut TestAppContext,
    what: &str,
    done: impl Fn(&Activity) -> bool,
) {
    cx.executor().allow_parking();
    crate::test_wait::until(
        cx,
        std::time::Duration::from_millis(1),
        crate::test_wait::park,
        |cx| done(&live.read_with(cx, |live, _| live.activity())),
        |_| what.into(),
    );
    cx.executor().advance_clock(POLL);
    cx.run_until_parked();
}

fn failed(activity: &Activity) -> bool {
    matches!(activity, Activity::Failed(_))
}

fn published(folder: &Path, writer: &str) -> usize {
    fs::read_dir(folder.join("devices").join(writer).join("log"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with('.'))
                .count()
        })
        .unwrap_or(0)
}

fn outbox(synced: &Synced, cx: &mut TestAppContext) -> u64 {
    block(client(synced, cx).publication_state()).outbox
}

fn page_text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let persistence = read(notebook, cx, |view, cx| {
        view.page.read(cx).persistence().cloned().unwrap()
    });
    cx.update(|cx| persistence.read(cx).buffer().read(cx).text())
}

#[gpui::test]
fn saves_publish_and_another_writers_change_reaches_the_open_page(cx: &mut TestAppContext) {
    let synced = open_synced("- one\n", cx);
    assert_eq!(
        published(&synced.folder, "local"),
        1,
        "the page saved before opening"
    );
    let live = live(&synced, cx);
    let line = live.read_with(cx, |live, _| live.line(&|_| String::new()));
    assert_eq!(line, "Published");

    let office_dir = tempfile::tempdir().unwrap();
    let office =
        NotebookStore::open(&office_dir.path().join("local.sqlite"), "nb", "office").unwrap();
    office
        .rescan(&synced.folder, Default::default(), None)
        .unwrap();
    save(&office, "p-a", "A.md", "- one\n- from the office\n");
    office.publish_to(&synced.folder, 5).unwrap().unwrap();
    wait_until(cx, "the office's line to reach the open page", |cx| {
        page_text(&synced.notebook, cx) == "- one\n- from the office\n"
    });
}

/// Writes one revision as the `peer` writer's batch `seq` into the synced folder. Returns the
/// batch's hash and bytes.
fn peer_batch(
    folder: &Path,
    seq: u32,
    previous: Option<&str>,
    revision: &serde_json::Value,
) -> (String, String, Vec<u8>) {
    use quire_store::sync::format::{Kind, encode};
    let batch = encode(
        &serde_json::json!({"notebook":"nb","device":"peer","seq":seq,"previous":previous,"created":2,"revisions":[revision],"attachments":[]}),
        Kind::Batch,
    )
    .unwrap();
    let log = folder.join("devices/peer/log");
    fs::create_dir_all(&log).unwrap();
    fs::write(log.join(&batch.name), &batch.data).unwrap();
    (batch.hash, batch.name, batch.data)
}

fn with_id(mut revision: serde_json::Value) -> serde_json::Value {
    revision["id"] = quire_store::sync::format::revision_id(&revision)
        .unwrap()
        .into();
    revision
}

/// rt `engine.ts` `resolveConflict` at 5ea5c22310ff: another computer's choice between a page's
/// competing versions reaches this one by import, and the open page, shown read-only with its
/// versions, opens again with the chosen text and editing allowed.
#[gpui::test]
fn an_imported_choice_reopens_a_page_with_competing_versions(cx: &mut TestAppContext) {
    let competing = std::cell::RefCell::new(Vec::new());
    let first = std::cell::RefCell::new(String::new());
    let synced = open_synced_after(
        "- here\n",
        |store, folder| {
            let other = with_id(
                serde_json::json!({"page":"p-a","parents":[],"generation":0,"path":"A.md","text":"- from the peer\n","deleted":false,"created":2,"device":"peer"}),
            );
            let (hash, name, data) = peer_batch(folder, 1, None, &other);
            store.import_batch("peer", &name, &data).unwrap();
            // A file among this computer's own changes that belongs to another notebook is a
            // problem the folder keeps reporting, and merging waits until this computer's own
            // changes read cleanly, so the competing versions stay, as a failed merge leaves
            // them.
            let stray = quire_store::sync::format::encode(
                &serde_json::json!({"notebook":"another","device":"local","seq":9,"previous":"0000000000000000000000000000000000000000000000000000000000000000","created":2,"revisions":[],"attachments":[]}),
                quire_store::sync::format::Kind::Batch,
            )
            .unwrap();
            let own = folder.join("devices/local/log");
            fs::create_dir_all(&own).unwrap();
            fs::write(own.join(&stray.name), &stray.data).unwrap();
            *first.borrow_mut() = hash;
            *competing.borrow_mut() = store.heads("p-a").unwrap();
        },
        cx,
    );
    let mut heads = competing.into_inner();
    assert_eq!(heads.len(), 2, "the merge never ran, so both versions stay");
    wait_until(cx, "the page with competing versions", |cx| {
        read(&synced.notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .loaded()
                .is_some_and(|page| page.conflict.is_some() && page.persistence.is_none())
        })
    });

    heads.sort();
    let chosen = with_id(
        serde_json::json!({"page":"p-a","parents":heads,"generation":1,"path":"A.md","text":"- chosen on the peer\n","deleted":false,"created":3,"device":"peer"}),
    );
    peer_batch(&synced.folder, 2, Some(&first.into_inner()), &chosen);
    wait_until(cx, "the page to reopen with the chosen version", |cx| {
        read(&synced.notebook, cx, |view, cx| {
            view.page.read(cx).loaded().is_some_and(|page| {
                page.conflict.is_none() && page.heads.len() == 1 && page.persistence.is_some()
            })
        })
    });
    assert_eq!(page_text(&synced.notebook, cx), "- chosen on the peer\n");
    read(&synced.notebook, cx, |view, cx| {
        assert_eq!(view.page.read(cx).id(), "p-a");
        assert!(view.render_version_conflict(cx).is_none());
    });
}

#[gpui::test]
fn quitting_waits_for_a_slow_publication_and_finishes_once_it_lands(cx: &mut TestAppContext) {
    let synced = open_synced("- one\n", cx);
    let live = live(&synced, cx);
    let (release, held) = oneshot::channel();
    live.update(cx, |live, _| live.hold = Some(held.shared()));
    save_elsewhere(&synced, "- two\n", cx);
    assert!(!cx.simulate_native_quit_request());
    cx.executor().advance_clock(PATIENCE);
    cx.run_until_parked();
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(message, STILL_WORKING);
    assert!(
        detail.starts_with("Saved changes are being published to the synced folder."),
        "{detail}"
    );
    cx.simulate_prompt_answer(KEEP_WAITING);
    cx.run_until_parked();
    assert_eq!(cx.platform_quit_count(), 0);
    release.send(()).unwrap();
    until_live(&live, cx, "the publication", |activity| {
        *activity == Activity::Idle
    });
    assert_eq!(cx.platform_quit_count(), 1);
    assert_eq!(published(&synced.folder, "local"), 2);
    assert_eq!(outbox(&synced, cx), 0);
}

#[gpui::test]
fn a_failed_publication_asks_at_once_keeps_the_save_and_retries_on_the_next_quit(
    cx: &mut TestAppContext,
) {
    let synced = open_synced("- one\n", cx);
    let live = live(&synced, cx);
    let log = synced.folder.join("devices/local/log");
    fs::set_permissions(&log, fs::Permissions::from_mode(0o500)).unwrap();
    save_elsewhere(&synced, "- two\n", cx);
    assert!(!cx.simulate_native_quit_request());
    until_live(&live, cx, "the publication to fail", failed);
    let (message, detail) = cx.pending_prompt().unwrap();
    assert_eq!(message, FAILED);
    assert!(
        detail.starts_with("Saved changes could not be published to the synced folder:"),
        "{detail}"
    );
    assert!(
        detail.contains("Quire publishes them the next time this notebook opens"),
        "{detail}"
    );
    cx.simulate_prompt_answer(KEEP_EDITING);
    cx.run_until_parked();
    assert_eq!(cx.platform_quit_count(), 0);
    let error = read(&synced.notebook, cx, |view, _| view.close_error.clone()).unwrap();
    assert!(
        error.starts_with("Saved changes could not be published"),
        "{error}"
    );
    assert_eq!(outbox(&synced, cx), 1, "the save stays on this computer");

    fs::set_permissions(&log, fs::Permissions::from_mode(0o700)).unwrap();
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(10));
    assert!(!cx.simulate_native_quit_request());
    until_live(&live, cx, "the retried publication", |activity| {
        *activity == Activity::Idle
    });
    assert_eq!(cx.platform_quit_count(), 1);
    assert_eq!(published(&synced.folder, "local"), 2);
}

#[gpui::test]
fn an_unavailable_folder_lets_close_anyway_quit_with_the_save_kept_for_next_time(
    cx: &mut TestAppContext,
) {
    let synced = open_synced("- one\n", cx);
    let live = live(&synced, cx);
    let away = synced.folder.with_file_name("unplugged");
    fs::rename(&synced.folder, &away).unwrap();
    save_elsewhere(&synced, "- two\n", cx);
    assert!(!cx.simulate_native_quit_request());
    until_live(&live, cx, "the publication to fail", failed);
    assert_eq!(cx.pending_prompt().unwrap().0, FAILED);
    cx.simulate_prompt_answer(CLOSE_ANYWAY);
    cx.run_until_parked();
    assert_eq!(cx.platform_quit_count(), 1);
    assert_eq!(outbox(&synced, cx), 1, "nothing saved is lost");
    assert_eq!(published(&away, "local"), 1);
}
const OTHER_COMPUTER: [(&str, &str); 6] = [
    ("GIT_CONFIG_GLOBAL", "/dev/null"),
    ("GIT_CONFIG_NOSYSTEM", "1"),
    ("GIT_AUTHOR_NAME", "Other Computer"),
    ("GIT_AUTHOR_EMAIL", "other@example.invalid"),
    ("GIT_COMMITTER_NAME", "Other Computer"),
    ("GIT_COMMITTER_EMAIL", "other@example.invalid"),
];

fn run_git(root: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git")
        .current_dir(root)
        .args(args)
        .envs(OTHER_COMPUTER)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn sync_message(synced: &Synced, cx: &mut TestAppContext) -> String {
    cx.dispatch_action(
        synced.notebook.window.into(),
        crate::sync_status::SyncWithGit,
    );
    let status = read(&synced.notebook, cx, |view, _| view.sync.clone());
    wait_until(cx, "the sync to finish", |cx| {
        status.read_with(cx, |status, _| !status.is_running())
    });
    status.read_with(cx, |status, _| {
        status
            .message()
            .map(|message| message.to_string())
            .unwrap_or_default()
    })
}

#[gpui::test]
fn personal_sync_publishes_first_then_brings_another_computers_change_into_the_open_page(
    cx: &mut TestAppContext,
) {
    use quire_store::git_sync::{self as sync, Profile, Target};
    use quire_store::profiles::ProfileId;
    let synced = open_synced("- one\n", cx);
    let temp = tempfile::tempdir().unwrap();
    let elsewhere = fs::canonicalize(temp.path()).unwrap();
    let remote = elsewhere.join("remote.git");
    run_git(
        &elsewhere,
        &[
            "init",
            "-q",
            "--bare",
            "-b",
            "main",
            remote.to_str().unwrap(),
        ],
    );
    let remote = remote.display().to_string();
    let location = read(&synced.notebook, cx, |view, _| {
        view.location.clone().unwrap()
    });
    cx.update(|cx| {
        crate::profiles::update(cx, |profiles| {
            profiles.set_notebook(ProfileId::Personal, location)?;
            profiles.activate(ProfileId::Personal)?;
            profiles
                .set_git_remote(ProfileId::Personal, &remote)
                .map(drop)
        })
        .unwrap()
    });

    save_elsewhere(&synced, "- saved just before syncing\n", cx);
    let message = sync_message(&synced, cx);
    assert!(message.starts_with("Pushed "), "{message}");
    let pushed = run_git(
        Path::new(&remote),
        &["ls-tree", "-r", "--name-only", "main"],
    );
    assert_eq!(
        pushed
            .lines()
            .filter(|path| path.starts_with("devices/local/log/"))
            .count(),
        2,
        "the save went out before the commit: {pushed}"
    );
    assert!(pushed.contains("quire-notebook.json"), "{pushed}");

    // Another computer clones the notebook, edits the open page, publishes, and pushes.
    let other = elsewhere.join("other");
    run_git(&elsewhere, &["clone", "-q", "-b", "main", &remote, "other"]);
    let office = NotebookStore::open(&elsewhere.join("office.sqlite"), "nb", "office").unwrap();
    office.rescan(&other, Default::default(), None).unwrap();
    save(&office, "p-a", "A.md", "- one\n- from the other computer\n");
    office.publish_to(&other, 9).unwrap().unwrap();
    let isolated = OTHER_COMPUTER
        .into_iter()
        .fold(sync::Git::default(), |runner, (key, value)| {
            runner.env(key, value)
        });
    let target = Target {
        profile: Profile::Personal,
        root: &other,
        remote: Some(&remote),
    };
    sync::sync_quire(&isolated, &target, 10).unwrap();

    let message = sync_message(&synced, cx);
    assert!(message.starts_with("Pushed "), "{message}");
    wait_until(
        cx,
        "the other computer's line to reach the open page",
        |cx| page_text(&synced.notebook, cx) == "- one\n- from the other computer\n",
    );
    assert_eq!(run_git(&synced.folder, &["status", "--porcelain"]), "");
}
