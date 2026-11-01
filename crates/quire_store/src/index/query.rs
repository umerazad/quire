//! Reference queries over the index. Each mirrors the reference notebook store's in-memory
//! lookup of the same name, including its ambiguity and ordering rules.

use super::{IndexedLink, IndexedTag, QueryIndex, block_key, page_key, strip_markdown, tag_key};
use crate::sync::json::{JsString, Json};
use anyhow::{Context as _, Result};
use quire_core::tag_edits::Retag;
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PageRef {
    pub page: JsString,
    pub revision: String,
    pub path: String,
    pub title: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Backlink {
    pub source: PageRef,
    pub link: IndexedLink,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaggedBlock {
    pub source: PageRef,
    pub tag: IndexedTag,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TagCount {
    pub label: String,
    pub count: usize,
}

/// Saved tags by key, tallied at one committed state.
pub(super) type TagTallies = std::rc::Rc<HashMap<String, TagTally>>;

/// How many blocks use a tag, and its spelling where it is first used by path and offset.
#[derive(Clone, Debug, Default)]
pub(super) struct TagTally {
    count: usize,
    first: Option<TagUse>,
}

#[derive(Clone, Debug)]
struct TagUse {
    label: String,
    path: String,
    from: u32,
    page: String,
}

/// Records which saved tags changed, so [`QueryIndex::tag_names`] recounts only those.
pub(super) const TAG_CHANGES: &[&str] = &[
    "CREATE TEMP TABLE tag_changes (tag_key TEXT PRIMARY KEY) WITHOUT ROWID",
    "CREATE TEMP TRIGGER tagged_added AFTER INSERT ON main.tagged BEGIN
       INSERT OR IGNORE INTO temp.tag_changes VALUES (new.tag_key); END",
    "CREATE TEMP TRIGGER tagged_removed AFTER DELETE ON main.tagged BEGIN
       INSERT OR IGNORE INTO temp.tag_changes VALUES (old.tag_key); END",
];

/// One use of a tag: its spelling, page path, offset, and page.
type TagUseRow = (String, String, u32, String);

/// One tag's tally from all of its saved uses, or none when it has none.
fn tally(uses: Vec<TagUseRow>) -> Option<TagTally> {
    (!uses.is_empty()).then(|| {
        let mut tally = TagTally {
            count: uses.len(),
            first: None,
        };
        uses.into_iter().for_each(|found| tally.consider(found));
        tally
    })
}

impl TagTally {
    fn consider(&mut self, (label, path, from, page): TagUseRow) {
        let earlier = self.first.as_ref().is_none_or(|first| {
            utf16_order(&path, &first.path).then(from.cmp(&first.from)) == Ordering::Less
        });
        if earlier {
            self.first = Some(TagUse {
                label,
                path,
                from,
                page,
            });
        }
    }
}

pub(super) const PAGE_COLUMNS: &str = "p.revision,p.page,p.path,p.title";
pub(super) type PageRow = (String, String, String, String);
/// A page's `name_key`, `title_key`, and `date`.
type PageKeys = (String, String, Option<String>);
type LinkRow = (
    PageRow,
    (String, String, String, Option<String>, Option<String>),
    (u32, u32, String),
);
const LINK_SELECT: &str = "SELECT p.revision,p.page,p.path,p.title,l.kind,l.syntax,l.target,l.label,l.media,l.start,l.finish,l.text FROM live_links l JOIN live_pages p ON p.revision=l.revision WHERE p.deleted=0 AND ";

/// The live, undeleted page of every block with an ID, one row per block.
pub(super) fn block_pages() -> String {
    format!(
        "SELECT {PAGE_COLUMNS} FROM live_blocks b JOIN live_pages p ON p.revision=b.revision WHERE p.deleted=0 AND b.id=?"
    )
}

pub(super) fn page_ref((revision, page, path, title): PageRow) -> Result<PageRef> {
    let page = Json::parse(page.as_bytes())
        .ok()
        .and_then(|json| json.root().as_js_string().cloned())
        .context(super::REBUILD)?;
    Ok(PageRef {
        page,
        revision,
        path,
        title,
    })
}

fn backlink(
    (page, (kind, syntax, target, label, media), (from, to, text)): LinkRow,
) -> Result<Backlink> {
    Ok(Backlink {
        source: page_ref(page)?,
        link: IndexedLink {
            kind,
            syntax,
            target,
            label,
            media,
            from,
            to,
            text,
        },
    })
}

fn utf16_order(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

impl QueryIndex {
    pub(super) fn compare_paths(&self, a: &str, b: &str) -> Ordering {
        self.collator.compare(a, b)
    }

    pub(super) fn pages_where(&self, clause: &str, value: &str) -> Result<Vec<PageRef>> {
        let rows: Vec<PageRow> = self.connection.select_bound(&format!(
            "SELECT {PAGE_COLUMNS} FROM live_pages p WHERE p.deleted=0 AND {clause}"
        ))?(value)?;
        rows.into_iter().map(page_ref).collect()
    }

    fn links_where(&self, clause: &str, value: &str) -> Result<Vec<Backlink>> {
        let rows: Vec<LinkRow> =
            self.connection
                .select_bound(&format!("{LINK_SELECT}{clause}"))?(value)?;
        rows.into_iter().map(backlink).collect()
    }

    fn sort_links(&self, links: &mut [Backlink]) {
        links.sort_by(|a, b| {
            self.compare_paths(&a.source.path, &b.source.path)
                .then(a.link.from.cmp(&b.link.from))
        });
    }

    /// The reference `findPage`: an exact path name wins, then a unique title, then a date link
    /// opens that day's page. Several pages sharing a title resolve to none.
    pub fn find_page(&self, target: &str) -> Result<Option<PageRef>> {
        self.refresh()?;
        self.resolve_page(target)
    }

    pub(super) fn resolve_page(&self, target: &str) -> Result<Option<PageRef>> {
        self.resolve_with(target, |column, value| {
            self.pages_where(&format!("p.{column}=?"), value)
        })
    }

    /// [`Self::resolve_page`] for many names: every live page read once and grouped by name,
    /// title, and date, because preparing its queries per name dominated the task browser.
    pub(super) fn page_resolver(&self) -> Result<impl Fn(&str) -> Result<Option<PageRef>> + '_> {
        let rows: Vec<(PageRow, PageKeys)> =
            self.connection.select(&format!(
                "SELECT {PAGE_COLUMNS},p.name_key,p.title_key,p.date FROM live_pages p WHERE p.deleted=0"
            ))?()?;
        let mut keyed: HashMap<(&'static str, String), Vec<PageRef>> = HashMap::new();
        for (page, (name, title, date)) in rows {
            let page = page_ref(page)?;
            for key in [
                ("name_key", Some(name)),
                ("title_key", Some(title)),
                ("date", date),
            ] {
                if let (column, Some(value)) = key {
                    keyed.entry((column, value)).or_default().push(page.clone());
                }
            }
        }
        Ok(move |target: &str| {
            self.resolve_with(target, |column, value| {
                Ok(keyed
                    .get(&(column, value.to_owned()))
                    .cloned()
                    .unwrap_or_default())
            })
        })
    }

    /// The reference resolution order over `pages(column, value)`, which lists the live pages
    /// whose `name_key`, `title_key`, or `date` column holds `value`.
    fn resolve_with(
        &self,
        target: &str,
        mut pages: impl FnMut(&'static str, &str) -> Result<Vec<PageRef>>,
    ) -> Result<Option<PageRef>> {
        let name = page_key(target);
        let mut exact = pages("name_key", &name)?;
        if !exact.is_empty() {
            // Paths differing only in case share a name; take a stable one.
            exact.sort_by(|a, b| utf16_order(&a.path, &b.path));
            return Ok(exact.into_iter().next());
        }
        let titled = pages("title_key", &name)?;
        if !titled.is_empty() {
            return Ok((titled.len() == 1)
                .then(|| titled.into_iter().next())
                .flatten());
        }
        Ok(match self.date(target) {
            Some(date) => self.preferred_daily(&date, pages("date", &date)?),
            None => None,
        })
    }

    /// The reference `dailyPage`: among pages titled for `date`, prefer the one in the
    /// configured daily folder, otherwise the first by path.
    pub fn daily_page(&self, date: &str) -> Result<Option<PageRef>> {
        Ok(self.preferred_daily(date, self.pages_where("p.date=?", date)?))
    }

    fn preferred_daily(&self, date: &str, mut pages: Vec<PageRef>) -> Option<PageRef> {
        pages.sort_by(|a, b| utf16_order(&a.path, &b.path));
        let folder = self.layout.folder(date, &self.calendar).ok();
        let preferred = pages.iter().position(|page| {
            Some(&page.path[..page.path.rfind('/').unwrap_or(0)]) == folder.as_deref()
        });
        match preferred {
            Some(index) => Some(pages.swap_remove(index)),
            None => pages.into_iter().next(),
        }
    }

    /// The reference `resolve`: a block reference names exactly one block, optionally within
    /// the page before `#^`.
    pub fn resolve_block(&self, target: &str) -> Result<Option<PageRef>> {
        self.refresh()?;
        self.resolve_block_page(target)
    }

    pub(super) fn resolve_block_page(&self, target: &str) -> Result<Option<PageRef>> {
        let scope = match target.find("#^") {
            Some(index) => Some(self.resolve_page(&target[..index])?),
            None => None,
        };
        // One row per block: grouping here stops SQLite from flattening the live views, so it
        // copied every page into a temporary index on each call (2 s per backlinks query at 5x).
        let rows: Vec<PageRow> = self.connection.select_bound(&block_pages())?(block_key(target))?;
        let mut matches: Vec<PageRow> = rows
            .into_iter()
            .filter(|(revision, ..)| match &scope {
                Some(scope) => scope
                    .as_ref()
                    .is_some_and(|page| &page.revision == revision),
                None => true,
            })
            .collect();
        match (matches.pop(), matches.is_empty()) {
            (Some(only), true) => page_ref(only).map(Some),
            _ => Ok(None),
        }
    }

    /// The reference `backlinks`: links naming the page by path or title, date links it owns as
    /// that day's page, and block references resolving into it, ordered by source path.
    pub fn backlinks(&self, page: &JsString) -> Result<Vec<Backlink>> {
        self.refresh()?;
        let Some(this) = self
            .pages_where("p.page=?", &page.to_json())?
            .into_iter()
            .next()
        else {
            return Ok(Vec::new());
        };
        let mut found = BTreeSet::new();
        let mut links = Vec::new();
        let mut add = |link: Backlink, links: &mut Vec<Backlink>| {
            if found.insert((
                link.source.revision.clone(),
                link.link.from,
                link.link.to,
                link.link.target.clone(),
            )) {
                links.push(link);
            }
        };
        let names: BTreeSet<_> =
            [page_key(strip_markdown(&this.path)), page_key(&this.title)].into();
        for name in names {
            if self.resolve_page(&name)?.map(|page| page.revision) != Some(this.revision.clone()) {
                continue;
            }
            for link in self.links_where("l.page_key=?", &name)? {
                add(link, &mut links);
            }
        }
        if let Some(date) = self.date(&this.title)
            && self.daily_page(&date)?.map(|page| page.revision) == Some(this.revision.clone())
        {
            let mut owners = HashMap::new();
            for link in self.links_where("l.date=?", &date)? {
                let key = page_key(&link.link.target);
                let mine = match owners.get(&key) {
                    Some(mine) => *mine,
                    None => {
                        let mine = self
                            .resolve_page(&link.link.target)?
                            .is_none_or(|target| target.revision == this.revision);
                        owners.insert(key, mine);
                        mine
                    }
                };
                if mine {
                    add(link, &mut links);
                }
            }
        }
        let ids: Vec<String> = self.connection.select_bound(
            "SELECT DISTINCT id FROM live_blocks WHERE revision=? AND id IS NOT NULL",
        )?(this.revision.as_str())?;
        for id in ids {
            for link in self.links_where("l.block_key=?", &id)? {
                if self
                    .resolve_block_page(&link.link.target)?
                    .map(|page| page.revision)
                    == Some(this.revision.clone())
                {
                    add(link, &mut links);
                }
            }
        }
        self.sort_links(&mut links);
        Ok(links)
    }

    /// The reference `mentions`: every page link naming `title`, whether or not a page exists.
    pub fn mentions(&self, title: &str) -> Result<Vec<Backlink>> {
        self.refresh()?;
        let mut links = self.links_where("l.page_key=?", &page_key(title))?;
        self.sort_links(&mut links);
        Ok(links)
    }

    /// The reference `taggedBlocks`: one entry per tagged block, ordered by source path.
    pub fn tagged_blocks(&self, tag: &str) -> Result<Vec<TaggedBlock>> {
        self.refresh()?;
        type Row = (PageRow, (String, u32, Option<String>, String));
        let rows: Vec<Row> = self.connection.select_bound(&format!(
            "SELECT {PAGE_COLUMNS},t.tag,t.start,t.block_id,t.snippet FROM live_tagged t JOIN live_pages p ON p.revision=t.revision WHERE p.deleted=0 AND t.tag_key=?"
        ))?(tag_key(tag))?;
        let mut tagged = rows
            .into_iter()
            .map(|(page, (tag, from, block_id, snippet))| {
                Ok(TaggedBlock {
                    source: page_ref(page)?,
                    tag: IndexedTag {
                        tag,
                        from,
                        block_id,
                        snippet,
                    },
                })
            })
            .collect::<Result<Vec<_>>>()?;
        tagged.sort_by(|a, b| {
            self.compare_paths(&a.source.path, &b.source.path)
                .then(a.tag.from.cmp(&b.tag.from))
        });
        Ok(tagged)
    }

    /// The reference `tagNames`: each tag key with its block count, labelled by its first use.
    /// Committed tags are tallied once per committed change: tag completion asks on every
    /// keystroke, and tallying all 198,000 tagged blocks at 5x took 125 ms each time. Open pages'
    /// overlays are folded in on each call.
    pub fn tag_names(&self) -> Result<Vec<TagCount>> {
        self.refresh()?;
        let mut tallies = (*self.committed_tags()?).clone();
        let shadowed: Vec<String> = self
            .connection
            .select("SELECT page FROM temp.overlay_pages")?()?;
        let mut stale = BTreeSet::new();
        for page in &shadowed {
            let keys: Vec<String> = self.connection.select_bound(
                "SELECT t.tag_key FROM main.tagged t JOIN main.pages p ON p.revision=t.revision
                 WHERE p.deleted=0 AND p.page=?",
            )?(page.as_str())?;
            for key in keys {
                if let Some(tally) = tallies.get_mut(&key) {
                    tally.count -= 1;
                    if tally
                        .first
                        .as_ref()
                        .is_some_and(|first| &first.page == page)
                    {
                        stale.insert(key);
                    }
                }
            }
        }
        for key in stale {
            let uses: Vec<TagUseRow> = self.connection.select_bound(
                "SELECT t.tag,p.path,t.start,p.page FROM main.tagged t JOIN main.pages p ON p.revision=t.revision
                 WHERE p.deleted=0 AND t.tag_key=? AND p.page NOT IN (SELECT page FROM temp.overlay_pages)",
            )?(key.as_str())?;
            let tally = tallies
                .get_mut(&key)
                .expect("stale keys come from the tallies");
            tally.first = None;
            uses.into_iter().for_each(|found| tally.consider(found));
        }
        let overlaid: Vec<(String, TagUseRow)> = self.connection.select(
            "SELECT t.tag_key,t.tag,p.path,t.start,p.page FROM temp.overlay_tagged t
             JOIN temp.overlay_pages p ON p.revision=t.revision WHERE p.deleted=0",
        )?()?;
        for (key, found) in overlaid {
            let tally = tallies.entry(key).or_default();
            tally.count += 1;
            tally.consider(found);
        }
        let mut names: Vec<_> = tallies
            .into_values()
            .filter_map(|tally| {
                Some(TagCount {
                    label: tally.first.filter(|_| tally.count > 0)?.label,
                    count: tally.count,
                })
            })
            .collect();
        names.sort_by(|a, b| self.collator.compare(&a.label, &b.label));
        Ok(names)
    }

    /// Saved tag tallies, kept current by recounting only the tags whose rows changed since the
    /// last call; triggers on `main.tagged` record them. Recounting every tag after each autosave
    /// read all 198,483 tagged blocks at 5x, 146 ms on the first call after typing committed.
    fn committed_tags(&self) -> Result<TagTallies> {
        let committed = self.committed.get();
        let mut cached = self.tag_tallies.borrow_mut();
        if let Some((at, tallies)) = &*cached
            && *at == committed
        {
            return Ok(tallies.clone());
        }
        let changed: Vec<String> =
            self.connection
                .select("DELETE FROM temp.tag_changes RETURNING tag_key")?()?;
        const TALLY: &str = "SELECT t.tag_key,t.tag,p.path,t.start,p.page FROM main.tagged t
             JOIN main.pages p ON p.revision=t.revision WHERE p.deleted=0";
        let tallies = match cached.take() {
            Some((_, tallies)) => {
                let mut tallies = std::rc::Rc::unwrap_or_clone(tallies);
                let mut uses = self
                    .connection
                    .select_bound(&format!("{TALLY} AND t.tag_key=?"))?;
                for key in changed {
                    let rows: Vec<(String, TagUseRow)> = uses(key.as_str())?;
                    self.count_rows(rows.len());
                    match tally(rows.into_iter().map(|(_, found)| found).collect()) {
                        Some(tally) => tallies.insert(key, tally),
                        None => tallies.remove(&key),
                    };
                }
                tallies
            }
            None => {
                let rows: Vec<(String, TagUseRow)> = self.connection.select(TALLY)?()?;
                self.count_rows(rows.len());
                let mut tallies: HashMap<String, TagTally> = HashMap::new();
                for (key, found) in rows {
                    let tally = tallies.entry(key).or_default();
                    tally.count += 1;
                    tally.consider(found);
                }
                tallies
            }
        };
        let tallies = std::rc::Rc::new(tallies);
        *cached = Some((committed, tallies.clone()));
        Ok(tallies)
    }

    /// Pages whose block or front matter tags this edit would change, ordered by path.
    pub fn tag_edit_pages(&self, retag: &Retag) -> Result<Vec<PageRef>> {
        self.refresh()?;
        let rows: Vec<(String, String)> = self.connection.select(
            "SELECT revision,tag_key FROM live_tagged UNION SELECT revision,tag FROM live_front_tags",
        )?()?;
        let touched: BTreeSet<String> = rows
            .into_iter()
            .filter(|(_, tag)| retag.touches(&tag_key(tag)))
            .map(|(revision, _)| revision)
            .collect();
        let mut pages = Vec::new();
        for revision in touched {
            pages.extend(self.pages_where("p.revision=?", &revision)?);
        }
        pages.sort_by(|a, b| self.compare_paths(&a.path, &b.path));
        Ok(pages)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SearchHit {
    pub source: PageRef,
    pub snippet: String,
    pub block_id: Option<String>,
    /// The block's UTF-16 start in its page, when the hit is one block.
    pub from: Option<u32>,
}

/// A shown row's position, page, snippet, and block ID.
type SnippetRow = (i64, PageRow, (String, Option<String>));

/// How many hits a search returns.
const SHOWN_HITS: usize = 40;

/// Where a search hit's row lives: saved rows rank before unsaved ones of equal rank.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Source {
    Saved,
    Overlay,
}

/// The scratch table shown rows are copied into for their snippets, inside a savepoint that is
/// always rolled back. It matches the search tables' columns and tokenizer, and snippets read
/// only the row itself, so they come out as the search tables would make them.
pub(super) const SNIPPETS: &str = "CREATE VIRTUAL TABLE temp.search_snippets USING fts5(revision UNINDEXED, title, text, block_id UNINDEXED, tokenize='unicode61')";

static TERM: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r"[\p{L}\p{N}_-]+").expect("the search term pattern is valid")
});

/// The reference search expression: every term as a quoted prefix, all required.
fn search_expression(query: &str) -> Option<String> {
    let terms: Vec<_> = TERM
        .find_iter(query)
        .map(|term| format!("\"{}\"*", term.as_str()))
        .collect();
    (!terms.is_empty()).then(|| terms.join(" AND "))
}

impl QueryIndex {
    /// The reference `search`: up to 40 block and title hits by bm25 rank, with 24-token snippets.
    /// An open page is searched in its unsaved text instead of its last save. Equal ranks keep
    /// saved rows first, then row order.
    ///
    /// Only the best-ranked rows are read and given snippets: making a snippet for every match
    /// took most of a 5x search, where "budget" matches 184,401 blocks. FTS5 cannot make snippets
    /// for a list of rows without expanding prefixes again per row, so the shown rows are copied
    /// into a scratch table, searched there, and rolled back.
    pub fn search(&self, query: &str) -> Result<Vec<SearchHit>> {
        self.refresh()?;
        let Some(expression) = search_expression(query) else {
            return Ok(Vec::new());
        };
        let mut hits = self.saved_hits(&expression)?;
        let overlaid: Vec<(i64, f64)> = self.connection.select_bound(
            "SELECT rowid,bm25(overlay_search) AS score FROM temp.overlay_search
             WHERE overlay_search MATCH ?1 ORDER BY score,rowid LIMIT ?2",
        )?((expression.as_str(), SHOWN_HITS))?;
        hits.extend(
            overlaid
                .into_iter()
                .map(|(row, score)| (score, Source::Overlay, row)),
        );
        hits.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)).then(a.2.cmp(&b.2)));
        hits.truncate(SHOWN_HITS);
        self.snippets(&expression, &hits)
    }

    /// The best-ranked saved rows of pages that are neither deleted nor open, as many as a search
    /// shows. Rows of open pages are skipped, so the first pass reads that many more, and reads
    /// again with a larger limit if skipped rows still left too few.
    fn saved_hits(&self, expression: &str) -> Result<Vec<(f64, Source, i64)>> {
        let hidden: Option<usize> = self.connection.select_row(
            "SELECT count(*) FROM search_rows WHERE revision IN
               (SELECT revision FROM main.pages WHERE page IN (SELECT page FROM temp.overlay_pages))",
        )?()?;
        let mut ranked = self.connection.select_bound(
            "SELECT rowid,bm25(search) AS score FROM search WHERE search MATCH ?1
             ORDER BY score,rowid LIMIT ?2",
        )?;
        let mut live = self.connection.select_row_bound(
            "SELECT 1 FROM search_rows s JOIN main.pages p ON p.revision=s.revision
             WHERE s.rowid=? AND p.deleted=0 AND p.page NOT IN (SELECT page FROM temp.overlay_pages)",
        )?;
        let mut limit = SHOWN_HITS + hidden.unwrap_or_default();
        loop {
            let rows: Vec<(i64, f64)> = ranked((expression, limit))?;
            self.count_rows(rows.len());
            let mut kept = Vec::new();
            for &(row, score) in &rows {
                if kept.len() == SHOWN_HITS {
                    break;
                }
                let found: Option<i64> = live(row)?;
                if found.is_some() {
                    kept.push((score, Source::Saved, row));
                }
            }
            if kept.len() == SHOWN_HITS || rows.len() < limit {
                return Ok(kept);
            }
            limit *= 4;
        }
    }

    /// Pages, block IDs, and snippets for the shown rows, in their order.
    fn snippets(&self, expression: &str, hits: &[(f64, Source, i64)]) -> Result<Vec<SearchHit>> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let rows = serde_json::to_string(
            &hits
                .iter()
                .map(|(_, source, row)| (*source == Source::Overlay, *row))
                .collect::<Vec<_>>(),
        )?;
        self.connection.exec("SAVEPOINT search_snippets")?()?;
        let found = (|| -> Result<Vec<SnippetRow>> {
            self.connection.exec_bound(
                "INSERT INTO temp.search_snippets(rowid,revision,title,text,block_id)
                 SELECT j.key,s.revision,s.title,s.text,s.block_id FROM json_each(?1) j
                   JOIN search_rows s ON s.rowid=j.value->>1 WHERE NOT j.value->>0
                 UNION ALL SELECT j.key,o.revision,o.title,o.text,o.block_id FROM json_each(?1) j
                   JOIN temp.overlay_search o ON o.rowid=j.value->>1 WHERE j.value->>0",
            )?(rows.as_str())?;
            self.connection.select_bound(&format!(
                "SELECT n.rowid,{PAGE_COLUMNS},snippet(search_snippets,2,'','',' … ',24),n.block_id
                 FROM temp.search_snippets n JOIN live_pages p ON p.revision=n.revision
                 WHERE search_snippets MATCH ?"
            ))?(expression)
        })();
        self.connection
            .exec("ROLLBACK TO search_snippets")
            .and_then(|mut rollback| rollback())
            .and_then(|()| self.connection.exec("RELEASE search_snippets")?())?;
        let mut found = found?;
        found.sort_by_key(|(position, ..)| *position);
        found
            .into_iter()
            .map(|(_, page, (snippet, block_id))| {
                Ok(SearchHit {
                    source: page_ref(page)?,
                    snippet,
                    block_id,
                    from: None,
                })
            })
            .collect()
    }

    /// Blocks whose text contains `query`, ignoring case, at most `per_page` from each page and
    /// `limit` in all, in page path and block order: the reference find palette's live hits. An
    /// open page is searched in its unsaved text instead of its last save.
    pub fn text_hits(&self, query: &str, per_page: usize, limit: usize) -> Result<Vec<SearchHit>> {
        self.refresh()?;
        let needle = query.to_lowercase();
        if needle.trim().is_empty() || per_page == 0 || limit == 0 {
            return Ok(Vec::new());
        }
        let (prefilter, exact) = sql_needle(&needle);
        let capped = if exact {
            format!("WHERE n<={per_page} ORDER BY path,ordinal LIMIT {limit}")
        } else {
            "ORDER BY path,ordinal".to_owned()
        };
        let rows: Vec<TextHitRow> =
            self.connection.select_bound(&format!(
                "SELECT revision,page,path,title,text,id,start FROM (
                   SELECT {PAGE_COLUMNS},b.text AS text,b.id AS id,b.start AS start,b.ordinal AS ordinal,
                     row_number() OVER (PARTITION BY p.revision ORDER BY b.ordinal) AS n
                   FROM live_blocks b JOIN live_pages p ON p.revision=b.revision
                   WHERE p.deleted=0 AND b.text!='' AND instr(lower(b.text),?1)>0
                 ) {capped}"
            ))?(prefilter.as_str())?;
        let mut counts: HashMap<String, usize> = HashMap::new();
        rows.into_iter()
            .filter(|(_, (text, _, _))| exact || text.to_lowercase().contains(&needle))
            .filter(|(page, _)| {
                let seen = counts.entry(page.0.clone()).or_default();
                *seen += 1;
                *seen <= per_page
            })
            .take(limit)
            .map(|(page, (snippet, block_id, from))| {
                Ok(SearchHit {
                    source: page_ref(page)?,
                    snippet,
                    block_id,
                    from: Some(from),
                })
            })
            .collect()
    }
}

/// A page and one block of it: its text, ID, and UTF-16 start.
type TextHitRow = (PageRow, (String, Option<String>, u32));

/// What SQLite can look for in `lower(text)` for the lowercased `needle`, and whether that test
/// is exact. SQLite folds only ASCII case, so a needle with other cased letters is narrowed to
/// its longest run of ASCII or caseless characters and checked again in Rust.
fn sql_needle(needle: &str) -> (String, bool) {
    let plain = |c: char| c.is_ascii() || c.to_lowercase().eq(c.to_uppercase());
    if needle.chars().all(plain) {
        return (needle.to_owned(), true);
    }
    let longest = needle
        .split(|c: char| !plain(c))
        .max_by_key(|run| run.chars().count())
        .unwrap_or_default();
    (longest.to_owned(), false)
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
