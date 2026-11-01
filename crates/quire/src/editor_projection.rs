use editor::{Addon, Editor, EditorEvent, FoldPlaceholder, display_map::Crease};
use gpui::{App, Context, Entity, Focusable, Subscription, WeakEntity, Window, prelude::*};
use multi_buffer::{
    Anchor, MultiBufferOffset, MultiBufferRow, MultiBufferSnapshot, ToOffset, ViewEdit,
    ViewEditIntent,
};
use std::{
    any::{Any, TypeId},
    collections::HashMap,
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

pub struct BlockSyntax {
    pub marker: Range<MultiBufferOffset>,
    pub id: Option<Range<MultiBufferOffset>>,
    pub children: Option<MultiBufferOffset>,
    /// Where a list item's children end, when it has any and they lie in the view.
    pub subtree_end: Option<MultiBufferOffset>,
    pub depth: usize,
}

struct AnchoredBlock {
    marker: Range<Anchor>,
    id: Option<Range<Anchor>>,
    /// Where a list item's children start, when it has any: a collapsed fold starts there.
    children: Option<Anchor>,
    subtree_end: Option<Anchor>,
    depth: usize,
}

struct Projection {
    blocks: Arc<[AnchoredBlock]>,
    links: Vec<AnchoredLink>,
    previews: Vec<AnchoredPreview>,
    /// Each previewed block's text by reference target; `None` when the target is unresolved.
    preview_text: HashMap<String, Option<String>>,
    /// The latest request for each target, so an older answer never replaces a newer one.
    requests: HashMap<String, u64>,
    sequence: u64,
    parsed: Option<(text::BufferSnapshot, usize)>,
    heads: Vec<Anchor>,
    /// How many folds hide or replace syntax after the last refresh, to notice when a fold
    /// command outside Write removed some.
    syntax_folds: usize,
    /// Inline marks, styles, rules, and tables from the latest parse.
    style: crate::write_style::Presentation,
    /// Whether the view's highlights show `style`; a parse or theme change clears it.
    styled: bool,
    hover: Arc<BulletHover>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct AnchoredPreview {
    range: Range<Anchor>,
    block: Range<Anchor>,
    target: String,
    source: String,
}

#[derive(Clone)]
struct AnchoredLink {
    range: Range<Anchor>,
    label: Range<Anchor>,
    block: Range<Anchor>,
    id: u64,
    data: Arc<crate::markdown_links::Link>,
}

impl AnchoredLink {
    fn url(&self) -> Option<&str> {
        match &self.data.destination {
            crate::markdown_links::Destination::Web(url) => Some(url),
            crate::markdown_links::Destination::Reference(_) => None,
        }
    }

    fn is_external(&self) -> bool {
        self.url().is_some()
    }
}

/// What hovering a link that leaves the notebook says, as `rt` titles its external links.
fn external_tooltip(url: &str) -> String {
    format!("Opens {url} outside the notebook")
}

struct ExternalLink;

fn link_key<T: 'static>() -> editor::HighlightKey {
    editor::HighlightKey::NavigationOverlay(editor::NavigationOverlayKey::unique::<T>())
}

/// Links to notebook pages and blocks: the theme's link colour, underlined.
fn internal_link_style(cx: &App) -> gpui::HighlightStyle {
    use theme::ActiveTheme;
    gpui::HighlightStyle {
        color: Some(cx.theme().colors().link_text_hover),
        underline: Some(gpui::UnderlineStyle {
            thickness: gpui::px(1.),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// `rt`'s blues for links that leave the notebook (`42de3c8949e1`): the text, its underline, and
/// the trailing arrow, in the light or dark theme.
#[derive(Clone, Copy)]
struct ExternalPalette {
    ink: u32,
    line: u32,
    arrow: u32,
}

impl ExternalPalette {
    fn of(cx: &App) -> Self {
        use theme::ActiveTheme;
        if cx.theme().appearance.is_light() {
            Self {
                ink: 0x2f6690,
                line: 0x8fb0cb,
                arrow: 0x6f93b3,
            }
        } else {
            Self {
                ink: 0x81b9e8,
                line: 0x6c8ba5,
                arrow: 0x7599b9,
            }
        }
    }

    /// GPUI underlines are solid or wavy, so the lighter underline colour stands in for `rt`'s
    /// dotted one.
    fn style(self) -> gpui::HighlightStyle {
        gpui::HighlightStyle {
            color: Some(gpui::rgb(self.ink).into()),
            underline: Some(gpui::UnderlineStyle {
                thickness: gpui::px(1.),
                color: Some(gpui::rgb(self.line).into()),
                wavy: false,
            }),
            ..Default::default()
        }
    }
}

impl Addon for Projection {
    fn extend_key_context(&self, context: &mut gpui::KeyContext, _: &App) {
        context.add("QuireWrite");
    }

    fn text_regions(
        &self,
        editor: &Entity<Editor>,
        visible: Range<MultiBufferOffset>,
        window: &Window,
        cx: &App,
    ) -> Vec<editor::TextRegion> {
        let view = editor.read(cx);
        let snapshot = view.buffer().read(cx).snapshot(cx);
        let start = self
            .links
            .partition_point(|link| link.range.end.to_offset(&snapshot) < visible.start);
        let links = self.links[start..]
            .iter()
            .take_while(|link| link.range.start.to_offset(&snapshot) <= visible.end)
            .filter(|link| rendered_link(view, link, &snapshot, window, cx))
            .map(|link| {
                let link = link.clone();
                let editor = editor.downgrade();
                editor::TextRegion {
                    range: link.label.clone(),
                    render: Arc::new(move |layout, _| link_region(&editor, &link, layout)),
                }
            });
        let lines = self
            .blocks
            .iter()
            .enumerate()
            .filter_map(|(ordinal, block)| {
                let end = block.children?;
                let range = block.marker.end..end;
                let offsets = range.start.to_offset(&snapshot)..range.end.to_offset(&snapshot);
                (offsets.start < offsets.end
                    && offsets.end >= visible.start
                    && offsets.start <= visible.end)
                    .then(|| {
                        let hover = self.hover.clone();
                        let editor = editor.downgrade();
                        editor::TextRegion {
                            range,
                            render: Arc::new(move |_, _| {
                                line_hover(ordinal, hover.clone(), editor.clone())
                            }),
                        }
                    })
            });
        links.chain(lines).collect()
    }

    /// `rt`'s child guides (`block-hierarchy.ts`) in place of Zed's indent guides: for each list
    /// item with children, a 2 px line under its bullet from just below the dot through its
    /// subtree's last line, in a tone per depth.
    fn indent_guides(
        &self,
        editor: &Entity<Editor>,
        visible: Range<MultiBufferRow>,
        cx: &App,
    ) -> Option<Vec<editor::HostIndentGuide>> {
        let snapshot = editor.read(cx).buffer().read(cx).snapshot(cx);
        let tones = guide_tones(cx);
        Some(
            self.blocks
                .iter()
                .enumerate()
                .filter_map(|(ordinal, block)| {
                    block.children?;
                    let start = snapshot.offset_to_point(block.marker.start.to_offset(&snapshot));
                    let end = snapshot.offset_to_point(block.subtree_end?.to_offset(&snapshot));
                    if end.row < visible.start.0 || start.row > visible.end.0 {
                        return None;
                    }
                    let column = snapshot
                        .chars_at(snapshot.point_to_offset(language::Point::new(start.row, 0)))
                        .take(start.column as usize)
                        .fold(0, |column, c| match c {
                            '\t' => column + GUIDE_TAB - column % GUIDE_TAB,
                            _ => column + 1,
                        });
                    Some(editor::HostIndentGuide {
                        rows: MultiBufferRow(start.row)..=MultiBufferRow(end.row),
                        column,
                        offset: BULLET_CENTER - GUIDE_WIDTH / 2.,
                        start_below_center: GUIDE_GAP,
                        width: GUIDE_WIDTH,
                        color: tones[block.depth % tones.len()],
                        debug_selector: format!("quire-guide-{ordinal}"),
                    })
                })
                .collect(),
        )
    }

    fn to_any(&self) -> &dyn Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

/// Tracks the pointer over a parent item's line text, which shows its fold arrow, without taking
/// clicks or selection from the text.
fn line_hover(
    ordinal: usize,
    hover: Arc<BulletHover>,
    editor: WeakEntity<Editor>,
) -> gpui::AnyElement {
    gpui::div()
        .id(("quire-bullet-line", ordinal))
        .debug_selector(move || format!("quire-line-{ordinal}"))
        .size_full()
        .on_hover(move |hovered, _, cx| hover.set(false, ordinal, *hovered, &editor, cx))
        .into_any_element()
}

/// Where a bullet's dot centre sits right of its marker's start: past the 16 px fold-arrow slot,
/// half the 18 px bullet.
const BULLET_CENTER: gpui::Pixels = gpui::px(25.);
const GUIDE_WIDTH: gpui::Pixels = gpui::px(2.);
/// The guide starts just below its parent's dot, as `rt`'s starts 1.1em into the line.
const GUIDE_GAP: gpui::Pixels = gpui::px(5.);
/// `rt`'s tab width when it measures a parent's indentation.
const GUIDE_TAB: u32 = 4;

/// `rt`'s four guide tones by depth (`--branch-guide-0` to `-3` in `controls.css`), which its
/// light and dark themes each set to one neutral colour.
fn guide_tones(cx: &App) -> [gpui::Hsla; 4] {
    use theme::ActiveTheme;
    let tone = if cx.theme().appearance.is_light() {
        0xd8e0e6
    } else {
        0x2b2d2f
    };
    [gpui::rgb(tone).into(); 4]
}

fn rendered_link(
    editor: &Editor,
    link: &AnchoredLink,
    snapshot: &MultiBufferSnapshot,
    window: &Window,
    cx: &App,
) -> bool {
    shown(
        editor,
        &link.range,
        &link.block,
        &link.data.source,
        snapshot,
        window,
        cx,
    )
}

/// Whether rendered syntax replaces `source` at `range`: the text is unchanged and the caret of
/// a focused editor is outside its block, where the source shows for editing.
fn shown(
    editor: &Editor,
    range: &Range<Anchor>,
    block: &Range<Anchor>,
    source: &str,
    snapshot: &MultiBufferSnapshot,
    window: &Window,
    cx: &App,
) -> bool {
    if editor.focus_handle(cx).is_focused(window) {
        let head = editor
            .selections
            .disjoint_anchors_arc()
            .last()
            .map(|selection| selection.head().to_offset(snapshot));
        if head.is_some_and(|head| {
            block.start.to_offset(snapshot) <= head && head <= block.end.to_offset(snapshot)
        }) {
            return false;
        }
    }
    let start = range.start.to_offset(snapshot);
    let end = range.end.to_offset(snapshot);
    end.0.saturating_sub(start.0) == source.len() && snapshot.contains_str_at(start, source)
}

fn text_start(block: &AnchoredBlock, snapshot: &MultiBufferSnapshot) -> MultiBufferOffset {
    let start = block.marker.end.to_offset(snapshot);
    if block.marker.start.to_offset(snapshot) == start
        || !matches!(snapshot.chars_at(start).next(), Some('[' | '{'))
    {
        return start;
    }
    let prefix: String = snapshot
        .chars_at(start)
        .take(crate::task_marker::PREFIX_CHAR_LIMIT)
        .take_while(|c| !matches!(c, '\n' | '\r'))
        .collect();
    let Some(task) = crate::task_marker::read(&prefix) else {
        return start;
    };
    let marker_end = MultiBufferOffset(start.0 + task.source.len());
    let gap = snapshot
        .chars_at(marker_end)
        .take_while(|c| matches!(c, ' ' | '\t'))
        .count();
    MultiBufferOffset(marker_end.0 + gap)
}

/// What Write hides after a block's text: its ID, and when it is collapsed, the ID through its
/// children.
fn hidden_tails(
    block: &AnchoredBlock,
    collapsed: &[Range<MultiBufferOffset>],
    snapshot: &MultiBufferSnapshot,
) -> Vec<Range<MultiBufferOffset>> {
    let id = id_range(block, snapshot);
    let children = block
        .children
        .map(|at| at.to_offset(snapshot))
        .and_then(|children| {
            let fold = collapsed.iter().find(|fold| fold.start == children)?;
            Some(id.as_ref().map_or(children, |id| id.start)..fold.end)
        });
    children.into_iter().chain(id).collect()
}

/// A block's hidden ID with the one space that separates it from the text. Spaces before that
/// one stay visible, so a space typed at the end of the text shows and keeps the caret after it.
fn id_range(
    block: &AnchoredBlock,
    snapshot: &MultiBufferSnapshot,
) -> Option<Range<MultiBufferOffset>> {
    let id = block.id.as_ref()?;
    let mut start = id.start.to_offset(snapshot);
    let end = id.end.to_offset(snapshot);
    let text: String = snapshot.text_for_range(start..end).collect();
    let tail = text.trim_end_matches([' ', '\t']);
    let caret = tail.rfind('^')?;
    if caret > 0 && !matches!(tail.as_bytes()[caret - 1], b' ' | b'\t') {
        return None;
    }
    let id = &tail[caret + 1..];
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return None;
    }
    start.0 += caret;
    let content = text_start(block, snapshot);
    if start > content && matches!(snapshot.reversed_chars_at(start).next(), Some(' ' | '\t')) {
        start.0 -= 1;
    }
    Some(start..end)
}

fn refresh(
    editor: &mut Editor,
    blocks: &[AnchoredBlock],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let collapsed: Vec<MultiBufferOffset> = crate::outline::folds::collapsed(editor, cx)
        .into_iter()
        .map(|range| range.start)
        .collect();
    let palette = BulletPalette::of(cx);
    let hover = editor
        .addon::<Projection>()
        .map(|projection| projection.hover.clone())
        .unwrap_or_default();
    let mut hidden = Vec::new();
    let mut tasks = Vec::new();
    for (ordinal, block) in blocks.iter().enumerate() {
        let marker = block.marker.start.to_offset(&snapshot)..block.marker.end.to_offset(&snapshot);
        if !marker.is_empty() {
            let content: String = snapshot
                .chars_at(marker.end)
                .take_while(|c| !matches!(c, '\n' | '\r'))
                .take(crate::task_marker::PREFIX_CHAR_LIMIT)
                .collect();
            if let Some(task) = crate::task_marker::read(&content) {
                let range = marker.end..MultiBufferOffset(marker.end.0 + task.source.len());
                tasks.push(Crease::simple(
                    range,
                    checkbox_placeholder(
                        cx.entity().downgrade(),
                        ordinal,
                        task.source.into(),
                        task.checked,
                    ),
                ));
            }
            let label: String = snapshot.text_for_range(marker.clone()).collect();
            let number = label
                .trim()
                .starts_with(|c: char| c.is_ascii_digit())
                .then(|| label.trim().to_owned().into());
            let collapsed = block
                .children
                .is_some_and(|at| collapsed.contains(&at.to_offset(&snapshot)));
            tasks.push(Crease::simple(
                marker,
                bullet_placeholder(
                    cx.entity().downgrade(),
                    Bullet {
                        ordinal,
                        number,
                        children: block.children,
                        collapsed,
                        palette,
                        hover: hover.clone(),
                    },
                ),
            ));
        }
        if let Some(id) = id_range(block, &snapshot) {
            hidden.push(id);
        }
    }
    editor.remove_folds_with_type(
        &[MultiBufferOffset(0)..snapshot.len()],
        TypeId::of::<Projection>(),
        false,
        cx,
    );
    let placeholder = FoldPlaceholder {
        collapsed_text: Some("".into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        ..Default::default()
    };
    let style = editor
        .addon::<Projection>()
        .map(|projection| projection.style.clone())
        .unwrap_or_default();
    let mut replaced = Vec::new();
    for mark in style.hidden.iter() {
        if shown(
            editor,
            &mark.range,
            &mark.block,
            &mark.source,
            &snapshot,
            window,
            cx,
        ) {
            hidden.push(mark.range.start.to_offset(&snapshot)..mark.range.end.to_offset(&snapshot));
        }
    }
    for rule in style.rules.iter() {
        if shown(
            editor,
            &rule.range,
            &rule.block,
            &rule.source,
            &snapshot,
            window,
            cx,
        ) {
            replaced.push(crate::write_style::Replacement::Rule(rule.clone()));
        }
    }
    for table in style.tables.iter() {
        let mark = &table.mark;
        if shown(
            editor,
            &mark.range,
            &mark.block,
            &mark.source,
            &snapshot,
            window,
            cx,
        ) {
            replaced.push(crate::write_style::Replacement::Table(table.clone()));
        }
    }
    crate::write_style::replace(editor, replaced, cx);
    let restyle = editor
        .addon_mut::<Projection>()
        .is_none_or(|projection| !std::mem::replace(&mut projection.styled, true));
    if restyle {
        crate::write_style::highlight(editor, &style, cx);
    }
    let (links, previews) = editor
        .addon::<Projection>()
        .map(|projection| {
            let previews: Vec<_> = projection
                .previews
                .iter()
                .filter_map(|preview| {
                    let text = projection.preview_text.get(&preview.target)?;
                    Some((preview.clone(), text.clone()))
                })
                .collect();
            (projection.links.clone(), previews)
        })
        .unwrap_or_default();
    for (ordinal, (preview, text)) in previews.into_iter().enumerate() {
        if shown(
            editor,
            &preview.range,
            &preview.block,
            &preview.source,
            &snapshot,
            window,
            cx,
        ) {
            let range =
                preview.range.start.to_offset(&snapshot)..preview.range.end.to_offset(&snapshot);
            tasks.push(Crease::simple(
                range,
                preview_placeholder(cx.entity().downgrade(), ordinal, preview, text),
            ));
        }
    }
    let type_tag = TypeId::of::<Projection>();
    for (spot, placeholder) in crate::task_lists::headers(editor, cx.entity().downgrade(), type_tag)
    {
        if shown(
            editor,
            &spot.range,
            &spot.block,
            &spot.source,
            &snapshot,
            window,
            cx,
        ) {
            let range = spot.range.start.to_offset(&snapshot)..spot.range.end.to_offset(&snapshot);
            tasks.push(Crease::simple(range, placeholder));
        }
    }
    for (spot, placeholder) in crate::media::headers(editor, cx.entity().downgrade(), type_tag) {
        if shown(
            editor,
            &spot.range,
            &spot.block,
            &spot.source,
            &snapshot,
            window,
            cx,
        ) {
            let range = spot.range.start.to_offset(&snapshot)..spot.range.end.to_offset(&snapshot);
            tasks.push(Crease::simple(range, placeholder));
        }
    }
    let toolbars =
        crate::code_format::toolbar::headers(editor, cx.entity().downgrade(), type_tag, cx);
    for (spot, placeholder) in toolbars {
        if shown(
            editor,
            &spot.range,
            &spot.block,
            &spot.source,
            &snapshot,
            window,
            cx,
        ) {
            let range = spot.range.start.to_offset(&snapshot)..spot.range.end.to_offset(&snapshot);
            tasks.push(Crease::simple(range, placeholder));
        }
    }
    let embeds = crate::embeds::headers(editor, cx.entity().downgrade(), type_tag);
    let folded: Vec<bool> = embeds
        .iter()
        .map(|(spot, _)| {
            shown(
                editor,
                &spot.range,
                &spot.block,
                &spot.source,
                &snapshot,
                window,
                cx,
            )
        })
        .collect();
    for ((spot, placeholder), folded) in embeds.into_iter().zip(folded.iter().copied()) {
        if folded {
            let range = spot.range.start.to_offset(&snapshot)..spot.range.end.to_offset(&snapshot);
            tasks.push(Crease::simple(range, placeholder));
        }
    }
    let palette = ExternalPalette::of(cx);
    let (external, internal): (Vec<_>, Vec<_>) = links.iter().partition(|link| link.is_external());
    for link in links.iter() {
        if !rendered_link(editor, link, &snapshot, window, cx) {
            continue;
        }
        let start = link.range.start.to_offset(&snapshot);
        let label_start = link.label.start.to_offset(&snapshot);
        let label_end = link.label.end.to_offset(&snapshot);
        let end = link.range.end.to_offset(&snapshot);
        if !link.is_external() {
            hidden.push(start..label_start);
            hidden.push(label_end..end);
            continue;
        }
        if start < label_start {
            hidden.push(start..label_start);
        }
        // A bare URL has no syntax after it to carry the arrow, so its last character does.
        let kept = (label_end == end)
            .then(|| snapshot.reversed_chars_at(end).next())
            .flatten();
        let trailing = match kept {
            Some(last) => MultiBufferOffset(end.0 - last.len_utf8())..end,
            None => label_end..end,
        };
        tasks.push(Crease::simple(
            trailing,
            arrow_placeholder(cx.entity().downgrade(), link.clone(), kept, palette),
        ));
    }
    for (key, links, style) in [
        (link_key::<Projection>(), internal, internal_link_style(cx)),
        (link_key::<ExternalLink>(), external, palette.style()),
    ] {
        editor.clear_highlights(key, cx);
        if !links.is_empty() {
            let labels = links.into_iter().map(|link| link.label.clone()).collect();
            editor.highlight_text(key, labels, style, cx);
        }
    }
    editor.fold_creases(
        hidden
            .into_iter()
            .map(|range| Crease::simple(range, placeholder.clone()))
            .chain(tasks)
            .collect(),
        false,
        window,
        cx,
    );
    crate::embeds::lift(editor, &folded, cx);
    let count = syntax_fold_count(editor, cx);
    if let Some(projection) = editor.addon_mut::<Projection>() {
        projection.syntax_folds = count;
    }
}

fn syntax_fold_count(editor: &Editor, cx: &mut App) -> usize {
    let snapshot = editor.display_snapshot(cx);
    let buffer = snapshot.buffer_snapshot();
    snapshot
        .folds_in_range(MultiBufferOffset(0)..buffer.len())
        .filter(|fold| fold.placeholder.type_tag == Some(TypeId::of::<Projection>()))
        .count()
}

/// Whether a fold command that does not know Write, such as Zed's Unfold, revealed syntax that
/// Write hides.
pub(crate) fn syntax_unfolded(editor: &Editor, cx: &mut App) -> bool {
    editor
        .addon::<Projection>()
        .is_some_and(|projection| syntax_fold_count(editor, cx) < projection.syntax_folds)
}

/// `rt`'s bullet ink and the ring a collapsed bullet wears (`src/ui/controls.css`), in the light or
/// dark theme.
#[derive(Clone, Copy)]
struct BulletPalette {
    ink: u32,
    ring: u32,
}

impl BulletPalette {
    fn of(cx: &App) -> Self {
        use theme::ActiveTheme;
        if cx.theme().appearance.is_light() {
            Self {
                ink: 0x5c7080,
                ring: 0xdbe3ea,
            }
        } else {
            Self {
                ink: 0x98adbf,
                ring: 0x282a2c,
            }
        }
    }
}

/// Which list item the pointer is over, by block ordinal: its line's text, or its drawn marker.
/// Each is tracked apart, so leaving one for the other never hides the item's fold arrow.
pub(crate) struct BulletHover {
    line: AtomicUsize,
    marker: AtomicUsize,
}

const NOT_HOVERED: usize = usize::MAX;

impl Default for BulletHover {
    fn default() -> Self {
        Self {
            line: AtomicUsize::new(NOT_HOVERED),
            marker: AtomicUsize::new(NOT_HOVERED),
        }
    }
}

impl BulletHover {
    fn shows(&self, ordinal: usize) -> bool {
        [&self.line, &self.marker]
            .iter()
            .any(|over| over.load(Ordering::Relaxed) == ordinal)
    }

    /// Records the pointer entering or leaving `ordinal`'s line or marker, redrawing the editor
    /// when that changes whether its arrow shows.
    fn set(
        &self,
        marker: bool,
        ordinal: usize,
        hovered: bool,
        editor: &WeakEntity<Editor>,
        cx: &mut App,
    ) {
        let over = if marker { &self.marker } else { &self.line };
        let changed = if hovered {
            over.swap(ordinal, Ordering::Relaxed) != ordinal
        } else {
            over.compare_exchange(ordinal, NOT_HOVERED, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        };
        if changed {
            editor.update(cx, |_, cx| cx.notify()).ok();
        }
    }
}

struct Bullet {
    ordinal: usize,
    /// A numbered item's marker, such as `2.`, shown in place of the dot.
    number: Option<gpui::SharedString>,
    /// Where the item's children start, when it has any.
    children: Option<Anchor>,
    collapsed: bool,
    palette: BulletPalette,
    hover: Arc<BulletHover>,
}

/// A list item's marker as `rt` draws it: a dot, or the item's number, that zooms into the
/// block on click, with a ring when its children are collapsed. An item with children also gets
/// an arrow at its left that collapses or expands them, shown while they are collapsed or the
/// pointer is over the item's line, as `rt` shows it on hover. The placeholder text is empty, so
/// the view's text and columns stay those of the hidden marker.
fn bullet_placeholder(editor: WeakEntity<Editor>, bullet: Bullet) -> FoldPlaceholder {
    let bullet = Arc::new(bullet);
    FoldPlaceholder {
        collapsed_text: Some("".into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        render: Arc::new(move |_, range, _| {
            use gpui::{InteractiveElement as _, ParentElement as _, Styled as _, px, rgb};
            let Bullet {
                ordinal,
                number,
                children,
                collapsed,
                palette,
                hover,
            } = &*bullet;
            let (ink, ring) = (rgb(palette.ink), rgb(palette.ring));
            let group = gpui::SharedString::from(format!("quire-bullet-{ordinal}"));
            let mark = match number {
                Some(number) => gpui::div()
                    .px(px(3.))
                    .rounded(px(4.))
                    .text_color(ink)
                    .when(*collapsed, |mark| mark.bg(ring))
                    .group_hover(group.clone(), |style| style.bg(ring))
                    .child(number.clone()),
                None => gpui::div()
                    .size(px(14.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when(*collapsed, |mark| mark.bg(ring))
                    .group_hover(group.clone(), |style| style.bg(ring))
                    .child(gpui::div().size(px(6.)).rounded_full().bg(ink)),
            };
            let zoom_editor = editor.clone();
            let text_start = range.end;
            let bullet = pointer_control(("quire-bullet", *ordinal))
                .debug_selector(|| "quire-bullet".into())
                .group(group)
                .flex()
                .items_center()
                .justify_center()
                .min_w(px(18.))
                .h_full()
                .cursor_pointer()
                .role(gpui::Role::Button)
                .aria_label("Zoom into this block")
                .child(mark)
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    zoom_editor
                        .update(cx, |editor, cx| {
                            crate::outline::zoom::zoom_from_bullet(editor, text_start, window, cx)
                        })
                        .ok();
                });
            let mut slot = gpui::div().flex_none().w(px(16.));
            if let Some(children) = children.filter(|_| *collapsed || hover.shows(*ordinal)) {
                let fold_editor = editor.clone();
                slot = slot.child(
                    pointer_control(("quire-bullet-fold", *ordinal))
                        .debug_selector(|| "quire-bullet-fold".into())
                        .flex()
                        .items_center()
                        .justify_center()
                        .size(px(16.))
                        .rounded(px(3.))
                        .hover(|style| style.bg(ring))
                        .role(gpui::Role::Button)
                        .aria_label(if *collapsed {
                            "Expand block"
                        } else {
                            "Collapse block"
                        })
                        .aria_expanded(!*collapsed)
                        .child(
                            ui::Icon::new(if *collapsed {
                                ui::IconName::ChevronRight
                            } else {
                                ui::IconName::ChevronDown
                            })
                            .size(ui::IconSize::XSmall)
                            .color(ui::Color::Muted),
                        )
                        .on_click(move |_, window, cx| {
                            cx.stop_propagation();
                            fold_editor
                                .update(cx, |editor, cx| {
                                    crate::outline::folds::toggle_at(editor, children, window, cx)
                                })
                                .ok();
                        }),
                );
            }
            let hover = hover.clone();
            let hover_editor = editor.clone();
            let ordinal = *ordinal;
            gpui::div()
                .id(("quire-bullet-marker", ordinal))
                .flex()
                .items_center()
                .h_full()
                .mr(px(2.))
                .on_hover(move |hovered, _, cx| {
                    hover.set(true, ordinal, *hovered, &hover_editor, cx)
                })
                .child(slot)
                .child(bullet)
                .into_any_element()
        }),
    }
}

fn projection_is_current(editor: &Editor, cx: &App) -> bool {
    let Some(projection) = editor.addon::<Projection>() else {
        return false;
    };
    let Some(source) = editor.buffer().read(cx).all_buffers_iter().next() else {
        return false;
    };
    let source = source.read(cx);
    let snapshot = source.snapshot();
    let idle = *source.parse_status().borrow() == language::ParseStatus::Idle;
    idle && projection.parsed.as_ref().is_some_and(|(text, count)| {
        text.version() == snapshot.version() && *count == snapshot.syntax_update_count()
    })
}

/// Opens a rendered link if the page still reads as when it was drawn: a web link in the browser,
/// a reference through the notebook (beside the page with Shift).
fn open_link(
    editor: &Editor,
    range: Range<Anchor>,
    expected: &str,
    beside: bool,
    window: &mut Window,
    cx: &mut App,
) {
    if !projection_is_current(editor, cx) {
        return;
    }
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let offsets = range.start.to_offset(&snapshot)..range.end.to_offset(&snapshot);
    let Some(link) = editor
        .addon::<Projection>()
        .unwrap()
        .links
        .iter()
        .find(|link| {
            link.range.start.to_offset(&snapshot) == offsets.start
                && link.range.end.to_offset(&snapshot) == offsets.end
                && link.data.source == expected
        })
    else {
        return;
    };
    let current: String = snapshot.text_for_range(offsets).collect();
    if current != expected {
        return;
    }
    match &link.data.destination {
        crate::markdown_links::Destination::Web(url) => cx.open_url(url),
        crate::markdown_links::Destination::Reference(target) => {
            follow(editor, target.clone(), beside, window, cx)
        }
    }
}

fn focusable_control(
    id: impl Into<gpui::ElementId>,
    editor: WeakEntity<Editor>,
) -> gpui::Stateful<gpui::Div> {
    gpui::div()
        .id(id)
        .key_context("QuireControl")
        .tab_index(0_isize)
        .tab_stop(true)
        .on_key_down(move |event, window, cx| {
            if event.keystroke.key == "escape" {
                editor
                    .update(cx, |editor, cx| window.focus(&editor.focus_handle(cx), cx))
                    .ok();
            }
            if !matches!(event.keystroke.key.as_str(), "enter" | "space" | "tab") {
                window.prevent_default();
                cx.stop_propagation();
            }
        })
}

/// A clickable fold placeholder that is no tab stop, for controls whose commands have keys, such as
/// zooming and folding from a bullet. The same ID rule as [`inline_control`] applies.
fn pointer_control(id: impl Into<gpui::ElementId>) -> gpui::Stateful<gpui::Div> {
    gpui::div()
        .id(id)
        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
}

/// A clickable fold placeholder. `id` must outlive a refold, which mints new fold IDs: GPUI keeps
/// a pressed element's click in state keyed by its ID, so a fold ID would drop a click whose
/// release lands after an index reply refolds the view.
pub(crate) fn inline_control(
    id: impl Into<gpui::ElementId>,
    editor: WeakEntity<Editor>,
) -> gpui::Stateful<gpui::Div> {
    focusable_control(id, editor).on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
        window.prevent_default();
        cx.stop_propagation();
    })
}

fn link_region(
    editor: &WeakEntity<Editor>,
    link: &AnchoredLink,
    layout: &editor::TextRegionLayout,
) -> gpui::AnyElement {
    gpui::div()
        .id(("quire-web-link", link.id))
        .size_full()
        .debug_selector(|| "quire-web-link".into())
        .children(layout.fragments.iter().enumerate().map(|(index, bounds)| {
            let local = bounds.origin - layout.bounds.origin;
            let fragment = if index == 0 {
                focusable_control(index, editor.clone())
                    .role(gpui::Role::Link)
                    .aria_label(link.data.label.clone())
            } else {
                gpui::div().id(index)
            };
            opens_link(fragment, editor.clone(), link)
                .absolute()
                .left(local.x)
                .top(local.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .when(index == 0, |div| {
                    div.debug_selector(|| "quire-web-link-first-fragment".into())
                })
                .when(index + 1 == layout.fragments.len(), |div| {
                    div.debug_selector(|| "quire-web-link-last-fragment".into())
                })
        }))
        .into_any_element()
}

/// Makes `element` open `link` on click (beside the page with Shift) without moving the caret,
/// with the address as its tooltip when the link leaves the notebook and a preview of the page
/// when it names one.
fn opens_link(
    element: gpui::Stateful<gpui::Div>,
    editor: WeakEntity<Editor>,
    link: &AnchoredLink,
) -> gpui::Stateful<gpui::Div> {
    let range = link.range.clone();
    let data = link.data.clone();
    let page = match &link.data.destination {
        crate::markdown_links::Destination::Reference(crate::references::Target::Page(page)) => {
            Some(page.clone())
        }
        _ => None,
    };
    let previewed = editor.clone();
    element
        .cursor_pointer()
        .when_some(link.url(), |div, url| {
            div.tooltip(ui::Tooltip::text(external_tooltip(url)))
        })
        .when_some(page, move |div, page| {
            div.hoverable_tooltip(move |_, cx| {
                let index = previewed.upgrade().and_then(|editor| {
                    crate::references::host(editor.read(cx))
                        .and_then(|host| host.read(cx).index().cloned())
                });
                cx.new(|cx| {
                    crate::references::preview::PagePreviewTooltip::new(
                        index.as_ref(),
                        page.clone(),
                        cx,
                    )
                })
                .into()
            })
        })
        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .on_click(move |event, window, cx| {
            cx.stop_propagation();
            let beside = event.modifiers().shift;
            editor
                .update(cx, |editor, cx| {
                    open_link(editor, range.clone(), &data.source, beside, window, cx)
                })
                .ok();
        })
}

#[cfg(test)]
pub type Highlight = Option<(gpui::HighlightStyle, Vec<Range<usize>>)>;

/// The style and source ranges of the internal and then the external link highlights, and the
/// style external links should have, for tests.
#[cfg(test)]
pub fn link_highlights(editor: &Editor, cx: &App) -> ([Highlight; 2], gpui::HighlightStyle) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let read = |key| {
        editor.text_highlights(key, cx).map(|(style, ranges)| {
            let ranges = ranges
                .iter()
                .map(|range| range.start.to_offset(&snapshot).0..range.end.to_offset(&snapshot).0)
                .collect();
            (style, ranges)
        })
    };
    (
        [
            read(link_key::<Projection>()),
            read(link_key::<ExternalLink>()),
        ],
        ExternalPalette::of(cx).style(),
    )
}

/// The ↗ after a link that leaves the notebook, standing in for its hidden closing syntax, or
/// for the `kept` last character of a bare URL, which it draws before the arrow. Clicking it
/// opens the link; the label stays the link's one keyboard stop.
fn arrow_placeholder(
    editor: WeakEntity<Editor>,
    link: AnchoredLink,
    kept: Option<char>,
    palette: ExternalPalette,
) -> FoldPlaceholder {
    FoldPlaceholder {
        collapsed_text: Some(kept.map_or_else(|| "↗".into(), |last| last.to_string().into())),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        render: Arc::new(move |_, _, cx| {
            use settings::Settings as _;
            let arrow = gpui::div()
                .id(("quire-link-arrow", link.id))
                .debug_selector(|| "quire-link-arrow".into());
            opens_link(arrow, editor.clone(), &link)
                .flex()
                .font(
                    theme_settings::ThemeSettings::get_global(cx)
                        .buffer_font
                        .clone(),
                )
                .when_some(kept, |div, last| {
                    div.child(
                        gpui::div()
                            .text_color(gpui::rgb(palette.ink))
                            .underline()
                            .text_decoration_color(gpui::rgb(palette.line))
                            .child(last.to_string()),
                    )
                })
                .child(gpui::div().text_color(gpui::rgb(palette.arrow)).child("↗"))
                .into_any_element()
        }),
    }
}

fn toggle_task_marker(
    editor: &mut Editor,
    range: Range<Anchor>,
    expected: &str,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(projection) = editor.addon::<Projection>() else {
        return;
    };
    if !projection_is_current(editor, cx) {
        return;
    }
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let start = range.start.to_offset(&snapshot);
    let is_list = projection.blocks.iter().any(|block| {
        block.marker.start.to_offset(&snapshot) < start
            && block.marker.end.to_offset(&snapshot) == start
    });
    if !is_list {
        return;
    }
    let current: String = snapshot.text_for_range(range.clone()).collect();
    if current != expected {
        return;
    }
    if crate::task_marker::read(&current).is_none() {
        return;
    }
    crate::tasks::toggle_at(editor, start.0, window, cx);
    window.focus(&editor.focus_handle(cx), cx);
}

fn follow(
    editor: &Editor,
    target: crate::references::Target,
    beside: bool,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(host) = crate::references::host(editor) else {
        return;
    };
    let notebook = host.read(cx).notebook().clone();
    notebook.follow(
        crate::references::Follow {
            target,
            beside,
            backlinks: false,
        },
        window,
        cx,
    );
}

const PREVIEW_CHARS: usize = 80;

/// What an unlabeled block reference shows: the block's text, or that it does not resolve.
fn preview_label(preview: &AnchoredPreview, text: Option<&str>) -> String {
    match text {
        Some(text) => {
            let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
            match text.char_indices().nth(PREVIEW_CHARS) {
                Some((end, _)) => format!("↗ {}…", &text[..end]),
                None => format!("↗ {text}"),
            }
        }
        None => format!("{} · unresolved", preview.target),
    }
}

fn preview_placeholder(
    editor: WeakEntity<Editor>,
    ordinal: usize,
    preview: AnchoredPreview,
    text: Option<String>,
) -> FoldPlaceholder {
    let label: gpui::SharedString = preview_label(&preview, text.as_deref()).into();
    let resolved = text.is_some();
    // Zed's wrap and tab maps assume a placeholder's text cannot split, so it stays one character
    // and the rendered element carries the label.
    FoldPlaceholder {
        collapsed_text: Some(if resolved { "↗" } else { "?" }.into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        render: Arc::new(move |_, _, cx| {
            use theme::ActiveTheme;
            let editor = editor.clone();
            let preview = preview.clone();
            let colors = cx.theme().colors();
            inline_control(("quire-block-preview", ordinal), editor.clone())
                .debug_selector(|| "quire-block-preview".into())
                .role(gpui::Role::Link)
                .aria_label(label.clone())
                .cursor_pointer()
                .text_color(if resolved {
                    colors.link_text_hover
                } else {
                    colors.text_muted
                })
                .child(label.clone())
                .on_click(move |event, window, cx| {
                    cx.stop_propagation();
                    let beside = event.modifiers().shift;
                    editor
                        .update(cx, |editor, cx| {
                            let snapshot = editor.buffer().read(cx).snapshot(cx);
                            let start = preview.range.start.to_offset(&snapshot);
                            if projection_is_current(editor, cx)
                                && snapshot.contains_str_at(start, &preview.source)
                            {
                                let target =
                                    crate::references::Target::Block(preview.target.clone());
                                follow(editor, target, beside, window, cx);
                            }
                        })
                        .ok();
                })
                .into_any_element()
        }),
    }
}

/// Asks the notebook for the text of each previewed block not asked for yet, or of all of them
/// when `again`, because the index changed.
fn request_previews(
    editor: &mut Editor,
    again: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(index) =
        crate::references::host(editor).and_then(|host| host.read(cx).index().cloned())
    else {
        return;
    };
    let Some(projection) = editor.addon_mut::<Projection>() else {
        return;
    };
    let mut targets: Vec<String> = projection
        .previews
        .iter()
        .map(|preview| preview.target.clone())
        .filter(|target| again || !projection.requests.contains_key(target))
        .collect();
    targets.sort();
    targets.dedup();
    for target in targets {
        projection.sequence += 1;
        let sequence = projection.sequence;
        projection.requests.insert(target.clone(), sequence);
        let reply = index.block_preview(target.clone());
        cx.spawn_in(window, async move |editor, cx| {
            let found = crate::query_index::ask(reply).await;
            editor
                .update_in(cx, |editor, window, cx| {
                    let Some(projection) = editor.addon_mut::<Projection>() else {
                        return;
                    };
                    if projection.requests.get(&target) != Some(&sequence) {
                        return;
                    }
                    match found {
                        Ok(found) => {
                            projection
                                .preview_text
                                .insert(target, found.map(|preview| preview.text));
                        }
                        Err(_) => {
                            projection.requests.remove(&target);
                        }
                    }
                    let blocks = projection.blocks.clone();
                    refresh(editor, &blocks, window, cx);
                })
                .ok();
        })
        .detach();
    }
}

/// The labels of block previews whose target has been looked up, in page order.
#[cfg(test)]
pub fn preview_labels(editor: &Editor) -> Vec<String> {
    let Some(projection) = editor.addon::<Projection>() else {
        return Vec::new();
    };
    projection
        .previews
        .iter()
        .filter_map(|preview| {
            let text = projection.preview_text.get(&preview.target)?;
            Some(preview_label(preview, text.as_deref()))
        })
        .collect()
}

/// Folds the view again, for decorations whose state changed outside a parse.
pub(crate) fn refold(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    if let Some(projection) = editor.addon::<Projection>() {
        let blocks = projection.blocks.clone();
        refresh(editor, &blocks, window, cx);
    }
}

/// The notebook's index answers differently: ask again for every preview.
pub fn references_changed(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    request_previews(editor, true, window, cx);
    crate::embeds::requery(editor, window, cx);
    crate::task_lists::requery(editor, cx);
}

fn checkbox_placeholder(
    editor: WeakEntity<Editor>,
    ordinal: usize,
    source: String,
    checked: bool,
) -> FoldPlaceholder {
    FoldPlaceholder {
        collapsed_text: Some(if checked { "☑" } else { "☐" }.into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        render: Arc::new(move |_, range, cx| {
            let editor = editor.clone();
            let source = source.clone();
            let group = gpui::SharedString::from(format!("quire-task-checkbox-{ordinal}"));
            inline_control(("quire-task-checkbox", ordinal), editor.clone())
                .debug_selector(|| "quire-task-checkbox".into())
                .group(group.clone())
                .h_full()
                .flex()
                .items_center()
                .pr(gpui::px(6.))
                .cursor_pointer()
                .role(gpui::Role::CheckBox)
                .aria_label(if checked {
                    "Mark task incomplete"
                } else {
                    "Complete task"
                })
                .aria_toggled(if checked {
                    gpui::Toggled::True
                } else {
                    gpui::Toggled::False
                })
                .child(crate::task_controls::checkbox(checked, group, cx))
                .on_click(move |_, window, cx| {
                    cx.stop_propagation();
                    editor
                        .update(cx, |editor, cx| {
                            toggle_task_marker(editor, range.clone(), &source, window, cx)
                        })
                        .ok();
                })
                .into_any_element()
        }),
    }
}

/// Keeps carets on visible text. On a task line the checkbox is visible too: a caret on the
/// text start or checkbox that jumps to the line start (Home, Vim's `0` or `^`) lands on the
/// checkbox, and one from further along lands on the text, so each jump moves.
/// A caret that lands in a collapsed block's hidden ID or children leaves them, as `rt` keeps it: a
/// step forward reaches the next line, any other move the block's text end.
fn clamp_carets(
    editor: &mut Editor,
    blocks: &[AnchoredBlock],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let previous: Vec<MultiBufferOffset> = editor
        .addon::<Projection>()
        .map(|projection| {
            projection
                .heads
                .iter()
                .map(|head| head.to_offset(&snapshot))
                .collect()
        })
        .unwrap_or_default();
    let collapsed = crate::outline::folds::collapsed(editor, cx);
    // Vim's Normal mode keeps the caret on a character, so the last visible one before an ID.
    let on_characters = editor.clip_at_line_ends(cx);
    let mut changed = false;
    let selections = editor
        .selections
        .disjoint_anchors_arc()
        .iter()
        .enumerate()
        .map(|(index, selection)| {
            let mut selection = selection.clone().map(|anchor| anchor.to_offset(&snapshot));
            if selection.start != selection.end {
                return selection;
            }
            let from = previous.get(index).copied();
            let mut position = selection.start;
            for block in blocks {
                let start = block.marker.start.to_offset(&snapshot);
                let checkbox = block.marker.end.to_offset(&snapshot);
                let content = text_start(block, &snapshot);
                if position >= start && position < content {
                    position = match from {
                        Some(from) if checkbox <= from && from <= content => position.max(checkbox),
                        _ => content,
                    };
                }
                if let Some(children) = block.children.map(|at| at.to_offset(&snapshot))
                    && let Some(fold) = collapsed.iter().find(|fold| fold.start == children)
                {
                    let tail = id_range(block, &snapshot).map_or(children, |id| id.start)..fold.end;
                    if position > tail.start && position <= tail.end {
                        let next = MultiBufferOffset(tail.end.0 + 1);
                        position = match from {
                            Some(from) if from <= tail.start && next <= snapshot.len() => next,
                            _ => tail.start,
                        };
                    }
                }
                if let Some(id) = id_range(block, &snapshot)
                    && position >= id.start
                    && position <= id.end
                {
                    position = match snapshot.reversed_chars_at(id.start).next() {
                        Some(last) if on_characters && id.start > content => {
                            MultiBufferOffset(id.start.0 - last.len_utf8())
                        }
                        _ => id.start,
                    };
                }
            }
            changed |= position != selection.start;
            selection.start = position;
            selection.end = position;
            selection
        })
        .collect::<Vec<_>>();
    if let Some(projection) = editor.addon_mut::<Projection>() {
        projection.heads = selections
            .iter()
            .map(|selection| snapshot.anchor_before(selection.head()))
            .collect();
    }
    if changed {
        editor.change_selections(editor::SelectionEffects::no_scroll(), window, cx, |s| {
            s.select(selections)
        });
    }
}

/// After a selection to the line end (Shift-End, Cmd-Shift-Right), ends each selection that
/// stopped inside a hidden ID or collapsed children at the text before them, as `rt`'s
/// `toTextEnd` does, so typing over the selection keeps them.
pub(crate) fn select_to_text_end(
    editor: &mut Editor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(blocks) = editor
        .addon::<Projection>()
        .map(|projection| projection.blocks.clone())
    else {
        return;
    };
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let collapsed = crate::outline::folds::collapsed(editor, cx);
    let hidden: Vec<_> = blocks
        .iter()
        .flat_map(|block| hidden_tails(block, &collapsed, &snapshot))
        .collect();
    let selections: Vec<_> = editor
        .selections
        .disjoint_anchors_arc()
        .iter()
        .map(|selection| {
            let mut selection = selection.clone().map(|anchor| anchor.to_offset(&snapshot));
            let head = selection.head();
            if let Some(hidden) = hidden
                .iter()
                .find(|hidden| hidden.start < head && head <= hidden.end)
            {
                selection.set_head(hidden.start, selection.goal);
            }
            selection
        })
        .collect();
    editor.change_selections(Default::default(), window, cx, |s| s.select(selections));
}

fn filter_edits(
    edits: &mut [ViewEdit],
    protected: &[AnchoredBlock],
    snapshot: &MultiBufferSnapshot,
    intent: ViewEditIntent,
) {
    if intent == ViewEditIntent::Structural {
        return;
    }
    for (range, inserted) in edits {
        if !inserted.is_empty()
            && range.start == MultiBufferOffset(0)
            && range.end == snapshot.len()
        {
            continue;
        }
        if intent == ViewEditIntent::DeleteLines && inserted.is_empty() {
            continue;
        }
        for block in protected.iter() {
            let content_start = text_start(block, snapshot);
            if intent == ViewEditIntent::Change
                && inserted.is_empty()
                && range.start <= content_start
            {
                let point = snapshot.offset_to_point(content_start);
                let line_end = snapshot.point_to_offset(language::Point::new(
                    point.row,
                    snapshot.line_len(MultiBufferRow(point.row)),
                ));
                if snapshot.offset_to_point(range.start).row == point.row && range.end == line_end {
                    range.start = content_start;
                    if let Some(id) = id_range(block, snapshot)
                        && range.end >= id.start
                    {
                        let gap = snapshot
                            .chars_at(id.start)
                            .take_while(|c| matches!(c, ' ' | '\t'))
                            .count();
                        range.end = MultiBufferOffset(id.start.0 + gap);
                    }
                }
            }
            let Some(id) = id_range(block, snapshot) else {
                continue;
            };
            let id_start = id.start;
            let id_end = id.end;
            if !inserted.is_empty() && range.start >= id_start && range.end <= id_end {
                *range = id_start..id_start;
            }
            if inserted.is_empty() && range.start == content_start && range.end > id_end {
                range.start = block.marker.start.to_offset(snapshot);
            }
            // Typing over a block's text and the lines after it, to a line end, keeps the
            // block's hidden ID after the typed text, as rt does.
            if !inserted.is_empty()
                && range.start >= content_start
                && range.start <= id_start
                && range.end > id_end
                && matches!(snapshot.chars_at(range.end).next(), None | Some('\n'))
            {
                let id: String = snapshot.text_for_range(id_start..id_end).collect();
                *inserted = format!("{inserted}{id}").into();
            }
            if !inserted.is_empty()
                && id_start == content_start
                && range.start >= block.marker.start.to_offset(snapshot)
                && range.start <= content_start
                && range.end <= id_end
            {
                *range = content_start..content_start;
            }
            if range.start >= content_start
                && range.start <= id_start
                && range.end > id_start
                && range.end <= id_end
            {
                range.end = id_start;
                if inserted.is_empty() && range.start == content_start {
                    let gap = snapshot
                        .chars_at(id_start)
                        .take_while(|c| matches!(c, ' ' | '\t'))
                        .count();
                    range.end.0 += gap;
                }
            }
            if !inserted.is_empty()
                && range.start == content_start
                && range.end == content_start
                && snapshot.chars_at(content_start).next() == Some('^')
                && !inserted.ends_with(char::is_whitespace)
            {
                *inserted = format!("{inserted} ").into();
            }
        }
    }
}

fn apply_spans(
    editor: &mut Editor,
    blocks: &[BlockSyntax],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let buffer = editor.buffer().clone();
    let snapshot = buffer.read(cx).snapshot(cx);
    let blocks: Arc<[AnchoredBlock]> = blocks
        .iter()
        .map(|block| AnchoredBlock {
            marker: snapshot.anchor_before(block.marker.start)
                ..snapshot.anchor_before(block.marker.end),
            id: block
                .id
                .as_ref()
                .map(|id| snapshot.anchor_after(id.start)..snapshot.anchor_before(id.end)),
            children: block.children.map(|at| snapshot.anchor_before(at)),
            subtree_end: block.subtree_end.map(|at| snapshot.anchor_before(at)),
            depth: block.depth,
        })
        .collect();
    let protected = blocks.clone();
    buffer.update(cx, |buffer, _| {
        buffer.set_edit_filter(Some(Arc::new(move |edits, snapshot, intent| {
            filter_edits(edits, &protected, snapshot, intent)
        })))
    });
    refresh(editor, &blocks, window, cx);
    clamp_carets(editor, &blocks, window, cx);
    editor
        .addon_mut::<Projection>()
        .expect("installed projection")
        .blocks = blocks;
}

fn rebuild(
    editor: &mut Editor,
    source: &Entity<language::Buffer>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if *source.read(cx).parse_status().borrow() != language::ParseStatus::Idle {
        return;
    }
    let snapshot = source.read(cx).snapshot();
    if editor
        .addon::<Projection>()
        .and_then(|p| p.parsed.as_ref())
        .is_some_and(|(text, count)| {
            text.version() == snapshot.version() && *count == snapshot.syntax_update_count()
        })
    {
        return;
    }
    let Some(blocks) = crate::markdown_blocks::blocks(&snapshot) else {
        return;
    };
    let Some(links) = crate::markdown_links::links(&snapshot, &blocks) else {
        return;
    };
    let Some(references) = crate::page_analysis::references(&snapshot) else {
        return;
    };
    apply_blocks(editor, &snapshot, &blocks, links, &references, window, cx);
}

/// Blocks holding only a task query, except on the Templates page where queries belong to
/// templates: the `{{...}}` range, the block's range, and the query.
fn task_queries(
    editor: &Editor,
    text: &str,
    blocks: &[crate::markdown_blocks::Block],
    cx: &App,
) -> Vec<(Range<usize>, Range<usize>, String)> {
    let templates = crate::references::host(editor)
        .and_then(|host| {
            let buffer = editor.buffer().read(cx).all_buffers_iter().next()?;
            host.read(cx).notebook().page_path(&buffer, cx)
        })
        .is_some_and(|path| crate::task_lists::is_templates_page(&path));
    if templates {
        return Vec::new();
    }
    blocks
        .iter()
        .filter(|block| block.kind != "code")
        .filter_map(|block| {
            let rest = text.get(block.content_from..)?;
            let line = &rest[..rest.find('\n').unwrap_or(rest.len())];
            if !line.contains("{{") {
                return None;
            }
            let found = quire_core::task_query::query_line(line)?;
            Some((
                block.content_from + found.range.start..block.content_from + found.range.end,
                block.from..block.content_to,
                found.query.to_owned(),
            ))
        })
        .collect()
}

fn apply_blocks(
    editor: &mut Editor,
    snapshot: &language::BufferSnapshot,
    blocks: &[crate::markdown_blocks::Block],
    mut links: Vec<crate::markdown_links::Link>,
    references: &[quire_core::markdown::Reference],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let spans = crate::markdown_blocks::syntax_spans(snapshot, blocks);
    let view = editor.buffer().read(cx).snapshot(cx);
    let (reference_links, previews) =
        crate::markdown_links::reference_links(snapshot, blocks, references);
    links.retain(|link| {
        link.label_range != link.range
            || !references.iter().any(|reference| {
                reference.range.start < link.range.end && link.range.start < reference.range.end
            })
    });
    links.extend(reference_links);
    links.sort_by_key(|link| link.range.start);
    let page_text = snapshot.text();
    let queries = task_queries(editor, &page_text, blocks, cx);
    let in_query = |range: &Range<usize>| {
        queries
            .iter()
            .any(|(braces, ..)| braces.start < range.end && range.start < braces.end)
    };
    links.retain(|link| !in_query(&link.range));
    if crate::tag_labels::formats(cx) {
        let tags = quire_core::markdown::wiki_tags(&page_text, references);
        links.retain(|link| {
            !tags
                .iter()
                .any(|tag| tag.range.start + 1 == link.range.start)
        });
    }
    let previews: Vec<_> = previews
        .into_iter()
        .filter(|preview| !in_query(&preview.range))
        .collect();
    let shown = |range: &Range<usize>| {
        view.excerpts().any(|excerpt| {
            excerpt.context.start.buffer_id == snapshot.remote_id()
                && text::ToOffset::to_offset(&excerpt.context.start, &snapshot.text) <= range.start
                && range.end <= text::ToOffset::to_offset(&excerpt.context.end, &snapshot.text)
        })
    };
    let anchor = |byte: usize| view.anchor_in_buffer(snapshot.anchor_before(byte));
    let previews: Vec<_> = previews
        .into_iter()
        .filter(|preview| shown(&preview.range))
        .filter_map(|preview| {
            Some(AnchoredPreview {
                range: anchor(preview.range.start)?..anchor(preview.range.end)?,
                block: anchor(preview.block_range.start)?..anchor(preview.block_range.end)?,
                target: preview.target,
                source: preview.source,
            })
        })
        .collect();
    editor.addon_mut::<Projection>().unwrap().previews = previews;
    let spots: Vec<_> = references
        .iter()
        .filter(|reference| crate::markdown_links::is_full_embed(&page_text, reference))
        .filter(|reference| shown(&reference.range))
        .filter_map(|reference| {
            let target = crate::references::Target::of(reference)?;
            let block = blocks
                .get(blocks.partition_point(|block| block.content_to <= reference.range.start))
                .filter(|block| block.content_from <= reference.range.start)?;
            Some(crate::embeds::Spot {
                range: anchor(reference.range.start)?..anchor(reference.range.end)?,
                block: anchor(block.from)?..anchor(block.content_to)?,
                source: page_text[reference.range.clone()].to_owned(),
                target,
            })
        })
        .collect();
    crate::embeds::sync(editor, spots, window, cx);
    let media = crate::media::found(snapshot, blocks, references)
        .into_iter()
        .filter(|found| shown(&found.range) && !in_query(&found.range))
        .filter_map(|found| {
            Some(crate::media::Spot {
                range: anchor(found.range.start)?..anchor(found.range.end)?,
                block: anchor(found.block_range.start)?..anchor(found.block_range.end)?,
                source: page_text[found.range].to_owned(),
                target: found.target,
                label: found.label,
                kind: found.kind,
            })
        })
        .collect();
    crate::media::sync(editor, media, window, cx);
    let toolbars = crate::code_format::toolbar::found(snapshot)
        .into_iter()
        .filter(|found| shown(&found.block_range))
        .filter_map(|found| {
            Some(crate::code_format::toolbar::Spot {
                range: anchor(found.range.start)?..anchor(found.range.end)?,
                block: anchor(found.block_range.start)?..anchor(found.block_range.end)?,
                source: page_text[found.range].to_owned(),
                tag: found.tag,
                text: found.text,
            })
        })
        .collect();
    crate::code_format::toolbar::sync(editor, toolbars);
    crate::task_controls::sync(editor, &page_text, blocks, shown, anchor, window, cx);
    let lists = queries
        .into_iter()
        .filter(|(braces, ..)| shown(braces))
        .filter_map(|(braces, block, query)| {
            Some(crate::task_lists::Spot {
                range: anchor(braces.start)?..anchor(braces.end)?,
                block: anchor(block.start)?..anchor(block.end)?,
                source: page_text[braces].to_owned(),
                query,
            })
        })
        .collect();
    crate::task_lists::sync(editor, lists, window, cx);
    let links = links
        .into_iter()
        .filter_map(|link| {
            if !view.excerpts().any(|excerpt| {
                excerpt.context.start.buffer_id == snapshot.remote_id()
                    && text::ToOffset::to_offset(&excerpt.context.start, &snapshot.text)
                        <= link.range.start
                    && link.range.end
                        <= text::ToOffset::to_offset(&excerpt.context.end, &snapshot.text)
            }) {
                return None;
            }
            let start = view.anchor_in_buffer(snapshot.anchor_before(link.range.start))?;
            let end = view.anchor_in_buffer(snapshot.anchor_before(link.range.end))?;
            let label = view.anchor_in_buffer(snapshot.anchor_before(link.label_range.start))?
                ..view.anchor_in_buffer(snapshot.anchor_before(link.label_range.end))?;
            let block = view.anchor_in_buffer(snapshot.anchor_before(link.block_range.start))?
                ..view.anchor_in_buffer(snapshot.anchor_before(link.block_range.end))?;
            let web = matches!(link.destination, crate::markdown_links::Destination::Web(_));
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&(web, start, end), &mut hasher);
            Some(AnchoredLink {
                range: start..end,
                label,
                block,
                id: std::hash::Hasher::finish(&hasher),
                data: Arc::new(link),
            })
        })
        .collect();
    editor.addon_mut::<Projection>().unwrap().links = links;
    let mut marks = crate::write_style::marks(snapshot, blocks).unwrap_or_default();
    marks.urdu = crate::write_style::urdu_runs(&page_text);
    let projection = editor.addon_mut::<Projection>().unwrap();
    projection.style = crate::write_style::Presentation::anchor(snapshot, marks, shown, anchor);
    projection.styled = false;
    let spans: Vec<_> = spans
        .into_iter()
        .filter_map(|span| {
            let contained = view.excerpts().any(|excerpt| {
                if excerpt.context.start.buffer_id != snapshot.remote_id() {
                    return false;
                }
                let start = text::ToOffset::to_offset(&excerpt.context.start, &snapshot.text);
                let end = text::ToOffset::to_offset(&excerpt.context.end, &snapshot.text);
                start <= span.marker.start
                    && span.marker.end <= end
                    && span
                        .id
                        .as_ref()
                        .is_none_or(|id| start <= id.start && id.end <= end)
            });
            if !contained {
                return None;
            }
            let offset = |byte| {
                view.anchor_in_buffer(snapshot.anchor_before(byte))
                    .map(|anchor| anchor.to_offset(&view))
            };
            let marker = offset(span.marker.start)?..offset(span.marker.end)?;
            let id = match span.id {
                Some(id) => Some(offset(id.start)?..offset(id.end)?),
                None => None,
            };
            let children = span.children.and_then(offset);
            Some(BlockSyntax {
                marker,
                id,
                children,
                subtree_end: span.subtree_end.and_then(offset),
                depth: span.depth,
            })
        })
        .collect();
    apply_spans(editor, &spans, window, cx);
    crate::outline::folds::blocks_changed(editor, snapshot, blocks, window, cx);
    editor
        .addon_mut::<Projection>()
        .expect("installed projection")
        .parsed = Some((snapshot.text.clone(), snapshot.syntax_update_count()));
    request_previews(editor, false, window, cx);
}

pub fn bind(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) -> Result<(), String> {
    editor.update(cx, |editor, cx| {
        if enabled(editor) { return Ok(()); }
        let sources: Vec<_> = editor.buffer().read(cx).all_buffers_iter().collect();
        let [source] = sources.as_slice() else {
            return Err("Cannot enable Write for a view containing multiple pages. Open one page or subtree and retry.".into());
        };
        let source = source.clone();
        let editor_subscription = cx.subscribe_in(
            &cx.entity(),
            window,
            |editor, _, event: &EditorEvent, window, cx| {
                let Some(blocks) = editor.addon::<Projection>().map(|p| p.blocks.clone()) else { return; };
                match event {
                EditorEvent::BufferEdited | EditorEvent::Focused | EditorEvent::Blurred => refresh(editor, &blocks, window, cx),
                EditorEvent::SelectionsChanged { local: true } => {
                    // Blocks from an older parse would clamp against moved text; the rebuild after
                    // the next parse clamps instead.
                    if projection_is_current(editor, cx) {
                        clamp_carets(editor, &blocks, window, cx);
                    }
                    refresh(editor, &blocks, window, cx);
                }
                _ => {}
                }
            },
        );
        let source_subscription = cx.subscribe_in(&source, window, |editor, source, event: &language::BufferEvent, window, cx| {
            if matches!(event, language::BufferEvent::Reparsed | language::BufferEvent::LanguageChanged(_)) { rebuild(editor, source, window, cx); }
        });
        let theme_subscription = cx.observe_global_in::<theme::GlobalTheme>(window, |editor, window, cx| {
            if let Some(projection) = editor.addon_mut::<Projection>() { projection.styled = false; }
            if let Some(blocks) = editor.addon::<Projection>().map(|projection| projection.blocks.clone()) {
                refresh(editor, &blocks, window, cx);
            }
        });
        editor.register_addon(Projection {
            blocks: Arc::default(), links: Vec::new(), previews: Vec::new(), preview_text: HashMap::new(),
            requests: HashMap::new(), sequence: 0, parsed: None, heads: Vec::new(), syntax_folds: 0, style: Default::default(), styled: false, hover: Arc::default(),
            _subscriptions: vec![editor_subscription, source_subscription, theme_subscription],
        });
        present(editor, true, cx);
        apply_spans(editor, &[], window, cx);
        rebuild(editor, &source, window, cx);
        Ok(())
    })
}

pub fn enabled(editor: &Editor) -> bool {
    editor.addon::<Projection>().is_some()
}

/// Rebuilds excerpt folds from the host's analysis of the current source version.
pub fn view_range_changed(
    editor: &mut Editor,
    source: &Entity<language::Buffer>,
    blocks: Option<&[crate::markdown_blocks::Block]>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(projection) = editor.addon_mut::<Projection>() else {
        return;
    };
    projection.parsed = None;
    projection.links.clear();
    projection.style = Default::default();
    projection.styled = false;
    projection.previews.clear();
    apply_spans(editor, &[], window, cx);
    if let Some(blocks) = blocks {
        let snapshot = source.read(cx).snapshot();
        let links = crate::markdown_links::links(&snapshot, blocks).unwrap_or_default();
        let references = crate::page_analysis::references(&snapshot).unwrap_or_default();
        apply_blocks(editor, &snapshot, blocks, links, &references, window, cx);
    }
}

pub fn unbind(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        if !enabled(editor) {
            return;
        }
        editor.unregister_addon::<Projection>();
        crate::embeds::clear(editor, cx);
        crate::media::clear(editor, cx);
        crate::task_lists::clear(editor, cx);
        crate::code_format::toolbar::clear(editor);
        crate::task_controls::clear(editor, cx);
        editor
            .buffer()
            .update(cx, |buffer, _| buffer.set_edit_filter(None));
        refresh(editor, &[], window, cx);
        crate::write_style::source_urdu(editor, cx);
        present(editor, false, cx);
    });
}

/// Write reads as rt's page: no current-line band, and no gutter fold arrows, since Write's
/// folds are its own presentation (hidden syntax and bullets) and each parent bullet draws its
/// own arrow. Source follows the editor settings again.
fn present(editor: &mut Editor, write: bool, cx: &mut Context<Editor>) {
    editor.set_current_line_highlight(write.then_some(editor::CurrentLineHighlight::None));
    editor.set_show_fold_toggles(write.then_some(false), cx);
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn old_link_cannot_open_a_url_after_the_source_becomes_code(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let note = "- [Link](https://example.com) ^id\n";
        let source = cx.update(|cx| crate::markdown::buffer(note, cx));
        let window =
            cx.add_window(|window, cx| Editor::for_buffer(source.clone(), None, window, cx));
        let editor = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            bind(&editor, window, cx).unwrap()
        })
        .unwrap();
        cx.executor().run_until_parked();
        let link = window
            .update(cx, |editor, _, _| {
                editor.addon::<Projection>().unwrap().links[0].clone()
            })
            .unwrap();
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit(
                [(0..0, "```md\n"), (note.len()..note.len(), "```\n")],
                None,
                cx,
            );
        });
        window
            .update(cx, |editor, view_window, cx| {
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                assert_eq!(
                    snapshot
                        .text_for_range(link.range.clone())
                        .collect::<String>(),
                    link.data.source
                );
                open_link(
                    editor,
                    link.range.clone(),
                    &link.data.source,
                    false,
                    view_window,
                    cx,
                );
            })
            .unwrap();
        assert!(cx.opened_url().is_none());
        cx.executor().run_until_parked();
        window
            .update(cx, |editor, view_window, cx| {
                open_link(
                    editor,
                    link.range.clone(),
                    &link.data.source,
                    false,
                    view_window,
                    cx,
                )
            })
            .unwrap();
        assert!(cx.opened_url().is_none());
    }

    #[gpui::test]
    fn old_checkbox_cannot_toggle_a_task_moved_into_code(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let note = "- [ ] task ^id\n";
        let code = format!("```md\n{note}```\n");
        let source = cx.update(|cx| crate::markdown::buffer(note, cx));
        let window =
            cx.add_window(|window, cx| Editor::for_buffer(source.clone(), None, window, cx));
        let editor = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            bind(&editor, window, cx).unwrap()
        })
        .unwrap();
        cx.executor().run_until_parked();
        let range = window
            .update(cx, |editor, _, cx| {
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                snapshot.anchor_before(MultiBufferOffset(2))
                    ..snapshot.anchor_before(MultiBufferOffset(5))
            })
            .unwrap();
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit(
                [(0..0, "```md\n"), (note.len()..note.len(), "```\n")],
                None,
                cx,
            );
        });
        window
            .update(cx, |editor, window, cx| {
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                assert_eq!(
                    snapshot.text_for_range(range.clone()).collect::<String>(),
                    "[ ]"
                );
                toggle_task_marker(editor, range.clone(), "[ ]", window, cx);
                assert_eq!(editor.text(cx), code);
            })
            .unwrap();
        cx.executor().run_until_parked();
        window
            .update(cx, |editor, window, cx| {
                toggle_task_marker(editor, range.clone(), "[ ]", window, cx);
                assert_eq!(editor.text(cx), code);
            })
            .unwrap();
    }

    #[gpui::test]
    fn obsolete_task_callbacks_do_not_edit_source_or_changed_markers(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer("- [ ] task ^id\n", cx));
        let window =
            cx.add_window(|window, cx| Editor::for_buffer(source.clone(), None, window, cx));
        let editor = window.update(cx, |_, _, cx| cx.entity()).unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            bind(&editor, window, cx).unwrap()
        })
        .unwrap();
        cx.executor().run_until_parked();
        let range = window
            .update(cx, |editor, _, cx| {
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                snapshot.anchor_before(MultiBufferOffset(2))
                    ..snapshot.anchor_before(MultiBufferOffset(5))
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| unbind(&editor, window, cx))
            .unwrap();
        window
            .update(cx, |editor, window, cx| {
                toggle_task_marker(editor, range.clone(), "[ ]", window, cx);
                assert_eq!(editor.text(cx), "- [ ] task ^id\n");
            })
            .unwrap();
        cx.update_window(window.into(), |_, window, cx| {
            bind(&editor, window, cx).unwrap()
        })
        .unwrap();
        source.update(cx, |source, cx| source.edit([(2..5, "[x]")], None, cx));
        window
            .update(cx, |editor, window, cx| {
                toggle_task_marker(editor, range.clone(), "[ ]", window, cx);
                assert_eq!(editor.text(cx), "- [x] task ^id\n");
            })
            .unwrap();
    }
}
