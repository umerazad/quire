//! The live sync scheduler against real stores and a real synced folder, with the executor's
//! fake clock standing in for time: publish windows, heartbeats, import reruns, retried partial
//! files, the watcher's filter, and two writers converging through one folder.

use super::*;
use futures::{FutureExt as _, channel::oneshot};
use gpui::{Entity, TestAppContext};
use quire_store::{
    notebook::Edit,
    worker::{QueueLimits, StorageWorker},
};
use std::fs;

const START: u64 = 1_800_000_000_000;

struct Folder {
    _temp: tempfile::TempDir,
    root: PathBuf,
    data: PathBuf,
}

fn folder() -> Folder {
    let temp = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(temp.path()).unwrap();
    let root = base.join("notebook");
    fs::create_dir(&root).unwrap();
    fs::write(
        root.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )
    .unwrap();
    Folder {
        _temp: temp,
        root,
        data: base,
    }
}

fn store(folder: &Folder, device: &str) -> StorageClient {
    let directory = folder.data.join(device);
    fs::create_dir_all(&directory).unwrap();
    let worker = StorageWorker::spawn(
        directory.join("local.sqlite"),
        "nb".into(),
        device.into(),
        QueueLimits::default(),
    )
    .unwrap();
    futures::executor::block_on(worker.ready).unwrap();
    worker.client
}

/// Milliseconds on the executor's fake clock.
fn clock(cx: &TestAppContext) -> Clock {
    let executor = cx.executor();
    let start = executor.now();
    Rc::new(move || START + executor.now().duration_since(start).as_millis() as u64)
}

fn identity(device: &str, label: &str) -> Identity {
    Identity {
        device: device.into(),
        label: label.into(),
        app: "quire-native test".into(),
    }
}

fn start(
    client: &StorageClient,
    folder: &Folder,
    identity: Identity,
    watch: bool,
    cx: &mut TestAppContext,
) -> Entity<LiveSync> {
    let clock = clock(cx);
    let root = folder.root.clone();
    let client = client.clone();
    let sync = cx.new(|cx| LiveSync::new(client, root, identity, clock, watch, cx));
    settle(cx, "the first import and announcement", |cx| {
        sync.read_with(cx, |sync, _| {
            sync.announced && !sync.imports.running() && !sync.publishing
        })
    });
    sync
}

/// Lets storage threads answer without moving the fake clock.
fn settle(cx: &mut TestAppContext, what: &str, mut done: impl FnMut(&mut TestAppContext) -> bool) {
    cx.executor().allow_parking();
    let deadline = std::time::Instant::now() + crate::test_patience::PATIENCE;
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Moves the clock in small steps, letting storage threads answer, until `done` holds.
fn run_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    cx.executor().allow_parking();
    let deadline = std::time::Instant::now() + crate::test_patience::PATIENCE;
    loop {
        cx.executor().advance_clock(Duration::from_millis(25));
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn block<T>(reply: Result<Reply<T>, QueueError>) -> T {
    futures::executor::block_on(reply.unwrap()).unwrap()
}

/// Saves `text` as page `page` and waits until live sync has heard of it.
fn save(
    client: &StorageClient,
    sync: &Entity<LiveSync>,
    page: &str,
    text: &str,
    cx: &mut TestAppContext,
) {
    let heard = sync.read_with(cx, |sync, _| sync.saves);
    block(client.save(Edit {
        page: page.into(),
        base: block(client.heads(page.into())),
        path: format!("{page}.md"),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }));
    settle(cx, "the save to be heard", |cx| {
        sync.read_with(cx, |sync, _| sync.saves > heard)
    });
}

fn log(folder: &Folder, device: &str) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder.root.join("devices").join(device).join("log"))
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| entry.file_name().to_string_lossy().into_owned())
                .filter(|name| !name.starts_with('.'))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn status(folder: &Folder, device: &str) -> DeviceStatus {
    serde_json::from_slice(
        &fs::read(folder.root.join("devices").join(device).join("status.json")).unwrap(),
    )
    .unwrap()
}

fn advance(by: u64, cx: &mut TestAppContext) {
    cx.executor().advance_clock(Duration::from_millis(by));
    cx.run_until_parked();
}

/// Advances `by` milliseconds, then lets a publication the clock started finish.
fn advance_and_settle(by: u64, sync: &Entity<LiveSync>, cx: &mut TestAppContext) {
    advance(by, cx);
    settle(cx, "publishing to stop", |cx| {
        sync.read_with(cx, |sync, _| !sync.publishing)
    });
}

fn text(client: &StorageClient, page: &str) -> Option<String> {
    let heads = block(client.heads(page.into()));
    let [head] = heads.as_slice() else {
        return None;
    };
    block(client.get(head.clone()))
        .and_then(|record| record.root().field("text").as_str().map(str::to_owned))
}

#[gpui::test]
fn alone_saves_publish_after_a_second_of_quiet_and_at_most_five_seconds(cx: &mut TestAppContext) {
    let folder = folder();
    let client = store(&folder, "desk");
    let sync = start(&client, &folder, identity("desk", "mac"), false, cx);
    assert!(!sync.read_with(cx, |sync, _| sync.nearby));
    save(&client, &sync, "a", "one", cx);
    advance_and_settle(999, &sync, cx);
    assert!(
        log(&folder, "desk").is_empty(),
        "not before a second of quiet"
    );
    advance_and_settle(1, &sync, cx);
    assert_eq!(log(&folder, "desk").len(), 1);
    assert_eq!(sync.read_with(cx, |sync, _| sync.pending), 0);

    for round in 0..5 {
        save(&client, &sync, "a", &format!("steady {round}"), cx);
        advance_and_settle(900, &sync, cx);
        assert_eq!(log(&folder, "desk").len(), 1, "typing steadily postpones");
    }
    save(&client, &sync, "a", "last", cx);
    advance_and_settle(499, &sync, cx);
    assert_eq!(log(&folder, "desk").len(), 1);
    advance_and_settle(1, &sync, cx);
    assert_eq!(
        log(&folder, "desk").len(),
        2,
        "five seconds after the first save, the burst goes out as one file"
    );
}

#[gpui::test]
fn near_another_app_saves_publish_after_150_ms_and_at_most_one_second(cx: &mut TestAppContext) {
    let folder = folder();
    let now = clock(cx)();
    live_sync::write_status(
        &folder.root,
        &DeviceStatus {
            format: 1,
            app: "quire-tui 0.9.57".into(),
            device: "tui".into(),
            label: "mac".into(),
            last_seq: 0,
            outbox: 0,
            heartbeat: now,
        },
    )
    .unwrap();
    let client = store(&folder, "desk");
    let sync = start(&client, &folder, identity("desk", "mac"), false, cx);
    assert!(sync.read_with(cx, |sync, _| sync.nearby));
    save(&client, &sync, "a", "one", cx);
    advance_and_settle(149, &sync, cx);
    assert!(log(&folder, "desk").is_empty());
    advance_and_settle(1, &sync, cx);
    assert_eq!(log(&folder, "desk").len(), 1);

    for round in 0..9 {
        save(&client, &sync, "a", &format!("steady {round}"), cx);
        advance_and_settle(100, &sync, cx);
    }
    assert_eq!(log(&folder, "desk").len(), 1);
    save(&client, &sync, "a", "last", cx);
    advance_and_settle(100, &sync, cx);
    assert_eq!(
        log(&folder, "desk").len(),
        2,
        "at most a second after the first save"
    );
}

#[gpui::test]
fn a_stale_or_differently_labelled_neighbour_leaves_the_calm_window(cx: &mut TestAppContext) {
    let folder = folder();
    let now = clock(cx)();
    let other = |device: &str, label: &str, heartbeat: u64| DeviceStatus {
        format: 1,
        app: "0.9.57".into(),
        device: device.into(),
        label: label.into(),
        last_seq: 0,
        outbox: 0,
        heartbeat,
    };
    live_sync::write_status(&folder.root, &other("old", "mac", now - 120_000)).unwrap();
    live_sync::write_status(&folder.root, &other("laptop", "laptop", now)).unwrap();
    let client = store(&folder, "desk");
    let sync = start(&client, &folder, identity("desk", "mac"), false, cx);
    assert!(!sync.read_with(cx, |sync, _| sync.nearby));
    save(&client, &sync, "a", "one", cx);
    advance_and_settle(150, &sync, cx);
    assert!(
        log(&folder, "desk").is_empty(),
        "no fast publishing without a live neighbour"
    );
    advance_and_settle(850, &sync, cx);
    assert_eq!(log(&folder, "desk").len(), 1);
}

#[gpui::test]
fn the_status_is_written_at_open_kept_while_unchanged_and_refreshed_each_minute(
    cx: &mut TestAppContext,
) {
    let folder = folder();
    let client = store(&folder, "desk");
    let sync = start(&client, &folder, identity("desk", "mac"), false, cx);
    let opened = status(&folder, "desk");
    assert_eq!(
        (
            opened.label.as_str(),
            opened.app.as_str(),
            opened.outbox,
            opened.last_seq
        ),
        ("mac", "quire-native test", 0, 0)
    );
    assert_eq!(opened.heartbeat, START);
    save(&client, &sync, "a", "one", cx);
    advance_and_settle(1_000, &sync, cx);
    assert_eq!(log(&folder, "desk").len(), 1);
    assert_eq!(
        status(&folder, "desk"),
        opened,
        "a publication that leaves nothing waiting keeps the unchanged status"
    );
    advance_and_settle(29_000 - 1_000, &sync, cx);
    advance_and_settle(1_000, &sync, cx);
    assert_eq!(
        status(&folder, "desk"),
        opened,
        "the 30 s rescan does not rewrite it"
    );
    advance_and_settle(30_000, &sync, cx);
    let refreshed = status(&folder, "desk");
    assert_eq!(
        refreshed.heartbeat,
        START + 60_000,
        "a minute later, the heartbeat"
    );
    assert_eq!(refreshed.last_seq, 1);
}

#[gpui::test]
fn an_import_asked_for_during_an_import_runs_again_for_the_late_file(cx: &mut TestAppContext) {
    let folder = folder();
    let office = store(&folder, "office");
    let office_sync = start(&office, &folder, identity("office", "work"), false, cx);
    let home = store(&folder, "home");
    let home_sync = start(&home, &folder, identity("home", "home"), false, cx);

    save(&office, &office_sync, "a", "first", cx);
    office_sync.update(cx, |sync, cx| sync.publish(cx));
    settle(cx, "the first file", |_| log(&folder, "office").len() == 1);

    let (release, held) = oneshot::channel();
    home_sync.update(cx, |sync, cx| {
        sync.hold_import = Some(held.shared());
        sync.import(cx);
    });
    cx.run_until_parked();
    // The worker answers in order, so this reply means the held import's rescan is done.
    block(home.publication_state());
    assert_eq!(text(&home, "a").as_deref(), Some("first"));

    save(&office, &office_sync, "a", "late", cx);
    office_sync.update(cx, |sync, cx| sync.publish(cx));
    settle(cx, "the late file", |_| log(&folder, "office").len() == 2);
    home_sync.update(cx, |sync, cx| {
        assert!(sync.imports.running());
        sync.import(cx);
        sync.hold_import = None;
    });
    release.send(()).unwrap();
    settle(cx, "the imports to finish", |cx| {
        home_sync.read_with(cx, |sync, _| !sync.imports.running())
    });
    assert_eq!(text(&home, "a").as_deref(), Some("late"));
}

/// Publishes one change file from `client` and returns its path in the synced folder.
fn publish_one(folder: &Folder, client: &StorageClient, page: &str, text: &str) -> PathBuf {
    block(client.save(Edit {
        page: page.into(),
        base: block(client.heads(page.into())),
        path: format!("{page}.md"),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }));
    let ack = block(client.publish_to(folder.root.clone(), 2)).unwrap();
    folder.root.join("devices/office/log").join(ack.name)
}

#[gpui::test]
fn a_part_written_change_file_is_read_again_after_a_second_without_a_warning(
    cx: &mut TestAppContext,
) {
    let folder = folder();
    let office = store(&folder, "office");
    let file = publish_one(&folder, &office, "a", "from the office");
    let whole = fs::read(&file).unwrap();
    fs::write(&file, &whole[..whole.len() / 2]).unwrap();
    let home = store(&folder, "home");
    let sync = start(&home, &folder, identity("home", "home"), false, cx);
    assert_eq!(text(&home, "a"), None);
    assert!(sync.read_with(cx, |sync, _| sync.retry.is_some()));
    assert!(
        sync.read_with(cx, |sync, _| sync.warnings.is_empty()),
        "no warning on the first read"
    );
    fs::write(&file, &whole).unwrap();
    advance(999, cx);
    assert_eq!(text(&home, "a"), None, "not read again before a second");
    advance(1, cx);
    settle(cx, "the retry", |cx| {
        sync.read_with(cx, |sync, _| !sync.imports.running())
    });
    assert_eq!(text(&home, "a").as_deref(), Some("from the office"));
    assert!(sync.read_with(cx, |sync, _| sync.warnings.is_empty()));
    assert!(sync.read_with(cx, |sync, _| sync.retry.is_none()));
}

#[gpui::test]
fn a_change_file_still_damaged_after_the_retry_warns_once(cx: &mut TestAppContext) {
    let folder = folder();
    let office = store(&folder, "office");
    let file = publish_one(&folder, &office, "a", "from the office");
    let whole = fs::read(&file).unwrap();
    fs::write(&file, &whole[..whole.len() / 2]).unwrap();
    let home = store(&folder, "home");
    let sync = start(&home, &folder, identity("home", "home"), false, cx);
    advance(1_000, cx);
    settle(cx, "the retry", |cx| {
        sync.read_with(cx, |sync, _| {
            !sync.imports.running() && !sync.warnings.is_empty()
        })
    });
    let warnings = sync.read_with(cx, |sync, _| sync.warnings.clone());
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert!(
        warnings[0].starts_with("Changes from another computer could not be read ("),
        "{warnings:?}"
    );
    assert!(
        warnings[0].ends_with("Quire will try again."),
        "{warnings:?}"
    );
    advance(1_000, cx);
    settle(cx, "another retry", |cx| {
        sync.read_with(cx, |sync, _| !sync.imports.running())
    });
    assert_eq!(
        sync.read_with(cx, |sync, _| sync.warnings.len()),
        1,
        "the same warning once"
    );
}

#[gpui::test]
fn the_watcher_wakes_an_import_only_for_delivered_files_after_50_ms(cx: &mut TestAppContext) {
    let folder = folder();
    let home = store(&folder, "home");
    let sync = start(&home, &folder, identity("home", "home"), false, cx);
    let roots = vec![folder.root.clone()];
    let log = folder.root.join("devices/office/log");
    sync.update(cx, |sync, cx| {
        sync.changed(
            &roots,
            [
                log.join(".quire-1234"),
                folder.root.join("devices/office/.status-x"),
                folder
                    .root
                    .join("devices/.office/log/0000000001-a.qlog.zst"),
                folder.root.join("attachments/a.png"),
                folder.root.join("quire-notebook.json"),
            ],
            cx,
        )
    });
    assert!(
        sync.read_with(cx, |sync, _| sync.debounce.is_none()),
        "temporary and outside files are ignored"
    );

    let office = store(&folder, "office");
    let file = publish_one(&folder, &office, "a", "from the office");
    sync.update(cx, |sync, cx| sync.changed(&roots, [file], cx));
    advance(49, cx);
    assert!(!sync.read_with(cx, |sync, _| sync.imports.running()));
    assert_eq!(text(&home, "a"), None);
    advance(1, cx);
    settle(cx, "the import", |cx| {
        sync.read_with(cx, |sync, _| !sync.imports.running())
    });
    assert_eq!(text(&home, "a").as_deref(), Some("from the office"));
}

#[gpui::test]
fn two_writers_on_one_computer_converge_through_the_synced_folder(cx: &mut TestAppContext) {
    let folder = folder();
    let desk = store(&folder, "desk");
    let desk_sync = start(&desk, &folder, identity("desk", "mac"), true, cx);
    let tui = store(&folder, "tui");
    let tui_sync = start(&tui, &folder, identity("tui", "mac"), true, cx);
    run_until(cx, "the first writer to see its neighbour", |cx| {
        desk_sync.update(cx, |sync, cx| sync.import(cx));
        desk_sync.read_with(cx, |sync, _| sync.nearby)
            && tui_sync.read_with(cx, |sync, _| sync.nearby)
    });

    save(
        &desk,
        &desk_sync,
        "daily",
        "- from the desk\n- shared\n",
        cx,
    );
    run_until(cx, "the desk's page to reach the other writer", |_| {
        text(&tui, "daily").is_some()
    });
    save(
        &desk,
        &desk_sync,
        "daily",
        "- from the desk, edited\n- shared\n",
        cx,
    );
    save(
        &tui,
        &tui_sync,
        "daily",
        "- from the desk\n- shared\n- from the terminal\n",
        cx,
    );
    run_until(cx, "both writers to agree", |cx| {
        let (a, b) = (text(&desk, "daily"), text(&tui, "daily"));
        let idle = |sync: &Entity<LiveSync>, cx: &mut TestAppContext| {
            sync.read_with(cx, |sync, _| !sync.waiting() && !sync.publishing)
        };
        a.is_some() && a == b && idle(&desk_sync, cx) && idle(&tui_sync, cx)
    });
    assert_eq!(
        text(&desk, "daily").as_deref(),
        Some("- from the desk, edited\n- shared\n- from the terminal\n")
    );
    assert_eq!(
        block(desk.heads("daily".into())),
        block(tui.heads("daily".into()))
    );
    let line = desk_sync.read_with(cx, |sync, _| sync.line(&|at| format!("t{at}")));
    assert!(line.starts_with("Published · mac: seen t"), "{line}");
}
