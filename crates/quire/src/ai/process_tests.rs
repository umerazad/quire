//! The reference `tests/ai-runner.test.ts` cases for transforms, run against the fake CLI built
//! from `examples/fake_ai_cli.rs` with recorded streams. No test reaches an installed CLI: every
//! host searches only its own folders.

use super::*;
use std::{path::PathBuf, time::Duration};

pub(crate) fn fake_cli() -> PathBuf {
    let exe = std::env::current_exe().unwrap();
    let fake = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("examples")
        .join(if cfg!(windows) {
            "fake_ai_cli.exe"
        } else {
            "fake_ai_cli"
        });
    assert!(
        fake.exists(),
        "The fake CLI is missing at {}. Run the tests through cargo test, which builds examples.",
        fake.display()
    );
    fake
}

pub(crate) fn fixture(name: &str) -> String {
    format!(
        "{}/../quire_core/tests/fixtures/ai/{name}",
        env!("CARGO_MANIFEST_DIR")
    )
}

pub(crate) struct Bench {
    pub folder: tempfile::TempDir,
}

impl Bench {
    pub fn new() -> Self {
        Self {
            folder: tempfile::tempdir().unwrap(),
        }
    }

    pub fn log(&self) -> PathBuf {
        self.folder.path().join("calls.jsonl")
    }

    pub fn host(&self, fixture_name: &str, extra: &[(&str, &str)]) -> Host {
        let mut env = vec![
            ("FAKE_AI_FIXTURE".to_owned(), fixture(fixture_name)),
            (
                "FAKE_AI_LOG".to_owned(),
                self.log().to_string_lossy().into_owned(),
            ),
        ];
        env.extend(
            extra
                .iter()
                .map(|(key, value)| ((*key).to_owned(), (*value).to_owned())),
        );
        Host {
            search: Some(Vec::new()),
            env,
        }
    }

    pub fn calls(&self) -> Vec<serde_json::Value> {
        std::fs::read_to_string(self.log())
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }
}

fn request(backend: Backend) -> Request {
    Request {
        backend,
        binary: fake_cli().to_string_lossy().into_owned(),
        model: String::new(),
        prompt: "Tighten: apples".into(),
        timeout: TRANSFORM_TIMEOUT,
        transcript: None,
    }
}

fn run(host: &Host, request: Request) -> (Vec<Update>, Outcome) {
    let mut updates = Vec::new();
    let outcome = smol::block_on(host.run(request, futures::future::pending(), |update| {
        updates.push(update)
    }));
    (updates, outcome)
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn streams_claude_tokens_in_order_and_finishes_with_the_result() {
    let bench = Bench::new();
    let (updates, outcome) = run(
        &bench.host("claude-transform.jsonl", &[]),
        request(Backend::Claude),
    );
    assert_eq!(
        updates,
        ["Ap", "ples are good", " and tasty."]
            .map(|text| Update::Token(text.into()))
            .to_vec()
    );
    assert_eq!(outcome, Outcome::Done("Apples are good and tasty.".into()));
    let calls = bench.calls();
    assert_eq!(calls[0]["stdin"], "Tighten: apples");
    let argv: Vec<String> = serde_json::from_value(calls[0]["argv"].clone()).unwrap();
    assert!(
        argv.windows(4)
            .any(|window| window == ["--tools", "", "--max-turns", "1"])
    );
    let cwd = PathBuf::from(calls[0]["cwd"].as_str().unwrap());
    assert!(
        cwd.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("quire-ai-")
    );
    assert!(!cwd.exists(), "the run's working folder is removed");
}

#[test]
fn runs_a_codex_transform_with_its_rules_on_standard_input() {
    let bench = Bench::new();
    let (updates, outcome) = run(
        &bench.host("codex-transform.jsonl", &[]),
        request(Backend::Codex),
    );
    assert_eq!(
        updates,
        [Update::Token("Apples are good and tasty.".into())]
    );
    assert_eq!(outcome, Outcome::Done("Apples are good and tasty.".into()));
    let call = &bench.calls()[0];
    let argv: Vec<String> = serde_json::from_value(call["argv"].clone()).unwrap();
    assert_eq!(argv[..4], ["exec", "--json", "--sandbox", "read-only"]);
    assert!(argv.contains(&"--ephemeral".into()) && !argv.contains(&"resume".into()));
    let stdin = call["stdin"].as_str().unwrap();
    assert!(stdin.starts_with("You edit one range") && stdin.ends_with("\n\nTighten: apples"));
}

#[test]
fn skips_malformed_lines_and_names_the_versions_when_no_result_arrives() {
    let bench = Bench::new();
    let (_, outcome) = run(
        &bench.host("claude-drift.jsonl", &[]),
        request(Backend::Claude),
    );
    assert_eq!(
        outcome,
        Outcome::Failed {
            message: "Claude Code 2.1.312 ended without a result Quire could read and sent no text. Quire expects Claude Code 2.1.312; install that version or update Quire.".into(),
            retry: false
        }
    );
    let (_, outcome) = run(
        &bench.host(
            "claude-drift.jsonl",
            &[("FAKE_AI_VERSION", "codex-cli 0.199.0")],
        ),
        request(Backend::Codex),
    );
    assert!(
        matches!(outcome, Outcome::Failed { message, .. } if message.starts_with(
            "Codex 0.199.0 ended without a result Quire could read and sent no text. Quire expects Codex 0.164.0"
        ))
    );
}

#[test]
fn explains_a_failed_exit_and_an_expired_login() {
    let bench = Bench::new();
    let (_, outcome) = run(
        &bench.host(
            "claude-drift.jsonl",
            &[("FAKE_AI_EXIT", "3"), ("FAKE_AI_STDERR", "boom happened")],
        ),
        request(Backend::Claude),
    );
    assert!(
        matches!(&outcome, Outcome::Failed { message, .. } if message.contains("exited with code 3: boom happened")),
        "{outcome:?}"
    );
    let (_, outcome) = run(
        &bench.host(
            "claude-drift.jsonl",
            &[
                ("FAKE_AI_EXIT", "1"),
                ("FAKE_AI_STDERR", "Invalid API key · Please run /login"),
            ],
        ),
        request(Backend::Claude),
    );
    assert!(
        matches!(&outcome, Outcome::Failed { message, .. } if message.contains("Run `claude` in a terminal and sign in with /login")),
        "{outcome:?}"
    );
}

#[test]
fn explains_a_missing_binary_and_finds_one_on_the_search_path() {
    let bench = Bench::new();
    let missing = bench.folder.path().join("nope");
    let (_, outcome) = run(
        &bench.host("claude-transform.jsonl", &[]),
        Request {
            binary: missing.to_string_lossy().into_owned(),
            ..request(Backend::Claude)
        },
    );
    assert_eq!(
        outcome,
        Outcome::Failed {
            message: format!(
                "Claude Code was not found at {}. Correct Binary path in Settings > AI, or clear it to detect automatically, then press Test.",
                missing.display()
            ),
            retry: false
        }
    );
    let (_, outcome) = run(
        &bench.host("claude-transform.jsonl", &[]),
        Request {
            binary: String::new(),
            ..request(Backend::Codex)
        },
    );
    assert!(
        matches!(&outcome, Outcome::Failed { message, .. } if message.starts_with("Codex was not found on this computer's PATH."))
    );
    let folder = bench.folder.path().join("bin");
    std::fs::create_dir(&folder).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(fake_cli(), folder.join("claude")).unwrap();
    let host = Host {
        search: Some(vec![folder.clone()]),
        ..bench.host("claude-transform.jsonl", &[])
    };
    assert_eq!(
        smol::block_on(host.locate(Backend::Claude, "")).unwrap(),
        folder.join("claude")
    );
    let (_, outcome) = run(
        &host,
        Request {
            binary: String::new(),
            ..request(Backend::Claude)
        },
    );
    assert_eq!(outcome, Outcome::Done("Apples are good and tasty.".into()));
}

#[test]
fn cancels_a_run_and_ends_its_whole_process_tree() {
    let bench = Bench::new();
    let pids = bench.folder.path().join("pids");
    let host = bench.host(
        "claude-transform.jsonl",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_FIXTURE", &fixture("claude-drift.jsonl")),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
    );
    let (cancel, canceled) = futures::channel::oneshot::channel::<()>();
    let watcher = std::thread::spawn({
        let pids = pids.clone();
        move || {
            wait_until("the fake CLI started its grandchild", || pids.exists());
            cancel.send(()).ok();
        }
    });
    let outcome = smol::block_on(host.run(request(Backend::Claude), canceled.map(|_| ()), |_| {}));
    watcher.join().unwrap();
    assert_eq!(outcome, Outcome::Canceled);
    let ids = std::fs::read_to_string(&pids).unwrap();
    for pid in ids.split_whitespace() {
        wait_until(&format!("process {pid} ended"), || !alive(pid));
    }
}

#[test]
fn dropping_the_cancel_handle_ends_the_run() {
    let bench = Bench::new();
    let pids = bench.folder.path().join("pids");
    let host = bench.host(
        "claude-drift.jsonl",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
    );
    let (cancel, canceled) = futures::channel::oneshot::channel::<()>();
    let watcher = std::thread::spawn({
        let pids = pids.clone();
        move || {
            wait_until("the fake CLI started", || pids.exists());
            drop(cancel);
        }
    });
    let outcome = smol::block_on(host.run(request(Backend::Claude), canceled.map(|_| ()), |_| {}));
    watcher.join().unwrap();
    assert_eq!(outcome, Outcome::Canceled);
    for pid in std::fs::read_to_string(&pids).unwrap().split_whitespace() {
        wait_until(&format!("process {pid} ended"), || !alive(pid));
    }
}

#[test]
fn dropping_the_run_ends_its_process_tree_and_removes_its_folder() {
    let bench = Bench::new();
    let pids = bench.folder.path().join("pids");
    let host = bench.host(
        "claude-drift.jsonl",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
    );
    let started = async {
        while !pids.exists() {
            smol::Timer::after(Duration::from_millis(20)).await;
        }
    };
    let run = host.run(request(Backend::Claude), futures::future::pending(), |_| {});
    smol::block_on(async {
        futures::pin_mut!(run, started);
        assert!(matches!(
            futures::future::select(run, started).await,
            futures::future::Either::Right(_)
        ));
    });
    for pid in std::fs::read_to_string(&pids).unwrap().split_whitespace() {
        wait_until(&format!("process {pid} ended"), || !alive(pid));
    }
    let cwd = bench.calls()[0]["cwd"].as_str().unwrap().to_owned();
    assert!(!Path::new(&cwd).exists(), "{cwd} outlived the run");
}

/// Installed CLIs only: finds `claude` and `codex` through the login shell's PATH and reads their
/// versions. It never sends a prompt. Run with `cargo test ... -- --ignored installed_clis`.
#[test]
#[ignore = "uses the CLIs installed on this machine; run with --ignored"]
fn installed_clis_are_found_on_the_login_path_and_report_versions() {
    let host = Host::default();
    for backend in [Backend::Claude, Backend::Codex] {
        let binary =
            smol::block_on(host.locate(backend, "")).unwrap_or_else(|error| panic!("{error}"));
        let version = smol::block_on(host.version(backend, &binary))
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(
            version.split('.').count() >= 3,
            "{} reported {version}",
            binary.display()
        );
        println!("{}: {} {version}", backend.name(), binary.display());
    }
}

#[test]
fn times_out_a_slow_run_and_offers_a_retry() {
    let bench = Bench::new();
    let (_, outcome) = run(
        &bench.host("claude-drift.jsonl", &[("FAKE_AI_HANG", "1")]),
        Request {
            timeout: Duration::from_millis(300),
            ..request(Backend::Claude)
        },
    );
    assert_eq!(
        outcome,
        Outcome::Failed {
            message: "Claude Code did not finish within 0 seconds. Retry, or run the command on a shorter range.".into(),
            retry: true
        }
    );
}

#[test]
fn honors_a_cancel_that_arrives_before_the_process_starts() {
    let bench = Bench::new();
    let outcome = smol::block_on(bench.host("claude-transform.jsonl", &[]).run(
        request(Backend::Claude),
        futures::future::ready(()),
        |_| {},
    ));
    assert_eq!(outcome, Outcome::Canceled);
    assert!(!bench.log().exists(), "the CLI never started");
}

#[test]
fn keeps_the_prompt_and_final_response_in_the_transcript_folder() {
    let bench = Bench::new();
    let folder = bench.folder.path().join("ai");
    let (_, outcome) = run(
        &bench.host("claude-transform.jsonl", &[]),
        Request {
            transcript: Some(Transcript {
                folder: folder.clone(),
                id: "ai-1/../x".into(),
                page: "Fruit.md".into(),
            }),
            ..request(Backend::Claude)
        },
    );
    assert!(matches!(outcome, Outcome::Done(_)));
    let day = std::fs::read_dir(&folder).unwrap().next().unwrap().unwrap();
    let file = std::fs::read_dir(day.path())
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    let name = file.file_name().to_string_lossy().into_owned();
    assert!(name.ends_with("-ai-1x.md"), "{name}");
    let text = std::fs::read_to_string(file.path()).unwrap();
    assert!(text.starts_with("# AI transform\n\n- Backend: Claude Code 2.1.312\n"));
    assert!(text.contains("- Page: Fruit.md"));
    assert!(text.contains("## Prompt\n\nTighten: apples\n"));
    assert!(text.ends_with("## Response\n\nApples are good and tasty.\n"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }
}

#[test]
fn tests_a_backend_by_its_version_and_a_one_word_answer() {
    let bench = Bench::new();
    let host = bench.host("claude-transform.jsonl", &[]);
    let fake = fake_cli().to_string_lossy().into_owned();
    assert_eq!(
        smol::block_on(test(&host, Backend::Claude, &fake, "")),
        Ok("2.1.312".into())
    );
    assert_eq!(bench.calls()[0]["stdin"], "Reply with the single word OK.");
    let empty = bench.folder.path().join("empty.jsonl");
    std::fs::write(
        &empty,
        r#"{"type":"result","subtype":"success","is_error":false,"result":"  "}"#,
    )
    .unwrap();
    let host = bench.host("", &[("FAKE_AI_FIXTURE", &empty.to_string_lossy())]);
    assert_eq!(
        smol::block_on(test(&host, Backend::Claude, &fake, "")),
        Err("Claude Code answered with nothing. Run `claude` in a terminal and sign in with /login, then retry.".into())
    );
}
