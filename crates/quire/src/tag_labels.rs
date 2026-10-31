//! Tags as soft labels in Write, coloured by the reference tag tones, while the Format tags
//! setting is on: a rounded pill in the tag's tone with the tone's icon before the name, as the
//! reference app draws them. The icon takes the place of the `#` and the name keeps its place, so
//! text layout, the caret, and clicks are those of the plain text. While the caret is on a tag's
//! line the pill steps back to its outline and the `#` shows, as the reference hides the icon in
//! the block being edited. Source always shows raw text, and the Markdown itself never changes.

use crate::page_analysis::PageAnalysis;
use editor::{Addon, Editor, HighlightKey, NavigationOverlayKey, TextRegion, TextRegionLayout};
use gpui::{
    AnyElement, App, Entity, Focusable as _, HighlightStyle, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _, px, rgb, svg,
};
use multi_buffer::{Anchor, MultiBufferOffset, MultiBufferSnapshot, ToOffset as _, ToPoint as _};
use quire_core::tag_tones::{NEUTRAL, TONES, Tone, tone_index};
use settings::Settings as _;
use std::{any::Any, ops::Range, sync::Arc};
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

fn tone(index: usize) -> &'static Tone {
    TONES.get(index).unwrap_or(&NEUTRAL)
}

/// A tag drawn as a pill.
#[derive(Clone)]
struct Pill {
    tone: usize,
    range: Range<Anchor>,
    text: SharedString,
}

/// The pills of an editor showing Write with Format tags on.
#[derive(Default)]
struct TagPills {
    pills: Vec<Pill>,
    dark: bool,
}

/// How a visible pill is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Look {
    /// Filled, with the tone's icon in place of the `#`.
    Full,
    /// Only the outline, over the plain text, while it is being edited or selected.
    Outline,
}

impl TagPills {
    fn look(
        &self,
        editor: &Editor,
        pill: &Pill,
        snapshot: &MultiBufferSnapshot,
        window: &Window,
        cx: &App,
    ) -> Option<Look> {
        let start = pill.range.start.to_offset(snapshot);
        let end = pill.range.end.to_offset(snapshot);
        if end.0.saturating_sub(start.0) != pill.text.len()
            || !snapshot.contains_str_at(start, &pill.text)
        {
            return None;
        }
        let row = start.to_point(snapshot).row;
        let focused = editor.focus_handle(cx).is_focused(window);
        let touched = editor
            .selections
            .disjoint_anchors_arc()
            .iter()
            .any(|selection| {
                let head = selection.head().to_offset(snapshot);
                let tail = selection.tail().to_offset(snapshot);
                (focused && head.to_point(snapshot).row == row)
                    || (head != tail && head.min(tail) < end && start < head.max(tail))
            });
        Some(if touched { Look::Outline } else { Look::Full })
    }
}

impl Addon for TagPills {
    fn text_regions(
        &self,
        editor: &Entity<Editor>,
        visible: Range<MultiBufferOffset>,
        window: &Window,
        cx: &App,
    ) -> Vec<TextRegion> {
        let view = editor.read(cx);
        let snapshot = view.buffer().read(cx).snapshot(cx);
        let dark = self.dark;
        self.pills
            .iter()
            .filter(|pill| {
                pill.range.end.to_offset(&snapshot) >= visible.start
                    && pill.range.start.to_offset(&snapshot) <= visible.end
            })
            .filter_map(|pill| {
                let look = self.look(view, pill, &snapshot, window, cx)?;
                let pill = pill.clone();
                Some(TextRegion {
                    range: pill.range.clone(),
                    render: Arc::new(move |layout, cx| render(&pill, look, dark, layout, cx)),
                })
            })
            .collect()
    }

    fn to_any(&self) -> &dyn Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

/// The pill's height within a line, as the reference pads its 0.92em text by 0.06em.
fn pill_height(font_size: Pixels, line_height: Pixels) -> Pixels {
    (font_size * 1.4).min(line_height)
}

fn render(
    pill: &Pill,
    look: Look,
    dark: bool,
    layout: &TextRegionLayout,
    cx: &mut App,
) -> AnyElement {
    let (ink, background) = if dark {
        tone(pill.tone).dark
    } else {
        tone(pill.tone).light
    };
    let ink: Hsla = rgb(ink).into();
    let theme = theme_settings::ThemeSettings::get_global(cx);
    let font = theme.buffer_font.clone();
    let font_size = theme.buffer_font_size(cx);
    let icon = TONES
        .get(pill.tone)
        .and_then(|tone| crate::quire_assets::tag_icon_path(tone.name));
    let single = layout.fragments.len() == 1;
    div()
        .size_full()
        .children(layout.fragments.iter().map(|bounds| {
            let local = bounds.origin - layout.bounds.origin;
            let height = pill_height(font_size, bounds.size.height);
            let shape = div()
                .absolute()
                .left(local.x - px(2.))
                .top(local.y + (bounds.size.height - height) / 2.)
                .w(bounds.size.width + px(4.))
                .h(height)
                .rounded_full()
                .border_1()
                .border_color(ink.opacity(0.16));
            if look == Look::Outline || !single {
                return shape.into_any_element();
            }
            let name = match icon {
                Some(_) => pill.text.strip_prefix('#').unwrap_or(&pill.text),
                None => &pill.text,
            };
            shape
                .debug_selector(|| "quire-tag-pill".into())
                .bg(rgb(background))
                .flex()
                .items_center()
                .justify_end()
                .px(px(2.))
                .overflow_hidden()
                .when_some(icon.clone(), |shape, icon| {
                    shape.child(
                        div().flex_1().flex().justify_center().child(
                            svg()
                                .path(icon)
                                .size(font_size * 0.86)
                                .flex_none()
                                .text_color(ink),
                        ),
                    )
                })
                .child(
                    div()
                        .flex_none()
                        .whitespace_nowrap()
                        .font(font.clone())
                        .text_size(font_size)
                        .line_height(height)
                        .text_color(ink)
                        .child(SharedString::from(name.to_owned())),
                )
                .into_any_element()
        }))
        .into_any_element()
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
        editor.register_addon(TagPills::default());
        let buffer = editor.buffer().read(cx);
        if !format || !crate::editor_projection::enabled(editor) || !buffer.is_singleton() {
            return;
        }
        let snapshot = buffer.snapshot(cx);
        let anchored = |range: &Range<usize>| {
            snapshot.anchor_after(MultiBufferOffset(range.start))
                ..snapshot.anchor_before(MultiBufferOffset(range.end))
        };
        let pills = tags
            .iter()
            .map(|(tone, range)| Pill {
                tone: *tone,
                range: anchored(range),
                text: snapshot
                    .text_for_range(MultiBufferOffset(range.start)..MultiBufferOffset(range.end))
                    .collect::<String>()
                    .into(),
            })
            .filter(|pill| !pill.text.is_empty())
            .collect::<Vec<_>>();
        let mut groups: Vec<Vec<_>> = vec![Vec::new(); keys.len()];
        for (index, range) in &tags {
            groups[*index].push(anchored(range));
        }
        for (index, ranges) in groups.into_iter().enumerate() {
            if ranges.is_empty() {
                continue;
            }
            let (ink, _) = if dark {
                tone(index).dark
            } else {
                tone(index).light
            };
            editor.highlight_text(
                keys[index],
                ranges,
                HighlightStyle {
                    color: Some(rgb(ink).into()),
                    ..Default::default()
                },
                cx,
            );
        }
        editor.register_addon(TagPills { pills, dark });
        cx.notify();
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

/// The tags `editor` draws as pills, as (tone index, text), for tests.
#[cfg(test)]
pub fn pills(editor: &Editor) -> Vec<(usize, String)> {
    editor
        .addon::<TagPills>()
        .map(|pills| {
            pills
                .pills
                .iter()
                .map(|pill| (pill.tone, pill.text.to_string()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn draw_pill(
        page: gpui::WindowHandle<crate::editor_host::NotebookWindow>,
        cx: &mut TestAppContext,
    ) -> bool {
        let mut visual = gpui::VisualTestContext::from_window(page.into(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        visual.debug_bounds("quire-tag-pill").is_some()
    }

    #[gpui::test]
    fn tags_are_pills_with_icons_that_step_back_on_the_line_being_edited(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "- first line\n- An #idea and #anything\n";
        let page = cx.add_window(|window, cx| {
            let mut view = crate::editor_host::NotebookWindow::new(text, window, cx);
            view.show_written(window, cx);
            view
        });
        cx.run_until_parked();
        let idea = tone_index("idea").unwrap();
        let shown = page
            .update(cx, |view, _, cx| pills(view.second_editor().read(cx)))
            .unwrap();
        assert_eq!(
            shown,
            vec![
                (idea, "#idea".to_owned()),
                (TONES.len(), "#anything".to_owned())
            ]
        );
        assert!(crate::quire_assets::tag_icon_path(TONES[idea].name).is_some());
        assert!(
            draw_pill(page, cx),
            "tags away from the caret are filled pills"
        );
        page.update(cx, |view, window, cx| {
            let editor = view.second_editor().clone();
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(16)..MultiBufferOffset(16)])
                })
            });
        })
        .unwrap();
        cx.run_until_parked();
        assert!(
            !draw_pill(page, cx),
            "the line being edited shows its tags as written"
        );
        let source = page.update(cx, |view, _, cx| view.source_text(cx)).unwrap();
        assert_eq!(source, text);
    }
}
