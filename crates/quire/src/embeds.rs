//! Full embeds in Write: a block or page reference standing alone on its line shows the
//! referenced subtree or page below that line, editable in place. The nested editor views the
//! referenced page's own shared buffer (a semantic excerpt for a block), so every occurrence
//! shares one buffer, one history, and that page's save stream. The reference itself folds into
//! a header naming the source, and shows its raw text while the caret is in its block. Embeds
//! nest at most eight deep, and an embed of something already being embedded shows a notice
//! instead.

use crate::references::{Follow, Target};
use editor::{
    Addon, Editor, EditorEvent, EditorMode, FoldPlaceholder, SizingBehavior,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId},
};
use gpui::{App, Context, Entity, SharedString, Subscription, WeakEntity, Window, prelude::*};
use multi_buffer::{Anchor, MultiBuffer, ToOffset};
use std::{any::TypeId, collections::HashMap, ops::Range, rc::Rc, sync::Arc};

/// How deep embeds may nest, as in the reference.
const MAX_DEPTH: usize = 8;

/// A full-embed reference in the view, in view anchors.
#[derive(Clone)]
pub(crate) struct Spot {
    pub range: Range<Anchor>,
    pub block: Range<Anchor>,
    pub source: String,
    pub target: Target,
}

/// Where a reference resolved: a page, and the block within it for block embeds.
#[derive(Clone, PartialEq, Eq)]
struct Resolved {
    page: String,
    title: String,
    block: Option<String>,
}

impl Resolved {
    fn key(&self) -> String {
        match &self.block {
            Some(block) => format!("{}#{block}", self.page),
            None => format!("page:{}", self.page),
        }
    }
}

enum Content {
    Loading,
    Unresolved,
    Recursive,
    Editor {
        editor: Entity<Editor>,
        _model: Option<Entity<crate::semantic_excerpt::SemanticExcerpt>>,
    },
}

struct Shown {
    spot: Spot,
    resolved: Option<Resolved>,
    content: Content,
    /// The block showing the nested editor, with its height in rows.
    block: Option<(CustomBlockId, u32)>,
    _subscriptions: Vec<Subscription>,
}

impl Shown {
    fn label(&self) -> SharedString {
        match (&self.content, &self.resolved) {
            (Content::Loading, _) => "↳ Loading embed…".into(),
            (Content::Unresolved, _) => format!(
                "{} · unresolved. Check the reference in Source.",
                self.spot.source.trim()
            )
            .into(),
            (Content::Recursive, _) => "↳ Recursive embed · open original ↗".into(),
            (Content::Editor { .. }, Some(resolved)) => match resolved.block {
                Some(_) => format!("↳ Embedded block from {} ↗", resolved.title),
                None => format!("↳ Embedded page {} ↗", resolved.title),
            }
            .into(),
            (Content::Editor { .. }, None) => "↳ Embed".into(),
        }
    }
}

/// The embeds of one Write view.
pub(crate) struct Embeds {
    /// The embeds this view is nested in, outermost first.
    chain: Rc<[String]>,
    spots: Vec<Spot>,
    shown: Vec<Shown>,
    targets: HashMap<Target, Option<Resolved>>,
    requests: HashMap<Target, u64>,
    sequence: u64,
    /// Waits for embedded pages still loading, by page.
    loads: HashMap<String, Subscription>,
}

impl Addon for Embeds {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn state(editor: &mut Editor) -> &mut Embeds {
    if editor.addon::<Embeds>().is_none() {
        editor.register_addon(Embeds {
            chain: Rc::from([]),
            spots: Vec::new(),
            shown: Vec::new(),
            targets: HashMap::new(),
            requests: HashMap::new(),
            sequence: 0,
            loads: HashMap::new(),
        });
    }
    editor.addon_mut::<Embeds>().expect("registered embeds")
}

/// Shows the embeds of the current parse, keeping nested editors whose embed is unchanged.
pub(crate) fn sync(
    editor: &mut Editor,
    spots: Vec<Spot>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    state(editor).spots = spots;
    request(editor, false, window, cx);
    place(editor, window, cx);
}

/// The index answers differently: resolve every embed again.
pub(crate) fn requery(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    if editor.addon::<Embeds>().is_some() {
        request(editor, true, window, cx);
    }
}

/// Removes every embed, for Source view.
pub(crate) fn clear(editor: &mut Editor, cx: &mut Context<Editor>) {
    let Some(embeds) = editor.addon_mut::<Embeds>() else {
        return;
    };
    embeds.spots.clear();
    let blocks = embeds
        .shown
        .drain(..)
        .filter_map(|shown| shown.block.map(|(block, _)| block))
        .collect();
    editor.remove_blocks(blocks, None, cx);
}

/// Embed state changed outside a parse: show it, and fold the headers again.
fn changed(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    place(editor, window, cx);
    crate::editor_projection::refold(editor, window, cx);
}

fn request(editor: &mut Editor, again: bool, window: &mut Window, cx: &mut Context<Editor>) {
    let Some(index) =
        crate::references::host(editor).and_then(|host| host.read(cx).index().cloned())
    else {
        return;
    };
    let embeds = state(editor);
    let mut targets: Vec<Target> = embeds
        .spots
        .iter()
        .map(|spot| spot.target.clone())
        .filter(|target| again || !embeds.requests.contains_key(target))
        .collect();
    targets.sort_by_key(|target| format!("{target:?}"));
    targets.dedup();
    for target in targets {
        embeds.sequence += 1;
        let sequence = embeds.sequence;
        embeds.requests.insert(target.clone(), sequence);
        let reply = match &target {
            Target::Page(name) => {
                let reply = index.find_page(name.clone());
                Box::pin(async move {
                    crate::query_index::ask(reply).await.map(|found| {
                        found.map(|page| Resolved {
                            page: page.page.to_lossy_string(),
                            title: page.title,
                            block: None,
                        })
                    })
                }) as std::pin::Pin<Box<dyn std::future::Future<Output = _>>>
            }
            Target::Block(id) => {
                let reply = index.block_preview(id.clone());
                Box::pin(async move {
                    crate::query_index::ask(reply).await.map(|found| {
                        found.map(|preview| Resolved {
                            page: preview.source.page.to_lossy_string(),
                            title: preview.source.title,
                            block: Some(preview.id),
                        })
                    })
                })
            }
        };
        cx.spawn_in(window, async move |editor, cx| {
            let found = reply.await;
            editor
                .update_in(cx, |editor, window, cx| {
                    let embeds = state(editor);
                    if embeds.requests.get(&target) != Some(&sequence) {
                        return;
                    }
                    match found {
                        Ok(found) => {
                            embeds.targets.insert(target, found);
                        }
                        Err(_) => {
                            embeds.requests.remove(&target);
                        }
                    }
                    changed(editor, window, cx);
                })
                .ok();
        })
        .detach();
    }
}

/// Matches shown embeds to the current spots and their resolutions, replacing what changed.
/// Each nested editor sits in a block below the reference's line; Zed keeps a block on a row
/// whose start is folded only when that fold ends within the row, as Write's list marker does.
fn place(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let embeds = state(editor);
    let chain = embeds.chain.clone();
    let wanted: Vec<(Spot, Option<Option<Resolved>>)> = embeds
        .spots
        .iter()
        .map(|spot| (spot.clone(), embeds.targets.get(&spot.target).cloned()))
        .collect();
    let mut old = std::mem::take(&mut embeds.shown);
    let mut kept = Vec::new();
    for (spot, resolution) in wanted {
        let resolved = resolution.clone().flatten();
        let at = spot.range.start.to_offset(&snapshot);
        let position = old.iter().position(|shown| {
            shown.spot.range.start.to_offset(&snapshot) == at
                && shown.spot.target == spot.target
                && shown.resolved == resolved
                && !(matches!(shown.content, Content::Loading) && resolution.is_some())
        });
        if let Some(index) = position {
            let mut shown = old.remove(index);
            shown.spot = spot;
            kept.push(shown);
            continue;
        }
        let content = match &resolution {
            None => Content::Loading,
            Some(None) => Content::Unresolved,
            Some(Some(resolved)) if chain.contains(&resolved.key()) || chain.len() >= MAX_DEPTH => {
                Content::Recursive
            }
            Some(Some(resolved)) => {
                nested(editor, resolved, &chain, window, cx).unwrap_or(Content::Loading)
            }
        };
        let (block, subscriptions) = match &content {
            Content::Editor { editor: nested, .. } => {
                let rows = content_rows(nested, cx);
                let block = editor.insert_blocks(
                    [BlockProperties {
                        placement: BlockPlacement::Below(spot.range.start),
                        height: Some(rows),
                        style: BlockStyle::Flex,
                        render: render(nested.clone()),
                        priority: 0,
                    }],
                    None,
                    cx,
                )[0];
                let resized =
                    cx.subscribe_in(nested, window, |editor, _, event: &EditorEvent, _, cx| {
                        if matches!(event, EditorEvent::BufferEdited) {
                            resize(editor, cx);
                        }
                    });
                (Some((block, rows)), vec![resized])
            }
            _ => (None, Vec::new()),
        };
        kept.push(Shown {
            spot,
            resolved,
            content,
            block,
            _subscriptions: subscriptions,
        });
    }
    let removed = old
        .into_iter()
        .filter_map(|shown| shown.block.map(|(block, _)| block))
        .collect();
    editor.remove_blocks(removed, None, cx);
    kept.sort_by_key(|shown| shown.spot.range.start.to_offset(&snapshot));
    state(editor).shown = kept;
    resize(editor, cx);
    cx.notify();
}

/// The rows a nested editor needs, including any embeds nested in it.
fn content_rows(nested: &Entity<Editor>, cx: &mut App) -> u32 {
    nested.update(cx, |nested, cx| nested.max_point(cx).row().0 + 1)
}

fn resize(editor: &mut Editor, cx: &mut Context<Editor>) {
    let mut heights = HashMap::default();
    let mut shown = std::mem::take(&mut state(editor).shown);
    for shown in &mut shown {
        if let (Content::Editor { editor: nested, .. }, Some((block, rows))) =
            (&shown.content, &mut shown.block)
        {
            let wanted = content_rows(nested, cx);
            if wanted != *rows {
                *rows = wanted;
                heights.insert(*block, wanted);
            }
        }
    }
    state(editor).shown = shown;
    if !heights.is_empty() {
        editor.resize_blocks(heights, None, cx);
    }
}

/// The nested editor for a resolved embed, once its page has loaded through the notebook.
fn nested(
    editor: &mut Editor,
    resolved: &Resolved,
    chain: &Rc<[String]>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Option<Content> {
    let host = crate::references::host(editor)?;
    let notebook = host.read(cx).notebook().clone();
    let page = notebook.open_page(&resolved.page, window, cx)?;
    let Some(analysis) = page.read(cx).loaded().map(|loaded| loaded.analysis.clone()) else {
        let loads = &mut state(editor).loads;
        if !loads.contains_key(&resolved.page) {
            let subscription = cx.observe_in(&page, window, |editor, page, window, cx| {
                if page.read(cx).loaded().is_some() {
                    changed(editor, window, cx);
                }
            });
            loads.insert(resolved.page.clone(), subscription);
        }
        return None;
    };
    let mut chain = chain.to_vec();
    chain.push(resolved.key());
    let chain: Rc<[String]> = chain.into();
    let (buffer, model) = match &resolved.block {
        Some(id) => {
            let model = cx.new(|cx| {
                crate::semantic_excerpt::SemanticExcerpt::new(analysis.clone(), id.clone(), cx)
            });
            (model.read(cx).buffer.clone(), Some(model))
        }
        None => {
            let source = analysis.read(cx).source.clone();
            (cx.new(|cx| MultiBuffer::singleton(source, cx)), None)
        }
    };
    let nested = cx.new(|cx| {
        let mut nested = Editor::new(
            EditorMode::Full {
                scale_ui_elements_with_buffer_font_size: true,
                show_active_line_background: false,
                sizing_behavior: SizingBehavior::SizeByContent,
            },
            buffer,
            None,
            window,
            cx,
        );
        state(&mut nested).chain = chain;
        nested
    });
    if let Some(model) = &model {
        crate::semantic_excerpt::attach(&nested, model, window, cx);
    }
    crate::references::install(&nested, &host, window, cx);
    crate::editor_projection::bind(&nested, window, cx).ok()?;
    Some(Content::Editor {
        editor: nested,
        _model: model,
    })
}

fn render(nested: Entity<Editor>) -> editor::display_map::RenderBlock {
    Arc::new(move |block| {
        use theme::ActiveTheme;
        let colors = block.app.theme().colors();
        gpui::div()
            .debug_selector(|| "quire-embed".into())
            .ml(block.anchor_x)
            .pl_2()
            .w_full()
            .border_l_2()
            .border_color(colors.border)
            .child(nested.clone())
            .into_any_element()
    })
}

/// A header for each embed, to fold its reference into: the source it shows, or why it shows
/// nothing. Clicking the header opens the original, beside with Shift.
pub(crate) fn headers(
    editor: &Editor,
    host: WeakEntity<Editor>,
    type_tag: TypeId,
) -> Vec<(Spot, FoldPlaceholder)> {
    let Some(embeds) = editor.addon::<Embeds>() else {
        return Vec::new();
    };
    embeds
        .shown
        .iter()
        .enumerate()
        .map(|(ordinal, shown)| {
            let label = shown.label();
            let original = shown.resolved.as_ref().map(|_| shown.spot.target.clone());
            let host = host.clone();
            let placeholder = FoldPlaceholder {
                collapsed_text: Some("↳".into()),
                constrain_width: false,
                merge_adjacent: false,
                type_tag: Some(type_tag),
                render: Arc::new(move |_, _, cx| {
                    use theme::ActiveTheme;
                    let colors = cx.theme().colors();
                    let host = host.clone();
                    let original = original.clone();
                    let selector = if original.is_some() {
                        "quire-embed-header"
                    } else {
                        "quire-embed-notice"
                    };
                    crate::editor_projection::inline_control(
                        ("quire-embed-header", ordinal),
                        host.clone(),
                    )
                    .debug_selector(|| selector.into())
                    .role(gpui::Role::Link)
                    .aria_label(label.clone())
                    .cursor_pointer()
                    .text_color(colors.text_muted)
                    .child(label.clone())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        let (Some(original), Some(host)) = (original.clone(), host.upgrade())
                        else {
                            return;
                        };
                        let Some(notebook) = crate::references::host(host.read(cx))
                            .map(|host| host.read(cx).notebook().clone())
                        else {
                            return;
                        };
                        notebook.follow(
                            Follow {
                                target: original,
                                beside: event.modifiers().shift,
                                backlinks: false,
                            },
                            window,
                            cx,
                        );
                    })
                    .into_any_element()
                }),
            };
            (shown.spot.clone(), placeholder)
        })
        .collect()
}

/// The nested editors this view shows, in page order.
#[cfg(test)]
pub(crate) fn nested_editors(editor: &Editor) -> Vec<Entity<Editor>> {
    editor
        .addon::<Embeds>()
        .map(|embeds| {
            embeds
                .shown
                .iter()
                .filter_map(|shown| match &shown.content {
                    Content::Editor { editor, .. } => Some(editor.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The header of each embed in this view, in page order.
#[cfg(test)]
pub(crate) fn header_labels(editor: &Editor) -> Vec<String> {
    editor
        .addon::<Embeds>()
        .map(|embeds| {
            embeds
                .shown
                .iter()
                .map(|shown| shown.label().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// Whether this view shows a recursion notice in place of an embed.
#[cfg(test)]
pub(crate) fn shows_recursion(editor: &Editor) -> bool {
    editor.addon::<Embeds>().is_some_and(|embeds| {
        embeds
            .shown
            .iter()
            .any(|shown| matches!(shown.content, Content::Recursive))
    })
}
