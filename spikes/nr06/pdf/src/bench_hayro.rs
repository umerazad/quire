mod common;

use common::{PageCache, args, png_path, stats_ms, write_png};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use std::time::Instant;

fn main() {
    let a = args();
    let t = Instant::now();
    let data = std::fs::read(&a.pdf).expect("read pdf");
    let pdf = Pdf::new(data).expect("open pdf");
    let pages = pdf.pages();
    let count = pages.len();
    let open_ms = t.elapsed().as_secs_f64() * 1000.0;

    // Default settings use hayro's embedded standard-14 substitutes (feature `embed-fonts`).
    let settings = InterpreterSettings::default();
    let cache = RenderCache::new();
    let mut page_cache = PageCache::new(a.keep);
    let mut times = Vec::new();
    for (i, page) in pages.iter().enumerate() {
        let (w, _) = page.render_dimensions();
        let scale = a.width as f32 / w;
        let t = Instant::now();
        let pixmap = render(
            page,
            &cache,
            &settings,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        // vello_cpu produces premultiplied RGBA; GPUI needs BGRA, so the swizzle is timed.
        let mut bgra = pixmap.data_as_u8_slice().to_vec();
        for px in bgra.chunks_exact_mut(4) {
            px.swap(0, 2);
        }
        times.push(t.elapsed());
        if let Some(dir) = &a.out
            && (i == 0 || i == 1)
        {
            write_png(
                &png_path(dir, "hayro", &a.pdf, i as usize + 1),
                pixmap.width() as u32,
                pixmap.height() as u32,
                pixmap.data_as_u8_slice(),
            );
        }
        page_cache.push(bgra);
    }

    println!(
        "{{\"renderer\":\"hayro\",\"file\":\"{}\",\"width\":{},\"page_count\":{count},\"open_ms\":{open_ms:.1},{},\"cache_pages\":{},\"cache_peak_mb\":{:.1},\"text_extraction\":\"unsupported\"}}",
        a.pdf,
        a.width,
        stats_ms(&mut times),
        a.keep,
        page_cache.peak_bytes as f64 / 1048576.0
    );
}
