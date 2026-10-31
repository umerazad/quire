use super::super::*;
use super::media_tests::{enter_write, open_with_attachments, serve};
use super::tests::{Notebook, draw, wait_until};
use crate::media::{KEPT_PAGES, PdfShows};
use gpui::{Modifiers, MouseButton, TestAppContext, VisualTestContext};
use quire_media::pdf::{Engine, testing::simple_pdf};

fn pdfs(
    editor: &Entity<editor::Editor>,
    cx: &mut TestAppContext,
) -> Vec<Entity<crate::media::PdfView>> {
    cx.update(|cx| crate::media::pdfs(editor.read(cx)))
}

fn wait_open(
    notebook: &Notebook,
    editor: &Entity<editor::Editor>,
    count: usize,
    cx: &mut TestAppContext,
) -> Vec<Entity<crate::media::PdfView>> {
    wait_until(cx, "the PDFs to open", |cx| {
        draw(notebook, cx);
        let views = pdfs(editor, cx);
        views.len() == count
            && views
                .iter()
                .all(|view| view.read_with(cx, |view, _| view.shows()) != PdfShows::Loading)
    });
    pdfs(editor, cx)
}

fn long_pdf() -> Vec<u8> {
    let pages: Vec<String> = (1..=30).map(|page| format!("Page {page} alpha")).collect();
    simple_pdf(&pages.iter().map(String::as_str).collect::<Vec<_>>())
}

#[gpui::test]
fn stored_pdfs_render_lazily_open_at_their_page_zoom_and_stay_bounded(cx: &mut TestAppContext) {
    serve(cx);
    let (notebook, _) = open_with_attachments(
        &[("paper.pdf", long_pdf())],
        |files| format!("- Notes\n- ![[{}#page=12]]\n", files[0]),
        cx,
    );
    let editor = enter_write(&notebook, cx);
    let view = wait_open(&notebook, &editor, 1, cx).remove(0);
    assert_eq!(
        view.read_with(cx, |view, _| view.shows()),
        PdfShows::Open {
            engine: Engine::Pdfium,
            pages: 30
        }
    );
    wait_until(cx, "page 12 to render", |cx| {
        draw(&notebook, cx);
        view.read_with(cx, |view, _| {
            view.rendered_pages().iter().any(|(page, _)| *page == 11)
        })
    });
    assert_eq!(
        view.read_with(cx, |view, _| view.top_page()),
        11,
        "#page=12"
    );
    let labels = cx.update(|cx| crate::media::header_labels(editor.read(cx)));
    assert!(
        labels.len() == 1 && labels[0].starts_with("PDF · ") && labels[0].ends_with(".pdf"),
        "{labels:?}"
    );
    let width = view.read_with(cx, |view, _| {
        view.rendered_pages()
            .into_iter()
            .find(|(page, _)| *page == 11)
            .unwrap()
            .1
    });

    view.update(cx, |view, cx| view.set_zoom(2., cx));
    wait_until(cx, "the zoomed page", |cx| {
        draw(&notebook, cx);
        view.read_with(cx, |view, _| {
            view.rendered_pages().contains(&(11, width * 2))
        })
    });
    assert_eq!(view.read_with(cx, |view, _| view.zoom()), 2.);

    for page in 0..30 {
        view.update(cx, |view, cx| view.scroll_to_page(page, cx));
        wait_until(cx, "each page to render", |cx| {
            draw(&notebook, cx);
            view.read_with(cx, |view, _| {
                view.rendered_pages()
                    .iter()
                    .any(|(rendered, _)| *rendered == page)
            })
        });
        let kept = view.read_with(cx, |view, _| view.rendered_pages().len());
        assert!(kept <= KEPT_PAGES, "{kept} pages kept after page {page}");
    }
}

#[gpui::test]
fn find_and_selection_read_the_stored_pdf_text(cx: &mut TestAppContext) {
    serve(cx);
    let (notebook, _) = open_with_attachments(
        &[("paper.pdf", long_pdf())],
        |files| format!("- ![[{}]]\n", files[0]),
        cx,
    );
    let editor = enter_write(&notebook, cx);
    let view = wait_open(&notebook, &editor, 1, cx).remove(0);
    wait_until(cx, "every page's text", |cx| {
        view.read_with(cx, |view, _| view.texts_read()) == 30
    });
    let find = view.read_with(cx, |view, _| view.find_editor());
    notebook
        .window
        .update(cx, |_, window, cx| {
            find.update(cx, |find, cx| find.set_text("page 2", window, cx))
        })
        .unwrap();
    draw(&notebook, cx);
    assert_eq!(
        view.read_with(cx, |view, _| view.matches()),
        (11, Some(0)),
        "Page 2 and Page 20 through 29"
    );
    assert_eq!(view.read_with(cx, |view, _| view.top_page()), 1);
    view.update(cx, |view, cx| view.next_match(true, cx));
    assert_eq!(view.read_with(cx, |view, _| view.matches()), (11, Some(1)));
    assert_eq!(view.read_with(cx, |view, _| view.top_page()), 19);
    view.update(cx, |view, cx| view.next_match(false, cx));
    view.update(cx, |view, cx| view.next_match(false, cx));
    assert_eq!(view.read_with(cx, |view, _| view.matches()), (11, Some(10)));
    assert_eq!(view.read_with(cx, |view, _| view.top_page()), 28);

    view.update(cx, |view, cx| view.scroll_to_page(0, cx));
    draw(&notebook, cx);
    draw(&notebook, cx);
    let (from, to) = view.read_with(cx, |view, _| {
        (
            view.char_center(0, 0).unwrap(),
            view.char_center(0, 3).unwrap(),
        )
    });
    let mut visual = VisualTestContext::from_window(notebook.window.into(), cx);
    visual.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
    visual.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
    assert_eq!(
        view.read_with(cx, |view, _| view.selected_text()),
        Some("Page".into())
    );
    visual.dispatch_action(editor::actions::Copy);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some("Page".into())
    );
}

#[gpui::test]
fn remote_and_data_pdfs_render_with_hayro_and_missing_ones_say_what_to_do(cx: &mut TestAppContext) {
    use base64::Engine as _;
    serve(cx);
    let data = base64::engine::general_purpose::STANDARD.encode(simple_pdf(&["Embedded"]));
    let (notebook, _) = open_with_attachments(
        &[],
        |_| {
            format!(
                "- {{{{pdf: https://example.com/paper.pdf}}}}\n- ![Inline](data:application/pdf;base64,{data})\n- ![[attachments/none.pdf]]\n- {{{{pdf: https://example.com/gone.pdf}}}}\n"
            )
        },
        cx,
    );
    let editor = enter_write(&notebook, cx);
    let views = wait_open(&notebook, &editor, 4, cx);
    let shows: Vec<_> = views
        .iter()
        .map(|view| view.read_with(cx, |view, _| view.shows()))
        .collect();
    assert_eq!(
        shows[0],
        PdfShows::Open {
            engine: Engine::Hayro,
            pages: 2
        }
    );
    assert_eq!(
        shows[1],
        PdfShows::Open {
            engine: Engine::Hayro,
            pages: 1
        }
    );
    assert!(
        matches!(&shows[2], PdfShows::Placeholder(message) if message.contains("not available on this computer")),
        "{:?}",
        shows[2]
    );
    assert!(
        matches!(&shows[3], PdfShows::Placeholder(message) if message.contains("HTTP 404")),
        "{:?}",
        shows[3]
    );
    wait_until(cx, "the remote page", |cx| {
        draw(&notebook, cx);
        views[0].read_with(cx, |view, _| !view.rendered_pages().is_empty())
    });
    assert_eq!(
        cx.update(|cx| crate::media::header_labels(editor.read(cx))),
        [
            "PDF · paper.pdf",
            "PDF · Inline",
            "PDF · none.pdf",
            "PDF · gone.pdf"
        ]
    );
    views[0].update(cx, |view, cx| view.open_externally(cx));
    assert_eq!(
        cx.opened_url().as_deref(),
        Some("https://example.com/paper.pdf")
    );
}
