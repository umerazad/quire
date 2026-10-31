//! `quire --measure-sync-latency [rounds]`, built with `test-support`: two live sync sessions on
//! one computer, each with its own writer and database, share a scratch notebook folder and take
//! turns saving a page. Each round reports the time from the save's acknowledgement to the other
//! session's import of it, with the real clock, worker threads, and file watcher. Nothing outside
//! a temporary folder is read or written.

use super::*;
use gpui::Entity;
use quire_store::{
    notebook::Edit,
    worker::{QueueLimits, StorageWorker},
};
use std::{cell::RefCell, time::Instant};

const PAUSE: Duration = Duration::from_millis(1_500);

struct Side {
    name: &'static str,
    client: StorageClient,
    sync: Entity<LiveSync>,
}

pub fn run(rounds: usize) {
    let scratch = std::env::temp_dir().join(format!("quire-sync-latency-{}", std::process::id()));
    std::fs::create_dir(&scratch).expect(
        "Cannot create a scratch folder in the temporary folder. Free disk space and retry.",
    );
    let base = std::fs::canonicalize(&scratch).expect("Cannot resolve the scratch folder.");
    let root = base.join("notebook");
    std::fs::create_dir(&root).expect("Cannot create the scratch notebook.");
    std::fs::write(
        root.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )
    .expect("Cannot write the scratch notebook's identity.");
    let label = live_sync::host_label();
    gpui_platform::headless().run(move |cx| {
        let open = |name: &'static str, cx: &mut App| {
            let directory = base.join(name);
            std::fs::create_dir(&directory).expect("Cannot create a scratch database folder.");
            let worker = StorageWorker::spawn(
                directory.join("local.sqlite"),
                "nb".into(),
                name.into(),
                QueueLimits::default(),
            )
            .expect("Cannot start a scratch store.");
            let client = worker.client;
            let identity = Identity {
                device: name.into(),
                label: label.clone(),
                app: concat!("quire-native ", env!("CARGO_PKG_VERSION")).into(),
            };
            let root = root.clone();
            let sync = cx
                .new(|cx| LiveSync::new(client.clone(), root, identity, system_clock(), true, cx));
            Side { name, client, sync }
        };
        let sides = Rc::new([open("desk-a", cx), open("desk-b", cx)]);
        let results: Rc<RefCell<Vec<(&'static str, f64)>>> = Rc::default();
        cx.spawn(async move |cx| {
            cx.background_executor().timer(PAUSE * 2).await;
            for round in 0..rounds * 2 {
                let (from, to) = (&sides[round % 2], &sides[1 - round % 2]);
                let text = format!("- round {round} from {}\n", from.name);
                let (seen, mut arrived) = futures::channel::mpsc::unbounded::<()>();
                let page = "p-latency".to_owned();
                let _subscription = cx.update(|cx| {
                    let page = page.clone();
                    cx.subscribe(&to.sync, move |_, Imported(pages), _| {
                        if pages.contains(&page) {
                            seen.unbounded_send(()).ok();
                        }
                    })
                });
                let client = from.client.clone();
                let base = ask(client.heads(page.clone())).await.unwrap_or_default();
                let saved = ask(client.save(Edit {
                    page: page.clone(),
                    base,
                    path: "Latency.md".into(),
                    text: text.clone(),
                    deleted: false,
                    created: 1,
                    recovery: None,
                }))
                .await;
                let started = Instant::now();
                if let Err(error) = saved {
                    eprintln!("Round {round} could not save: {error}");
                    continue;
                }
                loop {
                    arrived.next().await;
                    let heads = ask(to.client.heads(page.clone())).await.unwrap_or_default();
                    let current = match heads.as_slice() {
                        [head] => ask(to.client.get(head.clone())).await.ok().flatten(),
                        _ => None,
                    };
                    let current = current
                        .and_then(|record| record.root().field("text").as_str().map(str::to_owned));
                    if current.as_deref() == Some(text.as_str()) {
                        break;
                    }
                }
                let elapsed = started.elapsed().as_secs_f64();
                println!("{} to {}: {:.3} s", from.name, to.name, elapsed);
                results.borrow_mut().push((from.name, elapsed));
                cx.background_executor().timer(PAUSE).await;
            }
            let nearby = cx.update(|cx| sides.iter().all(|side| side.sync.read(cx).nearby));
            for name in ["desk-a", "desk-b"] {
                let mut times: Vec<f64> = results
                    .borrow()
                    .iter()
                    .filter(|(from, _)| *from == name)
                    .map(|(_, elapsed)| *elapsed)
                    .collect();
                times.sort_by(f64::total_cmp);
                if let (Some(first), Some(last)) = (times.first(), times.last()) {
                    println!(
                        "From {name}: {} rounds, min {first:.3} s, median {:.3} s, max {last:.3} s",
                        times.len(),
                        times[times.len() / 2]
                    );
                }
            }
            println!("Both sessions saw each other as nearby: {nearby}");
            std::fs::remove_dir_all(&scratch).ok();
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
