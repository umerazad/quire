//! Inline media in Write: `![alt](src)` images and `![[file.pdf]]`, `{{pdf: ...}}` embeds show
//! below their line in a Zed custom block, as full embeds do. The syntax folds into a one-line
//! header while the caret is outside its block and shows as source inside it. Targets resolve
//! against the page's folder and the notebook's attachment files; remote and `data:` sources
//! load too. Loading and decoding never block editing, and a source that fails shows a
//! placeholder saying what to do.

pub(crate) mod attach;
mod image_view;
pub(crate) mod load;
mod pdf_view;

use editor::{
    Addon, Editor, FoldPlaceholder,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId},
};
use gpui::{
    Context, Entity, Focusable as _, Pixels, SharedString, Subscription, WeakEntity, Window,
    prelude::*, px,
};
use image_view::ImageView;
use language::BufferSnapshot;
use multi_buffer::{Anchor, ToOffset};
pub(crate) use pdf_view::PdfView;
use quire_core::markdown::Reference;
use quire_media::{
    MediaError,
    files::Files,
    source::{MediaKind, MediaTarget, media_kind, media_target},
};
use std::{any::TypeId, collections::HashMap, ops::Range, sync::Arc};

/// Binds the PDF viewer keys and the attachment commands.
pub(crate) fn init(cx: &mut gpui::App) {
    pdf_view::bind_keys(cx);
    attach::init(cx);
}

/// A media reference in a page's text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub range: Range<usize>,
    pub block_range: Range<usize>,
    pub target: String,
    pub label: Option<String>,
    /// Set by `{{pdf: ...}}`, which names a PDF whatever its extension.
    pub kind: Option<MediaKind>,
}

/// Whether a reference embeds an image or PDF rather than a page or block.
pub fn is_media_embed(reference: &Reference) -> bool {
    reference.media.is_some()
        || (reference.kind == quire_core::markdown::ReferenceKind::Embed
            && media_kind(&reference.target).is_some())
}

/// The media in a page outside code: Markdown images and media embeds, in text order.
pub fn found(
    snapshot: &BufferSnapshot,
    blocks: &[crate::markdown_blocks::Block],
    references: &[Reference],
) -> Vec<Found> {
    let block_of = |range: &Range<usize>| {
        blocks
            .get(blocks.partition_point(|block| block.content_to <= range.start))
            .filter(|block| {
                block.kind != "code"
                    && block.content_from <= range.start
                    && range.end <= block.content_to
            })
            .map(|block| block.from..block.content_to)
    };
    let mut found: Vec<Found> = references
        .iter()
        .filter(|reference| is_media_embed(reference))
        .filter_map(|reference| {
            Some(Found {
                block_range: block_of(&reference.range)?,
                range: reference.range.clone(),
                target: reference.target.trim().to_owned(),
                label: reference.label.clone(),
                kind: reference.media.map(|_| MediaKind::Pdf),
            })
        })
        .collect();
    let primary = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")
        .map(|layer| layer.node());
    for layer in snapshot
        .syntax_layers()
        .filter(|layer| layer.language.name().as_ref() == "Markdown-Inline")
    {
        let mut stack = vec![layer.node()];
        while let Some(node) = stack.pop() {
            if matches!(node.kind(), "code_span" | "html_tag") {
                continue;
            }
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if node.kind() != "image" {
                stack.extend(children.into_iter().rev());
                continue;
            }
            let range = node.byte_range();
            let in_code = primary.is_some_and(|primary| {
                let mut ancestor = primary.descendant_for_byte_range(range.start, range.end);
                while let Some(node) = ancestor {
                    if matches!(node.kind(), "fenced_code_block" | "indented_code_block") {
                        return true;
                    }
                    ancestor = node.parent();
                }
                false
            });
            let Some(block_range) = block_of(&range).filter(|_| !in_code) else {
                continue;
            };
            let Some(destination) = children
                .iter()
                .find(|child| child.kind() == "link_destination")
            else {
                continue;
            };
            let destination: String = snapshot.text_for_range(destination.byte_range()).collect();
            let target = destination
                .strip_prefix('<')
                .and_then(|inner| inner.strip_suffix('>'))
                .unwrap_or(&destination)
                .to_owned();
            let label = children
                .iter()
                .find(|child| child.kind() == "image_description")
                .map(|label| {
                    snapshot
                        .text_for_range(label.byte_range())
                        .collect::<String>()
                });
            found.push(Found {
                range,
                block_range,
                kind: Some(media_kind(&target).unwrap_or(MediaKind::Image)),
                target,
                label,
            });
        }
    }
    found.sort_by_key(|found| found.range.start);
    found
}

/// A media reference in the view, in view anchors.
#[derive(Clone)]
pub(crate) struct Spot {
    pub range: Range<Anchor>,
    pub block: Range<Anchor>,
    pub source: String,
    pub target: String,
    pub label: Option<String>,
    pub kind: Option<MediaKind>,
}

#[derive(Clone)]
enum View {
    Image(Entity<ImageView>),
    Pdf(Entity<PdfView>),
}

impl View {
    fn height(&self, width: Pixels, cx: &mut gpui::App) -> Pixels {
        match self {
            Self::Image(view) => view.update(cx, |view, _| {
                view.set_width(width);
                view.height()
            }),
            Self::Pdf(view) => view.update(cx, |view, _| {
                view.set_width(width);
                view.height()
            }),
        }
    }

    fn any(&self) -> gpui::AnyView {
        match self {
            Self::Image(view) => view.clone().into(),
            Self::Pdf(view) => view.clone().into(),
        }
    }
}

struct Shown {
    spot: Spot,
    label: SharedString,
    view: View,
    block: CustomBlockId,
    rows: u32,
    _subscription: Subscription,
}

#[derive(Default)]
pub(crate) struct Media {
    shown: Vec<Shown>,
}

impl Addon for Media {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn state(editor: &mut Editor) -> &mut Media {
    if editor.addon::<Media>().is_none() {
        editor.register_addon(Media::default());
    }
    editor.addon_mut::<Media>().expect("registered media")
}

/// The page this editor shows and the notebook's attachment files, for resolving targets.
fn context(editor: &Editor, cx: &gpui::App) -> (Option<String>, Option<Files>) {
    let Some(host) = crate::references::host(editor) else {
        return (None, None);
    };
    let notebook = host.read(cx).notebook().clone();
    let path = editor
        .buffer()
        .read(cx)
        .all_buffers_iter()
        .next()
        .and_then(|buffer| notebook.page_path(&buffer, cx));
    (path, notebook.media_files())
}

/// Shows the media of the current parse, keeping views whose reference is unchanged.
pub(crate) fn sync(
    editor: &mut Editor,
    spots: Vec<Spot>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let (path, files) = context(editor, cx);
    let mut old = std::mem::take(&mut state(editor).shown);
    let mut kept = Vec::new();
    let mut created = Vec::new();
    for spot in spots {
        let at = spot.range.start.to_offset(&snapshot);
        if let Some(index) = old.iter().position(|shown| {
            shown.spot.range.start.to_offset(&snapshot) == at && shown.spot.source == spot.source
        }) {
            let mut shown = old.remove(index);
            shown.spot = spot;
            kept.push(shown);
            continue;
        }
        let target = media_target(
            &spot.target,
            spot.label.as_deref(),
            spot.kind,
            path.as_deref(),
        )
        .unwrap_or_else(|| {
            Err(MediaError::Refused(
                "This is not an image or PDF. Link to a .png, .jpg, .gif, .webp, .svg, or .pdf file."
                    .into(),
            ))
        });
        created.push((spot, target));
    }
    let removed = old.into_iter().map(|shown| shown.block).collect();
    editor.remove_blocks(removed, None, cx);
    let line_height = line_height(editor, window, cx);
    let width = column_width(editor);
    for (spot, target) in created {
        let label = header_label(&spot, &target);
        let kind = target
            .as_ref()
            .map(|target| target.kind)
            .ok()
            .or(spot.kind)
            .or_else(|| media_kind(&spot.target));
        let view = match kind {
            Some(MediaKind::Pdf) => {
                View::Pdf(cx.new(|cx| PdfView::new(target, files.clone(), window, cx)))
            }
            _ => View::Image(cx.new(|cx| ImageView::new(target, files.clone(), cx))),
        };
        let rows = rows(view.height(width, cx), line_height);
        let any = view.any();
        let block = editor.insert_blocks(
            [BlockProperties {
                placement: BlockPlacement::Below(spot.range.start),
                height: Some(rows),
                style: BlockStyle::Flex,
                render: Arc::new(move |block| {
                    gpui::div()
                        .debug_selector(|| "quire-media".into())
                        .ml(block.anchor_x)
                        .w_full()
                        .child(any.clone())
                        .into_any_element()
                }),
                priority: 0,
            }],
            None,
            cx,
        )[0];
        let subscription = match &view {
            View::Image(entity) => cx.observe_in(entity, window, |editor, _, window, cx| {
                resize(editor, window, cx);
            }),
            View::Pdf(entity) => cx.observe_in(entity, window, |editor, _, window, cx| {
                resize(editor, window, cx);
            }),
        };
        kept.push(Shown {
            spot,
            label,
            view,
            block,
            rows,
            _subscription: subscription,
        });
    }
    kept.sort_by_key(|shown| shown.spot.range.start.to_offset(&snapshot));
    state(editor).shown = kept;
    resize(editor, window, cx);
}

fn header_label(spot: &Spot, target: &Result<MediaTarget, MediaError>) -> SharedString {
    match target {
        Ok(target) => match target.kind {
            MediaKind::Image => format!("Image · {}", target.name),
            MediaKind::Pdf => format!("PDF · {}", target.name),
        },
        Err(_) => format!("{} · cannot be shown", spot.target),
    }
    .into()
}

fn line_height(editor: &mut Editor, window: &Window, cx: &gpui::App) -> Pixels {
    editor
        .style(cx)
        .text
        .line_height_in_pixels(window.rem_size())
        .max(px(1.))
}

/// The width media fit into: the editor's text column, or a typical page width before layout.
fn column_width(editor: &Editor) -> Pixels {
    editor.last_bounds().map_or(px(640.), |bounds| {
        (bounds.size.width - px(96.)).max(px(120.))
    })
}

fn rows(height: Pixels, line_height: Pixels) -> u32 {
    (f32::from(height) / f32::from(line_height)).ceil().max(1.) as u32
}

/// Matches each block's height to what its view needs now.
fn resize(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    let line_height = line_height(editor, window, cx);
    let width = column_width(editor);
    let mut shown = std::mem::take(&mut state(editor).shown);
    let mut heights = HashMap::default();
    for shown in &mut shown {
        let wanted = rows(shown.view.height(width, cx), line_height);
        if wanted != shown.rows {
            shown.rows = wanted;
            heights.insert(shown.block, wanted);
        }
    }
    state(editor).shown = shown;
    if !heights.is_empty() {
        editor.resize_blocks(heights, None, cx);
    }
    cx.notify();
}

/// Removes every media block, for Source view.
pub(crate) fn clear(editor: &mut Editor, cx: &mut Context<Editor>) {
    let Some(media) = editor.addon_mut::<Media>() else {
        return;
    };
    let blocks = media.shown.drain(..).map(|shown| shown.block).collect();
    editor.remove_blocks(blocks, None, cx);
}

/// A header for each media reference to fold its syntax into, naming what shows below.
pub(crate) fn headers(
    editor: &Editor,
    host: WeakEntity<Editor>,
    type_tag: TypeId,
) -> Vec<(Spot, FoldPlaceholder)> {
    let Some(media) = editor.addon::<Media>() else {
        return Vec::new();
    };
    media
        .shown
        .iter()
        .map(|shown| {
            let label = shown.label.clone();
            let spot = shown.spot.clone();
            let host = host.clone();
            let placeholder = FoldPlaceholder {
                collapsed_text: Some("▣".into()),
                constrain_width: false,
                merge_adjacent: false,
                type_tag: Some(type_tag),
                render: Arc::new(move |id, _, cx| {
                    use theme::ActiveTheme;
                    let host = host.clone();
                    let start = spot.range.start;
                    crate::editor_projection::inline_control(id, host.clone())
                        .debug_selector(|| "quire-media-header".into())
                        .aria_label(label.clone())
                        .cursor_text()
                        .text_color(cx.theme().colors().text_muted)
                        .child(label.clone())
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            host.update(cx, |editor, cx| {
                                window.focus(&editor.focus_handle(cx), cx);
                                editor.change_selections(
                                    Default::default(),
                                    window,
                                    cx,
                                    |selections| {
                                        selections.select_anchor_ranges([start..start]);
                                    },
                                );
                            })
                            .ok();
                        })
                        .into_any_element()
                }),
            };
            (shown.spot.clone(), placeholder)
        })
        .collect()
}

/// The images this view shows, in page order.
#[cfg(test)]
pub(crate) fn images(editor: &Editor) -> Vec<Entity<ImageView>> {
    editor
        .addon::<Media>()
        .map(|media| {
            media
                .shown
                .iter()
                .filter_map(|shown| match &shown.view {
                    View::Image(view) => Some(view.clone()),
                    View::Pdf(_) => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The PDFs this view shows, in page order.
#[cfg(test)]
pub(crate) fn pdfs(editor: &Editor) -> Vec<Entity<PdfView>> {
    editor
        .addon::<Media>()
        .map(|media| {
            media
                .shown
                .iter()
                .filter_map(|shown| match &shown.view {
                    View::Pdf(view) => Some(view.clone()),
                    View::Image(_) => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The header of each media reference in this view, in page order.
#[cfg(test)]
pub(crate) fn header_labels(editor: &Editor) -> Vec<String> {
    editor
        .addon::<Media>()
        .map(|media| {
            media
                .shown
                .iter()
                .map(|shown| shown.label.to_string())
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
pub(crate) use image_view::Shows;
#[cfg(test)]
pub(crate) use pdf_view::{KEPT_PAGES, PdfShows};

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn finds_images_and_media_embeds_outside_code(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "- ![A](a.png) and ![[Page]] and ![[b.pdf#page=2]]\n- {{pdf: https://x.example/doc}}\n- `![C](c.png)`\n\n```md\n![D](d.png)\n```\n- ![E](<e f.jpg>) ![[g.webp|Gee]]\n";
        let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.executor().run_until_parked();
        let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
        let blocks = crate::markdown_blocks::blocks(&snapshot).unwrap();
        let references = crate::page_analysis::references(&snapshot).unwrap();
        let found: Vec<_> = found(&snapshot, &blocks, &references)
            .into_iter()
            .map(|found| {
                (
                    found.target,
                    found.label,
                    found.kind,
                    text[found.range].to_owned(),
                )
            })
            .collect();
        let item = |target: &str, label: Option<&str>, kind, source: &str| {
            (
                target.to_owned(),
                label.map(str::to_owned),
                kind,
                source.to_owned(),
            )
        };
        assert_eq!(
            found,
            [
                item("a.png", Some("A"), Some(MediaKind::Image), "![A](a.png)"),
                item("b.pdf#page=2", None, None, "![[b.pdf#page=2]]"),
                item(
                    "https://x.example/doc",
                    None,
                    Some(MediaKind::Pdf),
                    "{{pdf: https://x.example/doc}}"
                ),
                item(
                    "e f.jpg",
                    Some("E"),
                    Some(MediaKind::Image),
                    "![E](<e f.jpg>)"
                ),
                item("g.webp", Some("Gee"), None, "![[g.webp|Gee]]"),
            ]
        );
        assert!(
            references
                .iter()
                .filter(|reference| crate::markdown_links::is_full_embed(text, reference))
                .all(|reference| reference.target == "Page"),
            "media embeds are not page embeds"
        );
    }
}
