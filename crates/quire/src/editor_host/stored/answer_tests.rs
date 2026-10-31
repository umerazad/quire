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
use gpui::TestAppContext;
use quire_core::ai::ask::DEFAULT_SYNTHESIS;

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
async fn a_question_sends_matching_excerpts_and_citations_open_what_they_name(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "trees", cx).await;
    ai.answer("Apples are [[Fruit]] ((b-apple)).", &[], cx);
    run_ai(&notebook, "", cx);
    let pane = pane(&notebook, cx);
    assert_eq!(cx.read(|cx| pane.read(cx).title()), "Ask the notebook");
    type_question(&notebook, "What about apples?", cx);
    wait_done(&notebook, 0, cx);
    draw(&notebook, cx);
    assert_eq!(
        ai.sent(),
        [
            "What about apples?\n\nMatching blocks:\n<excerpt page=\"Fruit\" id=\"b-apple\">\napples are good and tasty\n</excerpt>"
        ]
    );
    cx.read(|cx| {
        let pane = pane.read(cx);
        assert_eq!(
            pane.turns[0].context,
            ["Matching blocks: 1 block from 1 page"]
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
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
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
    ai.answer("Water the plants.", &[], cx);
    run_ai(&notebook, "due", cx);
    wait_done(&notebook, 0, cx);
    assert!(
        ai.sent()[0].ends_with("<excerpt page=\"Work\">\n[ ] Water plants 📅 [[2026-10-30]] (due 2026-10-30)\n</excerpt>"),
        "{}",
        ai.sent()[0]
    );
}

#[gpui::test]
async fn a_preset_sends_due_tasks_and_a_follow_up_carries_the_earlier_turn(
    cx: &mut TestAppContext,
) {
    let (ai, notebook) = notebook(&FRUIT, "work", cx).await;
    ai.answer("Pay rent is overdue.", &[], cx);
    run_ai(&notebook, "due", cx);
    wait_done(&notebook, 0, cx);
    assert_eq!(
        ai.sent(),
        [
            "What is due today? List overdue tasks separately, with their pages.\n\nTasks due today or overdue:\n<excerpt page=\"Work\">\n[ ] Pay rent 📅 [[2026-10-29]] (overdue since 2026-10-29)\n</excerpt>"
        ]
    );
    ai.answer("Pears.", &[], cx);
    type_question(&notebook, "Which fruit is gritty?", cx);
    wait_done(&notebook, 1, cx);
    assert_eq!(
        ai.sent()[0],
        "Earlier in this conversation:\n<question>\nWhat is due today? List overdue tasks separately, with their pages.\n</question>\n<answer>\nPay rent is overdue.\n</answer>\n\nWhich fruit is gritty?\n\nMatching blocks:\n<excerpt page=\"Fruit\">\npears are gritty\n</excerpt>\n<excerpt page=\"Fruit\" id=\"b-apple\">\napples are good and tasty\n</excerpt>\n<excerpt page=\"Trees\">\nOrchard notes about [[Fruit]]\n</excerpt>"
    );
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

const TIDIED: &str = "- Weekly sync with [[Dana Reyes|Dana]]\n  - [ ] Send the budget to Dana by Thursday #next 📅 [[2026-10-29]]\n  - [[Atlas]] review postponed\n  - Invented line";

fn tidy(notebook: &Notebook, cx: &mut TestAppContext) {
    let editor = page_editor(notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        proposal::tidy(&editor, window, cx)
    })
    .unwrap();
}

fn tidy_proposal(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<proposal::ProposalView> {
    let editor = page_editor(notebook, cx);
    wait_until(cx, "the Tidy proposal", |cx| {
        editor
            .read_with(cx, |editor, _| proposal::current(editor))
            .is_some_and(|view| view.read_with(cx, |view, _| view.is_ready()))
    });
    editor
        .read_with(cx, |editor, _| proposal::current(editor))
        .unwrap()
}

#[gpui::test]
async fn tidy_proposes_only_additions_and_accepts_them_as_one_undo_step(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&DAY, "day", cx).await;
    ai.answer(TIDIED, &[], cx);
    let editor = page_editor(&notebook, cx);
    assert!(editor.read_with(cx, proposal::on_daily_page));
    run_ai(&notebook, "", cx);
    let pane = pane(&notebook, cx);
    let presets: Vec<String> = cx.read(|cx| {
        pane.read(cx)
            .presets()
            .into_iter()
            .map(|(_, label)| label)
            .collect()
    });
    assert!(presets.contains(&"Tidy this day".to_owned()), "{presets:?}");
    pane.update(cx, |_, cx| cx.emit(crate::ai::answer::AnswerEvent::Tidy));
    let view = tidy_proposal(&notebook, cx);
    let sent = ai.sent();
    assert_eq!(sent.len(), 1);
    assert_eq!(
        sent[0],
        quire_core::ai::tidy::prompt(
            "2026-10-27",
            DAY[0].2,
            &["Atlas".to_owned(), "Dana Reyes".to_owned()],
            "2026-10-28"
        )
    );
    let refused: Vec<(String, String)> = cx.read(|cx| {
        view.read(cx)
            .refused()
            .iter()
            .map(|refusal| (refusal.line.clone(), refusal.reason.clone()))
            .collect()
    });
    assert_eq!(
        refused,
        [
            (
                "  - [[Atlas]] review postponed".to_owned(),
                quire_core::ai::tidy::REWRITE.to_owned()
            ),
            (
                "  - Invented line".to_owned(),
                quire_core::ai::tidy::ADDED_LINE.to_owned()
            ),
        ]
    );
    let editor = page_editor(&notebook, cx);
    assert_eq!(editor.read_with(cx, |editor, cx| editor.text(cx)), DAY[0].2);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        proposal::accept(&editor, window, cx)
    })
    .unwrap();
    cx.run_until_parked();
    let tidied = "- Weekly sync with [[Dana Reyes|Dana]] ^keep\n  - [ ] Send the budget to Dana by Thursday #next 📅 [[2026-10-29]]\n  - Atlas review moved\n";
    assert_eq!(editor.read_with(cx, |editor, cx| editor.text(cx)), tidied);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.undo(&editor::actions::Undo, window, cx)
        })
    })
    .unwrap();
    assert_eq!(
        editor.read_with(cx, |editor, cx| editor.text(cx)),
        DAY[0].2,
        "one undo restores the page"
    );
}

#[gpui::test]
async fn tidy_refuses_a_changed_page_and_runs_only_on_days(cx: &mut TestAppContext) {
    let (ai, notebook) = notebook(&DAY, "day", cx).await;
    ai.answer(TIDIED, &[], cx);
    tidy(&notebook, cx);
    tidy_proposal(&notebook, cx);
    let editor = page_editor(&notebook, cx);
    cx.update_window(notebook.window.into(), |_, window, cx| {
        editor.update(cx, |editor, cx| {
            editor.move_to_end(&editor::actions::MoveToEnd, window, cx);
            editor.insert("- typed meanwhile\n", window, cx);
        })
    })
    .unwrap();
    cx.run_until_parked();
    cx.update_window(notebook.window.into(), |_, window, cx| {
        proposal::accept(&editor, window, cx)
    })
    .unwrap();
    cx.run_until_parked();
    assert_eq!(notice(cx), proposal::STALE);
    assert_eq!(
        editor.read_with(cx, |editor, cx| editor.text(cx)),
        format!("{}- typed meanwhile\n", DAY[0].2)
    );

    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("other".into(), false, window, cx)
        })
        .unwrap();
    wait_until(cx, "the other page", |cx| active(&notebook, cx) == "other");
    let other = page_editor(&notebook, cx);
    assert!(!other.read_with(cx, proposal::on_daily_page));
    let before = ai.sent().len();
    tidy(&notebook, cx);
    cx.run_until_parked();
    assert_eq!(notice(cx), proposal::NOT_DAILY);
    assert_eq!(ai.sent().len(), before);
}
