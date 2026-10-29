use futures::executor::block_on;
use quire_store::{
    notebook::Edit,
    worker::{QueueError, QueueLimits, StorageWorker},
};
use sqlez::connection::Connection;
use std::time::{Duration, Instant};

fn edit(page: &str) -> Edit {
    Edit {
        page: page.into(),
        base: vec![],
        path: format!("{page}.md"),
        text: "one".into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

#[test]
fn bounded_worker_rejects_overflow_without_waiting_for_sqlite_and_drains_on_close() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "office".into(),
        QueueLimits {
            requests: 1,
            bytes: 1024 * 1024,
        },
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let lock = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    let first = worker.client.save(edit("p-one")).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !worker.client.stats().active {
        assert!(
            Instant::now() < deadline,
            "worker did not start the queued save"
        );
        std::thread::yield_now();
    }
    let second = worker.client.save(edit("p-two")).unwrap();
    assert!(matches!(
        worker.client.save(edit("p-three")),
        Err(QueueError::Busy)
    ));
    worker.client.close().unwrap();
    assert!(matches!(
        worker.client.save(edit("p-four")),
        Err(QueueError::Stopped)
    ));
    lock.exec("ROLLBACK").unwrap()().unwrap();
    block_on(first).unwrap();
    block_on(second).unwrap();
    block_on(worker.stopped).unwrap();
    assert_eq!(worker.client.stats().pending_requests, 0);
    assert_eq!(worker.client.stats().pending_bytes, 0);
    let store = quire_store::notebook::NotebookStore::open(&path, "nb", "office").unwrap();
    assert_eq!(store.outbox().unwrap().len(), 2);
}

#[test]
fn startup_errors_and_closed_workers_return_actionable_errors() {
    let root = tempfile::tempdir().unwrap();
    let worker = StorageWorker::spawn(
        root.path().join("missing/local.sqlite"),
        "nb".into(),
        "office".into(),
        QueueLimits::default(),
    )
    .unwrap();
    let error = block_on(worker.ready).unwrap_err();
    assert!(format!("{error:#}").contains("Cannot open"));
    block_on(worker.stopped).unwrap();
    assert!(matches!(
        worker.client.save(edit("p-one")),
        Err(QueueError::Stopped)
    ));
}

#[test]
fn an_oversized_atomic_save_runs_alone_and_releases_its_byte_budget() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let worker = StorageWorker::spawn(
        path.clone(),
        "nb".into(),
        "office".into(),
        QueueLimits {
            requests: 8,
            bytes: 1024,
        },
    )
    .unwrap();
    block_on(worker.ready).unwrap();
    let lock = Connection::try_open_file(path.to_str().unwrap()).unwrap();
    lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
    let mut large = edit("p-large");
    large.text = "large atomic save".repeat(2048);
    let saved = worker.client.save(large).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    while !worker.client.stats().active {
        assert!(
            Instant::now() < deadline,
            "worker did not start the large save"
        );
        std::thread::yield_now();
    }
    let stats = worker.client.stats();
    assert!(stats.pending_bytes > 1024);
    assert_eq!(stats.pending_requests, 1);
    assert!(matches!(
        worker.client.save(edit("p-small")),
        Err(QueueError::Busy)
    ));
    assert_eq!(worker.client.stats().pending_bytes, stats.pending_bytes);
    lock.exec("ROLLBACK").unwrap()().unwrap();
    block_on(saved).unwrap();
    // Wait for admission accounting as well as the save reply before submitting the next request.
    while worker.client.stats().pending_requests != 0 {
        assert!(
            Instant::now() < deadline,
            "worker did not release the large save's budget"
        );
        std::thread::yield_now();
    }
    block_on(worker.client.save(edit("p-small")).unwrap()).unwrap();
    worker.client.close().unwrap();
    block_on(worker.stopped).unwrap();
    assert_eq!(worker.client.stats().pending_bytes, 0);
}
