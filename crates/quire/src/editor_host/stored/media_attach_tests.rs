use super::super::*;
use super::media_tests::{enter_write, open_with_attachments, png, serve};
use super::tests::{Notebook, draw, wait_until};
use gpui::{ClipboardItem, TestAppContext, VisualTestContext};
use quire_store::{notebook::NotebookStore, sync::format::attachment_file_name};

fn stored_text(notebook: &Notebook) -> String {
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    store
        .checkout("p", None)
        .unwrap()
        .selected
        .unwrap()
        .root()
        .field("text")
        .as_str()
        .unwrap()
        .to_owned()
}

fn caret_after_first_line(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    cx: &mut TestAppContext,
) {
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([
                        multi_buffer::MultiBufferOffset(7)..multi_buffer::MultiBufferOffset(7)
                    ]);
                });
            })
        })
        .unwrap();
    draw(notebook, cx);
}

#[gpui::test]
fn attach_file_stores_files_and_links_them_in_one_undoable_edit(cx: &mut TestAppContext) {
    serve(cx);
    let (notebook, original) = open_with_attachments(&[], |_| "- first\n- second\n".into(), cx);
    let editor = enter_write(&notebook, cx);
    caret_after_first_line(&notebook, &editor, cx);
    let files = tempfile::tempdir().unwrap();
    let picture = files.path().join("Photo.PNG");
    std::fs::write(&picture, png(5, 4)).unwrap();
    let pdf_bytes = quire_media::pdf::testing::simple_pdf(&["Attached"]);
    let pdf = files.path().join("paper.pdf");
    std::fs::write(&pdf, &pdf_bytes).unwrap();
    let attach = if cfg!(target_os = "macos") {
        "cmd-alt-a"
    } else {
        "ctrl-alt-a"
    };
    cx.simulate_keystrokes(notebook.window.into(), attach);
    cx.run_until_parked();
    let chosen = vec![picture.clone(), pdf.clone()];
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.files && options.multiple && !options.directories);
        Some(chosen.clone())
    });
    let image = format!("attachments/{}", attachment_file_name(&png(5, 4), "x.png"));
    let paper = format!("attachments/{}", attachment_file_name(&pdf_bytes, "x.pdf"));
    let expected = format!("- first![](<{image}>) ![[{paper}]]\n- second\n");
    wait_until(cx, "the links", |cx| {
        cx.update(|cx| editor.read(cx).text(cx)) == expected
    });
    let cache = notebook.path.parent().unwrap();
    assert_eq!(std::fs::read(cache.join(&image)).unwrap(), png(5, 4));
    assert_eq!(std::fs::read(cache.join(&paper)).unwrap(), pdf_bytes);
    wait_until(cx, "the attached media to show", |cx| {
        draw(&notebook, cx);
        cx.update(|cx| {
            let editor = editor.read(cx);
            crate::media::images(editor).len() == 1 && crate::media::pdfs(editor).len() == 1
        })
    });
    wait_until(cx, "the page to save with its attachments", |_| {
        stored_text(&notebook) == expected
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

#[gpui::test]
fn pasting_an_image_attaches_it_and_text_still_pastes(cx: &mut TestAppContext) {
    serve(cx);
    let (notebook, _) = open_with_attachments(&[], |_| "- first\n".into(), cx);
    let editor = enter_write(&notebook, cx);
    caret_after_first_line(&notebook, &editor, cx);
    let bytes = png(3, 3);
    cx.write_to_clipboard(ClipboardItem::new_image(&gpui::Image::from_bytes(
        gpui::ImageFormat::Png,
        bytes.clone(),
    )));
    cx.dispatch_action(notebook.window.into(), editor::actions::Paste);
    let link = format!(
        "![](<attachments/{}>)",
        attachment_file_name(&bytes, "pasted.png")
    );
    wait_until(cx, "the pasted image link", |cx| {
        cx.update(|cx| editor.read(cx).text(cx)) == format!("- first{link}\n")
    });
    cx.write_to_clipboard(ClipboardItem::new_string(" plain".into()));
    cx.dispatch_action(notebook.window.into(), editor::actions::Paste);
    cx.run_until_parked();
    let text = cx.update(|cx| editor.read(cx).text(cx));
    assert!(text.contains("plain"), "{text:?}");
    assert_eq!(text.matches("attachments/").count(), 1);
}

#[gpui::test]
fn dropping_files_on_a_page_attaches_them(cx: &mut TestAppContext) {
    serve(cx);
    let (notebook, _) = open_with_attachments(&[], |_| "- first\n".into(), cx);
    let editor = enter_write(&notebook, cx);
    caret_after_first_line(&notebook, &editor, cx);
    let files = tempfile::tempdir().unwrap();
    let notes = files.path().join("notes.txt");
    std::fs::write(&notes, "dropped").unwrap();
    let bounds = cx.update(|cx| *editor.read(cx).last_bounds().unwrap());
    let position = bounds.center();
    let mut visual = VisualTestContext::from_window(notebook.window.into(), cx);
    visual.simulate_event(gpui::FileDropEvent::Entered {
        position,
        paths: gpui::ExternalPaths(vec![notes, files.path().to_owned()].into()),
    });
    draw(&notebook, cx);
    let mut visual = VisualTestContext::from_window(notebook.window.into(), cx);
    visual.simulate_event(gpui::FileDropEvent::Pending { position });
    visual.simulate_event(gpui::FileDropEvent::Submit { position });
    let link = format!(
        "[{name}](<attachments/{name}>)",
        name = attachment_file_name(b"dropped", "notes.txt")
    );
    wait_until(cx, "the dropped file link", |cx| {
        cx.update(|cx| editor.read(cx).text(cx)) == format!("- first{link}\n")
    });
}
