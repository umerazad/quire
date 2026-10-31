//! Indexing a folder notebook. The index reads the notebook's Markdown files itself: a rescan
//! stats every file, rereads only those whose size or times changed, and catalogues each page
//! under a key made of its text's revision and its path-derived identity. Queries, overlays, and
//! analysis then work exactly as for a Quire notebook.

use super::{PendingPage, QueryIndex, schema};
use crate::folder_notebook::{self, DiskDocument};
use crate::notebook::PageMetadata;
use crate::sync::{format::page_id_for_path, json::JsString};
use anyhow::{Context as _, Result};
use quire_core::markdown::page_title_for_editor;
use std::{
    cell::Cell,
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

pub(super) struct FolderSource {
    root: PathBuf,
    stale: Cell<bool>,
}

const FILES: &str = "CREATE TABLE IF NOT EXISTS folder_files (path TEXT PRIMARY KEY, stamp TEXT NOT NULL, revision TEXT NOT NULL REFERENCES pages(revision) ON DELETE CASCADE)";

fn key(document: &DiskDocument, page: &str) -> String {
    format!("{}:{page}", document.revision)
}

/// Size, modification time, and (on Unix) change time; a tool that restores a modification time
/// cannot restore the change time.
pub(super) fn stamp(path: &Path) -> Option<String> {
    let metadata = std::fs::metadata(path).ok()?;
    let nanos = |time: std::io::Result<std::time::SystemTime>| {
        time.ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |elapsed| elapsed.as_nanos())
    };
    #[cfg(unix)]
    let changed = {
        use std::os::unix::fs::MetadataExt as _;
        (metadata.ctime() as i128) * 1_000_000_000 + metadata.ctime_nsec() as i128
    };
    #[cfg(not(unix))]
    let changed = 0i128;
    Some(format!(
        "{}:{}:{changed}",
        metadata.len(),
        nanos(metadata.modified())
    ))
}

impl QueryIndex {
    /// Opens the query index of the folder notebook at `root`, kept in `state` outside it.
    pub fn open_folder(root: &Path, state: &Path, config: super::IndexConfig) -> Result<Self> {
        let real = std::fs::canonicalize(root).with_context(|| {
            format!(
                "Cannot open notebook folder {}. Make it available locally and reopen it.",
                root.display()
            )
        })?;
        std::fs::create_dir_all(state).with_context(|| format!("Cannot create Quire's state folder {}. Check write access to the application data folder and retry.", state.display()))?;
        let identity = format!("folder:{}", real.display());
        let index = Self::open_at(
            &state.join("query-index.sqlite"),
            schema::Notebook::Folder {
                identity: &identity,
            },
            Some(FolderSource {
                root: root.to_owned(),
                stale: Cell::new(true),
            }),
            config,
        )?;
        index.connection.exec(FILES)?()?;
        Ok(index)
    }

    /// Marks the folder's files as possibly changed; the next query rescans them. Does nothing
    /// for a Quire notebook, whose index follows the database.
    pub fn invalidate(&self) {
        if let Some(folder) = &self.folder {
            folder.stale.set(true);
        }
    }

    pub(super) fn refresh_folder(&self, folder: &FolderSource) -> Result<bool> {
        if !folder.stale.replace(false) {
            return Ok(false);
        }
        let known: HashMap<String, (String, String)> = self
            .connection
            .select::<(String, String, String)>("SELECT path,stamp,revision FROM folder_files")?(
        )?
        .into_iter()
        .map(|(path, stamp, revision)| (path, (stamp, revision)))
        .collect();
        let mut pages = HashSet::new();
        let mut kept = Vec::new();
        let mut added = Vec::new();
        for file in folder_notebook::list(&folder.root).files {
            let page = page_id_for_path(&file.path);
            if !pages.insert(page.clone()) {
                continue;
            }
            let Some(stamp) = stamp(&folder.root.join(&file.path)) else {
                continue;
            };
            match known.get(&file.path) {
                Some((known_stamp, revision)) if *known_stamp == stamp => {
                    kept.push(revision.clone());
                }
                _ => {
                    if let Ok(Some(document)) = folder_notebook::read(&folder.root, &file.path) {
                        added.push((page, stamp, document));
                    }
                }
            }
        }
        let wanted: HashSet<String> = kept
            .into_iter()
            .chain(added.iter().map(|(page, _, document)| key(document, page)))
            .collect();
        let changed = self.transaction(|| {
            let present: HashSet<String> = self
                .connection
                .select("SELECT revision FROM pages")?()?
            .into_iter()
            .collect();
            let mut changed = false;
            for (page, stamp, document) in &added {
                let revision = key(document, page);
                if !present.contains(&revision) {
                    let path = JsString::from(document.path.as_str());
                    self.catalogue(&PageMetadata {
                        page: JsString::from(page.as_str()),
                        revision: revision.clone(),
                        title: page_title_for_editor(&path, &JsString::from(document.text.as_str())),
                        path,
                        generation: 0,
                        created: 0,
                        deleted: false,
                        heads: 1,
                    })?;
                    changed = true;
                }
                self.connection.exec_bound(
                    "INSERT INTO folder_files(path,stamp,revision) VALUES (?,?,?) ON CONFLICT(path) DO UPDATE SET stamp=excluded.stamp,revision=excluded.revision",
                )?((document.path.as_str(), stamp.as_str(), revision.as_str()))?;
            }
            let stale: Vec<String> = present
                .into_iter()
                .filter(|revision| !wanted.contains(revision))
                .collect();
            Ok(self.retire(&stale)? || changed)
        })?;
        if changed {
            self.bump_committed();
            self.settle_releases()?;
        }
        Ok(changed)
    }

    /// Catalogued pages still needing analysis, read from their files. A file that changed since
    /// it was catalogued is skipped and the folder is rescanned on the next query.
    pub(super) fn pending_files(
        &self,
        folder: &FolderSource,
        limit: usize,
    ) -> Result<Vec<PendingPage>> {
        let rows: Vec<(String, String)> = self.connection.select_bound(
            "SELECT revision,path FROM pages WHERE analyzed=0 AND deleted=0 ORDER BY revision LIMIT ?",
        )?(limit.clamp(1, 256) as u32)?;
        let mut pending = Vec::new();
        for (revision, path) in rows {
            let page = page_id_for_path(&path);
            match folder_notebook::read(&folder.root, &path) {
                Ok(Some(document)) if key(&document, &page) == revision => {
                    pending.push(PendingPage {
                        revision,
                        page: JsString::from(page.as_str()),
                        path: JsString::from(path.as_str()),
                        text: JsString::from(document.text.as_str()),
                    })
                }
                _ => folder.stale.set(true),
            }
        }
        Ok(pending)
    }
}

impl super::IndexClient {
    pub fn invalidate(&self) -> super::Answer<()> {
        self.request(64, |index| {
            index.invalidate();
            Ok(())
        })
    }
}

impl super::IndexWorker {
    /// Starts the index of the folder notebook at `root` on its own thread.
    pub fn spawn_folder(root: PathBuf, state: PathBuf, config: super::IndexConfig) -> Result<Self> {
        anyhow::ensure!(
            root.is_absolute() && state.is_absolute(),
            "Cannot index notebook folder {}. Use an absolute path.",
            root.display()
        );
        Self::spawn_with(
            "quire-index",
            crate::worker::QueueLimits::default(),
            move || QueryIndex::open_folder(&root, &state, config),
        )
    }
}
