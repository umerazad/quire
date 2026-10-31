//! PDF documents behind one small interface. A notebook-stored PDF (an attachment the user added)
//! renders through PDFium, which also gives text with character boxes for selection and find. A
//! remote or `data:` PDF renders through hayro, which is memory-safe Rust and has no text API, so
//! untrusted bytes never reach C++. Each open document lives on its own thread; callers await
//! replies, and a request whose caller stopped waiting is skipped.

mod hayro_backend;
pub mod library;
mod pdfium_backend;
pub mod testing;

use crate::{Bitmap, MediaError};
use futures::channel::oneshot;
use std::{
    ops::Range,
    path::Path,
    sync::{Arc, mpsc},
};

/// A page's size in PDF points (1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// A character's box in points, measured from the page's top-left corner.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CharBox {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl CharBox {
    fn contains(&self, x: f32, y: f32) -> bool {
        self.left <= x && x <= self.right && self.top <= y && y <= self.bottom
    }

    fn distance(&self, x: f32, y: f32) -> f32 {
        let dx = (self.left - x).max(x - self.right).max(0.);
        let dy = (self.top - y).max(y - self.bottom).max(0.);
        dx * dx + dy * dy * 4.
    }
}

/// A page's text in reading order, with each character's box when it has one.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PageText {
    pub chars: Vec<char>,
    pub boxes: Vec<Option<CharBox>>,
}

impl PageText {
    pub fn text(&self, range: Range<usize>) -> String {
        let end = range.end.min(self.chars.len());
        self.chars[range.start.min(end)..end].iter().collect()
    }

    /// Case-insensitive matches of `query`, as character ranges.
    pub fn find(&self, query: &str) -> Vec<Range<usize>> {
        let needle: Vec<char> = query.chars().map(fold).collect();
        if needle.is_empty() || needle.len() > self.chars.len() {
            return Vec::new();
        }
        let haystack: Vec<char> = self.chars.iter().copied().map(fold).collect();
        let mut found = Vec::new();
        let mut start = 0;
        while start + needle.len() <= haystack.len() {
            if haystack[start..start + needle.len()] == needle[..] {
                found.push(start..start + needle.len());
                start += needle.len();
            } else {
                start += 1;
            }
        }
        found
    }

    /// The character under a point in page points, or the nearest one.
    pub fn char_at(&self, x: f32, y: f32) -> Option<usize> {
        let boxed = || {
            self.boxes
                .iter()
                .enumerate()
                .filter_map(|(index, bounds)| Some((index, (*bounds)?)))
        };
        boxed()
            .find(|(_, bounds)| bounds.contains(x, y))
            .or_else(|| {
                boxed().min_by(|(_, a), (_, b)| a.distance(x, y).total_cmp(&b.distance(x, y)))
            })
            .map(|(index, _)| index)
    }

    /// The boxes covering a character range, for highlighting.
    pub fn boxes(&self, range: Range<usize>) -> Vec<CharBox> {
        let end = range.end.min(self.boxes.len());
        self.boxes[range.start.min(end)..end]
            .iter()
            .flatten()
            .copied()
            .collect()
    }
}

fn fold(c: char) -> char {
    let mut lower = c.to_lowercase();
    match (lower.next(), lower.next()) {
        (Some(single), None) => single,
        _ => c,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Engine {
    Pdfium,
    Hayro,
}

/// What a document's renderer offers, known once it opens.
#[derive(Clone, Debug, PartialEq)]
pub struct Info {
    pub engine: Engine,
    pub pages: Vec<PageSize>,
    /// Whether text selection and find are available.
    pub has_text: bool,
}

/// One open document, owned by the thread its renderer runs on.
pub trait Renderer {
    fn page_sizes(&self) -> Vec<PageSize>;
    /// Renders a page `width` device pixels wide on white.
    fn render(&mut self, page: usize, width: u32) -> Result<Bitmap, MediaError>;
    /// The page's text, or `None` when this renderer has no text API.
    fn text(&mut self, page: usize) -> Result<Option<PageText>, MediaError>;
}

type Job = Box<dyn FnOnce(&mut dyn Renderer) + Send>;

/// Widest page bitmap a request may ask for, in device pixels.
pub const MAX_RENDER_WIDTH: u32 = 4096;
/// Tallest page bitmap, in device pixels; a very tall page renders narrower instead.
pub const MAX_RENDER_HEIGHT: u32 = 8192;
/// Largest PDF read into memory for hayro.
pub const MAX_UNTRUSTED_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone)]
pub struct Document {
    info: Arc<Info>,
    jobs: mpsc::Sender<Job>,
}

impl Document {
    /// Opens a PDF stored in the notebook with PDFium. Blocks; call it on a background thread.
    pub fn open_stored(path: &Path) -> Result<Self, MediaError> {
        let path = path.to_owned();
        Self::spawn(Engine::Pdfium, move || {
            pdfium_backend::open(&path).map(|renderer| Box::new(renderer) as Box<dyn Renderer>)
        })
    }

    /// Opens remote or embedded PDF bytes with hayro. Blocks; call it on a background thread.
    pub fn open_untrusted(bytes: Vec<u8>) -> Result<Self, MediaError> {
        if bytes.len() > MAX_UNTRUSTED_BYTES {
            return Err(crate::source::too_large(MAX_UNTRUSTED_BYTES));
        }
        Self::spawn(Engine::Hayro, move || {
            hayro_backend::open(bytes).map(|renderer| Box::new(renderer) as Box<dyn Renderer>)
        })
    }

    fn spawn(
        engine: Engine,
        open: impl FnOnce() -> Result<Box<dyn Renderer>, MediaError> + Send + 'static,
    ) -> Result<Self, MediaError> {
        let (jobs, receiver) = mpsc::channel::<Job>();
        let (opened, ready) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("quire-pdf".into())
            .spawn(move || {
                let mut renderer = match open() {
                    Ok(renderer) => renderer,
                    Err(error) => {
                        opened.send(Err(error)).ok();
                        return;
                    }
                };
                let pages = renderer.page_sizes();
                let has_text = engine == Engine::Pdfium;
                opened
                    .send(Ok(Info {
                        engine,
                        pages,
                        has_text,
                    }))
                    .ok();
                for job in receiver {
                    job(renderer.as_mut());
                }
            })
            .map_err(|error| {
                MediaError::Failed(format!(
                    "Cannot start a PDF renderer: {error}. Close other PDFs or free memory, then retry."
                ))
            })?;
        let info = ready.recv().map_err(|_| {
            MediaError::Failed(
                "The PDF renderer stopped while opening the file. Open it in the system viewer instead."
                    .into(),
            )
        })??;
        Ok(Self {
            info: Arc::new(info),
            jobs,
        })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    fn ask<T, F>(&self, work: F) -> impl Future<Output = Result<T, MediaError>> + use<T, F>
    where
        T: Send + 'static,
        F: FnOnce(&mut dyn Renderer) -> Result<T, MediaError> + Send + 'static,
    {
        let (reply, answer) = oneshot::channel();
        let job: Job = Box::new(move |renderer| {
            if !reply.is_canceled() {
                reply.send(work(renderer)).ok();
            }
        });
        let sent = self.jobs.send(job).is_ok();
        async move {
            if !sent {
                return Err(stopped());
            }
            answer.await.map_err(|_| stopped())?
        }
    }

    /// Renders `page` (0-based) at `width` device pixels.
    pub fn render(
        &self,
        page: usize,
        width: u32,
    ) -> impl Future<Output = Result<Bitmap, MediaError>> + use<> {
        let tallest = self
            .info
            .pages
            .get(page)
            .filter(|size| size.width > 0. && size.height > 0.)
            .map_or(MAX_RENDER_WIDTH, |size| {
                (MAX_RENDER_HEIGHT as f32 * size.width / size.height) as u32
            });
        let width = width.min(tallest).clamp(1, MAX_RENDER_WIDTH);
        self.ask(move |renderer| renderer.render(page, width))
    }

    /// The text of `page` (0-based), or `None` without a text API.
    pub fn text(
        &self,
        page: usize,
    ) -> impl Future<Output = Result<Option<Arc<PageText>>, MediaError>> + use<> {
        self.ask(move |renderer| renderer.text(page).map(|text| text.map(Arc::new)))
    }
}

fn stopped() -> MediaError {
    MediaError::Failed(
        "The PDF renderer stopped. Reopen the page to retry, or open the PDF in the system viewer."
            .into(),
    )
}

pub(crate) fn page_error(page: usize) -> MediaError {
    MediaError::Failed(format!(
        "Page {} of this PDF could not be rendered. Open it in the system viewer instead.",
        page + 1
    ))
}

pub(crate) fn unreadable() -> MediaError {
    MediaError::Failed(
        "This PDF could not be opened. It may be damaged or password protected; open it in the system viewer instead."
            .into(),
    )
}

#[cfg(test)]
mod tests;
