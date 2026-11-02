//! Collapsing a block's children in Write. Every list item with children gets a Zed crease over
//! them, so Zed's gutter disclosure and fold actions (`editor::Fold`, `FoldAll`, Vim `za`, `zc`,
//! `zo`) fold outline blocks with the outline placeholder; a collapsed item's bullet shows a
//! ring. A page's folds are saved in the notebook's fold store and come back when the page opens
//! again. Folds are display state: they never change the Markdown, and they follow their block
//! through outline commands that move it.

use super::{
    Page,
    fold_store::{FoldStore, Mark},
    source_buffer, to_source, view_range,
};
use crate::markdown_blocks::Block;
use editor::{
    Addon, Editor, EditorEvent, FoldPlaceholder,
    display_map::{Crease, CreaseId},
};
use gpui::{App, Context, Entity, Window};
use multi_buffer::{Anchor, MultiBufferOffset, ToOffset};
use std::{any::TypeId, ops::Range, rc::Rc};

/// Marks folds that collapse a block's children, apart from the folds hiding Write syntax.
pub(super) struct OutlineFold;

#[derive(Default)]
struct Folds {
    creases: Vec<CreaseId>,
    /// The collapsed children last seen, to notice when folds change.
    seen: Vec<Range<MultiBufferOffset>>,
    /// Whether the page's saved folds were applied to this editor.
    restored: bool,
    /// The folds this view had when it last saved, to save only what it changed since.
    saved: Vec<Mark>,
}

impl Addon for Folds {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn state(editor: &mut Editor) -> &mut Folds {
    if editor.addon::<Folds>().is_none() {
        editor.register_addon(Folds::default());
    }
    editor.addon_mut::<Folds>().expect("registered folds")
}

fn placeholder(editor: &Editor, cx: &App) -> FoldPlaceholder {
    FoldPlaceholder {
        type_tag: Some(TypeId::of::<OutlineFold>()),
        merge_adjacent: false,
        ..editor.default_fold_placeholder(cx)
    }
}

/// The collapsed block children in the view.
pub(crate) fn collapsed(editor: &Editor, cx: &mut App) -> Vec<Range<MultiBufferOffset>> {
    let snapshot = editor.display_snapshot(cx);
    let buffer = snapshot.buffer_snapshot();
    snapshot
        .folds_in_range(MultiBufferOffset(0)..buffer.len())
        .filter(|fold| fold.placeholder.type_tag == Some(TypeId::of::<OutlineFold>()))
        .map(|fold| fold.range.start.to_offset(buffer)..fold.range.end.to_offset(buffer))
        .filter(|range| !range.is_empty())
        .collect()
}

/// The collapsed block children that strictly intersect `range` in the view.
pub(crate) fn collapsed_in(
    editor: &Editor,
    range: Range<MultiBufferOffset>,
    cx: &mut App,
) -> Vec<Range<MultiBufferOffset>> {
    let snapshot = editor.display_snapshot(cx);
    let buffer = snapshot.buffer_snapshot();
    snapshot
        .folds_in_range(range)
        .filter(|fold| fold.placeholder.type_tag == Some(TypeId::of::<OutlineFold>()))
        .map(|fold| fold.range.start.to_offset(buffer)..fold.range.end.to_offset(buffer))
        .filter(|range| !range.is_empty())
        .collect()
}

/// The collapsed block children in the view, as source ranges.
pub(super) fn collapsed_source(editor: &Editor, cx: &mut App) -> Vec<Range<usize>> {
    collapsed(editor, cx)
        .into_iter()
        .filter_map(|range| {
            Some(to_source(editor, range.start, cx)?..to_source(editor, range.end, cx)?)
        })
        .collect()
}

/// Collapses the children in `range`, or expands them while keeping folds nested inside.
pub(super) fn set(
    editor: &mut Editor,
    range: Range<MultiBufferOffset>,
    collapse: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if collapse {
        let placeholder = placeholder(editor, cx);
        editor.fold_creases(vec![Crease::simple(range, placeholder)], false, window, cx);
        return;
    }
    let inner: Vec<_> = collapsed(editor, cx)
        .into_iter()
        .filter(|fold| range.start < fold.start && fold.end <= range.end)
        .collect();
    unfold(editor, &[range], cx);
    refold(editor, inner, window, cx);
}

fn unfold(editor: &mut Editor, ranges: &[Range<MultiBufferOffset>], cx: &mut Context<Editor>) {
    editor.remove_folds_with_type(ranges, TypeId::of::<OutlineFold>(), false, cx);
}

fn refold(
    editor: &mut Editor,
    ranges: Vec<Range<MultiBufferOffset>>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if ranges.is_empty() {
        return;
    }
    let placeholder = placeholder(editor, cx);
    let creases = ranges
        .into_iter()
        .map(|range| Crease::simple(range, placeholder.clone()))
        .collect();
    editor.fold_creases(creases, false, window, cx);
}

fn children(block: &Block) -> Option<Range<usize>> {
    (block.kind == "list" && block.subtree_to > block.content_to)
        .then_some(block.content_to..block.subtree_to)
}

/// Collapses or expands the children of the list item whose children start at `at`, for a click
/// on its bullet's arrow.
pub(crate) fn toggle_at(
    editor: &mut Editor,
    at: Anchor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let Some(buffer) = source_buffer(editor, cx) else {
        return;
    };
    let multi = editor.buffer().read(cx).snapshot(cx);
    let Some(start) = to_source(editor, at.to_offset(&multi), cx) else {
        return;
    };
    let Some(blocks) = crate::markdown_blocks::current_blocks(&buffer.read(cx).snapshot()) else {
        return;
    };
    let Some(range) = blocks
        .iter()
        .filter_map(children)
        .find(|range| range.start == start)
        .and_then(|range| view_range(editor, &buffer, range, cx))
    else {
        return;
    };
    let folded = collapsed(editor, cx).contains(&range);
    set(editor, range, !folded, window, cx);
}

/// Collapses or expands every list item with children that the view shows.
pub(super) fn set_all(
    editor: &mut Editor,
    page: &Page,
    collapse: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if !collapse {
        let everything = MultiBufferOffset(0)..editor.buffer().read(cx).snapshot(cx).len();
        unfold(editor, &[everything], cx);
        return;
    }
    let ranges = page
        .blocks
        .iter()
        .filter_map(children)
        .filter(|range| page.shown.start <= range.start && range.end <= page.shown.end)
        .filter(|range| page.zoom.is_none() || range.start > page.shown.start)
        .filter_map(|range| view_range(editor, &page.buffer, range, cx))
        .collect();
    refold(editor, ranges, window, cx);
}

/// Replaces the editor's outline creases after a parse, and on the first parse of a page
/// applies the folds saved for it. Then remembers the current folds.
pub(crate) fn blocks_changed(
    editor: &mut Editor,
    snapshot: &language::BufferSnapshot,
    blocks: &[Block],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let view = editor.buffer().read(cx).snapshot(cx);
    let anchor = |byte: usize| view.anchor_in_buffer(snapshot.anchor_before(byte));
    let placeholder = placeholder(editor, cx);
    let creases: Vec<Crease<Anchor>> = blocks
        .iter()
        .filter_map(children)
        .filter_map(|range| {
            Some(Crease::simple(
                anchor(range.start)?..anchor(range.end)?,
                placeholder.clone(),
            ))
        })
        .collect();
    let old = std::mem::take(&mut state(editor).creases);
    editor.remove_creases(old, cx);
    state(editor).creases = editor.insert_creases(creases, cx);
    let text = snapshot.text();
    if !state(editor).restored
        && let Some((store, page)) = persisted(editor, cx)
    {
        state(editor).restored = true;
        store.watch(&page, cx.entity().downgrade(), window.window_handle());
        let root = view_root(editor, blocks, cx);
        let ranges = super::fold_store::resolve(&text, blocks, &store.marks(&page))
            .into_iter()
            .filter(|block| Some(block.from) != root)
            .filter_map(children)
            .filter_map(|range| {
                let start = anchor(range.start)?.to_offset(&view);
                let end = anchor(range.end)?.to_offset(&view);
                Some(start..end)
            })
            .collect();
        refold(editor, ranges, window, cx);
    }
    remember(editor, &text, blocks, cx);
}

/// The fold store and page ID of the page a view shows: the whole page, a zoom, or an embed.
fn persisted(editor: &Editor, cx: &App) -> Option<(Rc<FoldStore>, String)> {
    let buffer = source_buffer(editor, cx)?;
    let host = crate::references::host(editor)?;
    let notebook = host.read(cx).notebook().clone();
    Some((notebook.fold_store()?, notebook.page_id(&buffer, cx)?))
}

/// Where the block a zoom or block embed shows starts, in source offsets. A whole-page view
/// has no root.
fn view_root(editor: &Editor, blocks: &[Block], cx: &App) -> Option<usize> {
    let multi = editor.buffer().read(cx);
    if multi.as_singleton().is_some() {
        return None;
    }
    let start = multi.snapshot(cx).excerpts().next()?.context.start;
    let buffer = source_buffer(editor, cx)?;
    let start = text::ToOffset::to_offset(&start, &buffer.read(cx).snapshot());
    blocks
        .iter()
        .find(|block| block.from <= start && start <= block.content_to)
        .map(|block| block.from)
}

/// Saves the folds this view collapsed or expanded since it last saved into the page's saved
/// folds. Several views can show one page, a zoom or an embed only part of it, so a view
/// changes only the folds it changed and keeps the others' as saved.
fn remember(editor: &mut Editor, text: &str, blocks: &[Block], cx: &mut Context<Editor>) {
    if !state(editor).restored {
        return;
    }
    let Some((store, page)) = persisted(editor, cx) else {
        return;
    };
    let starts = |marks: &[Mark]| -> Vec<usize> {
        super::fold_store::resolve(text, blocks, marks)
            .into_iter()
            .filter_map(children)
            .map(|range| range.start)
            .collect()
    };
    let current: Vec<usize> = collapsed_source(editor, cx)
        .into_iter()
        .map(|range| range.start)
        .collect();
    let previous = starts(&state(editor).saved);
    let mut folded: Vec<usize> = starts(&store.marks(&page))
        .into_iter()
        .filter(|start| current.contains(start) || !previous.contains(start))
        .chain(current.iter().copied())
        .collect();
    folded.sort_unstable();
    folded.dedup();
    state(editor).saved = super::fold_store::marks(text, blocks, &current);
    store.save(
        &page,
        super::fold_store::marks(text, blocks, &folded),
        cx.entity_id(),
        cx,
    );
}

/// Makes the view's folds the page's saved folds, after another view of the page, such as a zoom
/// or an embed, changed them. A Source view, or one whose saved folds are not applied yet, waits.
pub(super) fn sync(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    if !crate::editor_projection::enabled(editor) || !state(editor).restored {
        return;
    }
    let (Some((store, page)), Some(buffer)) = (persisted(editor, cx), source_buffer(editor, cx))
    else {
        return;
    };
    let snapshot = buffer.read(cx).snapshot();
    let Some(blocks) = crate::markdown_blocks::current_blocks(&snapshot) else {
        return;
    };
    let text = snapshot.text();
    let root = view_root(editor, &blocks, cx);
    let wanted: Vec<_> = super::fold_store::resolve(&text, &blocks, &store.marks(&page))
        .into_iter()
        .filter(|block| Some(block.from) != root)
        .filter_map(children)
        .filter_map(|range| view_range(editor, &buffer, range, cx))
        .collect();
    let extra: Vec<_> = collapsed(editor, cx)
        .into_iter()
        .filter(|range| !wanted.contains(range))
        .collect();
    unfold(editor, &extra, cx);
    let current = collapsed(editor, cx);
    let missing = wanted
        .into_iter()
        .filter(|range| !current.contains(range))
        .collect();
    refold(editor, missing, window, cx);
    let folded: Vec<usize> = collapsed_source(editor, cx)
        .into_iter()
        .map(|range| range.start)
        .collect();
    state(editor).saved = super::fold_store::marks(&text, &blocks, &folded);
}

/// Follows fold changes from any source, Zed's gutter and fold actions included: Write shows the
/// changed bullets, and the page's folds are remembered.
pub(super) fn follow(window: &mut Window, cx: &mut Context<Editor>) {
    cx.subscribe_in(
        &cx.entity(),
        window,
        |editor, _: &Entity<Editor>, event: &EditorEvent, window, cx| {
            if !matches!(event, EditorEvent::FoldsChanged)
                || !crate::editor_projection::enabled(editor)
            {
                return;
            }
            let now = collapsed(editor, cx);
            let changed = state(editor).seen != now;
            if changed {
                state(editor).seen = now;
            }
            if changed || crate::editor_projection::syntax_unfolded(editor, cx) {
                crate::editor_projection::refold(editor, window, cx);
            }
            if changed && let Some(buffer) = source_buffer(editor, cx) {
                let snapshot = buffer.read(cx).snapshot();
                if let Some(blocks) = crate::markdown_blocks::current_blocks(&snapshot) {
                    remember(editor, &snapshot.text(), &blocks, cx);
                }
            }
        },
    )
    .detach();
}

/// A collapsed block an outline command may move: its subtree's text and where it started.
pub(super) struct Kept {
    subtree: String,
    from: usize,
}

/// The collapsed blocks of the page before an outline command edits it.
pub(super) fn keep(editor: &Editor, page: &Page, cx: &mut App) -> Vec<Kept> {
    let starts: Vec<usize> = collapsed_source(editor, cx)
        .into_iter()
        .map(|range| range.start)
        .collect();
    page.blocks
        .iter()
        .filter(|block| children(block).is_some_and(|range| starts.contains(&range.start)))
        .map(|block| Kept {
            subtree: page.text[block.from..block.subtree_to].to_owned(),
            from: block.from,
        })
        .collect()
}

/// After an outline command: drops folds left over a range that no longer holds a block's
/// children, and collapses again each kept block the edit moved, found by its subtree's text
/// nearest its old place.
pub(super) fn reconcile(
    editor: &mut Editor,
    buffer: &Entity<language::Buffer>,
    kept: Vec<Kept>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let Some(blocks) = crate::markdown_blocks::current_blocks(&snapshot) else {
        return;
    };
    let source = collapsed_source(editor, cx);
    let view = collapsed(editor, cx);
    let stale: Vec<_> = source
        .iter()
        .zip(&view)
        .filter(|(range, _)| !blocks.iter().filter_map(children).any(|it| it == **range))
        .map(|(_, view)| view.clone())
        .collect();
    let starts: Vec<usize> = source
        .iter()
        .zip(&view)
        .filter(|(_, view)| !stale.contains(view))
        .map(|(range, _)| range.start)
        .collect();
    if !stale.is_empty() {
        let inner: Vec<_> = view
            .iter()
            .filter(|fold| {
                !stale.contains(fold)
                    && stale
                        .iter()
                        .any(|outer| outer.start <= fold.start && fold.end <= outer.end)
            })
            .cloned()
            .collect();
        unfold(editor, &stale, cx);
        refold(editor, inner, window, cx);
    }
    let folded_subtree = |block: &Block| text[block.from..block.subtree_to].to_owned();
    let still: Vec<String> = blocks
        .iter()
        .filter(|block| children(block).is_some_and(|range| starts.contains(&range.start)))
        .map(folded_subtree)
        .collect();
    let lost = kept
        .into_iter()
        .filter(|kept| !still.contains(&kept.subtree))
        .filter_map(|kept| {
            blocks
                .iter()
                .filter(|block| {
                    children(block).is_some_and(|range| !starts.contains(&range.start))
                        && text[block.from..block.subtree_to] == kept.subtree
                })
                .min_by_key(|block| block.from.abs_diff(kept.from))
                .and_then(children)
        })
        .filter_map(|range| view_range(editor, buffer, range, cx))
        .collect();
    refold(editor, lost, window, cx);
}
