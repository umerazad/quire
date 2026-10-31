//! Code formatting with Prettier 3.9.10's own standalone bundles, the formatter rt calls, run in
//! an embedded QuickJS-ng engine on one dedicated thread. The bundles are vendored under
//! `vendor/prettier` and compiled into the binary, so formatting needs no installed runtime and
//! no download. Every request can be cancelled, and size, time, and memory limits stop a request
//! that would otherwise hold the thread.

use futures::channel::oneshot;
use std::{
    fmt,
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    task::{Context, Poll},
    thread,
    time::Duration,
};

mod engine;

/// A Prettier parser, chosen from a fenced code block's language tag the way rt chooses it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parser {
    Babel,
    TypeScript,
    Json,
    Css,
    Html,
    Yaml,
    Markdown,
}

impl Parser {
    /// The parser for a fence tag, ignoring case, or `None` when rt has no formatter for it.
    pub fn for_tag(tag: &str) -> Option<Self> {
        Some(match tag.to_lowercase().as_str() {
            "js" | "jsx" | "javascript" => Self::Babel,
            "ts" | "tsx" | "typescript" => Self::TypeScript,
            "json" => Self::Json,
            "css" => Self::Css,
            "html" | "xml" => Self::Html,
            "yaml" | "yml" => Self::Yaml,
            "md" | "markdown" => Self::Markdown,
            _ => return None,
        })
    }

    fn name(self) -> &'static str {
        match self {
            Self::Babel => "babel",
            Self::TypeScript => "typescript",
            Self::Json => "json",
            Self::Css => "css",
            Self::Html => "html",
            Self::Yaml => "yaml",
            Self::Markdown => "markdown",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FormatError {
    /// The fence's language tag, which has no bundled formatter.
    Unsupported(String),
    TooLarge {
        bytes: usize,
        limit: usize,
    },
    /// The first line of Prettier's error, such as a syntax error with its line and column.
    Invalid(String),
    TimedOut(Duration),
    Cancelled,
    Engine(String),
}

impl fmt::Display for FormatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsupported(tag) if tag.is_empty() => write!(
                f,
                "This code block has no language tag, so Quire cannot format it. Add a tag such as `js` after the opening fence, then run Format code again."
            ),
            Self::Unsupported(tag) => write!(
                f,
                "Quire has no bundled formatter for `{tag}` code. Format it by hand; Format code supports JavaScript, TypeScript, JSON, CSS, HTML, XML, YAML, and Markdown."
            ),
            Self::TooLarge { bytes, limit } => write!(
                f,
                "Code formatting stopped: the block is {} KB and the formatter accepts at most {} KB. Split the block, then run Format code again.",
                bytes.div_ceil(1024),
                limit / 1024
            ),
            Self::Invalid(message) => write!(
                f,
                "Code formatting failed: {message}. Correct its syntax, then run Format code again."
            ),
            Self::TimedOut(limit) => write!(
                f,
                "Code formatting stopped after {}. Split the block into smaller ones, then run Format code again.",
                if limit.subsec_millis() == 0 {
                    format!("{} seconds", limit.as_secs())
                } else {
                    format!("{} ms", limit.as_millis())
                }
            ),
            Self::Cancelled => write!(f, "Code formatting was cancelled."),
            Self::Engine(message) => write!(
                f,
                "The bundled code formatter failed: {message}. Restart Quire; if this repeats, reinstall it."
            ),
        }
    }
}

impl std::error::Error for FormatError {}

#[derive(Clone, Copy, Debug)]
pub struct Limits {
    /// Larger inputs are refused before they reach the engine.
    pub max_bytes: usize,
    /// A request running longer is interrupted.
    pub timeout: Duration,
    /// The JavaScript heap limit; exceeding it fails the request.
    pub memory_bytes: usize,
}

impl Default for Limits {
    /// rt's 2,577-line `Editor.tsx` (about 100 KB) formats in about 0.8 s on Apple silicon, so
    /// these limits leave room for any code block a note plausibly holds.
    fn default() -> Self {
        Self {
            max_bytes: 512 * 1024,
            timeout: Duration::from_secs(20),
            memory_bytes: 512 * 1024 * 1024,
        }
    }
}

struct Job {
    text: String,
    parser: Parser,
    cancel: Arc<AtomicBool>,
    reply: oneshot::Sender<Result<String, FormatError>>,
}

/// Owns the formatter thread, started by the first request. Requests run one at a time in the
/// order they arrive; the engine loads lazily and is rebuilt after an interrupted request.
pub struct Formatter {
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    limits: Limits,
}

impl Formatter {
    pub fn new(limits: Limits) -> Self {
        Self {
            jobs: Mutex::new(None),
            limits,
        }
    }

    pub fn format(&self, text: impl Into<String>, parser: Parser) -> Formatting {
        let text = text.into();
        let (reply, result) = oneshot::channel();
        let formatting = Formatting {
            result,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        if text.len() > self.limits.max_bytes {
            let error = FormatError::TooLarge {
                bytes: text.len(),
                limit: self.limits.max_bytes,
            };
            reply.send(Err(error)).ok();
            return formatting;
        }
        let job = Job {
            text,
            parser,
            cancel: formatting.cancel.clone(),
            reply,
        };
        if let Err(job) = self.send(job) {
            job.reply
                .send(Err(FormatError::Engine(
                    "its thread could not start".into(),
                )))
                .ok();
        }
        formatting
    }

    fn send(&self, job: Job) -> Result<(), Job> {
        let mut jobs = self
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let job = match jobs.as_ref() {
            Some(sender) => match sender.send(job) {
                Ok(()) => return Ok(()),
                Err(mpsc::SendError(job)) => job,
            },
            None => job,
        };
        let (sender, receiver) = mpsc::channel();
        let limits = self.limits;
        let started = thread::Builder::new()
            .name("quire-format".into())
            .stack_size(32 * 1024 * 1024)
            .spawn(move || engine::serve(receiver, limits));
        if started.is_err() {
            return Err(job);
        }
        let sent = sender.send(job).map_err(|mpsc::SendError(job)| job);
        *jobs = Some(sender);
        sent
    }
}

impl Default for Formatter {
    fn default() -> Self {
        Self::new(Limits::default())
    }
}

/// A pending request. Dropping it cancels the request.
pub struct Formatting {
    result: oneshot::Receiver<Result<String, FormatError>>,
    cancel: Arc<AtomicBool>,
}

impl Formatting {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Future for Formatting {
    type Output = Result<String, FormatError>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.result).poll(cx).map(|result| {
            result.unwrap_or_else(|_| Err(FormatError::Engine("its thread stopped".into())))
        })
    }
}

impl Drop for Formatting {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests;
