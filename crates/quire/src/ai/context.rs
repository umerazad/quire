//! What a synthesis sends with its instruction: the linked references to a page, read from the
//! query index with their nested blocks. Questions read everything else through the notebook
//! tools.

use crate::query_index::ask;
use quire_core::ai::ask::{Excerpt, Gathered};
use quire_store::index::IndexClient;

/// The open pages and the store, for reading a whole page with its unsaved edits.
pub type Writers = (
    Vec<gpui::Entity<crate::notebook_session::OpenPage>>,
    crate::notebook_backend::NotebookBackend,
);

/// The linked references to the page titled `title`, each with its nested blocks.
pub async fn references(index: IndexClient, title: &str) -> Result<Gathered, String> {
    let Some(page) = ask(index.find_page(title.to_owned())).await? else {
        return Ok(Gathered::bounded([]));
    };
    let blocks = ask(index.linked_references(page.page, false)).await?;
    Ok(Gathered::bounded(blocks.into_iter().map(|block| {
        let children = block
            .outline
            .children
            .iter()
            .map(|(depth, text)| format!("{}- {text}", "  ".repeat(*depth as usize)));
        Excerpt {
            title: block.source.title.clone(),
            id: block.block_id.clone(),
            text: std::iter::once(block.snippet.clone())
                .chain(children)
                .collect::<Vec<_>>()
                .join("\n"),
        }
    })))
}
