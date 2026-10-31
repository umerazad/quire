use super::super::*;
use super::tests::{Notebook, draw, read, save, settle, wait_until};
use crate::media::Shows;
use gpui::{TestAppContext, VisualTestContext};
use quire_store::notebook::NotebookStore;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

pub(super) fn png(width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(width, height, image::Rgba([10, 200, 30, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

/// Serves `photo.png` and `paper.pdf` under https://example.com and 404 for anything else,
/// counting requests.
pub(super) fn serve(cx: &mut TestAppContext) -> Arc<AtomicUsize> {
    let requests = Arc::new(AtomicUsize::new(0));
    let counter = requests.clone();
    let client = http_client::FakeHttpClient::create(move |request| {
        counter.fetch_add(1, Ordering::SeqCst);
        let path = request.uri().path().to_owned();
        async move {
            let body = match path.as_str() {
                "/photo.png" => Some(png(6, 3)),
                "/paper.pdf" => Some(quire_media::pdf::testing::simple_pdf(&[
                    "Remote first",
                    "Remote second",
                ])),
                _ => None,
            };
            Ok(match body {
                Some(body) => http_client::Response::builder()
                    .status(200)
                    .body(body.into())
                    .unwrap(),
                None => http_client::Response::builder()
                    .status(404)
                    .body(Default::default())
                    .unwrap(),
            })
        }
    });
    cx.update(|cx| cx.set_http_client(client));
    requests
}

pub(super) fn page_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

pub(super) fn toggle_mode(notebook: &Notebook, cx: &mut TestAppContext) {
    let mode = if cfg!(target_os = "macos") {
        "cmd-shift-m"
    } else {
        "ctrl-shift-m"
    };
    cx.simulate_keystrokes(notebook.window.into(), mode);
}

/// Shows the notebook's page in Write in a large window.
pub(super) fn enter_write(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(1600.), gpui::px(1200.)),
    );
    wait_until(cx, "the page editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    toggle_mode(notebook, cx);
    page_editor(notebook, cx)
}

/// Opens `page` in Write and waits until `count` media views show something other than loading.
pub(super) fn open_in_write(
    notebook: &Notebook,
    count: usize,
    cx: &mut TestAppContext,
) -> Entity<editor::Editor> {
    let editor = enter_write(notebook, cx);
    wait_until(cx, "the media to load", |cx| {
        draw(notebook, cx);
        let shows = shows(&editor, cx);
        shows.len() == count && !shows.contains(&Shows::Loading)
    });
    editor
}

pub(super) fn shows(editor: &Entity<editor::Editor>, cx: &mut TestAppContext) -> Vec<Shows> {
    cx.update(|cx| {
        crate::media::images(editor.read(cx))
            .iter()
            .map(|view| view.read(cx).shows())
            .collect()
    })
}

/// Opens a stored notebook whose attachments were added through the store, as Attach file does,
/// with one page whose text is made from their references.
pub(super) fn open_with_attachments(
    files: &[(&str, Vec<u8>)],
    page: impl FnOnce(&[String]) -> String,
    cx: &mut TestAppContext,
) -> (Notebook, String) {
    cx.update(|cx| init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("local.sqlite");
    let store = NotebookStore::open(&path, "nb", "local").unwrap();
    let references: Vec<String> = files
        .iter()
        .map(|(name, bytes)| store.attach(bytes, name).unwrap())
        .collect();
    let text = page(&references);
    save(&store, "p", "Page.md", &text);
    drop(store);
    let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
    let opened = cx.update(|cx| session.open_page("p".into(), cx));
    let window = cx.add_window(|window, cx| {
        StoredWindow::new(
            session,
            opened.clone(),
            path.clone(),
            "nb".into(),
            window,
            cx,
        )
    });
    let notebook = Notebook {
        _root: root,
        path,
        window,
    };
    settle(&notebook, cx);
    (notebook, text)
}

fn attachments(notebook: &Notebook) -> std::path::PathBuf {
    let directory = notebook.path.parent().unwrap().join("attachments");
    std::fs::create_dir_all(&directory).unwrap();
    directory
}

const GONE: &str = "attachments/gone.png";

#[gpui::test]
fn write_shows_local_remote_and_data_images_with_placeholders_for_missing_ones(
    cx: &mut TestAppContext,
) {
    use base64::Engine as _;
    let requests = serve(cx);
    let data = base64::engine::general_purpose::STANDARD.encode(png(2, 2));
    let (notebook, text) = open_with_attachments(
        &[("local.png", png(4, 2))],
        |local| {
            let local = &local[0];
            format!(
                "- intro\n- ![Local]({local})\n- see ![Photo](https://example.com/photo.png) inline\n- ![](data:image/png;base64,{data})\n- ![Gone]({GONE})\n- ![[{local}]]\n- ![Absent](https://example.com/absent.png)\n- `![Code]({local})`\n"
            )
        },
        cx,
    );
    let local = text.split("](").nth(1).unwrap().split(')').next().unwrap();
    let editor = open_in_write(&notebook, 6, cx);
    let shows = shows(&editor, cx);
    assert_eq!(shows[0], Shows::Picture { natural: (4, 2) });
    assert_eq!(shows[1], Shows::Picture { natural: (6, 3) });
    assert_eq!(shows[2], Shows::Picture { natural: (2, 2) });
    assert!(
        matches!(&shows[3], Shows::Placeholder(message) if message.contains("not available on this computer") && message.contains("retry")),
        "{:?}",
        shows[3]
    );
    assert_eq!(shows[4], Shows::Picture { natural: (4, 2) });
    assert!(
        matches!(&shows[5], Shows::Placeholder(message) if message.contains("HTTP 404") && message.contains("retry")),
        "{:?}",
        shows[5]
    );
    assert_eq!(
        requests.load(Ordering::SeqCst),
        2,
        "one request per remote source"
    );
    let name = local.trim_start_matches("attachments/");
    assert_eq!(
        cx.update(|cx| crate::media::header_labels(editor.read(cx))),
        [
            "Image · Local",
            "Image · Photo",
            "Image · Embedded image",
            "Image · Gone",
            &format!("Image · {name}"),
            "Image · Absent",
        ]
    );
    cx.update(|cx| assert_eq!(editor.read(cx).text(cx), text, "the source is unchanged"));
    let mut visual = VisualTestContext::from_window(notebook.window.into(), cx);
    assert!(visual.debug_bounds("quire-image").is_some());
    assert!(visual.debug_bounds("quire-media-placeholder").is_some());
    let first = cx.update(|cx| {
        crate::media::images(editor.read(cx))[0]
            .read(cx)
            .image()
            .unwrap()
    });
    notebook
        .window
        .update(cx, |_, window, _| {
            assert!(
                window.has_image_atlas_entry(&first),
                "the picture is a texture"
            )
        })
        .unwrap();
    assert!(cx.update(|cx| crate::media::load::MediaCache::cached_bytes(cx)) >= 4 * 2 * 4);

    std::fs::write(attachments(&notebook).join("gone.png"), png(3, 5)).unwrap();
    let missing = cx.update(|cx| crate::media::images(editor.read(cx))[3].clone());
    missing.update(cx, |view, cx| view.retry(cx));
    wait_until(cx, "the retried attachment", |cx| {
        draw(&notebook, cx);
        missing.read_with(cx, |view, _| view.shows()) == Shows::Picture { natural: (3, 5) }
    });

    toggle_mode(&notebook, cx);
    draw(&notebook, cx);
    assert!(
        cx.update(|cx| crate::media::images(editor.read(cx)))
            .is_empty(),
        "Source view shows no media"
    );
}

#[gpui::test]
fn media_references_fold_into_headers_until_the_caret_enters_them(cx: &mut TestAppContext) {
    serve(cx);
    let (notebook, text) = open_with_attachments(
        &[("local.png", png(4, 2))],
        |local| format!("- before\n- ![Local]({})\n- after\n", local[0]),
        cx,
    );
    let editor = open_in_write(&notebook, 1, cx);
    let display = |cx: &mut TestAppContext| {
        notebook
            .window
            .update(cx, |_, window, cx| {
                editor.update(cx, |editor, cx| {
                    editor.snapshot(window, cx).display_snapshot.text()
                })
            })
            .unwrap()
    };
    assert!(display(cx).starts_with("before\n▣\n"), "{:?}", display(cx));
    assert!(!display(cx).contains("![Local]"));
    assert!(text.contains("![Local]"));
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections
                        .select_ranges([multi_buffer::MultiBufferOffset(12)
                            ..multi_buffer::MultiBufferOffset(12)]);
                });
            })
        })
        .unwrap();
    draw(&notebook, cx);
    assert!(display(cx).contains("![Local]"), "{:?}", display(cx));
}
