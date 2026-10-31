//! Ported from the question, citation, and Tidy cases of the reference `tests/ai-commands.test.ts`,
//! `tests/ai-page.test.tsx`, and `tests/tidy.test.ts`.

use quire_core::ai::{
    AiRange, Backend, Command,
    ask::{
        Cited, DEFAULT_SYNTHESIS, Excerpt, Gathered, MAX_EXCERPT_CHARS, MAX_EXCERPTS, Needs,
        PRESETS, Preset, Turn, answer_block, ask_context_summary, ask_system_prompt,
        build_ask_prompt, link_citations, parse_citation, resolve_quotes,
    },
    backend::{ToolServer, ask_args, run_input},
    commands::parse_command,
    tidy,
    tools::Writes,
};

fn excerpt(title: &str, id: Option<&str>, text: &str) -> Excerpt {
    Excerpt {
        title: title.into(),
        id: id.map(str::to_owned),
        text: text.into(),
    }
}

#[test]
fn presets_ask_their_questions_and_need_what_they_name() {
    assert_eq!(
        PRESETS.map(|preset| preset.key),
        ["due", "week", "last", "yesterday", "tidy", "tagged"]
    );
    let last = Preset::find("LAST").unwrap();
    assert_eq!(last.needs, Needs::Page);
    assert_eq!(
        last.label_for(Some("Atlas")),
        "Summarize the last occurrence of Atlas"
    );
    assert_eq!(
        last.question("Atlas"),
        "Find the last occurrence of [[Atlas]] and summarize it: decisions, follow-ups and open tasks."
    );
    assert_eq!(
        Preset::find("tagged").unwrap().question("#idea"),
        "Show me everything tagged #idea from the last month, grouped by page."
    );
    let tidy = Preset::find("tidy").unwrap();
    assert!(!tidy.offered(Some("Atlas"), false));
    assert!(tidy.offered(Some("2026-10-27"), true));
    assert!(!last.offered(None, false));
    assert!(Preset::find("due").unwrap().offered(None, false));
}

#[test]
fn ai_without_a_range_names_the_pane_and_its_presets() {
    let pane = |preset: Option<&str>, argument: &str| Command::Pane {
        preset: preset.map(str::to_owned),
        argument: argument.into(),
    };
    assert_eq!(parse_command("", false, None, false), pane(None, ""));
    assert_eq!(
        parse_command("due", false, None, false),
        pane(Some("due"), "")
    );
    assert_eq!(
        parse_command("tagged idea", false, None, false),
        pane(Some("tagged"), "idea")
    );
    assert_eq!(
        parse_command("last", false, Some("Atlas"), false),
        pane(Some("last"), "Atlas")
    );
    assert_eq!(
        parse_command("tidy", false, Some("2026-10-27"), true),
        pane(Some("tidy"), "2026-10-27")
    );
    assert!(matches!(
        parse_command("tidy", false, Some("Atlas"), false),
        Command::Run { .. }
    ));
    assert!(matches!(
        parse_command("due", true, None, false),
        Command::Run { .. }
    ));
}

#[test]
fn a_bare_question_goes_as_written_and_context_follows_it() {
    assert_eq!(
        build_ask_prompt("  What is due?  ", None, None, &[]),
        "What is due?"
    );
    let gathered = Gathered::bounded([
        excerpt("Oak & Ash", Some("b-1"), "Oaks grow slowly"),
        excerpt("Birch", None, "Birch \"bark\" peels"),
    ]);
    let prompt = build_ask_prompt(DEFAULT_SYNTHESIS, None, Some(&gathered), &[]);
    assert_eq!(
        prompt,
        format!(
            "{DEFAULT_SYNTHESIS}\n\nLinked references:\n<excerpt page=\"Oak &amp; Ash\" id=\"b-1\">\nOaks grow slowly\n</excerpt>\n<excerpt page=\"Birch\">\nBirch \"bark\" peels\n</excerpt>"
        )
    );
    let range = AiRange {
        path: "Fruit.md".into(),
        title: "Fruit".into(),
        text: "- apples ^b-1".into(),
        ancestors: vec!["Ideas".into()],
        block_ids: vec!["b-1".into()],
    };
    assert_eq!(
        build_ask_prompt("Why?", Some(&range), None, &[]),
        "Answer the question about the text. Why?\n\nPage: Fruit\n\nParent blocks, for context only:\n> Ideas\n\nText:\n<text>\n- apples ^b-1\n</text>"
    );
    let earlier = [Turn {
        question: "First?".into(),
        answer: "One.".into(),
    }];
    assert_eq!(
        build_ask_prompt("And then?", None, None, &earlier),
        "Earlier in this conversation:\n<question>\nFirst?\n</question>\n<answer>\nOne.\n</answer>\n\nAnd then?"
    );
}

#[test]
fn context_is_bounded_deduplicated_and_summarized() {
    let many = (0..MAX_EXCERPTS + 5)
        .map(|index| excerpt(&format!("P{}", index % 3), None, &format!("block {index}")));
    let gathered = Gathered::bounded(many.chain([excerpt("P0", None, "block 0")]));
    assert_eq!(gathered.excerpts.len(), MAX_EXCERPTS);
    assert_eq!(gathered.omitted, 5);
    assert_eq!(
        ask_context_summary(None, Some(&gathered)),
        [
            "Linked references: 60 blocks from 3 pages",
            "5 more blocks left out to stay under the size limit"
        ]
    );
    let long = Gathered::bounded([excerpt("P", None, &"x".repeat(5000))]);
    assert_eq!(long.excerpts[0].text.chars().count(), MAX_EXCERPT_CHARS + 1);
    assert_eq!(
        ask_context_summary(None, Some(&Gathered::bounded([]))),
        ["Linked references: none found"]
    );
}

#[test]
fn citations_become_links_that_name_their_target() {
    let linked = link_citations(
        "See [[Fruit Ideas]], [[Atlas|the project]] and ((b-1)) or [[Fruit#^b-2]].",
        |id| (id == "b-1").then(|| "Apples are good".to_owned()),
    );
    assert_eq!(
        linked,
        "See [Fruit Ideas](quire-page:Fruit%20Ideas), [the project](quire-page:Atlas) and [↗ Apples are good](quire-block:b-1) or [↗ block](quire-block:Fruit%23%5Eb-2)."
    );
    assert_eq!(
        parse_citation("quire-page:Fruit%20Ideas"),
        Some(Cited::Page("Fruit Ideas".into()))
    );
    assert_eq!(
        parse_citation("quire-block:Fruit%23%5Eb-2"),
        Some(Cited::Block("Fruit#^b-2".into()))
    );
    assert_eq!(parse_citation("https://example.com"), None);
}

#[test]
fn quotes_resolve_to_block_references_only_where_a_block_is_found() {
    let text = resolve_quotes(
        "Apples [[Fruit]] \"good and tasty\" and [[Code]] \"fenced words here\".",
        |title, quote| (title == "Fruit" && quote == "good and tasty").then(|| "b-9".into()),
    );
    assert_eq!(
        text,
        "Apples \"good and tasty\" ((b-9)) and [[Code]] \"fenced words here\"."
    );
    assert_eq!(
        answer_block("One\n\nTwo\nThree\n"),
        "- One\n\n  Two\n  Three"
    );
}

fn server(writes: Writes) -> ToolServer {
    ToolServer {
        command: "/Applications/Quire.app/Contents/MacOS/quire".into(),
        args: vec!["--mcp-server".into()],
        bridge: "/data/bridge.json".into(),
        run: "ask-r1".into(),
        writes,
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> &'a str {
    let at = args.iter().position(|arg| arg == name).unwrap();
    &args[at + 1]
}

const READS: &str = r#""search_notebook","list_pages","read_page","read_block","linked_references","tagged_blocks","tasks","daily_pages","mentions""#;

#[test]
fn ask_runs_attach_the_tool_server_and_keep_the_question_off_the_command_line() {
    let system = ask_system_prompt(Writes::Review, "2026-10-31");
    assert!(system.contains("quire tools"));
    assert!(system.contains("stage each change"));
    assert!(system.contains("Today is 2026-10-31."));
    assert!(system.contains("((block-id))"));
    let claude = ask_args(
        Backend::Claude,
        &system,
        "/tmp/run",
        "",
        &server(Writes::Review),
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(flag(&claude, "--mcp-config")).unwrap(),
        serde_json::json!({
            "mcpServers": {
                "quire": {
                    "type": "stdio",
                    "command": "/Applications/Quire.app/Contents/MacOS/quire",
                    "args": ["--mcp-server"],
                    "env": { "QUIRE_BRIDGE": "/data/bridge.json", "QUIRE_RUN": "ask-r1" },
                }
            }
        })
    );
    assert_eq!(flag(&claude, "--tools"), "");
    assert_eq!(
        flag(&claude, "--allowedTools"),
        "mcp__quire__search_notebook,mcp__quire__list_pages,mcp__quire__read_page,mcp__quire__read_block,mcp__quire__linked_references,mcp__quire__tagged_blocks,mcp__quire__tasks,mcp__quire__daily_pages,mcp__quire__mentions,mcp__quire__append_block,mcp__quire__create_page,mcp__quire__replace_block,mcp__quire__set_task_state,mcp__quire__capture_task"
    );
    for kept in [
        "--restricted",
        "--strict-mcp-config",
        "--no-session-persistence",
    ] {
        assert!(claude.contains(&kept.to_owned()), "{kept} is missing");
    }
    assert!(!claude.contains(&"--max-turns".to_owned()));
    assert_eq!(flag(&claude, "--append-system-prompt"), system);
    assert_eq!(run_input(Backend::Claude, &system, "Secret?"), "Secret?");
    assert_eq!(
        run_input(Backend::Codex, &system, "Secret?"),
        format!("{system}\n\nSecret?")
    );
    let codex = ask_args(
        Backend::Codex,
        &system,
        "/tmp/run",
        "o3",
        &server(Writes::Apply),
    );
    assert_eq!(
        codex,
        [
            "exec",
            "--json",
            "--sandbox",
            "read-only",
            "--skip-git-repo-check",
            "--ephemeral",
            "-C",
            "/tmp/run",
            "-m",
            "o3",
            "-c",
            r#"mcp_servers.quire.command="/Applications/Quire.app/Contents/MacOS/quire""#,
            "-c",
            r#"mcp_servers.quire.args=["--mcp-server"]"#,
            "-c",
            r#"mcp_servers.quire.env={QUIRE_BRIDGE="/data/bridge.json",QUIRE_RUN="ask-r1"}"#,
            "-c",
            r#"mcp_servers.quire.default_tools_approval_mode="approve""#,
            "-c",
            &format!(
                r#"mcp_servers.quire.enabled_tools=[{READS},"append_block","create_page","replace_block","set_task_state","capture_task"]"#
            ),
            "-",
        ]
    );
    assert!(
        !codex
            .iter()
            .chain(&claude)
            .any(|arg| arg.contains("Secret"))
    );
}

#[test]
fn write_tools_stay_off_the_allowed_list_while_ai_writes_is_off() {
    let system = ask_system_prompt(Writes::Off, "2026-10-31");
    assert!(system.contains("AI writes are off"));
    let claude = ask_args(
        Backend::Claude,
        &system,
        "/tmp/run",
        "",
        &server(Writes::Off),
    );
    let allowed: Vec<&str> = flag(&claude, "--allowedTools").split(',').collect();
    assert_eq!(allowed.len(), 9);
    assert!(!allowed.contains(&"mcp__quire__append_block"));
    let codex = ask_args(
        Backend::Codex,
        &system,
        "/tmp/run",
        "",
        &server(Writes::Off),
    );
    assert!(codex.contains(&format!("mcp_servers.quire.enabled_tools=[{READS}]")));
    assert!(ask_system_prompt(Writes::Apply, "2026-10-31").contains("apply at once"));
}

const DAY: &str = "2026-10-27-Tuesday";

fn exists(title: &str) -> bool {
    ["Dana Reyes", "Atlas"].contains(&title)
}

fn is_daily(title: &str) -> bool {
    title.starts_with("2026-")
}

fn check(before: &str, after: &str) -> Option<String> {
    tidy::check_change(before, after, exists, is_daily)
}

#[test]
fn tidy_accepts_links_an_alias_a_state_and_a_due_date_that_only_add() {
    assert_eq!(check("Atlas review moved", "[[Atlas]] review moved"), None);
    assert_eq!(
        check(
            "Weekly sync with Dana",
            "Weekly sync with [[Dana Reyes|Dana]]"
        ),
        None
    );
    assert_eq!(
        check(
            "[ ] Send the budget to Dana by Thursday",
            "[ ] Send the budget to [[Dana Reyes|Dana]] by Thursday #next 📅 [[2026-10-29-Thursday]]",
        ),
        None
    );
}

#[test]
fn tidy_refuses_rewrites_deletions_missing_pages_and_changed_links() {
    let refused = |before: &str, after: &str| check(before, after).unwrap_or_default();
    assert!(refused("Atlas review moved", "Atlas review postponed").contains("cannot rewrite"));
    assert!(refused("Atlas review moved", "Atlas review").contains("cannot rewrite"));
    assert!(refused("Meet Sam", "Meet [[Sam]]").contains("is not a page"));
    assert!(refused("See [[Atlas]] now", "See Atlas now").contains("existing link"));
    assert!(
        refused("[ ] Call #waiting", "[ ] Call #waiting #next").contains("already has a state")
    );
    assert!(refused("[x] Done thing", "[x] Done thing #next").contains("open task"));
    assert!(refused("Plain note", "Plain note 📅 [[2026-10-30]]").contains("open task"));
}

#[test]
fn tidy_keeps_allowed_lines_restores_ids_and_refuses_the_rest() {
    let original = "- Weekly sync with Dana ^keep\n  - [ ] Send the budget to Dana by Thursday\n  - Atlas review moved\n  - Lunch\n";
    let proposed = "- Weekly sync with [[Dana Reyes|Dana]]\n  - [ ] Send the budget to Dana by Thursday #next\n  - Atlas review postponed\n  - New idea\n  - Lunch";
    let tidied = tidy::filter(original, proposed, exists, is_daily);
    assert_eq!(
        tidied.text,
        "- Weekly sync with [[Dana Reyes|Dana]] ^keep\n  - [ ] Send the budget to Dana by Thursday #next\n  - Atlas review moved\n  - Lunch\n"
    );
    assert_eq!(tidied.changed, 2);
    assert_eq!(
        tidied.refused,
        [
            tidy::Refusal {
                line: "  - Atlas review postponed".into(),
                reason: tidy::REWRITE.into()
            },
            tidy::Refusal {
                line: "  - New idea".into(),
                reason: tidy::ADDED_LINE.into()
            },
        ]
    );
    let moved = tidy::filter("- a ^b-1\n- b", "- b\n- a ^b-1", exists, is_daily);
    assert_eq!(moved.text, "- a ^b-1\n- b");
    assert_eq!(moved.changed, 0);
    assert!(!moved.refused.is_empty());
    let id = tidy::filter("- Atlas ^b-1", "- [[Atlas]] ^b-2", exists, is_daily);
    assert_eq!(id.refused[0].reason, tidy::CHANGED_ID);
    let bullet = tidy::filter("- Atlas", "* [[Atlas]]", exists, is_daily);
    assert_eq!(bullet.text, "- Atlas");
    let trailing = tidy::filter("- Atlas\n", "- [[Atlas]]\n- extra", exists, is_daily);
    assert_eq!(trailing.text, "- [[Atlas]]\n");
    assert_eq!(
        trailing.refused,
        [tidy::Refusal {
            line: "- extra".into(),
            reason: tidy::ADDED_LINE.into()
        }],
        "a page's trailing blank line never pairs with an added line"
    );
}

#[test]
fn tidy_asks_for_the_whole_day_with_the_pages_it_may_link() {
    let titles = tidy::mentioned_titles(
        "- Weekly sync with Dana\n- Atlas review",
        ["Dana Reyes", "Atlas", "Other", "Al"],
        50,
    );
    assert_eq!(titles, ["Atlas", "Dana Reyes"]);
    let prompt = tidy::prompt(DAY, "- Atlas review", &titles, "2026-10-28-Wednesday");
    assert!(prompt.starts_with("Tidy the daily page [[2026-10-27-Tuesday]]."));
    assert!(prompt.contains("Existing pages it may link:\n- Atlas\n- Dana Reyes"));
    assert!(prompt.contains("📅 [[2026-10-28-Wednesday]]"));
    assert!(prompt.ends_with("Text:\n<text>\n- Atlas review\n</text>"));
}
