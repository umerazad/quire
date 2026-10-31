//! The export cases of the reference `tests/page-export.test.ts` and `ai-commands.ts`: the exact
//! prompt and rules, the sandboxed argument lists, and the file checks.

use quire_core::ai::{
    Backend,
    export::{
        EXPORT_RULES, Format, export_args, export_prompt, export_system_prompt, has_signature,
        invalid_file, unfinished,
    },
};

#[test]
fn the_prompt_names_the_page_format_and_file() {
    assert_eq!(
        export_prompt("Plan", Format::Pdf),
        "Export page.md, the Quire page \"Plan\", as a PDF document saved as export.pdf in the working directory."
    );
    assert_eq!(
        export_prompt("Q3 \"draft\"", Format::Docx),
        "Export page.md, the Quire page \"Q3 \"draft\"\", as a Word document saved as export.docx in the working directory."
    );
}

#[test]
fn the_system_prompt_is_the_reference_rules_then_today() {
    let system = export_system_prompt("2026-10-31");
    let lines: Vec<&str> = system.lines().collect();
    assert_eq!(lines.len(), EXPORT_RULES.len() + 1);
    assert_eq!(
        lines[0],
        "You turn one page of the writer's Quire notebook into a finished document for readers outside the notebook."
    );
    assert!(lines[5].ends_with("Do not fetch anything from the network."));
    assert_eq!(
        lines[8],
        "When the file is written, reply with only its file name."
    );
    assert_eq!(lines[9], "Today is 2026-10-31.");
}

#[test]
fn claude_runs_exports_with_document_tools_in_its_sandbox() {
    let args = export_args(Backend::Claude, "RULES", "/tmp/quire-export-1", "");
    assert!(!args.contains(&"--restricted".to_owned()));
    let after = |flag: &str| args[args.iter().position(|arg| arg == flag).unwrap() + 1].clone();
    assert_eq!(after("--tools"), "Bash,Read,Write,Edit,Glob,Grep,Skill");
    assert_eq!(
        after("--allowedTools"),
        "Bash,Read,Write,Edit,Glob,Grep,Skill"
    );
    assert_eq!(after("--permission-mode"), "acceptEdits");
    assert_eq!(after("--permission-prompts"), "none");
    assert_eq!(after("--append-system-prompt"), "RULES");
    let sandbox: serde_json::Value = serde_json::from_str(&after("--settings")).unwrap();
    assert_eq!(sandbox["sandbox"]["enabled"], true);
    assert_eq!(sandbox["sandbox"]["autoAllowBashIfSandboxed"], true);
    assert!(args.contains(&"--strict-mcp-config".to_owned()));
    assert!(args.contains(&"--no-session-persistence".to_owned()));
    assert!(!args.contains(&"--model".to_owned()));
    let with_model = export_args(Backend::Claude, "RULES", "/x", " sonnet ");
    assert_eq!(with_model[with_model.len() - 2..], ["--model", "sonnet"]);
}

#[test]
fn codex_runs_exports_writable_only_in_the_export_folder() {
    let args = export_args(Backend::Codex, "RULES", "/tmp/x", "gpt-5");
    assert_eq!(
        args[..4],
        ["exec", "--json", "--sandbox", "workspace-write"]
    );
    assert_eq!(
        args[args.iter().position(|arg| arg == "-C").unwrap() + 1],
        "/tmp/x"
    );
    assert!(args.contains(&"mcp_servers.quire.enabled=false".to_owned()));
    assert!(args.contains(&"--ephemeral".to_owned()));
    assert_eq!(args[args.len() - 3..], ["-m", "gpt-5", "-"]);
    assert!(!args.iter().any(|arg| arg.contains("RULES")));
}

#[test]
fn only_files_that_start_as_their_format_pass() {
    assert!(has_signature(Format::Pdf, b"%PDF-1.7 fake"));
    assert!(!has_signature(Format::Pdf, b"not a pdf"));
    assert!(!has_signature(Format::Docx, b"not a zip"));
    assert!(!has_signature(Format::Docx, b"PK\x03\x04 other.xml"));
    assert!(has_signature(
        Format::Docx,
        b"PK\x03\x04....word/document.xml"
    ));
    assert_eq!(
        invalid_file(Backend::Claude, Format::Docx),
        "Claude Code finished but did not write a valid Word file named export.docx. Export again, or check that your Word tools work in a terminal."
    );
    assert_eq!(
        unfinished(Backend::Codex, Format::Pdf),
        "Codex stopped without finishing the PDF file. Export again."
    );
}
