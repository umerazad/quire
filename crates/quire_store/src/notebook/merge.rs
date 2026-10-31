use super::NotebookStore;
use crate::sync::{
    format,
    json::{JsString, Json},
};
use anyhow::{Context as _, Result, ensure};
use quire_core::merge::merge_text;
use std::collections::HashMap;

/// A page whose competing versions could not be merged; its heads stay in place.
#[derive(Debug)]
pub struct MergeFailure {
    /// The page identity as stored in the heads table (canonical JSON).
    pub page: String,
    pub reason: String,
}

#[derive(Debug, Default)]
pub struct MergeReport {
    pub merged: Vec<MergedPage>,
    pub failed: Vec<MergeFailure>,
}

/// A merge revision that combined every head of one page.
#[derive(Debug)]
pub struct MergedPage {
    pub page: JsString,
    pub revision: Json,
    /// Places where both versions of the same lines were kept.
    pub overlaps: usize,
}

fn js_string(record: &Json, field: &str) -> Result<JsString> {
    record
        .root()
        .field(field)
        .as_js_string()
        .cloned()
        .with_context(|| format!("A stored revision has no {field} string. Preserve the database and restore a verified backup."))
}

fn id_of(record: &Json) -> &str {
    record
        .root()
        .field("id")
        .as_str()
        .expect("validated revision id")
}

impl NotebookStore {
    fn generation(&self, id: &str) -> Result<Option<i64>> {
        self.connection
            .select_row_bound::<_, i64>("SELECT generation FROM revisions WHERE id=?")?(id)
    }

    /// The newest revision both a and b descend from: the highest generation, then the smallest ID.
    /// Mirrors `mergeBase` in `rt` `graph.ts` using indexed graph rows instead of revision bodies.
    fn merge_base(&self, a: &str, b: &str) -> Result<Option<String>> {
        let mut marks = HashMap::from([(a.to_owned(), 1u8)]);
        marks.insert(b.to_owned(), u8::from(a == b) | 2);
        let mut pending = Vec::new();
        for id in [a, b] {
            if let Some(generation) = self.generation(id)? {
                pending.push((generation, id.to_owned()));
            }
        }
        while !pending.is_empty() {
            pending.sort_by(|p, q| q.0.cmp(&p.0).then_with(|| p.1.cmp(&q.1)));
            let (_, id) = pending.remove(0);
            let mark = marks[&id];
            if mark == 3 {
                return Ok(Some(id));
            }
            let parents = self
                .connection
                .select_bound::<_, String>("SELECT parent FROM parents WHERE child=?")?(
                id.as_str(),
            )?;
            for parent in parents {
                let before = marks.get(&parent).copied().unwrap_or(0);
                if before | mark == before {
                    continue;
                }
                marks.insert(parent.clone(), before | mark);
                if before == 0
                    && let Some(generation) = self.generation(&parent)?
                {
                    pending.push((generation, parent));
                }
            }
        }
        Ok(None)
    }

    /// Combines a page's heads into one revision when it has several. Mirrors `mergeHeads`
    /// in `rt` `graph.ts`: text merges against the newest common ancestor, an edit outlives a
    /// deletion, and of two moves the newer wins. Must run inside the caller's transaction.
    pub(super) fn merge_page(&self, key: &str, created: u64) -> Result<Option<MergedPage>> {
        let ids = self.heads_key(key)?;
        if ids.len() < 2 {
            return Ok(None);
        }
        let heads = ids
            .iter()
            .map(|id| {
                self.get(id)?.context(
                    "A page version is missing. Preserve the database and restore a verified backup.",
                )
            })
            .collect::<Result<Vec<_>>>()?;
        let deleted = |head: &Json| head.root().field("deleted").as_bool() == Some(true);
        let created_of = |head: &Json| head.root().field("created").as_f64().unwrap_or(0.0);
        let live: Vec<&Json> = heads.iter().filter(|head| !deleted(head)).collect();
        let mut order = if live.is_empty() {
            heads.iter().collect()
        } else {
            live.clone()
        };
        order.sort_by(|p, q| {
            created_of(p)
                .total_cmp(&created_of(q))
                .then_with(|| id_of(p).cmp(id_of(q)))
        });
        let first = order[0];
        let units = |record: &Json| -> Result<Vec<u16>> {
            Ok(js_string(record, "text")?.utf16_units().collect())
        };
        let mut text = units(first)?;
        let mut path = js_string(first, "path")?;
        let mut overlaps = 0;
        for head in &order[1..] {
            let base = self
                .merge_base(id_of(first), id_of(head))?
                .map(|id| {
                    self.get(&id)?.context(
                        "A common page version is missing. Preserve the database and restore a verified backup.",
                    )
                })
                .transpose()?;
            let merged = merge_text(
                &base.as_ref().map(units).transpose()?.unwrap_or_default(),
                &text,
                &units(head)?,
            );
            text = merged.text;
            overlaps += merged.overlaps;
            let head_path = js_string(head, "path")?;
            if base
                .as_ref()
                .map(|base| js_string(base, "path"))
                .transpose()?
                != Some(head_path.clone())
            {
                path = head_path;
            }
        }
        let page = js_string(first, "page")?;
        let mut parents = ids.clone();
        parents.sort();
        parents.dedup();
        let generation = parents
            .iter()
            .map(|id| self.generation(id)?.context("A page version is missing. Preserve the database and restore a verified backup."))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .max()
            .map_or(0, |generation| generation + 1);
        let fields = |id: Option<&str>| {
            format!(
                "{{\"created\":{created},\"deleted\":{},\"device\":{},\"generation\":{generation},{}\"page\":{},\"parents\":{},\"path\":{},\"text\":{}}}",
                live.is_empty(),
                serde_json::to_string(&self.device).expect("device identity"),
                id.map(|id| format!("\"id\":\"{id}\",")).unwrap_or_default(),
                page.to_json(),
                serde_json::to_string(&parents).expect("revision identities"),
                path.to_json(),
                JsString::from_utf16(&text).to_json(),
            )
        };
        let id = format::revision_id(Json::parse(fields(None).as_bytes())?)?;
        let revision = Json::parse(fields(Some(&id)).as_bytes())?;
        ensure!(
            self.insert_revision(revision.root())?,
            "Two versions of a page could not be combined because their merge already exists. Reopen the notebook to try again."
        );
        self.connection
            .exec_bound("INSERT INTO outbox(revision) VALUES (?)")?(id.as_str())?;
        self.outbox_grew();
        Ok(Some(MergedPage {
            page,
            revision,
            overlaps,
        }))
    }

    /// Merges every page that still has competing versions, such as heads kept by an older
    /// build or left by an import, in one transaction. A page whose merge fails keeps its
    /// heads and is reported. Call it only once this device's own change files are all
    /// imported, as a reconciled rescan does: a merge waiting in the outbox makes the
    /// importer refuse this device's later batches.
    pub fn merge_conflicts(&self) -> Result<MergeReport> {
        self.transaction(|| {
            let pages = self.connection.select::<String>(
                "SELECT page FROM heads GROUP BY page HAVING COUNT(*)>1 ORDER BY page",
            )?()?;
            let mut report = MergeReport::default();
            for page in pages {
                self.connection.exec("SAVEPOINT merge_page")?()?;
                match self.merge_page(&page, (self.clock)()) {
                    Ok(merged) => {
                        self.connection.exec("RELEASE merge_page")?()?;
                        report.merged.extend(merged);
                    }
                    Err(error) => {
                        self.connection.exec("ROLLBACK TO merge_page")?()?;
                        self.connection.exec("RELEASE merge_page")?()?;
                        report.failed.push(MergeFailure {
                            reason: format!("Competing versions of page {page} could not be merged ({error:#}). They stay as separate versions; preserve the database and reopen the notebook to retry."),
                            page,
                        });
                    }
                }
            }
            Ok(report)
        })
    }
}
