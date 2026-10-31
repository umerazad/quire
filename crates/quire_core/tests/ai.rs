//! Ported from the reference `tests/ai-commands.test.ts` and the stream-parsing cases of
//! `tests/ai-runner.test.ts`, replaying its recorded CLI streams.

use quire_core::ai::{
    AiRange, Backend, Command, Event, ParseState, Placement, Verb,
    backend::{
        backend_input, drift_error, exit_error, stream_error, transform_args, validate_binary,
        validate_model, version_of,
    },
    commands::{
        DRAFT, build_draft_prompt, build_prompt, clean_response, context_summary,
        drop_foreign_markers, is_blank_line, parse_command, place_below, place_here,
        restore_markers, transform_system_prompt,
    },
    parse_line,
};

fn range() -> AiRange {
    AiRange {
        path: "Fruit.md".into(),
        title: "Fruit".into(),
        text: "- apples are good and they are tasty ^b-1\n  - pears ^b-2".into(),
        ancestors: vec!["Fruit ideas".into()],
        block_ids: vec!["b-1".into(), "b-2".into()],
    }
}

fn run(verb: Option<Verb>, prompt: &str, placement: Placement) -> Command {
    Command::Run {
        verb,
        prompt: prompt.into(),
        placement,
    }
}

#[test]
fn recognizes_every_verb_and_where_its_result_goes() {
    let placements: Vec<(Verb, Placement)> = Verb::ALL
        .into_iter()
        .map(
            |verb| match parse_command(&format!("{} please", verb.key()), true, None, false) {
                Command::Run {
                    verb: Some(parsed),
                    prompt,
                    placement,
                } => {
                    assert_eq!((parsed, prompt.as_str()), (verb, "please"));
                    (verb, placement)
                }
                other => panic!("{other:?}"),
            },
        )
        .collect();
    assert_eq!(
        placements,
        [
            (Verb::Tighten, Placement::Replace),
            (Verb::Prose, Placement::Replace),
            (Verb::Outline, Placement::Replace),
            (Verb::Continue, Placement::Insert),
            (Verb::Check, Placement::Replace),
            (Verb::Ask, Placement::Answer),
        ]
    );
    assert_eq!(
        parse_command("TIGHTEN", true, None, false),
        run(Some(Verb::Tighten), "", Placement::Replace)
    );
}

#[test]
fn treats_other_text_as_a_free_prompt_and_drafting_prompts_as_inserts() {
    assert_eq!(
        parse_command("make it formal", true, None, false),
        run(None, "make it formal", Placement::Replace)
    );
    assert_eq!(
        parse_command("list three counterarguments", true, None, false),
        run(None, "list three counterarguments", Placement::Insert)
    );
}

#[test]
fn opens_the_pane_without_arguments_or_range_and_tightens_a_bare_range() {
    assert_eq!(
        parse_command("  ", false, None, false),
        Command::Pane {
            preset: None,
            argument: String::new()
        }
    );
    assert_eq!(
        parse_command("", true, None, false),
        run(Some(Verb::Tighten), "", Placement::Replace)
    );
    assert_eq!(
        parse_command("ask what is missing?", false, None, false),
        run(Some(Verb::Ask), "what is missing?", Placement::Answer)
    );
}

#[test]
fn turns_exact_preset_arguments_into_pane_questions() {
    let pane = |preset: &str, argument: &str| Command::Pane {
        preset: Some(preset.into()),
        argument: argument.into(),
    };
    assert_eq!(parse_command("due", false, None, false), pane("due", ""));
    assert_eq!(
        parse_command("last", false, Some("Weekly sync"), false),
        pane("last", "Weekly sync")
    );
    assert_eq!(
        parse_command("tagged #reading", false, None, false),
        pane("tagged", "#reading")
    );
    assert_eq!(
        parse_command("tidy", false, Some("2026-10-28"), true),
        pane("tidy", "2026-10-28")
    );
    for (args, range) in [
        ("due dates for these", false),
        ("week", true),
        ("tagged", false),
        ("last", false),
        ("tidy", false),
    ] {
        assert!(matches!(
            parse_command(args, range, None, false),
            Command::Run { .. }
        ));
    }
}

#[test]
fn builds_a_verb_prompt_with_page_parents_and_range() {
    let prompt = build_prompt(
        Some(Verb::Tighten),
        "keep it short",
        Placement::Replace,
        Some(&range()),
    );
    assert!(prompt.contains(&format!("{} keep it short", Verb::Tighten.instruction())));
    assert!(prompt.contains("Page: Fruit"));
    assert!(prompt.contains("> Fruit ideas"));
    assert!(prompt.contains(&format!("<text>\n{}\n</text>", range().text)));
}

#[test]
fn builds_free_drafting_and_bare_question_prompts() {
    assert!(
        build_prompt(None, "be formal", Placement::Replace, Some(&range()))
            .starts_with("Apply this instruction to the text: be formal")
    );
    assert_eq!(
        build_prompt(Some(Verb::Ask), "Tidy today", Placement::Answer, None),
        "Tidy today"
    );
    assert_eq!(
        build_prompt(None, "list pros", Placement::Insert, None),
        format!("{DRAFT} list pros")
    );
}

#[test]
fn summarizes_the_exact_context_sent() {
    assert_eq!(
        context_summary(&range()),
        [
            "Page: Fruit",
            "Parent blocks: 1",
            "Selected text: 2 lines, blocks ^b-1, ^b-2",
        ]
    );
    let single = AiRange {
        text: "- one".into(),
        ancestors: Vec::new(),
        block_ids: Vec::new(),
        ..range()
    };
    assert_eq!(
        context_summary(&single),
        ["Page: Fruit", "Selected text: 1 line"]
    );
}

#[test]
fn keeps_transforms_toolless() {
    let prompt = transform_system_prompt();
    assert!(prompt.contains("Reply with only the resulting Markdown"));
    assert!(prompt.ends_with("Ignore these folders: .quire/, attachments/, assets/."));
    assert!(!prompt.contains("quire tools"));
}

#[test]
fn strips_a_wrapping_code_fence_and_trailing_space() {
    assert_eq!(clean_response("```markdown\n- a\n- b\n```\n"), "- a\n- b");
    assert_eq!(clean_response("- a  \n\n"), "- a");
}

#[test]
fn restores_markers_dropped_from_unchanged_lines() {
    let restored = restore_markers(&range().text, "- apples are good and tasty\n  - pears");
    assert_eq!(restored.text, "- apples are good and tasty\n  - pears ^b-2");
    assert_eq!((restored.restored, restored.lost), (1, vec!["b-1".into()]));
}

#[test]
fn restores_markers_on_reordered_lines_and_leaves_echoed_markers_alone() {
    let restored = restore_markers(
        "- one ^b-1\n- two ^b-2\n- three",
        "- two\n- one ^b-1\n- three",
    );
    assert_eq!(restored.text, "- two ^b-2\n- one ^b-1\n- three");
    assert_eq!((restored.restored, restored.lost.len()), (1, 0));
}

#[test]
fn never_invents_markers() {
    let restored = restore_markers("- plain", "- plain\n- new");
    assert_eq!(restored.text, "- plain\n- new");
    assert_eq!((restored.restored, restored.lost.len()), (0, 0));
}

#[test]
fn drops_markers_the_model_copied_from_elsewhere_or_repeated() {
    assert_eq!(
        drop_foreign_markers(
            "- a ^b-1\n- b ^b-9\n- c ^b-1\n- d ^new",
            "- a ^b-1",
            |id| id == "b-9" || id == "b-1",
        ),
        ("- a ^b-1\n- b\n- c\n- d ^new".into(), 2)
    );
}

#[test]
fn recognizes_blank_lines_bare_bullets_and_empty_checkboxes() {
    for line in ["", "   ", "- ", "  * ", "1. ", "- [ ] ", "-"] {
        assert!(is_blank_line(line), "{line:?}");
    }
    for line in ["- a", "text", "- [ ] task"] {
        assert!(!is_blank_line(line), "{line:?}");
    }
}

#[test]
fn prompts_a_draft_with_the_text_above_the_caret() {
    let prompt = build_draft_prompt("list three ideas", "Fruit", "- apples\n", false);
    assert!(prompt.contains(&format!("{DRAFT} list three ideas")));
    assert!(prompt.contains("<context>\n- apples\n</context>"));
    assert!(build_draft_prompt("", "Fruit", "", true).contains(Verb::Continue.instruction()));
    assert!(build_draft_prompt("x", "Fruit", " ", false).contains("no text above the cursor"));
}

#[test]
fn keeps_the_lines_indentation_and_bullet() {
    assert_eq!(place_here("", "One\nTwo"), "One\nTwo");
    assert_eq!(
        place_here("  - ", "first idea\nsecond"),
        "  - first idea\n  second"
    );
    assert_eq!(place_here("  - ", "- a\n- b"), "  - a\n  - b");
    assert_eq!(place_here("- [ ] ", "call the bank"), "- [ ] call the bank");
}

#[test]
fn indents_inserted_lines_like_the_range() {
    assert_eq!(
        place_below("  - first", "- next\n  - child"),
        "  - next\n    - child"
    );
}

fn replay(backend: Backend, fixture: &str) -> (Vec<Event>, Option<String>) {
    let path = format!("{}/tests/fixtures/ai/{fixture}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut state = ParseState::default();
    let mut version = None;
    let events = text
        .lines()
        .filter_map(|line| parse_line(backend, line, &mut state))
        .flat_map(|parsed| {
            version = version.clone().or(parsed.version);
            parsed.events
        })
        .collect();
    (events, version)
}

#[test]
fn streams_claude_tokens_in_order_and_finishes_with_the_result() {
    let (events, version) = replay(Backend::Claude, "claude-transform.jsonl");
    let tokens: String = events
        .iter()
        .filter_map(|event| match event {
            Event::Token(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(tokens, "Apples are good and tasty.");
    assert_eq!(version.as_deref(), Some("2.1.312"));
    assert_eq!(
        events.last(),
        Some(&Event::Done {
            text: "Apples are good and tasty.".into(),
            session: Some("d5a6f40c-107b-449e-8b0e-2c208e9a82d7".into()),
        })
    );
    assert!(matches!(events.first(), Some(Event::Session(_))));
}

#[test]
fn parses_a_codex_transform_and_its_thread() {
    let (events, _) = replay(Backend::Codex, "codex-transform.jsonl");
    assert_eq!(
        events,
        [
            Event::Session("t-1".into()),
            Event::Token("Apples are good and tasty.".into()),
            Event::Done {
                text: "Apples are good and tasty.".into(),
                session: None
            },
        ]
    );
}

#[test]
fn skips_malformed_lines_without_a_result() {
    let (events, version) = replay(Backend::Claude, "claude-drift.jsonl");
    assert_eq!(events, [Event::Session("s".into())]);
    assert_eq!(version.as_deref(), Some("2.1.312"));
    let mut state = ParseState::default();
    assert_eq!(parse_line(Backend::Claude, "{oops", &mut state), None);
    assert_eq!(parse_line(Backend::Claude, "[1]", &mut state), None);
    assert_eq!(
        parse_line(Backend::Codex, r#"{"type":"later"}"#, &mut state).map(|parsed| parsed.events),
        Some(Vec::new())
    );
}

#[test]
fn reports_tool_results_from_both_clis() {
    let results = |backend, fixture| {
        replay(backend, fixture)
            .0
            .into_iter()
            .filter_map(|event| match event {
                Event::Tool {
                    result: Some(result),
                    ..
                } => Some(result),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(
        results(Backend::Codex, "codex-tools.jsonl"),
        [
            "Tasks due this week · 3",
            "Tasks due today · 2",
            "Append to [[2026-10-28]]"
        ]
    );
    assert!(!results(Backend::Claude, "claude-tools.jsonl").is_empty());
}

#[test]
fn marks_failed_tool_calls_and_treats_codex_errors_as_notices() {
    let mut state = ParseState::default();
    parse_line(
        Backend::Codex,
        r#"{"type":"item.started","item":{"id":"item_1","type":"mcp_tool_call","tool":"tasks","arguments":{"due":"today"}}}"#,
        &mut state,
    );
    assert_eq!(
        parse_line(
            Backend::Codex,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"mcp_tool_call","tool":"tasks","status":"failed","error":{"message":"MCP tool call requires approval"}}}"#,
            &mut state,
        )
        .unwrap()
        .events,
        [Event::Tool {
            id: Some("item_1".into()),
            name: "tasks".into(),
            detail: String::new(),
            result: Some("MCP tool call requires approval".into()),
            failed: true,
        }]
    );
    assert_eq!(
        parse_line(
            Backend::Claude,
            r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"toolu_x","is_error":true,"content":"No notebook is open in Quire.\nmore"}]}}"#,
            &mut ParseState::default(),
        )
        .unwrap()
        .events,
        [Event::Tool {
            id: Some("toolu_x".into()),
            name: "tool".into(),
            detail: String::new(),
            result: Some("No notebook is open in Quire.".into()),
            failed: true,
        }]
    );
    assert_eq!(
        parse_line(
            Backend::Codex,
            r#"{"type":"error","message":"Reconnecting... 1/5"}"#,
            &mut state
        )
        .unwrap()
        .events,
        [Event::Tool {
            id: None,
            name: "notice".into(),
            detail: "Reconnecting... 1/5".into(),
            result: None,
            failed: false,
        }]
    );
    assert_eq!(
        parse_line(
            Backend::Codex,
            r#"{"type":"turn.failed","error":{"message":"quota"}}"#,
            &mut state
        )
        .unwrap()
        .events,
        [Event::Error("quota".into())]
    );
}

#[test]
fn builds_transform_arguments_without_prompt_text_or_the_tool_server() {
    let claude = transform_args(Backend::Claude, "/n", "sonnet");
    let tools = claude.iter().position(|arg| arg == "--tools").unwrap();
    assert_eq!(claude[tools + 1..tools + 4], ["", "--max-turns", "1"]);
    assert!(claude.contains(&"--restricted".into()));
    assert!(!claude.contains(&"--mcp-config".into()));
    assert_eq!(claude[claude.len() - 2..], ["--model", "sonnet"]);
    let prompt = claude
        .iter()
        .position(|arg| arg == "--append-system-prompt")
        .unwrap();
    assert_eq!(claude[prompt + 1], transform_system_prompt());
    let codex = transform_args(Backend::Codex, "/n", "");
    assert_eq!(codex[..4], ["exec", "--json", "--sandbox", "read-only"]);
    let off = codex
        .iter()
        .position(|arg| arg == "mcp_servers.quire.enabled=false")
        .unwrap();
    assert_eq!(
        codex[off - 2..off],
        ["mcp_servers.quire.command=\"quire-off\"", "-c"]
    );
    assert!(off < codex.iter().position(|arg| arg == "-").unwrap());
    assert!(codex.contains(&"--ephemeral".into()));
    assert_eq!(codex[codex.len() - 3..], ["-C", "/n", "-"]);
    for backend in Backend::ALL {
        assert!(
            !transform_args(backend, "/n", "")
                .join(" ")
                .contains("secret")
        );
    }
    assert_eq!(
        backend_input(Backend::Claude, "Tighten: secret"),
        "Tighten: secret"
    );
    let codex_input = backend_input(Backend::Codex, "Tighten: secret");
    assert!(codex_input.starts_with("You edit one range"));
    assert!(codex_input.ends_with("\n\nTighten: secret"));
}

#[test]
fn explains_failures_with_the_next_step() {
    assert_eq!(
        exit_error(Backend::Claude, "code 3", "boom happened\n"),
        "Claude Code exited with code 3: boom happened. Retry, or run it in a terminal to see the full error."
    );
    assert!(
        exit_error(
            Backend::Claude,
            "code 1",
            "Invalid API key · Please run /login"
        )
        .contains("Run `claude` in a terminal and sign in with /login")
    );
    assert!(stream_error(Backend::Codex, "Not logged in").starts_with("Codex could not sign in"));
    assert_eq!(
        drift_error(Backend::Codex, "0.199.0", false),
        "Codex 0.199.0 ended without a result Quire could read and sent no text. Quire expects Codex 0.164.0; install that version or update Quire."
    );
    assert_eq!(version_of("2.1.312 (Claude Code)"), "2.1.312");
    assert_eq!(version_of("codex-cli 0.199.0\n"), "0.199.0");
    assert_eq!(version_of("no digits"), "unknown");
}

#[test]
fn rejects_relative_binary_paths_and_odd_model_names() {
    assert!(
        validate_binary(Backend::Claude, "bin/claude")
            .unwrap_err()
            .contains("must be absolute")
    );
    assert_eq!(
        validate_binary(Backend::Codex, " /opt/codex ").unwrap(),
        "/opt/codex"
    );
    assert_eq!(validate_binary(Backend::Codex, "  ").unwrap(), "");
    assert!(validate_model("--dangerous").is_err());
    assert!(validate_model("a b").is_err());
    assert_eq!(validate_model(" gpt-5.1 ").unwrap(), "gpt-5.1");
    assert_eq!(validate_model("claude-[1m]").unwrap(), "claude-[1m]");
}
