use editor::{Addon, Editor, EditorEvent, FoldPlaceholder, display_map::Crease};
use gpui::{App, Context, Entity, Subscription, Window};
use multi_buffer::{Anchor, MultiBufferOffset, MultiBufferSnapshot, ToOffset, ViewEdit};
use std::{
    any::{Any, TypeId},
    ops::Range,
    sync::Arc,
};

pub struct BlockSyntax {
    pub marker: Range<MultiBufferOffset>,
    pub id: Option<Range<MultiBufferOffset>>,
}

struct AnchoredBlock {
    marker: Range<Anchor>,
    id: Option<Range<Anchor>>,
}

struct Projection {
    _subscription: Subscription,
}
impl Addon for Projection {
    fn to_any(&self) -> &dyn Any {
        self
    }
}

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
    let content = block.marker.end.to_offset(snapshot);
    let gap = snapshot
        .reversed_chars_at(start)
        .take_while(|c| matches!(c, ' ' | '\t'))
        .count();
    start.0 -= gap.min(start.0.saturating_sub(content.0));
    Some(start..end)
}

fn refresh(
    editor: &mut Editor,
    blocks: &[AnchoredBlock],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let mut hidden = Vec::new();
    for block in blocks {
        let marker = block.marker.start.to_offset(&snapshot)..block.marker.end.to_offset(&snapshot);
        if !marker.is_empty() {
            hidden.push(marker);
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
    editor.fold_creases(
        hidden
            .into_iter()
            .map(|range| Crease::simple(range, placeholder.clone()))
            .collect(),
        false,
        window,
        cx,
    );
}

fn clamp_carets(
    editor: &mut Editor,
    blocks: &[AnchoredBlock],
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let mut changed = false;
    let selections = editor
        .selections
        .disjoint_anchors_arc()
        .iter()
        .map(|selection| {
            let mut selection = selection.clone().map(|anchor| anchor.to_offset(&snapshot));
            if selection.start != selection.end {
                return selection;
            }
            let mut position = selection.start;
            for block in blocks {
                let start = block.marker.start.to_offset(&snapshot);
                let content = block.marker.end.to_offset(&snapshot);
                if position >= start && position < content {
                    position = content;
                }
                if let Some(id) = id_range(block, &snapshot)
                    && position > id.start
                    && position <= id.end
                {
                    position = id.start;
                }
            }
            changed |= position != selection.start;
            selection.start = position;
            selection.end = position;
            selection
        })
        .collect::<Vec<_>>();
    if changed {
        editor.change_selections(editor::SelectionEffects::no_scroll(), window, cx, |s| {
            s.select(selections)
        });
    }
}

fn filter_edits(
    edits: &mut [ViewEdit],
    protected: &[AnchoredBlock],
    snapshot: &MultiBufferSnapshot,
) {
    for (range, inserted) in edits {
        for block in protected.iter() {
            let Some(id) = id_range(block, snapshot) else {
                continue;
            };
            let content_start = block.marker.end.to_offset(snapshot);
            let id_start = id.start;
            let id_end = id.end;
            if !inserted.is_empty() && range.start >= id_start && range.end <= id_end {
                *range = id_start..id_start;
            }
            if inserted.is_empty() && range.start == content_start && range.end > id_end {
                range.start = block.marker.start.to_offset(snapshot);
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

/// Applies valid UTF-8 view coordinates supplied by the semantic span producer.
pub fn show(editor: &Entity<Editor>, blocks: &[BlockSyntax], window: &mut Window, cx: &mut App) {
    editor.update(cx, |editor, cx| {
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
            })
            .collect();
        let protected = blocks.clone();
        buffer.update(cx, |buffer, _| {
            buffer.set_edit_filter(Some(Arc::new(move |edits, snapshot| {
                filter_edits(edits, &protected, snapshot)
            })))
        });
        refresh(editor, &blocks, window, cx);
        let subscription = cx.subscribe_in(
            &cx.entity(),
            window,
            move |editor, _, event: &EditorEvent, window, cx| match event {
                EditorEvent::BufferEdited => refresh(editor, &blocks, window, cx),
                EditorEvent::SelectionsChanged { local: true } => {
                    clamp_carets(editor, &blocks, window, cx)
                }
                _ => {}
            },
        );
        editor.register_addon(Projection {
            _subscription: subscription,
        });
    });
}
