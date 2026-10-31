//! A rebuildable query index derived from the notebook's current heads.
//!
//! The index lives in its own SQLite file with the notebook database attached read-only. It is a
//! cache: deleting it loses nothing, and any version, identity, or corruption mismatch rebuilds it.
//! Rows are keyed by immutable revision IDs, so a row is either complete or absent, never stale.

mod blocks;
mod catalog;
mod folder;
mod overlay;
mod query;
mod references;
mod schema;
pub(crate) use schema::uri_path;
mod tasks;
mod tools;

pub use blocks::{BlockChoice, BlockPreview};
pub use catalog::{Deletion, DeletionTarget, ListedPage, RelocationFacts, SETTINGS_PAGE};
pub use overlay::Overlay;
pub use query::{Backlink, PageRef, SearchHit, TagCount, TaggedBlock};
pub use references::{Facet, FacetKind, Outline, ReferenceBlock, SHOWN_CHILDREN};
pub use tasks::{
    ProjectSummary, TaskBoard, TaskCounts, TaskItem, TaskList, TaskListGroup, TaskListQuery,
    TaskQuery, UPCOMING_DAYS,
};

use crate::notebook::{PageMetadata, read_revision, revision_metadata};
use crate::sync::json::JsString;
use crate::worker::{Client, QueueError, QueueLimits, Reply, Worker};
use anyhow::{Context as _, Result, bail, ensure};
use quire_core::daily::{DailyCalendar, DailyLayout, DailyParser, write_settings};
use quire_core::reference_filter::{FilterNames, ReferenceContext, ReferenceFilter};
use quire_core::tag_edits::Retag;
use quire_core::tasks::{DailyNames, DateSources};
use serde::{Deserialize, Serialize};
use sqlez::connection::Connection;
use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    path::{Path, PathBuf},
};

const APPLICATION_ID: i32 = i32::from_be_bytes(*b"QIDX");
const REBUILD: &str = "The notebook query index is damaged. Quire will rebuild it; your notes are unaffected. Reopen the notebook if this repeats.";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedBlock {
    pub key: String,
    pub id: Option<String>,
    pub from: u32,
    pub to: u32,
    pub content_from: u32,
    pub content_to: u32,
    pub subtree_to: u32,
    pub depth: u32,
    pub kind: String,
    pub parent: Option<String>,
    pub text: String,
    /// The raw content, ID included, as linked-reference filters read it.
    pub content: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedLink {
    pub kind: String,
    pub syntax: String,
    pub target: String,
    pub label: Option<String>,
    pub media: Option<String>,
    pub from: u32,
    pub to: u32,
    /// The containing block's text, or the reference source when no block contains it.
    pub text: String,
}

impl IndexedLink {
    /// Page links, and wiki embeds of whole pages, resolve by page name; the rest by block ID.
    pub fn is_page_reference(&self) -> bool {
        self.kind == "page"
            || (self.kind == "embed" && self.syntax == "wiki" && !self.target.contains("#^"))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedTag {
    pub tag: String,
    pub from: u32,
    pub block_id: Option<String>,
    pub snippet: String,
}

/// A task as the page's analysis reads it, with the sources of its dates rather than the dates,
/// so a daily-layout change re-derives them without reparsing.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexedTask {
    pub key: String,
    pub id: Option<String>,
    /// The block's start, for ordering tasks within a page.
    pub from: u32,
    /// The block's text, marker included; a task changed when this did.
    pub raw: String,
    pub text: String,
    pub state: String,
    pub tags: Vec<String>,
    pub links: Vec<String>,
    pub dates: DateSources,
    pub completed: Option<String>,
}

/// Everything the query index derives from one revision. Offsets are UTF-16 code units in the
/// editor's line-break-normalized text, matching the reference analysis.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageIndex {
    pub blocks: Vec<IndexedBlock>,
    pub links: Vec<IndexedLink>,
    pub tagged: Vec<IndexedTag>,
    pub front_tags: Vec<String>,
    #[serde(default)]
    pub tasks: Vec<IndexedTask>,
}

/// A selected head that still needs analysis. Text is the stored source, not yet normalized.
#[derive(Clone, Debug)]
pub struct PendingPage {
    pub revision: String,
    pub page: JsString,
    pub path: JsString,
    pub text: JsString,
}

pub struct IndexConfig {
    /// Bump when analysis output changes; a mismatch rebuilds the index.
    pub analyzer: i64,
    pub layout: DailyLayout,
    pub calendar: DailyCalendar,
}

pub struct QueryIndex {
    connection: Connection,
    layout: DailyLayout,
    calendar: DailyCalendar,
    parser: DailyParser,
    days: DailyNames,
    seen_version: Cell<Option<i64>>,
    generation: Cell<u64>,
    releasing: RefCell<BTreeSet<String>>,
    collator: icu_collator::CollatorBorrowed<'static>,
    folder: Option<folder::FolderSource>,
}

/// Matches the reference `pageKey`: drop any `#` suffix and `.md`, then lowercase.
pub fn page_key(target: &str) -> String {
    strip_markdown(target.split('#').next().unwrap_or_default()).to_lowercase()
}

/// Matches the reference `blockKey`: the identifier after `#^`, or the whole target.
pub fn block_key(target: &str) -> &str {
    target
        .find("#^")
        .map_or(target, |index| &target[index + 2..])
}

pub use quire_core::tag_edits::tag_key;

fn strip_markdown(path: &str) -> &str {
    let split = path.len().saturating_sub(3);
    match path.get(split..) {
        Some(tail) if tail.eq_ignore_ascii_case(".md") => &path[..split],
        _ => path,
    }
}

impl QueryIndex {
    /// Opens `query-index.sqlite` beside the notebook database, rebuilding it when it is missing,
    /// damaged, from another notebook, or produced by a different analyzer or format.
    pub fn open(notebook_database: &Path, config: IndexConfig) -> Result<Self> {
        let directory = notebook_database.parent().context(
            "The notebook database path has no parent folder. Choose an absolute database path.",
        )?;
        Self::open_at(
            &directory.join("query-index.sqlite"),
            schema::Notebook::Database(notebook_database),
            None,
            config,
        )
    }

    fn open_at(
        path: &Path,
        notebook: schema::Notebook<'_>,
        folder: Option<folder::FolderSource>,
        config: IndexConfig,
    ) -> Result<Self> {
        let connection = match schema::open(path, notebook, config.analyzer) {
            Ok(connection) => connection,
            Err(_) => {
                schema::remove(path)?;
                schema::open(path, notebook, config.analyzer)
                    .context("Cannot create the notebook query index. Check free space and write access to the notebook folder, then reopen it.")?
            }
        };
        let index = Self {
            connection,
            collator: icu_collator::Collator::try_new(Default::default(), Default::default())
                .context(
                    "Cannot load page sort order data. Rebuild Quire with its locked ICU data.",
                )?,
            parser: DailyParser::new(&config.layout, config.calendar.clone()),
            days: DailyNames::new(config.layout.clone(), config.calendar.clone()),
            layout: config.layout,
            calendar: config.calendar,
            seen_version: Cell::new(None),
            generation: Cell::new(0),
            releasing: RefCell::default(),
            folder,
        };
        overlay::create(&index.connection)?;
        index.apply_layout()?;
        Ok(index)
    }

    fn transaction<T>(&self, run: impl FnOnce() -> Result<T>) -> Result<T> {
        self.connection.exec("BEGIN IMMEDIATE")?()?;
        match run() {
            Ok(value) => {
                self.connection.exec("COMMIT")?()?;
                Ok(value)
            }
            Err(error) => {
                let _ = self
                    .connection
                    .exec("ROLLBACK")
                    .and_then(|mut rollback| rollback());
                Err(error)
            }
        }
    }

    /// The daily date a page title names under the notebook layout, as `YYYY-MM-DD`.
    pub fn date(&self, title: &str) -> Option<String> {
        self.parser.date(title)
    }

    /// A task's created and due dates under the notebook layout, from its stored date sources.
    /// Neither falls back to the page's date here; queries add that from the live page.
    fn task_dates(&self, dates: &str) -> Result<(Option<String>, Option<String>)> {
        let sources: DateSources = serde_json::from_str(dates).context(REBUILD)?;
        Ok((sources.created(&self.days), sources.due(&self.days)))
    }

    /// Date keys depend on the daily layout. A changed layout re-derives them from stored titles
    /// and targets; pages are not reparsed.
    fn apply_layout(&self) -> Result<()> {
        let key = write_settings(&self.layout);
        let stored: Option<String> = self.connection.select_row("SELECT layout FROM meta")?()?;
        if stored.as_deref() == Some(key.as_str()) {
            return Ok(());
        }
        self.transaction(|| {
            let pages: Vec<(String, String)> =
                self.connection.select("SELECT revision,title FROM pages")?()?;
            for (revision, title) in pages {
                self.connection
                    .exec_bound("UPDATE pages SET date=? WHERE revision=?")?((
                    self.date(&title),
                    revision.as_str(),
                ))?;
            }
            let links: Vec<(i64, String)> = self
                .connection
                .select("SELECT rowid,target FROM links WHERE page_key IS NOT NULL")?(
            )?;
            for (row, target) in links {
                self.connection
                    .exec_bound("UPDATE links SET date=? WHERE rowid=?")?((
                    self.date(&target),
                    row,
                ))?;
            }
            let tasks: Vec<(i64, String)> =
                self.connection.select("SELECT rowid,dates FROM tasks")?()?;
            for (row, dates) in tasks {
                let (created, due) = self.task_dates(&dates)?;
                self.connection
                    .exec_bound("UPDATE tasks SET created=?,due=? WHERE rowid=?")?(
                    (
                    created, due, row,
                )
                )?;
            }
            self.connection.exec_bound("UPDATE meta SET layout=?")?(key.as_str())?;
            Ok(())
        })
    }

    /// Brings the page catalogue up to date with the notebook's selected heads. The catalogue
    /// reads checksummed metadata only, so names, titles, and dates are complete before analysis.
    /// Returns whether anything changed.
    pub fn refresh(&self) -> Result<bool> {
        if let Some(folder) = &self.folder {
            return self.refresh_folder(folder);
        }
        let version: i64 =
            self.connection.select_row("PRAGMA nb.data_version")?()?.context(REBUILD)?;
        if self.seen_version.get() == Some(version) {
            return Ok(false);
        }
        let changed = self.transaction(|| {
            let stale: Vec<String> = self.connection.select(schema::STALE_PAGES)?()?;
            for revision in &stale {
                self.connection
                    .exec_bound("DELETE FROM pages WHERE revision=?")?(
                    revision.as_str()
                )?;
            }
            let missing: Vec<String> = self.connection.select(schema::MISSING_PAGES)?()?;
            for revision in &missing {
                let Some(metadata) = revision_metadata(&self.connection, revision)? else {
                    bail!(REBUILD);
                };
                self.catalogue(&metadata)?;
            }
            Ok(!stale.is_empty() || !missing.is_empty())
        })?;
        self.seen_version.set(Some(version));
        if changed {
            self.bump();
            self.settle_releases()?;
        }
        Ok(changed)
    }

    fn catalogue(&self, metadata: &PageMetadata) -> Result<()> {
        let path = metadata.path.to_lossy_string();
        let title = metadata.title.to_lossy_string();
        self.connection.exec_bound(
            "INSERT INTO pages(revision,page,path,name_key,title,title_key,date,deleted,analyzed) VALUES (?,?,?,?,?,?,?,?,0)",
        )?((
            metadata.revision.as_str(),
            metadata.page.to_json(),
            path.as_str(),
            strip_markdown(&path).to_lowercase(),
            title.as_str(),
            title.to_lowercase(),
            self.date(&title),
            metadata.deleted,
        ))?;
        Ok(())
    }

    /// Selected live heads that still need analysis, with their stored source.
    pub fn pending(&self, limit: usize) -> Result<Vec<PendingPage>> {
        self.refresh()?;
        if let Some(folder) = &self.folder {
            return self.pending_files(folder, limit);
        }
        let rows: Vec<String> = self.connection.select_bound(
            "SELECT revision FROM pages WHERE analyzed=0 AND deleted=0 ORDER BY revision LIMIT ?",
        )?(limit.clamp(1, 256) as u32)?;
        rows.into_iter()
            .map(|revision| {
                let record = read_revision(&self.connection, "nb", &revision)?
                    .context("A page revision disappeared while indexing. Quire will retry.")?;
                let value = record.root();
                let text = value.field("text").as_js_string().context(REBUILD)?.clone();
                Ok(PendingPage {
                    page: value.field("page").as_js_string().context(REBUILD)?.clone(),
                    path: value.field("path").as_js_string().context(REBUILD)?.clone(),
                    text,
                    revision,
                })
            })
            .collect()
    }

    /// Selected live heads not yet analyzed. Queries are complete when this is zero.
    pub fn unanalyzed(&self) -> Result<usize> {
        self.refresh()?;
        let count: Option<i64> = self
            .connection
            .select_row("SELECT count(*) FROM pages WHERE analyzed=0 AND deleted=0")?(
        )?;
        Ok(count.unwrap_or_default() as usize)
    }

    /// Records analysis for one revision. Returns false when the revision is no longer a selected
    /// head, so the caller can drop the work.
    pub fn put(&self, revision: &str, analysis: &PageIndex) -> Result<bool> {
        Ok(self.put_many(&[(revision, analysis)])?[0])
    }

    /// Records analysis for several revisions in one transaction, so a rebuild commits and
    /// flushes the search index once per batch instead of once per page. Each result is what
    /// [`Self::put`] returns for that revision.
    pub fn put_many(&self, analyses: &[(&str, &PageIndex)]) -> Result<Vec<bool>> {
        let stored = self.transaction(|| {
            analyses
                .iter()
                .map(|(revision, analysis)| self.store(revision, analysis))
                .collect::<Result<Vec<_>>>()
        })?;
        if stored.contains(&true) {
            self.bump();
            self.settle_releases()?;
        }
        Ok(stored)
    }

    fn store(&self, revision: &str, analysis: &PageIndex) -> Result<bool> {
        let row: Option<(bool, String)> = self
            .connection
            .select_row_bound("SELECT analyzed,title FROM pages WHERE revision=?")?(
            revision
        )?;
        let title = match row {
            None => return Ok(false),
            Some((true, _)) => return Ok(true),
            Some((false, title)) => title,
        };
        let mut search = self
            .connection
            .exec_bound("INSERT INTO search_rows(revision,title,text,block_id) VALUES (?,?,?,?)")?;
        search((revision, title.as_str(), "", None::<&str>))?;
        for block in &analysis.blocks {
            search((
                revision,
                title.as_str(),
                block.text.as_str(),
                block.id.as_deref(),
            ))?;
        }
        self.insert_rows("main.", revision, analysis)?;
        self.connection
            .exec_bound("UPDATE pages SET analyzed=1 WHERE revision=?")?(revision)?;
        Ok(true)
    }

    /// Writes block, link, and tag rows into `main.` tables or the `temp.overlay_` copies.
    fn insert_rows(&self, tables: &str, revision: &str, analysis: &PageIndex) -> Result<()> {
        for (ordinal, tag) in analysis.front_tags.iter().enumerate() {
            self.connection.exec_bound(&format!(
                "INSERT INTO {tables}front_tags(revision,ordinal,tag) VALUES (?,?,?)"
            ))?((revision, ordinal as u32, tag.as_str()))?;
        }
        for (ordinal, block) in analysis.blocks.iter().enumerate() {
            self.connection.exec_bound(
                    &format!("INSERT INTO {tables}blocks(revision,ordinal,key,id,start,finish,content_from,content_to,subtree_to,depth,kind,parent,text,content) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)"),
                )?((
                    (revision, ordinal as u32, block.key.as_str(), block.id.as_deref()),
                    (block.from, block.to, block.content_from, block.content_to),
                    (block.subtree_to, block.depth, block.kind.as_str(), block.parent.as_deref(), block.text.as_str()),
                    block.content.as_str(),
                ))?;
        }
        for (ordinal, link) in analysis.links.iter().enumerate() {
            let page = link.is_page_reference();
            self.connection.exec_bound(
                    &format!("INSERT INTO {tables}links(revision,ordinal,kind,syntax,target,label,media,start,finish,text,page_key,block_key,date) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?)"),
                )?((
                    (revision, ordinal as u32, link.kind.as_str(), link.syntax.as_str(), link.target.as_str()),
                    (link.label.as_deref(), link.media.as_deref(), link.from, link.to, link.text.as_str()),
                    (
                        page.then(|| page_key(&link.target)),
                        (!page).then(|| block_key(&link.target)),
                        page.then(|| self.date(&link.target)).flatten(),
                    ),
                ))?;
        }
        for (ordinal, task) in analysis.tasks.iter().enumerate() {
            let dates = serde_json::to_string(&task.dates)?;
            let (created, due) = self.task_dates(&dates)?;
            self.connection.exec_bound(
                    &format!("INSERT INTO {tables}tasks(revision,ordinal,key,id,start,raw,text,state,tags,links,dates,completed,created,due) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?)"),
                )?((
                    (revision, ordinal as u32, task.key.as_str(), task.id.as_deref(), task.from),
                    (task.raw.as_str(), task.text.as_str(), task.state.as_str()),
                    (serde_json::to_string(&task.tags)?, serde_json::to_string(&task.links)?, dates.as_str()),
                    (task.completed.as_deref(), created, due),
                ))?;
        }
        for (ordinal, tag) in analysis.tagged.iter().enumerate() {
            self.connection.exec_bound(
                    &format!("INSERT INTO {tables}tagged(revision,ordinal,tag,tag_key,start,block_id,snippet) VALUES (?,?,?,?,?,?,?)"),
                )?((
                    revision,
                    ordinal as u32,
                    tag.tag.as_str(),
                    tag_key(&tag.tag),
                    tag.from,
                    tag.block_id.as_deref(),
                    tag.snippet.as_str(),
                ))?;
        }
        Ok(())
    }
}

pub type IndexClient = Client<QueryIndex>;
pub type IndexWorker = Worker<QueryIndex>;

impl Worker<QueryIndex> {
    /// Starts the index on its own thread so queries never wait behind saves.
    pub fn spawn(notebook_database: PathBuf, config: IndexConfig) -> Result<Self> {
        ensure!(
            notebook_database.is_absolute(),
            "Cannot index notebook database {}. Use an absolute path.",
            notebook_database.display()
        );
        Self::spawn_with("quire-index", QueueLimits::default(), move || {
            QueryIndex::open(&notebook_database, config)
        })
    }
}

type Answer<T> = std::result::Result<Reply<T>, QueueError>;

impl Client<QueryIndex> {
    pub fn pending(&self, limit: usize) -> Answer<Vec<PendingPage>> {
        self.request(128, move |index| index.pending(limit))
    }
    pub fn unanalyzed(&self) -> Answer<usize> {
        self.request(64, QueryIndex::unanalyzed)
    }
    pub fn put(&self, revision: String, analysis: PageIndex) -> Answer<bool> {
        let bytes = 256 + serde_json::to_vec(&analysis).map_or(0, |data| data.len());
        self.request(bytes, move |index| index.put(&revision, &analysis))
    }
    pub fn put_many(&self, analyses: Vec<(String, PageIndex)>) -> Answer<Vec<bool>> {
        let bytes = analyses
            .iter()
            .map(|(_, analysis)| 256 + serde_json::to_vec(analysis).map_or(0, |data| data.len()))
            .sum();
        self.request(bytes, move |index| {
            let borrowed: Vec<_> = analyses
                .iter()
                .map(|(revision, analysis)| (revision.as_str(), analysis))
                .collect();
            index.put_many(&borrowed)
        })
    }
    pub fn find_page(&self, target: String) -> Answer<Option<PageRef>> {
        self.request(128 + target.len(), move |index| index.find_page(&target))
    }
    pub fn resolve_block(&self, target: String) -> Answer<Option<PageRef>> {
        self.request(128 + target.len(), move |index| {
            index.resolve_block(&target)
        })
    }
    pub fn backlinks(&self, page: JsString) -> Answer<Vec<Backlink>> {
        self.request(128 + page.as_bytes().len(), move |index| {
            index.backlinks(&page)
        })
    }
    pub fn mentions(&self, title: String) -> Answer<Vec<Backlink>> {
        self.request(128 + title.len(), move |index| index.mentions(&title))
    }
    pub fn tagged_blocks(&self, tag: String) -> Answer<Vec<TaggedBlock>> {
        self.request(128 + tag.len(), move |index| index.tagged_blocks(&tag))
    }
    pub fn tag_names(&self) -> Answer<Vec<TagCount>> {
        self.request(64, QueryIndex::tag_names)
    }
    pub fn tag_edit_pages(&self, retag: Retag) -> Answer<Vec<PageRef>> {
        self.request(256, move |index| index.tag_edit_pages(&retag))
    }
    pub fn search(&self, query: String) -> Answer<Vec<SearchHit>> {
        self.request(128 + query.len(), move |index| index.search(&query))
    }
    pub fn block_choices(&self, query: String, limit: usize) -> Answer<Vec<BlockChoice>> {
        self.request(128 + query.len(), move |index| {
            index.block_choices(&query, limit)
        })
    }
    pub fn block_preview(&self, target: String) -> Answer<Option<BlockPreview>> {
        self.request(128 + target.len(), move |index| {
            index.block_preview(&target)
        })
    }
    pub fn linked_references(
        &self,
        page: JsString,
        oldest_first: bool,
    ) -> Answer<Vec<ReferenceBlock>> {
        self.request(128 + page.as_bytes().len(), move |index| {
            index.linked_references(&page, oldest_first)
        })
    }
    pub fn filter_names(&self, filter: ReferenceFilter) -> Answer<FilterNames> {
        self.request(256, move |index| index.filter_names(&filter))
    }
    pub fn reference_facets(
        &self,
        shown: JsString,
        contexts: Vec<ReferenceContext>,
        limit: usize,
    ) -> Answer<Vec<Facet>> {
        let bytes = contexts
            .iter()
            .fold(256, |size, context| size + context.text.len());
        self.request(bytes, move |index| {
            index.reference_facets(&shown, &contexts, limit)
        })
    }
    pub fn set_overlay(&self, page: JsString, overlay: Overlay) -> Answer<()> {
        let bytes = 256 + serde_json::to_vec(&overlay.index).map_or(0, |data| data.len());
        self.request(bytes, move |index| index.set_overlay(&page, &overlay))
    }
    pub fn release_overlay(&self, page: JsString) -> Answer<()> {
        self.request(128, move |index| index.release_overlay(&page))
    }
    /// Runs a query and returns it with the generation it observed, in one worker step.
    pub fn with_generation<T: Send + 'static>(
        &self,
        query: impl FnOnce(&QueryIndex) -> Result<T> + Send + 'static,
    ) -> Answer<(u64, T)> {
        self.request(128, move |index| {
            let value = query(index)?;
            Ok((index.generation(), value))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superseded_revisions_leave_search_through_the_cascade() -> Result<()> {
        let root = tempfile::tempdir()?;
        let database = root.path().join("local.sqlite");
        let store = crate::notebook::NotebookStore::open(&database, "nb", "local")?;
        let save = |text: &str| -> Result<()> {
            store.save(&crate::notebook::Edit {
                page: "p".into(),
                path: "P.md".into(),
                text: text.into(),
                base: store.heads("p")?,
                deleted: false,
                created: 1,
                recovery: None,
            })?;
            Ok(())
        };
        let analyze = |index: &QueryIndex, word: &str| -> Result<()> {
            for page in index.pending(8)? {
                let block = IndexedBlock {
                    text: word.into(),
                    ..Default::default()
                };
                index.put(
                    &page.revision,
                    &PageIndex {
                        blocks: vec![block],
                        ..Default::default()
                    },
                )?;
            }
            Ok(())
        };
        save("old")?;
        let index = QueryIndex::open(
            &database,
            IndexConfig {
                analyzer: 1,
                layout: DailyLayout::default(),
                calendar: DailyCalendar::utc(),
            },
        )?;
        analyze(&index, "obsoleteword")?;
        assert_eq!(index.search("obsoleteword")?.len(), 1);
        save("new")?;
        analyze(&index, "freshword")?;
        assert!(index.search("obsoleteword")?.is_empty());
        assert_eq!(index.search("fresh")?[0].snippet, "freshword");
        assert_eq!(
            index.search("P")?.len(),
            2,
            "one title row and one block row"
        );
        index
            .connection
            .exec("INSERT INTO search(search) VALUES ('integrity-check')")?()?;
        let rows: Option<i64> = index
            .connection
            .select_row("SELECT count(*) FROM search_rows")?()?;
        assert_eq!(rows, Some(2));
        Ok(())
    }

    #[test]
    fn a_batch_of_analyses_stores_each_page_as_put_does() -> Result<()> {
        let root = tempfile::tempdir()?;
        let database = root.path().join("local.sqlite");
        let store = crate::notebook::NotebookStore::open(&database, "nb", "local")?;
        for page in ["a", "b"] {
            store.save(&crate::notebook::Edit {
                page: page.into(),
                path: format!("{page}.md"),
                text: page.into(),
                base: Vec::new(),
                deleted: false,
                created: 1,
                recovery: None,
            })?;
        }
        let index = QueryIndex::open(
            &database,
            IndexConfig {
                analyzer: 1,
                layout: DailyLayout::default(),
                calendar: DailyCalendar::utc(),
            },
        )?;
        let analysis = |word: &str| PageIndex {
            blocks: vec![IndexedBlock {
                text: word.into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        let pending = index.pending(8)?;
        let (alpha, beta, gone) = (analysis("alphaword"), analysis("betaword"), analysis("x"));
        let stored = index.put_many(&[
            (pending[0].revision.as_str(), &alpha),
            (pending[1].revision.as_str(), &beta),
            ("no-such-revision", &gone),
        ])?;
        assert_eq!(stored, [true, true, false]);
        assert_eq!(index.unanalyzed()?, 0);
        assert_eq!(index.search("alphaword")?.len(), 1);
        assert_eq!(index.search("betaword")?.len(), 1);
        assert_eq!(
            index.put_many(&[(pending[0].revision.as_str(), &alpha)])?,
            [true],
            "an analyzed revision stays stored, as put reports"
        );
        assert_eq!(index.search("alphaword")?.len(), 1);
        Ok(())
    }

    #[test]
    fn key_lookups_use_indexes_instead_of_scanning() -> Result<()> {
        let root = tempfile::tempdir()?;
        let database = root.path().join("local.sqlite");
        drop(crate::notebook::NotebookStore::open(
            &database, "nb", "local",
        )?);
        let index = QueryIndex::open(
            &database,
            IndexConfig {
                analyzer: 1,
                layout: DailyLayout::default(),
                calendar: DailyCalendar::utc(),
            },
        )?;
        for (query, expected) in [
            (
                "SELECT * FROM pages p WHERE p.deleted=0 AND p.name_key=?",
                "pages_by_name",
            ),
            (
                "SELECT * FROM pages p WHERE p.deleted=0 AND p.title_key=?",
                "pages_by_title",
            ),
            (
                "SELECT * FROM pages p WHERE p.deleted=0 AND p.date=?",
                "pages_by_date",
            ),
            (
                "SELECT * FROM live_links l JOIN live_pages p ON p.revision=l.revision WHERE p.deleted=0 AND l.page_key=?",
                "links_by_page",
            ),
            (
                "SELECT * FROM live_links l JOIN live_pages p ON p.revision=l.revision WHERE p.deleted=0 AND l.block_key=?",
                "links_by_block",
            ),
            (
                "SELECT * FROM live_links l JOIN live_pages p ON p.revision=l.revision WHERE p.deleted=0 AND l.date=?",
                "links_by_date",
            ),
            (
                "SELECT * FROM live_blocks b JOIN live_pages p ON p.revision=b.revision WHERE p.deleted=0 AND b.id=?",
                "blocks_by_id",
            ),
            (
                "SELECT * FROM live_tagged t JOIN live_pages p ON p.revision=t.revision WHERE p.deleted=0 AND t.tag_key=?",
                "tagged_by_key",
            ),
        ] {
            let plan: Vec<(i64, i64, i64, String)> =
                index
                    .connection
                    .select_bound(&format!("EXPLAIN QUERY PLAN {query}"))?("")?;
            assert!(
                plan.iter()
                    .any(|(_, _, _, detail)| detail.contains(expected)),
                "{query}: {plan:?}"
            );
            assert!(
                !plan
                    .iter()
                    .any(|(_, _, _, detail)| detail.starts_with("SCAN")),
                "{query}: {plan:?}"
            );
        }
        Ok(())
    }
}
