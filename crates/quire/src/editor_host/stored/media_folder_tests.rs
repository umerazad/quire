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
    let mut session = NotebookSession::open_folder(root, state.path().into(), None).unwrap();
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

fn attach_key() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd-alt-a"
    } else {
        "ctrl-alt-a"
    }
}

fn caret_at(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    offset: usize,
    cx: &mut TestAppContext,
) {
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([multi_buffer::MultiBufferOffset(offset)
                        ..multi_buffer::MultiBufferOffset(offset)]);
                });
            })
        })
        .unwrap();
    draw(notebook, cx);
}

/// The files in the notebook's attachments folder.
fn attached(folder: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder.join("attachments"))
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

#[gpui::test]
fn attaching_in_a_folder_notebook_copies_into_attachments_and_links_from_the_page(
    cx: &mut TestAppContext,
) {
    serve(cx);
    let original = "- first\n- second\n";
    let (notebook, folder) = open_folder(
        &[("Notes/Page.md", original.as_bytes().to_vec())],
        "Notes/Page.md",
        cx,
    );
    let editor = enter_write(&notebook, cx);
    caret_at(&notebook, &editor, 7, cx);
    let files = tempfile::tempdir().unwrap();
    let picture = files.path().join("Photo:1.PNG");
    std::fs::write(&picture, png(5, 4)).unwrap();
    let notes = files.path().join("notes.txt");
    std::fs::write(&notes, "notes").unwrap();

    cx.simulate_keystrokes(notebook.window.into(), attach_key());
    cx.run_until_parked();
    let chosen = vec![picture, notes];
    cx.simulate_path_prompt_response(move |_| Some(chosen.clone()));
    wait_until(cx, "the links", |cx| {
        cx.update(|cx| editor.read(cx).text(cx)) != original
    });
    let names = attached(folder.path());
    assert_eq!(names.len(), 2, "{names:?}");
    let named = |suffix: &str| {
        names
            .iter()
            .find(|name| {
                name.len() == 9 + suffix.len()
                    && name[..8].bytes().all(|byte| byte.is_ascii_hexdigit())
                    && name[8..] == *format!("-{suffix}")
            })
            .unwrap_or_else(|| panic!("no {suffix} in {names:?}"))
            .clone()
    };
    let (image, text) = (named("Photo-1.PNG"), named("notes.txt"));
    assert_eq!(
        cx.update(|cx| editor.read(cx).text(cx)),
        format!(
            "- first![](<../attachments/{image}>) [{text}](<../attachments/{text}>)\n- second\n"
        )
    );
    assert_eq!(
        std::fs::read(folder.path().join("attachments").join(&image)).unwrap(),
        png(5, 4)
    );
    assert_eq!(
        std::fs::read(folder.path().join("attachments").join(&text)).unwrap(),
        b"notes"
    );
    wait_until(cx, "the attached picture", |cx| {
        draw(&notebook, cx);
        shows(&editor, cx) == [Shows::Picture { natural: (5, 4) }]
    });

    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.undo(&editor::actions::Undo, window, cx)
            })
        })
        .unwrap();
    cx.update(|cx| assert_eq!(editor.read(cx).text(cx), original, "one undo step"));
}

#[cfg(unix)]
#[gpui::test]
fn a_linked_attachments_folder_refuses_attaching_and_leaves_the_page(cx: &mut TestAppContext) {
    let original = "- first\n";
    let (notebook, folder) =
        open_folder(&[("Page.md", original.as_bytes().to_vec())], "Page.md", cx);
    let elsewhere = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(elsewhere.path(), folder.path().join("attachments")).unwrap();
    let editor = enter_write(&notebook, cx);
    caret_at(&notebook, &editor, 7, cx);
    let files = tempfile::tempdir().unwrap();
    let picture = files.path().join("a.png");
    std::fs::write(&picture, png(1, 1)).unwrap();

    cx.simulate_keystrokes(notebook.window.into(), attach_key());
    cx.run_until_parked();
    cx.simulate_path_prompt_response(move |_| Some(vec![picture.clone()]));
    wait_until(cx, "the refusal", |cx| cx.has_pending_prompt());
    assert_eq!(
        cx.pending_prompt().map(|(message, _)| message),
        Some("Could not attach the file: The attachments folder is a link or a file. Replace it with a directory inside the notebook and retry.".into())
    );
    cx.update(|cx| assert_eq!(editor.read(cx).text(cx), original));
    assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
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
