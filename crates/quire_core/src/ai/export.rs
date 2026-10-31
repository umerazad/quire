//! PDF and Word export through the CLI (reference `exportRules`, `exportPrompt`, `exportFile` in
//! `src/core/ai-commands.ts` and the export branch of `backendArgs` in
//! `src/desktop/ai-runner.ts`). The CLI works in a folder holding only `page.md` and writes one
//! file there, with the writer's document tools on and Claude Code's sandbox keeping commands
//! inside that folder.

use super::backend::{Backend, TOOL_SERVER};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Format {
    Pdf,
    Docx,
}

impl Format {
    pub const ALL: [Format; 2] = [Self::Pdf, Self::Docx];

    /// The name the writer reads.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pdf => "PDF",
            Self::Docx => "Word",
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            Self::Pdf => "pdf",
            Self::Docx => "docx",
        }
    }

    /// The file the CLI writes in its working folder.
    pub fn file(self) -> String {
        format!("export.{}", self.extension())
    }
}

/// The page as the CLI finds it in its working folder.
pub const PAGE_FILE: &str = "page.md";

pub const EXPORT_RULES: [&str; 9] = [
    "You turn one page of the writer's Quire notebook into a finished document for readers outside the notebook.",
    "The working directory holds page.md, the page as Markdown. Write exactly one file, with the name given in the request, in the working directory. Do not change page.md and do not write anywhere else.",
    "Make it look like a deliverable from a top-tier strategy consulting firm: a restrained, professional design; a clear type hierarchy in a modern sans-serif; generous margins and white space; one accent colour used sparingly; a title block with the page title and today's date; running headers or footers with page numbers; tables with clean rules and right-aligned numbers; consistent spacing between sections.",
    "Keep every piece of the page's content, in its order and wording. Do not add facts, figures, conclusions, sources or sections that are not in the page. You may add a title block, a table of contents for long documents, headers, footers and page numbers.",
    "Render the Markdown structure faithfully: headings, nested bullet and numbered lists at every depth, open and done task checkboxes, tables, block quotes, code, emphasis, links and images. Images are local files given by absolute path; include them.",
    "Use the best document tools you have: your installed skills for PDF or Word documents first, then tools such as pandoc, Typst, LaTeX, a headless browser, python-docx or ReportLab. Do not fetch anything from the network.",
    "For Word, use real Word styles (Title, Heading 1 to 3, List Bullet, List Number) and native Word tables, so the document stays editable.",
    "Before finishing, inspect the result: confirm every section of page.md is present and fix layout problems such as overflowing tables or headings stranded at the foot of a page.",
    "When the file is written, reply with only its file name.",
];

/// The system prompt of an export run.
pub fn export_system_prompt(today: &str) -> String {
    EXPORT_RULES
        .iter()
        .map(|line| (*line).to_owned())
        .chain([format!("Today is {today}.")])
        .collect::<Vec<_>>()
        .join("\n")
}

/// The request text of an export run. The title is the writer's, so it goes through standard
/// input, never the argument list.
pub fn export_prompt(title: &str, format: Format) -> String {
    format!(
        "Export page.md, the Quire page \"{title}\", as a {} document saved as {} in the working directory.",
        format.name(),
        format.file()
    )
}

/// The tools an export run may use: the writer's own document skills and shell, sandboxed.
pub const EXPORT_TOOLS: &str = "Bash,Read,Write,Edit,Glob,Grep,Skill";
/// Claude Code's sandbox, which lets commands write only inside the export folder.
pub const EXPORT_SANDBOX: &str = r#"{"sandbox":{"enabled":true,"autoAllowBashIfSandboxed":true}}"#;

/// The argument list of an export run in `cwd`. Unlike transforms, it keeps the writer's user
/// settings and skills; the notebook tool server stays off.
pub fn export_args(backend: Backend, system: &str, cwd: &str, model: &str) -> Vec<String> {
    let model = model.trim();
    let mut args: Vec<String> = match backend {
        Backend::Claude => [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--strict-mcp-config",
            "--tools",
            EXPORT_TOOLS,
            "--allowedTools",
            EXPORT_TOOLS,
            "--permission-mode",
            "acceptEdits",
            "--permission-prompts",
            "none",
            "--settings",
            EXPORT_SANDBOX,
            "--no-session-persistence",
            "--append-system-prompt",
            system,
        ]
        .map(str::to_owned)
        .to_vec(),
        Backend::Codex => vec![
            "exec".into(),
            "--json".into(),
            "--sandbox".into(),
            "workspace-write".into(),
            "--skip-git-repo-check".into(),
            "--ephemeral".into(),
            "-c".into(),
            format!("mcp_servers.{TOOL_SERVER}.command=\"quire-off\""),
            "-c".into(),
            format!("mcp_servers.{TOOL_SERVER}.enabled=false"),
            "-C".into(),
            cwd.into(),
        ],
    };
    if !model.is_empty() {
        let flag = match backend {
            Backend::Claude => "--model",
            Backend::Codex => "-m",
        };
        args.extend([flag.into(), model.into()]);
    }
    if backend == Backend::Codex {
        args.push("-".into());
    }
    args
}

/// Whether `bytes` start as a finished file of `format` must: `%PDF-` for PDF, a ZIP holding
/// `word/document.xml` for Word. A run that wrote the wrong kind of file, or a stub, fails here.
pub fn has_signature(format: Format, bytes: &[u8]) -> bool {
    match format {
        Format::Pdf => bytes.starts_with(b"%PDF-"),
        Format::Docx => {
            bytes.starts_with(b"PK")
                && bytes
                    .windows(b"word/document.xml".len())
                    .any(|window| window == b"word/document.xml")
        }
    }
}

/// The message for a run that finished without a valid file, with what opening it found.
pub fn invalid_file(backend: Backend, format: Format, reason: Option<&str>) -> String {
    format!(
        "{} finished but did not write a valid {name} file named {file}{}. Export again, or check that your {name} tools work in a terminal.",
        backend.name(),
        reason
            .map(|reason| format!(": {reason}"))
            .unwrap_or_default(),
        name = format.name(),
        file = format.file()
    )
}

/// Wraps any other failure of an export.
pub fn export_failed(format: Format, message: &str) -> String {
    format!(
        "The {} export could not finish: {message}. Export again.",
        format.name()
    )
}

/// What the export buttons explain while AI commands are off.
pub const EXPORT_OFF: &str = "Turn on AI commands in Settings > AI to export PDF and Word";

/// The export buttons' tooltip while AI commands are on.
pub fn export_hint(backend: Backend) -> String {
    format!(
        "Formats the page with {} and your document tools",
        backend.name()
    )
}
