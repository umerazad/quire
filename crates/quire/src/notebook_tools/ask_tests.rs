//! Ask runs through the notebook tools (reference `tests/ai-runner.test.ts`, the tool server
//! cases): the fake CLI starts the real MCP server from the run's own arguments, and each call
//! reaches the live fixture notebook over the bridge.

use super::{
    bridge::{CALL_TIMEOUT, NotebookBridge, Options},
    changes,
    read_tests::{Bench, finish},
    stdio_tests::server,
};
use crate::ai::process::{
    ASK_TIMEOUT, Mode, Outcome, Request, Update,
    tests::{Bench as Cli, fake_cli},
};
use gpui::TestAppContext;
use quire_core::ai::{Backend, backend::ToolServer, tools::Writes};
use serde_json::{Value, json};
use std::{
    rc::Rc,
    sync::{Arc, Mutex},
};

pub(crate) struct Ran {
    pub updates: Vec<Update>,
    pub outcome: Outcome,
    /// Each call the fake CLI made: name, arguments, the text it got back, and whether it failed.
    pub tools: Vec<Value>,
}

pub(crate) fn bridge(
    writes: Writes,
    folder: &std::path::Path,
    cx: &mut TestAppContext,
) -> NotebookBridge {
    let options = Options {
        directory: folder.to_owned(),
        enabled: Rc::new(|_| true),
        writes: Rc::new(move |_| writes),
        notebook_open: Rc::new(|cx| super::live::current(cx).is_some()),
        forward: Rc::new(super::host::answer),
        timeout: CALL_TIMEOUT,
    };
    cx.update(|cx| NotebookBridge::start(options, cx)).unwrap()
}

/// Runs one question whose CLI makes `calls` through the tool server of run `run`.
pub(crate) fn ask(
    calls: Value,
    run: &str,
    writes: Writes,
    bridge: &NotebookBridge,
    cx: &mut TestAppContext,
) -> Ran {
    let cli = Cli::new();
    let calls_file = cli.folder.path().join("calls.json");
    std::fs::write(&calls_file, calls.to_string()).unwrap();
    let tool_log = cli.folder.path().join("tools.json");
    let host = cli.host(
        "claude-transform.jsonl",
        &[
            ("FAKE_AI_TOOLS", &calls_file.to_string_lossy()),
            ("FAKE_AI_TOOL_LOG", &tool_log.to_string_lossy()),
        ],
    );
    let request = Request {
        backend: Backend::Claude,
        mode: Mode::Ask {
            today: "2026-10-28".into(),
            server: ToolServer {
                command: server().to_string_lossy().into_owned(),
                args: vec![super::SERVER_FLAG.into()],
                bridge: bridge.file.to_string_lossy().into_owned(),
                run: run.into(),
                writes,
            },
        },
        binary: fake_cli().to_string_lossy().into_owned(),
        model: String::new(),
        prompt: "What is due?".into(),
        timeout: ASK_TIMEOUT,
        transcript: None,
    };
    let updates = Arc::new(Mutex::new(Vec::new()));
    let task = cx.executor().spawn({
        let updates = updates.clone();
        async move {
            host.run(request, futures::future::pending(), move |update| {
                updates.lock().unwrap().push(update)
            })
            .await
        }
    });
    let outcome = finish(task, cx);
    let tools = serde_json::from_str(&std::fs::read_to_string(tool_log).unwrap()).unwrap();
    let updates = updates.lock().unwrap().clone();
    Ran {
        updates,
        outcome,
        tools,
    }
}

fn summary(text: &Value) -> Value {
    serde_json::from_str::<Value>(text.as_str().unwrap()).unwrap()["summary"].clone()
}

#[gpui::test]
fn a_question_calls_the_live_notebook_and_stages_its_writes_under_its_run(cx: &mut TestAppContext) {
    let bench = Bench::new(&[], cx);
    let folder = tempfile::tempdir().unwrap();
    let bridge = bridge(Writes::Review, folder.path(), cx);
    let ran = ask(
        json!([
            { "name": "tasks", "arguments": { "due": "today" } },
            { "name": "append_block", "arguments": { "page": "Inbox", "text": "From the run" } },
            { "name": "read_page", "arguments": { "page": "Nope" } },
        ]),
        "ask-r1",
        Writes::Review,
        &bridge,
        cx,
    );
    assert_eq!(
        ran.outcome,
        Outcome::Done("Apples are good and tasty.".into())
    );
    let names: Vec<&str> = ran
        .tools
        .iter()
        .map(|call| call["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["tasks", "append_block", "read_page"]);
    assert_eq!(summary(&ran.tools[0]["text"]), "Tasks due today · 2");
    let staged: Value = serde_json::from_str(ran.tools[1]["text"].as_str().unwrap()).unwrap();
    assert_eq!(staged["status"], "awaiting review");
    assert_eq!(staged["change_id"], "change-1");
    assert_eq!(
        ran.tools[2],
        json!({
            "name": "read_page",
            "arguments": { "page": "Nope" },
            "text": "No page named \"Nope\". Use list_pages or search_notebook to find its title.",
            "error": true,
        })
    );
    let rows: Vec<Update> = ran
        .updates
        .into_iter()
        .filter(|update| !matches!(update, Update::Token(_)))
        .collect();
    assert_eq!(
        rows,
        [
            Update::Tool {
                id: Some("toolu_0".into()),
                name: "tasks".into(),
                detail: "today".into()
            },
            Update::ToolDone {
                id: Some("toolu_0".into()),
                summary: "Tasks due today · 2".into(),
                failed: false
            },
            Update::Tool {
                id: Some("toolu_1".into()),
                name: "append_block".into(),
                detail: "Inbox".into()
            },
            Update::ToolDone {
                id: Some("toolu_1".into()),
                summary: "Append to [[Inbox]]".into(),
                failed: false
            },
            Update::Tool {
                id: Some("toolu_2".into()),
                name: "read_page".into(),
                detail: "Nope".into()
            },
            Update::ToolDone {
                id: Some("toolu_2".into()),
                summary:
                    "No page named \"Nope\". Use list_pages or search_notebook to find its title."
                        .into(),
                failed: true
            },
        ]
    );
    let listed = cx.update(|cx| changes::list(Some("ask-r1"), cx));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "change-1");
    assert_eq!(listed[0].request.texts(), ("", "- From the run"));
    assert!(
        cx.update(|cx| changes::list(Some(changes::TERMINAL_RUN), cx))
            .is_empty()
    );
    assert_eq!(
        bench
            .call("read_page", json!({ "page": "Inbox" }), cx)
            .unwrap()["text"],
        "- [ ] Sort the receipts\n"
    );
}

#[gpui::test]
fn write_tools_are_not_allowed_while_ai_writes_is_off(cx: &mut TestAppContext) {
    let _bench = Bench::new(&[], cx);
    let folder = tempfile::tempdir().unwrap();
    let bridge = bridge(Writes::Off, folder.path(), cx);
    let ran = ask(
        json!([{ "name": "append_block", "arguments": { "page": "Inbox", "text": "No" } }]),
        "ask-r2",
        Writes::Off,
        &bridge,
        cx,
    );
    assert_eq!(
        Value::Array(ran.tools),
        json!([{
            "name": "append_block",
            "arguments": { "page": "Inbox", "text": "No" },
            "text": "Claude requested permissions to use mcp__quire__append_block, but you haven't granted it yet.",
            "error": true,
        }])
    );
    assert!(cx.update(|cx| changes::list(None, cx)).is_empty());
}

#[gpui::test]
fn questions_need_the_tool_server_running(cx: &mut TestAppContext) {
    let _bench = Bench::new(&[], cx);
    assert_eq!(
        cx.update(|cx| super::ask_server("ask-r3", cx)),
        Err(quire_core::ai::backend::NO_TOOL_SERVER),
        "AI commands are off here, so the bridge is not served"
    );
}
