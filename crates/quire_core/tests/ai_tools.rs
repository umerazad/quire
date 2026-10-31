//! Ported from the reference `tests/notebook-tools.test.ts` (catalogue and input checks) and the
//! protocol half of `tests/mcp-server.test.ts`. `fixtures/ai/tools-list.json` is the reference
//! `listedTools("review")` output, so the catalogue is compared byte for byte.

use quire_core::ai::{
    mcp::{Bridge, PROTOCOL_VERSION, answer_line},
    tools::{
        READ_TOOLS, WRITE_TOOLS, Writes, as_block, block_key, check_input, compact, line_at,
        listed_tools, outside_pages, take, tool_specs, validate_input,
    },
};
use serde_json::{Value, json};

#[test]
fn catalogue_matches_the_reference_byte_for_byte() {
    let reference: Value =
        serde_json::from_str(include_str!("fixtures/ai/tools-list.json")).unwrap();
    assert_eq!(
        listed_tools(Writes::Review).to_string(),
        reference.to_string()
    );
    assert_eq!(
        listed_tools(Writes::Apply).to_string(),
        reference.to_string()
    );
    let off: Vec<Value> = reference
        .as_array()
        .unwrap()
        .iter()
        .filter(|tool| READ_TOOLS.contains(&tool["name"].as_str().unwrap()))
        .cloned()
        .collect();
    assert_eq!(
        listed_tools(Writes::Off).to_string(),
        Value::Array(off).to_string()
    );
}

#[test]
fn lists_nine_read_tools_and_five_write_tools_with_object_schemas() {
    let specs = tool_specs();
    let reads: Vec<&str> = specs.iter().filter(|s| !s.write).map(|s| s.name).collect();
    let writes: Vec<&str> = specs.iter().filter(|s| s.write).map(|s| s.name).collect();
    assert_eq!(reads, READ_TOOLS);
    assert_eq!(writes, WRITE_TOOLS);
    for spec in specs {
        assert_eq!(spec.input_schema["type"], "object");
        assert!(spec.description.len() > 30);
    }
}

#[test]
fn validates_input_against_the_schema() {
    let search = &tool_specs()[0].input_schema;
    assert_eq!(
        validate_input(search, &json!({ "query": "x" }), "input").as_deref(),
        Some("input.query must have at least 2 characters.")
    );
    let failure = |name: &str, input: Value| check_input(name, &input).unwrap_err();
    assert_eq!(
        failure("tasks", json!({ "due": "tomorrow" })),
        "input.due must be one of today, overdue, week, range. Check the tasks input schema."
    );
    assert!(
        failure("read_page", json!({ "page": "A", "extra": 1 }))
            .contains("input.extra is not a known field.")
    );
    assert!(failure("list_pages", json!({ "limit": 500 })).contains("at most 200"));
    assert!(
        failure("nope", json!({}))
            .starts_with("Unknown tool nope. Available tools: search_notebook,")
    );
    assert_eq!(
        failure("read_page", json!({})),
        "input.page is required. Check the read_page input schema."
    );
    assert_eq!(
        failure("list_pages", json!({ "daily_from": "yesterday" })),
        "input.daily_from has the wrong format. Earliest daily date. As YYYY-MM-DD or \"today\". Check the list_pages input schema."
    );
    for date in ["2026-10-28", "today"] {
        assert!(check_input("daily_pages", &json!({ "from": date, "to": date })).is_ok());
    }
    assert_eq!(
        failure("read_page", json!({ "page": "A", "from_line": 1.5 })),
        "input.from_line must be a whole number. Check the read_page input schema."
    );
    assert!(check_input("read_page", &json!({ "page": "A", "from_line": 2.0 })).is_ok());
    assert_eq!(
        failure("read_page", json!({ "page": 3 })),
        "input.page must be text. Check the read_page input schema."
    );
    assert_eq!(
        failure("tasks", json!([])),
        "input must be an object. Check the tasks input schema."
    );
    assert_eq!(
        failure("read_page", json!({ "page": "A", "constructor": 1 })),
        "input.constructor is not a known field. Check the read_page input schema."
    );
    let long = "🙂".repeat(101);
    assert!(
        failure("read_block", json!({ "id": long })).contains("at most 100 characters"),
        "lengths count UTF-16 units, as the reference does"
    );
}

#[test]
fn block_keys_match_the_reference_hash() {
    let text = "- one\n- Gödel 🙂 two\n";
    assert_eq!(block_key(line_at(text, 2), "one"), "L1-1fnfegf");
    assert_eq!(block_key(line_at(text, 8), "Gödel 🙂 two"), "L2-ysg67i");
    assert_eq!(block_key(line_at("", 0), ""), "L1-ztntfp");
}

#[test]
fn diffs_lines_as_the_reference_does() {
    use quire_core::ai::tools::{LineChange::*, diff_lines};
    assert_eq!(
        diff_lines("a\nb\nc", "a\nB\nc\nd"),
        [
            (Same, "a"),
            (Remove, "b"),
            (Add, "B"),
            (Same, "c"),
            (Add, "d")
        ]
    );
    assert_eq!(diff_lines("", "- x"), [(Remove, ""), (Add, "- x")]);
}

#[test]
fn replacement_text_drops_only_the_blocks_own_marker() {
    use quire_core::ai::tools::without_marker;
    assert_eq!(without_marker("fine ^d14 ", "d14"), "fine");
    assert_eq!(without_marker("fine^d14", "d14"), "fine");
    assert_eq!(without_marker("fine ^d15", "d14"), "fine ^d15");
    assert_eq!(without_marker("a ^d14 b", "d14"), "a ^d14 b");
}

#[test]
fn new_blocks_become_list_items() {
    assert_eq!(as_block("Notes\nmore"), "- Notes\n  more");
    assert_eq!(as_block("  - already\n  - two "), "- already\n  - two");
    assert_eq!(as_block("1. first"), "1. first");
    assert_eq!(as_block("a\n\nb"), "- a\n\n  b");
}

#[test]
fn refuses_targets_outside_the_notebook_pages() {
    for page in [
        "/etc/passwd",
        "../secret.md",
        ".quire/ai/x.md",
        "C:\\x.md",
        ".Quire",
    ] {
        assert!(
            outside_pages(page).is_some_and(|error| error.contains("outside the notebook's pages")),
            "{page}"
        );
    }
    for page in ["Projects/Plan.md", ".quirenotes", "a..b"] {
        assert_eq!(outside_pages(page), None, "{page}");
    }
}

#[test]
fn caps_lists_and_says_so() {
    let (items, note) = take((0..250).collect::<Vec<_>>(), Some(200));
    assert_eq!(items.len(), 200);
    assert_eq!(
        note.as_deref(),
        Some("Showing 200 of 250. Narrow the request or raise limit (at most 200).")
    );
    let (items, note) = take((0..60).collect::<Vec<_>>(), None);
    assert_eq!((items.len(), note.is_some()), (50, true));
    assert_eq!(take(vec![1], Some(5)), (vec![1], None));
    assert_eq!(
        compact(json!({ "a": null, "b": [{ "c": null, "d": 1 }] })),
        json!({ "b": [{ "d": 1 }] })
    );
}

struct Fake {
    reachable: bool,
}

impl Bridge for Fake {
    fn list(&self) -> Result<Value, String> {
        if self.reachable {
            Ok(
                json!([{ "name": "tasks", "description": "d", "inputSchema": { "type": "object" } }]),
            )
        } else {
            Err("closed".into())
        }
    }

    fn call(&self, name: &str, _: Value) -> Result<Value, String> {
        if name == "tasks" {
            Ok(json!({ "summary": "Tasks · 0", "tasks": [] }))
        } else {
            Err("nope".into())
        }
    }
}

fn frame(id: Option<i64>, method: &str, params: Option<Value>) -> String {
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

fn answer(line: &str) -> String {
    answer_line(line, &Fake { reachable: true }, "1.2.3").unwrap()
}

#[test]
fn negotiates_the_protocol_version() {
    assert_eq!(
        answer(&frame(
            Some(1),
            "initialize",
            Some(json!({ "protocolVersion": PROTOCOL_VERSION }))
        )),
        r#"{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{"listChanged":false}},"serverInfo":{"name":"quire","version":"1.2.3"},"instructions":"Tools over the Quire notebook open in the app. Pages are named by title; \"today\" is today's daily page. Block ids (^id) are stable: cite blocks by them. Use tasks, tagged_blocks and mentions for structured questions and search_notebook for words. Write tools may wait for the writer's review in Quire."}}"#
    );
    let version = |asked: &str| {
        let reply: Value = serde_json::from_str(&answer(&frame(
            Some(2),
            "initialize",
            Some(json!({ "protocolVersion": asked })),
        )))
        .unwrap();
        reply["result"]["protocolVersion"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(version("2025-03-26"), "2025-03-26");
    assert_eq!(version("1999-01-01"), PROTOCOL_VERSION);
}

#[test]
fn answers_ping_ignores_notifications_and_rejects_unknown_methods() {
    assert_eq!(
        answer(&frame(Some(4), "ping", None)),
        r#"{"jsonrpc":"2.0","id":4,"result":{}}"#
    );
    assert_eq!(
        answer_line(
            &frame(None, "notifications/initialized", None),
            &Fake { reachable: true },
            "1"
        ),
        None
    );
    assert_eq!(
        answer(&frame(Some(5), "resources/list", None)),
        r#"{"jsonrpc":"2.0","id":5,"error":{"code":-32601,"message":"Method resources/list is not supported."}}"#
    );
    assert_eq!(
        answer("{oops"),
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":"Parse error: each line must be one JSON object."}}"#
    );
    assert_eq!(
        answer("[1,2]"),
        r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32600,"message":"Invalid request: send one JSON-RPC 2.0 object with a method."}}"#
    );
    assert_eq!(
        answer(r#"{"jsonrpc":"1.0","id":"x","method":"ping"}"#),
        r#"{"jsonrpc":"2.0","id":"x","error":{"code":-32600,"message":"Invalid request: send one JSON-RPC 2.0 object with a method."}}"#
    );
    assert_eq!(
        answer(&frame(Some(6), "tools/call", Some(json!({})))),
        r#"{"jsonrpc":"2.0","id":6,"error":{"code":-32602,"message":"tools/call needs a tool name."}}"#
    );
}

#[test]
fn wraps_tool_results_and_errors_as_text_content() {
    assert_eq!(
        answer(&frame(
            Some(7),
            "tools/call",
            Some(json!({ "name": "tasks", "arguments": {} }))
        )),
        r#"{"jsonrpc":"2.0","id":7,"result":{"content":[{"type":"text","text":"{\"summary\":\"Tasks · 0\",\"tasks\":[]}"}],"isError":false}}"#
    );
    assert_eq!(
        answer(&frame(Some(8), "tools/call", Some(json!({ "name": "x" })))),
        r#"{"jsonrpc":"2.0","id":8,"result":{"content":[{"type":"text","text":"nope"}],"isError":true}}"#
    );
}

#[test]
fn lists_the_catalogue_when_the_app_cannot_be_reached() {
    let listed: Value = serde_json::from_str(
        &answer_line(
            &frame(Some(1), "tools/list", None),
            &Fake { reachable: false },
            "1",
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(listed["result"]["tools"].as_array().unwrap().len(), 14);
    let reachable: Value =
        serde_json::from_str(&answer(&frame(Some(2), "tools/list", None))).unwrap();
    assert_eq!(reachable["result"]["tools"][0]["name"], "tasks");
}
