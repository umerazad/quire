//! A stand-in for the Claude Code and Codex CLIs in tests, replaying a recorded event stream the
//! way the reference `tests/fixtures/ai/fake-cli.mjs` does. Environment variables shape the run:
//! FAKE_AI_FIXTURE names the stream, FAKE_AI_LOG records argv, stdin, and the working folder,
//! FAKE_AI_DELAY_MS waits between lines, FAKE_AI_STDERR and FAKE_AI_EXIT end the run,
//! FAKE_AI_HANG keeps running after the stream, FAKE_AI_VERSION answers `--version`, and
//! FAKE_AI_PIDS starts a grandchild and records both process IDs, so tests can check that a
//! cancel ends the whole process tree. FAKE_AI_WRITE ("name=content") writes a file in the working
//! folder, and FAKE_AI_RENDER names a Word or PDF file to make from its page.md, as an export
//! run's document tools would. FAKE_AI_TOOLS makes the calls it lists through the MCP server an
//! ask run attaches before the stream, as Claude Code does.

#[path = "fake_ai_cli/documents.rs"]
mod documents;

use std::{
    io::{Read, Write},
    time::Duration,
};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == name)?;
    args.get(at + 1).map(String::as_str)
}

/// Plays a Claude Code ask run with tools: starts the MCP server named by `--mcp-config`, makes
/// each call in the JSON file `calls` names (`[{name, arguments}]`) that `--allowedTools` permits,
/// streams each as a tool use and its result, and logs every call and result to
/// FAKE_AI_TOOL_LOG.
fn call_tools(args: &[String], calls: &str, stdout: &mut std::io::Stdout) {
    use serde_json::{Value, json};
    use std::io::BufRead as _;
    let config: Value = serde_json::from_str(flag(args, "--mcp-config").unwrap_or("{}"))
        .expect("--mcp-config is not JSON");
    let server = &config["mcpServers"]["quire"];
    let allowed: Vec<&str> = flag(args, "--allowedTools")
        .unwrap_or_default()
        .split(',')
        .collect();
    let calls: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string(calls).expect("FAKE_AI_TOOLS cannot be read"),
    )
    .expect("FAKE_AI_TOOLS is not a JSON list");
    let mut child = std::process::Command::new(
        server["command"]
            .as_str()
            .expect("--mcp-config names no server command"),
    )
    .args(
        server["args"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str),
    )
    .envs(
        server["env"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(key, value)| Some((key, value.as_str()?))),
    )
    .stdin(std::process::Stdio::piped())
    .stdout(std::process::Stdio::piped())
    .spawn()
    .expect("the MCP server could not start");
    let mut input = child.stdin.take().unwrap();
    let mut replies = std::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let mut ask = |id: usize, method: &str, params: Value| -> Value {
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(input, "{frame}").unwrap();
        let line = replies.next().expect("the MCP server closed").unwrap();
        serde_json::from_str(&line).unwrap()
    };
    ask(
        0,
        "initialize",
        json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "fake", "version": "1" } }),
    );
    let mut log = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        let name = call["name"].as_str().unwrap_or_default();
        let id = format!("toolu_{index}");
        let tool = format!("mcp__quire__{name}");
        let use_line = json!({ "type": "assistant", "message": { "content": [
            { "type": "tool_use", "id": id, "name": tool, "input": call["arguments"] }
        ] } });
        writeln!(stdout, "{use_line}").ok();
        let (text, error) = if allowed.contains(&tool.as_str()) {
            let reply = ask(
                index + 1,
                "tools/call",
                json!({ "name": name, "arguments": call["arguments"] }),
            );
            let result = &reply["result"];
            (
                result["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned(),
                result["isError"] == Value::Bool(true),
            )
        } else {
            (
                format!(
                    "Claude requested permissions to use {tool}, but you haven't granted it yet."
                ),
                true,
            )
        };
        let result_line = json!({ "type": "user", "message": { "content": [
            { "type": "tool_result", "tool_use_id": id, "content": [{ "type": "text", "text": text }], "is_error": error }
        ] } });
        writeln!(stdout, "{result_line}").ok();
        stdout.flush().ok();
        log.push(
            json!({ "name": name, "arguments": call["arguments"], "text": text, "error": error }),
        );
    }
    drop(input);
    child.wait().ok();
    if let Some(path) = var("FAKE_AI_TOOL_LOG") {
        std::fs::write(path, Value::Array(log).to_string()).ok();
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("--sleep") {
        std::thread::sleep(Duration::from_secs(60));
        return;
    }
    if args.iter().any(|arg| arg == "--version") {
        println!(
            "{}",
            var("FAKE_AI_VERSION").unwrap_or_else(|| "2.1.312 (Claude Code)".into())
        );
        return;
    }
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).ok();
    if let Some(log) = var("FAKE_AI_LOG") {
        let cwd = std::env::current_dir().unwrap_or_default();
        let line = serde_json::json!({ "argv": args, "stdin": input, "cwd": cwd });
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log)
            .expect("FAKE_AI_LOG names a file that cannot be opened");
        writeln!(file, "{line}").ok();
    }
    if let Some(pids) = var("FAKE_AI_PIDS") {
        let mut child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--sleep")
            .spawn()
            .expect("the fake CLI could not start its grandchild");
        std::fs::write(pids, format!("{} {}", std::process::id(), child.id())).ok();
        std::thread::spawn(move || child.wait());
    }
    if let Some((name, content)) = var("FAKE_AI_WRITE")
        .as_deref()
        .and_then(|write| write.split_once('='))
    {
        std::fs::write(name, content).expect("FAKE_AI_WRITE could not write its file");
    }
    if let Some(name) = var("FAKE_AI_RENDER") {
        documents::render(&name);
    }
    let delay = Duration::from_millis(
        var("FAKE_AI_DELAY_MS")
            .and_then(|delay| delay.parse().ok())
            .unwrap_or(0),
    );
    let stream = var("FAKE_AI_FIXTURE")
        .map(|path| std::fs::read_to_string(path).expect("FAKE_AI_FIXTURE cannot be read"))
        .unwrap_or_default();
    let mut stdout = std::io::stdout();
    if let Some(calls) = var("FAKE_AI_TOOLS") {
        call_tools(&args, &calls, &mut stdout);
    }
    for line in stream.lines().filter(|line| !line.is_empty()) {
        writeln!(stdout, "{line}").ok();
        stdout.flush().ok();
        std::thread::sleep(delay);
    }
    if var("FAKE_AI_HANG").is_some() {
        std::thread::sleep(Duration::from_secs(60));
    }
    if let Some(stderr) = var("FAKE_AI_STDERR") {
        eprint!("{stderr}");
    }
    std::process::exit(
        var("FAKE_AI_EXIT")
            .and_then(|code| code.parse().ok())
            .unwrap_or(0),
    );
}
