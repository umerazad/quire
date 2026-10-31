//! Format code and Copy code for the fenced code block at the caret or under a Write toolbar
//! (`toolbar`), as rt's code-block buttons do. Format code runs the bundled Prettier
//! (`quire_format`): the fence lines stay as they are, the formatted lines keep the closing
//! fence's container prefix (list indentation or quote markers), and the change is one undo step.
//! Formatting runs on the formatter thread; a result for a block that changed in the meantime is
//! discarded, and a newer request in the same view cancels an older one. Both commands work in a
//! page's own view and in excerpt views that show the whole block.

pub(crate) mod toolbar;

use editor::{Addon, Editor};
use gpui::{App, ClipboardItem, Context, Entity, Global, KeyBinding, Task, Window, actions};
use language::ToOffset as _;
use language::{Buffer, BufferSnapshot};
use multi_buffer::{MultiBufferOffset, MultiBufferSnapshot, ToOffset as _};
use quire_format::{FormatError, Formatter, Parser};
use std::{any::Any, ops::Range, sync::Arc};

actions!(
    quire_code,
    [
        /// Formats the fenced code block at the caret with the bundled Prettier.
        FormatCode,
        /// Copies the code in the fenced code block at the caret, without its fences.
        CopyCode
    ]
);

pub const FORMATTING: &str = "Formatting code…";
pub const NOT_IN_CODE: &str = "Place the caret in a fenced code block, then run Format code.";
pub const UNCLOSED: &str =
    "Close the fenced code block with a matching fence line, then run Format code again.";
pub const CHANGED: &str = "The code block changed while it was being formatted, so the result was discarded. Run Format code again on its current text.";
pub const READ_ONLY: &str =
    "This page is read-only, so its code cannot be formatted. Resolve or reopen the page first.";
pub const PARTLY_SHOWN: &str =
    "Only part of this code block is shown here. Open its page, then run Format code.";
pub const COPY_NOT_IN_CODE: &str = "Place the caret in a fenced code block, then run Copy code.";
pub const COPY_UNCLOSED: &str =
    "Close the fenced code block with a matching fence line, then run Copy code again.";
pub const COPY_PARTLY_SHOWN: &str =
    "Only part of this code block is shown here. Open its page, then run Copy code.";

struct CodeFormatter(Arc<Formatter>);
impl Global for CodeFormatter {}

/// The view's pending request; replacing or dropping it cancels the request.
struct PendingFormat {
    _task: Task<()>,
    /// Where the block being formatted starts in this view.
    start: multi_buffer::Anchor,
    running: bool,
}

impl Addon for PendingFormat {
    fn to_any(&self) -> &dyn Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

/// Whether the view is waiting for a formatter result.
pub(crate) fn formatting(editor: &Editor) -> bool {
    formatting_start(editor).is_some()
}

/// Where the block being formatted in this view starts, while it is being formatted.
fn formatting_start(editor: &Editor) -> Option<multi_buffer::Anchor> {
    editor
        .addon::<PendingFormat>()
        .filter(|pending| pending.running)
        .map(|pending| pending.start)
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        editor
            .register_action(move |_: &FormatCode, window, cx| {
                if let Some(editor) = handle.upgrade() {
                    editor.update(cx, |editor, cx| format_code(editor, window, cx));
                }
            })
            .detach();
        let handle = cx.entity().downgrade();
        editor
            .register_action(move |_: &CopyCode, _, cx| {
                if let Some(editor) = handle.upgrade() {
                    editor.update(cx, |editor, cx| {
                        let head = caret(editor, cx);
                        copy_code_at(editor, head, cx)
                    });
                }
            })
            .detach();
    })
    .detach();
    cx.bind_keys([KeyBinding::new(
        if cfg!(target_os = "macos") {
            "cmd-shift-i"
        } else {
            "ctrl-shift-i"
        },
        FormatCode,
        Some("Editor && !QuireControl"),
    )]);
}

#[cfg(test)]
pub(crate) fn use_formatter(formatter: Formatter, cx: &mut App) {
    cx.set_global(CodeFormatter(Arc::new(formatter)));
}

fn formatter(cx: &mut App) -> Arc<Formatter> {
    if !cx.has_global::<CodeFormatter>() {
        cx.set_global(CodeFormatter(Arc::new(Formatter::default())));
    }
    cx.global::<CodeFormatter>().0.clone()
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Fence {
    /// From the opening fence characters to the end of the closing fence line.
    pub range: Range<usize>,
    /// The lines between the fence lines.
    pub body: Range<usize>,
    pub tag: String,
    /// The closing fence line's text before its fence characters.
    pub prefix: String,
    /// The body without its container prefix: what Prettier formats and Copy code copies.
    pub code: String,
}

/// The fenced code block around `offset` in the page's own Markdown parse.
pub(crate) fn fence_at(snapshot: &BufferSnapshot, offset: usize) -> Result<Fence, &'static str> {
    let layer = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")
        .ok_or(NOT_IN_CODE)?;
    let mut node = layer.node().descendant_for_byte_range(offset, offset);
    while let Some(current) = node {
        if current.kind() == "fenced_code_block" {
            break;
        }
        node = current.parent();
    }
    fence(snapshot, node.ok_or(NOT_IN_CODE)?)
}

/// Every closed fenced code block in the page, in text order.
pub(crate) fn fences(snapshot: &BufferSnapshot) -> Vec<Fence> {
    let Some(layer) = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")
    else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut stack = vec![layer.node()];
    while let Some(node) = stack.pop() {
        if node.kind() == "fenced_code_block" {
            found.extend(fence(snapshot, node).ok());
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        stack.extend(children.into_iter().rev());
    }
    found
}

fn fence(snapshot: &BufferSnapshot, node: tree_sitter::Node) -> Result<Fence, &'static str> {
    let mut cursor = node.walk();
    let children: Vec<_> = node.named_children(&mut cursor).collect();
    let delimiters: Vec<_> = children
        .iter()
        .filter(|child| child.kind() == "fenced_code_block_delimiter")
        .collect();
    let [open, .., close] = delimiters.as_slice() else {
        return Err(UNCLOSED);
    };
    let text = |range: Range<usize>| snapshot.text_for_range(range).collect::<String>();
    let tag = children
        .iter()
        .find(|child| child.kind() == "info_string")
        .and_then(|info| {
            let mut cursor = info.walk();
            let language = info
                .named_children(&mut cursor)
                .find(|child| child.kind() == "language");
            language.map(|language| text(language.byte_range()))
        })
        .unwrap_or_default();
    let open_row = snapshot.offset_to_point(open.start_byte()).row;
    let close_row = snapshot.offset_to_point(close.start_byte()).row;
    let close_line = snapshot.point_to_offset(language::Point::new(close_row, 0));
    let body = snapshot
        .point_to_offset(language::Point::new(open_row + 1, 0))
        .min(close_line)..close_line;
    let end = snapshot.point_to_offset(language::Point::new(
        close_row,
        snapshot.line_len(close_row),
    ));
    let prefix = text(close_line..close.start_byte());
    let code = text(body.clone())
        .split_inclusive('\n')
        .map(|line| without_prefix(line, &prefix))
        .collect();
    Ok(Fence {
        range: open.start_byte()..end,
        body,
        tag,
        prefix,
        code,
    })
}

/// `line` without the longest start it shares with `prefix`, so a blank line or a bare `>`
/// inside a quoted block loses what it has of the container prefix.
fn without_prefix<'a>(line: &'a str, prefix: &str) -> &'a str {
    let shared = line
        .char_indices()
        .zip(prefix.chars())
        .take_while(|((_, a), b)| a == b)
        .last()
        .map_or(0, |((index, c), _)| index + c.len_utf8());
    &line[shared..]
}

/// The new body: Prettier's output without trailing whitespace, each line behind the closing
/// fence's prefix, as rt's `replaceFencedBody` writes it.
pub(crate) fn formatted_body(prefix: &str, formatted: &str) -> String {
    formatted
        .trim_end()
        .split('\n')
        .map(|line| format!("{prefix}{line}\n"))
        .collect()
}

fn caret(editor: &Editor, cx: &App) -> MultiBufferOffset {
    editor
        .selections
        .newest_anchor()
        .head()
        .to_offset(&editor.buffer().read(cx).snapshot(cx))
}

/// Where `range` of `buffer` shows in `view`, when the view shows all of it in one piece.
fn shown_range(
    view: &MultiBufferSnapshot,
    buffer: &BufferSnapshot,
    range: &Range<usize>,
) -> Option<Range<MultiBufferOffset>> {
    let offset = |at: usize| {
        view.anchor_in_buffer(buffer.anchor_before(at))
            .map(|anchor| anchor.to_offset(view))
    };
    let shown = offset(range.start)?..offset(range.end)?;
    let whole = shown.end.0.checked_sub(shown.start.0) == Some(range.len())
        && view.contains_str_at(
            shown.start,
            &buffer.text_for_range(range.clone()).collect::<String>(),
        );
    whole.then_some(shown)
}

/// The block around `at` in the view, its page, and where it starts in the view.
struct Located {
    buffer: Entity<Buffer>,
    snapshot: BufferSnapshot,
    fence: Fence,
    start: MultiBufferOffset,
}

enum Missing {
    NotInCode,
    Unclosed,
    PartlyShown,
}

fn locate(editor: &Editor, at: MultiBufferOffset, cx: &App) -> Result<Located, Missing> {
    let multi = editor.buffer().read(cx);
    let view = multi.snapshot(cx);
    let (snapshot, offset) = view.point_to_buffer_offset(at).ok_or(Missing::NotInCode)?;
    let buffer = multi
        .buffer(snapshot.remote_id())
        .ok_or(Missing::NotInCode)?;
    let snapshot = snapshot.clone();
    let fence = fence_at(&snapshot, offset.0).map_err(|message| {
        if message == UNCLOSED {
            Missing::Unclosed
        } else {
            Missing::NotInCode
        }
    })?;
    let start = shown_range(&view, &snapshot, &fence.range)
        .ok_or(Missing::PartlyShown)?
        .start;
    Ok(Located {
        buffer,
        snapshot,
        fence,
        start,
    })
}

/// The block at the caret, for menus that offer Format code and Copy code.
pub(crate) fn fence_at_caret(editor: &Editor, cx: &App) -> Option<Fence> {
    locate(editor, caret(editor, cx), cx)
        .ok()
        .map(|located| located.fence)
}

/// Copies the code of the block around `at`: its body without fences or container prefixes,
/// as rt's Copy code button copies it.
pub(crate) fn copy_code_at(editor: &mut Editor, at: MultiBufferOffset, cx: &mut Context<Editor>) {
    match locate(editor, at, cx) {
        Ok(located) => {
            cx.write_to_clipboard(ClipboardItem::new_string(located.fence.code));
            toolbar::copied(editor, located.start, cx);
        }
        Err(missing) => crate::outline::set_notice(
            Some(match missing {
                Missing::NotInCode => COPY_NOT_IN_CODE,
                Missing::Unclosed => COPY_UNCLOSED,
                Missing::PartlyShown => COPY_PARTLY_SHOWN,
            }),
            cx,
        ),
    }
}

/// Formats the block at the caret.
fn format_code(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    let head = caret(editor, cx);
    format_code_at(editor, head, window, cx)
}

/// Formats the block around `at` in the view.
pub(crate) fn format_code_at(
    editor: &mut Editor,
    at: MultiBufferOffset,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if editor.read_only(cx) {
        return crate::outline::set_notice(Some(READ_ONLY), cx);
    }
    let Located {
        buffer,
        snapshot,
        fence,
        start,
    } = match locate(editor, at, cx) {
        Ok(located) => located,
        Err(missing) => {
            return crate::outline::set_notice(
                Some(match missing {
                    Missing::NotInCode => NOT_IN_CODE,
                    Missing::Unclosed => UNCLOSED,
                    Missing::PartlyShown => PARTLY_SHOWN,
                }),
                cx,
            );
        }
    };
    let Some(parser) = Parser::for_tag(&fence.tag) else {
        let error = FormatError::Unsupported(fence.tag);
        return crate::outline::show_notice(error.to_string(), cx);
    };
    if fence.code.trim().is_empty() {
        return crate::outline::set_notice(None, cx);
    }
    let anchors = snapshot.anchor_before(fence.range.start)..snapshot.anchor_after(fence.range.end);
    let original: String = snapshot.text_for_range(fence.range.clone()).collect();
    let request = formatter(cx).format(fence.code.clone(), parser);
    let start = editor.buffer().read(cx).snapshot(cx).anchor_before(start);
    let task = cx.spawn_in(window, async move |editor, cx| {
        let result = request.await;
        editor
            .update_in(cx, |editor, window, cx| {
                if let Some(pending) = editor.addon_mut::<PendingFormat>() {
                    pending.running = false;
                }
                let pending = Pending {
                    buffer,
                    anchors,
                    original,
                    fence,
                };
                finish(editor, pending, result, window, cx);
                crate::editor_projection::refold(editor, window, cx);
            })
            .ok();
    });
    editor.register_addon(PendingFormat {
        _task: task,
        start,
        running: true,
    });
    crate::outline::set_notice(Some(FORMATTING), cx);
    crate::editor_projection::refold(editor, window, cx);
}

struct Pending {
    buffer: Entity<Buffer>,
    anchors: Range<text::Anchor>,
    original: String,
    fence: Fence,
}

fn finish(
    editor: &mut Editor,
    pending: Pending,
    result: Result<String, FormatError>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let formatted = match result {
        Ok(formatted) => formatted,
        Err(FormatError::Cancelled) => return,
        Err(error) => return crate::outline::show_notice(error.to_string(), cx),
    };
    let snapshot = pending.buffer.read(cx).snapshot();
    let range =
        pending.anchors.start.to_offset(&snapshot)..pending.anchors.end.to_offset(&snapshot);
    if snapshot.text_for_range(range.clone()).collect::<String>() != pending.original {
        return crate::outline::set_notice(Some(CHANGED), cx);
    }
    if editor.read_only(cx) {
        return crate::outline::set_notice(Some(READ_ONLY), cx);
    }
    let view = editor.buffer().read(cx).snapshot(cx);
    let Some(shown) = shown_range(&view, &snapshot, &range) else {
        return crate::outline::set_notice(Some(PARTLY_SHOWN), cx);
    };
    if crate::outline::notice(cx).as_deref() == Some(FORMATTING) {
        crate::outline::set_notice(None, cx);
    }
    let fence = &pending.fence;
    let body = fence.body.start - fence.range.start..fence.body.end - fence.range.start;
    let old = &pending.original[body.clone()];
    let new = formatted_body(&fence.prefix, &formatted);
    let start = shown.start.0 + body.start;
    let edits: Vec<_> = language::text_diff(old, &new)
        .into_iter()
        .map(|(range, text)| (range.start + start..range.end + start, text.to_string()))
        .collect();
    if edits.is_empty() {
        return;
    }
    let buffer = editor.buffer().clone();
    editor.finalize_last_transaction(cx);
    editor.transact(window, cx, |_, _, cx| {
        crate::outline::structural_edit(&buffer, &edits, cx)
    });
    editor.finalize_last_transaction(cx);
}

#[cfg(test)]
#[path = "code_format_tests.rs"]
mod tests;
