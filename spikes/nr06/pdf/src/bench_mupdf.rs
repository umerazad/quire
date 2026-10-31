mod common;

use common::{PageCache, args, png_path, stats_ms, write_png};
use mupdf::{Colorspace, Document, Matrix, TextPageFlags};
use std::time::Instant;

fn main() {
    let a = args();
    let t = Instant::now();
    let doc = Document::open(&a.pdf).expect("open pdf");
    let count = doc.page_count().expect("page count");
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;

    let bgr = Colorspace::device_bgr();
    let mut cache = PageCache::new(a.keep);
    let mut times = Vec::new();
    for i in 0..count {
        let t = Instant::now();
        let page = doc.load_page(i).expect("page");
        let b = page.bounds().expect("bounds");
        let s = a.width as f32 / (b.x1 - b.x0);
        // BGR plus alpha is GPUI's BGRA layout; mupdf fills a transparent background.
        let pix = page
            .to_pixmap(&Matrix::new_scale(s, s), &bgr, true, true)
            .expect("render");
        let bgra = pix.samples().to_vec();
        times.push(t.elapsed());
        if let Some(dir) = &a.out
            && (i == 0 || i == 1)
        {
            let mut rgba = bgra.clone();
            for px in rgba.chunks_exact_mut(4) {
                px.swap(0, 2);
                // Premultiplied over white, matching the white page the other renderers paint.
                let under = 255 - px[3];
                for c in &mut px[..3] {
                    *c = c.saturating_add(under);
                }
                px[3] = 255;
            }
            write_png(
                &png_path(dir, "mupdf", &a.pdf, i as usize + 1),
                pix.width(),
                pix.height(),
                &rgba,
            );
        }
        cache.push(bgra);
    }

    let t = Instant::now();
    let mut chars = 0usize;
    let mut hits = 0usize;
    for i in 0..count {
        let page = doc.load_page(i).expect("page");
        chars += page
            .to_text_page(TextPageFlags::empty())
            .and_then(|tp| tp.to_text())
            .map(|s| s.chars().count())
            .unwrap_or(0);
        hits += page.search(&a.search, 10_000).map(|q| q.len()).unwrap_or(0);
    }
    let text_ms = t.elapsed().as_secs_f64() * 1000.0;

    println!(
        "{{\"renderer\":\"mupdf\",\"file\":\"{}\",\"width\":{},\"page_count\":{count},\"open_ms\":{open_ms:.1},{},\"cache_pages\":{},\"cache_peak_mb\":{:.1},\"text_chars\":{chars},\"search\":\"{}\",\"search_hits\":{hits},\"text_and_search_ms\":{text_ms:.1}}}",
        a.pdf,
        a.width,
        stats_ms(&mut times),
        a.keep,
        cache.peak_bytes as f64 / 1048576.0,
        a.search
    );
}
