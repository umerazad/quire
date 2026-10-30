//! The page catalogue that navigation lists: every live page with its title, path, and daily
//! date, including unsaved titles from open pages.

use super::query::{PAGE_COLUMNS, PageRow, page_ref};
use super::{QueryIndex, strip_markdown};
use crate::index::PageRef;
use anyhow::Result;

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
}

impl super::IndexClient {
    pub fn pages(&self) -> super::Answer<Vec<ListedPage>> {
        self.request(64, QueryIndex::pages)
    }
    pub fn path_taken(&self, path: String) -> super::Answer<bool> {
        self.request(128 + path.len(), move |index| index.path_taken(&path))
    }
}
