//! rt's code-block toolbar in Write: while the caret is outside a fenced code block, its opening
//! fence line folds into a toolbar naming the language ("Plain text" when untagged) with Format
//! code, for languages the bundled Prettier formats, and Copy code. Format code reads
//! "Formatting…" while its request runs; Copy code reads "Copied" until the block changes. The
//! caret in the block shows the fence as written.

use editor::{Editor, FoldPlaceholder};
use gpui::{App, Context, SharedString, WeakEntity, prelude::*};
use language::BufferSnapshot;
use multi_buffer::{Anchor, MultiBufferOffset, ToOffset};
use std::{any::TypeId, ops::Range, sync::Arc};

pub const PLAIN: &str = "Plain text";
pub const FORMAT: &str = "Format code";
pub const FORMATTING: &str = "Formatting…";
pub const COPY: &str = "Copy code";
pub const COPIED: &str = "Copied";

/// A fenced code block's opening fence line, which the toolbar replaces.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Found {
    /// From the opening fence characters to the end of their line.
    pub range: Range<usize>,
    /// From the start of the opening fence line to the end of the closing one.
    pub block_range: Range<usize>,
    pub tag: String,
    /// The whole block as written, so "Copied" ends when the block changes.
    pub text: String,
}

/// The toolbar of each closed fenced code block in a page, in text order.
pub(crate) fn found(snapshot: &BufferSnapshot) -> Vec<Found> {
    super::fences(snapshot)
        .into_iter()
        .map(|fence| {
            let open = snapshot.offset_to_point(fence.range.start);
            let line_start = snapshot.point_to_offset(language::Point::new(open.row, 0));
            let line_end = snapshot
                .point_to_offset(language::Point::new(open.row, snapshot.line_len(open.row)));
            Found {
                range: fence.range.start..line_end,
                block_range: line_start..fence.range.end,
                tag: fence.tag,
                text: snapshot.text_for_range(fence.range).collect(),
            }
        })
        .collect()
}

/// A toolbar in the view, in view anchors.
#[derive(Clone)]
pub(crate) struct Spot {
    pub range: Range<Anchor>,
    pub block: Range<Anchor>,
    pub source: String,
    pub tag: String,
    pub text: String,
}

/// What a toolbar shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toolbar {
    pub language: SharedString,
    /// Absent for languages Format code cannot format and in read-only views.
    pub format: Option<&'static str>,
    pub copy: &'static str,
}

impl Toolbar {
    /// The toolbar as one line of text, for tests and the accessibility label.
    pub fn text(&self) -> String {
        [Some(self.language.as_ref()), self.format, Some(self.copy)]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

#[derive(Default)]
pub(crate) struct Toolbars {
    spots: Vec<Spot>,
    /// The start and text of the block last copied in this view.
    copied: Option<(MultiBufferOffset, String)>,
}

impl editor::Addon for Toolbars {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// Shows the toolbars of the current parse.
pub(crate) fn sync(editor: &mut Editor, spots: Vec<Spot>) {
    match editor.addon_mut::<Toolbars>() {
        Some(toolbars) => toolbars.spots = spots,
        None => editor.register_addon(Toolbars {
            spots,
            copied: None,
        }),
    }
}

/// Removes every toolbar, for Source view.
pub(crate) fn clear(editor: &mut Editor) {
    editor.unregister_addon::<Toolbars>();
}

/// Marks the block starting at `start` as copied.
pub(crate) fn copied(editor: &mut Editor, start: MultiBufferOffset, cx: &mut Context<Editor>) {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let Some(toolbars) = editor.addon_mut::<Toolbars>() else {
        return;
    };
    let Some(spot) = toolbars
        .spots
        .iter()
        .find(|spot| spot.range.start.to_offset(&snapshot) == start)
    else {
        return;
    };
    toolbars.copied = Some((start, spot.text.clone()));
}

fn toolbar(editor: &Editor, spot: &Spot, cx: &App) -> Toolbar {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let start = spot.range.start.to_offset(&snapshot);
    let busy = super::formatting_start(editor)
        .is_some_and(|pending| pending.to_offset(&snapshot) == start);
    let copied = editor
        .addon::<Toolbars>()
        .and_then(|toolbars| toolbars.copied.as_ref())
        .is_some_and(|(at, text)| *at == start && *text == spot.text);
    let formattable = !editor.read_only(cx) && quire_format::Parser::for_tag(&spot.tag).is_some();
    Toolbar {
        language: if spot.tag.is_empty() {
            PLAIN.into()
        } else {
            spot.tag.clone().into()
        },
        format: formattable.then_some(if busy { FORMATTING } else { FORMAT }),
        copy: if copied { COPIED } else { COPY },
    }
}

/// The toolbars this view shows, in page order.
#[cfg(test)]
pub(crate) fn toolbars(editor: &Editor, cx: &App) -> Vec<Toolbar> {
    editor
        .addon::<Toolbars>()
        .map(|toolbars| {
            toolbars
                .spots
                .iter()
                .map(|spot| toolbar(editor, spot, cx))
                .collect()
        })
        .unwrap_or_default()
}

/// A toolbar for each block to fold its opening fence line into.
pub(crate) fn headers(
    editor: &Editor,
    host: WeakEntity<Editor>,
    type_tag: TypeId,
    cx: &App,
) -> Vec<(Spot, FoldPlaceholder)> {
    let Some(toolbars) = editor.addon::<Toolbars>() else {
        return Vec::new();
    };
    toolbars
        .spots
        .iter()
        .enumerate()
        .map(|(ordinal, spot)| {
            let shown = toolbar(editor, spot, cx);
            let host = host.clone();
            let placeholder = FoldPlaceholder {
                collapsed_text: Some(shown.text().into()),
                constrain_width: false,
                merge_adjacent: false,
                type_tag: Some(type_tag),
                render: Arc::new(move |_, range, cx| render(&shown, ordinal, &host, range, cx)),
            };
            (spot.clone(), placeholder)
        })
        .collect()
}

fn render(
    shown: &Toolbar,
    ordinal: usize,
    host: &WeakEntity<Editor>,
    range: Range<Anchor>,
    cx: &mut App,
) -> gpui::AnyElement {
    use theme::ActiveTheme;
    let colors = cx.theme().colors();
    let start = range.start;
    let button = |kind: &'static str, label: &'static str| {
        crate::editor_projection::inline_control((kind, ordinal), host.clone())
            .debug_selector(move || format!("{kind}-{ordinal}"))
            .role(gpui::Role::Button)
            .aria_label(label)
            .px_1()
            .rounded_sm()
            .text_color(colors.text_accent)
            .child(label)
    };
    let language = {
        let host = host.clone();
        crate::editor_projection::inline_control(("quire-code-language", ordinal), host.clone())
            .debug_selector(move || format!("quire-code-language-{ordinal}"))
            .aria_label(format!("{} code block", shown.language))
            .cursor_text()
            .text_color(colors.text_muted)
            .child(shown.language.clone())
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                host.update(cx, |editor, cx| {
                    window.focus(&gpui::Focusable::focus_handle(editor, cx), cx);
                    editor.change_selections(Default::default(), window, cx, |selections| {
                        selections.select_anchor_ranges([start..start]);
                    });
                })
                .ok();
            })
    };
    let format = shown.format.map(|label| {
        let host = host.clone();
        let button = button("quire-code-format", label);
        if label == FORMATTING {
            return button.cursor_default().opacity(0.6);
        }
        button
            .cursor_pointer()
            .hover(|style| style.bg(colors.element_hover))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                host.update(cx, |editor, cx| {
                    let at = start.to_offset(&editor.buffer().read(cx).snapshot(cx));
                    super::format_code_at(editor, at, window, cx);
                })
                .ok();
            })
    });
    let copy = {
        let host = host.clone();
        button("quire-code-copy", shown.copy)
            .cursor_pointer()
            .hover(|style| style.bg(colors.element_hover))
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                host.update(cx, |editor, cx| {
                    let at = start.to_offset(&editor.buffer().read(cx).snapshot(cx));
                    super::copy_code_at(editor, at, cx);
                    crate::editor_projection::refold(editor, window, cx);
                })
                .ok();
            })
    };
    gpui::div()
        .debug_selector(move || format!("quire-code-toolbar-{ordinal}"))
        .flex()
        .gap_3()
        .px_2()
        .rounded_sm()
        .bg(colors.element_background)
        .text_sm()
        .child(language)
        .children(format)
        .child(copy)
        .into_any_element()
}
