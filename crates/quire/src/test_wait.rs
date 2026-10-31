//! Waiting on background work in tests. Tests poll for state rather than sleeping, so
//! [`PATIENCE`] only bounds a hang; it is generous because full suites share the machine with
//! other builds.

use gpui::TestAppContext;
use std::time::{Duration, Instant};

pub const PATIENCE: Duration = Duration::from_secs(60);

/// Runs `step`, then checks `done`, until it holds, sleeping `pause` of real time between looks
/// so storage, index, and process threads can answer. `cx` is the app under test, or `()` for
/// work outside one. Panics with `describe` once [`PATIENCE`]
/// passes, so a hang names what never arrived.
pub fn until<C>(
    cx: &mut C,
    pause: Duration,
    mut step: impl FnMut(&mut C),
    mut done: impl FnMut(&mut C) -> bool,
    describe: impl FnOnce(&mut C) -> String,
) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        step(cx);
        if done(cx) {
            return;
        }
        if Instant::now() >= deadline {
            panic!("timed out waiting for {}", describe(cx));
        }
        std::thread::sleep(pause);
    }
}

/// A step that lets the app's executor finish its ready work without moving the fake clock.
pub fn park(cx: &mut TestAppContext) {
    cx.run_until_parked();
}

/// A step that also moves the fake clock by `by` first, so debounces and timers fire.
pub fn advance(by: Duration) -> impl Fn(&mut TestAppContext) {
    move |cx| {
        cx.executor().advance_clock(by);
        cx.run_until_parked();
    }
}
