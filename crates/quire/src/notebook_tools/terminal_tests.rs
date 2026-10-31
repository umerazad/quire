//! The reference `tests/ai-runner.test.ts` Use from a terminal cases, against the fake CLI: no
//! test reaches an installed CLI or the writer's own `~/.claude` or `~/.codex`.

use super::terminal::*;
use crate::ai::{
    process::{
        Host,
        tests::{Bench, fake_cli},
    },
    settings::{
        registered,
        tests::{init_ai, pass_test, wait_until},
    },
};
use gpui::TestAppContext;
use quire_core::ai::Backend;
use serde_json::{Value, json};
use std::path::PathBuf;

fn server() -> Server {
    Server {
        command: PathBuf::from("/Applications/Quire.app/Contents/MacOS/quire"),
        bridge: PathBuf::from("/Users/me/Library/Application Support/Quire/bridge.json"),
    }
}

#[test]
fn builds_the_reference_registration_commands() {
    let server = server();
    let claude = terminal_args(Backend::Claude, &server, false);
    assert_eq!(
        shell_line("claude", &claude),
        r#"claude mcp add-json quire '{"type":"stdio","command":"/Applications/Quire.app/Contents/MacOS/quire","args":["--mcp-server"],"env":{"QUIRE_BRIDGE":"/Users/me/Library/Application Support/Quire/bridge.json"}}' --scope user"#
    );
    assert_eq!(
        shell_line("claude", &terminal_args(Backend::Claude, &server, true)),
        "claude mcp remove quire --scope user"
    );
    assert_eq!(
        shell_line("codex", &terminal_args(Backend::Codex, &server, false)),
        "codex mcp add quire --env 'QUIRE_BRIDGE=/Users/me/Library/Application Support/Quire/bridge.json' -- /Applications/Quire.app/Contents/MacOS/quire --mcp-server"
    );
    assert_eq!(
        shell_line("codex", &terminal_args(Backend::Codex, &server, true)),
        "codex mcp remove quire"
    );
    assert_eq!(
        shell_line("a", &["it's".into(), String::new()]),
        r"a 'it'\''s' ''"
    );
}

/// A folder holding the fake CLI as `claude` and `codex`, and a host that searches only it.
fn installed(bench: &Bench, extra: &[(&str, &str)]) -> Host {
    for name in ["claude", "codex"] {
        std::os::unix::fs::symlink(fake_cli(), bench.folder.path().join(name)).unwrap();
    }
    Host {
        search: Some(vec![bench.folder.path().to_owned()]),
        ..bench.host("claude-transform.jsonl", extra)
    }
}

fn argv(bench: &Bench) -> Vec<Value> {
    bench
        .calls()
        .into_iter()
        .map(|call| call["argv"].clone())
        .collect()
}

fn binaries() -> Vec<(Backend, String)> {
    vec![
        (Backend::Claude, String::new()),
        (Backend::Codex, String::new()),
    ]
}

#[test]
fn registers_with_each_cli_after_removing_an_older_entry_then_removes() {
    let bench = Bench::new();
    let host = installed(&bench, &[]);
    let server = server();
    let (outcomes, terminal) = smol::block_on(register(
        &host,
        &binaries(),
        Default::default(),
        &server,
        false,
    ));
    let strings = |args: Vec<String>| json!(args);
    assert_eq!(
        argv(&bench),
        [
            strings(terminal_args(Backend::Claude, &server, true)),
            strings(terminal_args(Backend::Claude, &server, false)),
            strings(terminal_args(Backend::Codex, &server, true)),
            strings(terminal_args(Backend::Codex, &server, false)),
        ]
    );
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| (outcome.ok, outcome.message.as_str()))
            .collect::<Vec<_>>(),
        [
            (
                true,
                "Claude Code sessions in a terminal can now use Quire's tools while the app is open."
            ),
            (
                true,
                "Codex sessions in a terminal can now use Quire's tools while the app is open."
            ),
        ]
    );
    assert_eq!(
        terminal.get("codex").map(String::as_str),
        Some(shell_line("codex", &terminal_args(Backend::Codex, &server, false)).as_str())
    );
    let mut only_claude = terminal.clone();
    only_claude.remove("codex");
    let (outcomes, terminal) =
        smol::block_on(register(&host, &binaries(), only_claude, &server, true));
    assert_eq!(
        outcomes
            .iter()
            .map(|outcome| outcome.message.as_str())
            .collect::<Vec<_>>(),
        ["Removed Quire from Claude Code."],
        "only CLIs Quire registered with are touched"
    );
    assert!(terminal.is_empty());
    assert_eq!(
        argv(&bench).last().unwrap(),
        &strings(terminal_args(Backend::Claude, &server, true))
    );
}

#[test]
fn reports_each_cli_that_failed_and_keeps_what_stands() {
    let bench = Bench::new();
    let host = installed(
        &bench,
        &[
            ("FAKE_AI_EXIT", "1"),
            ("FAKE_AI_STDERR", "boom\nmore detail"),
        ],
    );
    let missing = bench.folder.path().join("nowhere").join("codex");
    let binaries = vec![
        (Backend::Claude, String::new()),
        (Backend::Codex, missing.to_string_lossy().into_owned()),
    ];
    let (outcomes, terminal) = smol::block_on(register(
        &host,
        &binaries,
        Default::default(),
        &server(),
        false,
    ));
    assert!(terminal.is_empty());
    assert_eq!(
        outcomes[0].message,
        "Claude Code could not register the server: boom. Run the command in a terminal to see the full error."
    );
    assert!(outcomes[0].command.is_some());
    assert_eq!(
        outcomes[1].message,
        quire_core::ai::backend::missing_binary(Backend::Codex, Some(&binaries[1].1))
    );
    assert!(!outcomes[1].ok && outcomes[1].command.is_none());
}

#[gpui::test]
async fn registers_only_when_asked_with_ai_commands_on_and_remembers_it(cx: &mut TestAppContext) {
    let bench = Bench::new();
    let _fs = init_ai(&bench, cx).await;
    cx.update(|cx| start(false, cx));
    cx.run_until_parked();
    assert!(
        bench.calls().is_empty(),
        "Register does nothing while AI commands are off"
    );
    pass_test(cx);
    let calls = bench.calls().len();
    cx.update(|cx| start(false, cx));
    wait_until(cx, "registration", |cx| {
        cx.read(|cx| !cx.global::<Registration>().running)
    });
    let fake = fake_cli();
    let name = fake.file_name().unwrap().to_string_lossy().into_owned();
    let saved = cx.read(registered);
    let bridge = cx.read(|cx| cx.global::<Registration>().server.clone());
    assert_eq!(
        saved.get("claude"),
        Some(&shell_line(
            &name,
            &terminal_args(Backend::Claude, &bridge, false)
        ))
    );
    assert_eq!(
        argv(&bench)[calls..],
        [
            json!(terminal_args(Backend::Claude, &bridge, true)),
            json!(terminal_args(Backend::Claude, &bridge, false)),
        ]
    );
    let outcomes = cx.read(|cx| cx.global::<Registration>().outcomes.clone());
    assert!(outcomes[0].ok);
    assert!(
        !outcomes[1].ok,
        "Codex is not installed in the test's search folders"
    );
    cx.update(|cx| start(true, cx));
    wait_until(cx, "removal", |cx| cx.read(registered).is_empty());
}

/// Registers with the real CLIs on this computer, changing the writer's own user settings, then
/// removes the entry again. Run by hand with `--ignored` only.
#[test]
#[ignore = "changes the real ~/.claude and ~/.codex settings"]
fn registers_with_the_installed_clis() {
    let host = Host::default();
    let server = server();
    let (outcomes, terminal) = smol::block_on(register(
        &host,
        &binaries(),
        Default::default(),
        &server,
        false,
    ));
    assert!(outcomes.iter().any(|outcome| outcome.ok), "{outcomes:?}");
    let (removed, left) = smol::block_on(register(&host, &binaries(), terminal, &server, true));
    assert!(left.is_empty(), "{removed:?}");
}
