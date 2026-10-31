//! Ported from the reference `tests/notebook-bridge.test.ts`: a real socket and bridge file in a
//! temporary folder, clients on their own threads, and the app's answers faked per test.

use super::*;
use gpui::TestAppContext;
use std::{
    cell::Cell,
    io::{BufRead as _, BufReader, Write as _},
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
};

pub(crate) struct Switches {
    pub enabled: Rc<Cell<bool>>,
    pub writes: Rc<Cell<Writes>>,
    pub open: Rc<Cell<bool>>,
    pub calls: Rc<RefCell<Vec<Call>>>,
}

pub(crate) fn options(
    directory: &Path,
    forward: impl Fn(&Call) -> Option<Result<Value, String>> + 'static,
) -> (Options, Switches) {
    let switches = Switches {
        enabled: Rc::new(Cell::new(true)),
        writes: Rc::new(Cell::new(Writes::Review)),
        open: Rc::new(Cell::new(true)),
        calls: Rc::default(),
    };
    let (enabled, writes, open, calls) = (
        switches.enabled.clone(),
        switches.writes.clone(),
        switches.open.clone(),
        switches.calls.clone(),
    );
    let options = Options {
        directory: directory.to_owned(),
        enabled: Rc::new(move |_| enabled.get()),
        writes: Rc::new(move |_| writes.get()),
        notebook_open: Rc::new(move |_| open.get()),
        forward: Rc::new(move |call, cx| {
            calls.borrow_mut().push(call.clone());
            match forward(&call) {
                Some(answer) => Task::ready(answer),
                // A call the app never answers.
                None => cx.spawn(async |_| futures::future::pending().await),
            }
        }),
        timeout: CALL_TIMEOUT,
    };
    (options, switches)
}

/// Sends `lines` on one connection from another thread, and waits while the app answers until
/// `expected` replies arrive or the server closes the connection.
pub(crate) fn exchange(
    socket: &Path,
    lines: &[String],
    expected: usize,
    cx: &mut TestAppContext,
) -> Vec<Value> {
    cx.executor().allow_parking();
    let replies = Arc::new(Mutex::new(Vec::new()));
    let finished = Arc::new(Mutex::new(false));
    let client = {
        let (socket, lines) = (socket.to_owned(), lines.to_vec());
        let (replies, finished) = (replies.clone(), finished.clone());
        std::thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).unwrap();
            for line in lines {
                // The bridge may close a connection mid-request; its reply is still readable.
                if stream.write_all(format!("{line}\n").as_bytes()).is_err() {
                    break;
                }
            }
            let mut reader = BufReader::new(stream);
            for _ in 0..expected {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
                replies
                    .lock()
                    .unwrap()
                    .push(serde_json::from_str::<Value>(&line).unwrap());
            }
            *finished.lock().unwrap() = true;
        })
    };
    crate::test_wait::until(
        cx,
        Duration::from_millis(5),
        crate::test_wait::park,
        |_| *finished.lock().unwrap(),
        |_| "the bridge to answer".into(),
    );
    client.join().unwrap();
    replies.lock().unwrap().clone()
}

pub(crate) fn token_of(file: &Path) -> String {
    let bridge: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
    bridge["token"].as_str().unwrap().to_owned()
}

fn request(id: i64, token: &str, method: &str, params: Value) -> String {
    json!({ "id": id, "token": token, "method": method, "params": params }).to_string()
}

fn call_line(id: i64, token: &str, name: &str, arguments: Value) -> String {
    request(
        id,
        token,
        "tools/call",
        json!({ "name": name, "arguments": arguments }),
    )
}

fn start(options: Options, cx: &mut TestAppContext) -> NotebookBridge {
    cx.update(|cx| NotebookBridge::start(options, cx)).unwrap()
}

#[gpui::test]
fn writes_a_private_bridge_file_and_socket_and_removes_both_on_stop(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let saved: Value = serde_json::from_slice(&std::fs::read(&bridge.file).unwrap()).unwrap();
    assert_eq!(saved["version"], 1);
    assert_eq!(saved["socket"], bridge.socket.to_string_lossy().as_ref());
    assert_eq!(saved["pid"], std::process::id());
    assert!(
        saved["token"]
            .as_str()
            .is_some_and(|token| token.len() == 64 && hex::decode(token).is_ok())
    );
    use std::os::unix::fs::PermissionsExt as _;
    for path in [&bridge.file, &bridge.socket] {
        let mode = std::fs::metadata(path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "{}", path.display());
    }
    let (file, socket) = (bridge.file.clone(), bridge.socket.clone());
    drop(bridge);
    assert!(!file.exists() && !socket.exists());
}

#[gpui::test]
fn rejects_a_wrong_token_and_a_restarted_bridges_old_token(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (first, _) = options(folder.path(), |_| None);
    let bridge = start(first, cx);
    let old = token_of(&bridge.file);
    let wrong = exchange(
        &bridge.socket,
        &[request(1, &"0".repeat(64), "tools/list", json!({}))],
        1,
        cx,
    );
    assert_eq!(
        wrong,
        [json!({ "id": 1, "error": TOKEN_MISMATCH })],
        "a wrong token gets nothing but the fix"
    );
    drop(bridge);
    let (second, _) = options(folder.path(), |_| None);
    let bridge = start(second, cx);
    assert_ne!(token_of(&bridge.file), old);
    let stale = exchange(
        &bridge.socket,
        &[request(2, &old, "tools/list", json!({}))],
        1,
        cx,
    );
    assert_eq!(stale[0]["error"], TOKEN_MISMATCH);
    let fresh = exchange(
        &bridge.socket,
        &[request(3, &token_of(&bridge.file), "tools/list", json!({}))],
        1,
        cx,
    );
    assert_eq!(fresh[0]["result"]["tools"].as_array().unwrap().len(), 14);
}

#[gpui::test]
fn lists_tools_and_hides_write_tools_while_ai_writes_is_off(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, switches) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    switches.writes.set(Writes::Off);
    let listed = exchange(
        &bridge.socket,
        &[request(1, &token, "tools/list", json!({}))],
        1,
        cx,
    );
    let names: Vec<&str> = listed[0]["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, quire_core::ai::tools::READ_TOOLS);
}

#[gpui::test]
fn forwards_valid_calls_with_the_run_id_and_writes_setting(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, switches) = options(folder.path(), |call| {
        Some(Ok(json!({ "summary": format!("ran {}", call.name) })))
    });
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    switches.writes.set(Writes::Apply);
    let replies = exchange(
        &bridge.socket,
        &[
            request(
                1,
                &token,
                "tools/call",
                json!({ "name": "tasks", "arguments": { "due": "today" }, "run": "ask-1" }),
            ),
            request(
                2,
                &token,
                "tools/call",
                json!({ "name": "read_page", "arguments": { "page": "A" }, "run": "bad run!" }),
            ),
        ],
        2,
        cx,
    );
    let mut replies = replies;
    replies.sort_by_key(|reply| reply["id"].as_i64());
    assert_eq!(
        replies,
        [
            json!({ "id": 1, "result": { "ok": true, "result": { "summary": "ran tasks" } } }),
            json!({ "id": 2, "result": { "ok": true, "result": { "summary": "ran read_page" } } }),
        ]
    );
    let calls = switches.calls.borrow();
    assert_eq!(
        calls[0],
        Call {
            name: "tasks".into(),
            input: json!({ "due": "today" }),
            run: Some("ask-1".into()),
            writes: Writes::Apply,
        }
    );
    assert_eq!(calls[1].run, None, "a malformed run id is dropped");
}

#[gpui::test]
fn blocks_write_tools_while_ai_writes_is_off_before_the_app_sees_them(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, switches) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    switches.writes.set(Writes::Off);
    let replies = exchange(
        &bridge.socket,
        &[call_line(
            1,
            &token,
            "append_block",
            json!({ "page": "Inbox", "text": "x" }),
        )],
        1,
        cx,
    );
    assert_eq!(
        replies[0]["result"],
        json!({ "ok": false, "error": WRITES_OFF })
    );
    assert!(switches.calls.borrow().is_empty());
}

#[gpui::test]
fn validates_input_and_unknown_tools_before_the_app_sees_them(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, switches) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    let mut replies = exchange(
        &bridge.socket,
        &[
            call_line(1, &token, "tasks", json!({ "due": "tomorrow" })),
            call_line(2, &token, "nope", json!({})),
            request(3, &token, "tools/call", json!({})),
            request(4, &token, "resources/list", json!({})),
        ],
        4,
        cx,
    );
    replies.sort_by_key(|reply| reply["id"].as_i64());
    assert_eq!(
        replies,
        [
            json!({ "id": 1, "result": { "ok": false, "error": "input.due must be one of today, overdue, week, range. Check the tasks input schema." } }),
            json!({ "id": 2, "result": { "ok": false, "error": "Unknown tool nope. Call tools/list for the available tools." } }),
            json!({ "id": 3, "result": { "ok": false, "error": "Unknown tool (none). Call tools/list for the available tools." } }),
            json!({ "id": 4, "error": "Unknown bridge method resources/list. Update Quire so its tool server matches the app." }),
        ]
    );
    assert!(switches.calls.borrow().is_empty());
}

#[gpui::test]
fn names_the_fix_when_no_notebook_is_open_or_ai_commands_are_off(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, switches) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    switches.open.set(false);
    let closed = exchange(
        &bridge.socket,
        &[call_line(1, &token, "tasks", json!({}))],
        1,
        cx,
    );
    assert_eq!(closed[0]["result"]["error"], NO_NOTEBOOK);
    switches.enabled.set(false);
    let off = exchange(
        &bridge.socket,
        &[call_line(2, &token, "tasks", json!({}))],
        1,
        cx,
    );
    assert_eq!(off[0]["result"]["error"], AI_COMMANDS_OFF);
    assert!(switches.calls.borrow().is_empty());
}

#[gpui::test]
fn times_out_an_app_that_does_not_answer(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    cx.executor().allow_parking();
    let socket = bridge.socket.clone();
    let line = call_line(1, &token, "tasks", json!({}));
    let reply = Arc::new(Mutex::new(None));
    let client = {
        let reply = reply.clone();
        std::thread::spawn(move || {
            let mut stream = UnixStream::connect(socket).unwrap();
            writeln!(stream, "{line}").unwrap();
            let mut answer = String::new();
            BufReader::new(stream).read_line(&mut answer).unwrap();
            *reply.lock().unwrap() = Some(serde_json::from_str::<Value>(&answer).unwrap());
        })
    };
    crate::test_wait::until(
        cx,
        Duration::from_millis(5),
        |cx| {
            cx.run_until_parked();
            cx.executor().advance_clock(Duration::from_secs(1));
        },
        |_| reply.lock().unwrap().is_some(),
        |_| "the bridge to time out".into(),
    );
    client.join().unwrap();
    assert_eq!(
        reply.lock().unwrap().clone().unwrap()["result"]["error"],
        "Quire did not answer within 20 seconds. Make sure its window is open and the notebook has loaded, then retry."
    );
}

#[gpui::test]
fn answers_malformed_lines_without_closing_the_connection(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |_| Some(Ok(json!({ "summary": "ok" }))));
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    let replies = exchange(
        &bridge.socket,
        &["not json".into(), call_line(2, &token, "tasks", json!({}))],
        2,
        cx,
    );
    assert!(replies.contains(&json!({
        "id": null,
        "error": "The bridge request is not JSON. Update Quire so its tool server matches the app.",
    })));
    assert!(
        replies
            .contains(&json!({ "id": 2, "result": { "ok": true, "result": { "summary": "ok" } } }))
    );
}

#[gpui::test]
fn closes_a_connection_that_sends_over_a_megabyte_without_a_line_break(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |_| None);
    let bridge = start(options, cx);
    let replies = exchange(&bridge.socket, &["x".repeat(1_100_000)], 2, cx);
    assert_eq!(
        replies,
        [
            json!({ "id": null, "error": "The bridge request is over 1 MB. Send a smaller request." })
        ]
    );
}

#[gpui::test]
fn answers_concurrent_calls_as_each_finishes(cx: &mut TestAppContext) {
    let folder = tempfile::tempdir().unwrap();
    let (options, _) = options(folder.path(), |call| {
        (call.name != "daily_pages").then(|| Ok(json!({ "summary": call.name })))
    });
    let bridge = start(options, cx);
    let token = token_of(&bridge.file);
    let replies = exchange(
        &bridge.socket,
        &[
            call_line(
                1,
                &token,
                "daily_pages",
                json!({ "from": "today", "to": "today" }),
            ),
            call_line(2, &token, "tasks", json!({})),
        ],
        1,
        cx,
    );
    assert_eq!(
        replies,
        [json!({ "id": 2, "result": { "ok": true, "result": { "summary": "tasks" } } })],
        "a call waiting on the app does not hold up the next one"
    );
}
