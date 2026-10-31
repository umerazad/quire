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
use gpui::{
    App, Context, Entity, Focusable as _, KeyBinding, SharedString, Subscription, WeakEntity,
    Window, prelude::*,
};
use multi_buffer::{Anchor, MultiBuffer, MultiBufferRow, ToOffset, ToPoint as _};
use std::{any::TypeId, collections::HashMap, ops::Range, rc::Rc, sync::Arc};
use text::{Bias, Point};

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
    /// The view this nested editor is embedded in.
    outer: Option<WeakEntity<Editor>>,
    /// Where Down and Up leave this editor from the caret's row.
    crossing: Crossing,
    _caret: Option<Subscription>,
}

impl Addon for Embeds {
    fn extend_key_context(&self, context: &mut gpui::KeyContext, _: &App) {
        if self.crossing.down.is_some() {
            context.add("QuireEmbedDown");
        }
        if self.crossing.up.is_some() {
            context.add("QuireEmbedUp");
        }
    }

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
            outer: None,
            crossing: Crossing::default(),
            _caret: None,
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
    if state(editor)._caret.is_none() {
        let this = cx.entity();
        let caret = cx.subscribe(&this, |editor, _, event: &EditorEvent, cx| {
            if matches!(
                event,
                EditorEvent::SelectionsChanged { .. } | EditorEvent::BufferEdited
            ) {
                track(editor, cx);
            }
        });
        state(editor)._caret = Some(caret);
    }
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
    track(editor, cx);
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
    track(editor, cx);
    cx.notify();
}

/// The rows a nested editor needs, including any embeds nested in it, leaving out the empty
/// row after a page's final newline.
fn content_rows(nested: &Entity<Editor>, cx: &mut App) -> u32 {
    nested.update(cx, |nested, cx| {
        let rows = nested.max_point(cx).row().0 + 1;
        if rows > 1 && ends_with_newline(&nested.buffer().read(cx).snapshot(cx)) {
            rows - 1
        } else {
            rows
        }
    })
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
    let outer = cx.entity().downgrade();
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
        let embeds = state(&mut nested);
        embeds.chain = chain;
        embeds.outer = Some(outer);
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
        let rows = content_rows(&nested, block.app);
        let colors = block.app.theme().colors();
        gpui::div()
            .debug_selector(|| "quire-embed".into())
            .ml(block.anchor_x)
            .pl_2()
            .w_full()
            .h(block.line_height * rows as f32)
            .overflow_hidden()
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

gpui::actions!(
    quire,
    [
        /// Moves the caret down into the embed below its line, or out of an embed's last line.
        CaretDownAcrossEmbed,
        /// Moves the caret up into the embed above its line, or out of an embed's first line.
        CaretUpAcrossEmbed
    ]
);

/// Where Down or Up takes the caret when it leaves its editor.
#[derive(Clone, PartialEq)]
enum Cross {
    /// Into the nested editor of the embed beside the caret's line.
    Into(Entity<Editor>),
    /// Out of this nested editor, to the view around it.
    Out,
}

#[derive(Clone, Default, PartialEq)]
struct Crossing {
    down: Option<Cross>,
    up: Option<Cross>,
}

fn ends_with_newline(buffer: &multi_buffer::MultiBufferSnapshot) -> bool {
    buffer.reversed_chars_at(buffer.len()).next() == Some('\n')
}

/// The last row a nested editor shows, leaving out the empty row after a final newline.
fn last_row(buffer: &multi_buffer::MultiBufferSnapshot) -> u32 {
    let max = buffer.max_point().row;
    if max > 0 && ends_with_newline(buffer) {
        max - 1
    } else {
        max
    }
}

/// Where Down and Up would leave this editor, for a single caret on a line next to an embed or
/// on the first or last line of a nested editor. Wrapped lines cross only from their edge rows.
fn crossing(editor: &mut Editor, cx: &mut App) -> Crossing {
    let Some(embeds) = editor.addon::<Embeds>() else {
        return Crossing::default();
    };
    if editor.selections.count() != 1 {
        return Crossing::default();
    }
    let display = editor.display_snapshot(cx);
    let selection = editor.selections.newest::<Point>(&display);
    if !selection.is_empty() {
        return Crossing::default();
    }
    let buffer = display.buffer_snapshot();
    let caret = selection.head();
    let row_of = |point: Point| display.point_to_display_point(point, Bias::Left).row();
    let line_end = |row: u32| Point::new(row, buffer.line_len(MultiBufferRow(row)));
    let caret_row = row_of(caret);
    let nested_at = |row: u32| {
        embeds.shown.iter().find_map(|shown| match &shown.content {
            Content::Editor { editor, .. }
                if shown.spot.range.start.to_point(buffer).row == row =>
            {
                Some(Cross::Into(editor.clone()))
            }
            _ => None,
        })
    };
    let nested = embeds.outer.is_some();
    let down = (caret_row == row_of(line_end(caret.row)))
        .then(|| nested_at(caret.row))
        .flatten()
        .or_else(|| {
            (nested && caret_row >= row_of(line_end(last_row(buffer)))).then_some(Cross::Out)
        });
    let up = (caret.row > 0 && caret_row == row_of(Point::new(caret.row, 0)))
        .then(|| nested_at(caret.row - 1))
        .flatten()
        .or_else(|| (nested && caret_row == row_of(Point::zero())).then_some(Cross::Out));
    Crossing { down, up }
}

/// Keeps the key context that binds Down and Up across embeds current with the caret.
fn track(editor: &mut Editor, cx: &mut Context<Editor>) {
    let crossing = crossing(editor, cx);
    let embeds = state(editor);
    if embeds.crossing != crossing {
        embeds.crossing = crossing;
        cx.notify();
    }
}

/// Moves the caret across an embed's edge, keeping its column where the line allows.
fn cross(editor: &Entity<Editor>, down: bool, window: &mut Window, cx: &mut App) -> bool {
    let Some((cross, outer, column)) = editor.update(cx, |editor, cx| {
        let display = editor.display_snapshot(cx);
        let column = editor.selections.newest::<Point>(&display).head().column;
        let embeds = editor.addon::<Embeds>()?;
        let cross = if down {
            embeds.crossing.down.clone()
        } else {
            embeds.crossing.up.clone()
        };
        Some((cross?, embeds.outer.clone(), column))
    }) else {
        return false;
    };
    let (target, row) = match cross {
        Cross::Into(nested) => {
            let row = if down {
                0
            } else {
                last_row(&nested.read(cx).buffer().read(cx).snapshot(cx))
            };
            (nested, row)
        }
        Cross::Out => {
            let Some(outer) = outer.and_then(|outer| outer.upgrade()) else {
                return false;
            };
            let buffer = outer.read(cx).buffer().read(cx).snapshot(cx);
            let Some(spot) = outer.read(cx).addon::<Embeds>().and_then(|embeds| {
                embeds.shown.iter().find_map(|shown| match &shown.content {
                    Content::Editor { editor: nested, .. } if nested == editor => {
                        Some(shown.spot.range.start.to_point(&buffer).row)
                    }
                    _ => None,
                })
            }) else {
                return false;
            };
            let row = if down { spot + 1 } else { spot };
            if row > buffer.max_point().row {
                return false;
            }
            (outer, row)
        }
    };
    window.focus(&target.focus_handle(cx), cx);
    target.update(cx, |target, cx| {
        let buffer = target.buffer().read(cx).snapshot(cx);
        let point = buffer.clip_point(Point::new(row, column), Bias::Left);
        target.change_selections(editor::SelectionEffects::default(), window, cx, |s| {
            s.select_ranges([point..point])
        });
    });
    true
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        let register = |down: bool| {
            let handle = handle.clone();
            move |window: &mut Window, cx: &mut App| {
                if !handle
                    .upgrade()
                    .is_some_and(|editor| cross(&editor, down, window, cx))
                {
                    cx.propagate();
                }
            }
        };
        let down = register(true);
        editor
            .register_action(move |_: &CaretDownAcrossEmbed, window, cx| down(window, cx))
            .detach();
        let up = register(false);
        editor
            .register_action(move |_: &CaretUpAcrossEmbed, window, cx| up(window, cx))
            .detach();
    })
    .detach();
    const ARROWS: &str = "Editor && !QuireControl && !showing_completions && !showing_code_actions && vim_mode != operator && vim_mode != waiting";
    const VIM: &str = "Editor && !QuireControl && vim_mode == normal && !VimCount";
    cx.bind_keys([
        KeyBinding::new(
            "down",
            CaretDownAcrossEmbed,
            Some(&format!("{ARROWS} && QuireEmbedDown")),
        ),
        KeyBinding::new(
            "up",
            CaretUpAcrossEmbed,
            Some(&format!("{ARROWS} && QuireEmbedUp")),
        ),
        KeyBinding::new(
            "j",
            CaretDownAcrossEmbed,
            Some(&format!("{VIM} && QuireEmbedDown")),
        ),
        KeyBinding::new(
            "k",
            CaretUpAcrossEmbed,
            Some(&format!("{VIM} && QuireEmbedUp")),
        ),
    ]);
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
