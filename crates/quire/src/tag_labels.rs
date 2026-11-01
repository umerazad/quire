//! Tags as soft labels in Write, coloured by the reference tag tones, while the Format tags
//! setting is on: a rounded pill in the tag's tone with the tone's icon before the name, as the
//! reference app draws them. Roam's `#[[name]]` tags are pills too. A fold puts the icon in the
//! place of the `#` (and of a Roam tag's brackets), wide enough to draw it whole, so the caret
//! steps over it as one character and the name stays plain, editable text. The folds change only
//! when the page's tags do, never on a caret move. While the caret is on a tag's line the pill
//! steps back to its outline over the name as written, as the reference does in the block being
//! edited. Source always shows raw text, and the Markdown itself never changes.

use crate::markdown_blocks::Block;
use crate::page_analysis::PageAnalysis;
use editor::{
    Addon, Editor, FoldPlaceholder, HighlightKey, NavigationOverlayKey, TextRegion,
    TextRegionLayout, display_map::Crease,
};
use gpui::{
    AnyElement, App, Entity, Focusable as _, HighlightStyle, Hsla, InteractiveElement as _,
    IntoElement, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _,
    Styled as _, WeakEntity, Window, div, px, rgb, svg,
};
use multi_buffer::{
    Anchor, AnchorRangeExt as _, MultiBufferOffset, MultiBufferSnapshot, ToOffset as _,
    ToPoint as _,
};
use quire_core::markdown::TagToken;
use quire_core::tag_tones::{NEUTRAL, TONES, Tone, tone_index};
use settings::Settings as _;
use std::{
    any::{Any, TypeId},
    ops::Range,
    sync::Arc,
};
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

/// Whether tags are drawn as pills: the Format tags setting.
pub(crate) fn formats(cx: &App) -> bool {
    crate::preferences::QuirePreferences::get_global(cx).format_tags
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

/// Marks the folds that draw tag icons.
struct TagIcon;

/// A tag drawn as a pill.
#[derive(Clone)]
struct Pill {
    tone: usize,
    /// The whole tag, `#` or brackets included.
    range: Range<Anchor>,
    /// The name after the icon.
    name: Range<Anchor>,
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
    /// Filled, with the icon and the name.
    Full,
    /// Only the outline, over the name as written, while it is being edited or selected.
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
                    range: pill.name.clone(),
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

/// The icon's side, a little smaller than the text as the reference draws it.
fn icon_size(font_size: Pixels) -> Pixels {
    (font_size * 0.8).round()
}

const ICON_PAD: Pixels = px(4.);
const ICON_GAP: Pixels = px(3.);

/// How wide the fold that draws a tag's icon is: the pill's padding, the icon, and a gap.
fn icon_width(font_size: Pixels) -> Pixels {
    ICON_PAD + icon_size(font_size) + ICON_GAP
}

fn ink(tone_index: usize, dark: bool) -> Hsla {
    let (ink, _) = if dark {
        tone(tone_index).dark
    } else {
        tone(tone_index).light
    };
    rgb(ink).into()
}

fn icon(tone_index: usize) -> SharedString {
    TONES
        .get(tone_index)
        .and_then(|tone| crate::quire_assets::tag_icon_path(tone.name))
        .unwrap_or_else(|| ui::IconName::Hash.path().into())
}

fn icon_element(tone_index: usize, ink: Hsla, font_size: Pixels) -> impl IntoElement {
    svg()
        .path(icon(tone_index))
        .size(icon_size(font_size))
        .flex_none()
        .text_color(ink)
}

/// The fold in place of a tag's `#` (or `#[[`): the tag's icon, in the pill's left end.
fn icon_placeholder(tone_index: usize, dark: bool) -> FoldPlaceholder {
    FoldPlaceholder {
        collapsed_text: Some("#".into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<TagIcon>()),
        render: Arc::new(move |_, _, cx| {
            let font_size = theme_settings::ThemeSettings::get_global(cx).buffer_font_size(cx);
            div()
                .debug_selector(|| "quire-tag-icon".into())
                .flex_none()
                .w(icon_width(font_size))
                .h_full()
                .flex()
                .items_center()
                .pl(ICON_PAD)
                .child(icon_element(tone_index, ink(tone_index, dark), font_size))
                .into_any_element()
        }),
    }
}

/// The fold over a Roam tag's closing brackets.
fn hidden_placeholder() -> FoldPlaceholder {
    FoldPlaceholder {
        collapsed_text: Some("".into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<TagIcon>()),
        render: Arc::new(|_, _, _| div().into_any_element()),
    }
}

fn render(
    pill: &Pill,
    look: Look,
    dark: bool,
    layout: &TextRegionLayout,
    cx: &mut App,
) -> AnyElement {
    let background = if dark {
        tone(pill.tone).dark.1
    } else {
        tone(pill.tone).light.1
    };
    let ink = ink(pill.tone, dark);
    let theme = theme_settings::ThemeSettings::get_global(cx);
    let font = theme.buffer_font.clone();
    let font_size = theme.buffer_font_size(cx);
    let lead = icon_width(font_size);
    let single = layout.fragments.len() == 1;
    let name = pill
        .text
        .trim_start_matches("#[[")
        .trim_start_matches('#')
        .trim_end_matches("]]")
        .to_owned();
    div()
        .size_full()
        .children(layout.fragments.iter().enumerate().map(|(index, bounds)| {
            let local = bounds.origin - layout.bounds.origin;
            let height = pill_height(font_size, bounds.size.height);
            let left = if index == 0 { lead } else { px(2.) };
            let shape = div()
                .absolute()
                .left(local.x - left)
                .top(local.y + (bounds.size.height - height) / 2.)
                .w(bounds.size.width + left + px(4.))
                .h(height)
                .rounded_full()
                .border_1()
                .border_color(ink.opacity(0.18));
            if look == Look::Outline || !single {
                return shape.into_any_element();
            }
            shape
                .debug_selector(|| "quire-tag-pill".into())
                .bg(rgb(background))
                .flex()
                .items_center()
                .pl(ICON_PAD - px(1.))
                .overflow_hidden()
                .child(icon_element(pill.tone, ink, font_size))
                .child(
                    div()
                        .flex_none()
                        .ml(ICON_GAP)
                        .whitespace_nowrap()
                        .font(font.clone())
                        .text_size(font_size)
                        .line_height(height)
                        .text_color(ink)
                        .child(SharedString::from(name.clone())),
                )
                .into_any_element()
        }))
        .into_any_element()
}

/// The page's tags, Roam `#[[name]]` tags included, by start.
fn page_tags(
    page: &Entity<PageAnalysis>,
    text: impl FnOnce() -> String,
    cx: &App,
) -> Vec<TagToken> {
    let Some(inline) = page.read(cx).current_inline(cx) else {
        return Vec::new();
    };
    let mut tags = inline.tags.to_vec();
    if !inline.references.is_empty() {
        let text = text();
        if text.contains("#[[") {
            tags.extend(quire_core::markdown::wiki_tags(&text, &inline.references));
            tags.sort_by_key(|tag| tag.range.start);
        }
    }
    tags
}

/// Paints the page's tags in `editor` when it shows Write and Format tags is on; clears them
/// otherwise. Editors over excerpts are left alone.
pub fn refresh(
    editor: &Entity<Editor>,
    page: &Entity<PageAnalysis>,
    window: &mut Window,
    cx: &mut App,
) {
    let format = formats(cx);
    let dark = !cx.theme().appearance.is_light();
    editor.update(cx, |editor, cx| {
        let keys = keys();
        keys.iter()
            .for_each(|key| editor.clear_highlights(*key, cx));
        editor.register_addon(TagPills::default());
        let buffer = editor.buffer().read(cx);
        let snapshot = buffer.snapshot(cx);
        let everything = MultiBufferOffset(0)..snapshot.len();
        editor.remove_folds_with_type(&[everything], TypeId::of::<TagIcon>(), false, cx);
        let buffer = editor.buffer().read(cx);
        if !format || !crate::editor_projection::enabled(editor) || !buffer.is_singleton() {
            editor.unregister_addon::<TagLinks>();
            return;
        }
        let tokens = page_tags(page, || snapshot.text(), cx);
        let tags = toned(&tokens);
        // Blocks are parsed only for tags to link: parsing the whole page on every edit took
        // most of each keystroke's work on a 1 MiB page without tags.
        let blocks = (!tokens.is_empty())
            .then(|| page.read(cx).current(cx))
            .flatten();
        let links = TagLinks::anchored(
            &tokens,
            blocks.as_ref().map(|parsed| &parsed.blocks[..]),
            &snapshot,
        );
        editor.register_addon(links);
        let anchored = |range: Range<usize>| {
            snapshot.anchor_after(MultiBufferOffset(range.start))
                ..snapshot.anchor_before(MultiBufferOffset(range.end))
        };
        let mut pills = Vec::new();
        let mut folds = Vec::new();
        let mut groups: Vec<Vec<_>> = vec![Vec::new(); keys.len()];
        for (tone, range) in &tags {
            let text: String = snapshot
                .text_for_range(MultiBufferOffset(range.start)..MultiBufferOffset(range.end))
                .collect();
            let wiki = text.starts_with("#[[") && text.ends_with("]]");
            let (lead, tail) = if wiki { (3, 2) } else { (1, 0) };
            if text.len() <= lead + tail {
                continue;
            }
            let name = range.start + lead..range.end - tail;
            folds.push(Crease::simple(
                anchored(range.start..name.start),
                icon_placeholder(*tone, dark),
            ));
            if wiki {
                folds.push(Crease::simple(
                    anchored(name.end..range.end),
                    hidden_placeholder(),
                ));
            }
            groups[*tone].push(anchored(name.clone()));
            pills.push(Pill {
                tone: *tone,
                range: anchored(range.clone()),
                name: anchored(name),
                text: text.into(),
            });
        }
        for (index, ranges) in groups.into_iter().enumerate() {
            if ranges.is_empty() {
                continue;
            }
            editor.highlight_text(
                keys[index],
                ranges,
                HighlightStyle {
                    color: Some(ink(index, dark)),
                    ..Default::default()
                },
                cx,
            );
        }
        editor.fold_creases(folds, false, window, cx);
        editor.register_addon(TagPills { pills, dark });
        cx.notify();
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
    fn a_page_without_tags_is_not_parsed_into_blocks_to_link_them(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let page = cx.add_window(|window, cx| {
            let mut view = crate::editor_host::NotebookWindow::new("- plain\n- text\n", window, cx);
            view.show_written(window, cx);
            view
        });
        cx.run_until_parked();
        let parsed = |cx: &mut TestAppContext| {
            page.update(cx, |view, _, cx| view.analysis().read(cx).blocks_parsed())
                .unwrap()
        };
        assert!(!parsed(cx));
        page.update(cx, |view, window, cx| {
            view.second_editor()
                .update(cx, |editor, cx| editor.insert(" #idea", window, cx))
        })
        .unwrap();
        cx.run_until_parked();
        assert!(parsed(cx), "a tag is linked to its block");
    }

    #[gpui::test]
    fn icons_replace_the_hash_and_roam_tags_are_pills(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "- Wait #[[waiting]] and #idea or #[[Some Page]]\n";
        let page = cx.add_window(|window, cx| {
            let mut view = crate::editor_host::NotebookWindow::new(text, window, cx);
            view.show_written(window, cx);
            view
        });
        cx.run_until_parked();
        let (shown, pills) = page
            .update(cx, |view, window, cx| {
                let editor = view.second_editor().clone();
                let shown = editor.update(cx, |editor, cx| {
                    editor.snapshot(window, cx).display_snapshot.text()
                });
                (shown, pills(editor.read(cx)))
            })
            .unwrap();
        assert_eq!(shown, "Wait #waiting and #idea or #Some Page\n");
        assert_eq!(
            pills,
            vec![
                (tone_index("waiting").unwrap(), "#[[waiting]]".to_owned()),
                (tone_index("idea").unwrap(), "#idea".to_owned()),
                (TONES.len(), "#[[Some Page]]".to_owned()),
            ]
        );
        let mut visual = gpui::VisualTestContext::from_window(page.into(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let icon = visual.debug_bounds("quire-tag-icon").expect("tag icon");
        let font_size = visual
            .update(|_, cx| theme_settings::ThemeSettings::get_global(cx).buffer_font_size(cx));
        assert_eq!(
            icon.size.width,
            icon_width(font_size),
            "the icon is drawn whole"
        );
        assert!(icon.size.width > icon_size(font_size));
        page.update(cx, |view, _, cx| assert_eq!(view.source_text(cx), text))
            .unwrap();
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
