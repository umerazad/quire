//! The page catalogue that navigation lists: every live page with its title, path, and daily
//! date, including unsaved titles from open pages.

use super::query::{PAGE_COLUMNS, PageRow, page_ref};
use super::{QueryIndex, strip_markdown};
use crate::index::PageRef;
use crate::sync::json::JsString;
use anyhow::Result;
use std::collections::HashSet;

/// The replicated page that carries notebook settings; navigation never lists it.
pub const SETTINGS_PAGE: &str = "s-notebook";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedPage {
    pub page: PageRef,
    pub date: Option<String>,
}

impl QueryIndex {
    /// Every live page ordered by title, as the reference `pagePaths` lists them.
    pub fn pages(&self) -> Result<Vec<ListedPage>> {
        self.refresh()?;
        let rows: Vec<(PageRow, Option<String>)> = self.connection.select(&format!(
            "SELECT {PAGE_COLUMNS},p.date FROM live_pages p WHERE p.deleted=0"
        ))?()?;
        let mut pages = rows
            .into_iter()
            .map(|(row, date)| {
                Ok(ListedPage {
                    page: page_ref(row)?,
                    date,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        pages.retain(|listed| listed.page.page.as_str() != Some(SETTINGS_PAGE));
        pages.sort_by(|a, b| {
            self.collator
                .compare(&a.page.title, &b.page.title)
                .then_with(|| self.collator.compare(&a.page.path, &b.page.path))
        });
        Ok(pages)
    }

    /// Whether a live page already uses `path`, compared case-insensitively as page names are.
    pub fn path_taken(&self, path: &str) -> Result<bool> {
        self.refresh()?;
        let name = strip_markdown(path).to_lowercase();
        Ok(!self.pages_where("p.name_key=?", &name)?.is_empty())
    }

    pub fn relocation(&self, page: &JsString) -> Result<Option<RelocationFacts>> {
        self.refresh()?;
        let Some(this) = self
            .pages_where("p.page=?", &page.to_json())?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        let unique_title = self
            .pages_where("p.title_key=?", &this.title.to_lowercase())?
            .len()
            == 1;
        let mut seen: HashSet<Vec<u8>> = HashSet::from([this.page.as_bytes().to_vec()]);
        let mut candidates = vec![this.clone()];
        let linked = self.backlinks(page)?.into_iter().map(|link| link.source);
        let marked = self.pages_where(
            "p.revision IN (SELECT revision FROM live_blocks WHERE content LIKE ?)",
            "%.md%",
        )?;
        // A page saved elsewhere and not analyzed yet has no links or blocks to look at, so it
        // is read in full rather than missed.
        let unread = self.pages_where("p.analyzed=0 AND ?", "1")?;
        for candidate in linked.chain(marked).chain(unread) {
            if seen.insert(candidate.page.as_bytes().to_vec()) {
                candidates.push(candidate);
            }
        }
        Ok(Some(RelocationFacts {
            page: this,
            unique_title,
            candidates,
        }))
    }

    /// What deleting a page or a folder takes: the pages that go, ordered by path, and how many
    /// links from the pages that stay will stop resolving.
    pub fn deletion(&self, target: &DeletionTarget) -> Result<Deletion> {
        let mut pages: Vec<PageRef> = match target {
            DeletionTarget::Page(page) => self.pages_where("p.page=?", &page.to_json())?,
            DeletionTarget::Folder(folder) => self
                .pages()?
                .into_iter()
                .map(|listed| listed.page)
                .filter(|page| {
                    page.path
                        .strip_prefix(folder.as_str())
                        .is_some_and(|rest| rest.starts_with('/'))
                })
                .collect(),
        };
        pages.sort_by(|a, b| self.collator.compare(&a.path, &b.path));
        let gone: HashSet<&[u8]> = pages.iter().map(|page| page.page.as_bytes()).collect();
        let mut broken_links = 0;
        for page in &pages {
            broken_links += self
                .backlinks(&page.page)?
                .iter()
                .filter(|link| !gone.contains(link.source.page.as_bytes()))
                .count();
        }
        Ok(Deletion {
            pages,
            broken_links,
        })
    }
}

/// What a rename or move of one page needs from the index: its title, whether any other page
/// shares it, and the pages whose links may follow it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RelocationFacts {
    pub page: PageRef,
    pub unique_title: bool,
    /// Pages linking to it by name, title, or block, and pages with Markdown links to `.md`
    /// files, which may point at it by relative path. The page itself comes first.
    pub candidates: Vec<PageRef>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DeletionTarget {
    Page(JsString),
    Folder(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Deletion {
    pub pages: Vec<PageRef>,
    pub broken_links: usize,
}

impl super::IndexClient {
    pub fn pages(&self) -> super::Answer<Vec<ListedPage>> {
        self.request(64, QueryIndex::pages)
    }
    pub fn path_taken(&self, path: String) -> super::Answer<bool> {
        self.request(128 + path.len(), move |index| index.path_taken(&path))
    }
    pub fn relocation(&self, page: JsString) -> super::Answer<Option<RelocationFacts>> {
        self.request(128 + page.as_bytes().len(), move |index| {
            index.relocation(&page)
        })
    }
    pub fn deletion(&self, target: DeletionTarget) -> super::Answer<Deletion> {
        self.request(256, move |index| index.deletion(&target))
    }
}
