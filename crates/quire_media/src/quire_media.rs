//! Attachments and inline media for Quire: which Markdown targets name images or PDFs, where a
//! notebook keeps their bytes, bounded image decoding, and PDF rendering behind one interface.
//! Nothing here touches the UI thread; callers run it on a background executor.

pub mod files;
pub mod images;
pub mod pdf;
pub mod source;

use std::fmt;

/// Why media cannot be shown. Every message says what failed and what to do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MediaError {
    /// The file or download is absent; it may appear later.
    Missing(String),
    /// The target is refused: outside the notebook, an unsupported scheme, or over a limit.
    Refused(String),
    /// The bytes could not be decoded or rendered.
    Failed(String),
}

impl MediaError {
    pub fn message(&self) -> &str {
        match self {
            Self::Missing(message) | Self::Refused(message) | Self::Failed(message) => message,
        }
    }
}

impl fmt::Display for MediaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for MediaError {}

/// A decoded picture in GPUI's texture layout: BGRA, four bytes per pixel, rows top to bottom.
#[derive(Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl fmt::Debug for Bitmap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Bitmap({}x{})", self.width, self.height)
    }
}

impl Bitmap {
    /// Converts RGBA pixels in place.
    pub fn from_rgba(width: u32, height: u32, mut rgba: Vec<u8>) -> Self {
        rgba.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|pixel| pixel.swap(0, 2));
        Self {
            width,
            height,
            bgra: rgba,
        }
    }

    pub fn byte_len(&self) -> usize {
        self.bgra.len()
    }
}
