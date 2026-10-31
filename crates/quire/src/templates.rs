//! Templates in the notebook editor. `Templates.md` at the notebook root holds them; the window
//! opens it from a pinned sidebar entry and shows the guide beside it, and typing `jj` at the
//! start of an empty block lists them through the editor's completion menu. Parsing and
//! expansion live in `quire_core::templates`; this module feeds them the editor's Markdown
//! analysis and applies the result to the shared page buffer.

use gpui::{App, Entity};
use language::{Buffer, BufferSnapshot};
use quire_core::templates::{self as core, Block, Definition};

pub mod guide;

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
