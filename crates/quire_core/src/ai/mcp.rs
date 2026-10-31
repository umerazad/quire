//! The Model Context Protocol (MCP) subset both CLIs use, ported from the reference
//! `mcp-protocol.ts`: JSON-RPC 2.0 over newline-delimited standard input and output with
//! `initialize`, `ping`, `tools/list`, and `tools/call`. The server forwards every call to the
//! running app; this module answers one frame at a time.

use super::tools::{Writes, listed_tools};
use serde_json::{Value, json};

pub const PROTOCOL_VERSION: &str = "2025-06-18";
const SUPPORTED: [&str; 3] = [PROTOCOL_VERSION, "2025-03-26", "2024-11-05"];
pub const INSTRUCTIONS: &str = "Tools over the Quire notebook open in the app. Pages are named by title; \"today\" is today's daily page. Block ids (^id) are stable: cite blocks by them. Use tasks, tagged_blocks and mentions for structured questions and search_notebook for words. Write tools may wait for the writer's review in Quire.";
pub const SERVER_NAME: &str = "quire";

/// What the server asks of the running app.
pub trait Bridge {
    /// The tools to list; an error lists the whole catalogue.
    fn list(&self) -> Result<Value, String>;
    /// One tool call: the result object, or the message the model reads.
    fn call(&self, name: &str, input: Value) -> Result<Value, String>;
}

fn reply(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

fn id_of(value: Option<&Value>) -> Value {
    match value {
        Some(id @ (Value::String(_) | Value::Number(_))) => id.clone(),
        _ => Value::Null,
    }
}

fn answer(message: &Value, bridge: &dyn Bridge, version: &str) -> Option<Value> {
    let method = message.get("method").and_then(Value::as_str);
    let (Some(object), Some(method)) = (message.as_object(), method) else {
        return Some(failure(
            id_of(message.get("id")),
            -32600,
            "Invalid request: send one JSON-RPC 2.0 object with a method.",
        ));
    };
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Some(failure(
            id_of(object.get("id")),
            -32600,
            "Invalid request: send one JSON-RPC 2.0 object with a method.",
        ));
    }
    if !object.contains_key("id") {
        return None;
    }
    let id = id_of(object.get("id"));
    let params = object.get("params").filter(|params| !params.is_null());
    let param = |name: &str| params.and_then(|params| params.get(name));
    Some(match method {
        "initialize" => {
            let asked = param("protocolVersion").and_then(Value::as_str);
            let version_used = asked
                .filter(|asked| SUPPORTED.contains(asked))
                .unwrap_or(PROTOCOL_VERSION);
            reply(
                id,
                json!({
                    "protocolVersion": version_used,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": SERVER_NAME, "version": version },
                    "instructions": INSTRUCTIONS,
                }),
            )
        }
        "ping" => reply(id, json!({})),
        "tools/list" => {
            let tools = bridge
                .list()
                .unwrap_or_else(|_| listed_tools(Writes::Review));
            reply(id, json!({ "tools": tools }))
        }
        "tools/call" => {
            let Some(name) = param("name").and_then(Value::as_str) else {
                return Some(failure(id, -32602, "tools/call needs a tool name."));
            };
            let input = param("arguments")
                .filter(|input| !input.is_null())
                .cloned()
                .unwrap_or_else(|| json!({}));
            let result = match bridge.call(name, input) {
                Ok(result) => json!({
                    "content": [{ "type": "text", "text": result.to_string() }],
                    "isError": false,
                }),
                Err(error) => json!({
                    "content": [{ "type": "text", "text": error }],
                    "isError": true,
                }),
            };
            reply(id, result)
        }
        other => failure(id, -32601, &format!("Method {other} is not supported.")),
    })
}

/// Answers one line: the reply line, or None for a notification.
pub fn answer_line(line: &str, bridge: &dyn Bridge, version: &str) -> Option<String> {
    let reply = match serde_json::from_str::<Value>(line) {
        Ok(message) => answer(&message, bridge, version)?,
        Err(_) => failure(
            Value::Null,
            -32700,
            "Parse error: each line must be one JSON object.",
        ),
    };
    Some(reply.to_string())
}
