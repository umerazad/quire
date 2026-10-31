//! What a synthesis sends with its instruction: the linked references to a page, or the blocks
//! carrying a tag, read from the query index with their nested blocks. Questions read everything
//! else through the notebook tools.

use super::answer::Scope;
use crate::query_index::ask;
use quire_core::ai::ask::{Excerpt, Gathered};
use quire_store::index::{IndexClient, Outline};

/// The open pages and the store, for reading a whole page with its unsaved edits.
pub type Writers = (
    Vec<gpui::Entity<crate::notebook_session::OpenPage>>,
    crate::notebook_backend::NotebookBackend,
);

/// One block with the nested blocks its outline lists, as an outline under it.
fn excerpt(title: &str, id: Option<String>, text: &str, outline: &Outline) -> Excerpt {
    let children = outline
        .children
        .iter()
        .map(|(depth, text)| format!("{}- {text}", "  ".repeat(*depth as usize)));
    Excerpt {
        title: title.to_owned(),
        id,
        text: std::iter::once(text.to_owned())
            .chain(children)
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

pub async fn gather(index: IndexClient, scope: &Scope) -> Result<Gathered, String> {
    match scope {
        Scope::References(title) => references(index, title).await,
        Scope::Tag(tag) => tagged(index, tag.clone()).await,
    }
}

/// The linked references to the page titled `title`, each with its nested blocks.
pub async fn references(index: IndexClient, title: &str) -> Result<Gathered, String> {
    let Some(page) = ask(index.find_page(title.to_owned())).await? else {
        return Ok(Gathered::bounded([]));
    };
    let blocks = ask(index.linked_references(page.page, false)).await?;
    Ok(Gathered::bounded(blocks.into_iter().map(|block| {
        excerpt(
            &block.source.title,
            block.block_id.clone(),
            &block.snippet,
            &block.outline,
        )
    })))
}

/// The blocks carrying `tag`, in the tag page's order, each with its nested blocks.
pub async fn tagged(index: IndexClient, tag: String) -> Result<Gathered, String> {
    let (_, blocks) = ask(index.with_generation(move |index| {
        let blocks = index.tagged_blocks(&tag)?;
        let at: Vec<_> = blocks
            .iter()
            .map(|block| (block.source.revision.clone(), block.tag.from))
            .collect();
        Ok(blocks
            .into_iter()
            .zip(index.outlines(&at)?)
            .collect::<Vec<_>>())
    }))
    .await?;
    Ok(Gathered::bounded(blocks.into_iter().map(
        |(block, outline)| {
            excerpt(
                &block.source.title,
                block.tag.block_id,
                &block.tag.snippet,
                &outline,
            )
        },
    )))
}
