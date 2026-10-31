//! Bounded image decoding. Encoded size, pixel count, and decoder allocation are capped before
//! any pixels are produced, and the result is downscaled to what inline display needs, so a large
//! photo never becomes a full-resolution texture.

use crate::{Bitmap, MediaError, source::too_large};
use image::{ImageReader, imageops::FilterType};
use std::io::Cursor;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Largest encoded file accepted.
    pub max_bytes: usize,
    /// Largest decoded picture accepted, in pixels.
    pub max_pixels: u64,
    /// Largest side accepted, in pixels.
    pub max_side: u32,
    /// The decoded bitmap is downscaled to fit this box, in device pixels.
    pub display: (u32, u32),
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_bytes: 50 * 1024 * 1024,
            max_pixels: 64 * 1024 * 1024,
            max_side: 16384,
            display: (1600, 1600),
        }
    }
}

/// A decoded image: its bitmap, downscaled to the display box, and its original size.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    pub bitmap: Bitmap,
    pub natural: (u32, u32),
}

/// Decodes PNG, JPEG, GIF (first frame), WebP, BMP, TIFF, ICO, or SVG bytes within `limits`.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Decoded, MediaError> {
    if bytes.len() > limits.max_bytes {
        return Err(too_large(limits.max_bytes));
    }
    if looks_like_svg(bytes) {
        return decode_svg(bytes, limits);
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| unreadable())?;
    if reader.format().is_none() {
        return Err(
            if bytes.starts_with(b"\0\0\0")
                && bytes
                    .get(4..12)
                    .is_some_and(|tag| tag.starts_with(b"ftypavif"))
            {
                MediaError::Failed(
                "AVIF images cannot be shown inline yet. Open the image in the system viewer, or convert it to PNG or JPEG.".into(),
            )
            } else {
                unreadable()
            },
        );
    }
    let mut decoder_limits = image::Limits::default();
    decoder_limits.max_image_width = Some(limits.max_side);
    decoder_limits.max_image_height = Some(limits.max_side);
    decoder_limits.max_alloc = Some(limits.max_pixels.saturating_mul(4));
    reader.limits(decoder_limits.clone());
    let (width, height) = reader.into_dimensions().map_err(map_error)?;
    if u64::from(width) * u64::from(height) > limits.max_pixels {
        return Err(over_pixels());
    }
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| unreadable())?;
    reader.limits(decoder_limits.clone());
    let image = reader.decode().map_err(map_error)?;
    let (width, height) = (image.width(), image.height());
    let (fit_width, fit_height) = fit((width, height), limits.display);
    let rgba = if (fit_width, fit_height) == (width, height) {
        image.into_rgba8()
    } else {
        image::imageops::resize(
            &image.into_rgba8(),
            fit_width,
            fit_height,
            FilterType::Triangle,
        )
    };
    Ok(Decoded {
        bitmap: Bitmap::from_rgba(rgba.width(), rgba.height(), rgba.into_raw()),
        natural: (width, height),
    })
}

/// The largest size with the same aspect ratio within `bounds`, never enlarging.
pub fn fit((width, height): (u32, u32), (max_width, max_height): (u32, u32)) -> (u32, u32) {
    if width == 0 || height == 0 {
        return (width, height);
    }
    let scale = (max_width as f64 / width as f64)
        .min(max_height as f64 / height as f64)
        .min(1.0);
    (
        ((width as f64 * scale).round() as u32).max(1),
        ((height as f64 * scale).round() as u32).max(1),
    )
}

fn looks_like_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start_matches('\u{feff}').trim_start();
    text.starts_with("<svg") || (text.starts_with("<?xml") && text.contains("<svg"))
}

/// Renders an SVG without loading anything it links to: only embedded `data:` images resolve.
fn decode_svg(bytes: &[u8], limits: &Limits) -> Result<Decoded, MediaError> {
    let options = usvg::Options {
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: usvg::ImageHrefResolver::default_data_resolver(),
            resolve_string: Box::new(|_, _| None),
        },
        ..Default::default()
    };
    let tree = usvg::Tree::from_data(bytes, &options).map_err(|_| unreadable())?;
    let size = tree.size();
    let natural = (
        size.width().ceil().max(1.0) as u32,
        size.height().ceil().max(1.0) as u32,
    );
    if natural.0 > limits.max_side || natural.1 > limits.max_side {
        return Err(over_pixels());
    }
    let (width, height) = fit(natural, limits.display);
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height).ok_or_else(unreadable)?;
    let transform = resvg::tiny_skia::Transform::from_scale(
        width as f32 / size.width(),
        height as f32 / size.height(),
    );
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let mut rgba = pixmap.take();
    unpremultiply(&mut rgba);
    Ok(Decoded {
        bitmap: Bitmap::from_rgba(width, height, rgba),
        natural,
    })
}

fn unpremultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let alpha = pixel[3];
        if alpha != 0 && alpha != 255 {
            for channel in &mut pixel[..3] {
                *channel =
                    ((*channel as u16 * 255 + alpha as u16 / 2) / alpha as u16).min(255) as u8;
            }
        }
    }
}

fn map_error(error: image::ImageError) -> MediaError {
    match error {
        image::ImageError::Limits(_) => over_pixels(),
        _ => unreadable(),
    }
}

fn unreadable() -> MediaError {
    MediaError::Failed(
        "This image could not be decoded. Check that the file is a PNG, JPEG, GIF, WebP, or SVG image, or open it in the system viewer.".into(),
    )
}

fn over_pixels() -> MediaError {
    MediaError::Refused(
        "This image has too many pixels to show inline. Open it in the system viewer, or attach a smaller copy.".into(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbaImage::from_fn(width, height, |x, _| {
            image::Rgba([if x % 2 == 0 { 255 } else { 0 }, 0, 255, 255])
        });
        let mut bytes = Cursor::new(Vec::new());
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        bytes.into_inner()
    }

    #[test]
    fn decodes_to_bgra_and_downscales_to_the_display_box() {
        let small = decode(&png(3, 2), &Limits::default()).unwrap();
        assert_eq!(small.natural, (3, 2));
        assert_eq!((small.bitmap.width, small.bitmap.height), (3, 2));
        assert_eq!(&small.bitmap.bgra[..4], &[255, 0, 255, 255]);
        let limits = Limits {
            display: (100, 100),
            ..Limits::default()
        };
        let large = decode(&png(400, 200), &limits).unwrap();
        assert_eq!(large.natural, (400, 200));
        assert_eq!((large.bitmap.width, large.bitmap.height), (100, 50));
        assert_eq!(large.bitmap.byte_len(), 100 * 50 * 4);
    }

    #[test]
    fn refuses_oversized_files_and_pictures_before_decoding_pixels() {
        let bytes = png(64, 64);
        let tight = Limits {
            max_bytes: 16,
            ..Limits::default()
        };
        assert!(matches!(
            decode(&bytes, &tight),
            Err(MediaError::Refused(_))
        ));
        let few_pixels = Limits {
            max_pixels: 1000,
            ..Limits::default()
        };
        assert!(matches!(
            decode(&bytes, &few_pixels),
            Err(MediaError::Refused(_))
        ));
        let short_sides = Limits {
            max_side: 32,
            ..Limits::default()
        };
        assert!(matches!(
            decode(&bytes, &short_sides),
            Err(MediaError::Refused(_))
        ));
        assert!(matches!(
            decode(b"not an image", &Limits::default()),
            Err(MediaError::Failed(_))
        ));
        let mut truncated = bytes.clone();
        truncated.truncate(bytes.len() / 2);
        assert!(matches!(
            decode(&truncated, &Limits::default()),
            Err(MediaError::Failed(_))
        ));
    }

    #[test]
    fn renders_svg_without_following_external_links() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="40" height="20"><rect width="40" height="20" fill="#0000ff"/><image href="/etc/passwd" width="10" height="10"/></svg>"##;
        let decoded = decode(svg, &Limits::default()).unwrap();
        assert_eq!(decoded.natural, (40, 20));
        assert_eq!(&decoded.bitmap.bgra[..4], &[255, 0, 0, 255]);
        let huge = br#"<svg xmlns="http://www.w3.org/2000/svg" width="900000" height="10"/>"#;
        assert!(matches!(
            decode(huge, &Limits::default()),
            Err(MediaError::Refused(_))
        ));
    }

    #[test]
    fn fitting_keeps_the_aspect_ratio_and_never_enlarges() {
        assert_eq!(fit((4000, 1000), (1600, 1600)), (1600, 400));
        assert_eq!(fit((10, 10), (1600, 1600)), (10, 10));
        assert_eq!(fit((1, 10000), (1600, 1600)), (1, 1600));
    }
}
