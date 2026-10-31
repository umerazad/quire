//! When a Quire notebook publishes and imports, ported from the reference engine's sync scheduler
//! (`engine.ts`, `replica.ts`, and the `main.ts` watcher at `rt` 0.9.53).
//!
//! Saves publish after a quiet spell, so a burst of saves makes one change file, but never later
//! than a deadline set by the first of them. While another Quire app on this computer has the
//! notebook open, it reads the folder directly, so saves go out after 150 ms (at most 1 s);
//! otherwise after 1 s (at most 5 s), which writes fewer change files. Each writer announces
//! itself in `devices/<device>/status.json`, rewritten only when its count of unpublished saves
//! changes or once a minute as a heartbeat. Everything here takes the time as an argument, so
//! callers drive it with any clock.

use crate::{durable_files, notebook::RescanProblem};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    io::{Read as _, Write as _},
    path::{Component, Path},
    time::Duration,
};

/// How often an unchanged status is rewritten to show this app is still running.
pub const HEARTBEAT_EVERY: u64 = 60_000;
/// A status with this computer's label and a heartbeat this recent means another app is nearby.
pub const NEARBY_WITHIN: u64 = 2 * HEARTBEAT_EVERY;
/// How long a watcher event waits for others before an import starts.
pub const WATCH_DELAY: Duration = Duration::from_millis(50);
/// When an import that hit an unreadable change file tries again; a sync tool may still be
/// writing it.
pub const RETRY_AFTER: Duration = Duration::from_secs(1);
/// How often the folder is rescanned and the heartbeat refreshed without any event.
pub const RESCAN_EVERY: Duration = Duration::from_secs(30);
/// The largest status file read; real ones are a few hundred bytes.
const STATUS_LIMIT: u64 = 64 * 1024;
const STATUS_FILE: &str = "status.json";

/// How long saves wait to publish: `idle` after the last one, at most `latest` after the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Window {
    pub idle: u64,
    pub latest: u64,
}

/// Another Quire app on this computer reads the folder directly: publish at once.
pub const SOON: Window = Window {
    idle: 150,
    latest: 1_000,
};
/// Alone on this computer: a sync tool takes seconds between computers anyway.
pub const CALM: Window = Window {
    idle: 1_000,
    latest: 5_000,
};

/// The deadline of the saves waiting to publish.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublishTimer {
    deadline: Option<u64>,
}

impl PublishTimer {
    /// A save landed at `now`: how many milliseconds until publishing.
    pub fn saved(&mut self, now: u64, nearby: bool) -> u64 {
        let Window { idle, latest } = if nearby { SOON } else { CALM };
        let deadline = *self.deadline.get_or_insert(now.saturating_add(latest));
        idle.min(deadline.saturating_sub(now))
    }

    /// Publishing started; the next save starts a new deadline.
    pub fn started(&mut self) {
        self.deadline = None;
    }
}

/// What one writer says about itself in `devices/<device>/status.json`. The field names and
/// meanings match the desktop app and the terminal app, which sets `label` to the host name and
/// `app` to `quire-tui <version>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceStatus {
    pub format: u64,
    pub app: String,
    pub device: String,
    /// The computer, as its host name; several writers on one computer share it.
    pub label: String,
    pub last_seq: u64,
    /// Saves not yet published.
    pub outbox: u64,
    /// Milliseconds since the epoch when this was written.
    pub heartbeat: u64,
}

/// The largest integer JavaScript represents exactly; the reference app accepts only these.
const SAFE_INTEGER: u64 = (1 << 53) - 1;

impl DeviceStatus {
    fn valid(&self) -> bool {
        [self.format, self.last_seq, self.outbox, self.heartbeat]
            .iter()
            .all(|count| *count <= SAFE_INTEGER)
    }
}

/// The status last written, so an unchanged one is rewritten only as a heartbeat.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StatusMemory {
    written: Option<(u64, u64)>,
}

impl StatusMemory {
    /// Whether a status with `outbox` unpublished saves should be written at `now`.
    pub fn due(&self, outbox: u64, now: u64) -> bool {
        self.written.is_none_or(|(written, at)| {
            written != outbox || now.saturating_sub(at) >= HEARTBEAT_EVERY
        })
    }

    pub fn wrote(&mut self, outbox: u64, at: u64) {
        self.written = Some((outbox, at));
    }
}

/// Whether another Quire app runs on this computer: a status from another writer with this
/// computer's label and a heartbeat from the last two minutes.
pub fn nearby(statuses: &[DeviceStatus], device: &str, label: &str, now: u64) -> bool {
    statuses.iter().any(|status| {
        status.device != device
            && status.label == label
            && now.saturating_sub(status.heartbeat) < NEARBY_WITHIN
    })
}

/// Problems an import reported, so each is reported only once it survives a retry: a change file
/// caught mid-delivery is read again quietly.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProblemMemory {
    seen: BTreeSet<String>,
}

impl ProblemMemory {
    /// Records this import's problems and returns the warnings for those the previous import
    /// also hit.
    pub fn survived(&mut self, problems: &[RescanProblem]) -> Vec<String> {
        let previous = std::mem::take(&mut self.seen);
        problems
            .iter()
            .filter_map(|problem| {
                let key = format!(
                    "{}\n{}\n{}",
                    problem.device.as_deref().unwrap_or_default(),
                    problem.file.as_deref().unwrap_or_default(),
                    problem.reason
                );
                let warn = previous.contains(&key);
                self.seen.insert(key);
                warn.then(|| warning(problem))
            })
            .collect()
    }
}

fn warning(problem: &RescanProblem) -> String {
    let file = problem
        .file
        .as_deref()
        .map(|file| format!(" ({file})"))
        .unwrap_or_default();
    format!(
        "Changes from another computer could not be read{file}: {}. Quire will try again.",
        problem.reason.trim_end_matches('.')
    )
}

/// One import at a time; a request that arrives while one runs starts another once it ends,
/// since the running one may have listed the folder before the new file landed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Imports {
    running: bool,
    again: bool,
}

impl Imports {
    /// An import was asked for: whether to start one now.
    pub fn request(&mut self) -> bool {
        if self.running {
            self.again = true;
            false
        } else {
            self.running = true;
            true
        }
    }

    /// The running import ended: whether to start another.
    pub fn finished(&mut self) -> bool {
        self.running = std::mem::take(&mut self.again);
        self.running
    }

    pub fn running(&self) -> bool {
        self.running
    }
}

/// Whether a changed path is a file other writers deliver: inside `devices/` of the notebook
/// folder `root` and with no part starting with a dot, which marks temporary files.
pub fn delivered(root: &Path, path: &Path) -> bool {
    let devices = root.join("devices");
    path.strip_prefix(&devices).is_ok_and(|relative| {
        relative.components().all(|part| match part {
            Component::Normal(name) => !name.to_string_lossy().starts_with('.'),
            _ => false,
        })
    })
}

/// Writes this writer's status, replacing the previous one in one rename. The temporary file
/// starts with a dot, so watchers ignore it.
pub fn write_status(root: &Path, status: &DeviceStatus) -> Result<()> {
    ensure!(
        status.valid(),
        "This writer's status has a count too large to record. Reopen the notebook and retry."
    );
    let failed = || {
        format!(
            "Cannot write this computer's status in {}. Check that the synced folder is available and writable; Quire tries again within a minute.",
            root.display()
        )
    };
    let devices = durable_files::directory_below(root, "devices").with_context(failed)?;
    let folder = durable_files::directory_below(&devices, &status.device).with_context(failed)?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".status-")
        .tempfile_in(&folder)
        .with_context(failed)?;
    temporary
        .write_all(&serde_json::to_vec(status)?)
        .and_then(|()| temporary.as_file().sync_all())
        .with_context(failed)?;
    temporary
        .persist(folder.join(STATUS_FILE))
        .map_err(|error| error.error)
        .with_context(failed)?;
    durable_files::sync_directory(&folder).with_context(failed)?;
    Ok(())
}

/// Every writer's status in the notebook folder. A status that is unreadable, incomplete, or
/// names another writer is skipped: one mid-delivery is read again next time.
pub fn read_statuses(root: &Path) -> Vec<DeviceStatus> {
    let Ok(entries) = fs::read_dir(root.join("devices")) else {
        return Vec::new();
    };
    let mut statuses: Vec<DeviceStatus> = entries
        .flatten()
        .filter_map(|entry| {
            let device = entry.file_name().into_string().ok()?;
            if device.starts_with('.') || !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let file = fs::File::open(entry.path().join(STATUS_FILE)).ok()?;
            if !file.metadata().ok()?.is_file() {
                return None;
            }
            let mut data = Vec::new();
            file.take(STATUS_LIMIT + 1).read_to_end(&mut data).ok()?;
            let status: DeviceStatus = serde_json::from_slice(&data).ok()?;
            (data.len() as u64 <= STATUS_LIMIT && status.valid() && status.device == device)
                .then_some(status)
        })
        .collect();
    statuses.sort_by(|a, b| a.device.cmp(&b.device));
    statuses
}

/// This computer's name for statuses: its host name, as the desktop and terminal apps use, so
/// every Quire app on one computer agrees.
pub fn host_label() -> String {
    #[cfg(unix)]
    {
        let mut buffer = [0u8; 256];
        // SAFETY: the buffer is valid for its whole length, and the result is read only up to
        // its first NUL byte.
        let result = unsafe { libc::gethostname(buffer.as_mut_ptr().cast(), buffer.len()) };
        if result == 0 {
            let end = buffer.iter().position(|byte| *byte == 0).unwrap_or(0);
            if let Ok(name) = std::str::from_utf8(&buffer[..end])
                && !name.is_empty()
            {
                return name.to_owned();
            }
        }
    }
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "this computer".into())
}

#[cfg(test)]
mod tests;
