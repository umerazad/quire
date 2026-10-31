//! The release-build performance run behind `tools/bench/run.ts` (NR34). With `QUIRE_BENCH`
//! naming an output file, the first notebook window times launch to its first editable page,
//! waits for the index, types into the page, opens other pages, queries the index, types into a
//! large page, writes the samples as JSON, and quits. Frame times come from a marker painted last
//! in the window, so they end when the frame's paint ends, before the GPU presents it.

use super::StoredWindow;
use futures::channel::oneshot;
use gpui::{
    App, AsyncWindowContext, Focusable as _, Global, Keystroke, ParentElement as _, Styled as _,
    WeakEntity, Window,
};
use quire_store::index::TaskQuery;
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const OUTPUT: &str = "QUIRE_BENCH";
/// Nanoseconds since the Unix epoch when the harness spawned the process, so launch time
/// includes loading the executable.
const SPAWNED: &str = "QUIRE_BENCH_SPAWNED_NS";
/// The page the harness wrote for large-page typing.
const LARGE_PAGE: &str = "Large";
const KEYSTROKES: usize = 120;
/// About 12 characters a second, a fast typist's pace.
const TYPING_PAUSE: Duration = Duration::from_millis(80);
const PAGE_OPENS: usize = 20;
/// How long a measure waits for a painted frame before settling for state; see [`paint`].
const UNPAINTED: Duration = Duration::from_millis(250);
/// Index queries that found the queue full and were retried; see [`timed`].
static BUSY: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
const QUERIES: [&str; 10] = [
    "budget",
    "design storage",
    "meeting",
    "review draft",
    "customer support",
    "quarter release",
    "schedule",
    "vendor plan",
    "ملاحظات",
    "research summary",
];

type Probe = Rc<dyn Fn(&Window, &App) -> bool>;
type Waiter = (Probe, oneshot::Sender<Instant>);

/// Frames waiting for a condition, checked when the marker paints.
#[derive(Default)]
struct Painted {
    waiting: Rc<RefCell<Vec<Waiter>>>,
    /// Measures that settled for state because no frame painted, as when the window is covered.
    unpainted: Cell<usize>,
}

impl Global for Painted {}

/// A zero-size element painted last, which resolves the frames waiting on this paint.
pub(super) fn marker(cx: &App) -> Option<impl gpui::IntoElement> {
    let waiting = cx.try_global::<Painted>()?.waiting.clone();
    Some(gpui::div().absolute().size_0().child(gpui::canvas(
        |_, _, _| (),
        move |_, _, window, cx| {
            let now = Instant::now();
            let ready: Vec<_> = {
                let mut waiting = waiting.borrow_mut();
                let (ready, pending) = std::mem::take(&mut *waiting)
                    .into_iter()
                    .filter(|(_, sender)| !sender.is_canceled())
                    .partition(|(probe, _)| probe(window, cx));
                *waiting = pending;
                ready
            };
            ready.into_iter().for_each(|(_, sender)| {
                sender.send(now).ok();
            });
        },
    )))
}

/// Starts the run in the first window when `QUIRE_BENCH` is set.
pub(super) fn start(window: &mut Window, cx: &mut gpui::Context<StoredWindow>) {
    let Some(output) = std::env::var_os(OUTPUT).map(PathBuf::from) else {
        return;
    };
    if cx.has_global::<Painted>() {
        return;
    }
    let launched = launched();
    cx.set_global(Painted::default());
    cx.spawn_in(window, async move |this, cx| {
        let result = run(&this, launched, cx).await.and_then(|report| {
            std::fs::write(&output, report.to_string()).map_err(|error| {
                format!(
                    "Cannot write the performance report to {}: {error}. Pass a writable QUIRE_BENCH path.",
                    output.display()
                )
            })
        });
        if let Err(error) = result {
            eprintln!("Quire performance run failed: {error}");
            std::fs::write(&output, serde_json::json!({ "error": error }).to_string()).ok();
        }
        cx.update(|_, cx| cx.quit()).ok();
    })
    .detach();
}

/// When the harness spawned this process, or now when it did not say.
fn launched() -> Instant {
    let now = Instant::now();
    std::env::var(SPAWNED)
        .ok()
        .and_then(|spawned| spawned.parse::<u128>().ok())
        .and_then(|spawned| {
            let since = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos();
            now.checked_sub(Duration::from_nanos(
                u64::try_from(since.checked_sub(spawned)?).ok()?,
            ))
        })
        .unwrap_or(now)
}

fn ms(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 1_000_000.).round() / 1000.
}

fn closed(error: impl std::fmt::Display) -> String {
    format!("The window closed during the performance run: {error}")
}

/// When the first frame for which `probe` holds finished painting. macOS stops drawing a window
/// that is fully covered, so when no such frame paints within [`UNPAINTED`] this settles for
/// when `probe` first held, polled every millisecond, and counts the measure as unpainted.
async fn paint(probe: Probe, cx: &mut AsyncWindowContext) -> Result<Instant, String> {
    let (sender, mut receiver) = oneshot::channel();
    cx.update(|window, cx| {
        cx.global::<Painted>()
            .waiting
            .borrow_mut()
            .push((probe.clone(), sender));
        window.refresh();
    })
    .map_err(closed)?;
    let started = Instant::now();
    let mut held = None;
    loop {
        if let Ok(Some(painted)) = receiver.try_recv() {
            return Ok(painted);
        }
        if held.is_none() && cx.update(|window, cx| probe(window, cx)).map_err(closed)? {
            held = Some(Instant::now());
        }
        if let Some(held) = held
            && started.elapsed() >= UNPAINTED
        {
            cx.update(|_, cx| {
                let unpainted = &cx.global::<Painted>().unpainted;
                unpainted.set(unpainted.get() + 1);
            })
            .map_err(closed)?;
            return Ok(held);
        }
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    }
}

/// The shown page is loaded, writable, and its editor has focus.
fn editable(this: &WeakEntity<StoredWindow>, page: Option<String>) -> Probe {
    let this = this.clone();
    Rc::new(move |window, cx| {
        this.upgrade().is_some_and(|this| {
            let this = this.read(cx);
            let shown = this.page.read(cx);
            page.as_deref().is_none_or(|page| shown.id() == page)
                && shown.loaded().is_some()
                && shown.persistence().is_some()
                && this.editor.as_ref().is_some_and(|editor| {
                    editor.read(cx).source.focus_handle(cx).is_focused(window)
                })
        })
    })
}

fn resident_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// The index, once the window has read the notebook's settings and started it, which can come
/// after the first editable page.
async fn client(
    this: &WeakEntity<StoredWindow>,
    cx: &mut AsyncWindowContext,
) -> Result<quire_store::index::IndexClient, String> {
    let started = Instant::now();
    loop {
        let client = this
            .read_with(cx, |this, cx| this.index_client(cx))
            .map_err(closed)?;
        if let Some(client) = client {
            return Ok(client);
        }
        if started.elapsed() > Duration::from_secs(60) {
            return Err("The notebook index did not start within 60 s, so queries cannot be timed. Check the index status line and retry.".into());
        }
        cx.background_executor()
            .timer(Duration::from_millis(10))
            .await;
    }
}

/// Times `count` keystrokes typed into the focused page at a typist's pace: the foreground work
/// each one takes, and the time until the frame showing it has painted.
async fn type_into(
    count: usize,
    cx: &mut AsyncWindowContext,
) -> Result<(Vec<f64>, Vec<f64>), String> {
    let mut work = Vec::with_capacity(count);
    let mut frames = Vec::with_capacity(count);
    // Vim starts in Normal mode: `A` appends at the end of the line, `escape` leaves Insert.
    press("A", cx).await?;
    for key in ["q", "u", "i", "r", "e"].into_iter().cycle().take(count) {
        let (handled, shown) = press(key, cx).await?;
        work.push(handled);
        frames.push(shown);
    }
    press("escape", cx).await?;
    Ok((work, frames))
}

/// Presses `key` through the window's key dispatch and input handler, as the platform does,
/// returning the foreground time it took and the time until its frame painted.
async fn press(key: &'static str, cx: &mut AsyncWindowContext) -> Result<(f64, f64), String> {
    let keystroke = Keystroke::parse(key)
        .map_err(|error| format!("Cannot parse the keystroke {key:?}: {error}"))?;
    cx.background_executor().timer(TYPING_PAUSE).await;
    let started = Instant::now();
    cx.update(|window, cx| window.dispatch_keystroke(keystroke, cx))
        .map_err(|error| format!("The window closed while typing: {error}"))?;
    let handled = started.elapsed();
    let shown = paint(Rc::new(|_, _| true), cx).await?;
    Ok((ms(handled), ms(shown.saturating_duration_since(started))))
}

async fn open(
    this: &WeakEntity<StoredWindow>,
    page: String,
    cx: &mut AsyncWindowContext,
) -> Result<f64, String> {
    let started = Instant::now();
    this.update_in(cx, |this, window, cx| {
        this.open_page(page.clone(), false, window, cx)
    })
    .map_err(|error| format!("The window closed while opening a page: {error}"))?;
    let shown = paint(editable(this, Some(page)), cx).await?;
    Ok(ms(shown.saturating_duration_since(started)))
}

/// Times one index query from the foreground, as a view waits for it. A full queue is retried
/// every millisecond, as the views retry, and the wait counts toward the time and in `BUSY`.
async fn timed<T>(
    what: &str,
    request: impl Fn() -> Result<quire_store::worker::Reply<T>, quire_store::worker::QueueError>,
    cx: &mut AsyncWindowContext,
) -> Result<f64, String> {
    let started = Instant::now();
    let reply = loop {
        match request() {
            Err(quire_store::worker::QueueError::Busy) => {
                BUSY.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                cx.background_executor()
                    .timer(Duration::from_millis(1))
                    .await;
            }
            reply => break reply,
        }
    };
    reply
        .map_err(|error| format!("The index refused the {what} query: {error}. Retry the run."))?
        .await
        .map_err(|error| format!("The {what} query failed: {error:#}. Retry the run."))?;
    Ok(ms(started.elapsed()))
}

async fn run(
    this: &WeakEntity<StoredWindow>,
    launched: Instant,
    cx: &mut AsyncWindowContext,
) -> Result<serde_json::Value, String> {
    stage(launched, "waiting for the first editable page");
    let first_editable = paint(editable(this, None), cx).await?;
    let resident_at_editable = resident_kib();
    let client = client(this, cx).await?;
    stage(launched, "waiting for the index");
    let index_ready = loop {
        let unanalyzed = client
            .unanalyzed()
            .map_err(|error| format!("The index refused a status query: {error}. Retry the run."))?
            .await
            .map_err(|error| {
                format!("The index failed: {error:#}. Check the index status line and retry.")
            })?;
        if unanalyzed == 0 {
            break Instant::now();
        }
        cx.background_executor()
            .timer(Duration::from_millis(50))
            .await;
    };
    stage(launched, "typing");
    let (typing_work, typing_frames) = type_into(KEYSTROKES, cx).await?;

    stage(launched, "opening pages");
    let listed = client
        .pages()
        .map_err(|error| format!("The index refused the page list: {error}. Retry the run."))?
        .await
        .map_err(|error| format!("Cannot list pages: {error:#}. Retry the run."))?;
    let current = this
        .read_with(cx, |this, cx| this.page.read(cx).id().to_owned())
        .map_err(|error| format!("The window closed: {error}"))?;
    let candidates: Vec<_> = listed
        .iter()
        .map(|listed| listed.page.page.to_lossy_string())
        .filter(|page| *page != current)
        .collect();
    let step = (candidates.len() / PAGE_OPENS).max(1);
    let picked: Vec<String> = candidates
        .iter()
        .step_by(step)
        .take(PAGE_OPENS)
        .cloned()
        .collect();
    let mut page_opens = Vec::new();
    for page in picked.iter().cloned() {
        page_opens.push(open(this, page, cx).await?);
    }

    let resident_after_opens = resident_kib();
    stage(launched, "querying the index");
    let mut search = Vec::new();
    for query in QUERIES {
        search.push(timed("search", || client.search(query.to_owned()), cx).await?);
    }
    let mut backlinks = Vec::new();
    for page in &listed[..listed.len().min(PAGE_OPENS)] {
        let page = page.page.page.clone();
        backlinks.push(timed("backlinks", || client.backlinks(page.clone()), cx).await?);
    }
    let mut tasks = Vec::new();
    for view in [
        quire_core::workspace::TaskView::Due,
        quire_core::workspace::TaskView::Inbox,
        quire_core::workspace::TaskView::Next,
        quire_core::workspace::TaskView::Upcoming,
    ] {
        let query = TaskQuery {
            view,
            today: "2024-06-01".into(),
            filter: String::new(),
            project: None,
            limit: 200,
        };
        tasks.push(timed("tasks", || client.task_board(query.clone()), cx).await?);
    }
    let tags = vec![timed("tags", || client.tag_names(), cx).await?];

    stage(launched, "typing in the large page");
    let large = client
        .find_page(LARGE_PAGE.into())
        .map_err(|error| {
            format!("The index refused the large-page lookup: {error}. Retry the run.")
        })?
        .await
        .map_err(|error| format!("Cannot find the large page: {error:#}. Retry the run."))?;
    let large = match large {
        Some(large) => {
            let opened = open(this, large.page.to_lossy_string(), cx).await?;
            stage(launched, "typing after opening the large page");
            let (work, frames) = type_into(KEYSTROKES, cx).await?;
            serde_json::json!({ "open_ms": opened, "typing_work_ms": work, "typing_frame_ms": frames })
        }
        None => serde_json::Value::Null,
    };
    let unpainted = cx
        .update(|_, cx| cx.global::<Painted>().unpainted.get())
        .map_err(closed)?;
    let counts = this
        .read_with(cx, |this, cx| {
            let pages = this.session.borrow().open_pages();
            let bodies: usize = pages
                .iter()
                .filter_map(|page| {
                    page.read(cx)
                        .loaded()
                        .map(|page| page.loaded_revision_bodies)
                })
                .sum();
            serde_json::json!({
                "open_pages": pages.len(),
                "loaded_revision_bodies": bodies,
                "editors": this.nav.editors.len(),
            })
        })
        .map_err(|error| format!("The window closed: {error}"))?;
    Ok(serde_json::json!({
        "first_editable_ms": ms(first_editable.saturating_duration_since(launched)),
        "index_ready_ms": ms(index_ready.saturating_duration_since(launched)),
        "analyzer": crate::page_index::ANALYZER_VERSION,
        "resident_kib_at_editable": resident_at_editable,
        "resident_kib_after_opens": resident_after_opens,
        "resident_kib_at_end": resident_kib(),
        "typing_work_ms": typing_work,
        "typing_frame_ms": typing_frames,
        "page_open_ms": page_opens,
        "search_ms": search,
        "backlinks_ms": backlinks,
        "tasks_ms": tasks,
        "tags_ms": tags,
        "large_page": large,
        "counts": counts,
        "unpainted": unpainted,
        "busy_retries": BUSY.load(std::sync::atomic::Ordering::Relaxed),
        "pages": listed.len(),
    }))
}

fn stage(launched: Instant, what: &str) {
    eprintln!(
        "Quire performance run at {} ms: {what}.",
        ms(launched.elapsed())
    );
}
