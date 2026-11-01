//! Full embeds in Write: a block or page reference standing alone on its line shows the
//! referenced subtree or page in its place, editable there. The nested editor views the
//! referenced page's own shared buffer (a semantic excerpt for a block), so every occurrence
//! shares one buffer, one history, and that page's save stream. The reference folds away and the
//! nested editor's block, placed below the reference's line, draws its first line on that line,
//! with a small link at its right that opens the source. While the caret is on the reference's
//! line, the raw reference shows there and the embed sits below it. An embed that cannot show,
//! because it is loading, unresolved, or recursive, folds into a notice saying so. Embeds nest at
//! most eight deep.

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

/// The block showing a nested editor: `rows` of content, and whether the first sits on the
/// reference's own line.
#[derive(Clone, Copy)]
struct Placed {
    id: CustomBlockId,
    rows: u32,
    lifted: bool,
}

impl Placed {
    /// The rows the block adds below the reference's line.
    fn height(rows: u32, lifted: bool) -> u32 {
        rows - u32::from(lifted)
    }
}

struct Shown {
    spot: Spot,
    resolved: Option<Resolved>,
    content: Content,
    block: Option<Placed>,
    /// Whether the reference is folded away, so the content starts on its line.
    lifted: bool,
    _subscriptions: Vec<Subscription>,
}

impl Shown {
    /// What the embed says in place of its reference when it cannot show its content.
    fn notice(&self) -> Option<SharedString> {
        match &self.content {
            Content::Loading => Some("↳ Loading embed…".into()),
            Content::Unresolved => Some(
                format!(
                    "{} · unresolved. Check the reference in Source.",
                    self.spot.source.trim()
                )
                .into(),
            ),
            Content::Recursive => Some("↳ Recursive embed · open original ↗".into()),
            Content::Editor { .. } => None,
        }
    }

    /// The name of the link that opens a working embed's source.
    fn link(&self) -> Option<SharedString> {
        match (&self.content, &self.resolved) {
            (Content::Editor { .. }, Some(resolved)) => {
                Some(format!("Open {}", resolved.title).into())
            }
            _ => None,
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
        .filter_map(|shown| shown.block.map(|placed| placed.id))
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
        let subscriptions = match &content {
            Content::Editor { editor: nested, .. } => vec![cx.subscribe_in(
                nested,
                window,
                |editor, nested, event: &EditorEvent, window, cx| match event {
                    EditorEvent::BufferEdited => fit(editor, cx),
                    EditorEvent::SelectionsChanged { local: true } => {
                        reveal(editor, nested, window, cx)
                    }
                    _ => {}
                },
            )],
            _ => Vec::new(),
        };
        kept.push(Shown {
            spot,
            resolved,
            content,
            block: None,
            lifted: true,
            _subscriptions: subscriptions,
        });
    }
    let removed = old
        .into_iter()
        .filter_map(|shown| shown.block.map(|placed| placed.id))
        .collect();
    editor.remove_blocks(removed, None, cx);
    kept.sort_by_key(|shown| shown.spot.range.start.to_offset(&snapshot));
    state(editor).shown = kept;
    fit(editor, cx);
    track(editor, cx);
    cx.notify();
}

/// Places or sizes each nested editor's block to its content and to whether the reference is
/// folded. The block sits below the reference's line; while the reference is folded, it is one
/// row shorter and draws its first row up on that line. A block that would add no rows has no
/// height at all, since Zed grows a block with a height to fit what it draws. Zed keeps a block
/// on a row whose start is folded only when that fold ends within the row, as Write's list
/// marker does.
fn fit(editor: &mut Editor, cx: &mut Context<Editor>) {
    let outer = cx.entity().downgrade();
    let mut shown = std::mem::take(&mut state(editor).shown);
    let mut removed = Vec::new();
    let mut heights = HashMap::default();
    let mut inserted = Vec::new();
    for (index, shown) in shown.iter_mut().enumerate() {
        let Content::Editor { editor: nested, .. } = &shown.content else {
            continue;
        };
        let rows = content_rows(nested, cx);
        let (lifted, height) = (shown.lifted, Placed::height(rows, shown.lifted));
        match &mut shown.block {
            Some(placed)
                if placed.lifted == lifted
                    && (Placed::height(placed.rows, lifted) == 0) == (height == 0) =>
            {
                if placed.rows != rows {
                    placed.rows = rows;
                    heights.insert(placed.id, height);
                }
            }
            old => {
                removed.extend(old.take().map(|placed| placed.id));
                let link = shown.link().map(|label| Link {
                    target: shown.spot.target.clone(),
                    label,
                    outer: outer.clone(),
                });
                inserted.push((
                    index,
                    rows,
                    BlockProperties {
                        placement: BlockPlacement::Below(shown.spot.range.start),
                        height: (height > 0).then_some(height),
                        style: BlockStyle::Flex,
                        render: render(nested.clone(), lifted, height == 0, link),
                        priority: 0,
                    },
                ));
            }
        }
    }
    if !removed.is_empty() {
        editor.remove_blocks(removed.into_iter().collect(), None, cx);
    }
    if !heights.is_empty() {
        editor.resize_blocks(heights, None, cx);
    }
    if !inserted.is_empty() {
        let (places, properties): (Vec<_>, Vec<_>) = inserted
            .into_iter()
            .map(|(index, rows, properties)| ((index, rows), properties))
            .unzip();
        let ids = editor.insert_blocks(properties, None, cx);
        for ((index, rows), id) in places.into_iter().zip(ids) {
            shown[index].block = Some(Placed {
                id,
                rows,
                lifted: shown[index].lifted,
            });
        }
    }
    state(editor).shown = shown;
}

/// Folds or reveals each embed's reference, in page order: content starts on a folded
/// reference's line and below a revealed one.
pub(crate) fn lift(editor: &mut Editor, folded: &[bool], cx: &mut Context<Editor>) {
    let Some(embeds) = editor.addon_mut::<Embeds>() else {
        return;
    };
    let mut changed = false;
    for (shown, folded) in embeds.shown.iter_mut().zip(folded) {
        changed |= shown.lifted != *folded;
        shown.lifted = *folded;
    }
    if changed {
        fit(editor, cx);
    }
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

/// Scrolls the outermost view so the caret of `nested`, an embed in `editor`, stays in sight.
/// A nested editor sizes to its content and never scrolls, so a caret moving in a tall embed
/// would otherwise leave the screen.
fn reveal(
    editor: &mut Editor,
    nested: &Entity<Editor>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if !nested.focus_handle(cx).is_focused(window) {
        return;
    }
    let row = nested.update(cx, |nested, cx| {
        let display = nested.display_snapshot(cx);
        let head = nested.selections.newest::<Point>(&display).head();
        display.point_to_display_point(head, Bias::Left).row().0
    });
    reveal_row(editor, nested, row, window, cx);
}

/// Scrolls so `row` of the embed `nested` shows, asking the view around `editor` when
/// `editor` is itself an embed.
fn reveal_row(
    editor: &mut Editor,
    nested: &Entity<Editor>,
    row: u32,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(placed) = state(editor)
        .shown
        .iter()
        .find_map(|shown| match &shown.content {
            Content::Editor { editor, .. } if editor == nested => shown.block,
            _ => None,
        })
    else {
        return;
    };
    let display = editor.display_snapshot(cx);
    let end = editor::display_map::DisplayRow(display.max_point().row().0 + 1);
    let Some(start) = display
        .blocks_in_range(editor::display_map::DisplayRow(0)..end)
        .find_map(|(start, shown)| match shown {
            editor::display_map::Block::Custom(custom) if custom.id == placed.id => Some(start),
            _ => None,
        })
    else {
        return;
    };
    let row = (start.0 + row).saturating_sub(u32::from(placed.lifted));
    if let Some(outer) = state(editor)
        .outer
        .clone()
        .and_then(|outer| outer.upgrade())
    {
        let this = cx.entity();
        outer.update(cx, |outer, cx| reveal_row(outer, &this, row, window, cx));
        return;
    }
    let Some(visible) = editor.visible_line_count() else {
        return;
    };
    let mut position = editor.scroll_position(cx);
    let row = f64::from(row);
    position.y = if row < position.y {
        row
    } else if row + 1. > position.y + visible {
        row + 1. - visible
    } else {
        return;
    };
    editor.set_scroll_position(position, window, cx);
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
        // An embed reads as part of the page: no line numbers, excerpt arrows, or scrollbar.
        nested.set_show_gutter(false, cx);
        nested.disable_scrollbars_and_minimap(window, cx);
        crate::page_menu::install(&mut nested);
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
    crate::spellcheck::attach(&nested, &analysis, cx);
    Some(Content::Editor {
        editor: nested,
        _model: model,
    })
}

/// The link at the right of an embed's first line, which opens its source.
#[derive(Clone)]
struct Link {
    target: Target,
    label: SharedString,
    /// The view the embed is in, which follows the link.
    outer: WeakEntity<Editor>,
}

/// Opens an embed's source from the view showing it, beside with Shift.
fn follow(
    outer: &WeakEntity<Editor>,
    target: Target,
    beside: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(notebook) = outer
        .upgrade()
        .and_then(|outer| crate::references::host(outer.read(cx)))
        .map(|host| host.read(cx).notebook().clone())
    else {
        return;
    };
    notebook.follow(
        Follow {
            target,
            beside,
            backlinks: false,
        },
        window,
        cx,
    );
}

/// Draws a nested editor in its block, `lifted` one row up onto the reference's folded line.
/// A block without height (`empty`) still draws the one row it lifts.
fn render(
    nested: Entity<Editor>,
    lifted: bool,
    empty: bool,
    link: Option<Link>,
) -> editor::display_map::RenderBlock {
    Arc::new(move |block| {
        use theme::ActiveTheme;
        let added = if empty { 0 } else { block.height };
        let rows = added + u32::from(lifted);
        let line = block.line_height;
        let colors = block.app.theme().colors();
        let link = link.clone().map(|link| {
            let Link {
                target,
                label,
                outer,
            } = link;
            gpui::div()
                .id(("quire-embed-link", nested.entity_id()))
                .debug_selector(|| "quire-embed-link".into())
                .absolute()
                .top_0()
                .right_0()
                .h(line)
                .px_1()
                .flex()
                .items_center()
                .role(gpui::Role::Link)
                .aria_label(label.clone())
                .cursor_pointer()
                .tooltip(ui::Tooltip::text(label))
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    follow(&outer, target.clone(), event.modifiers().shift, window, cx);
                })
                .child(
                    ui::Icon::new(ui::IconName::ArrowUpRight)
                        .size(ui::IconSize::XSmall)
                        .color(ui::Color::Muted),
                )
        });
        let width = (block.margins.gutter.full_width() + block.max_width
            - block.margins.right
            - block.anchor_x)
            .max(gpui::px(0.));
        gpui::div()
            .relative()
            .w_full()
            .h(line * added as f32)
            .child(
                gpui::div()
                    .debug_selector(|| "quire-embed".into())
                    .absolute()
                    .top(if lifted { -line } else { gpui::px(0.) })
                    .left(block.anchor_x)
                    .w(width)
                    .h(line * rows as f32)
                    .pl_2()
                    .pr_6()
                    .overflow_hidden()
                    .border_l_2()
                    .border_color(colors.border)
                    .child(nested.clone())
                    .children(link),
            )
            .into_any_element()
    })
}

/// What each embed's reference folds into while the caret is away: nothing for an embed that
/// shows its content, or a notice saying why it shows none. A notice for a resolved embed opens
/// the original, beside with Shift.
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
            let Some(notice) = shown.notice() else {
                // Draws nothing, but keeps text: Zed's block map loses count of rows when a
                // row with blocks below it folds to no text at all, and panics when they go.
                let placeholder = FoldPlaceholder {
                    collapsed_text: Some("↳".into()),
                    constrain_width: false,
                    merge_adjacent: false,
                    type_tag: Some(type_tag),
                    ..Default::default()
                };
                return (shown.spot.clone(), placeholder);
            };
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
                    crate::editor_projection::inline_control(
                        ("quire-embed-notice", ordinal),
                        host.clone(),
                    )
                    .debug_selector(|| "quire-embed-notice".into())
                    .role(gpui::Role::Link)
                    .aria_label(notice.clone())
                    .cursor_pointer()
                    .text_color(colors.text_muted)
                    .child(notice.clone())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        if let Some(original) = original.clone() {
                            follow(&host, original, event.modifiers().shift, window, cx);
                        }
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

/// Whether this editor is embedded in another view.
pub(crate) fn is_nested(editor: &Editor) -> bool {
    editor
        .addon::<Embeds>()
        .is_some_and(|embeds| embeds.outer.is_some())
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

/// What each embed in this view names, in page order: its link to the source when it shows its
/// content, or its notice.
#[cfg(test)]
pub(crate) fn labels(editor: &Editor) -> Vec<String> {
    editor
        .addon::<Embeds>()
        .map(|embeds| {
            embeds
                .shown
                .iter()
                .map(|shown| {
                    shown
                        .link()
                        .or_else(|| shown.notice())
                        .unwrap_or_default()
                        .to_string()
                })
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
