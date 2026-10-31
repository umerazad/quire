//! The reference `tests/editor-ai.test.tsx` cases, through native keys and the fake CLI: a
//! proposal streams into a block and leaves the page alone until Accept, which is one undo step
//! keeping block IDs; Reject, a newer command, and closing the window end the run's process
//! tree; a range edited meanwhile is refused.

use super::*;
use crate::ai::{
    process::tests::{Bench, fixture},
    settings::tests::{init_ai, pass_test, wait_until},
};
use crate::editor_host::NotebookWindow;
use gpui::{TestAppContext, WindowHandle};

pub(crate) struct Ai {
    pub bench: Bench,
    _fs: Arc<fs::FakeFs>,
}

/// AI commands on and tested against the fake CLI, with Vim on or off.
pub(crate) async fn setup(vim: bool, cx: &mut TestAppContext) -> Ai {
    let bench = Bench::new();
    let fs = init_ai(&bench, cx).await;
    cx.update(|cx| crate::preferences::change(crate::preferences::Change::Vim(vim), cx));
    cx.run_until_parked();
    pass_test(cx);
    Ai { bench, _fs: fs }
}

impl Ai {
    /// The next run streams `result` in two pieces, then finishes with it.
    pub fn answer(&self, result: &str, extra: &[(&str, &str)], cx: &mut TestAppContext) {
        let split = result.len() / 2;
        let split = (split..=result.len())
            .find(|index| result.is_char_boundary(*index))
            .unwrap_or(result.len());
        let lines = [
            serde_json::json!({"type":"system","subtype":"init","session_id":"s","claude_code_version":"2.1.312"}),
            serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":&result[..split]}}}),
            serde_json::json!({"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":&result[split..]}}}),
            serde_json::json!({"type":"result","subtype":"success","is_error":false,"result":result,"session_id":"s"}),
        ];
        let path = self
            .bench
            .folder
            .path()
            .join(format!("answer-{}.jsonl", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            lines
                .iter()
                .map(|line| format!("{line}\n"))
                .collect::<String>(),
        )
        .unwrap();
        let path = path.to_string_lossy().into_owned();
        let mut extra = extra.to_vec();
        extra.insert(0, ("FAKE_AI_FIXTURE", &path));
        let host = self.bench.host("claude-transform.jsonl", &extra);
        cx.update(|cx| cx.set_global(host));
    }

    /// The next run makes `calls` (`[{name, arguments}]`) through its tool server, then streams
    /// `result`.
    pub fn answer_with_tools(
        &self,
        result: &str,
        calls: serde_json::Value,
        cx: &mut TestAppContext,
    ) {
        let file = self
            .bench
            .folder
            .path()
            .join(format!("calls-{}.json", uuid::Uuid::new_v4()));
        std::fs::write(&file, calls.to_string()).unwrap();
        let log = self.tool_log();
        std::fs::remove_file(&log).ok();
        self.answer(
            result,
            &[
                ("FAKE_AI_TOOLS", &file.to_string_lossy()),
                ("FAKE_AI_TOOL_LOG", &log.to_string_lossy()),
            ],
            cx,
        );
    }

    fn tool_log(&self) -> std::path::PathBuf {
        self.bench.folder.path().join("tools.json")
    }

    /// The last run's tool calls: name, arguments, the text each returned, and whether it failed.
    pub fn tool_calls(&self) -> Vec<serde_json::Value> {
        serde_json::from_str(&std::fs::read_to_string(self.tool_log()).unwrap()).unwrap()
    }

    /// What the CLI received on standard input, last run first.
    pub fn sent(&self) -> Vec<String> {
        let mut sent: Vec<String> = self
            .bench
            .calls()
            .into_iter()
            .filter_map(|call| call["stdin"].as_str().map(str::to_owned))
            .filter(|stdin| stdin != "Reply with the single word OK.")
            .collect();
        sent.reverse();
        sent
    }
}

pub(crate) fn page(text: &str, cx: &mut TestAppContext) -> WindowHandle<NotebookWindow> {
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    cx.run_until_parked();
    window
}

pub(crate) fn editor(
    window: WindowHandle<NotebookWindow>,
    cx: &mut TestAppContext,
) -> Entity<Editor> {
    window
        .read_with(cx, |view, _| view.write_editor().clone())
        .unwrap()
}

pub(crate) fn text(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    editor(window, cx).read_with(cx, |editor, cx| editor.text(cx))
}

pub(crate) fn select(
    window: WindowHandle<NotebookWindow>,
    range: Range<usize>,
    cx: &mut TestAppContext,
) {
    let editor = editor(window, cx);
    window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                    selections.select_ranges([editor::MultiBufferOffset(range.start)
                        ..editor::MultiBufferOffset(range.end)])
                })
            })
        })
        .unwrap();
    cx.run_until_parked();
}

pub(crate) fn proposal(
    window: WindowHandle<NotebookWindow>,
    cx: &mut TestAppContext,
) -> Option<Entity<ProposalView>> {
    editor(window, cx).read_with(cx, |editor, _| current(editor))
}

pub(crate) fn wait_ready(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) {
    wait_until(cx, "the proposal to be ready", |cx| {
        proposal(window, cx).is_some_and(|view| view.read_with(cx, |view, _| view.is_ready()))
    });
}

fn shown(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    proposal(window, cx)
        .map(|view| view.read_with(cx, |view, _| view.shown_text()))
        .unwrap_or_default()
}

fn diff_text(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    let view = proposal(window, cx).unwrap();
    let diff = view.read_with(cx, |view, _| view.diff_editor().cloned().unwrap());
    diff.read_with(cx, |diff, cx| diff.buffer().read(cx).snapshot(cx).text())
}

pub(crate) fn keys(window: WindowHandle<NotebookWindow>, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), keys);
    cx.run_until_parked();
}

pub(crate) fn notice(cx: &mut TestAppContext) -> String {
    cx.update(|cx| crate::outline::notice(cx))
        .map(|notice| notice.to_string())
        .unwrap_or_default()
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

/// Waits for the fake CLI's process IDs, then returns them.
pub(crate) fn started(path: &std::path::Path, cx: &mut TestAppContext) -> Vec<String> {
    wait_until(cx, "the fake CLI to start", |_| path.exists());
    std::fs::read_to_string(path)
        .unwrap()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

pub(crate) fn ended(pids: &[String], cx: &mut TestAppContext) {
    for pid in pids {
        wait_until(cx, &format!("process {pid} to end"), |_| !alive(pid));
    }
}

/// Spells `text` as keystrokes for the command prompt.
pub(crate) fn typed(text: &str) -> String {
    text.chars()
        .map(|c| match c {
            ' ' => "space".to_owned(),
            c => c.to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[gpui::test]
async fn streams_a_proposal_leaves_the_page_alone_until_accept_and_undoes_in_one_step(
    cx: &mut TestAppContext,
) {
    let ai = setup(true, cx).await;
    let original = "- apples are good and they are tasty ^b-1\n- pears\n";
    let window = page(original, cx);
    ai.answer(
        "- apples are good and tasty",
        &[("FAKE_AI_DELAY_MS", "150")],
        cx,
    );
    keys(window, &format!(": {} enter", typed("1ai tighten")), cx);
    wait_until(cx, "the first streamed piece", |cx| {
        shown(window, cx) == "- apples are "
    });
    assert_eq!(
        text(window, cx),
        original,
        "streaming leaves the page alone"
    );
    let view = proposal(window, cx).unwrap();
    view.read_with(cx, |view, _| {
        assert!(view.is_writing());
        assert_eq!(
            view.context(),
            ["Page: Untitled", "Selected text: 1 line, blocks ^b-1"]
        );
    });
    wait_ready(window, cx);
    assert_eq!(
        diff_text(window, cx),
        "- apples are good and they are tasty ^b-1\n- apples are good and tasty"
    );
    assert_eq!(
        text(window, cx),
        original,
        "a ready proposal leaves the page alone"
    );
    let sent = &ai.sent()[0];
    assert!(sent.starts_with(Verb::Tighten.instruction()), "{sent}");
    assert!(sent.contains("Text:\n<text>\n- apples are good and they are tasty ^b-1\n</text>"));
    keys(window, "enter", cx);
    assert_eq!(text(window, cx), "- apples are good and tasty\n- pears\n");
    assert!(proposal(window, cx).is_none());
    assert!(
        notice(cx)
            .starts_with("1 block marker (^b-1) did not survive because their lines changed.")
    );
    keys(window, "u", cx);
    assert_eq!(
        text(window, cx),
        original,
        "one undo step restores the page"
    );
}

#[gpui::test]
async fn restores_dropped_markers_on_unchanged_lines_without_vim(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- one ^b-1\n- two ^b-2\n", cx);
    select(window, 0..21, cx);
    ai.answer("- one\n- 2", &[], cx);
    let host = editor(window, cx);
    window
        .update(cx, |_, window, cx| {
            start(
                &host,
                Invocation {
                    args: String::new(),
                    rows: None,
                    verb: Some(Verb::Tighten),
                },
                window,
                cx,
            )
        })
        .unwrap();
    wait_ready(window, cx);
    assert!(ai.sent()[0].contains("<text>\n- one ^b-1\n- two ^b-2\n</text>"));
    window
        .update(cx, |_, window, cx| assert!(accept(&host, window, cx)))
        .unwrap();
    assert_eq!(text(window, cx), "- one ^b-1\n- 2\n");
    assert!(notice(cx).starts_with("1 block marker (^b-2) did not survive"));
    let undo = if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };
    keys(window, undo, cx);
    assert_eq!(text(window, cx), "- one ^b-1\n- two ^b-2\n");
}

#[gpui::test]
async fn escape_rejects_without_a_trace_and_ends_the_process_tree(cx: &mut TestAppContext) {
    let ai = setup(true, cx).await;
    let original = "- keep me\n";
    let window = page(original, cx);
    let pids = ai.bench.folder.path().join("pids");
    ai.answer(
        "- partial",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_FIXTURE", &fixture("claude-drift.jsonl")),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    keys(window, &format!(": {} enter", typed("ai prose")), cx);
    let running = started(&pids, cx);
    assert!(proposal(window, cx).is_some());
    keys(window, "escape", cx);
    assert!(proposal(window, cx).is_none());
    ended(&running, cx);
    keys(window, "u", cx);
    assert_eq!(
        text(window, cx),
        original,
        "nothing was written or recorded for undo"
    );
}

#[gpui::test]
async fn refuses_a_range_changed_while_the_proposal_was_written(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- one\n- two\n", cx);
    ai.answer("- ONE", &[], cx);
    let host = editor(window, cx);
    window
        .update(cx, |_, window, cx| {
            start(
                &host,
                Invocation {
                    args: "shout".into(),
                    rows: None,
                    verb: None,
                },
                window,
                cx,
            )
        })
        .unwrap();
    wait_ready(window, cx);
    host.update(cx, |editor, cx| {
        editor.buffer().update(cx, |buffer, cx| {
            buffer.edit(
                [(
                    editor::MultiBufferOffset(2)..editor::MultiBufferOffset(5),
                    "uno",
                )],
                None,
                cx,
            )
        })
    });
    keys(window, "enter", cx);
    assert_eq!(notice(cx), STALE);
    assert_eq!(text(window, cx), "- uno\n- two\n");
    assert!(
        proposal(window, cx).is_some(),
        "the refused proposal stays for review"
    );
    keys(window, "escape", cx);
    assert!(proposal(window, cx).is_none());
}

#[gpui::test]
async fn inserts_continue_results_below_and_drops_copied_block_ids(cx: &mut TestAppContext) {
    let ai = setup(true, cx).await;
    let window = page("- first ^b-1\n  - nested idea\n", cx);
    select(window, 20..20, cx);
    ai.answer("- next idea ^b-new\n- again ^b-1", &[], cx);
    keys(window, &format!(": {} enter", typed("ai continue")), cx);
    wait_ready(window, cx);
    assert_eq!(diff_text(window, cx), "\n  - next idea ^b-new\n  - again");
    keys(window, "enter", cx);
    assert_eq!(
        text(window, cx),
        "- first ^b-1\n  - nested idea\n  - next idea ^b-new\n  - again\n"
    );
}

#[gpui::test]
async fn drafts_on_a_blank_line_and_refuses_verbs_that_need_text(cx: &mut TestAppContext) {
    let ai = setup(true, cx).await;
    let window = page("- apples\n\n- \n", cx);
    select(window, 9..9, cx);
    ai.answer("- pears\n- plums", &[], cx);
    keys(
        window,
        &format!(": {} enter", typed("ai list two fruits")),
        cx,
    );
    wait_ready(window, cx);
    let sent = &ai.sent()[0];
    assert!(sent.contains("list two fruits") && sent.contains("<context>\n- apples\n</context>"));
    let view = proposal(window, cx).unwrap();
    view.read_with(cx, |view, _| {
        assert_eq!(
            view.context(),
            ["Page: Untitled", "Text above the cursor: 1 line"]
        )
    });
    let host = editor(window, cx);
    window
        .update(cx, |_, window, cx| assert!(accept(&host, window, cx)))
        .unwrap();
    assert_eq!(text(window, cx), "- apples\n- pears\n- plums\n- \n");
    select(window, 25..25, cx);
    ai.answer("figs", &[], cx);
    keys(
        window,
        &format!(": {} enter", typed("ai name a third fruit")),
        cx,
    );
    wait_ready(window, cx);
    window
        .update(cx, |_, window, cx| assert!(accept(&host, window, cx)))
        .unwrap();
    assert_eq!(text(window, cx), "- apples\n- pears\n- plums\n- figs\n");
    host.update(cx, |editor, cx| {
        editor.buffer().update(cx, |buffer, cx| {
            buffer.edit(
                [(
                    editor::MultiBufferOffset(32)..editor::MultiBufferOffset(32),
                    "\n",
                )],
                None,
                cx,
            )
        })
    });
    select(window, 33..33, cx);
    keys(window, &format!(": {} enter", typed("ai tighten")), cx);
    assert!(
        notice(cx).starts_with(":ai tighten needs text to work on."),
        "{}",
        notice(cx)
    );
    assert!(proposal(window, cx).is_none());
}

#[gpui::test]
async fn a_new_command_replaces_the_proposal_and_ends_the_earlier_run(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- one\n- two\n", cx);
    let pids = ai.bench.folder.path().join("pids");
    ai.answer(
        "",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_FIXTURE", &fixture("claude-drift.jsonl")),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    let host = editor(window, cx);
    let run = |verb, window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext| {
        window
            .update(cx, |_, window, cx| {
                start(
                    &host,
                    Invocation {
                        args: String::new(),
                        rows: None,
                        verb: Some(verb),
                    },
                    window,
                    cx,
                )
            })
            .unwrap();
        cx.run_until_parked();
    };
    run(Verb::Tighten, window, cx);
    let first = started(&pids, cx);
    std::fs::remove_file(&pids).unwrap();
    ai.answer("- 2", &[], cx);
    select(window, 8..8, cx);
    run(Verb::Prose, window, cx);
    ended(&first, cx);
    wait_ready(window, cx);
    assert!(ai.sent()[0].contains("<text>\n- two\n</text>"));
    assert_eq!(diff_text(window, cx), "- two\n- 2");
}

#[gpui::test]
async fn closing_the_window_ends_a_run_still_writing(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- one\n", cx);
    let pids = ai.bench.folder.path().join("pids");
    ai.answer(
        "",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_FIXTURE", &fixture("claude-drift.jsonl")),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    let host = editor(window, cx);
    window
        .update(cx, |_, window, cx| {
            start(
                &host,
                Invocation {
                    args: "tighten".into(),
                    rows: None,
                    verb: None,
                },
                window,
                cx,
            )
        })
        .unwrap();
    let running = started(&pids, cx);
    drop(host);
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    ended(&running, cx);
}

#[gpui::test]
async fn failures_show_in_the_block_and_commands_refuse_while_off(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- one\n", cx);
    ai.answer(
        "",
        &[
            ("FAKE_AI_FIXTURE", &fixture("claude-drift.jsonl")),
            ("FAKE_AI_EXIT", "3"),
            ("FAKE_AI_STDERR", "boom happened"),
        ],
        cx,
    );
    let host = editor(window, cx);
    let check = Invocation {
        args: String::new(),
        rows: None,
        verb: Some(Verb::Check),
    };
    window
        .update(cx, |_, window, cx| start(&host, check.clone(), window, cx))
        .unwrap();
    wait_until(cx, "the failure", |cx| {
        shown(window, cx).contains("exited with code 3")
    });
    assert_eq!(
        shown(window, cx),
        "Claude Code exited with code 3: boom happened. Retry, or run it in a terminal to see the full error."
    );
    keys(window, "escape", cx);
    assert!(proposal(window, cx).is_none());
    cx.update(|cx| settings::set_enabled(false, cx)).unwrap();
    cx.run_until_parked();
    window
        .update(cx, |_, window, cx| start(&host, check, window, cx))
        .unwrap();
    assert_eq!(notice(cx), AI_OFF);
    assert!(proposal(window, cx).is_none());
}

#[gpui::test]
async fn switching_to_codex_runs_the_codex_protocol(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    cx.update(|cx| {
        settings::set_backend(Backend::Codex, cx);
    });
    cx.run_until_parked();
    let fake = crate::ai::process::tests::fake_cli()
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| settings::set_binary(Backend::Codex, &fake, cx))
        .unwrap();
    cx.run_until_parked();
    let codex = ai.bench.host("codex-transform.jsonl", &[]);
    cx.update(|cx| cx.set_global(codex));
    let tested = cx.update(settings::test);
    let result = std::rc::Rc::new(std::cell::RefCell::new(None));
    cx.spawn({
        let result = result.clone();
        async move |_| *result.borrow_mut() = Some(tested.await)
    })
    .detach();
    wait_until(cx, "the Codex test", |_| result.borrow().is_some());
    assert_eq!(result.take(), Some(Ok("Codex 2.1.312 works.".into())));
    cx.run_until_parked();
    let window = page("- apples are tasty\n", cx);
    let host = editor(window, cx);
    window
        .update(cx, |_, window, cx| {
            start(
                &host,
                Invocation {
                    args: String::new(),
                    rows: None,
                    verb: Some(Verb::Tighten),
                },
                window,
                cx,
            )
        })
        .unwrap();
    wait_ready(window, cx);
    assert_eq!(
        diff_text(window, cx),
        "- apples are tasty\nApples are good and tasty."
    );
    let call = ai.bench.calls().pop().unwrap();
    assert_eq!(call["argv"][0], "exec");
    assert!(
        call["stdin"]
            .as_str()
            .unwrap()
            .starts_with("You edit one range")
    );
}
