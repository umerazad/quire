//! Deleting and renaming pages. A deletion plans what goes and how many links break, then
//! writes every tombstone or none; a rename rewrites every link that follows the page and writes
//! every page or none. Both go through `page_writes`.

use crate::{
    notebook_session::OpenPage,
    page_writes::{self, PageSource, PageWrite},
    query_index::ask,
};
use gpui::{App, AppContext as _, Entity, Task};
use quire_core::{
    relocate::{MISSING_SOURCE, Relocation, relocate_page, rename_target},
    source_text::SourceText,
};
use quire_store::{
    index::{DeletionTarget, IndexClient},
    worker::StorageClient,
};
use std::collections::{BTreeSet, HashMap};

pub const STALE_DELETION: &str =
    "A page changed after you chose to delete it. Nothing was deleted; try again.";

/// Paths listed by name in a folder deletion before "And N more."
pub const LISTED_PAGES: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Page(String),
    Folder(String),
}

pub struct DeletionPlan {
    pub target: Target,
    /// The page's title, for a page.
    pub title: String,
    /// Each page that goes, by identity and path, ordered by path.
    pub pages: Vec<(String, String)>,
    pub broken_links: usize,
    sources: Vec<PageSource>,
}

fn count(number: usize, one: &str, many: &str) -> String {
    match number {
        1 => format!("1 {one}"),
        number => format!("{number} {many}"),
    }
}

impl DeletionPlan {
    pub fn folder(&self) -> Option<&str> {
        match &self.target {
            Target::Folder(folder) => Some(folder),
            Target::Page(_) => None,
        }
    }

    pub fn heading(&self) -> String {
        match self.folder() {
            Some(folder) => format!(
                "Delete {folder} with its {}?",
                count(self.pages.len(), "page", "pages")
            ),
            None => format!("Delete {}?", self.title),
        }
    }

    pub fn detail(&self) -> String {
        let links = match self.broken_links {
            0 => "No other page links here.".to_owned(),
            links => format!(
                "{} from other pages will point to a missing page.",
                count(links, "link", "links")
            ),
        };
        format!("{links} Every computer on this notebook removes it too.")
    }

    /// The paths a folder deletion names, and how many more it does not.
    pub fn listed(&self) -> (Vec<&str>, usize) {
        if self.folder().is_none() {
            return (Vec::new(), 0);
        }
        let shown = self
            .pages
            .iter()
            .take(LISTED_PAGES)
            .map(|(_, path)| path.as_str())
            .collect();
        (shown, self.pages.len().saturating_sub(LISTED_PAGES))
    }
}

/// Reads what a deletion takes from the index and each page's current text, open or stored.
pub fn plan_deletion(
    target: Target,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<DeletionPlan, String>> {
    cx.spawn(async move |cx| {
        let query = match &target {
            Target::Page(page) => DeletionTarget::Page(page.as_str().into()),
            Target::Folder(folder) => DeletionTarget::Folder(folder.clone()),
        };
        let deletion = ask(index.deletion(query)).await?;
        let title = deletion
            .pages
            .first()
            .map(|page| page.title.clone())
            .unwrap_or_default();
        let mut pages = Vec::new();
        let mut sources = Vec::new();
        for listed in deletion.pages {
            let id = listed.page.to_lossy_string();
            if let Some(source) = page_writes::read(id.clone(), &open_pages, &storage, cx).await? {
                pages.push((id, source.path.clone()));
                sources.push(source);
            }
        }
        if pages.is_empty() {
            return Err(match target {
                Target::Page(_) => {
                    "This page has no saved version to delete. Close its tab instead.".to_owned()
                }
                Target::Folder(folder) => {
                    format!("{folder} has no pages to delete. Choose another folder.")
                }
            });
        }
        Ok(DeletionPlan {
            target,
            title,
            pages,
            broken_links: deletion.broken_links,
            sources,
        })
    })
}

/// Deletes every planned page or none. Returns the deleted page identities.
pub fn delete(
    plan: DeletionPlan,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<Vec<String>, String>> {
    let folder = plan.folder().map(str::to_owned);
    let writes = plan
        .sources
        .into_iter()
        .map(|source| {
            let text = source.stored.as_str().into();
            source.write(text)
        })
        .collect();
    let ids: Vec<String> = plan.pages.into_iter().map(|(id, _)| id).collect();
    let work = page_writes::delete(writes, folder, storage, STALE_DELETION, cx);
    cx.spawn(async move |_| work.await.map(|_| ids))
}

pub const STALE_RENAME: &str =
    "A page changed while it was being renamed. Nothing was renamed; rename it again.";

/// A planned rename: the writes of the renamed page and every page whose links follow it.
pub struct RenamePlan {
    pub target: String,
    writes: Vec<PageWrite>,
}

/// Plans renaming `page` to `name` in its folder, as the reference `renamePage` does: the file
/// name and title change together, every link to the page follows, and a front matter `title`
/// takes the new name. None when the name is unchanged or empty.
pub fn plan_rename(
    page: String,
    name: String,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<Option<RenamePlan>, String>> {
    cx.spawn(async move |cx| {
        let Some(source) = page_writes::read(page.clone(), &open_pages, &storage, cx).await? else {
            return Err(MISSING_SOURCE.into());
        };
        let Some(target) = rename_target(&source.path, &name)? else {
            return Ok(None);
        };
        let Some(facts) = ask(index.relocation(page.as_str().into())).await? else {
            return Err(MISSING_SOURCE.into());
        };
        let taken = ask(index.path_taken(target.clone())).await?;
        let relocation = Relocation::new(
            &source.path,
            &target,
            true,
            &facts.page.title,
            facts.unique_title,
            true,
            |_| taken,
        )?;
        let mut sources = vec![source];
        for candidate in facts.candidates.iter().skip(1) {
            let id = candidate.page.to_lossy_string();
            if let Some(source) = page_writes::read(id, &open_pages, &storage, cx).await? {
                sources.push(source);
            }
        }
        let mut analyzed = Vec::new();
        for source in sources {
            let (original, normalized) = SourceText::prepare(source.stored.clone());
            let work = cx.update(|cx| crate::page_index::relocation_links(normalized.clone(), cx));
            let Some((references, urls)) = cx.background_spawn(work).await else {
                return Err("Cannot read links because the Markdown grammar is unavailable. Rebuild Quire with its bundled grammars.".into());
            };
            analyzed.push((source, original, normalized, references, urls));
        }
        let bases: Vec<String> = analyzed
            .iter()
            .flat_map(|(_, _, _, references, _)| references.iter())
            .map(|reference| reference.target.split('#').next().unwrap_or_default().to_owned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let (_, resolved) = ask(index.with_generation(move |index| {
            bases
                .into_iter()
                .map(|base| Ok((base.clone(), index.find_page(&base)?.map(|page| page.path))))
                .collect::<Result<HashMap<_, _>, _>>()
        }))
        .await?;
        let mut writes = Vec::new();
        for (source, original, normalized, references, urls) in analyzed {
            let Some(planned) = relocate_page(
                &relocation,
                &source.path,
                &normalized,
                &references,
                &urls,
                |base| resolved.get(base).cloned().flatten(),
            ) else {
                continue;
            };
            let text = original.apply(planned.edits).map_err(|error| {
                format!("Could not rewrite links in {}: {error}. Rename again.", source.path)
            })?;
            writes.push(source.write_at(planned.path, text));
        }
        Ok(Some(RenamePlan {
            target: relocation.target,
            writes,
        }))
    })
}

/// Writes the rename and every link change or none. Returns how many pages were written.
pub fn rename(
    plan: RenamePlan,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    page_writes::apply(plan.writes, storage, STALE_RENAME, cx)
}
