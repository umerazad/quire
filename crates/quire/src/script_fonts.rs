//! The Arabic-script fonts the reference app bundles: Noto Naskh Arabic (its "Quire Arabic") and
//! Noto Nastaliq Urdu (its "Quire Urdu"). They are added to the text system at startup, and
//! Naskh ends the editor's font fallbacks, so Arabic and Urdu text gets a font with full shaping
//! and diacritics whatever the editor font. Sources and hashes are in `assets/fonts/SOURCES.txt`.

use gpui::App;
use std::borrow::Cow;

/// The family that Arabic-script text falls back to.
pub const ARABIC: &str = "Noto Naskh Arabic";

macro_rules! font {
    ($path:literal) => {
        Cow::Borrowed(include_bytes!(concat!("../assets/fonts/", $path)).as_slice())
    };
}

pub fn fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        font!("noto-naskh-arabic/NotoNaskhArabic-Regular.ttf"),
        font!("noto-naskh-arabic/NotoNaskhArabic-Medium.ttf"),
        font!("noto-naskh-arabic/NotoNaskhArabic-Bold.ttf"),
        font!("noto-nastaliq-urdu/NotoNastaliqUrdu-Regular.ttf"),
        font!("noto-nastaliq-urdu/NotoNastaliqUrdu-Bold.ttf"),
    ]
}

/// Adds the bundled script fonts to the text system.
pub fn load(cx: &App) -> anyhow::Result<()> {
    cx.text_system().add_fonts(fonts())
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use gpui::{AssetSource as _, Font, FontFallbacks, FontRun, TextSystem, px};

    /// A font file's units per em, ascender, and descender, from its `head` and `hhea` tables.
    fn vertical_metrics(font: &[u8]) -> (u16, i16, i16) {
        let u16_at = |at: usize| u16::from_be_bytes([font[at], font[at + 1]]);
        let table = |tag: &[u8]| {
            (0..u16_at(4) as usize)
                .map(|index| 12 + index * 16)
                .find(|record| &font[*record..*record + 4] == tag)
                .map(|record| u32::from_be_bytes(font[record + 8..record + 12].try_into().unwrap()))
                .unwrap() as usize
        };
        let (head, hhea) = (table(b"head"), table(b"hhea"));
        (
            u16_at(head + 18),
            u16_at(hhea + 4) as i16,
            u16_at(hhea + 6) as i16,
        )
    }

    #[test]
    fn arabic_text_falls_back_to_the_bundled_naskh_even_when_the_editor_font_is_missing() {
        assert_eq!(
            crate::preferences::FONT_FALLBACKS[1..],
            [".ZedMono", ARABIC],
            "Naskh must follow the editor fonts so Latin text keeps them"
        );
        let platform: std::sync::Arc<dyn gpui::PlatformTextSystem> =
            std::sync::Arc::new(gpui_apple::AppleTextSystem::new());
        let text_system = TextSystem::new(platform.clone());
        let lilex = assets::Assets
            .load("fonts/lilex/Lilex-Regular.ttf")
            .unwrap()
            .unwrap();
        text_system.add_fonts(vec![lilex]).unwrap();
        text_system.add_fonts(fonts()).unwrap();
        let names = text_system.all_font_names();
        for family in [ARABIC, "Noto Nastaliq Urdu"] {
            assert!(names.iter().any(|name| name == family), "{family} missing");
        }
        let editor = Font {
            fallbacks: Some(FontFallbacks::from_fonts(
                ["Quire Missing Fallback", ".ZedMono", ARABIC]
                    .map(str::to_owned)
                    .to_vec(),
            )),
            ..gpui::font("Quire Missing Editor Font")
        };
        let editor = text_system.resolve_font(&editor);
        assert!(
            platform.glyph_for_char(editor, 'm').is_some(),
            "a missing editor font gives way to its own fallbacks"
        );
        let text = "Notes: سلام دنیا";
        let layout = platform.layout_line(
            text,
            px(16.),
            &[FontRun {
                len: text.len(),
                font_id: editor,
            }],
        );
        let fonts: Vec<(char, gpui::FontId)> = layout
            .runs
            .iter()
            .flat_map(|run| {
                run.glyphs.iter().filter_map(move |glyph| {
                    Some((text[glyph.index..].chars().next()?, run.font_id))
                })
            })
            .collect();
        let (units, ascender, descender) = vertical_metrics(&super::fonts()[0]);
        let arabic = |ch: char| ('\u{0600}'..='\u{06FF}').contains(&ch);
        assert!(fonts.iter().any(|(ch, _)| arabic(*ch)));
        for (ch, font_id) in fonts {
            if arabic(ch) {
                let metrics = platform.font_metrics(font_id);
                assert_eq!(
                    (
                        metrics.units_per_em,
                        metrics.ascent.round() as i16,
                        metrics.descent.round() as i16
                    ),
                    (units as u32, ascender, descender),
                    "{ch} was set in another font than the bundled Naskh"
                );
            } else if ch.is_ascii_alphabetic() {
                assert_eq!(font_id, editor, "{ch} left the editor font");
            }
        }
    }
}
