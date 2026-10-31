//! Tags as soft labels in Write, coloured by the reference tag tones, while the Format tags
//! setting is on. Source always shows raw text, and the Markdown itself never changes.

use crate::page_analysis::PageAnalysis;
use editor::{Editor, HighlightKey, NavigationOverlayKey};
use gpui::{App, Entity, HighlightStyle, rgb};
use multi_buffer::MultiBufferOffset;
use quire_core::tag_tones::{NEUTRAL, TONES, tone_index};
use settings::Settings as _;
use std::ops::Range;
use theme::ActiveTheme as _;

struct ToneKey<const N: usize>;

macro_rules! keys {
    ($($n:literal),*) => {
        [$(HighlightKey::NavigationOverlay(NavigationOverlayKey::unique::<ToneKey<$n>>())),*]
    };
}

/// One highlight key per tone, with the neutral look last.
fn keys() -> [HighlightKey; 25] {
    keys!(
        0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24
    )
}

/// The tone index of each tag range on the page, with `TONES.len()` for the neutral look.
pub fn toned(tags: &[quire_core::markdown::TagToken]) -> Vec<(usize, Range<usize>)> {
    tags.iter()
        .map(|tag| {
            (
                tone_index(&tag.text).unwrap_or(TONES.len()),
                tag.range.clone(),
            )
        })
        .collect()
}

/// Paints the page's tags in `editor` when it shows Write and Format tags is on; clears them
/// otherwise. Editors over excerpts are left alone.
pub fn refresh(editor: &Entity<Editor>, page: &Entity<PageAnalysis>, cx: &mut App) {
    let format = crate::preferences::QuirePreferences::get_global(cx).format_tags;
    let tags = page
        .read(cx)
        .current_inline(cx)
        .map(|inline| toned(&inline.tags))
        .unwrap_or_default();
    let dark = !cx.theme().appearance.is_light();
    editor.update(cx, |editor, cx| {
        let keys = keys();
        keys.iter()
            .for_each(|key| editor.clear_highlights(*key, cx));
        let buffer = editor.buffer().read(cx);
        if !format || !crate::editor_projection::enabled(editor) || !buffer.is_singleton() {
            return;
        }
        let snapshot = buffer.snapshot(cx);
        let mut groups: Vec<Vec<_>> = vec![Vec::new(); keys.len()];
        for (index, range) in tags {
            groups[index].push(
                snapshot.anchor_after(MultiBufferOffset(range.start))
                    ..snapshot.anchor_before(MultiBufferOffset(range.end)),
            );
        }
        for (index, ranges) in groups.into_iter().enumerate() {
            if ranges.is_empty() {
                continue;
            }
            let tone = TONES.get(index).unwrap_or(&NEUTRAL);
            let (ink, background) = if dark { tone.dark } else { tone.light };
            editor.highlight_text(
                keys[index],
                ranges,
                HighlightStyle {
                    color: Some(rgb(ink).into()),
                    background_color: Some(rgb(background).into()),
                    ..Default::default()
                },
                cx,
            );
        }
    });
}

/// The tone highlights `editor` paints, as (tone index, ranges), for tests.
#[cfg(test)]
pub fn painted(editor: &Editor, cx: &App) -> Vec<(usize, usize)> {
    keys()
        .iter()
        .enumerate()
        .filter_map(|(index, key)| {
            editor
                .text_highlights(*key, cx)
                .map(|(_, ranges)| (index, ranges.len()))
        })
        .collect()
}
