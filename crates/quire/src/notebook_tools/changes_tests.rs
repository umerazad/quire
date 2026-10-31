//! Ported from the reference `tests/ai-changes.test.ts`, the write half of
//! `tests/notebook-tools.test.ts`, and the window host cases of `tests/notebook-bridge.test.ts`.
//! `quire_core/tests/fixtures/ai/tool-writes.json` holds the reference's own results (made by
//! `tool-writes.ts` with TZ=UTC); the reference stamps applied changes with the system clock, so
//! its run date is read as the fixture's today.

use super::*;
use crate::notebook_tools::read_tests::{Bench, TODAY, finish};
use gpui::TestAppContext;
use quire_core::ai::tools::{WRITES_OFF, Writes};
use serde_json::{Value, json};

const REFERENCE_RUN_DAY: &str = "2026-10-31";

/// Masks what differs by design: task keys, task source text, and change IDs.
fn comparable(value: Value) -> Value {
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(key, value)| match key.as_str() {
                    "key" if value.as_str().is_some_and(|key| !key.starts_with('L')) => {
                        (key, json!("task-key"))
                    }
                    "change_id" => (key, json!("change-n")),
                    _ => (key, comparable(value)),
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.into_iter().map(comparable).collect()),
        Value::String(text) => Value::String(text.replace(REFERENCE_RUN_DAY, TODAY)),
        other => other,
    }
}

fn outcome(answer: Result<Value, String>) -> Value {
    comparable(match answer {
        Ok(result) => json!({ "ok": true, "result": result }),
        Err(error) => json!({ "ok": false, "error": error }),
    })
}

fn page_text(bench: &Bench, title: &str, cx: &mut TestAppContext) -> String {
    bench.settle(cx);
    bench
        .call("read_page", json!({ "page": title }), cx)
        .unwrap()["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn list(cx: &mut TestAppContext) -> Vec<Change> {
    cx.update(|cx| super::list(None, cx))
}

#[gpui::test]
fn every_write_call_proposes_and_applies_exactly_as_the_reference_does(cx: &mut TestAppContext) {
    let expected: Value = serde_json::from_str(include_str!(
        "../../../quire_core/tests/fixtures/ai/tool-writes.json"
    ))
    .unwrap();
    for case in expected.as_array().unwrap() {
        let name = case["name"].as_str().unwrap();
        let input = case["input"].clone();
        // A task state change names its task by the key the tasks tool lists.
        let arguments = |bench: &Bench, cx: &mut TestAppContext| {
            if name != "set_task_state" {
                return input.clone();
            }
            let task = bench
                .call("tasks", json!({ "text": input["text"] }), cx)
                .unwrap();
            json!({ "key": task["tasks"][0]["key"], "state": input["state"] })
        };
        if let Some(review) = case.get("review") {
            let bench = Bench::new(&[], cx);
            let call_input = arguments(&bench, cx);
            let staged = bench.call_with(name, call_input, Writes::Review, None, cx);
            assert_eq!(
                outcome(staged).to_string(),
                comparable(review.clone()).to_string(),
                "review {name} {input}"
            );
            for (path, _) in case["pages"].as_object().unwrap() {
                let title = path.trim_end_matches(".md");
                if let Ok(before) = bench.call("read_page", json!({ "page": title }), cx) {
                    assert!(
                        crate::notebook_tools::read_tests::FIXTURE
                            .iter()
                            .any(|(fixture, text, _)| fixture == path && before["text"] == *text),
                        "review leaves {path} alone"
                    );
                }
            }
        }
        let bench = Bench::new(&[], cx);
        let call_input = arguments(&bench, cx);
        let applied = bench.call_with(name, call_input, Writes::Apply, None, cx);
        assert_eq!(
            outcome(applied).to_string(),
            comparable(case["apply"].clone()).to_string(),
            "apply {name} {input}"
        );
        for (path, text) in case["pages"].as_object().unwrap() {
            let title = path.trim_end_matches(".md");
            assert_eq!(
                page_text(&bench, title, cx),
                comparable(text.clone()).as_str().unwrap(),
                "{name} {input} leaves {path}"
            );
        }
    }
}

fn buffer_of(
    page: &gpui::Entity<crate::notebook_session::OpenPage>,
    cx: &mut TestAppContext,
) -> gpui::Entity<language::Buffer> {
    cx.read(|cx| {
        page.read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    })
}

#[gpui::test]
fn stages_without_editing_and_applies_an_open_page_append_as_one_undo_step(
    cx: &mut TestAppContext,
) {
    let bench = Bench::new(&[], cx);
    let page = bench.open("Inbox.md", cx);
    let buffer = buffer_of(&page, cx);
    let staged = bench
        .call(
            "append_block",
            json!({ "page": "Inbox", "text": "From the terminal" }),
            cx,
        )
        .unwrap();
    assert_eq!(staged["status"], "awaiting review");
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "- [ ] Sort the receipts\n",
        "a staged change leaves the page alone"
    );
    let changes = list(cx);
    assert_eq!(changes.len(), 1);
    assert_eq!(
        (changes[0].status, changes[0].run.as_str()),
        (Status::Staged, TERMINAL_RUN)
    );
    let id = changes[0].id.clone();
    finish(cx.update(|cx| apply(&id, cx)), cx).unwrap();
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "- [ ] Sort the receipts\n- From the terminal\n"
    );
    assert_eq!(list(cx)[0].status, Status::Applied);
    buffer.update(cx, |buffer, cx| buffer.undo(cx));
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "- [ ] Sort the receipts\n",
        "one undo restores the page"
    );
    assert_eq!(
        finish(cx.update(|cx| apply(&id, cx)), cx).unwrap_err(),
        "This change is already applied."
    );
}

#[gpui::test]
fn rejects_and_undoes_only_while_the_page_is_as_apply_left_it(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    bench
        .call(
            "append_block",
            json!({ "page": "Inbox", "text": "first" }),
            cx,
        )
        .unwrap();
    bench
        .call(
            "append_block",
            json!({ "page": "Inbox", "text": "second" }),
            cx,
        )
        .unwrap();
    let ids: Vec<String> = list(cx).into_iter().map(|change| change.id).collect();
    cx.update(|cx| reject(&ids[0], cx));
    assert_eq!(list(cx)[0].status, Status::Rejected);
    assert_eq!(
        finish(cx.update(|cx| apply(&ids[0], cx)), cx).unwrap_err(),
        "This change is already rejected."
    );
    finish(cx.update(|cx| apply(&ids[1], cx)), cx).unwrap();
    assert_eq!(
        page_text(&bench, "Inbox", cx),
        "- [ ] Sort the receipts\n- second\n"
    );
    finish(cx.update(|cx| undo(&ids[1], cx)), cx).unwrap();
    assert_eq!(list(cx)[1].status, Status::Undone);
    assert_eq!(page_text(&bench, "Inbox", cx), "- [ ] Sort the receipts\n");
    bench
        .call(
            "append_block",
            json!({ "page": "Inbox", "text": "third" }),
            cx,
        )
        .unwrap();
    let third = list(cx)[2].id.clone();
    finish(cx.update(|cx| apply(&third, cx)), cx).unwrap();
    let page = bench.open("Inbox.md", cx);
    let buffer = buffer_of(&page, cx);
    buffer.update(cx, |buffer, cx| {
        let end = buffer.len();
        buffer.edit([(end..end, "- typed\n")], None, cx)
    });
    assert_eq!(
        finish(cx.update(|cx| undo(&third, cx)), cx).unwrap_err(),
        "[[Inbox]] changed after this AI change. Undo it in the page with Command/Ctrl-Z instead."
    );
    assert_eq!(list(cx)[2].status, Status::Applied);
}

#[gpui::test]
fn refuses_a_block_that_changed_since_it_was_proposed(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    bench
        .call(
            "replace_block",
            json!({ "id": "d14", "text": "[[Weekly sync]] budget is fine" }),
            cx,
        )
        .unwrap();
    bench
        .call(
            "replace_block",
            json!({ "id": "t1", "text": "[ ] Call the bank today #next" }),
            cx,
        )
        .unwrap();
    let [closed, open] = [list(cx)[0].id.clone(), list(cx)[1].id.clone()];
    let store = quire_store::notebook::NotebookStore::open(&bench.database, "nb", "local").unwrap();
    store
        .save(&quire_store::notebook::Edit {
            page: "2026-10-14.md".into(),
            path: "2026-10-14.md".into(),
            text: "- [[Weekly sync]] budget is over ^d14\n".into(),
            base: store.heads("2026-10-14.md").unwrap(),
            deleted: false,
            created: 1,
            recovery: None,
        })
        .unwrap();
    drop(store);
    bench.settle(cx);
    assert_eq!(
        finish(cx.update(|cx| apply(&closed, cx)), cx).unwrap_err(),
        STALE
    );
    let page = bench.open("2026-10-28.md", cx);
    let buffer = buffer_of(&page, cx);
    buffer.update(cx, |buffer, cx| buffer.edit([(6..10, "Phone")], None, cx));
    assert_eq!(
        finish(cx.update(|cx| apply(&open, cx)), cx).unwrap_err(),
        STALE
    );
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "- [ ] Phone the bank #next ^t1\n- [x] Pay rent ✅ 2026-10-27\n",
        "a refused change leaves the typing alone"
    );
    assert_eq!(
        page_text(&bench, "2026-10-14", cx),
        "- [[Weekly sync]] budget is over ^d14\n"
    );
    assert!(
        list(cx)
            .iter()
            .all(|change| change.status == Status::Staged)
    );
}

#[gpui::test]
fn applies_a_task_state_in_an_open_page_as_one_undo_step(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    let page = bench.open("2026-10-28.md", cx);
    let buffer = buffer_of(&page, cx);
    let task = bench.call("tasks", json!({ "text": "bank" }), cx).unwrap();
    let applied = bench
        .call_with(
            "set_task_state",
            json!({ "key": task["tasks"][0]["key"], "state": "done" }),
            Writes::Apply,
            None,
            cx,
        )
        .unwrap();
    assert_eq!(applied["status"], "applied");
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        format!("- [x] Call the bank #next ✅ {TODAY} ^t1\n- [x] Pay rent ✅ 2026-10-27\n")
    );
    buffer.update(cx, |buffer, cx| buffer.undo(cx));
    assert_eq!(
        cx.read(|cx| buffer.read(cx).text()),
        "- [ ] Call the bank #next ^t1\n- [x] Pay rent ✅ 2026-10-27\n"
    );
    let stale = bench.call_with(
        "set_task_state",
        json!({ "key": "nope", "state": "done" }),
        Writes::Apply,
        None,
        cx,
    );
    assert_eq!(
        stale.unwrap_err(),
        "No task has that key, or it changed. Call tasks again for current keys."
    );
}

#[gpui::test]
fn creates_pages_starts_todays_page_and_applies_all_across_pages(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    cx.update(|cx| {
        let mut tomorrow = super::live::current(cx).unwrap();
        tomorrow.today = Ok("2026-10-29".into());
        tomorrow.daily = Ok(CaptureTo::Today {
            date: "2026-10-29".into(),
            title: "2026-10-29".into(),
            folder: String::new(),
        });
        super::live::register(std::rc::Rc::new(move |_| Some(tomorrow.clone())), cx);
    });
    let started = bench
        .call(
            "append_block",
            json!({ "page": "today", "text": "first" }),
            cx,
        )
        .unwrap();
    assert_eq!(
        started["change"],
        json!({ "tool": "append_block", "title": "2026-10-29", "before": "", "after": "- first" })
    );
    bench
        .call("create_page", json!({ "title": "Plan", "text": "- a" }), cx)
        .unwrap();
    bench
        .call("capture_task", json!({ "text": "Buy milk" }), cx)
        .unwrap();
    let failed = finish(cx.update(|cx| apply_all(TERMINAL_RUN, cx)), cx);
    assert_eq!(failed, Vec::<(String, String)>::new());
    bench.settle(cx);
    assert_eq!(page_text(&bench, "2026-10-29", cx), "- first\n");
    assert_eq!(page_text(&bench, "Plan", cx), "- a");
    assert_eq!(
        page_text(&bench, "Inbox", cx),
        "- [ ] Sort the receipts\n- [ ] Buy milk #inbox ➕ [[2026-10-29]]\n"
    );
    let changes = list(cx);
    assert!(
        changes
            .iter()
            .all(|change| change.status == Status::Applied)
    );
    assert_eq!(
        finish(cx.update(|cx| undo(&changes[1].id, cx)), cx).unwrap_err(),
        "This change created [[Plan]], and Undo cannot remove pages. Delete the page from its page menu if you do not want it."
    );
}

#[gpui::test]
fn keeps_changes_with_the_notebook_they_were_proposed_for(cx: &mut TestAppContext) {
    let first = Bench::new(&[], cx);
    first
        .call("append_block", json!({ "page": "Inbox", "text": "x" }), cx)
        .unwrap();
    let id = list(cx)[0].id.clone();
    let second = Bench::new(&[], cx);
    assert!(list(cx).is_empty(), "another notebook lists none of them");
    assert_eq!(
        finish(cx.update(|cx| apply(&id, cx)), cx).unwrap_err(),
        "That AI change is gone. Ask the AI again if you still want it."
    );
    assert_eq!(page_text(&second, "Inbox", cx), "- [ ] Sort the receipts\n");
    first.register(cx);
    assert_eq!(list(cx).len(), 1);
}

#[gpui::test]
fn refuses_writes_when_off_and_groups_runs(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    assert_eq!(
        bench
            .call_with(
                "append_block",
                json!({ "page": "Inbox", "text": "x" }),
                Writes::Off,
                None,
                cx
            )
            .unwrap_err(),
        WRITES_OFF
    );
    bench
        .call_with(
            "append_block",
            json!({ "page": "Inbox", "text": "x" }),
            Writes::Review,
            Some("ask-9"),
            cx,
        )
        .unwrap();
    assert_eq!(cx.update(|cx| super::list(Some("ask-9"), cx)).len(), 1);
    assert!(
        cx.update(|cx| super::list(Some(TERMINAL_RUN), cx))
            .is_empty()
    );
    assert_eq!(
        bench
            .call("tasks", json!({}), cx)
            .map(|tasks| tasks["summary"].clone())
            .unwrap(),
        "Tasks all · 8",
        "reads pass through whatever AI writes is"
    );
}

fn review_window(
    cx: &mut TestAppContext,
) -> gpui::WindowHandle<super::super::review::AiChangesWindow> {
    cx.update(super::super::review::open);
    cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|window| window.downcast())
            .unwrap()
    })
}

/// Waits until `done` holds, driving the app, rather than assuming background work finished.
fn wait_for(cx: &mut TestAppContext, done: impl Fn(&mut TestAppContext) -> bool) {
    cx.executor().allow_parking();
    let deadline = std::time::Instant::now() + crate::test_patience::PATIENCE;
    while !done(cx) {
        cx.run_until_parked();
        assert!(
            std::time::Instant::now() < deadline,
            "the AI changes did not settle"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

#[gpui::test]
fn the_review_window_labels_diffs_applies_and_undoes_terminal_changes(cx: &mut TestAppContext) {
    use super::super::review::{diff, label, status_text};
    let bench = Bench::new(&[], cx);
    bench
        .call(
            "append_block",
            json!({ "page": "Inbox", "text": "From the terminal" }),
            cx,
        )
        .unwrap();
    assert_eq!(
        cx.read(crate::outline::notice).as_deref(),
        Some(super::super::write::STAGED)
    );
    let key = bench
        .call("read_page", json!({ "page": "Inbox" }), cx)
        .unwrap()["blocks"][0]["key"]
        .clone();
    for text in ["Sort the receipts today", "Sort the receipts tomorrow"] {
        bench
            .call(
                "replace_block",
                json!({ "page": "Inbox", "key": key, "text": text }),
                cx,
            )
            .unwrap();
    }
    let staged = list(cx);
    assert_eq!(
        staged
            .iter()
            .map(|change| label(&change.request))
            .collect::<Vec<_>>(),
        [
            "Add to [[Inbox]]",
            "Edit a block on [[Inbox]]",
            "Edit a block on [[Inbox]]"
        ]
    );
    assert_eq!(diff(&staged[0].request), ["+ - From the terminal"]);
    let (before, _) = staged[1].request.texts();
    assert_eq!(
        diff(&staged[1].request),
        [
            format!("\u{2212} {before}"),
            "+ Sort the receipts today".into()
        ]
    );
    let window = review_window(cx);
    let changes_list = window.read_with(cx, |view, _| view.list.clone()).unwrap();
    changes_list.update(cx, |list, cx| list.apply_all(cx));
    wait_for(cx, |cx| {
        window
            .read_with(cx, |view, cx| view.list.read(cx).error.is_some())
            .unwrap()
    });
    let changes = list(cx);
    assert_eq!(
        changes
            .iter()
            .map(|change| change.status)
            .collect::<Vec<_>>(),
        [Status::Applied, Status::Applied, Status::Staged]
    );
    assert_eq!(status_text(&changes[0]), "Applied");
    let error = window
        .read_with(cx, |view, cx| view.list.read(cx).error.clone())
        .unwrap()
        .unwrap();
    assert!(
        error.starts_with("1 of 3 changes did not apply: "),
        "{error}"
    );
    for at in [1, 0] {
        let id = changes[at].id.clone();
        changes_list.update(cx, |list, cx| list.undo(&id, cx));
        wait_for(cx, |cx| list(cx)[at].status == Status::Undone);
    }
    assert_eq!(status_text(&list(cx)[0]), "Undone");
    assert_eq!(page_text(&bench, "Inbox", cx), "- [ ] Sort the receipts\n");
}
