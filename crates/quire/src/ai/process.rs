//! The AI process host (reference `AiRunner` in `src/desktop/ai-runner.ts`): finds the CLI,
//! starts it through Zed's process wrapper in its own process group, sends the prompt on standard
//! input, streams parsed events back, and ends the whole process tree on cancel, timeout, or
//! completion. Each run works in a fresh empty folder, and its prompt and response are kept in the
//! notebook's transcript folder.

use futures::{AsyncBufReadExt as _, AsyncReadExt as _, AsyncWriteExt as _, FutureExt as _};
use quire_core::ai::{
    Backend, Event, ParseState,
    backend::{
        CANCELED, backend_input, drift_error, exit_error, missing_binary, stream_error,
        timeout_error, transform_args, version_of,
    },
    commands::transform_system_prompt,
    parse_line,
};
use smol::future::FutureExt as _;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::OnceLock,
    time::{Duration, Instant},
};

pub const TRANSFORM_TIMEOUT: Duration = Duration::from_secs(120);
pub const TEST_TIMEOUT: Duration = Duration::from_secs(60);
const VERSION_TIMEOUT: Duration = Duration::from_secs(10);
const SHELL_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_KEPT: usize = 4000;

/// How runs find and start CLIs. The app uses the login shell's PATH; tests pass their own
/// folders and environment so they never reach an installed CLI.
#[derive(Clone, Debug, Default)]
pub struct Host {
    /// Folders searched for a CLI with no configured binary; None means the login shell's PATH.
    pub search: Option<Vec<PathBuf>>,
    /// Extra environment for every CLI started.
    pub env: Vec<(String, String)>,
}

impl gpui::Global for Host {}

/// One transform request.
#[derive(Clone, Debug)]
pub struct Request {
    pub backend: Backend,
    /// The configured binary path, or empty to search for it.
    pub binary: String,
    pub model: String,
    pub prompt: String,
    pub timeout: Duration,
    /// Where to keep the prompt and response, with the run's ID and page.
    pub transcript: Option<Transcript>,
}

#[derive(Clone, Debug)]
pub struct Transcript {
    pub folder: PathBuf,
    pub id: String,
    pub page: String,
}

/// What a run reports while it works.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Update {
    Token(String),
    Tool { name: String, detail: String },
}

/// How a run ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done(String),
    Failed { message: String, retry: bool },
    Canceled,
}

impl Outcome {
    fn failed(message: impl Into<String>) -> Self {
        Self::Failed {
            message: message.into(),
            retry: false,
        }
    }
}

/// The login shell's PATH and the usual install folders, captured once: apps started from the
/// Dock do not inherit the folders where CLIs are usually installed.
async fn login_search_path() -> Vec<PathBuf> {
    static CAPTURED: OnceLock<Vec<PathBuf>> = OnceLock::new();
    if let Some(found) = CAPTURED.get() {
        return found.clone();
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    let extra = [
        home.join(".local").join("bin"),
        "/opt/homebrew/bin".into(),
        "/usr/local/bin".into(),
    ];
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/zsh".into());
    let mut command = util::command::new_std_command(shell);
    command.args(["-l", "-c", "/usr/bin/env"]);
    let output = smol::process::Command::from(command)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|output| output.ok())
        .or(async {
            smol::Timer::after(SHELL_TIMEOUT).await;
            None
        })
        .await;
    let shell_path = output
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .and_then(|env| {
            env.lines()
                .find_map(|line| line.strip_prefix("PATH=").map(str::to_owned))
        })
        .unwrap_or_else(|| std::env::var("PATH").unwrap_or_default());
    let found: Vec<PathBuf> = std::env::split_paths(&shell_path).chain(extra).collect();
    CAPTURED.get_or_init(|| found).clone()
}

impl Host {
    async fn search(&self) -> Vec<PathBuf> {
        match &self.search {
            Some(folders) => folders.clone(),
            None => login_search_path().await,
        }
    }

    /// The configured binary, or the first match in the searched folders.
    pub async fn locate(&self, backend: Backend, configured: &str) -> Result<PathBuf, String> {
        if !configured.is_empty() {
            let path = PathBuf::from(configured);
            return executable(&path)
                .then_some(path)
                .ok_or_else(|| missing_binary(backend, Some(configured)));
        }
        let name = if cfg!(windows) {
            format!("{}.exe", backend.binary())
        } else {
            backend.binary().to_owned()
        };
        self.search()
            .await
            .into_iter()
            .filter(|folder| !folder.as_os_str().is_empty())
            .map(|folder| folder.join(&name))
            .find(|file| executable(file))
            .ok_or_else(|| missing_binary(backend, None))
    }

    /// The CLI's environment: Quire's own, without variables that make a CLI think it runs
    /// inside another agent, with the searched folders first on PATH.
    async fn command(&self, binary: &Path) -> std::process::Command {
        let mut command = util::command::new_std_command(binary);
        for key in [
            "CLAUDECODE",
            "CLAUDE_CODE_ENTRYPOINT",
            "ELECTRON_RUN_AS_NODE",
        ] {
            command.env_remove(key);
        }
        let current = std::env::var_os("PATH").unwrap_or_default();
        let mut folders = self.search().await;
        folders.extend(std::env::split_paths(&current));
        let mut seen = std::collections::HashSet::new();
        folders.retain(|folder| !folder.as_os_str().is_empty() && seen.insert(folder.clone()));
        if let Ok(path) = std::env::join_paths(folders) {
            command.env("PATH", path);
        }
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
        command
    }

    /// `binary --version`, as the version number it prints.
    pub async fn version(&self, backend: Backend, binary: &Path) -> Result<String, String> {
        let mut command = self.command(binary).await;
        command.arg("--version");
        let name = binary
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| backend.binary().into());
        let failed = |error: String| {
            format!("{name} --version failed: {error}. Check Binary path in Settings > AI.")
        };
        let output = smol::process::Command::from(command)
            .stdin(Stdio::null())
            .output()
            .map(|output| output.map_err(|error| error.to_string()))
            .or(async {
                smol::Timer::after(VERSION_TIMEOUT).await;
                Err("it did not answer within 10 seconds".into())
            })
            .await
            .map_err(failed)?;
        if !output.status.success() {
            return Err(failed(format!("it exited with {}", output.status)));
        }
        Ok(version_of(&String::from_utf8_lossy(&output.stdout)))
    }

    /// Runs one transform until it finishes, fails, or `cancel` resolves, reporting progress
    /// through `emit`. The process tree ends with the run in every case.
    pub async fn run(
        &self,
        request: Request,
        cancel: impl Future<Output = ()>,
        mut emit: impl FnMut(Update),
    ) -> Outcome {
        let cancel = cancel.shared();
        let started = Instant::now();
        let backend = request.backend;
        let binary = match self.locate(backend, request.binary.trim()).await {
            Ok(binary) => binary,
            Err(message) => return Outcome::failed(message),
        };
        if cancel.clone().now_or_never().is_some() {
            return Outcome::Canceled;
        }
        let folder =
            RunFolder(std::env::temp_dir().join(format!("quire-ai-{}", uuid::Uuid::new_v4())));
        if let Err(error) = std::fs::create_dir_all(&folder.0) {
            return Outcome::failed(format!(
                "Quire could not make a working folder for {} at {}: {error}. Check that the temporary folder is writable, then retry.",
                backend.name(),
                folder.0.display()
            ));
        }
        let (outcome, version) = self
            .spawn_and_read(&request, &binary, &folder.0, cancel, &mut emit)
            .await;
        drop(folder);
        if let Some(transcript) = &request.transcript {
            write_transcript(transcript, &request, version.as_deref(), &outcome, started).ok();
        }
        outcome
    }

    async fn spawn_and_read(
        &self,
        request: &Request,
        binary: &Path,
        folder: &Path,
        cancel: impl Future<Output = ()>,
        emit: &mut impl FnMut(Update),
    ) -> (Outcome, Option<String>) {
        let backend = request.backend;
        let mut command = self.command(binary).await;
        command
            .args(transform_args(
                backend,
                &folder.to_string_lossy(),
                &request.model,
            ))
            .current_dir(folder);
        let mut child = match util::process::Child::spawn(
            command,
            Stdio::piped(),
            Stdio::piped(),
            Stdio::piped(),
        ) {
            Ok(child) => Running(child),
            Err(error) => {
                let missing = error.chain().any(|cause| {
                    cause
                        .downcast_ref::<std::io::Error>()
                        .is_some_and(|io| io.kind() == std::io::ErrorKind::NotFound)
                });
                let message = if missing {
                    format!(
                        "{} was not found at {}. Correct Binary path in Settings > AI, then press Test.",
                        backend.name(),
                        binary.display()
                    )
                } else {
                    format!(
                        "{} could not start: {error:#}. Check Binary path in Settings > AI.",
                        backend.name()
                    )
                };
                return (Outcome::failed(message), None);
            }
        };
        let input = backend_input(backend, &request.prompt);
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let mut version = None;
        let mut answer = String::new();
        let writing = async move {
            if let Some(mut stdin) = stdin {
                stdin.write_all(input.as_bytes()).await.ok();
                stdin.close().await.ok();
            }
        };
        let errors = async move {
            let mut text = String::new();
            if let Some(mut stderr) = stderr {
                stderr.read_to_string(&mut text).await.ok();
            }
            let start = text.len().saturating_sub(STDERR_KEPT);
            let start = (start..text.len())
                .find(|index| text.is_char_boundary(*index))
                .unwrap_or(text.len());
            text[start..].to_owned()
        }
        .shared();
        let reading = async {
            let stdout = stdout?;
            let mut lines = futures::io::BufReader::new(stdout).lines();
            let mut state = ParseState::default();
            while let Some(Ok(line)) = futures::StreamExt::next(&mut lines).await {
                if line.trim().is_empty() {
                    continue;
                }
                let Some(parsed) = parse_line(backend, &line, &mut state) else {
                    continue;
                };
                version = version.take().or(parsed.version);
                for event in parsed.events {
                    match event {
                        Event::Token(text) => {
                            answer.push_str(&text);
                            emit(Update::Token(text));
                        }
                        Event::Tool {
                            name,
                            detail,
                            result: None,
                            ..
                        } => emit(Update::Tool { name, detail }),
                        Event::Done { text, .. } => return Some(Outcome::Done(text)),
                        Event::Error(message) => {
                            return Some(Outcome::failed(stream_error(backend, &message)));
                        }
                        Event::Session(_) | Event::Tool { .. } => {}
                    }
                }
            }
            None
        };
        let read_all = async {
            let stderr_read = errors.clone();
            futures::pin_mut!(reading, stderr_read);
            match futures::future::select(reading, stderr_read).await {
                futures::future::Either::Left((read, _)) => read,
                futures::future::Either::Right((_, reading)) => reading.await,
            }
        };
        let timeout = request.timeout;
        let raced = futures::future::join(writing, read_all)
            .map(|(_, read)| Ok(read))
            .or(cancel.map(|()| Err(Outcome::Canceled)))
            .or(async {
                smol::Timer::after(timeout).await;
                Err(Outcome::Failed {
                    message: timeout_error(backend, timeout.as_secs()),
                    retry: true,
                })
            })
            .await;
        let outcome = match raced {
            Err(stopped) => {
                child.kill().ok();
                stopped
            }
            Ok(Some(finished)) => {
                child.kill().ok();
                finished
            }
            Ok(None) => {
                let status = child
                    .status()
                    .map(Some)
                    .or(async {
                        smol::Timer::after(timeout).await;
                        None
                    })
                    .await;
                match status {
                    Some(Ok(status)) if !status.success() => {
                        Outcome::failed(exit_error(backend, &exit_status(&status), &errors.await))
                    }
                    Some(Ok(_)) => {
                        let found = match &version {
                            Some(version) => version.clone(),
                            None => self
                                .version(backend, binary)
                                .await
                                .unwrap_or_else(|_| "(unknown version)".into()),
                        };
                        Outcome::failed(drift_error(backend, &found, !answer.is_empty()))
                    }
                    _ => {
                        child.kill().ok();
                        Outcome::Failed {
                            message: timeout_error(backend, timeout.as_secs()),
                            retry: true,
                        }
                    }
                }
            }
        };
        child.status().await.ok();
        (outcome, version)
    }
}

/// A started CLI whose process tree ends when the run is dropped, even mid-await, as when the
/// window that started it closes.
struct Running(util::process::Child);

impl std::ops::Deref for Running {
    type Target = util::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Running {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if matches!(self.0.try_status(), Ok(None)) {
            self.0.kill().ok();
        }
    }
}

/// The empty folder a run works in, removed with the run.
struct RunFolder(PathBuf);

impl Drop for RunFolder {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn exit_status(status: &std::process::ExitStatus) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    status
        .code()
        .map_or_else(|| status.to_string(), |code| format!("code {code}"))
}

fn executable(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    metadata.is_file()
}

/// Keeps the prompt and final response of a run in `<folder>/<date>/<time>-<id>.md`, readable
/// only by the owner, as the reference keeps them in `.quire/ai`.
fn write_transcript(
    transcript: &Transcript,
    request: &Request,
    version: Option<&str>,
    outcome: &Outcome,
    started: Instant,
) -> std::io::Result<()> {
    let now = jiff::Timestamp::now();
    let stamp = now.to_string();
    let folder = transcript.folder.join(&stamp[..10]);
    std::fs::create_dir_all(&folder)?;
    let safe_id: String = transcript
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-'))
        .take(40)
        .collect();
    let result = match outcome {
        Outcome::Done(text) => text.clone(),
        Outcome::Failed { message, .. } => format!("Error: {message}"),
        Outcome::Canceled => format!("Error: {CANCELED}"),
    };
    let text = [
        "# AI transform".to_owned(),
        String::new(),
        format!(
            "- Backend: {} {}",
            request.backend.name(),
            version.unwrap_or_default()
        )
        .trim_end()
        .to_owned(),
        format!("- Elapsed: {:.1} s", started.elapsed().as_secs_f64()),
        format!("- Page: {}", transcript.page),
        String::new(),
        "## System prompt".into(),
        String::new(),
        transform_system_prompt(),
        String::new(),
        "## Prompt".into(),
        String::new(),
        request.prompt.clone(),
        String::new(),
        "## Response".into(),
        String::new(),
        result,
        String::new(),
    ]
    .join("\n");
    let name = format!("{}-{safe_id}.md", stamp.replace([':', '.'], "-"));
    let path = folder.join(name);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(path)?, text.as_bytes())
}

/// Checks that the CLI runs and answers, as Settings' Test button does: the version, then a
/// one-word transform. Returns the version.
pub async fn test(
    host: &Host,
    backend: Backend,
    binary: &str,
    model: &str,
) -> Result<String, String> {
    let located = host.locate(backend, binary.trim()).await?;
    let version = host.version(backend, &located).await?;
    let request = Request {
        backend,
        binary: located.to_string_lossy().into_owned(),
        model: model.to_owned(),
        prompt: "Reply with the single word OK.".into(),
        timeout: TEST_TIMEOUT,
        transcript: None,
    };
    match host.run(request, futures::future::pending(), |_| {}).await {
        Outcome::Done(reply) if !reply.trim().is_empty() => Ok(version),
        Outcome::Done(_) => Err(format!(
            "{} answered with nothing. {}",
            backend.name(),
            backend.login_help()
        )),
        Outcome::Failed { message, .. } => Err(message),
        Outcome::Canceled => Err(CANCELED.into()),
    }
}

#[cfg(test)]
#[path = "process_tests.rs"]
pub(crate) mod tests;
