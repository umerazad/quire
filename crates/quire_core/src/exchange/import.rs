//! Planning a Markdown folder import, ported from the reference `importMarkdown`: entries whose
//! names start with a dot are skipped, `.md` files (any case) become pages at their relative
//! paths, every other ordinary file is copied beside them so links to it still work, and a page
//! whose path the notebook already uses, ignoring case, is left alone.

use crate::relocate::notebook_path;
use std::collections::HashSet;
use unicode_normalization::UnicodeNormalization as _;

/// What an entry in the source folder becomes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceEntry {
    Skipped,
    Folder,
    Page,
    File,
}

/// Classifies an entry by its name and type, as the reference walk does. Symbolic links and other
/// special files are skipped.
pub fn source_entry(name: &str, directory: bool, file: bool) -> SourceEntry {
    if name.starts_with('.') {
        SourceEntry::Skipped
    } else if directory {
        SourceEntry::Folder
    } else if !file {
        SourceEntry::Skipped
    } else if name.to_lowercase().ends_with(".md") {
        SourceEntry::Page
    } else {
        SourceEntry::File
    }
}

/// The key two paths share when the notebook treats them as one page.
pub fn path_key(path: &str) -> String {
    path.nfc().collect::<String>().to_lowercase()
}

/// Which source pages to create, in source order, and which the notebook already has.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ImportPlan {
    /// Source path and notebook path of each page to create.
    pub pages: Vec<(String, String)>,
    /// Source paths of pages left alone because the notebook already has their path.
    pub skipped: Vec<String>,
}

/// A source page whose path cannot be a notebook path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnusablePath {
    pub path: String,
    pub reason: &'static str,
}

impl std::fmt::Display for UnusablePath {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "Cannot import {}: {} Rename it in the source folder and import again; nothing was imported.",
            self.path, self.reason
        )
    }
}

impl std::error::Error for UnusablePath {}

/// Plans `pages` (relative source paths in walk order) against the notebook's taken path keys.
/// A page planned earlier takes its path for the pages after it.
pub fn plan_import(
    pages: &[String],
    taken: impl Fn(&str) -> bool,
) -> Result<ImportPlan, UnusablePath> {
    let mut planned = HashSet::new();
    let mut plan = ImportPlan::default();
    for source in pages {
        let target = notebook_path(source, false).map_err(|reason| UnusablePath {
            path: source.clone(),
            reason,
        })?;
        let key = path_key(&target);
        if taken(&key) || !planned.insert(key) {
            plan.skipped.push(source.clone());
        } else {
            plan.pages.push((source.clone(), target));
        }
    }
    Ok(plan)
}

/// The reference report after an import.
pub fn import_summary(pages: usize, files: usize, skipped: usize) -> String {
    let skipped = if skipped > 0 {
        format!("{skipped} pages were already here and were left unchanged.")
    } else {
        String::new()
    };
    format!("Imported {pages} pages and {files} other files. {skipped}")
        .trim_end()
        .to_owned()
}
