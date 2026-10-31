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
    let synced = Synced { notebook, folder };
    let live = live(&synced, cx);
    wait_until(cx, "the first publication", |cx| {
        live.read_with(cx, |live, _| !live.is_publishing() && !live.is_importing())
    });
    synced
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
    let deadline = std::time::Instant::now() + crate::test_patience::PATIENCE;
    while !done(&live.read_with(cx, |live, _| live.activity())) {
        cx.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
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
