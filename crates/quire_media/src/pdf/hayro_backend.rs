use super::{PageSize, PageText, Renderer, page_error, unreadable};
use crate::{Bitmap, MediaError};
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};

pub(super) struct HayroRenderer {
    pdf: Pdf,
    settings: InterpreterSettings,
}

pub(super) fn open(bytes: Vec<u8>) -> Result<HayroRenderer, MediaError> {
    let pdf = Pdf::new(bytes).map_err(|_| unreadable())?;
    if pdf.pages().is_empty() {
        return Err(unreadable());
    }
    Ok(HayroRenderer {
        pdf,
        settings: InterpreterSettings::default(),
    })
}

impl Renderer for HayroRenderer {
    fn page_sizes(&self) -> Vec<PageSize> {
        self.pdf
            .pages()
            .iter()
            .map(|page| {
                let (width, height) = page.render_dimensions();
                PageSize { width, height }
            })
            .collect()
    }

    fn render(&mut self, page: usize, width: u32) -> Result<Bitmap, MediaError> {
        let pages = self.pdf.pages();
        let handle = pages.get(page).ok_or_else(|| page_error(page))?;
        let (page_width, _) = handle.render_dimensions();
        if page_width <= 0. {
            return Err(page_error(page));
        }
        let scale = width as f32 / page_width;
        let pixmap = render(
            handle,
            &RenderCache::new(),
            &self.settings,
            &RenderSettings::default(),
            &PixmapSettings {
                x_scale: scale,
                y_scale: scale,
                bg_color: WHITE,
            },
        );
        Ok(Bitmap::from_rgba(
            pixmap.width() as u32,
            pixmap.height() as u32,
            pixmap.data_as_u8_slice().to_vec(),
        ))
    }

    fn text(&mut self, _: usize) -> Result<Option<PageText>, MediaError> {
        Ok(None)
    }
}
