use super::NotebookStore;
use crate::sync::{
    format,
    json::{JsString, Json, ValueRef},
};
use anyhow::{Context as _, Result, ensure};
use quire_core::markdown::page_title_for_editor;
use serde::Serialize;
use sqlez::{bindable::Column, statement::Statement};

const VERSION: i64 = 1;
const INDEX_ERROR: &str = "The notebook metadata index is missing, damaged, or out of date. Rebuild the metadata index before querying pages; your revisions are preserved.";
const HEADS_ERROR: &str =
    "Notebook heads are inconsistent. Preserve the database and restore a verified backup.";

#[derive(Debug, Clone, Copy)]
pub struct MetadataLimits {
    pub pages: usize,
    pub bytes: usize,
}

impl Default for MetadataLimits {
    fn default() -> Self {
        Self {
            pages: 64,
            bytes: 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageMetadata {
    pub page: JsString,
    pub revision: String,
    pub path: JsString,
    pub title: JsString,
    pub generation: u64,
    pub created: u64,
    pub deleted: bool,
    pub heads: usize,
}

// Materialize only keys and sizes before the byte cutoff, never the title payloads.
const PAGE_QUERY: &str = "
WITH candidates AS MATERIALIZED (
    SELECT page FROM heads WHERE page>? GROUP BY page ORDER BY page LIMIT ?
), selected AS MATERIALIZED (
    SELECT page, (SELECT h.revision FROM heads h LEFT JOIN revisions rr ON rr.id=h.revision
      WHERE h.page=c.page ORDER BY rr.generation DESC,h.revision DESC LIMIT 1) AS revision
    FROM candidates c
), weighted AS (
    SELECT s.page,s.revision,
      octet_length(s.page)+256+
      coalesce(octet_length(r.path),0)+coalesce(octet_length(m.title),0) AS weight
    FROM selected s LEFT JOIN revisions r ON r.id=s.revision
    LEFT JOIN revision_metadata m ON m.revision=s.revision
), ranked AS (
    SELECT page,revision,row_number() OVER (ORDER BY page) AS position,
      sum(weight) OVER (ORDER BY page) AS used FROM weighted
), bounded AS (
    SELECT page,revision FROM ranked WHERE position=1 OR used<=?
)
SELECT b.page,r.id,r.path,m.title,m.version,r.body_hash,m.hash,r.generation,r.created,r.deleted,
  (SELECT count(*) FROM heads h WHERE h.page=b.page),
  (SELECT count(*) FROM heads h LEFT JOIN revisions rr ON rr.id=h.revision
    WHERE h.page=b.page AND (rr.id IS NULL OR rr.page<>h.page
      OR typeof(rr.generation)<>'integer' OR typeof(rr.created)<>'integer'
      OR typeof(rr.deleted)<>'integer' OR rr.deleted NOT IN (0,1)))
FROM bounded b LEFT JOIN revisions r ON r.id=b.revision
LEFT JOIN revision_metadata m ON m.revision=b.revision ORDER BY b.page";

const BACKFILL_QUERY: &str = "SELECT h.revision,r.body_hash,h.page,r.page,
  typeof(r.generation)='integer' AND typeof(r.created)='integer'
  AND typeof(r.deleted)='integer' AND r.deleted IN (0,1)
 FROM heads h LEFT JOIN revisions r ON r.id=h.revision
 WHERE h.revision>? ORDER BY h.revision LIMIT 128";

#[derive(Serialize)]
struct Digest<'a> {
    version: i64,
    revision: &'a str,
    page: &'a str,
    path: &'a str,
    body_hash: &'a str,
    generation: i64,
    created: i64,
    deleted: bool,
    title: &'a str,
}

impl Digest<'_> {
    fn hash(&self) -> Result<String> {
        Ok(format::sha256(serde_json::to_vec(self)?))
    }
}

fn read_string(text: &str) -> Result<JsString> {
    let json = Json::parse(text.as_bytes()).context(INDEX_ERROR)?;
    let string = json.root().as_js_string().context(INDEX_ERROR)?.clone();
    ensure!(string.to_json() == text, INDEX_ERROR);
    Ok(string)
}

struct MetadataRow {
    page: String,
    revision: Option<String>,
    path: Option<String>,
    title: Option<String>,
    version: Option<i64>,
    body_hash: Option<String>,
    hash: Option<String>,
    generation: Option<i64>,
    created: Option<i64>,
    deleted: Option<bool>,
    heads: i64,
    damaged_heads: i64,
}

impl Column for MetadataRow {
    fn column(statement: &mut Statement, start: i32) -> Result<(Self, i32)> {
        // sqlez supports ten tuple columns; decode two groups into the named row.
        let ((page, revision, path, title, version, body_hash), next) =
            Column::column(statement, start)?;
        let ((hash, generation, created, deleted, heads, damaged_heads), next) =
            Column::column(statement, next)?;
        Ok((
            Self {
                page,
                revision,
                path,
                title,
                version,
                body_hash,
                hash,
                generation,
                created,
                deleted,
                heads,
                damaged_heads,
            },
            next,
        ))
    }
}

impl MetadataRow {
    fn into_page(self) -> Result<PageMetadata> {
        let Self {
            page,
            revision,
            path,
            title,
            version,
            body_hash,
            hash,
            generation,
            created,
            deleted,
            heads,
            damaged_heads,
        } = self;
        ensure!(damaged_heads == 0 && heads > 0, HEADS_ERROR);
        let revision = revision.context(INDEX_ERROR)?;
        let path = path.context(INDEX_ERROR)?;
        let title = title.context(INDEX_ERROR)?;
        let version = version.context(INDEX_ERROR)?;
        let body_hash = body_hash.context(INDEX_ERROR)?;
        let generation = generation.context(INDEX_ERROR)?;
        let created = created.context(INDEX_ERROR)?;
        let deleted = deleted.context(INDEX_ERROR)?;
        let digest = Digest {
            version,
            revision: &revision,
            page: &page,
            path: &path,
            title: &title,
            body_hash: &body_hash,
            generation,
            created,
            deleted,
        };
        ensure!(
            version == VERSION && Some(digest.hash()?) == hash,
            INDEX_ERROR
        );
        Ok(PageMetadata {
            page: read_string(&page)?,
            path: read_string(&path)?,
            title: read_string(&title)?,
            revision,
            generation: generation.try_into().context(INDEX_ERROR)?,
            created: created.try_into().context(INDEX_ERROR)?,
            deleted,
            heads: heads.try_into().context(INDEX_ERROR)?,
        })
    }
}

impl NotebookStore {
    /// Selected heads in exclusive canonical page-identity order. Recovery is a separate overlay.
    /// Row/byte limits bound payloads; one oversized row can progress alone.
    pub fn page_metadata(
        &self,
        after: Option<&JsString>,
        limits: MetadataLimits,
    ) -> Result<Vec<PageMetadata>> {
        self.require_usable()?;
        ensure!(
            (1..=256).contains(&limits.pages) && (1..=8 * 1024 * 1024).contains(&limits.bytes),
            "Cannot query metadata with these limits. Request 1..256 pages and 1..8388608 bytes."
        );
        let query_error = "Cannot read page metadata. Preserve the database, check storage access and free space, then reopen the notebook. Use a verified backup if corruption is reported.";
        let mut query = self
            .connection
            .select_bound(PAGE_QUERY)
            .context(query_error)?;
        let rows: Vec<MetadataRow> = query((
            after.map(JsString::to_json).unwrap_or_default(),
            limits.pages as u32,
            limits.bytes as u32,
        ))
        .context(query_error)?;
        rows.into_iter().map(MetadataRow::into_page).collect()
    }

    pub(super) fn index_revision(&self, value: ValueRef<'_>, body_hash: &str) -> Result<()> {
        let revision = value.field("id").as_str().context(INDEX_ERROR)?;
        let page = value.field("page").canonical()?;
        let path = value.field("path").canonical()?;
        let title = page_title_for_editor(
            value.field("path").as_js_string().context(INDEX_ERROR)?,
            value.field("text").as_js_string().context(INDEX_ERROR)?,
        )
        .to_json();
        let digest = Digest {
            version: VERSION,
            revision,
            page: &page,
            path: &path,
            body_hash,
            title: &title,
            generation: value.field("generation").as_f64().context(INDEX_ERROR)? as i64,
            created: value.field("created").as_f64().context(INDEX_ERROR)? as i64,
            deleted: value.field("deleted").as_bool().context(INDEX_ERROR)?,
        };
        self.connection.exec_bound(
            "INSERT INTO revision_metadata(revision,version,title,hash) VALUES (?,?,?,?)",
        )?((revision, VERSION, title.as_str(), digest.hash()?))?;
        Ok(())
    }

    /// Rebuild current-head metadata transactionally. Historical bodies stay on demand.
    /// A corrupt required body aborts without changing the old index.
    pub fn rebuild_metadata(&self) -> Result<usize> {
        self.transaction(|| {
            self.connection.exec("DELETE FROM revision_metadata")?()?;
            self.backfill_metadata()
        }).context("Cannot rebuild page metadata. Preserve the database, correct the reported storage or corruption error, and retry.")
    }

    pub(super) fn backfill_metadata(&self) -> Result<usize> {
        let mut after = String::new();
        let mut count = 0;
        loop {
            type Row = (String, Option<String>, String, Option<String>, bool);
            let rows: Vec<Row> = self.connection.select_bound(BACKFILL_QUERY)?(after.as_str())?;
            if rows.is_empty() {
                return Ok(count);
            }
            for (id, hash, page, revision_page, valid_types) in &rows {
                ensure!(
                    *valid_types && revision_page.as_deref() == Some(page.as_str()),
                    HEADS_ERROR
                );
                let hash = hash.as_deref().context(INDEX_ERROR)?;
                let record = self.get(id)?.context(INDEX_ERROR)?;
                self.index_revision(record.root(), hash)?;
                count += 1;
            }
            after = rows.last().unwrap().0.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_plans_seek_keys_and_size_titles_without_loading_payloads_or_bodies() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
        let query = format!("EXPLAIN QUERY PLAN {PAGE_QUERY}");
        let plan: Vec<(i64, i64, i64, String)> =
            store.connection.select_bound(&query)?(("", 64u32, 1024u32))?;
        assert!(
            plan.iter()
                .any(|(_, _, _, detail)| detail.contains("SEARCH heads")
                    && detail.contains("page>?")),
            "{plan:?}"
        );
        assert!(
            !plan.iter().any(|(_, _, _, detail)| detail == "SCAN r"
                || detail.starts_with("SCAN r ")
                || detail == "SCAN rr"
                || detail.starts_with("SCAN rr ")),
            "{plan:?}"
        );
        let plan: Vec<(i64, i64, i64, String)> =
            store
                .connection
                .select_bound(&format!("EXPLAIN QUERY PLAN {BACKFILL_QUERY}"))?("")?;
        assert!(
            plan.iter()
                .any(|(_, _, _, detail)| detail.contains("heads_by_revision")
                    && detail.contains("revision>?")),
            "{plan:?}"
        );
        type Op = (
            i64,
            String,
            i64,
            i64,
            i64,
            Option<String>,
            i64,
            Option<String>,
        );
        let ops: Vec<Op> =
            store
                .connection
                .select_bound(&format!("EXPLAIN {PAGE_QUERY}"))?(("", 64u32, 1024u32))?;
        let metadata_root = store.connection.select_row::<i64>(
            "SELECT rootpage FROM sqlite_schema WHERE name='revision_metadata'",
        )?()?
        .unwrap();
        let revisions_root = store
            .connection
            .select_row::<i64>("SELECT rootpage FROM sqlite_schema WHERE name='revisions'")?(
        )?
        .unwrap();
        let cursors = |root| {
            ops.iter()
                .filter(|(_, op, _, page, _, _, _, _)| op == "OpenRead" && *page == root)
                .map(|(_, _, cursor, _, _, _, _, _)| *cursor)
                .collect::<Vec<_>>()
        };
        let metadata = cursors(metadata_root);
        let revisions = cursors(revisions_root);
        assert!(!metadata.is_empty() && !revisions.is_empty());
        assert!(
            ops.iter()
                .any(|(_, op, cursor, column, _, _, flags, _)| op == "Column"
                    && metadata.contains(cursor)
                    && *column == 2
                    && *flags & 0xc0 == 0xc0),
            "Title budgeting must use SQLite's header-only byte-length operation."
        );
        assert!(
            ops.iter()
                .any(|(_, op, cursor, column, _, _, flags, _)| op == "Column"
                    && revisions.contains(cursor)
                    && *column == 2
                    && *flags & 0xc0 == 0xc0),
            "Path budgeting must use SQLite's header-only byte-length operation."
        );
        assert!(
            !ops.iter()
                .any(|(_, op, cursor, column, _, _, _, _)| op == "Column"
                    && revisions.contains(cursor)
                    && *column == 8),
            "Metadata queries must never select revision bodies."
        );
        Ok(())
    }
}
