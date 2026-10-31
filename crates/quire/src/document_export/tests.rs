//! The reference `tests/page-export.test.ts` export runs against the fake CLI, which makes real
//! Word and PDF files from `page.md`: the exact request, the sandboxed arguments, the folder
//! removed before the result, files reopened to check their headings, text, and images, and the
//! failures a run can end in.

use super::*;
use crate::{
    ai::process::tests::{Bench, fake_cli},
    test_patience::PATIENCE,
};
use futures::FutureExt as _;
use std::time::{Duration, Instant};

fn job(format: Format, text: &str, transcripts: Option<PathBuf>) -> Job {
    Job {
        backend: Backend::Claude,
        binary: fake_cli().to_string_lossy().into_owned(),
        model: String::new(),
        format,
        title: "Plan".into(),
        text: text.into(),
        today: "2026-10-31".into(),
        transcript: transcripts.map(|folder| Transcript {
            folder,
            id: "export-1".into(),
            page: "Plan.md".into(),
        }),
    }
}

fn produced(host: &Host, job: Job) -> (Produced, usize) {
    let mut steps = 0;
    let produced = smol::block_on(produce(host, job, futures::future::pending(), |update| {
        if matches!(update, Update::Tool { .. }) {
            steps += 1;
        }
    }));
    (produced, steps)
}

fn file(produced: Produced) -> Vec<u8> {
    match produced {
        Produced::File(bytes) => bytes,
        other => panic!("the export did not produce a file: {other:?}"),
    }
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + PATIENCE;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

const PAGE: &str = "# Plan\n\n## Goals\n- Ship by **December**\n### Risks\nBudget & time\n";

#[test]
fn a_word_export_sends_the_reference_request_and_keeps_headings_text_and_images() {
    let bench = Bench::new();
    let image = bench.folder.path().join("chart.png");
    std::fs::write(&image, b"\x89PNG chart bytes").unwrap();
    let text = format!("{PAGE}- ![Chart](<{}>)\n", image.display());
    let transcripts = bench.folder.path().join("ai");
    let host = bench.host("claude-tools.jsonl", &[("FAKE_AI_RENDER", "export.docx")]);
    let (produced, steps) = produced(&host, job(Format::Docx, &text, Some(transcripts.clone())));
    let docx = smol::block_on(check::read_docx(file(produced))).unwrap();
    let paragraphs: Vec<(&str, &str)> = docx
        .paragraphs
        .iter()
        .map(|paragraph| (paragraph.style.as_str(), paragraph.text.as_str()))
        .collect();
    assert_eq!(
        paragraphs,
        [
            ("Heading1", "Plan"),
            ("Heading2", "Goals"),
            ("ListBullet", "Ship by **December**"),
            ("Heading3", "Risks"),
            ("Normal", "Budget & time"),
            ("ListBullet", ""),
        ]
    );
    let shown = &docx.paragraphs[5].images;
    assert_eq!(shown, &["word/media/image1.png".to_owned()]);
    assert_eq!(docx.media[&shown[0]], b"\x89PNG chart bytes");
    assert!(steps > 0, "tool calls report progress");

    let calls = bench.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["stdin"],
        "Export page.md, the Quire page \"Plan\", as a Word document saved as export.docx in the working directory."
    );
    let argv: Vec<String> = serde_json::from_value(calls[0]["argv"].clone()).unwrap();
    let after = |flag: &str| argv[argv.iter().position(|arg| arg == flag).unwrap() + 1].clone();
    assert_eq!(after("--tools"), "Bash,Read,Write,Edit,Glob,Grep,Skill");
    assert_eq!(
        after("--append-system-prompt"),
        quire_core::ai::export::export_system_prompt("2026-10-31")
    );
    let cwd = PathBuf::from(calls[0]["cwd"].as_str().unwrap());
    assert!(
        cwd.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("quire-export-")
    );
    assert!(
        !cwd.exists(),
        "the export folder is gone once the run reports"
    );
    let days: Vec<_> = std::fs::read_dir(&transcripts).unwrap().collect();
    assert_eq!(days.len(), 1);
}

#[test]
fn a_pdf_export_reopens_with_every_line_of_the_page() {
    let bench = Bench::new();
    let host = bench.host(
        "claude-transform.jsonl",
        &[("FAKE_AI_RENDER", "export.pdf")],
    );
    let bytes = file(produced(&host, job(Format::Pdf, PAGE, None)).0);
    let saved = bench.folder.path().join("Plan.pdf");
    assert_eq!(save(&saved, &bytes, false), Ok(Saved::Written));
    let document = quire_media::pdf::Document::open_stored(&saved).unwrap();
    assert_eq!(document.info().pages.len(), 1);
    let text = smol::block_on(document.text(0)).unwrap().unwrap();
    let text = text.text(0..text.chars.len());
    for line in [
        "Plan",
        "Goals",
        "Ship by **December**",
        "Risks",
        "Budget & time",
    ] {
        assert!(text.contains(line), "{line:?} is missing from {text:?}");
    }
}

#[test]
fn a_run_that_writes_the_wrong_file_or_none_is_reported_not_saved() {
    let bench = Bench::new();
    let cases = [
        (
            Format::Docx,
            "export.docx=not a zip",
            ": it does not start as a Word file.",
        ),
        (
            Format::Docx,
            "export.docx=PK word/document.xml",
            ": it is not a readable ZIP archive",
        ),
        (
            Format::Pdf,
            "export.pdf=%PDF-1.7 fake",
            ": it does not open as a PDF",
        ),
        (
            Format::Pdf,
            "elsewhere.pdf=%PDF-1.7",
            " named export.pdf. Export",
        ),
    ];
    for (format, write, expected) in cases {
        let host = bench.host("claude-transform.jsonl", &[("FAKE_AI_WRITE", write)]);
        let Produced::Failed(message) = produced(&host, job(format, PAGE, None)).0 else {
            panic!("{write} was accepted");
        };
        assert!(
            message.starts_with(&format!(
                "Claude Code finished but did not write a valid {} file named {}",
                format.name(),
                format.file()
            )),
            "{message}"
        );
        assert!(message.contains(expected), "{message} lacks {expected}");
        assert!(message.ends_with("tools work in a terminal."), "{message}");
    }
    for call in bench.calls() {
        assert!(!Path::new(call["cwd"].as_str().unwrap()).exists());
    }
}

#[test]
fn a_failed_run_reports_the_cli_error_and_leaves_no_folder() {
    let bench = Bench::new();
    let host = bench.host(
        "claude-transform.jsonl",
        &[
            ("FAKE_AI_FIXTURE", ""),
            ("FAKE_AI_STDERR", "Error: not logged in"),
            ("FAKE_AI_EXIT", "1"),
        ],
    );
    let Produced::Failed(message) = produced(&host, job(Format::Pdf, PAGE, None)).0 else {
        panic!("a failed CLI produced a file");
    };
    assert!(message.contains("Claude Code"), "{message}");
    let cwd = bench.calls()[0]["cwd"].as_str().unwrap().to_owned();
    assert!(!Path::new(&cwd).exists());
}

#[test]
fn cancel_ends_the_process_tree_and_leaves_nothing() {
    let bench = Bench::new();
    let pids = bench.folder.path().join("pids");
    let host = bench.host(
        "claude-drift.jsonl",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_RENDER", "export.pdf"),
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
    let outcome = smol::block_on(produce(
        &host,
        job(Format::Pdf, PAGE, None),
        canceled.map(|_| ()),
        |_| {},
    ));
    watcher.join().unwrap();
    assert_eq!(outcome, Produced::Canceled);
    for pid in std::fs::read_to_string(&pids).unwrap().split_whitespace() {
        wait_until(&format!("process {pid} ended"), || !alive(pid));
    }
    let cwd = bench.calls()[0]["cwd"].as_str().unwrap().to_owned();
    assert!(
        !Path::new(&cwd).exists(),
        "the half-made file went with its folder"
    );
}

#[test]
fn saving_replaces_only_a_file_the_writer_agreed_to_replace() {
    let folder = tempfile::tempdir().unwrap();
    let target = folder.path().join("Plan.pdf");
    assert_eq!(save(&target, b"first", false), Ok(Saved::Written));
    assert_eq!(save(&target, b"second", false), Ok(Saved::Exists));
    assert_eq!(std::fs::read(&target).unwrap(), b"first");
    assert_eq!(save(&target, b"third", true), Ok(Saved::Written));
    assert_eq!(std::fs::read(&target).unwrap(), b"third");
    let names: Vec<_> = std::fs::read_dir(folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Plan.pdf"]);
    let missing = folder.path().join("gone").join("Plan.pdf");
    let error = save(&missing, b"x", false).unwrap_err();
    assert!(error.starts_with("Cannot write "), "{error}");
    assert!(error.ends_with("Choose a folder you can write to and export again."));
}

/// Installed Claude Code only, and it sends the page to the model: exports a synthetic page as
/// PDF and Word with the writer's own document tools, then reopens both files. Run with
/// `cargo test ... -- --ignored real_cli_exports --nocapture`.
#[test]
#[ignore = "runs the installed Claude Code against its service; run with --ignored"]
fn real_cli_exports_a_synthetic_page_as_pdf_and_word() {
    let host = Host::default();
    let page = "# Quarterly plan\n\n## Goals\n- Ship the native app\n- Keep every note\n\n## Risks\n| Risk | Owner |\n| --- | --- |\n| Scope | Ops |\n";
    let real = |format| Job {
        binary: String::new(),
        ..job(format, page, None)
    };
    let pdf = file(produced(&host, real(Format::Pdf)).0);
    let folder = tempfile::tempdir().unwrap();
    let saved = folder.path().join("Plan.pdf");
    save(&saved, &pdf, false).unwrap();
    let document = quire_media::pdf::Document::open_stored(&saved).unwrap();
    let text: String = (0..document.info().pages.len())
        .map(|page| {
            let text = smol::block_on(document.text(page)).unwrap().unwrap();
            text.text(0..text.chars.len())
        })
        .collect();
    for line in [
        "Quarterly plan",
        "Goals",
        "Ship the native app",
        "Risks",
        "Scope",
    ] {
        assert!(text.contains(line), "{line:?} is missing from the PDF");
    }
    let docx = smol::block_on(check::read_docx(file(
        produced(&host, real(Format::Docx)).0,
    )))
    .unwrap();
    let styles: Vec<&str> = docx.paragraphs.iter().map(|p| p.style.as_str()).collect();
    println!("Word styles: {styles:?}");
    let words: String = docx
        .paragraphs
        .iter()
        .map(|p| p.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    for line in ["Quarterly plan", "Goals", "Ship the native app", "Risks"] {
        assert!(
            words.contains(line),
            "{line:?} is missing from the Word file"
        );
    }
    assert!(styles.iter().any(|style| style.starts_with("Heading")));
}

#[test]
fn a_word_file_without_text_or_with_a_missing_image_is_refused() {
    let paragraph = |text: &str, image: &str| check::Paragraph {
        text: text.into(),
        images: vec![image.into()],
        ..Default::default()
    };
    let docx = |paragraph, part: &str| check::Docx {
        paragraphs: vec![paragraph],
        media: [(part.to_owned(), Vec::new())].into_iter().collect(),
    };
    let image = "word/media/a.png";
    assert_eq!(
        docx(paragraph(" ", image), image).problem(),
        Some("it holds no text".into())
    );
    assert_eq!(
        docx(paragraph("Plan", "word/media/b.png"), image).problem(),
        Some("it shows an image, word/media/b.png, that is not in the file".into())
    );
    assert_eq!(docx(paragraph("Plan", image), image).problem(), None);
}
