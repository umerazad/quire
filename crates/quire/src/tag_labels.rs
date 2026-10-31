//! Tags as soft labels in Write, coloured by the reference tag tones, while the Format tags
//! setting is on. Source always shows raw text, and the Markdown itself never changes.

use crate::markdown_blocks::Block;
use crate::page_analysis::PageAnalysis;
use editor::{Editor, HighlightKey, NavigationOverlayKey};
use gpui::{
    AnyElement, App, Entity, Focusable as _, HighlightStyle, InteractiveElement as _,
    IntoElement as _, ParentElement as _, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, rgb,
};
use multi_buffer::{
    Anchor, AnchorRangeExt as _, MultiBufferOffset, MultiBufferSnapshot, ToOffset as _,
};
use quire_core::markdown::TagToken;
use quire_core::tag_tones::{NEUTRAL, TONES, tone_index};
use settings::Settings as _;
use std::{ops::Range, sync::Arc};
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
    let tags: Vec<_> = page
        .read(cx)
        .current_inline(cx)
        .map(|inline| inline.tags.iter().cloned().collect())
        .unwrap_or_default();
    let blocks = page.read(cx).current(cx);
    let blocks = blocks.as_ref().map(|parsed| parsed.blocks.clone());
    let dark = !cx.theme().appearance.is_light();
    editor.update(cx, |editor, cx| {
        let keys = keys();
        keys.iter()
            .for_each(|key| editor.clear_highlights(*key, cx));
        let buffer = editor.buffer().read(cx);
        if !format || !crate::editor_projection::enabled(editor) || !buffer.is_singleton() {
            editor.unregister_addon::<TagLinks>();
            return;
        }
        let snapshot = buffer.snapshot(cx);
        let links = TagLinks::anchored(&tags, blocks.as_deref(), &snapshot);
        editor.register_addon(links);
        let mut groups: Vec<Vec<_>> = vec![Vec::new(); keys.len()];
        for (index, range) in toned(&tags) {
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

/// A painted tag that opens its tag page on click, as rt's `tag-link` marks do, unless the caret
/// of the focused editor is in the tag's block, where a click edits the tag.
#[derive(Clone)]
struct TagLink {
    range: Range<Anchor>,
    block: Range<Anchor>,
    tag: SharedString,
}

struct TagLinks {
    links: Vec<TagLink>,
}

impl TagLinks {
    fn anchored(
        tags: &[TagToken],
        blocks: Option<&[Block]>,
        snapshot: &MultiBufferSnapshot,
    ) -> Self {
        let anchor = |range: Range<usize>| {
            snapshot.anchor_after(MultiBufferOffset(range.start))
                ..snapshot.anchor_before(MultiBufferOffset(range.end))
        };
        let links = blocks
            .map(|blocks| {
                tags.iter()
                    .filter_map(|tag| {
                        let block = blocks
                            .get(blocks.partition_point(|block| block.content_to < tag.range.end))
                            .filter(|block| block.from <= tag.range.start)?;
                        Some(TagLink {
                            range: anchor(tag.range.clone()),
                            block: anchor(block.from..block.content_to),
                            tag: tag.text.clone().into(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { links }
    }
}

impl editor::Addon for TagLinks {
    fn text_regions(
        &self,
        editor: &Entity<Editor>,
        visible: Range<MultiBufferOffset>,
        window: &Window,
        cx: &App,
    ) -> Vec<editor::TextRegion> {
        let view = editor.read(cx);
        let snapshot = view.buffer().read(cx).snapshot(cx);
        let caret = view
            .focus_handle(cx)
            .is_focused(window)
            .then(|| view.selections.newest_anchor().head().to_offset(&snapshot));
        self.links
            .iter()
            .enumerate()
            .filter(|(_, link)| {
                let range = link.range.to_offset(&snapshot);
                let block = link.block.to_offset(&snapshot);
                range.end >= visible.start
                    && range.start <= visible.end
                    && !caret.is_some_and(|caret| block.contains(&caret) || caret == block.end)
            })
            .map(|(ordinal, link)| {
                let tag = link.tag.clone();
                let editor = editor.downgrade();
                editor::TextRegion {
                    range: link.range.clone(),
                    render: Arc::new(move |layout, _| tag_region(&editor, ordinal, &tag, layout)),
                }
            })
            .collect()
    }

    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn tag_region(
    editor: &WeakEntity<Editor>,
    ordinal: usize,
    tag: &SharedString,
    layout: &editor::TextRegionLayout,
) -> AnyElement {
    div()
        .id(("quire-tag-link", ordinal))
        .size_full()
        .children(layout.fragments.iter().enumerate().map(|(index, bounds)| {
            let local = bounds.origin - layout.bounds.origin;
            let editor = editor.clone();
            let tag = tag.clone();
            div()
                .id(index)
                .debug_selector(|| "quire-tag-link".into())
                .absolute()
                .left(local.x)
                .top(local.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .cursor_pointer()
                .role(gpui::Role::Link)
                .aria_label(format!("Show blocks tagged {tag}"))
                .tooltip(ui::Tooltip::text(format!("Show blocks tagged {tag}")))
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    let Some(host) = editor
                        .upgrade()
                        .and_then(|editor| crate::references::host(editor.read(cx)))
                    else {
                        return;
                    };
                    let notebook = host.read(cx).notebook().clone();
                    notebook.show_tag(&tag, window, cx);
                })
        }))
        .into_any_element()
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
