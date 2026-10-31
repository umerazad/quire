//! What closing a window, quitting, or switching profiles waits for, and when it asks instead of
//! waiting. Pages still being recorded and running backups or Git syncs report an [`Activity`];
//! a close waits while every activity is busy, asks after [`PATIENCE`], and asks at once when
//! one has failed, so a stalled service never loses data silently or holds a close forever.

use gpui::{App, Global, SharedString};
use std::time::Duration;

/// How long a close waits quietly before asking, as the reference app's close coordinator does.
pub const PATIENCE: Duration = Duration::from_secs(3);
/// How often a waiting close looks again.
pub const POLL: Duration = Duration::from_millis(100);

pub const KEEP_WAITING: &str = "Keep waiting";
pub const KEEP_EDITING: &str = "Keep editing";
pub const CLOSE_ANYWAY: &str = "Close anyway";
pub const STILL_WORKING: &str = "Quire is still finishing work for this notebook.";
pub const FAILED: &str = "Some work for this notebook did not finish.";
pub const CLOSE_ANYWAY_DETAIL: &str = "Closing now keeps everything already saved locally or recorded for recovery; recovery copies reopen with the notebook. Typing not yet recorded is lost. An interrupted backup leaves only a hidden partial folder, and an interrupted Git sync keeps every note and commit in the folder.";

/// Why a close asks when a page in conflict with its file holds typing not yet recorded.
pub fn unrecorded_conflict(path: &str) -> String {
    format!(
        "{path} changed outside Quire while it had unsaved edits, and the newest of those edits are not yet recorded for recovery. Keep editing to choose a version in the page's banner."
    )
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Activity {
    Idle,
    Busy(SharedString),
    Failed(SharedString),
}

impl Activity {
    /// The combined state: any failure first, then anything busy.
    pub fn all(activities: impl IntoIterator<Item = Activity>) -> Activity {
        let (mut busy, mut failed) = (Vec::new(), Vec::new());
        for activity in activities {
            match activity {
                Activity::Idle => {}
                Activity::Busy(label) => busy.push(label),
                Activity::Failed(label) => failed.push(label),
            }
        }
        let join = |labels: Vec<SharedString>| -> SharedString {
            labels
                .iter()
                .map(|label| label.as_ref())
                .collect::<Vec<_>>()
                .join(" ")
                .into()
        };
        if !failed.is_empty() {
            Activity::Failed(join(failed))
        } else if !busy.is_empty() {
            Activity::Busy(join(busy))
        } else {
            Activity::Idle
        }
    }
}

/// Work a window's close waits for besides its pages, such as a backup or a Git sync.
pub type Service = dyn Fn(&App) -> Activity;

/// Set by Close anyway on a quit, so every window lets the quit through.
#[derive(Default)]
struct Forced(bool);

impl Global for Forced {}

/// Whether the person chose Close anyway on a quit.
pub fn quit_forced(cx: &App) -> bool {
    cx.try_global::<Forced>().is_some_and(|forced| forced.0)
}

/// Lets the next quit through every window's guard.
pub fn force_quit(cx: &mut App) {
    cx.set_global(Forced(true));
    cx.quit();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn failures_come_before_busy_work_and_idle_needs_everything_idle() {
        assert_eq!(Activity::all([]), Activity::Idle);
        assert_eq!(
            Activity::all([
                Activity::Idle,
                Activity::Busy("A backup is running.".into())
            ]),
            Activity::Busy("A backup is running.".into())
        );
        assert_eq!(
            Activity::all([
                Activity::Busy("A backup is running.".into()),
                Activity::Failed("Storage stopped.".into()),
                Activity::Failed("Git failed.".into()),
            ]),
            Activity::Failed("Storage stopped. Git failed.".into())
        );
    }
}
