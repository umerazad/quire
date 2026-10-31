use super::{CharBox, PageSize, PageText, Renderer, library, page_error, unreadable};
use crate::{Bitmap, MediaError};
use pdfium_render::prelude::*;
use std::{path::Path, sync::OnceLock};

static PDFIUM: OnceLock<Result<Pdfium, String>> = OnceLock::new();

fn pdfium() -> Result<&'static Pdfium, MediaError> {
    PDFIUM
        .get_or_init(|| {
            let path = library::locate().ok_or_else(|| {
                format!(
                    "PDF rendering needs the PDFium library ({}), which was not found. Run `bun tools/fetch-pdfium.ts` in a development checkout, or reinstall Quire, then reopen the page.",
                    library::PDFIUM_BUILD
                )
            })?;
            Pdfium::bind_to_library(&path).map(Pdfium::new).map_err(|error| {
                format!(
                    "Cannot load PDFium from {}: {error}. Run `bun tools/fetch-pdfium.ts` to restore the pinned build, then reopen the page.",
                    path.display()
                )
            })
        })
        .as_ref()
        .map_err(|message| MediaError::Failed(message.clone()))
}

pub(super) struct PdfiumRenderer {
    document: PdfDocument<'static>,
}

pub(super) fn open(path: &Path) -> Result<PdfiumRenderer, MediaError> {
    let document = pdfium()?
        .load_pdf_from_file(path, None)
        .map_err(|_| unreadable())?;
    Ok(PdfiumRenderer { document })
}

impl Renderer for PdfiumRenderer {
    fn page_sizes(&self) -> Vec<PageSize> {
        self.document
            .pages()
            .page_sizes()
            .unwrap_or_default()
            .into_iter()
            .map(|rect| PageSize {
                width: rect.width().value,
                height: rect.height().value,
            })
            .collect()
    }

    fn render(&mut self, page: usize, width: u32) -> Result<Bitmap, MediaError> {
        let index = PdfPageIndex::try_from(page).map_err(|_| page_error(page))?;
        let page_handle = self
            .document
            .pages()
            .get(index)
            .map_err(|_| page_error(page))?;
        let config = PdfRenderConfig::new()
            .set_target_width(width as Pixels)
            .render_form_data(true);
        let bitmap = page_handle
            .render_with_config(&config)
            .map_err(|_| page_error(page))?;
        let (width, height) = (bitmap.width() as u32, bitmap.height() as u32);
        let raw = bitmap.as_raw_bytes();
        let row = width as usize * 4;
        let stride = raw.len().checked_div(height as usize).unwrap_or(row);
        if stride < row {
            return Err(page_error(page));
        }
        let bgra = if stride == row {
            raw
        } else {
            raw.chunks_exact(stride)
                .flat_map(|line| line[..row].iter().copied())
                .collect()
        };
        Ok(Bitmap {
            width,
            height,
            bgra,
        })
    }

    fn text(&mut self, page: usize) -> Result<Option<PageText>, MediaError> {
        let index = PdfPageIndex::try_from(page).map_err(|_| page_error(page))?;
        let page_handle = self
            .document
            .pages()
            .get(index)
            .map_err(|_| page_error(page))?;
        let height = page_handle.height().value;
        let text = page_handle.text().map_err(|_| page_error(page))?;
        let mut result = PageText::default();
        for character in text.chars().iter() {
            let Some(c) = character.unicode_char() else {
                continue;
            };
            let bounds = character.loose_bounds().ok().and_then(|rect| {
                let bounds = CharBox {
                    left: rect.left().value,
                    right: rect.right().value,
                    top: height - rect.top().value,
                    bottom: height - rect.bottom().value,
                };
                (bounds.right > bounds.left && bounds.bottom > bounds.top).then_some(bounds)
            });
            result.chars.push(c);
            result.boxes.push(bounds);
        }
        Ok(Some(result))
    }
}
