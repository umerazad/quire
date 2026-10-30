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
    _subscriptions: Vec<Subscription>,
}

#[derive(Clone)]
struct AnchoredLink {
    range: Range<Anchor>,
    data: Arc<crate::markdown_links::Link>,
}
impl Addon for Projection {
    fn to_any(&self) -> &dyn Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
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
    let links = links.into_iter().filter_map(|link| {
        let range = link.range.start.to_offset(&snapshot)..link.range.end.to_offset(&snapshot);
        let current: String = snapshot.text_for_range(range.clone()).collect();
        (current == link.data.source)
            .then(|| Crease::simple(range, link_placeholder(cx.entity().downgrade(), link.data)))
    });
    editor.fold_creases(
        hidden
            .into_iter()
            .map(|range| Crease::simple(range, placeholder.clone()))
            .chain(tasks)
            .chain(links)
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

fn inline_control(
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
        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
}

fn link_placeholder(
    editor: WeakEntity<Editor>,
    link: Arc<crate::markdown_links::Link>,
) -> FoldPlaceholder {
    use ui::LabelCommon;
    let label: gpui::SharedString = link.label.clone().into();
    FoldPlaceholder {
        collapsed_text: Some("⋯".into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<Projection>()),
        render: Arc::new(move |id, range, _| {
            let editor = editor.clone();
            let link = link.clone();
            inline_control(id, editor.clone())
                .debug_selector(|| "quire-web-link".into())
                .role(gpui::Role::Link)
                .aria_label(label.clone())
                .cursor_pointer()
                .child(
                    ui::Label::new(label.clone())
                        .color(ui::Color::Accent)
                        .underline(),
                )
                .on_click(move |_, _, cx| {
                    cx.stop_propagation();
                    editor
                        .update(cx, |editor, cx| {
                            open_web_link(editor, range.clone(), &link.source, cx)
                        })
                        .ok();
                })
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
    let Some(task) = crate::task_marker::read(&current) else {
        return;
    };
    editor.transact(window, cx, |editor, _, cx| {
        editor.edit([(range, task.toggled())], cx)
    });
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
                let content = text_start(block, &snapshot);
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
    intent: ViewEditIntent,
) {
    for (range, inserted) in edits {
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
    let spans = crate::markdown_blocks::syntax_spans(&snapshot, &blocks);
    let Some(links) = crate::markdown_links::links(&snapshot, &blocks) else {
        return;
    };
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
            Some(AnchoredLink {
                range: start..end,
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
                EditorEvent::BufferEdited => refresh(editor, &blocks, window, cx),
                EditorEvent::SelectionsChanged { local: true } => {
                    clamp_carets(editor, &blocks, window, cx)
                }
                _ => {}
                }
            },
        );
        let source_subscription = cx.subscribe_in(&source, window, |editor, source, event: &language::BufferEvent, window, cx| {
            if matches!(event, language::BufferEvent::Reparsed | language::BufferEvent::LanguageChanged(_)) { rebuild(editor, source, window, cx); }
        });
        editor.register_addon(Projection {
            blocks: Arc::default(), links: Vec::new(), parsed: None,
            _subscriptions: vec![editor_subscription, source_subscription],
        });
        apply_spans(editor, &[], window, cx);
        rebuild(editor, &source, window, cx);
        Ok(())
    })
}

pub fn enabled(editor: &Editor) -> bool {
    editor.addon::<Projection>().is_some()
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
