mod common;

use common::{PageCache, args, png_path, stats_ms, write_png};
use pdfium_render::prelude::*;
use std::time::Instant;

fn main() {
    let a = args();
    let lib = std::env::var("PDFIUM_LIB").expect("set PDFIUM_LIB to the libpdfium path");
    let t = Instant::now();
    let pdfium = Pdfium::new(Pdfium::bind_to_library(&lib).expect("bind pdfium"));
    let bind_ms = t.elapsed().as_secs_f64() * 1000.0;

    let t = Instant::now();
    let doc = pdfium.load_pdf_from_file(&a.pdf, None).expect("open pdf");
    let pages = doc.pages();
    let count = pages.len();
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;

    let config = PdfRenderConfig::new()
        .set_target_width(a.width as Pixels)
        .render_form_data(true);
    let mut cache = PageCache::new(a.keep);
    let mut times = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        let t = Instant::now();
        let bitmap = page.render_with_config(&config).expect("render");
        // pdfium writes BGRA, which is GPUI's RenderImage layout, so no conversion is timed.
        let bgra = bitmap.as_raw_bytes();
        times.push(t.elapsed());
        if let Some(dir) = &a.out
            && (i == 0 || i == 1)
        {
            let rgba = bitmap.as_rgba_bytes();
            write_png(
                &png_path(dir, "pdfium", &a.pdf, i as usize + 1),
                bitmap.width() as u32,
                bitmap.height() as u32,
                &rgba,
            );
        }
        cache.push(bgra);
    }

    let t = Instant::now();
    let mut chars = 0usize;
    let mut hits = 0usize;
    for page in pages.iter() {
        let text = page.text().expect("text");
        chars += text.all().chars().count();
        let search = text
            .search(&a.search, &PdfSearchOptions::new())
            .expect("search");
        while search.find_next().is_some() {
            hits += 1;
        }
    }
    let text_ms = t.elapsed().as_secs_f64() * 1000.0;

    println!(
        "{{\"renderer\":\"pdfium\",\"file\":\"{}\",\"width\":{},\"page_count\":{count},\"bind_ms\":{bind_ms:.1},\"open_ms\":{open_ms:.1},{},\"cache_pages\":{},\"cache_peak_mb\":{:.1},\"text_chars\":{chars},\"search\":\"{}\",\"search_hits\":{hits},\"text_and_search_ms\":{text_ms:.1}}}",
        a.pdf,
        a.width,
        stats_ms(&mut times),
        a.keep,
        cache.peak_bytes as f64 / 1048576.0,
        a.search
    );
}
