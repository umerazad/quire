//! What Quire sends to each CLI and how it reads the answer (reference
//! `src/desktop/ai-runner.ts`): argument lists that never carry user text, the prompt sent on
//! standard input, one parser per JSON event stream, and the settings checks with their messages.

use regex::Regex;
use serde_json::Value;
use std::{collections::HashMap, sync::LazyLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Backend {
    #[default]
    Claude,
    Codex,
}

impl Backend {
    pub const ALL: [Backend; 2] = [Self::Claude, Self::Codex];

    pub fn name(self) -> &'static str {
        match self {
            Self::Claude => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    /// The binary's name on PATH.
    pub fn binary(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    /// The CLI version whose event stream the parser was written against.
    pub fn tested_version(self) -> &'static str {
        match self {
            Self::Claude => "2.1.312",
            Self::Codex => "0.164.0",
        }
    }

    pub fn login_help(self) -> &'static str {
        match self {
            Self::Claude => "Run `claude` in a terminal and sign in with /login, then retry.",
            Self::Codex => "Run `codex login` in a terminal, then retry.",
        }
    }

    /// What a transform can see, as Settings explains it.
    pub fn isolation(self) -> &'static str {
        match self {
            Self::Claude => {
                "Edits run in restricted mode with no tools, so the model receives the selected text, its parent blocks and Claude Code's own instructions."
            }
            Self::Codex => {
                "Edits run in Codex's read-only sandbox in an empty folder Quire makes for each run. The model is asked to use only the selected text; the sandbox still lets it read files elsewhere on this computer."
            }
        }
    }
}

/// The name of the notebook tool server, kept off in transforms.
pub const TOOL_SERVER: &str = "quire";

/// The argument list of a transform run in `cwd`. User text never appears here: the prompt goes
/// through standard input, and the model name is validated first.
pub fn transform_args(backend: Backend, cwd: &str, model: &str) -> Vec<String> {
    run_args(
        backend,
        &super::commands::transform_system_prompt(),
        cwd,
        model,
    )
}

/// The argument list of a run with `system` rules and no tools. Transforms and questions both
/// run this way; questions get their notebook context in the prompt.
pub fn run_args(backend: Backend, system: &str, cwd: &str, model: &str) -> Vec<String> {
    let model = model.trim();
    let args: Vec<&str> = match backend {
        Backend::Claude => vec![
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--restricted",
            "--strict-mcp-config",
            "--tools",
            "",
            "--max-turns",
            "1",
            "--no-session-persistence",
            "--append-system-prompt",
        ],
        Backend::Codex => vec![
            "exec",
            "--json",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "-c",
        ],
    };
    let mut args: Vec<String> = args.into_iter().map(str::to_owned).collect();
    match backend {
        Backend::Claude => {
            args.push(system.to_owned());
            if !model.is_empty() {
                args.extend(["--model".into(), model.into()]);
            }
        }
        Backend::Codex => {
            // A server registered for terminal use stays off in transforms.
            args.extend([
                format!("mcp_servers.{TOOL_SERVER}.command=\"quire-off\""),
                "-c".into(),
                format!("mcp_servers.{TOOL_SERVER}.enabled=false"),
                "-C".into(),
                cwd.into(),
            ]);
            if !model.is_empty() {
                args.extend(["-m".into(), model.into()]);
            }
            args.push("-".into());
        }
    }
    args
}

/// What goes to standard input for a transform.
pub fn backend_input(backend: Backend, prompt: &str) -> String {
    run_input(backend, &super::commands::transform_system_prompt(), prompt)
}

/// What goes to standard input. Codex has no system prompt flag, so its rules lead the prompt.
pub fn run_input(backend: Backend, system: &str, prompt: &str) -> String {
    match backend {
        Backend::Claude => prompt.to_owned(),
        Backend::Codex => format!("{system}\n\n{prompt}"),
    }
}

/// One step of a run, in the same shape for both CLIs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    Session(String),
    Token(String),
    Tool {
        id: Option<String>,
        name: String,
        detail: String,
        result: Option<String>,
        failed: bool,
    },
    Done {
        text: String,
        session: Option<String>,
    },
    Error(String),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Parsed {
    pub events: Vec<Event>,
    pub version: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct ParseState {
    answer: Option<String>,
    tools: HashMap<String, String>,
}

fn text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn tool_name(name: &Value) -> String {
    text(name).replace(&format!("mcp__{TOOL_SERVER}__"), "")
}

/// The input field that best names what a tool call is about.
fn tool_detail(input: &Value) -> String {
    const KEYS: [&str; 12] = [
        "file_path",
        "pattern",
        "path",
        "query",
        "page",
        "tag",
        "id",
        "title",
        "due",
        "state",
        "from",
        "text",
    ];
    KEYS.iter()
        .find_map(|key| input.get(key)?.as_str().filter(|value| !value.is_empty()))
        .map(|value| value.chars().take(120).collect())
        .unwrap_or_default()
}

/// A tool result's summary line, or the first line of an error.
fn tool_summary(content: &Value) -> String {
    let joined = match content {
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .map(|part| text(part.get("text").unwrap_or(&Value::Null)))
            .collect::<Vec<_>>()
            .join("\n"),
        other => text(other),
    };
    if let Ok(Value::Object(object)) = serde_json::from_str::<Value>(&joined)
        && let Some(Value::String(summary)) = object.get("summary")
    {
        return summary.clone();
    }
    joined
        .split('\n')
        .next()
        .unwrap_or_default()
        .chars()
        .take(200)
        .collect()
}

fn events(events: Vec<Event>) -> Option<Parsed> {
    Some(Parsed {
        events,
        version: None,
    })
}

/// Turns one line of a CLI's JSON stream into events. Lines that are not JSON objects parse to
/// nothing; unknown event types parse to no events, and the run fails later if no result comes.
pub fn parse_line(backend: Backend, line: &str, state: &mut ParseState) -> Option<Parsed> {
    let value: Value = serde_json::from_str(line).ok()?;
    let object = value.as_object()?;
    let kind = object
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match backend {
        Backend::Claude => parse_claude(&value, kind, state),
        Backend::Codex => parse_codex(&value, kind, state),
    }
}

fn parse_claude(value: &Value, kind: &str, state: &mut ParseState) -> Option<Parsed> {
    let content = || {
        value
            .pointer("/message/content")
            .and_then(Value::as_array)
            .cloned()
    };
    match kind {
        "system" if value.get("subtype").and_then(Value::as_str) == Some("init") => Some(Parsed {
            events: value
                .get("session_id")
                .filter(|id| !id.is_null() && !text(id).is_empty())
                .map(|id| Event::Session(text(id)))
                .into_iter()
                .collect(),
            version: value
                .get("claude_code_version")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        "stream_event"
            if value.pointer("/event/type").and_then(Value::as_str)
                == Some("content_block_delta")
                && value.pointer("/event/delta/type").and_then(Value::as_str)
                    == Some("text_delta") =>
        {
            events(vec![Event::Token(text(
                value.pointer("/event/delta/text").unwrap_or(&Value::Null),
            ))])
        }
        "assistant" if content().is_some() => events(
            content()
                .unwrap_or_default()
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_use"))
                .map(|part| {
                    let name = tool_name(part.get("name").unwrap_or(&Value::Null));
                    let id = part.get("id").filter(|id| !id.is_null()).map(text);
                    if let Some(id) = &id {
                        state.tools.insert(id.clone(), name.clone());
                    }
                    Event::Tool {
                        id,
                        name,
                        detail: tool_detail(part.get("input").unwrap_or(&Value::Null)),
                        result: None,
                        failed: false,
                    }
                })
                .collect(),
        ),
        "user" if content().is_some() => events(
            content()
                .unwrap_or_default()
                .iter()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("tool_result"))
                .map(|part| {
                    let id = text(part.get("tool_use_id").unwrap_or(&Value::Null));
                    Event::Tool {
                        name: state
                            .tools
                            .get(&id)
                            .cloned()
                            .unwrap_or_else(|| "tool".into()),
                        id: Some(id),
                        detail: String::new(),
                        result: Some(tool_summary(part.get("content").unwrap_or(&Value::Null))),
                        failed: part.get("is_error") == Some(&Value::Bool(true)),
                    }
                })
                .collect(),
        ),
        "result" => {
            let success = value.get("subtype").and_then(Value::as_str) == Some("success")
                && !value
                    .get("is_error")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
            events(vec![if success {
                Event::Done {
                    text: text(value.get("result").unwrap_or(&Value::Null)),
                    session: value
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                }
            } else {
                let message = [value.get("result"), value.get("subtype")]
                    .into_iter()
                    .flatten()
                    .map(text)
                    .find(|message| !message.is_empty())
                    .unwrap_or_else(|| "unknown".into());
                Event::Error(message)
            }])
        }
        _ => events(Vec::new()),
    }
}

fn parse_codex(value: &Value, kind: &str, state: &mut ParseState) -> Option<Parsed> {
    let item = value.get("item").unwrap_or(&Value::Null);
    let item_kind = item.get("type").and_then(Value::as_str);
    match (kind, item_kind) {
        ("thread.started", _) if value.get("thread_id").is_some_and(|id| !id.is_null()) => {
            events(vec![Event::Session(text(&value["thread_id"]))])
        }
        ("item.completed", Some("agent_message")) => {
            let message = text(item.get("text").unwrap_or(&Value::Null));
            let token = match state.answer {
                None => message.clone(),
                Some(_) => format!("\n\n{message}"),
            };
            state.answer = Some(message);
            events(vec![Event::Token(token)])
        }
        ("item.started", Some("mcp_tool_call")) => events(vec![Event::Tool {
            id: Some(text(item.get("id").unwrap_or(&Value::Null))),
            name: text(item.get("tool").unwrap_or(&Value::Null)),
            detail: tool_detail(item.get("arguments").unwrap_or(&Value::Null)),
            result: None,
            failed: false,
        }]),
        ("item.completed", Some("mcp_tool_call")) => {
            let failed = item.get("status").and_then(Value::as_str) == Some("failed")
                || item.pointer("/result/isError") == Some(&Value::Bool(true));
            let result = match item.pointer("/error/message") {
                Some(message) if !text(message).is_empty() => text(message),
                _ => tool_summary(item.pointer("/result/content").unwrap_or(&Value::Null)),
            };
            events(vec![Event::Tool {
                id: Some(text(item.get("id").unwrap_or(&Value::Null))),
                name: text(item.get("tool").unwrap_or(&Value::Null)),
                detail: String::new(),
                result: Some(result),
                failed,
            }])
        }
        (_, Some("mcp_tool_call")) => events(Vec::new()),
        ("item.started", Some("command_execution")) => events(vec![Event::Tool {
            id: None,
            name: "command".into(),
            detail: text(item.get("command").unwrap_or(&Value::Null)),
            result: None,
            failed: false,
        }]),
        ("turn.completed", _) => events(vec![Event::Done {
            text: state.answer.clone().unwrap_or_default(),
            session: None,
        }]),
        ("turn.failed", _) => events(vec![Event::Error(
            value
                .pointer("/error/message")
                .map(text)
                .unwrap_or_else(|| "unknown".into()),
        )]),
        // Top-level errors include transient notices such as reconnect attempts; the run fails
        // only through turn.failed or a missing result.
        ("error", _) => events(vec![Event::Tool {
            id: None,
            name: "notice".into(),
            detail: text(value.get("message").unwrap_or(&Value::Null)),
            result: None,
            failed: false,
        }]),
        _ => events(Vec::new()),
    }
}

static LOGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i-u)not logged in|log ?in|authenticat|api key|unauthori[sz]ed|(?-u:\b)401(?-u:\b)|expired")
        .expect("The sign-in pattern is invalid. Fix it and rebuild.")
});
static MODEL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[A-Za-z0-9_.:/\[\]][A-Za-z0-9_.:/\[\]-]{0,99}$")
        .expect("The model name pattern is invalid. Fix it and rebuild.")
});
static VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9]+\.[0-9]+[A-Za-z0-9_.-]*").expect("The version pattern is invalid. Rebuild.")
});

/// The version in `--version` output, such as "2.1.312 (Claude Code)" or "codex-cli 0.164.0".
pub fn version_of(output: &str) -> String {
    VERSION
        .find(output)
        .map_or_else(|| "unknown".into(), |found| found.as_str().to_owned())
}

/// The message for an error event the CLI reported.
pub fn stream_error(backend: Backend, message: &str) -> String {
    let name = backend.name();
    if LOGIN.is_match(message) {
        format!(
            "{name} could not sign in: {message}. {}",
            backend.login_help()
        )
    } else {
        format!("{name} stopped with an error: {message}. Retry, or check the CLI in a terminal.")
    }
}

/// The message for a CLI that exited unsuccessfully. `status` is "code N" or a signal name and
/// `stderr` the end of what it wrote there.
pub fn exit_error(backend: Backend, status: &str, stderr: &str) -> String {
    let lines: Vec<&str> = stderr.trim().split('\n').collect();
    let detail = lines[lines.len().saturating_sub(3)..].join(" ");
    let name = backend.name();
    if LOGIN.is_match(&detail) {
        format!("{name} could not sign in. {}", backend.login_help())
    } else if detail.is_empty() {
        format!(
            "{name} exited with {status}. Retry, or run it in a terminal to see the full error."
        )
    } else {
        format!(
            "{name} exited with {status}: {detail}. Retry, or run it in a terminal to see the full error."
        )
    }
}

/// The message for a run that ended without a result the parser recognized.
pub fn drift_error(backend: Backend, version: &str, sent_text: bool) -> String {
    let name = backend.name();
    format!(
        "{name} {version} ended without a result Quire could read{}. Quire expects {name} {}; install that version or update Quire.",
        if sent_text { "" } else { " and sent no text" },
        backend.tested_version()
    )
}

pub fn timeout_error(backend: Backend, seconds: u64) -> String {
    format!(
        "{} did not finish within {seconds} seconds. Retry, or run the command on a shorter range.",
        backend.name()
    )
}

pub fn missing_binary(backend: Backend, configured: Option<&str>) -> String {
    match configured {
        Some(path) => format!(
            "{} was not found at {path}. Correct Binary path in Settings > AI, or clear it to detect automatically, then press Test.",
            backend.name()
        ),
        None => format!(
            "{} was not found on this computer's PATH. Install it, or enter its location in Binary path under Settings > AI, then press Test.",
            backend.name()
        ),
    }
}

/// Checks a binary path typed in Settings: empty to detect automatically, or absolute.
pub fn validate_binary(backend: Backend, path: &str) -> Result<String, String> {
    let path = path.trim();
    if path.is_empty() || std::path::Path::new(path).is_absolute() {
        Ok(path.to_owned())
    } else {
        Err(format!(
            "Binary path for {} must be absolute, such as /opt/homebrew/bin/{}. Clear it to detect automatically.",
            backend.name(),
            backend.binary()
        ))
    }
}

/// Checks a model name: empty for the CLI's default, or an identifier that cannot read as a flag.
pub fn validate_model(model: &str) -> Result<String, String> {
    let model = model.trim();
    if model.is_empty() || MODEL.is_match(model) {
        Ok(model.to_owned())
    } else {
        Err("The model name has unsupported characters. Use the model's identifier, such as sonnet or gpt-5.".into())
    }
}

pub const AI_OFF: &str =
    "AI commands are off. Turn them on in Settings > AI after a successful Test.";
pub const CANCELED: &str = "Canceled.";
pub const PROMPT_LIMIT: usize = 400_000;
pub const PROMPT_TOO_LONG: &str = "The AI request is incomplete or over 400,000 characters. Select less text, or synthesize a page with fewer references, then retry.";
