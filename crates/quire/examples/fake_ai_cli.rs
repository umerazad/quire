//! A stand-in for the Claude Code and Codex CLIs in tests, replaying a recorded event stream the
//! way the reference `tests/fixtures/ai/fake-cli.mjs` does. Environment variables shape the run:
//! FAKE_AI_FIXTURE names the stream, FAKE_AI_LOG records argv, stdin, and the working folder,
//! FAKE_AI_DELAY_MS waits between lines, FAKE_AI_STDERR and FAKE_AI_EXIT end the run,
//! FAKE_AI_HANG keeps running after the stream, FAKE_AI_VERSION answers `--version`, and
//! FAKE_AI_PIDS starts a grandchild and records both process IDs, so tests can check that a
//! cancel ends the whole process tree.

use std::{
    io::{Read, Write},
    time::Duration,
};

fn var(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
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
        let child = std::process::Command::new(std::env::current_exe().unwrap())
            .arg("--sleep")
            .spawn()
            .expect("the fake CLI could not start its grandchild");
        std::fs::write(pids, format!("{} {}", std::process::id(), child.id())).ok();
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
