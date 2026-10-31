//! Format code: formats the fenced code block at the caret with the bundled Prettier
//! (`quire_format`), as rt's "Format code" button does. The fence lines stay as they are, the
//! formatted lines keep the closing fence's container prefix (list indentation or quote markers),
//! and the change is one undo step. Formatting runs on the formatter thread; a result for a block
//! that changed in the meantime is discarded, and a newer request in the same view cancels an
//! older one.

use editor::{Addon, Editor};
use gpui::{App, Context, Entity, Global, KeyBinding, Task, Window, actions};
use language::ToOffset as _;
use language::{Buffer, BufferSnapshot};
use multi_buffer::ToOffset as _;
use quire_format::{FormatError, Formatter, Parser};
use std::{any::Any, ops::Range, sync::Arc};

actions!(
    quire_code,
    [
        /// Formats the fenced code block at the caret with the bundled Prettier.
        FormatCode
    ]
);

pub const FORMATTING: &str = "Formatting code…";
pub const NOT_IN_CODE: &str = "Place the caret in a fenced code block, then run Format code.";
pub const UNCLOSED: &str =
    "Close the fenced code block with a matching fence line, then run Format code again.";
pub const CHANGED: &str = "The code block changed while it was being formatted, so the result was discarded. Run Format code again on its current text.";
pub const READ_ONLY: &str =
    "This page is read-only, so its code cannot be formatted. Resolve or reopen the page first.";
pub const SINGLE_PAGE: &str =
    "Format code works in a page's own view. Open the page, then run Format code.";

struct CodeFormatter(Arc<Formatter>);
impl Global for CodeFormatter {}

/// The view's pending request; replacing or dropping it cancels the request.
struct PendingFormat {
    _task: Task<()>,
}

impl Addon for PendingFormat {
    fn to_any(&self) -> &dyn Any {
        self
    }
}

/// Whether the view is waiting for a formatter result.
#[cfg(test)]
fn formatting(editor: &Editor) -> bool {
    editor
        .addon::<PendingFormat>()
        .is_some_and(|pending| !pending._task.is_ready())
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
    /// The body without its container prefix: what Prettier formats.
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
    let node = node.ok_or(NOT_IN_CODE)?;
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

fn format_code(editor: &mut Editor, window: &mut Window, cx: &mut Context<Editor>) {
    let Some(buffer) = editor.buffer().read(cx).as_singleton() else {
        return crate::outline::set_notice(Some(SINGLE_PAGE), cx);
    };
    if editor.read_only(cx) {
        return crate::outline::set_notice(Some(READ_ONLY), cx);
    }
    let head = editor
        .selections
        .newest_anchor()
        .head()
        .to_offset(&editor.buffer().read(cx).snapshot(cx))
        .0;
    let snapshot = buffer.read(cx).snapshot();
    let fence = match fence_at(&snapshot, head) {
        Ok(fence) => fence,
        Err(message) => return crate::outline::set_notice(Some(message), cx),
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
    crate::outline::set_notice(Some(FORMATTING), cx);
    let task = cx.spawn_in(window, async move |editor, cx| {
        let result = request.await;
        editor
            .update_in(cx, |editor, window, cx| {
                let pending = Pending {
                    buffer,
                    anchors,
                    original,
                    fence,
                };
                finish(editor, pending, result, window, cx)
            })
            .ok();
    });
    editor.register_addon(PendingFormat { _task: task });
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
    if crate::outline::notice(cx).as_deref() == Some(FORMATTING) {
        crate::outline::set_notice(None, cx);
    }
    let fence = &pending.fence;
    let body = fence.body.start - fence.range.start..fence.body.end - fence.range.start;
    let old = &pending.original[body.clone()];
    let new = formatted_body(&fence.prefix, &formatted);
    let start = range.start + body.start;
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
