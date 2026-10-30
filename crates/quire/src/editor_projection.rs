use editor::{Addon, Editor, EditorEvent, FoldPlaceholder, display_map::Crease};
use gpui::{App, Context, Entity, Focusable, Subscription, WeakEntity, Window, prelude::*};
use multi_buffer::{
    Anchor, MultiBufferOffset, MultiBufferRow, MultiBufferSnapshot, ToOffset, ViewEdit,
    ViewEditIntent,
};
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
    blocks: Arc<[AnchoredBlock]>,
    links: Vec<AnchoredLink>,
    parsed: Option<(text::BufferSnapshot, usize)>,
    heads: Vec<Anchor>,
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct AnchoredLink {
    range: Range<Anchor>,
    label: Range<Anchor>,
    block: Range<Anchor>,
    id: u64,
    data: Arc<crate::markdown_links::Link>,
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
        self.links[start..]
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

fn rendered_link(
    editor: &Editor,
    link: &AnchoredLink,
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
            link.block.start.to_offset(snapshot) <= head
                && head <= link.block.end.to_offset(snapshot)
        }) {
            return false;
        }
    }
    let start = link.range.start.to_offset(snapshot);
    let end = link.range.end.to_offset(snapshot);
    end.0.saturating_sub(start.0) == link.data.source.len()
        && snapshot.contains_str_at(start, &link.data.source)
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
        .take_while(|c| *c != '\n')
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
    let mut tasks = Vec::new();
    for block in blocks {
        let marker = block.marker.start.to_offset(&snapshot)..block.marker.end.to_offset(&snapshot);
        if !marker.is_empty() {
            let content: String = snapshot
                .chars_at(marker.end)
                .take_while(|c| *c != '\n')
                .take(crate::task_marker::PREFIX_CHAR_LIMIT)
                .collect();
            if let Some(task) = crate::task_marker::read(&content) {
                let range = marker.end..MultiBufferOffset(marker.end.0 + task.source.len());
                tasks.push(Crease::simple(
                    range,
                    checkbox_placeholder(cx.entity().downgrade(), task.source.into(), task.checked),
                ));
            }
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
    let links = editor
        .addon::<Projection>()
        .map(|projection| projection.links.clone())
        .unwrap_or_default();
    let mut labels = Vec::new();
    for link in links.iter() {
        labels.push(link.label.clone());
        if rendered_link(editor, link, &snapshot, window, cx) {
            hidden
                .push(link.range.start.to_offset(&snapshot)..link.label.start.to_offset(&snapshot));
            hidden.push(link.label.end.to_offset(&snapshot)..link.range.end.to_offset(&snapshot));
        }
    }
    let key = editor::HighlightKey::NavigationOverlay(editor::NavigationOverlayKey::unique::<
        Projection,
    >());
    editor.clear_highlights(key, cx);
    if !labels.is_empty() {
        use theme::ActiveTheme;
        editor.highlight_text(
            key,
            labels,
            gpui::HighlightStyle {
                color: Some(cx.theme().colors().link_text_hover),
                underline: Some(gpui::UnderlineStyle {
                    thickness: gpui::px(1.),
                    ..Default::default()
                }),
                ..Default::default()
            },
            cx,
        );
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

fn open_web_link(editor: &Editor, range: Range<Anchor>, expected: &str, cx: &mut App) {
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
    if current == expected {
        cx.open_url(&link.data.url);
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

fn inline_control(
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
            let editor = editor.clone();
            let source_range = link.range.clone();
            let data = link.data.clone();
            let fragment = if index == 0 {
                focusable_control(index, editor.clone())
                    .role(gpui::Role::Link)
                    .aria_label(link.data.label.clone())
            } else {
                gpui::div().id(index)
            };
            fragment
                .absolute()
                .left(local.x)
                .top(local.y)
                .w(bounds.size.width)
                .h(bounds.size.height)
                .cursor_pointer()
                .when(index == 0, |div| {
                    div.debug_selector(|| "quire-web-link-first-fragment".into())
                })
                .when(index + 1 == layout.fragments.len(), |div| {
                    div.debug_selector(|| "quire-web-link-last-fragment".into())
                })
                .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                })
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    editor
                        .update(cx, |editor, cx| {
                            open_web_link(editor, source_range.clone(), &data.source, cx)
                        })
                        .ok();
                })
        }))
        .into_any_element()
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

fn checkbox_placeholder(
    editor: WeakEntity<Editor>,
    source: String,
    checked: bool,
) -> FoldPlaceholder {
    FoldPlaceholder {
        collapsed_text: Some(if checked { "☑" } else { "☐" }.into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        render: Arc::new(move |id, range, _| {
            let editor = editor.clone();
            let source = source.clone();
            inline_control(id, editor.clone())
                .debug_selector(|| "quire-task-checkbox".into())
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
                .child(ui::Checkbox::new("task-marker", checked.into()).visualization_only(true))
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
    apply_blocks(editor, &snapshot, &blocks, links, window, cx);
}

fn apply_blocks(
    editor: &mut Editor,
    snapshot: &language::BufferSnapshot,
    blocks: &[crate::markdown_blocks::Block],
    links: Vec<crate::markdown_links::Link>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let spans = crate::markdown_blocks::syntax_spans(snapshot, blocks);
    let view = editor.buffer().read(cx).snapshot(cx);
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
            let mut hasher = std::hash::DefaultHasher::new();
            std::hash::Hash::hash(&(start, end), &mut hasher);
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
            Some(BlockSyntax { marker, id })
        })
        .collect();
    apply_spans(editor, &spans, window, cx);
    editor
        .addon_mut::<Projection>()
        .expect("installed projection")
        .parsed = Some((snapshot.text.clone(), snapshot.syntax_update_count()));
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
            if let Some(blocks) = editor.addon::<Projection>().map(|projection| projection.blocks.clone()) {
                refresh(editor, &blocks, window, cx);
            }
        });
        editor.register_addon(Projection {
            blocks: Arc::default(), links: Vec::new(), parsed: None, heads: Vec::new(),
            _subscriptions: vec![editor_subscription, source_subscription, theme_subscription],
        });
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
    apply_spans(editor, &[], window, cx);
    if let Some(blocks) = blocks {
        let snapshot = source.read(cx).snapshot();
        let links = crate::markdown_links::links(&snapshot, blocks).unwrap_or_default();
        apply_blocks(editor, &snapshot, blocks, links, window, cx);
    }
}

pub fn unbind(editor: &Entity<Editor>, window: &mut Window, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        if !enabled(editor) {
            return;
        }
        editor.unregister_addon::<Projection>();
        editor
            .buffer()
            .update(cx, |buffer, _| buffer.set_edit_filter(None));
        refresh(editor, &[], window, cx);
    });
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
            .update(cx, |editor, _, cx| {
                let snapshot = editor.buffer().read(cx).snapshot(cx);
                assert_eq!(
                    snapshot
                        .text_for_range(link.range.clone())
                        .collect::<String>(),
                    link.data.source
                );
                open_web_link(editor, link.range.clone(), &link.data.source, cx);
            })
            .unwrap();
        assert!(cx.opened_url().is_none());
        cx.executor().run_until_parked();
        window
            .update(cx, |editor, _, cx| {
                open_web_link(editor, link.range.clone(), &link.data.source, cx)
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
