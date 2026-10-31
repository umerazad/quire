//! Pixel difference between two RGBA PNGs over their overlapping area.
//! Prints mean absolute channel error (0-255) and the share of pixels whose
//! largest channel difference exceeds 64, a threshold that ignores antialiasing noise.
fn load(path: &str) -> (u32, u32, Vec<u8>) {
    let dec = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(path).expect(path),
    ));
    let mut reader = dec.read_info().expect("png header");
    let mut buf = vec![0; reader.output_buffer_size()];
    let info = reader.next_frame(&mut buf).expect("png frame");
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "{path}: expected RGBA"
    );
    (info.width, info.height, buf)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (aw, ah, a) = load(&args[0]);
    let (bw, bh, b) = load(&args[1]);
    let (w, h) = (aw.min(bw), ah.min(bh));
    let mut sum = 0u64;
    let mut big = 0u64;
    for y in 0..h {
        for x in 0..w {
            let i = ((y * aw + x) * 4) as usize;
            let j = ((y * bw + x) * 4) as usize;
            let mut worst = 0u8;
            for c in 0..3 {
                let d = a[i + c].abs_diff(b[j + c]);
                sum += d as u64;
                worst = worst.max(d);
            }
            if worst > 64 {
                big += 1;
            }
        }
    }
    let px = (w as u64) * (h as u64);
    println!(
        "{{\"a\":\"{}\",\"b\":\"{}\",\"size_a\":[{aw},{ah}],\"size_b\":[{bw},{bh}],\"mean_abs_error\":{:.3},\"pixels_over_64_pct\":{:.3}}}",
        args[0].rsplit('/').next().unwrap(),
        args[1].rsplit('/').next().unwrap(),
        sum as f64 / (px * 3) as f64,
        big as f64 * 100.0 / px as f64
    );
}
