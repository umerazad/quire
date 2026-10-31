use super::*;
use futures::executor::block_on;
use std::path::PathBuf;

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/two-pages.pdf")
}

const PAGES: [&str; 2] = ["Alpha Beta", "Gamma (page two) beta"];

#[test]
fn the_checked_in_fixture_is_the_generator_output() {
    if std::env::var_os("QUIRE_REGENERATE_FIXTURES").is_some() {
        std::fs::write(fixture(), testing::simple_pdf(&PAGES)).unwrap();
    }
    assert_eq!(
        std::fs::read(fixture()).unwrap(),
        testing::simple_pdf(&PAGES),
        "regenerate tests/fixtures/two-pages.pdf from testing::simple_pdf"
    );
}

fn dark_pixels(bitmap: &Bitmap) -> usize {
    bitmap
        .bgra
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] < 128 && pixel[1] < 128 && pixel[2] < 128)
        .count()
}

fn check_rendering(document: &Document, engine: Engine) {
    let info = document.info();
    assert_eq!(info.engine, engine);
    assert_eq!(
        info.pages,
        vec![
            PageSize {
                width: 300.,
                height: 200.
            };
            2
        ]
    );
    let page = block_on(document.render(1, 600)).unwrap();
    assert_eq!((page.width, page.height), (600, 400));
    assert_eq!(page.byte_len(), 600 * 400 * 4);
    assert!(dark_pixels(&page) > 200, "the text is drawn");
    assert_eq!(&page.bgra[..4], &[255, 255, 255, 255], "on white");
    let zoomed = block_on(document.render(0, 900)).unwrap();
    assert_eq!((zoomed.width, zoomed.height), (900, 600));
    assert!(block_on(document.render(7, 600)).is_err());
}

#[test]
fn pdfium_renders_pages_and_gives_text_with_boxes() {
    let document = Document::open_stored(&fixture()).unwrap();
    check_rendering(&document, Engine::Pdfium);
    assert!(document.info().has_text);
    let text = block_on(document.text(1)).unwrap().unwrap();
    let all = text.text(0..text.chars.len());
    assert_eq!(all, "Gamma (page two) beta");
    assert_eq!(text.find("BETA"), vec![17..21]);
    assert_eq!(text.find("a"), vec![1..2, 4..5, 8..9, 20..21]);
    let first = text.boxes[0].unwrap();
    assert!(first.left >= 18. && first.left < 24., "{first:?}");
    assert!(first.top > 25. && first.bottom < 60., "{first:?}");
    let under = text.char_at(first.left + 1., (first.top + first.bottom) / 2.);
    assert_eq!(under, Some(0));
    assert_eq!(text.char_at(299., 199.), Some(20), "nearest character");
    assert_eq!(text.boxes(0..5).len(), 5);
}

#[test]
fn hayro_renders_untrusted_bytes_without_text() {
    let document = Document::open_untrusted(testing::simple_pdf(&PAGES)).unwrap();
    check_rendering(&document, Engine::Hayro);
    assert!(!document.info().has_text);
    assert_eq!(block_on(document.text(0)).unwrap(), None);
}

#[test]
fn damaged_and_missing_pdfs_report_what_to_do() {
    let error = Document::open_untrusted(b"%PDF-1.4 nothing".to_vec())
        .err()
        .unwrap();
    assert!(error.message().contains("system viewer"), "{error}");
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("broken.pdf");
    std::fs::write(&path, b"not a pdf").unwrap();
    assert!(matches!(
        Document::open_stored(&path),
        Err(MediaError::Failed(_))
    ));
    assert!(Document::open_stored(&root.path().join("absent.pdf")).is_err());
}

#[test]
fn very_tall_pages_render_within_the_height_bound() {
    let mut bytes = String::from_utf8(testing::simple_pdf(&["tall"])).unwrap();
    bytes = bytes.replace("[0 0 300 200]", "[0 0 300 20000]");
    let document = Document::open_untrusted(bytes.into_bytes()).unwrap();
    let page = block_on(document.render(0, 4000)).unwrap();
    assert!(page.height <= MAX_RENDER_HEIGHT, "{page:?}");
}

#[test]
fn abandoned_requests_are_skipped() {
    let document = Document::open_untrusted(testing::simple_pdf(&PAGES)).unwrap();
    for _ in 0..20 {
        drop(document.render(0, 2000));
    }
    let page = block_on(document.render(0, 300)).unwrap();
    assert_eq!(page.width, 300);
}

#[test]
fn page_text_finds_and_selects_by_character() {
    let text = PageText {
        chars: "Straße Ab".chars().collect(),
        boxes: (0..9)
            .map(|index| {
                Some(CharBox {
                    left: index as f32 * 10.,
                    right: index as f32 * 10. + 9.,
                    top: 0.,
                    bottom: 10.,
                })
            })
            .collect(),
    };
    assert_eq!(text.find("ab"), vec![7..9]);
    assert_eq!(text.find("STRASSE"), Vec::<Range<usize>>::new());
    assert_eq!(text.find("straße"), vec![0..6]);
    assert_eq!(text.find(""), Vec::<Range<usize>>::new());
    assert_eq!(text.char_at(25., 5.), Some(2));
    assert_eq!(text.char_at(500., 5.), Some(8));
    assert_eq!(text.text(2..6), "raße");
    assert_eq!(text.text(7..99), "Ab");
}
