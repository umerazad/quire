//! The reference `tests/ai-page.test.tsx` and `tests/tidy.test.ts` cases in a stored notebook,
//! through the fake CLI: the exact prompt each question sends, citations that open what they
//! name, presets, follow-ups, synthesis, keeping an answer in today's page, cancellation, and
//! Tidy's review with its limits, IDs, single undo, and stale refusal.

use super::super::*;
use super::template_tests::stored_text;
use super::tests::{Notebook, active, draw, open_initialized, read, wait_until};
use crate::ai::{
    answer::{AnswerPane, Status},
    process::tests::fixture,
    proposal::tests::{Ai, ended, notice, setup, started},
    proposal::{self, Invocation},
};
use crate::notebook_tools::{
    changes::{self, Status as ChangeStatus},
    review::{ChangeList, label},
};
use gpui::TestAppContext;
use quire_core::ai::{ask::DEFAULT_SYNTHESIS, tools::Writes};

/// 2026-10-30T12:00:00Z, a date every inhabited time zone shares at that instant.
const NOW: i64 = 1_793_361_600_000;

const FRUIT: [(&str, &str, &str); 3] = [
    (
        "fruit",
        "Fruit.md",
        "- apples are good and tasty ^b-apple\n- pears are gritty\n",
    ),
    ("trees", "Trees.md", "- Orchard notes about [[Fruit]]\n"),
    (
        "work",
        "Work.md",
        "- [ ] Pay rent 📅 [[2026-10-29]]\n- [ ] Plan trip 📅 [[2026-11-03]]\n- [ ] Someday maybe\n",
    ),
];

async fn notebook(
    pages: &[(&str, &str, &str)],
    first: &str,
    cx: &mut TestAppContext,
) -> (Ai, Notebook) {
    let ai = setup(false, cx).await;
    let notebook = open_initialized(pages, first, cx);
    notebook
        .window
        .update(cx, |view, _, _| view.nav.daily.now = Rc::new(|| NOW))
        .unwrap();
    (ai, notebook)
}

fn page_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<Editor> {
    wait_until(cx, "the page editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn run_ai(notebook: &Notebook, args: &str, cx: &mut TestAppContext) {
    let editor = page_editor(notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        proposal::start(
            &editor,
            Invocation {
                args: args.into(),
                rows: None,
                verb: None,
            },
            window,
            cx,
        )
    })
    .unwrap();
    cx.run_until_parked();
}

fn pane(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<AnswerPane> {
    read(notebook, cx, |view, _| view.nav.answer.pane.clone()).expect("the answer pane is open")
}

fn status(notebook: &Notebook, index: usize, cx: &mut TestAppContext) -> Option<Status> {
    let pane = pane(notebook, cx);
    cx.read(|cx| {
        pane.read(cx)
            .turns
            .get(index)
            .map(|turn| turn.status.clone())
    })
}

fn wait_done(notebook: &Notebook, index: usize, cx: &mut TestAppContext) {
    wait_until(cx, "the answer", |cx| {
        matches!(
            status(notebook, index, cx),
            Some(Status::Done | Status::Failed { .. })
        )
    });
    assert_eq!(status(notebook, index, cx), Some(Status::Done));
}

fn type_question(notebook: &Notebook, text: &str, cx: &mut TestAppContext) {
    cx.simulate_input(notebook.window.into(), text);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    cx.run_until_parked();
}

#[gpui::test]
async fn a_question_reads_the_notebook_through_its_tools_and_citations_open_what_they_name(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    ai.answer_with_tools(
        "Apples are [[Fruit]] ((b-apple)).",
        serde_json::json!([{ "name": "read_block", "arguments": { "id": "b-apple" } }]),
        cx,
    );
    run_ai(&notebook, "", cx);
    let pane = pane(&notebook, cx);
    assert_eq!(cx.read(|cx| pane.read(cx).title()), "Ask the notebook");
    type_question(&notebook, "What about apples?", cx);
    wait_done(&notebook, 0, cx);
    draw(&notebook, cx);
    assert_eq!(ai.sent(), ["What about apples?"]);
    let calls = ai.tool_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["error"], false);
    let read: serde_json::Value = serde_json::from_str(calls[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(read["text"], "- apples are good and tasty ^b-apple");
    wait_until(cx, "the cited block's label", |cx| {
        cx.read(|cx| pane.read(cx).rendered(0, cx)).contains('↗')
    });
    cx.read(|cx| {
        let pane = pane.read(cx);
        assert!(pane.turns[0].context.is_empty());
        assert_eq!(
            pane.turns[0].tools,
            [crate::ai::answer::ToolRow {
                id: Some("toolu_0".into()),
                label: read["summary"].as_str().unwrap().into(),
                done: true,
                failed: false,
            }]
        );
        assert_eq!(
            pane.rendered(0, cx),
            "Apples are [Fruit](quire-page:Fruit) [↗ apples are good and tasty](quire-block:b-apple)."
        );
    });
    cx.update_window(notebook.window.into(), |_, window, cx| {
        AnswerPane::click_link(&pane, "quire-page:Work", window, cx)
    })
    .unwrap();
    wait_until(cx, "the cited page", |cx| active(&notebook, cx) == "work");
    cx.update_window(notebook.window.into(), |_, window, cx| {
        AnswerPane::click_link(&pane, "quire-block:b-apple", window, cx)
    })
    .unwrap();
    wait_until(cx, "the cited block's page", |cx| {
        active(&notebook, cx) == "fruit"
    });
}

#[gpui::test]
async fn questions_read_unsaved_edits_of_open_pages(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "work", cx).await;
    let editor = page_editor(&notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.move_to_end(&editor::actions::MoveToEnd, window, cx);
            editor.insert("- [ ] Water plants 📅 [[2026-10-30]]\n", window, cx);
        })
    })
    .unwrap();
    let client = read(&notebook, cx, |view, cx| view.index_client(cx).unwrap());
    // Past the index's 150 ms debounce but short of the 450 ms idle save.
    cx.executor()
        .advance_clock(std::time::Duration::from_millis(200));
    let deadline = std::time::Instant::now() + crate::test_patience::PATIENCE;
    while !futures::executor::block_on(client.with_generation(|index| index.tasks()).unwrap())
        .unwrap()
        .1
        .iter()
        .any(|task| task.text.contains("Water plants"))
    {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the unsaved task in the index"
        );
        cx.run_until_parked();
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert_eq!(
        stored_text(&notebook, "work").unwrap(),
        FRUIT[2].2,
        "the index answers from the unsaved edit"
    );
    ai.answer_with_tools(
        "Water the plants.",
        serde_json::json!([{ "name": "tasks", "arguments": { "due": "today" } }]),
        cx,
    );
    run_ai(&notebook, "due", cx);
    wait_done(&notebook, 0, cx);
    let calls = ai.tool_calls();
    let tasks: serde_json::Value =
        serde_json::from_str(calls[0]["text"].as_str().unwrap()).unwrap();
    let texts: Vec<&str> = tasks["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["text"].as_str().unwrap())
        .collect();
    assert!(
        texts == ["Water plants"] && tasks["summary"] == "Tasks due today · 1",
        "the tool reads the unsaved task: {tasks}"
    );
}

#[gpui::test]
async fn a_preset_asks_its_question_and_a_follow_up_carries_the_earlier_turn(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "work", cx).await;
    ai.answer("Pay rent is overdue.", &[], cx);
    run_ai(&notebook, "due", cx);
    wait_done(&notebook, 0, cx);
    assert_eq!(
        ai.sent(),
        ["What is due today? List overdue tasks separately, with their pages."]
    );
    ai.answer("Pears.", &[], cx);
    type_question(&notebook, "Which fruit is gritty?", cx);
    wait_done(&notebook, 1, cx);
    assert_eq!(
        ai.sent()[0],
        "Earlier in this conversation:\n<question>\nWhat is due today? List overdue tasks separately, with their pages.\n</question>\n<answer>\nPay rent is overdue.\n</answer>\n\nWhich fruit is gritty?"
    );
    let runs: Vec<String> = ai
        .bench
        .calls()
        .iter()
        .filter(|call| call["stdin"] != "Reply with the single word OK.")
        .map(|call| {
            let argv: Vec<String> = serde_json::from_value(call["argv"].clone()).unwrap();
            let at = argv.iter().position(|arg| arg == "--mcp-config").unwrap();
            let config: serde_json::Value = serde_json::from_str(&argv[at + 1]).unwrap();
            config["mcpServers"]["quire"]["env"]["QUIRE_RUN"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|run| run.starts_with("ask-")), "{runs:?}");
    assert_ne!(runs[0], runs[1], "each question is its own run");
    let pane = pane(&notebook, cx);
    let presets: Vec<String> = cx.read(|cx| {
        pane.read(cx)
            .presets()
            .into_iter()
            .map(|(_, label)| label)
            .collect()
    });
    assert_eq!(
        presets,
        [
            "Due today",
            "This week",
            "Summarize the last occurrence of Work",
            "What changed yesterday",
            "Tagged with…"
        ]
    );
}

#[gpui::test]
async fn a_synthesis_waits_for_its_instruction_and_reruns_over_the_same_references(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "fruit", cx).await;
    ai.answer("Orchards.", &[], cx);
    wait_until(cx, "the linked references", |cx| {
        read(&notebook, cx, |view, cx| {
            view.shown_references(cx).is_some()
        })
    });
    let references = read(&notebook, cx, |view, cx| view.shown_references(cx).unwrap());
    references.update(cx, |_, cx| {
        cx.emit(crate::navigation::references::ReferencesEvent::Synthesize)
    });
    cx.run_until_parked();
    draw(&notebook, cx);
    let pane = pane(&notebook, cx);
    assert_eq!(
        cx.read(|cx| pane.read(cx).title()),
        "Linked references to Fruit"
    );
    let input = cx.read(|cx| pane.read(cx).input.clone());
    assert_eq!(cx.read(|cx| input.read(cx).text(cx)), DEFAULT_SYNTHESIS);
    cx.simulate_keystrokes(notebook.window.into(), "end enter");
    cx.run_until_parked();
    assert!(ai.sent().is_empty(), "Enter alone never runs a synthesis");
    cx.update_window(notebook.window.into(), |_, window, cx| {
        input.update(cx, |editor, cx| {
            editor.set_text("List every decision.", window, cx)
        })
    })
    .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("enter"));
    wait_done(&notebook, 0, cx);
    let first = "List every decision.\n\nLinked references:\n<excerpt page=\"Trees\">\nOrchard notes about [[Fruit]]\n</excerpt>";
    assert_eq!(ai.sent(), [first]);
    assert_eq!(
        cx.read(crate::ai::settings::synthesis),
        "List every decision."
    );
    cx.update_window(notebook.window.into(), |_, window, cx| {
        pane.update(cx, |pane, cx| pane.revise(0, window, cx))
    })
    .unwrap();
    assert!(cx.read(|cx| pane.read(cx).editing_synthesis));
    assert_eq!(
        cx.read(|cx| input.read(cx).text(cx)),
        "List every decision."
    );
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("enter"));
    wait_done(&notebook, 1, cx);
    assert_eq!(
        ai.sent()[0],
        first,
        "a revised synthesis starts a fresh conversation over the same references"
    );
}

#[gpui::test]
async fn keeping_an_answer_marks_quoted_blocks_only_then(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    ai.answer("Pears [[Fruit]] \"are gritty\" and ((b-apple)).", &[], cx);
    run_ai(&notebook, "", cx);
    type_question(&notebook, "Pears?", cx);
    wait_done(&notebook, 0, cx);
    assert_eq!(
        stored_text(&notebook, "fruit").unwrap(),
        FRUIT[0].2,
        "asking leaves the notebook alone"
    );
    let pane = pane(&notebook, cx);
    let answer = cx.read(|cx| pane.read(cx).turns[0].answer.clone());
    pane.update(cx, |_, cx| {
        cx.emit(crate::ai::answer::AnswerEvent::Keep { index: 0, answer })
    });
    wait_until(cx, "the kept answer", |cx| {
        cx.read(|cx| pane.read(cx).turns[0].kept.is_some())
    });
    assert_eq!(
        cx.read(|cx| pane.read(cx).turns[0].kept.clone()),
        Some("2026-10-30".into())
    );
    let fruit = stored_text(&notebook, "fruit").unwrap();
    let id = fruit
        .strip_prefix("- apples are good and tasty ^b-apple\n- pears are gritty ^")
        .and_then(|rest| rest.strip_suffix('\n'))
        .expect("the quoted block took an ID")
        .to_owned();
    let today = read(&notebook, cx, |view, _| {
        view.nav
            .pages
            .iter()
            .find(|listed| listed.page.title == "2026-10-30")
            .map(|listed| listed.page.page.to_lossy_string())
    });
    let today = match today {
        Some(today) => today,
        None => {
            let mut found = None;
            wait_until(cx, "today's page", |cx| {
                notebook
                    .window
                    .update(cx, |view, _, cx| view.refresh_catalogue(cx))
                    .unwrap();
                cx.run_until_parked();
                found = read(&notebook, cx, |view, _| {
                    view.nav
                        .pages
                        .iter()
                        .find(|listed| listed.page.title == "2026-10-30")
                        .map(|listed| listed.page.page.to_lossy_string())
                });
                found.is_some()
            });
            found.unwrap()
        }
    };
    assert_eq!(
        stored_text(&notebook, &today).unwrap(),
        format!("- Pears \"are gritty\" (({id})) and ((b-apple)).\n")
    );
}

#[gpui::test]
async fn cancel_ends_the_question_at_once_and_drops_its_late_answer(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    let pids = ai.bench.folder.path().join("cancel-pids");
    ai.answer(
        "Late answer",
        &[
            ("FAKE_AI_DELAY_MS", "400"),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    run_ai(&notebook, "", cx);
    type_question(&notebook, "Slow?", cx);
    let running = started(&pids, cx);
    let pane = pane(&notebook, cx);
    pane.update(cx, |pane, cx| pane.cancel(cx));
    ended(&running, cx);
    let canceled = Some(Status::Failed {
        message: "Canceled.".into(),
        retry: true,
    });
    assert_eq!(status(&notebook, 0, cx), canceled);
    std::thread::sleep(std::time::Duration::from_millis(1500));
    cx.run_until_parked();
    assert_eq!(
        status(&notebook, 0, cx),
        canceled,
        "the late answer is dropped"
    );
    assert_eq!(cx.read(|cx| pane.read(cx).turns[0].answer.clone()), "");
}

#[gpui::test]
async fn closing_the_notebook_window_ends_a_running_question(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    let pids = ai.bench.folder.path().join("close-pids");
    ai.answer(
        "Never",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_FIXTURE", &fixture("claude-drift.jsonl")),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    run_ai(&notebook, "due", cx);
    let running = started(&pids, cx);
    notebook
        .window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    ended(&running, cx);
}

#[gpui::test]
async fn a_question_shows_each_tool_call_as_a_row_that_becomes_its_summary(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    cx.update(|cx| cx.set_global(ai.bench.host("claude-ask.jsonl", &[])));
    run_ai(&notebook, "", cx);
    type_question(&notebook, "What fruit ideas do I have?", cx);
    wait_done(&notebook, 0, cx);
    let pane = pane(&notebook, cx);
    let (tools, rendered) = cx.read(|cx| {
        let pane = pane.read(cx);
        (pane.turns[0].tools.clone(), pane.rendered(0, cx))
    });
    assert_eq!(
        tools.first().map(|row| row.id.is_some()),
        Some(true),
        "{tools:?}"
    );
    assert!(tools.iter().all(|row| row.done && !row.failed), "{tools:?}");
    assert!(rendered.contains("[Fruit](quire-page:Fruit)"), "{rendered}");
}

fn turn_changes(notebook: &Notebook, index: usize, cx: &mut TestAppContext) -> Entity<ChangeList> {
    let pane = pane(notebook, cx);
    cx.read(|cx| pane.read(cx).turns[index].changes.clone())
        .and_then(|view| view.downcast::<ChangeList>().ok())
        .expect("the turn has a change list")
}

/// Each change in `list`: its ID, label, and status.
fn listed(
    list: &Entity<ChangeList>,
    cx: &mut TestAppContext,
) -> Vec<(String, String, ChangeStatus)> {
    cx.read(|cx| {
        list.read(cx)
            .changes(cx)
            .into_iter()
            .map(|change| (change.id, label(&change.request), change.status))
            .collect()
    })
}

fn page_text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    page_editor(notebook, cx).read_with(cx, |editor, cx| editor.text(cx))
}

#[gpui::test]
async fn a_questions_changes_wait_under_it_to_apply_reject_and_undo(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "fruit", cx).await;
    ai.answer_with_tools(
        "I proposed two additions.",
        serde_json::json!([
            { "name": "append_block", "arguments": { "page": "Fruit", "text": "plums are sweet" } },
            { "name": "append_block", "arguments": { "page": "Trees", "text": "Plant a plum" } },
        ]),
        cx,
    );
    run_ai(&notebook, "", cx);
    type_question(&notebook, "Add plums", cx);
    wait_done(&notebook, 0, cx);
    let list = turn_changes(&notebook, 0, cx);
    assert_eq!(
        listed(&list, cx),
        [
            (
                "change-1".into(),
                "Add to [[Fruit]]".into(),
                ChangeStatus::Staged
            ),
            (
                "change-2".into(),
                "Add to [[Trees]]".into(),
                ChangeStatus::Staged
            ),
        ]
    );
    draw(&notebook, cx);
    assert_eq!(
        page_text(&notebook, cx),
        FRUIT[0].2,
        "staging leaves the page alone"
    );
    cx.update(|cx| changes::reject("change-2", cx));
    list.update(cx, |list, cx| list.apply("change-1", cx));
    wait_until(cx, "the applied change", |cx| {
        listed(&list, cx)[0].2 == ChangeStatus::Applied
    });
    assert_eq!(listed(&list, cx)[1].2, ChangeStatus::Rejected);
    assert_eq!(
        page_text(&notebook, cx),
        format!("{}- plums are sweet\n", FRUIT[0].2)
    );
    list.update(cx, |list, cx| list.undo("change-1", cx));
    wait_until(cx, "the undone change", |cx| {
        listed(&list, cx)[0].2 == ChangeStatus::Undone
    });
    assert_eq!(page_text(&notebook, cx), FRUIT[0].2);
    assert_eq!(stored_text(&notebook, "trees").unwrap(), FRUIT[1].2);
    draw(&notebook, cx);
}

#[gpui::test]
async fn apply_all_applies_a_questions_staged_changes_and_reports_a_stale_one(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "fruit", cx).await;
    ai.answer_with_tools(
        "Proposed.",
        serde_json::json!([
            { "name": "append_block", "arguments": { "page": "Trees", "text": "Plant a plum" } },
            { "name": "replace_block", "arguments": { "id": "b-apple", "text": "apples are crisp" } },
        ]),
        cx,
    );
    run_ai(&notebook, "", cx);
    type_question(&notebook, "Tidy fruit", cx);
    wait_done(&notebook, 0, cx);
    let list = turn_changes(&notebook, 0, cx);
    wait_until(cx, "both changes", |cx| listed(&list, cx).len() == 2);
    let editor = page_editor(&notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.move_to_beginning(&editor::actions::MoveToBeginning, window, cx);
            editor.move_to_end_of_line(&Default::default(), window, cx);
            editor.insert("!", window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    list.update(cx, |list, cx| list.apply_all(cx));
    wait_until(cx, "Apply all", |cx| {
        cx.read(|cx| list.read(cx).error.is_some())
    });
    assert_eq!(
        cx.read(|cx| list.read(cx).error.clone()).unwrap(),
        format!("1 of 2 changes did not apply: {}", changes::STALE)
    );
    assert_eq!(
        listed(&list, cx)
            .into_iter()
            .map(|(_, _, status)| status)
            .collect::<Vec<_>>(),
        [ChangeStatus::Applied, ChangeStatus::Staged]
    );
    assert!(page_text(&notebook, cx).starts_with("- apples are good and tasty ^b-apple!"));
}

#[gpui::test]
async fn with_ai_writes_on_apply_a_questions_changes_apply_at_once_and_undo(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "fruit", cx).await;
    cx.update(|cx| crate::ai::settings::set_writes(Writes::Apply, cx));
    cx.run_until_parked();
    ai.answer_with_tools(
        "Added.",
        serde_json::json!([
            { "name": "append_block", "arguments": { "page": "Fruit", "text": "figs" } },
        ]),
        cx,
    );
    run_ai(&notebook, "", cx);
    type_question(&notebook, "Add figs", cx);
    wait_done(&notebook, 0, cx);
    let applied: serde_json::Value =
        serde_json::from_str(ai.tool_calls()[0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(applied["status"], "applied");
    assert!(
        ai.bench.calls().iter().any(|call| call["argv"]
            .as_array()
            .unwrap()
            .iter()
            .any(|arg| arg.as_str().unwrap().contains("apply at once"))),
        "the run's rules say writes apply at once"
    );
    let list = turn_changes(&notebook, 0, cx);
    assert_eq!(
        listed(&list, cx),
        [(
            "change-1".into(),
            "Add to [[Fruit]]".into(),
            ChangeStatus::Applied
        )]
    );
    assert_eq!(page_text(&notebook, cx), format!("{}- figs\n", FRUIT[0].2));
    list.update(cx, |list, cx| list.undo("change-1", cx));
    wait_until(cx, "the undone change", |cx| {
        listed(&list, cx)[0].2 == ChangeStatus::Undone
    });
    assert_eq!(page_text(&notebook, cx), FRUIT[0].2);
}

/// Calls a notebook tool as a terminal session would, waiting for its answer.
fn tool_call(
    name: &str,
    input: serde_json::Value,
    cx: &mut TestAppContext,
) -> Result<serde_json::Value, String> {
    let call = crate::notebook_tools::bridge::Call {
        name: name.into(),
        input,
        run: None,
        writes: Writes::Review,
    };
    let answered = cx.update(|cx| crate::notebook_tools::host::answer(call, cx));
    let result = std::rc::Rc::new(std::cell::RefCell::new(None));
    cx.spawn({
        let result = result.clone();
        async move |_| *result.borrow_mut() = Some(answered.await)
    })
    .detach();
    wait_until(cx, "the tool call", |_| result.borrow().is_some());
    result.take().unwrap()
}

#[gpui::test]
async fn the_pane_lists_changes_from_a_terminal(cx: &mut TestAppContext) {
    let (_ai, notebook) = notebook(&FRUIT, "fruit", cx).await;
    run_ai(&notebook, "", cx);
    let pane = pane(&notebook, cx);
    let terminal = cx
        .read(|cx| pane.read(cx).terminal.clone())
        .and_then(|view| view.downcast::<ChangeList>().ok())
        .unwrap();
    assert!(listed(&terminal, cx).is_empty());
    tool_call(
        "append_block",
        serde_json::json!({ "page": "Trees", "text": "From the terminal" }),
        cx,
    )
    .unwrap();
    assert_eq!(
        listed(&terminal, cx),
        [(
            "change-1".into(),
            "Add to [[Trees]]".into(),
            ChangeStatus::Staged
        )]
    );
    draw(&notebook, cx);
}

#[gpui::test]
async fn questions_explain_why_they_cannot_run_while_ai_is_off(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    cx.update(|cx| crate::ai::settings::set_enabled(false, cx))
        .unwrap();
    cx.run_until_parked();
    run_ai(&notebook, "due", cx);
    let pane = pane(&notebook, cx);
    assert_eq!(
        cx.read(|cx| pane.read(cx).notice.clone()).as_deref(),
        Some(quire_core::ai::backend::AI_OFF)
    );
    assert!(ai.sent().is_empty());
}

fn palette_commands(notebook: &Notebook, query: &str, cx: &mut TestAppContext) -> Vec<String> {
    use crate::navigation::palette::PaletteItem;
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("k"));
    cx.run_until_parked();
    super::tests::type_in_palette(notebook, query, cx);
    super::tests::palette_items(notebook, cx)
        .into_iter()
        .filter_map(|item| match item {
            PaletteItem::Command(row) => Some(row.label),
            _ => None,
        })
        .collect()
}

#[gpui::test]
async fn the_find_palette_runs_ai_and_appearance_commands(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&FRUIT, "fruit", cx).await;
    page_editor(&notebook, cx);
    assert_eq!(
        palette_commands(&notebook, "ai:", cx),
        [
            "AI: Tighten wording",
            "AI: Rewrite as prose",
            "AI: Rewrite as an outline",
            "AI: Continue writing",
            "AI: Check facts and logic",
            "AI: Ask about this text",
            "AI: Ask the notebook",
            "AI: Due today",
            "AI: This week",
            "AI: Summarize the last occurrence of Fruit",
            "AI: What changed yesterday",
            "AI: Tagged with…",
        ]
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    assert_eq!(
        palette_commands(&notebook, "tighten", cx),
        ["AI: Tighten wording"]
    );
    ai.answer("apples are tasty ^b-apple", &[], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the verb to run", |_| !ai.sent().is_empty());
    assert!(
        ai.sent()[0].starts_with(quire_core::ai::Verb::Tighten.instruction()),
        "the verb runs on the block at the caret: {:?}",
        ai.sent()
    );
    assert!(ai.sent()[0].contains("apples are good and tasty"));

    palette_commands(&notebook, "ask the notebook", cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the answer pane", |cx| {
        read(&notebook, cx, |view, _| view.nav.answer.pane.is_some())
    });
    let asked = pane(&notebook, cx);
    assert_eq!(cx.read(|cx| asked.read(cx).title()), "Ask the notebook");

    assert_eq!(
        palette_commands(&notebook, "appearance: d", cx),
        ["Appearance: Dark"]
    );
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the dark appearance", |cx| {
        cx.read(|cx| crate::preferences::Preferences::current(cx).appearance)
            == settings::ThemeAppearanceMode::Dark
    });

    cx.update(|cx| crate::preferences::change(crate::preferences::Change::Vim(true), cx));
    cx.run_until_parked();
    let with_vim = palette_commands(&notebook, "ai:", cx);
    assert_eq!(
        with_vim.first().map(String::as_str),
        Some("AI: Ask the notebook"),
        "with Vim on, :ai runs the verbs and the palette offers only questions"
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    cx.update(|cx| crate::ai::settings::set_enabled(false, cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        palette_commands(&notebook, "ai:", cx).is_empty(),
        "AI commands are offered only while AI is on"
    );
}

const DAY: [(&str, &str, &str); 4] = [
    (
        "day",
        "2026-10-27.md",
        "- Weekly sync with Dana ^keep\n  - [ ] Send the budget to Dana by Thursday\n  - Atlas review moved\n",
    ),
    ("dana", "People/Dana Reyes.md", "- Engineering lead\n"),
    ("atlas", "Projects/Atlas.md", "- Pilot\n"),
    ("other", "Other.md", "- Unrelated\n"),
];

fn tidy(notebook: &Notebook, cx: &mut TestAppContext) {
    let editor = page_editor(notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        proposal::tidy(&editor, window, cx)
    })
    .unwrap();
    cx.run_until_parked();
}

/// The key `read_page` gives each block of `page`, with the line it starts on.
fn keys(page: &str, cx: &mut TestAppContext) -> Vec<(String, String)> {
    let read = tool_call("read_page", serde_json::json!({ "page": page }), cx).unwrap();
    let lines: Vec<&str> = read["text"].as_str().unwrap().lines().collect();
    read["blocks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|block| {
            let line = block["line"].as_u64().unwrap() as usize;
            (
                lines[line - 1].to_owned(),
                block["key"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn key_of(keys: &[(String, String)], text: &str) -> String {
    keys.iter()
        .find(|(block, _)| block.contains(text))
        .map(|(_, key)| key.clone())
        .unwrap_or_else(|| panic!("no block contains {text}: {keys:?}"))
}

#[gpui::test]
async fn tidy_stages_only_allowed_additions_to_its_day_and_apply_all_is_one_undo_step(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&DAY, "day", cx).await;
    cx.update(|cx| crate::ai::settings::set_writes(Writes::Apply, cx));
    cx.run_until_parked();
    let day = keys("2026-10-27", cx);
    let other = keys("Other", cx);
    ai.answer_with_tools(
        "Proposed a link, a state and a date.",
        serde_json::json!([
            { "name": "replace_block", "arguments": { "id": "keep", "text": "Weekly sync with [[Dana Reyes|Dana]]" } },
            { "name": "replace_block", "arguments": { "page": "2026-10-27", "key": key_of(&day, "Send the budget"), "text": "[ ] Send the budget to Dana by Thursday #next 📅 [[2026-10-29]]" } },
            { "name": "replace_block", "arguments": { "page": "2026-10-27", "key": key_of(&day, "Atlas review"), "text": "[[Atlas]] review postponed" } },
            { "name": "replace_block", "arguments": { "page": "2026-10-27", "key": key_of(&day, "Atlas review"), "text": "[[Nobody]] Atlas review moved" } },
            { "name": "replace_block", "arguments": { "page": "Other", "key": key_of(&other, "Unrelated"), "text": "[[Atlas]] Unrelated" } },
            { "name": "append_block", "arguments": { "page": "2026-10-27", "text": "Invented" } },
        ]),
        cx,
    );
    tidy(&notebook, cx);
    wait_done(&notebook, 0, cx);
    let pane = pane(&notebook, cx);
    assert_eq!(cx.read(|cx| pane.read(cx).title()), "Tidy 2026-10-27");
    assert_eq!(
        ai.sent(),
        [quire_core::ai::tidy::question("2026-10-27", "2026-10-28")]
    );
    let outcomes: Vec<(bool, String)> = ai
        .tool_calls()
        .iter()
        .map(|call| {
            let text = call["text"].as_str().unwrap().to_owned();
            let status = serde_json::from_str::<serde_json::Value>(&text)
                .ok()
                .and_then(|result| result["status"].as_str().map(str::to_owned));
            (call["error"] == true, status.unwrap_or(text))
        })
        .collect();
    assert_eq!(
        outcomes,
        [
            (false, "awaiting review".to_owned()),
            (false, "awaiting review".to_owned()),
            (true, quire_core::ai::tidy::REWRITE.to_owned()),
            (
                true,
                "[[Nobody]] is not a page in this notebook. Link only to pages that exist."
                    .to_owned()
            ),
            (true, quire_core::ai::tidy::OTHER_PAGE.to_owned()),
            (true, quire_core::ai::tidy::TIDY_ONLY.to_owned()),
        ],
        "Tidy stays in review even with AI writes on Apply"
    );
    let run = cx.read(|cx| pane.read(cx).turns[0].run.clone());
    assert_eq!(
        cx.read(|cx| crate::notebook_tools::host::tidy_day(Some(&run), cx)),
        None,
        "the run's Tidy limits end with it"
    );
    assert_eq!(page_text(&notebook, cx), DAY[0].2);
    let list = turn_changes(&notebook, 0, cx);
    assert_eq!(listed(&list, cx).len(), 2);
    list.update(cx, |list, cx| list.apply_all(cx));
    wait_until(cx, "Apply all", |cx| {
        listed(&list, cx)
            .iter()
            .all(|(_, _, status)| *status == ChangeStatus::Applied)
    });
    assert_eq!(
        page_text(&notebook, cx),
        "- Weekly sync with [[Dana Reyes|Dana]] ^keep\n  - [ ] Send the budget to Dana by Thursday #next 📅 [[2026-10-29]]\n  - Atlas review moved\n"
    );
    let editor = page_editor(&notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.undo(&editor::actions::Undo, window, cx)
        })
    })
    .unwrap();
    assert_eq!(
        page_text(&notebook, cx),
        DAY[0].2,
        "one undo restores the page"
    );
}

#[gpui::test]
async fn tidy_needs_ai_writes_and_runs_only_on_days(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&DAY, "day", cx).await;
    cx.update(|cx| crate::ai::settings::set_writes(Writes::Off, cx));
    cx.run_until_parked();
    tidy(&notebook, cx);
    let pane = pane(&notebook, cx);
    assert_eq!(
        cx.read(|cx| pane.read(cx).notice.clone()).as_deref(),
        Some(quire_core::ai::tidy::WRITES_OFF)
    );
    assert!(ai.sent().is_empty());
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("other".into(), false, window, cx)
        })
        .unwrap();
    wait_until(cx, "the other page", |cx| active(&notebook, cx) == "other");
    let other = page_editor(&notebook, cx);
    assert!(!other.read_with(cx, proposal::on_daily_page));
    tidy(&notebook, cx);
    assert_eq!(notice(cx), proposal::NOT_DAILY);
    assert!(ai.sent().is_empty());
}
