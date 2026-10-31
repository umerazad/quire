use super::super::*;
use super::media_tests::{enter_write, png, serve, shows};
use super::tests::{Notebook, draw, settle, wait_until};
use crate::media::{PdfShows, Shows};
use gpui::TestAppContext;
use quire_media::pdf::{Engine, testing::simple_pdf};
use quire_store::sync::format::page_id_for_path;

/// Opens a folder notebook holding `files`, showing `first`.
fn open_folder(
    files: &[(&str, Vec<u8>)],
    first: &str,
    cx: &mut TestAppContext,
) -> (Notebook, tempfile::TempDir) {
    cx.update(|cx| init(cx).unwrap());
    let folder = tempfile::tempdir().unwrap();
    for (path, bytes) in files {
        let target = folder.path().join(path);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(target, bytes).unwrap();
    }
    let root = std::fs::canonicalize(folder.path()).unwrap();
    let state = tempfile::tempdir().unwrap();
    let mut session = NotebookSession::open_folder(root, state.path().into()).unwrap();
    let page = cx.update(|cx| session.open_page(page_id_for_path(first), cx));
    let database = state.path().join("folder.sqlite");
    let window = cx.add_window(|window, cx| {
        StoredWindow::new(
            session,
            page.clone(),
            database.clone(),
            "folder".into(),
            window,
            cx,
        )
    });
    let notebook = Notebook {
        _root: state,
        path: database,
        window,
    };
    settle(&notebook, cx);
    (notebook, folder)
}

#[gpui::test]
fn folder_notebooks_show_media_by_relative_path_and_refuse_escapes(cx: &mut TestAppContext) {
    serve(cx);
    let page = "- ![Chart](../assets/chart.png)\n- ![[../assets/report.pdf#page=2]]\n- ![Out](../../outside.png)\n- ![Hidden](../.secret/a.png)\n";
    let (notebook, folder) = open_folder(
        &[
            ("Notes/Page.md", page.as_bytes().to_vec()),
            ("assets/chart.png", png(7, 3)),
            (
                "assets/report.pdf",
                simple_pdf(&["Report one", "Report two", "Report three", "Report four"]),
            ),
            (".secret/a.png", png(1, 1)),
        ],
        "Notes/Page.md",
        cx,
    );
    let editor = enter_write(&notebook, cx);
    wait_until(cx, "the folder media", |cx| {
        draw(&notebook, cx);
        let images = shows(&editor, cx);
        let pdfs = cx.update(|cx| crate::media::pdfs(editor.read(cx)));
        images.len() == 3
            && !images.contains(&Shows::Loading)
            && pdfs.len() == 1
            && pdfs[0].read_with(cx, |view, _| view.shows()) != PdfShows::Loading
    });
    let images = shows(&editor, cx);
    assert_eq!(images[0], Shows::Picture { natural: (7, 3) });
    for refused in &images[1..] {
        assert!(
            matches!(refused, Shows::Placeholder(message) if message.contains("cannot be opened") || message.contains("outside the notebook")),
            "{refused:?}"
        );
    }
    let pdf = cx.update(|cx| crate::media::pdfs(editor.read(cx))[0].clone());
    assert_eq!(
        pdf.read_with(cx, |view, _| view.shows()),
        PdfShows::Open {
            engine: Engine::Pdfium,
            pages: 4
        },
        "a PDF inside the notebook folder renders with PDFium"
    );
    assert_eq!(pdf.read_with(cx, |view, _| view.top_page()), 1);
    pdf.update(cx, |view, cx| view.open_externally(cx));
    wait_until(cx, "the system viewer", |cx| cx.opened_url().is_some());
    let opened = cx.opened_url().unwrap();
    assert!(
        opened.starts_with("file://") && opened.ends_with("/assets/report.pdf"),
        "{opened}"
    );
    drop(folder);
}
