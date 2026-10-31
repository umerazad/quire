//! The Quire MCP server (reference `mcp-server.ts`): the process AI CLIs start with
//! `quire --mcp-server`. It never reads the notebook; every call goes to the running app over the
//! bridge named in `bridge.json`, which it reads on each request so a restarted app is picked up.
//! Standard library only, so it starts without the app's windowing and also builds into the
//! `quire_mcp` test example.

use quire_core::ai::mcp::{Bridge, answer_line};
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

pub const CLOSED_APP: &str =
    "Quire is not running with a notebook open. Open Quire with the notebook, then retry.";
const SLOW: &str = "Quire did not answer in time. Make sure its window is open, then retry.";
const UNREADABLE: &str =
    "Quire sent a reply the tool server could not read. Update Quire, then retry.";
const TIMEOUT: Duration = Duration::from_secs(25);
/// The variable naming the bridge file, and the one naming the in-app run calls belong to.
pub const BRIDGE_VARIABLE: &str = "QUIRE_BRIDGE";
pub const RUN_VARIABLE: &str = "QUIRE_RUN";

/// Reaches the running app through the socket and token in a bridge file.
pub struct BridgeClient {
    pub file: PathBuf,
    pub run: Option<String>,
    next: AtomicU64,
}

impl BridgeClient {
    pub fn new(file: PathBuf, run: Option<String>) -> Self {
        Self {
            file,
            run,
            next: AtomicU64::new(0),
        }
    }

    fn send(&self, method: &str, params: Option<Value>) -> Result<Value, String> {
        let bridge: Value = std::fs::read(&self.file)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .ok_or_else(|| CLOSED_APP.to_owned())?;
        let (Some(socket), Some(token)) = (bridge["socket"].as_str(), bridge["token"].as_str())
        else {
            return Err(CLOSED_APP.into());
        };
        let mut stream = UnixStream::connect(socket).map_err(|_| CLOSED_APP.to_owned())?;
        stream.set_read_timeout(Some(TIMEOUT)).ok();
        let id = self.next.fetch_add(1, Ordering::Relaxed) + 1;
        let mut request = json!({ "id": id, "token": token, "method": method });
        if let Some(params) = params {
            request["params"] = params;
        }
        writeln!(stream, "{request}").map_err(|_| CLOSED_APP.to_owned())?;
        let mut line = String::new();
        match BufReader::new(stream).read_line(&mut line) {
            Ok(0) => return Err(CLOSED_APP.into()),
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                return Err(SLOW.into());
            }
            Err(_) => return Err(CLOSED_APP.into()),
        }
        let answer: Value = serde_json::from_str(&line).map_err(|_| UNREADABLE.to_owned())?;
        match answer.get("error").and_then(Value::as_str) {
            Some(error) => Err(error.to_owned()),
            None => Ok(answer.get("result").cloned().unwrap_or(Value::Null)),
        }
    }
}

impl Bridge for BridgeClient {
    fn list(&self) -> Result<Value, String> {
        let result = self.send("tools/list", None)?;
        result
            .get("tools")
            .filter(|tools| tools.is_array())
            .cloned()
            .ok_or_else(|| UNREADABLE.to_owned())
    }

    fn call(&self, name: &str, input: Value) -> Result<Value, String> {
        let mut params = json!({ "name": name, "arguments": input });
        if let Some(run) = &self.run {
            params["run"] = run.clone().into();
        }
        let outcome = self.send("tools/call", Some(params))?;
        match outcome.get("ok").and_then(Value::as_bool) {
            Some(true) => Ok(outcome.get("result").cloned().unwrap_or(Value::Null)),
            _ => Err(outcome
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or(UNREADABLE)
                .to_owned()),
        }
    }
}

/// Answers frames as they arrive, each on its own thread so a slow call never holds up others,
/// and returns once input ends and every reply is written.
pub fn serve(
    input: impl BufRead,
    output: impl Write + Send + 'static,
    client: BridgeClient,
    version: &str,
) {
    let client = Arc::new(client);
    let output = Arc::new(Mutex::new(output));
    let workers: Vec<_> = input
        .lines()
        .map_while(Result::ok)
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (client, output, version) = (client.clone(), output.clone(), version.to_owned());
            std::thread::spawn(move || {
                if let Some(reply) = answer_line(&line, client.as_ref(), &version)
                    && let Ok(mut output) = output.lock()
                {
                    writeln!(output, "{reply}").ok();
                    output.flush().ok();
                }
            })
        })
        .collect();
    for worker in workers {
        worker.join().ok();
    }
}

/// The server's entry: standard input and output, the bridge file from `QUIRE_BRIDGE` or
/// `default_file`, and the in-app run from `QUIRE_RUN`.
pub fn run(default_file: PathBuf, version: &str) {
    let file = std::env::var_os(BRIDGE_VARIABLE)
        .filter(|value| !value.is_empty())
        .map_or(default_file, PathBuf::from);
    let run = std::env::var(RUN_VARIABLE)
        .ok()
        .filter(|run| !run.is_empty());
    serve(
        std::io::stdin().lock(),
        std::io::stdout(),
        BridgeClient::new(file, run),
        version,
    );
}
