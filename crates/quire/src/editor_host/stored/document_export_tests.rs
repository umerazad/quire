//! Export as PDF and Export as Word from a notebook window, through the fake CLI: the save
//! dialog, the status line with its steps and Cancel, the attachment that reaches the Word file,
//! and a file that appears during the run, which is replaced only after the writer agrees.

use super::exchange_tests::{dispatch, exchange, finished, open_folder_initialized};
use super::tests::{Notebook, read, wait_until};
use crate::{
    ai::proposal::tests::{Ai, setup},
    document_export::{ExportPagePdf, ExportPageWord, check::read_docx},
    exchange::{CancelExchange, Exchange},
};
use gpui::TestAppContext;
use std::{fs, path::Path};

const PLAN: &str = "# Plan\n\n## Goals\n- Ship by **December**\n- ![Chart](img/chart.png)\n";

async fn notebook(cx: &mut TestAppContext) -> (Ai, Notebook, tempfile::TempDir) {
    let ai = setup(false, cx).await;
    let (notebook, folder) = open_folder_initialized(
        &[("Plan.md", PLAN), ("img/chart.png", "png bytes")],
        "Plan.md",
        cx,
    );
    (ai, notebook, folder)
}

fn uses(ai: &Ai, fixture: &str, extra: &[(&str, &str)], cx: &mut TestAppContext) {
    let host = ai.bench.host(fixture, extra);
    cx.update(|cx| cx.set_global(host));
}

fn choose(target: &Path, cx: &mut TestAppContext) {
    wait_until(cx, "the save dialog", |cx| cx.did_prompt_for_new_path());
    let chosen = target.to_owned();
    cx.simulate_new_path_selection(move |_| Some(chosen));
}

fn label(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    exchange(notebook, cx).read_with(cx, |status, _| {
        status
            .message()
            .map(|message| message.to_string())
            .unwrap_or_default()
    })
}

/// The CLI calls export runs made, leaving out the backend Test.
fn export_calls(ai: &Ai) -> Vec<serde_json::Value> {
    ai.bench
        .calls()
        .into_iter()
        .filter(|call| {
            call["stdin"]
                .as_str()
                .is_some_and(|stdin| stdin.starts_with("Export page.md"))
        })
        .collect()
}

fn run_folder(ai: &Ai) -> String {
    export_calls(ai)[0]["cwd"].as_str().unwrap().to_owned()
}

#[gpui::test]
async fn a_word_export_formats_the_page_with_its_image_and_saves_it(cx: &mut TestAppContext) {
    let (ai, notebook, folder) = notebook(cx).await;
    uses(
        &ai,
        "claude-tools.jsonl",
        &[
            ("FAKE_AI_RENDER", "export.docx"),
            ("FAKE_AI_DELAY_MS", "50"),
        ],
        cx,
    );
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.docx");

    dispatch(&notebook, ExportPageWord, cx);
    choose(&target, cx);
    wait_until(cx, "a step in the status line", |cx| {
        label(&notebook, cx).contains(" · 1 step")
    });
    assert!(
        label(&notebook, cx).starts_with("Claude Code is formatting Plan as Word… "),
        "{}",
        label(&notebook, cx)
    );
    assert_eq!(finished(&notebook, cx), "Exported Plan as Word.");

    let docx = smol::block_on(read_docx(fs::read(&target).unwrap())).unwrap();
    let headings: Vec<(&str, &str)> = docx
        .paragraphs
        .iter()
        .map(|paragraph| (paragraph.style.as_str(), paragraph.text.as_str()))
        .collect();
    assert_eq!(
        headings,
        [
            ("Heading1", "Plan"),
            ("Heading2", "Goals"),
            ("ListBullet", "Ship by **December**"),
            ("ListBullet", ""),
        ]
    );
    assert_eq!(docx.media[&docx.paragraphs[3].images[0]], b"png bytes");
    let calls = export_calls(&ai);
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0]["stdin"],
        "Export page.md, the Quire page \"Plan\", as a Word document saved as export.docx in the working directory."
    );
    assert!(!Path::new(&run_folder(&ai)).exists());
    let shown = exchange(&notebook, cx).read_with(cx, |status, _| match status {
        Exchange::Finished { file, .. } => file.clone(),
        _ => None,
    });
    assert_eq!(shown, Some(target));
    assert!(folder.path().join("Plan.md").exists());
}

#[gpui::test]
async fn cancel_ends_the_run_and_writes_nothing(cx: &mut TestAppContext) {
    let (ai, notebook, _folder) = notebook(cx).await;
    let pids = ai.bench.folder.path().join("pids");
    uses(
        &ai,
        "claude-drift.jsonl",
        &[
            ("FAKE_AI_HANG", "1"),
            ("FAKE_AI_RENDER", "export.pdf"),
            ("FAKE_AI_PIDS", &pids.to_string_lossy()),
        ],
        cx,
    );
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.pdf");

    dispatch(&notebook, ExportPagePdf, cx);
    choose(&target, cx);
    wait_until(cx, "the CLI to start", |cx| {
        pids.exists()
            && exchange(&notebook, cx).read_with(cx, |status, _| {
                matches!(
                    status,
                    Exchange::Running {
                        cancel: Some(_),
                        ..
                    }
                )
            })
    });
    cx.executor()
        .advance_clock(std::time::Duration::from_secs(3));
    cx.run_until_parked();
    let shown = label(&notebook, cx);
    let seconds: u64 = shown
        .strip_prefix("Claude Code is formatting Plan as PDF… ")
        .and_then(|rest| rest.strip_suffix(" s"))
        .and_then(|seconds| seconds.parse().ok())
        .unwrap_or_else(|| panic!("unexpected status line {shown:?}"));
    assert!(seconds >= 3, "the clock moved 3 s but {shown:?}");
    dispatch(&notebook, CancelExchange, cx);
    assert_eq!(
        finished(&notebook, cx),
        "The export was canceled. Nothing was written."
    );
    assert!(!target.exists());
    assert!(!Path::new(&run_folder(&ai)).exists());
    for pid in fs::read_to_string(&pids).unwrap().split_whitespace() {
        wait_until(cx, "the CLI's processes to end", |_| {
            !std::process::Command::new("kill")
                .args(["-0", pid])
                .stderr(std::process::Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        });
    }
}

#[gpui::test]
async fn a_file_that_appears_during_the_run_is_replaced_only_after_asking(cx: &mut TestAppContext) {
    let (ai, notebook, _folder) = notebook(cx).await;
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.pdf");
    for (answer, kept) in [("Cancel", true), ("Replace", false)] {
        let release = ai.bench.folder.path().join(format!("release-{answer}"));
        uses(
            &ai,
            "claude-transform.jsonl",
            &[
                ("FAKE_AI_RENDER", "export.pdf"),
                ("FAKE_AI_DELAY_MS", "200"),
                ("FAKE_AI_PIDS", &release.to_string_lossy()),
            ],
            cx,
        );
        fs::remove_file(&target).ok();
        dispatch(&notebook, ExportPagePdf, cx);
        choose(&target, cx);
        wait_until(cx, "the CLI to start", |_| release.exists());
        fs::write(&target, "written meanwhile").unwrap();
        wait_until(cx, "the replace question", |cx| cx.has_pending_prompt());
        cx.simulate_prompt_answer(answer);
        let message = finished(&notebook, cx);
        if kept {
            assert_eq!(message, "The export was canceled. Nothing was written.");
            assert_eq!(fs::read_to_string(&target).unwrap(), "written meanwhile");
        } else {
            assert_eq!(message, "Exported Plan as PDF.");
            assert!(fs::read(&target).unwrap().starts_with(b"%PDF-"));
        }
    }
    let names: Vec<_> = fs::read_dir(out.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["Plan.pdf"], "no staging file is left");
}

#[gpui::test]
async fn a_file_chosen_in_the_save_dialog_is_replaced_without_asking_again(
    cx: &mut TestAppContext,
) {
    let (ai, notebook, _folder) = notebook(cx).await;
    uses(
        &ai,
        "claude-transform.jsonl",
        &[("FAKE_AI_RENDER", "export.pdf")],
        cx,
    );
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.pdf");
    fs::write(&target, "older export").unwrap();

    dispatch(&notebook, ExportPagePdf, cx);
    choose(&target, cx);
    assert_eq!(finished(&notebook, cx), "Exported Plan as PDF.");
    assert!(!cx.has_pending_prompt());
    assert!(fs::read(&target).unwrap().starts_with(b"%PDF-"));
}

#[gpui::test]
async fn a_malformed_file_is_reported_and_nothing_is_saved(cx: &mut TestAppContext) {
    let (ai, notebook, _folder) = notebook(cx).await;
    uses(
        &ai,
        "claude-transform.jsonl",
        &[("FAKE_AI_WRITE", "export.docx=not a zip")],
        cx,
    );
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.docx");
    dispatch(&notebook, ExportPageWord, cx);
    choose(&target, cx);
    assert_eq!(
        finished(&notebook, cx),
        "Claude Code finished but did not write a valid Word file named export.docx: it does not start as a Word file. Export again, or check that your Word tools work in a terminal."
    );
    assert!(!target.exists());
    assert!(!Path::new(&run_folder(&ai)).exists());
}

#[gpui::test]
async fn exports_explain_why_they_cannot_run_while_ai_is_off(cx: &mut TestAppContext) {
    let (ai, notebook, _folder) = notebook(cx).await;
    cx.update(|cx| crate::ai::settings::set_enabled(false, cx))
        .unwrap();
    cx.run_until_parked();
    dispatch(&notebook, ExportPagePdf, cx);
    cx.run_until_parked();
    assert!(!cx.did_prompt_for_new_path());
    let error = read(&notebook, cx, |view, _| view.nav.error.clone()).unwrap();
    assert_eq!(
        error.to_string(),
        "AI commands are off. Turn them on in Settings > AI after a successful Test. PDF and Word export use the AI command line tool."
    );
    assert!(export_calls(&ai).is_empty());
}
