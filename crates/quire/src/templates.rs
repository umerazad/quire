//! Templates in the notebook editor. `Templates.md` at the notebook root holds them; the window
//! opens it from a pinned sidebar entry and shows the guide beside it, and typing `jj` at the
//! start of an empty block lists them through the editor's completion menu. Parsing and
//! expansion live in `quire_core::templates`; this module feeds them the editor's Markdown
//! analysis and applies the result to the shared page buffer.

use gpui::{App, Entity};
use language::{Buffer, BufferSnapshot};
use quire_core::templates::{self as core, Block, Definition, Parsed};

pub mod guide;
pub(crate) mod insert;
pub mod prompt;

/// The core view of the editor's blocks.
fn core_blocks(snapshot: &BufferSnapshot, blocks: &[crate::markdown_blocks::Block]) -> Vec<Block> {
    blocks
        .iter()
        .map(|block| Block {
            key: block.key.clone(),
            id: block.id.clone(),
            list: block.kind == "list",
            from: block.from,
            content_from: block.content_from,
            content_to: block.content_to,
            subtree_to: block.subtree_to,
            parent: block.parent.clone(),
            text: crate::markdown_blocks::block_text(snapshot, block),
        })
        .collect()
}

/// The templates a page holds, from its current text. None while the text cannot be parsed.
pub fn definitions(path: &str, snapshot: &BufferSnapshot) -> Option<Vec<Definition>> {
    let blocks = crate::markdown_blocks::current_blocks(snapshot)?;
    Some(core::templates_in_page(
        path,
        &snapshot.text(),
        &core_blocks(snapshot, &blocks),
    ))
}

/// Appends a template definition to the end of the Templates page.
pub fn append(buffer: &Entity<Buffer>, definition: &str, cx: &mut App) {
    buffer.update(cx, |buffer, cx| {
        let text = buffer.text();
        let gap = if !text.is_empty() && !text.ends_with('\n') {
            "\n"
        } else {
            ""
        };
        let end = text.len();
        buffer.edit([(end..end, format!("{gap}{definition}"))], None, cx);
    });
}

/// Blocks and references of text that is not in any page, parsed off the main thread.
pub(crate) async fn parse(text: &str, cx: &mut gpui::AsyncWindowContext) -> Option<Parsed> {
    let snapshot = cx
        .update(|_, cx| crate::markdown::detached_snapshot(text, cx))
        .ok()?
        .await;
    let blocks = crate::markdown_blocks::blocks(&snapshot)?;
    Some(Parsed {
        blocks: core_blocks(&snapshot, &blocks),
        references: crate::page_analysis::references(&snapshot)?,
    })
}

/// The page's shared buffer once it has loaded.
pub(crate) async fn loaded(
    page: Entity<crate::notebook_session::OpenPage>,
    cx: &mut gpui::AsyncWindowContext,
) -> Option<Entity<Buffer>> {
    let (send, receive) = futures::channel::oneshot::channel();
    cx.update(|_, cx| {
        crate::references::when_loaded(
            page,
            move |buffer, _| {
                send.send(buffer).ok();
            },
            cx,
        )
    })
    .ok()?;
    receive.await.ok()
}
