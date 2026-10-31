//! Ported from the reference `tests/notebook-tools.test.ts`. The reference fixture notebook
//! (`tests/fixtures/ai/notebook.ts`, today 2026-10-28) is saved into a real store and indexed;
//! `quire_core/tests/fixtures/ai/tool-reads.json` holds the reference's own results for the same
//! calls (made by `tool-reads.ts` with TZ=UTC), and each native result must equal it exactly,
//! task keys aside: keys are opaque, and the native ones name blocks the native way.

use super::{bridge::Call, host, live};
use crate::{
    notebook_session::{NotebookSession, OpenPage},
    query_index::{IndexSync, default_config},
    task_edit::CaptureTo,
};
use gpui::{AppContext as _, Entity, Task, TestAppContext};
use quire_core::{ai::tools::Writes, daily::DailyCalendar};
use quire_store::notebook::{Edit, NotebookStore};
use serde_json::{Value, json};
use std::{
    cell::RefCell,
    rc::Rc,
    time::{Duration, Instant},
};

pub(crate) const TODAY: &str = "2026-10-28";

pub(crate) const FIXTURE: [(&str, &str, &str); 8] = [
    (
        "Weekly sync.md",
        "- Recurring team meeting #meeting ^ws\n",
        "2026-09-01",
    ),
    (
        "2026-10-14.md",
        "- [[Weekly sync]] budget is on track ^d14\n",
        "2026-10-14",
    ),
    (
        "2026-10-21.md",
        "- [[Weekly sync]] decided to ship v2 on Friday ^d21\n  - Ana follows up with design\n- [ ] Draft release notes [[2026-10-28]] #next\n",
        "2026-10-21",
    ),
    (
        "2026-10-28.md",
        "- [ ] Call the bank #next ^t1\n- [x] Pay rent ✅ 2026-10-27\n",
        "2026-10-28",
    ),
    (
        "Reading list.md",
        "- Gödel, Escher, Bach #reading ^r1\n- [ ] Finish Dune #reading [[2026-10-30]]\n",
        "2026-10-20",
    ),
    (
        "Old notes.md",
        "- The Mythical Man-Month #reading\n",
        "2026-06-01",
    ),
    (
        "Projects/Website.md",
        "---\ntitle: Projects/Website\n---\n\n- [ ] Fix the header #waiting\n- [ ] Try a new font #someday\n- [ ] Renew the domain [[2026-10-20]]\n",
        "2026-10-25",
    ),
    ("Inbox.md", "- [ ] Sort the receipts\n", "2026-10-26"),
];

fn noon(date: &str) -> u64 {
    format!("{date}T12:00:00Z")
        .parse::<jiff::Timestamp>()
        .unwrap()
        .as_millisecond() as u64
}

/// A real store and index holding the fixture, offered to the tools as the open notebook.
pub(crate) struct Bench {
    _root: tempfile::TempDir,
    pub database: std::path::PathBuf,
    pub identity: String,
    pub sync: Entity<IndexSync>,
    pub session: Rc<RefCell<NotebookSession>>,
}

impl Bench {
    /// Another fixture notebook in the same app, which becomes the one the tools answer for.
    pub fn new(extra: &[(&str, &str)], cx: &mut TestAppContext) -> Self {
        cx.executor().allow_parking();
        if !cx.update(|cx| cx.has_global::<super::Served>()) {
            cx.update(|cx| crate::editor_host::init(cx).unwrap());
        }
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        let pages = FIXTURE
            .iter()
            .map(|(path, text, day)| (*path, *text, *day))
            .chain(
                extra
                    .iter()
                    .map(|(path, text)| (*path, *text, "2026-10-01")),
            );
        for (path, text, day) in pages {
            store
                .save(&Edit {
                    page: path.into(),
                    path: path.into(),
                    text: text.into(),
                    base: vec![],
                    deleted: false,
                    created: noon(day),
                    recovery: None,
                })
                .unwrap();
        }
        drop(store);
        let sync = cx
            .update(|cx| {
                IndexSync::spawn(database.clone(), default_config(DailyCalendar::utc()), cx)
            })
            .unwrap();
        let session = NotebookSession::open(database.clone(), "nb".into(), "local".into()).unwrap();
        let bench = Self {
            _root: root,
            identity: database.display().to_string(),
            database,
            sync,
            session: Rc::new(RefCell::new(session)),
        };
        bench.settle(cx);
        bench.register(cx);
        bench
    }

    pub fn settle(&self, cx: &mut TestAppContext) {
        let client = cx.read(|cx| self.sync.read(cx).client().clone());
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            cx.executor().advance_clock(Duration::from_secs(1));
            cx.run_until_parked();
            let left = finish(
                cx.background_spawn({
                    let client = client.clone();
                    async move { client.unanalyzed().unwrap().await.unwrap() }
                }),
                cx,
            );
            if left == 0 {
                return;
            }
            assert!(Instant::now() < deadline, "indexing did not finish");
        }
    }

    pub fn register(&self, cx: &mut TestAppContext) {
        let session = self.session.clone();
        let sync = self.sync.clone();
        let identity = self.identity.clone();
        cx.update(|cx| {
            live::register(
                Rc::new(move |cx| {
                    let session = session.borrow();
                    Some(live::LiveNotebook {
                        identity: identity.clone(),
                        open_pages: session.open_pages(),
                        storage: session.storage(),
                        index: sync.read(cx).client().clone(),
                        today: Ok(TODAY.into()),
                        calendar: Some(DailyCalendar::utc()),
                        daily: Ok(CaptureTo::Today {
                            date: TODAY.into(),
                            title: TODAY.into(),
                            folder: String::new(),
                        }),
                        capture: Ok(CaptureTo::Inbox),
                    })
                }),
                cx,
            )
        });
    }

    /// Opens a page as an editor would, feeding its live text to the index.
    pub fn open(&self, page: &str, cx: &mut TestAppContext) -> Entity<OpenPage> {
        let opened = cx.update(|cx| {
            let mut session = self.session.borrow_mut();
            let page = session.open_page(page.into(), cx);
            session.enable_editing(cx);
            page
        });
        crate::notebook_session::tests::wait(&opened, cx);
        self.sync
            .update(cx, |sync, cx| sync.track(opened.clone(), cx));
        cx.executor().advance_clock(Duration::from_secs(1));
        cx.run_until_parked();
        opened
    }

    /// Calls a tool as the bridge would, waiting for its answer.
    pub fn call(&self, name: &str, input: Value, cx: &mut TestAppContext) -> Result<Value, String> {
        self.call_with(name, input, Writes::Review, None, cx)
    }

    pub fn call_with(
        &self,
        name: &str,
        input: Value,
        writes: Writes,
        run: Option<&str>,
        cx: &mut TestAppContext,
    ) -> Result<Value, String> {
        let call = Call {
            name: name.into(),
            input,
            run: run.map(str::to_owned),
            writes,
        };
        let task = cx.update(|cx| host::answer(call, cx));
        finish(task, cx)
    }
}

/// Drives the app until `task` finishes; the index answers from its own thread.
pub(crate) fn finish<T: 'static>(task: Task<T>, cx: &mut TestAppContext) -> T {
    let result = Rc::new(RefCell::new(None));
    cx.spawn({
        let result = result.clone();
        async move |_| *result.borrow_mut() = Some(task.await)
    })
    .detach();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        cx.run_until_parked();
        if let Some(value) = result.borrow_mut().take() {
            return value;
        }
        assert!(Instant::now() < deadline, "the tool call did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Replaces every task `key` with a placeholder: keys are opaque handles.
fn without_keys(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| {
                    if key == "key" && value.as_str().is_some_and(|key| !key.starts_with('L')) {
                        (key, json!("task-key"))
                    } else {
                        (key, without_keys(value))
                    }
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(without_keys).collect()),
        other => other,
    }
}

#[gpui::test]
fn every_read_call_answers_exactly_as_the_reference_does(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    let expected: Value = serde_json::from_str(include_str!(
        "../../../quire_core/tests/fixtures/ai/tool-reads.json"
    ))
    .unwrap();
    for case in expected.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let input = case["input"].clone();
        let outcome = match bench.call(name, input.clone(), cx) {
            Ok(result) => json!({ "ok": true, "result": without_keys(result) }),
            Err(error) => json!({ "ok": false, "error": error }),
        };
        assert_eq!(
            outcome.to_string(),
            without_keys(case["outcome"].clone()).to_string(),
            "{name} {input}"
        );
    }
}

#[gpui::test]
fn the_parity_fixture_covers_every_read_tool(cx: &mut TestAppContext) {
    let expected: Value = serde_json::from_str(include_str!(
        "../../../quire_core/tests/fixtures/ai/tool-reads.json"
    ))
    .unwrap();
    let mut names: Vec<&str> = expected
        .as_array()
        .unwrap()
        .iter()
        .map(|case| case["name"].as_str().unwrap())
        .collect();
    names.sort();
    names.dedup();
    let mut reads = quire_core::ai::tools::READ_TOOLS.to_vec();
    reads.sort();
    assert_eq!(names, reads);
    let bench = Bench::new(&[], cx);
    let due = bench.call("tasks", json!({ "due": "today" }), cx).unwrap();
    assert_eq!(due["summary"], "Tasks due today · 2");
    assert_ne!(
        due["tasks"][0]["key"], "task-key",
        "native task keys are real handles"
    );
}

#[gpui::test]
fn reads_unsaved_text_of_open_pages(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    let page = bench.open("Inbox.md", cx);
    let buffer = cx.read(|cx| {
        page.read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    });
    buffer.update(cx, |buffer, cx| {
        let end = buffer.len();
        buffer.edit(
            [(end..end, "- Call Ana about the quarterly plan\n")],
            None,
            cx,
        )
    });
    cx.executor().advance_clock(Duration::from_millis(200));
    cx.run_until_parked();
    let read = bench
        .call("read_page", json!({ "page": "Inbox" }), cx)
        .unwrap();
    assert_eq!(
        read["text"],
        "- [ ] Sort the receipts\n- Call Ana about the quarterly plan\n"
    );
    let found = bench
        .call("search_notebook", json!({ "query": "quarterly plan" }), cx)
        .unwrap();
    assert_eq!(found["summary"], "Searched quarterly plan · 1 block");
    assert_eq!(
        found["results"][0]["snippet"],
        "Call Ana about the quarterly plan"
    );
}

#[gpui::test]
fn windows_long_pages_and_says_where_to_continue(cx: &mut TestAppContext) {
    let long: String = (0..3000)
        .map(|line| format!("- line {line} {}\n", "x".repeat(30)))
        .collect();
    let bench = Bench::new(&[("Long.md", long.trim_end())], cx);
    let first = bench
        .call("read_page", json!({ "page": "Long" }), cx)
        .unwrap();
    let text = first["text"].as_str().unwrap();
    assert!(text.encode_utf16().count() <= 60_000);
    let truncated = first["truncated"].as_str().unwrap();
    let next: usize = truncated
        .rsplit("from_line ")
        .next()
        .unwrap()
        .trim_end_matches('.')
        .parse()
        .unwrap();
    assert_eq!(
        truncated,
        format!(
            "Cut at line {} of 3000. Call read_page again with from_line {next}.",
            next - 1
        )
    );
    assert_eq!(
        first["lines"],
        json!({ "from": 1, "to": next - 1, "total": 3000 })
    );
    let rest = bench
        .call(
            "read_page",
            json!({ "page": "Long", "from_line": next }),
            cx,
        )
        .unwrap();
    assert!(
        rest["text"]
            .as_str()
            .unwrap()
            .starts_with(&format!("- line {} ", next - 1))
    );
    assert_eq!(rest["blocks"][0]["line"], next);
}

#[gpui::test]
fn caps_lists_at_the_limit_and_says_so(cx: &mut TestAppContext) {
    let many: String = (0..250)
        .map(|task| format!("- [ ] task {task}\n"))
        .collect();
    let bench = Bench::new(&[("Many.md", &many)], cx);
    let listed = bench
        .call("tasks", json!({ "page": "Many", "limit": 200 }), cx)
        .unwrap();
    assert_eq!(listed["tasks"].as_array().unwrap().len(), 200);
    assert_eq!(
        listed["truncated"],
        "Showing 200 of 250. Narrow the request or raise limit (at most 200)."
    );
    let default = bench.call("tasks", json!({ "page": "Many" }), cx).unwrap();
    assert_eq!(default["tasks"].as_array().unwrap().len(), 50);
}

#[gpui::test]
fn matches_tagged_blocks_to_tasks_per_page_quickly(cx: &mut TestAppContext) {
    let page: String = (0..30)
        .map(|task| format!("- [ ] task {task} #bulk\n"))
        .collect();
    let paths: Vec<String> = (0..100).map(|page| format!("Bulk {page}.md")).collect();
    let extra: Vec<(&str, &str)> = paths
        .iter()
        .map(|path| (path.as_str(), page.as_str()))
        .collect();
    let bench = Bench::new(&extra, cx);
    let started = Instant::now();
    let tagged = bench
        .call(
            "tagged_blocks",
            json!({ "tag": "bulk", "status": "open", "limit": 5 }),
            cx,
        )
        .unwrap();
    assert_eq!(tagged["summary"], "Tagged #bulk · 3000 blocks");
    assert_eq!(tagged["blocks"][0]["task"], "inbox");
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "took {:?}",
        started.elapsed()
    );
}

#[gpui::test]
fn rejects_paths_outside_the_notebook_pages(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    for page in ["/etc/passwd", "../secret.md", ".quire/ai/x.md", "C:\\x.md"] {
        let error = bench
            .call("read_page", json!({ "page": page }), cx)
            .unwrap_err();
        assert!(error.contains("outside the notebook's pages"), "{error}");
    }
    assert_eq!(
        bench
            .call("read_page", json!({ "page": "today " }), cx)
            .unwrap()["title"],
        TODAY
    );
    assert_eq!(
        bench.call("nope", json!({}), cx).unwrap_err(),
        "Unknown tool nope. Available tools: search_notebook, list_pages, read_page, read_block, linked_references, tagged_blocks, tasks, daily_pages, mentions, append_block, create_page, replace_block, set_task_state, capture_task."
    );
}

#[gpui::test]
fn names_the_fix_when_no_notebook_is_open(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let call = Call {
        name: "tasks".into(),
        input: json!({}),
        run: None,
        writes: Writes::Review,
    };
    let answer = finish(cx.update(|cx| host::answer(call, cx)), cx);
    assert_eq!(answer.unwrap_err(), quire_core::ai::tools::NO_NOTEBOOK);
}
