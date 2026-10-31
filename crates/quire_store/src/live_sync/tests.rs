use super::*;
use std::path::PathBuf;

fn status(device: &str, label: &str, heartbeat: u64) -> DeviceStatus {
    DeviceStatus {
        format: 1,
        app: "0.9.57".into(),
        device: device.into(),
        label: label.into(),
        last_seq: 3,
        outbox: 0,
        heartbeat,
    }
}

fn problem(file: &str, reason: &str) -> RescanProblem {
    RescanProblem {
        device: Some("office".into()),
        file: Some(file.into()),
        reason: reason.into(),
        newer_format: None,
    }
}

#[test]
fn alone_saves_publish_one_second_after_the_last_and_at_most_five_after_the_first() {
    let mut timer = PublishTimer::default();
    assert_eq!(timer.saved(10_000, false), 1_000);
    assert_eq!(
        timer.saved(10_900, false),
        1_000,
        "a save restarts the quiet spell"
    );
    assert_eq!(timer.saved(13_500, false), 1_000);
    assert_eq!(
        timer.saved(14_500, false),
        500,
        "the deadline set by the first save holds"
    );
    assert_eq!(timer.saved(15_000, false), 0);
    assert_eq!(
        timer.saved(16_000, false),
        0,
        "a missed deadline publishes at once"
    );
    timer.started();
    assert_eq!(
        timer.saved(20_000, false),
        1_000,
        "publishing starts a new deadline"
    );
}

#[test]
fn near_another_app_saves_publish_after_150_ms_and_at_most_one_second() {
    let mut timer = PublishTimer::default();
    assert_eq!(timer.saved(0, true), 150);
    assert_eq!(timer.saved(100, true), 150);
    assert_eq!(timer.saved(900, true), 100);
    assert_eq!(timer.saved(1_000, true), 0);
    timer.started();
    assert_eq!(timer.saved(1_200, true), 150);
    assert_eq!(
        timer.saved(1_300, false),
        900,
        "losing the neighbour keeps the deadline set while it was near"
    );
}

#[test]
fn status_is_rewritten_when_the_outbox_changes_or_as_a_minute_heartbeat() {
    let mut memory = StatusMemory::default();
    assert!(memory.due(0, 5), "the first status is always written");
    memory.wrote(2, 1_000);
    assert!(!memory.due(2, 1_001));
    assert!(
        !memory.due(2, 60_999),
        "an unchanged status waits for the heartbeat"
    );
    assert!(memory.due(3, 1_002), "a changed count is written at once");
    assert!(
        memory.due(2, 61_000),
        "the heartbeat refreshes an unchanged status"
    );
    memory.wrote(2, 61_000);
    assert!(!memory.due(2, 61_001));
}

#[test]
fn nearby_needs_another_writer_with_this_label_and_a_heartbeat_under_two_minutes() {
    let now = 1_000_000;
    let mine = status("desk", "mac", now);
    assert!(
        !nearby(std::slice::from_ref(&mine), "desk", "mac", now),
        "this writer is not a neighbour"
    );
    let terminal = DeviceStatus {
        app: "quire-tui 0.9.57".into(),
        ..status("tui", "mac", now - NEARBY_WITHIN + 1)
    };
    assert!(nearby(
        &[mine.clone(), terminal.clone()],
        "desk",
        "mac",
        now
    ));
    let stale = status("tui", "mac", now - NEARBY_WITHIN);
    assert!(!nearby(&[mine.clone(), stale], "desk", "mac", now));
    let elsewhere = status("laptop", "other-mac", now);
    assert!(!nearby(&[mine, elsewhere], "desk", "mac", now));
    let future = status("tui", "mac", now + 5_000);
    assert!(
        nearby(&[future], "desk", "mac", now),
        "a clock slightly ahead still counts"
    );
}

#[test]
fn a_problem_warns_only_when_it_survives_the_retry() {
    let mut memory = ProblemMemory::default();
    let partial = problem(
        "0000000002-abc.qlog",
        "Change file checksum does not match.",
    );
    assert!(memory.survived(std::slice::from_ref(&partial)).is_empty());
    assert_eq!(
        memory.survived(std::slice::from_ref(&partial)),
        vec![
            "Changes from another computer could not be read (0000000002-abc.qlog): Change file checksum does not match. Quire will try again."
                .to_owned()
        ]
    );
    assert!(
        memory.survived(&[]).is_empty(),
        "a delivered file clears the problem"
    );
    assert!(
        memory.survived(std::slice::from_ref(&partial)).is_empty(),
        "a new occurrence starts quiet again"
    );
    let other = problem("0000000002-abc.qlog", "Change file is truncated.");
    assert!(
        memory.survived(&[other]).is_empty(),
        "a different reason is a new problem"
    );
}

#[test]
fn an_import_requested_while_one_runs_runs_again_once() {
    let mut imports = Imports::default();
    assert!(imports.request());
    assert!(!imports.request());
    assert!(
        !imports.request(),
        "requests during a run collapse into one rerun"
    );
    assert!(imports.finished(), "the rerun starts");
    assert!(imports.running());
    assert!(!imports.finished());
    assert!(!imports.running());
    assert!(imports.request());
}

#[test]
fn only_delivered_files_without_dot_parts_wake_an_import() {
    let root = PathBuf::from("/notebook");
    let file = |relative: &str| root.join(relative);
    assert!(delivered(
        &root,
        &file("devices/office/log/0000000001-ab.qlog")
    ));
    assert!(delivered(&root, &file("devices/office/status.json")));
    assert!(delivered(&root, &file("devices/office")));
    assert!(!delivered(&root, &file("devices/office/log/.quire-tmp")));
    assert!(!delivered(&root, &file("devices/office/.status-x1")));
    assert!(!delivered(
        &root,
        &file("devices/.office/log/0000000001-ab.qlog")
    ));
    assert!(!delivered(&root, &file("attachments/abc.png")));
    assert!(!delivered(&root, &file("quire-notebook.json")));
    assert!(!delivered(&root, &file("devices/../quire-notebook.json")));
}

#[test]
fn statuses_round_trip_with_the_reference_field_names_and_skip_damaged_files() -> Result<()> {
    let folder = tempfile::tempdir()?;
    let root = folder.path();
    let mine = DeviceStatus {
        outbox: 2,
        ..status("desk", "mac", 1_700_000_000_000)
    };
    write_status(root, &mine)?;
    let written: serde_json::Value =
        serde_json::from_slice(&fs::read(root.join("devices/desk/status.json"))?)?;
    assert_eq!(
        written,
        serde_json::json!({"format":1,"app":"0.9.57","device":"desk","label":"mac","lastSeq":3,"outbox":2,"heartbeat":1_700_000_000_000u64})
    );
    write_status(
        root,
        &DeviceStatus {
            outbox: 0,
            ..mine.clone()
        },
    )?;
    let place = |device: &str, text: &str| -> Result<()> {
        fs::create_dir_all(root.join("devices").join(device))?;
        fs::write(root.join("devices").join(device).join("status.json"), text)?;
        Ok(())
    };
    place(
        "tui",
        r#"{"format":1,"app":"quire-tui 0.9.57","device":"tui","label":"mac","lastSeq":0,"outbox":1,"heartbeat":5,"extra":true}"#,
    )?;
    place(
        "copied",
        r#"{"format":1,"app":"","device":"tui","label":"mac","lastSeq":0,"outbox":0,"heartbeat":5}"#,
    )?;
    place("partial", r#"{"format":1,"app":"","device":"partial","#)?;
    place(
        "negative",
        r#"{"format":1,"app":"","device":"negative","label":"mac","lastSeq":-1,"outbox":0,"heartbeat":5}"#,
    )?;
    place(
        "fraction",
        r#"{"format":1,"app":"","device":"fraction","label":"mac","lastSeq":0,"outbox":0.5,"heartbeat":5}"#,
    )?;
    place(
        "missing",
        r#"{"format":1,"device":"missing","label":"mac","lastSeq":0,"outbox":0,"heartbeat":5}"#,
    )?;
    place(
        "huge",
        r#"{"format":1,"app":"","device":"huge","label":"mac","lastSeq":9007199254740992,"outbox":0,"heartbeat":5}"#,
    )?;
    place(
        ".hidden",
        r#"{"format":1,"app":"","device":".hidden","label":"mac","lastSeq":0,"outbox":0,"heartbeat":5}"#,
    )?;
    let statuses = read_statuses(root);
    assert_eq!(
        statuses
            .iter()
            .map(|status| (status.device.as_str(), status.outbox))
            .collect::<Vec<_>>(),
        vec![("desk", 0), ("tui", 1)]
    );
    let leftovers: Vec<_> = fs::read_dir(root.join("devices/desk"))?
        .flatten()
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(leftovers, vec![std::ffi::OsString::from("status.json")]);
    Ok(())
}

#[test]
fn an_unavailable_folder_reports_what_failed_and_what_to_do() {
    let error = write_status(
        Path::new("/nonexistent/quire-folder"),
        &status("desk", "mac", 1),
    )
    .unwrap_err();
    let message = format!("{error:#}");
    assert!(
        message.contains("Cannot write this computer's status"),
        "{message}"
    );
    assert!(message.contains("tries again"), "{message}");
    assert!(read_statuses(Path::new("/nonexistent/quire-folder")).is_empty());
}

#[test]
fn a_save_wakes_the_outbox_watcher_and_publication_empties_the_outbox() -> Result<()> {
    use crate::notebook::{Edit, NotebookStore, PublicationState};
    let folder = tempfile::tempdir()?;
    let root = folder.path().join("notebook");
    fs::create_dir(&root)?;
    fs::write(
        root.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )?;
    let store = NotebookStore::open(&folder.path().join("local.sqlite"), "nb", "desk")?;
    let edit = |base: Vec<String>, text: &str| Edit {
        page: "p".into(),
        base,
        path: "A.md".into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    };
    let mut watcher = store.watch_outbox();
    assert!(watcher.try_recv().is_err(), "nothing saved yet");
    store.save(&edit(vec![], "one"))?;
    assert_eq!(watcher.try_recv()?, 1);
    assert_eq!(
        store.publication_state()?,
        PublicationState {
            outbox: 1,
            last_seq: 0,
            saves: 1
        }
    );
    store.publish_to(&root, 2)?.expect("one batch");
    assert_eq!(
        store.publication_state()?,
        PublicationState {
            outbox: 0,
            last_seq: 1,
            saves: 1
        }
    );
    drop(watcher);
    store.save(&edit(store.heads("p")?, "two"))?;
    assert_eq!(
        store.publication_state()?.outbox,
        1,
        "a dropped watcher never blocks a save"
    );
    Ok(())
}

#[test]
fn the_host_label_is_the_host_name() {
    let label = host_label();
    assert!(!label.is_empty());
    assert!(!label.contains('\0'));
}
