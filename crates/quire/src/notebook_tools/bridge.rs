//! The notebook bridge (reference `notebook-bridge.ts`): a private Unix socket in Quire's data
//! folder that serves the notebook tools to local MCP servers over newline-delimited JSON. The
//! socket comes from Zed's `net` crate, as Zed's own MCP server socket does, so Windows (AF_UNIX)
//! uses the same file, token, and framing as macOS and Linux. Each
//! request carries the per-launch token from `bridge.json`; a call is checked here (tool, AI
//! commands on, AI writes, input, open notebook) before the app answers it against the live
//! notebook.

use futures::{AsyncReadExt as _, AsyncWriteExt as _, StreamExt as _, channel::mpsc};
use gpui::{App, AppContext as _, AsyncApp, Task};
use net::async_net::{UnixListener, UnixStream};
use quire_core::ai::tools::{
    AI_COMMANDS_OFF, NO_NOTEBOOK, WRITES_OFF, Writes, check_input, listed_tools,
};
use serde_json::{Value, json};
use smol::future::FutureExt as _;
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    time::Duration,
};

pub const FILE_NAME: &str = "bridge.json";
pub const CALL_TIMEOUT: Duration = Duration::from_secs(20);
const MAX_LINE: usize = 1_000_000;
const TOKEN_MISMATCH: &str = "The Quire token did not match. Restart the AI session so the tool server reads the current bridge file.";

/// One checked tool call, as the app answers it.
#[derive(Clone, Debug, PartialEq)]
pub struct Call {
    pub name: String,
    pub input: Value,
    /// The in-app run the call belongs to; None for a terminal session.
    pub run: Option<String>,
    pub writes: Writes,
}

pub type Forward = Rc<dyn Fn(Call, &mut App) -> Task<Result<Value, String>>>;

/// What the bridge asks of the app on every request.
#[derive(Clone)]
pub struct Options {
    pub directory: PathBuf,
    pub enabled: Rc<dyn Fn(&App) -> bool>,
    pub writes: Rc<dyn Fn(&App) -> Writes>,
    pub notebook_open: Rc<dyn Fn(&App) -> bool>,
    pub forward: Forward,
    pub timeout: Duration,
}

/// A running bridge. Dropping it closes the socket and every connection and removes the bridge
/// file, so the MCP server reports the app as closed.
pub struct NotebookBridge {
    pub file: PathBuf,
    pub socket: PathBuf,
    _accept: Task<()>,
}

fn private_write(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let temporary = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&temporary)?, bytes)?;
    std::fs::rename(&temporary, path)
}

fn token() -> Result<[u8; 32], String> {
    use rand::{RngCore as _, SeedableRng as _};
    let mut token = [0; 32];
    rand::rngs::StdRng::try_from_os_rng()
        .map_err(|error| format!("Quire could not make a bridge token: {error}. Restart Quire, then turn AI commands on again."))?
        .fill_bytes(&mut token);
    Ok(token)
}

impl NotebookBridge {
    /// Binds the socket, writes `bridge.json` with a fresh token, and starts answering.
    pub fn start(options: Options, cx: &mut App) -> Result<Self, String> {
        let failed = |error: std::io::Error| {
            format!(
                "The AI tool server could not start: {error}. Turn AI commands off and on in Settings > AI to retry."
            )
        };
        let file = options.directory.join(FILE_NAME);
        let socket = crate::single_instance::socket_path(&options.directory, "bridge.sock");
        std::fs::create_dir_all(&options.directory).map_err(failed)?;
        std::fs::remove_file(&socket).ok();
        let listener = UnixListener::bind(&socket).map_err(failed)?;
        #[cfg(unix)]
        std::fs::set_permissions(&socket, std::os::unix::fs::PermissionsExt::from_mode(0o600))
            .map_err(failed)?;
        let token = token()?;
        let contents = json!({
            "version": 1,
            "socket": socket.to_string_lossy(),
            "token": hex::encode(token),
            "pid": std::process::id(),
        });
        private_write(&file, contents.to_string().as_bytes()).map_err(failed)?;
        let accept = cx.spawn(async move |cx| {
            let connections = Rc::new(RefCell::new(Vec::new()));
            while let Ok((stream, _)) = listener.accept().await {
                let options = options.clone();
                let task = cx.spawn(async move |cx| connect(stream, token, options, cx).await);
                let mut connections = connections.borrow_mut();
                connections.retain(|task: &Task<()>| !task.is_ready());
                connections.push(task);
            }
        });
        Ok(Self {
            file,
            socket,
            _accept: accept,
        })
    }
}

impl Drop for NotebookBridge {
    fn drop(&mut self) {
        std::fs::remove_file(&self.file).ok();
        std::fs::remove_file(&self.socket).ok();
    }
}

async fn connect(stream: UnixStream, token: [u8; 32], options: Options, cx: &mut AsyncApp) {
    let (send, mut replies) = mpsc::unbounded::<Value>();
    let (mut reader, mut writer) = stream.split();
    let writing = cx.background_spawn(async move {
        while let Some(reply) = replies.next().await {
            if writer
                .write_all(format!("{reply}\n").as_bytes())
                .await
                .is_err()
            {
                break;
            }
        }
    });
    let mut buffer = Vec::new();
    let mut chunk = [0; 8192];
    let mut answering = Vec::new();
    while let Ok(read) = reader.read(&mut chunk).await {
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
        while let Some(end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line: Vec<u8> = buffer.drain(..=end).collect();
            let line = String::from_utf8_lossy(&line[..end]).into_owned();
            if line.trim().is_empty() {
                continue;
            }
            let reply = cx.update(|cx| answer(&line, &token, &options, cx));
            let send = send.clone();
            answering.push(cx.spawn(async move |_| {
                send.unbounded_send(reply.await).ok();
            }));
        }
        if buffer.len() > MAX_LINE {
            send.unbounded_send(json!({ "id": null, "error": "The bridge request is over 1 MB. Send a smaller request." })).ok();
            break;
        }
    }
    for task in answering {
        task.await;
    }
    drop(send);
    writing.await;
}

fn authorized(given: Option<&str>, token: &[u8; 32]) -> bool {
    let Some(given) = given
        .filter(|given| given.len() == 64)
        .and_then(|given| hex::decode(given).ok())
    else {
        return false;
    };
    given.len() == token.len()
        && given
            .iter()
            .zip(token)
            .fold(0u8, |difference, (a, b)| difference | (a ^ b))
            == 0
}

/// Answers one request line: `{id, result}` or `{id, error}`.
pub fn answer(line: &str, token: &[u8; 32], options: &Options, cx: &mut App) -> Task<Value> {
    let Ok(request) = serde_json::from_str::<Value>(line) else {
        return Task::ready(json!({
            "id": null,
            "error": "The bridge request is not JSON. Update Quire so its tool server matches the app.",
        }));
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if !authorized(request.get("token").and_then(Value::as_str), token) {
        return Task::ready(json!({ "id": id, "error": TOKEN_MISMATCH }));
    }
    match request.get("method").and_then(Value::as_str) {
        Some("tools/list") => {
            let tools = listed_tools((options.writes)(cx));
            Task::ready(json!({ "id": id, "result": { "tools": tools } }))
        }
        Some("tools/call") => {
            let outcome = call(request.get("params"), options, cx);
            cx.spawn(async move |_| {
                let result = match outcome.await {
                    Ok(result) => json!({ "ok": true, "result": result }),
                    Err(error) => json!({ "ok": false, "error": error }),
                };
                json!({ "id": id, "result": result })
            })
        }
        method => {
            let method = match method {
                Some(method) => method.to_owned(),
                None => request
                    .get("method")
                    .map_or("undefined".into(), Value::to_string),
            };
            Task::ready(json!({
                "id": id,
                "error": format!("Unknown bridge method {method}. Update Quire so its tool server matches the app."),
            }))
        }
    }
}

fn valid_run(run: &str) -> bool {
    (1..=80).contains(&run.len())
        && run
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

fn call(params: Option<&Value>, options: &Options, cx: &mut App) -> Task<Result<Value, String>> {
    let field = |name: &str| params.and_then(|params| params.get(name));
    let name = field("name").and_then(Value::as_str).unwrap_or_default();
    let input = field("arguments")
        .filter(|input| !input.is_null())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let Some(spec) = quire_core::ai::tools::spec(name) else {
        let shown = if name.is_empty() { "(none)" } else { name };
        return Task::ready(Err(format!(
            "Unknown tool {shown}. Call tools/list for the available tools."
        )));
    };
    if !(options.enabled)(cx) {
        return Task::ready(Err(AI_COMMANDS_OFF.into()));
    }
    let writes = (options.writes)(cx);
    if spec.write && writes == Writes::Off {
        return Task::ready(Err(WRITES_OFF.into()));
    }
    if let Err(problem) = check_input(name, &input) {
        return Task::ready(Err(problem));
    }
    if !(options.notebook_open)(cx) {
        return Task::ready(Err(NO_NOTEBOOK.into()));
    }
    let run = field("run")
        .and_then(Value::as_str)
        .filter(|run| valid_run(run))
        .map(str::to_owned);
    let call = Call {
        name: name.to_owned(),
        input,
        run,
        writes,
    };
    let timeout = options.timeout;
    let answered = (options.forward)(call, cx);
    let timer = cx.background_executor().timer(timeout);
    cx.spawn(async move |_| {
        answered
            .or(async move {
                timer.await;
                Err(format!(
                    "Quire did not answer within {} seconds. Make sure its window is open and the notebook has loaded, then retry.",
                    timeout.as_secs()
                ))
            })
            .await
    })
}

#[cfg(test)]
#[path = "bridge_tests.rs"]
pub(crate) mod tests;
