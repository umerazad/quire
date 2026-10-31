//! Ported from the stdio half of the reference `tests/mcp-server.test.ts`: the server runs as a
//! native process (`examples/quire_mcp.rs`, the code `quire --mcp-server` runs) against a bridge
//! the test app serves.

use super::bridge::{NotebookBridge, tests::options};
use super::stdio::CLOSED_APP;
use gpui::TestAppContext;
use serde_json::{Value, json};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

pub(crate) fn server() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let server = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("examples")
        .join("quire_mcp");
    assert!(
        server.exists(),
        "The MCP server example is missing at {}. Run the tests through cargo test, which builds examples.",
        server.display()
    );
    server
}

pub(crate) fn frame(id: Option<i64>, method: &str, params: Option<Value>) -> String {
    let mut frame = json!({ "jsonrpc": "2.0" });
    if let Some(id) = id {
        frame["id"] = id.into();
    }
    frame["method"] = method.into();
    if let Some(params) = params {
        frame["params"] = params;
    }
    frame.to_string()
}

pub(crate) struct Ran {
    pub code: Option<i32>,
    pub replies: Vec<Value>,
    pub stderr: String,
}

/// Runs the server with `lines` on standard input, then closes it, driving the app until the
/// server exits.
pub(crate) fn run(
    lines: &[String],
    bridge_file: &Path,
    env: &[(&str, &str)],
    cx: &mut TestAppContext,
) -> Ran {
    cx.executor().allow_parking();
    let mut child = Command::new(server())
        .env("QUIRE_BRIDGE", bridge_file)
        .env_remove("QUIRE_RUN")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut input = child.stdin.take().unwrap();
    input
        .write_all(format!("{}\n", lines.join("\n")).as_bytes())
        .unwrap();
    drop(input);
    let deadline = Instant::now() + crate::test_patience::PATIENCE;
    while child.try_wait().unwrap().is_none() {
        cx.run_until_parked();
        assert!(Instant::now() < deadline, "the MCP server did not exit");
        std::thread::sleep(Duration::from_millis(5));
    }
    let output = child.wait_with_output().unwrap();
    let mut replies: Vec<Value> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    replies.sort_by_key(|reply| reply["id"].as_i64().unwrap_or(-1));
    Ran {
        code: output.status.code(),
        replies,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    }
}

fn text(reply: &Value) -> Value {
    serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}

#[gpui::test]
fn serves_the_tools_from_the_running_app_and_exits_0_on_stdin_close(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, switches) = options(folder.path(), |call| {
        Some(Ok(
            json!({ "summary": format!("{} for {:?}", call.name, call.run) }),
        ))
    });
    let bridge = cx.update(|cx| NotebookBridge::start(options, cx)).unwrap();
    let ran = run(
        &[
            frame(
                Some(1),
                "initialize",
                Some(json!({
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": { "name": "t", "version": "1" },
                })),
            ),
            frame(None, "notifications/initialized", None),
            frame(Some(2), "tools/list", None),
            frame(
                Some(3),
                "tools/call",
                Some(json!({ "name": "tasks", "arguments": { "due": "today" } })),
            ),
        ],
        &bridge.file,
        &[("QUIRE_RUN", "ask-9")],
        cx,
    );
    assert_eq!(ran.stderr, "");
    assert_eq!(ran.code, Some(0));
    let ids: Vec<i64> = ran
        .replies
        .iter()
        .map(|reply| reply["id"].as_i64().unwrap())
        .collect();
    assert_eq!(ids, [1, 2, 3]);
    assert_eq!(
        ran.replies[0]["result"]["serverInfo"],
        json!({ "name": "quire", "version": "test" })
    );
    assert_eq!(
        ran.replies[1]["result"]["tools"].as_array().unwrap().len(),
        14
    );
    assert_eq!(
        text(&ran.replies[2]),
        json!({ "summary": "tasks for Some(\"ask-9\")" })
    );
    assert_eq!(switches.calls.borrow()[0].input, json!({ "due": "today" }));
}

#[gpui::test]
fn tells_the_model_to_open_the_app_when_it_is_closed(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let ran = run(
        &[
            frame(Some(1), "tools/list", None),
            frame(
                Some(2),
                "tools/call",
                Some(json!({ "name": "tasks", "arguments": {} })),
            ),
        ],
        &folder.path().join("missing").join("bridge.json"),
        &[],
        cx,
    );
    assert_eq!(ran.code, Some(0));
    assert_eq!(
        ran.replies[0]["result"]["tools"].as_array().unwrap().len(),
        14
    );
    assert_eq!(
        ran.replies[1]["result"],
        json!({ "content": [{ "type": "text", "text": CLOSED_APP }], "isError": true })
    );
}

#[gpui::test]
fn reports_a_stale_token_and_survives_a_malformed_frame(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |_| Some(Ok(json!({ "summary": "x" }))));
    let bridge = cx.update(|cx| NotebookBridge::start(options, cx)).unwrap();
    let mut current: Value = serde_json::from_slice(&std::fs::read(&bridge.file).unwrap()).unwrap();
    current["token"] = "0".repeat(64).into();
    let stale = folder.path().join("stale.json");
    std::fs::write(&stale, current.to_string()).unwrap();
    let ran = run(
        &[
            "not json".into(),
            frame(
                Some(2),
                "tools/call",
                Some(json!({ "name": "tasks", "arguments": {} })),
            ),
        ],
        &stale,
        &[],
        cx,
    );
    assert_eq!(ran.code, Some(0));
    assert_eq!(ran.replies[0]["id"], Value::Null);
    assert_eq!(ran.replies[0]["error"]["code"], -32700);
    assert!(
        ran.replies[1]["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("token did not match")
    );
}

#[gpui::test]
fn reports_the_app_closed_once_its_bridge_stops(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |_| Some(Ok(json!({ "summary": "x" }))));
    let bridge = cx.update(|cx| NotebookBridge::start(options, cx)).unwrap();
    let file = bridge.file.clone();
    let open = run(
        &[frame(
            Some(1),
            "tools/call",
            Some(json!({ "name": "tasks", "arguments": {} })),
        )],
        &file,
        &[],
        cx,
    );
    assert_eq!(text(&open.replies[0]), json!({ "summary": "x" }));
    drop(bridge);
    let closed = run(
        &[frame(
            Some(2),
            "tools/call",
            Some(json!({ "name": "tasks", "arguments": {} })),
        )],
        &file,
        &[],
        cx,
    );
    assert_eq!(
        closed.replies[0]["result"]["content"][0]["text"],
        CLOSED_APP
    );
}

/// A terminal session against the real tool host and fixture notebook: reads answer from the
/// live notebook, and a write waits for review without touching the page.
#[gpui::test]
fn a_terminal_session_reads_the_live_notebook_and_stages_writes(cx: &mut TestAppContext) {
    use super::{changes, read_tests::Bench};
    let bench = Bench::new(&[], cx);
    let folder = tempfile::tempdir().unwrap();
    let options = super::bridge::Options {
        directory: folder.path().to_owned(),
        enabled: std::rc::Rc::new(|_| true),
        writes: std::rc::Rc::new(|_| quire_core::ai::tools::Writes::Review),
        notebook_open: std::rc::Rc::new(|cx| super::live::current(cx).is_some()),
        forward: std::rc::Rc::new(super::host::answer),
        timeout: super::bridge::CALL_TIMEOUT,
    };
    let bridge = cx.update(|cx| NotebookBridge::start(options, cx)).unwrap();
    let ran = run(
        &[
            frame(
                Some(1),
                "tools/call",
                Some(json!({ "name": "tasks", "arguments": { "due": "today" } })),
            ),
            frame(
                Some(2),
                "tools/call",
                Some(json!({
                    "name": "append_block",
                    "arguments": { "page": "Inbox", "text": "From the terminal" },
                })),
            ),
            frame(
                Some(3),
                "tools/call",
                Some(json!({ "name": "read_page", "arguments": { "page": "Nope" } })),
            ),
        ],
        &bridge.file,
        &[],
        cx,
    );
    assert_eq!(ran.code, Some(0));
    assert_eq!(text(&ran.replies[0])["summary"], "Tasks due today · 2");
    let staged = text(&ran.replies[1]);
    assert_eq!(staged["status"], "awaiting review");
    assert_eq!(staged["change_id"], "change-1");
    assert_eq!(
        ran.replies[2]["result"],
        json!({
            "content": [{ "type": "text", "text": "No page named \"Nope\". Use list_pages or search_notebook to find its title." }],
            "isError": true,
        })
    );
    let listed = cx.update(|cx| changes::list(Some(changes::TERMINAL_RUN), cx));
    assert_eq!(listed.len(), 1);
    assert_eq!(
        bench
            .call("read_page", json!({ "page": "Inbox" }), cx)
            .unwrap()["text"],
        "- [ ] Sort the receipts\n"
    );
}
