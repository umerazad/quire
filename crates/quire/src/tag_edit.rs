//! Bulk tag rename and delete across the notebook: plan from each page's current text, preview,
//! then apply as one all-or-nothing save through `page_writes`.

use crate::{
    notebook_session::OpenPage,
    page_writes::{self, PageWrite},
    query_index::ask,
};
use gpui::{App, Entity, Task};
use quire_core::{
    source_text::SourceText,
    tag_edits::{Retag, TagChange, check_page_count},
};
use quire_store::{index::IndexClient, worker::StorageClient};

pub const STALE_PREVIEW: &str =
    "A page changed after the preview. Preview again before applying it.";

pub struct PlannedPage {
    pub changes: Vec<TagChange>,
    write: PageWrite,
}

impl PlannedPage {
    pub fn path(&self) -> &str {
        &self.write.path
    }
}

pub struct TagEditPlan {
    pub pages: Vec<PlannedPage>,
}

/// Plans the edit over every page whose tags it touches, refusing more than one apply may write.
pub fn plan(
    retag: Retag,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<TagEditPlan, String>> {
    cx.spawn(async move |cx| {
        let candidates = ask(index.tag_edit_pages(retag.clone())).await?;
        let mut pages = Vec::new();
        for candidate in candidates {
            let id = candidate.page.to_lossy_string();
            let Some(source) = page_writes::read(id, &open_pages, &storage, cx).await? else {
                continue;
            };
            let (original, normalized) = SourceText::prepare(source.stored.clone());
            let work = cx.update(|cx| crate::page_index::edit_tags(normalized, retag.clone(), cx));
            let Some(edit) = work.await else {
                continue;
            };
            let text = original.apply(edit.edits).map_err(|error| {
                format!(
                    "Could not rewrite tags in {}: {error}. Preview again.",
                    source.path
                )
            })?;
            pages.push(PlannedPage {
                changes: edit.changes,
                write: source.write(text),
            });
        }
        check_page_count(pages.len())?;
        Ok(TagEditPlan { pages })
    })
}

/// Writes every planned page or none, refusing when any page changed after the preview.
pub fn apply(
    plan: TagEditPlan,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    let writes = plan.pages.into_iter().map(|page| page.write).collect();
    page_writes::apply(writes, storage, STALE_PREVIEW, cx)
}

#[cfg(test)]
mod tests;
