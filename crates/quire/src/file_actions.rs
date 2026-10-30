//! Deleting pages and folders: plan what goes and how many links break, then write every
//! tombstone or none through `page_writes`.

use crate::{
    notebook_session::OpenPage,
    page_writes::{self, PageSource},
    query_index::ask,
};
use gpui::{App, Entity, Task};
use quire_store::{
    index::{DeletionTarget, IndexClient},
    worker::StorageClient,
};

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
