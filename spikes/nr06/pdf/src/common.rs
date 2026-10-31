use std::collections::VecDeque;
use std::time::Duration;

pub struct Args {
    pub pdf: String,
    pub width: u32,
    pub keep: usize,
    pub out: Option<String>,
    pub search: String,
}

pub fn args() -> Args {
    let mut it = std::env::args().skip(1);
    let pdf = it
        .next()
        .expect("usage: bench <file.pdf> [width=1600] [keep=6] [png_out_dir|-] [search=tax]");
    let width = it.next().map_or(1600, |s| s.parse().expect("width"));
    let keep = it.next().map_or(6, |s| s.parse().expect("keep"));
    let out = it.next().filter(|s| s != "-");
    let search = it.next().unwrap_or_else(|| "tax".into());
    Args {
        pdf,
        width,
        keep,
        out,
        search,
    }
}

/// Holds the most recent rendered pages, as an inline viewer would for the visible window.
pub struct PageCache {
    keep: usize,
    pages: VecDeque<Vec<u8>>,
    pub peak_bytes: usize,
}

impl PageCache {
    pub fn new(keep: usize) -> Self {
        Self {
            keep,
            pages: VecDeque::new(),
            peak_bytes: 0,
        }
    }
    pub fn push(&mut self, page: Vec<u8>) {
        self.pages.push_back(page);
        while self.pages.len() > self.keep {
            self.pages.pop_front();
        }
        let total = self.pages.iter().map(Vec::len).sum();
        self.peak_bytes = self.peak_bytes.max(total);
    }
}

pub fn stats_ms(times: &mut [Duration]) -> String {
    times.sort();
    let ms = |d: Duration| d.as_secs_f64() * 1000.0;
    let n = times.len();
    if n == 0 {
        return "\"pages\":0".into();
    }
    let total: Duration = times.iter().sum();
    format!(
        "\"pages\":{n},\"total_ms\":{:.1},\"mean_ms\":{:.2},\"p50_ms\":{:.2},\"p95_ms\":{:.2},\"max_ms\":{:.2}",
        ms(total),
        ms(total) / n as f64,
        ms(times[n / 2]),
        ms(times[(n * 95 / 100).min(n - 1)]),
        ms(times[n - 1])
    )
}

/// `<out>/<renderer>-<pdf stem>-p<page>.png`, the naming `diff.sh` expects.
pub fn png_path(dir: &str, renderer: &str, pdf: &str, page: usize) -> String {
    let stem = std::path::Path::new(pdf)
        .file_stem()
        .unwrap()
        .to_string_lossy();
    format!("{dir}/{renderer}-{stem}-p{page}.png")
}

pub fn write_png(path: &str, width: u32, height: u32, rgba: &[u8]) {
    let file = std::fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .and_then(|mut w| w.write_image_data(rgba))
        .expect("write png");
}
