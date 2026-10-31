use super::{CharBox, PageSize, PageText, Renderer, page_error, unreadable};
use crate::{Bitmap, MediaError};
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, DrawMode, DrawProps, Image, ImageDrawProps,
    InterpreterCache, InterpreterSettings, SoftMask, TransformExt as _, font::Glyph,
    font::GlyphRun, hayro_cmap::BfString, interpret_page,
};
use hayro::hayro_syntax::Pdf;
use hayro::kurbo::{BezPath, Point, Rect};
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

    /// The page's text from its glyphs' Unicode mappings, in drawing order, with boxes in page
    /// points from the top-left corner, as the page renders at scale 1. Invisible text, such as
    /// a scan's recognized words, is included.
    fn text(&mut self, page: usize) -> Result<Option<PageText>, MediaError> {
        let pages = self.pdf.pages();
        let handle = pages.get(page).ok_or_else(|| page_error(page))?;
        let (width, height) = handle.render_dimensions();
        let cache = InterpreterCache::new();
        let mut context = Context::new(
            handle.initial_transform(true).to_kurbo(),
            Rect::new(0., 0., f64::from(width), f64::from(height)),
            &cache,
            handle.xref(),
            self.settings.clone(),
        );
        let mut device = TextDevice::default();
        interpret_page(handle, &mut context, &mut device);
        Ok(Some(device.text))
    }
}

/// Collects each glyph's characters and box, separating words and lines where the PDF leaves a
/// gap instead of drawing a space.
#[derive(Default)]
struct TextDevice {
    text: PageText,
    last: Option<CharBox>,
}

impl TextDevice {
    fn push(&mut self, characters: &str, bounds: Option<CharBox>) {
        if let (Some(last), Some(next)) = (self.last, bounds) {
            let line = (last.bottom - last.top).max(1.);
            let new_line = next.top >= last.bottom - line * 0.2 || next.bottom <= last.top;
            let gap = next.left - last.right > line * 0.15;
            let spaced = self.text.chars.last().is_some_and(|c| c.is_whitespace())
                || characters.starts_with(char::is_whitespace);
            if !spaced && (new_line || gap) {
                self.text.chars.push(if new_line { '\n' } else { ' ' });
                self.text.boxes.push(None);
            }
        }
        for character in characters.chars() {
            self.text.chars.push(character);
            self.text.boxes.push(bounds);
        }
        if bounds.is_some() {
            self.last = bounds;
        }
    }
}

impl<'a> Device<'a> for TextDevice {
    fn draw_path(&mut self, _: &BezPath, _: DrawProps<'a>, _: &DrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_image(&mut self, _: Image<'a, '_>, _: ImageDrawProps<'a>) {}
    fn pop_clip(&mut self) {}
    fn pop_transparency_group(&mut self) {}

    fn draw_glyph_run(&mut self, run: &GlyphRun<'_, 'a>, props: DrawProps<'a>, _: &DrawMode) {
        for glyph in run.glyphs() {
            let Some(unicode) = glyph.as_unicode() else {
                continue;
            };
            let characters = match unicode {
                BfString::Char(character) => character.to_string(),
                BfString::String(text) => text,
            };
            // Outlines use 1000 units to the em; the box spans the advance and a typical
            // ascent and descent, so highlights line up along a line.
            let bounds = match &**glyph {
                Glyph::Outline(outline) => {
                    let advance = f64::from(outline.advance_width().unwrap_or(500.).max(1.));
                    let transform = props.transform * glyph.transform();
                    let corners = [(0., -200.), (advance, -200.), (0., 800.), (advance, 800.)]
                        .map(|(x, y)| transform * Point::new(x, y));
                    let (xs, ys) = (corners.map(|p| p.x as f32), corners.map(|p| p.y as f32));
                    let bounds = CharBox {
                        left: xs.into_iter().fold(f32::INFINITY, f32::min),
                        right: xs.into_iter().fold(f32::NEG_INFINITY, f32::max),
                        top: ys.into_iter().fold(f32::INFINITY, f32::min),
                        bottom: ys.into_iter().fold(f32::NEG_INFINITY, f32::max),
                    };
                    (bounds.right > bounds.left && bounds.bottom > bounds.top).then_some(bounds)
                }
                Glyph::Type3(_) => None,
            };
            self.push(&characters, bounds);
        }
    }
}
