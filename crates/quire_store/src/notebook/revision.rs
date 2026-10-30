use super::{
    DraftStamp, NotebookStore,
    recovery::{pack, page_key, unpack, version},
};
use crate::sync::{
    format::{self, sha256},
    json::{Json, ValueRef},
};
use anyhow::{Context as _, Result, ensure};
use regex::bytes::Regex;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlez::connection::Connection;
use std::{
    collections::{BTreeSet, HashSet},
    sync::OnceLock,
};
use unicode_normalization::UnicodeNormalization;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cursor {
    pub seq: u32,
    pub hash: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Edit {
    pub page: String,
    pub base: Vec<String>,
    pub path: String,
    pub text: String,
    pub deleted: bool,
    pub created: u64,
    /// Identifies the editor stream and the matching recovery snapshot, when present.
    pub recovery: Option<DraftStamp>,
}
#[derive(Debug)]
pub struct SaveAck {
    pub revision: Json,
    pub heads: Vec<String>,
    pub conflict: bool,
    pub changed: bool,
    pub replaced: Option<String>,
    pub stamp: Option<DraftStamp>,
    /// The revision that merged this save with competing versions, when it left several.
    pub merged: Option<Json>,
    pub overlaps: usize,
}

#[derive(Debug)]
pub enum CheckedSave {
    Saved(SaveAck),
    HeadsChanged { heads: Vec<String> },
}

#[derive(Debug)]
pub enum CheckedSaves {
    Saved(Vec<SaveAck>),
    /// Nothing was written: this page's heads no longer equal its edit's base.
    HeadsChanged {
        page: String,
        heads: Vec<String>,
    },
    /// Nothing was written: a live page at `path` is inside the folder being deleted but was not
    /// chosen with it.
    Appeared {
        path: String,
    },
}
impl SaveAck {
    pub fn revision_id(&self) -> &str {
        self.revision
            .root()
            .field("id")
            .as_str()
            .expect("validated revision id")
    }
}

fn same_content(value: ValueRef<'_>, edit: &Edit) -> bool {
    value.field("page").as_str() == Some(edit.page.as_str())
        && value.field("path").as_str() == Some(edit.path.as_str())
        && value.field("text").as_str() == Some(edit.text.as_str())
        && value.field("deleted").as_bool() == Some(edit.deleted)
}

pub(super) fn referenced_attachments(text: &[u8]) -> Vec<String> {
    static REFERENCES: OnceLock<Regex> = OnceLock::new();
    let pattern = REFERENCES.get_or_init(|| {
        Regex::new(r"attachments/([0-9a-f]{64}(?:\.[a-z0-9]{1,10})?)").expect("attachment pattern")
    });
    pattern
        .captures_iter(text)
        .map(|capture| {
            std::str::from_utf8(capture.get(1).unwrap().as_bytes())
                .expect("ASCII attachment name")
                .to_owned()
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

impl NotebookStore {
    pub fn heads(&self, page: &str) -> Result<Vec<String>> {
        self.heads_key(&page_key(page))
    }
    pub(super) fn heads_key(&self, key: &str) -> Result<Vec<String>> {
        let rows = self.connection.select_bound::<_, (String, Option<String>)>("SELECT h.revision,r.page FROM heads h LEFT JOIN revisions r ON r.id=h.revision WHERE h.page=? ORDER BY r.generation DESC,h.revision DESC")?(key)?;
        rows.into_iter().map(|(id, page)| {
            ensure!(page.as_deref() == Some(key), "Notebook heads are inconsistent. Preserve the database and restore a verified backup.");
            Ok(id)
        }).collect()
    }
    pub fn outbox(&self) -> Result<Vec<String>> {
        self.connection
            .select("SELECT revision FROM outbox ORDER BY position")?()
    }
    pub fn cursor(&self, device: &str) -> Result<Option<Cursor>> {
        let row =
            self.connection.select_row_bound::<_, (i64, String)>(
                "SELECT seq,hash FROM cursors WHERE device=?",
            )?(device)?;
        row.map(|(seq, hash)| {
            ensure!(
                (1..=999_999_999).contains(&seq)
                    && hash.len() == 64
                    && format::attachment_hash(&hash).is_some(),
                "Notebook cursor is invalid. Preserve the database and reconcile the change log."
            );
            Ok(Cursor {
                seq: u32::try_from(seq)?,
                hash,
            })
        })
        .transpose()
    }

    /// Called by the publisher only after its folder scan confirms this exact cursor
    /// and finds no unrecorded batch. It must not be used to bypass reconciliation.
    pub fn mark_reconciled(&self, observed: Option<&Cursor>) -> Result<()> {
        self.transaction(|| {
            let count = self
                .connection
                .select_row::<i64>("SELECT COUNT(*) FROM publication")?()?;
            ensure!(
                count == Some(0) && self.cursor(&self.device)?.as_ref() == observed,
                "Publication changed during reconciliation. Scan this writer's folder again."
            );
            self.connection
                .exec("UPDATE meta SET uncertain=0 WHERE singleton=1")?()?;
            Ok(())
        })
    }

    pub fn get(&self, id: &str) -> Result<Option<Json>> {
        read_revision(&self.connection, "main", id)
    }
}

/// Reads and validates one revision from `schema` (the store's own or an attached read-only copy).
pub(crate) fn read_revision(
    connection: &Connection,
    schema: &str,
    id: &str,
) -> Result<Option<Json>> {
    type Row = (String, String, i64, i64, bool, String, String, Vec<u8>);
    let row = connection.select_row_bound::<_, Row>(&format!("SELECT id,page,generation,created,deleted,path,body_hash,body FROM {schema}.revisions WHERE id=?"))?(id)?;
    row.map(|(stored_id, page, generation, created, deleted, path, hash, body)| {
            let record = Json::parse(&unpack(&hash, &body)?)?;
            let value = record.root();
            format::validate_revision_ref(value)?;
            ensure!(value.field("id").as_str() == Some(id) && stored_id == id
                && value.field("page").canonical()? == page && value.field("path").canonical()? == path
                && value.field("generation").as_f64() == Some(generation as f64)
                && value.field("created").as_f64() == Some(created as f64)
                && value.field("deleted").canonical()? == if deleted { "true" } else { "false" },
                "Revision metadata does not match its body. Preserve the database and use a verified backup.");
            Ok(record)
        }).transpose()
}

impl NotebookStore {
    /// Returns only after all page, outbox, and recovery changes are committed.
    pub fn save(&self, edit: &Edit) -> Result<SaveAck> {
        Ok(self.save_many(std::slice::from_ref(edit))?.remove(0))
    }
    pub fn save_many(&self, edits: &[Edit]) -> Result<Vec<SaveAck>> {
        distinct_pages(edits)?;
        self.transaction(|| edits.iter().map(|edit| self.save_one(edit)).collect())
    }

    /// Saves every edit or none: each page's heads must still equal its edit's base, checked in
    /// the same write transaction as the saves.
    pub fn save_many_if_heads(&self, edits: &[Edit]) -> Result<CheckedSaves> {
        self.save_many_checked(edits, None)
    }

    /// Deletes every page in `edits` or none, as `save_many_if_heads` saves them. With `folder`,
    /// every edit must lie inside it, and the deletion also refuses when a live page inside it
    /// is not among the edits: someone added it after the pages were chosen. The tombstones keep
    /// each page's last text, so history stays.
    pub fn delete_if_heads(&self, edits: &[Edit], folder: Option<&str>) -> Result<CheckedSaves> {
        ensure!(
            edits.iter().all(|edit| edit.deleted),
            "A deletion contains a page edit that is not a deletion. Choose the pages again and retry."
        );
        if let Some(folder) = folder {
            ensure!(
                edits.iter().all(|edit| inside(&edit.path, folder)),
                "A page to delete is not in {folder}. Nothing was deleted; choose the folder again and retry."
            );
        }
        self.save_many_checked(edits, folder)
    }

    /// Live pages whose current path lies inside `folder`, as page identities and paths.
    pub fn pages_in_folder(&self, folder: &str) -> Result<Vec<(String, String)>> {
        let prefix: String = format!("{folder}/")
            .nfc()
            .collect::<String>()
            .to_lowercase();
        let rows = self.connection.select_bound::<_, (String, String)>(
            "SELECT DISTINCT r.page,r.path FROM heads h JOIN revisions r ON r.id=h.revision WHERE r.deleted=0 AND substr(r.path_key,1,?)=?",
        )?((prefix.chars().count() as i64, prefix.as_str()))?;
        let mut pages = Vec::new();
        for (page, path) in rows {
            let page: String = serde_json::from_str(&page)?;
            let path: String = serde_json::from_str(&path)?;
            if inside(&path, folder) {
                pages.push((page, path));
            }
        }
        pages.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        pages.dedup_by(|a, b| a.0 == b.0);
        Ok(pages)
    }

    fn save_many_checked(&self, edits: &[Edit], folder: Option<&str>) -> Result<CheckedSaves> {
        distinct_pages(edits)?;
        self.transaction(|| {
            if let Some(folder) = folder {
                let chosen: HashSet<&str> = edits.iter().map(|edit| edit.page.as_str()).collect();
                if let Some((_, path)) = self
                    .pages_in_folder(folder)?
                    .into_iter()
                    .find(|(page, _)| !chosen.contains(page.as_str()))
                {
                    return Ok(CheckedSaves::Appeared { path });
                }
            }
            for edit in edits {
                let heads = self.heads(&edit.page)?;
                if heads != edit.base {
                    return Ok(CheckedSaves::HeadsChanged {
                        page: edit.page.clone(),
                        heads,
                    });
                }
            }
            edits
                .iter()
                .map(|edit| self.save_one(edit))
                .collect::<Result<_>>()
                .map(CheckedSaves::Saved)
        })
    }

    /// Native editor save; resolving conflicts requires a separate explicit command.
    /// Head comparison and the save share the same write transaction.
    pub fn save_if_heads(&self, edit: &Edit, expected: &[String]) -> Result<CheckedSave> {
        ensure!(
            expected.len() <= 1 && edit.base == expected,
            "This save does not match one opened page version. Keep the buffer and review the page's current versions before saving."
        );
        self.transaction(|| {
            let heads = self.heads(&edit.page)?;
            if heads != expected {
                return Ok(CheckedSave::HeadsChanged { heads });
            }
            self.save_one(edit).map(CheckedSave::Saved)
        })
    }

    fn save_one(&self, edit: &Edit) -> Result<SaveAck> {
        ensure!(
            !edit.page.is_empty(),
            "Cannot save a page without an identity. Reopen the page and retry."
        );
        let input_hash = sha256(format::canonical(
            &json!({"page":edit.page,"base":edit.base,"path":edit.path,"text":edit.text,"deleted":edit.deleted}),
        )?);
        let mut base = edit.base.clone();
        let mut replay = None;
        if let Some(stamp) = &edit.recovery {
            let incoming_version = version(stamp)?;
            type EditorRow = (String, i64, String, Option<String>, String);
            let prior = self.connection.select_row_bound::<_, EditorRow>(
                "SELECT session,version,input_hash,revision,base FROM editor_saves WHERE page=? AND session=?",
            )?((page_key(&edit.page), stamp.session.as_str()))?;
            if let Some((session, saved_version, hash, revision, original)) = prior
                && session == stamp.session
            {
                ensure!(
                    incoming_version >= saved_version,
                    "A stale editor snapshot cannot replace a later save. Save the current buffer instead."
                );
                if incoming_version == saved_version {
                    let id = revision
                        .as_deref()
                        .context("This snapshot was replaced. Reopen the page before retrying.")?;
                    let saved = self.get(id)?.context("Saved editor revision is missing. Preserve the database and reopen the page.")?;
                    ensure!(
                        input_hash == hash
                            || (edit.base == [id] && same_content(saved.root(), edit)),
                        "An editor version was reused for different save contents. Preserve the buffer and reopen the page."
                    );
                    replay = Some(saved);
                } else if let Some(revision) = revision {
                    let original: Vec<String> = serde_json::from_str(&original)?;
                    if base == original || base == [revision.clone()] {
                        base = vec![revision];
                    }
                }
            }
            if let Some(draft) = self.draft(&edit.page)?
                && draft.stamp == *stamp
            {
                ensure!(
                    draft.text == edit.text && draft.path == edit.path,
                    "Save contents do not match their recovery snapshot. Save the current buffer again."
                );
            }
        }
        let heads = self.heads(&edit.page)?;
        let previous = if base.len() == 1 {
            self.get(&base[0])?
        } else {
            None
        };
        let unchanged = previous
            .as_ref()
            .filter(|record| same_content(record.root(), edit));
        let is_replay = replay.is_some();
        let mut merged = None;
        let (record, heads, changed, replaced) = if let Some(record) =
            replay.or_else(|| unchanged.cloned())
        {
            (record, heads, false, None)
        } else {
            let certain = self
                .connection
                .select_row::<i32>("SELECT uncertain FROM meta WHERE singleton=1")?(
            )? == Some(0);
            let pending = if let Some(previous) = &previous {
                let id = previous.root().field("id").as_str().unwrap();
                self.connection.select_row_bound::<_, i32>(
                    "SELECT 1 FROM outbox WHERE revision=? AND batch IS NULL",
                )?(id)?
                .is_some()
            } else {
                false
            };
            let observed = if pending {
                self.connection.select_row_bound::<_, i32>(
                    "SELECT 1 FROM observed_revisions WHERE revision=?",
                )?(base[0].as_str())?
                .is_some()
            } else {
                false
            };
            let replaces = previous.as_ref().filter(|previous| {
                certain
                    && pending
                    && !observed
                    && heads == base
                    && previous.root().field("device").as_str() == Some(self.device.as_str())
                    && previous
                        .root()
                        .field("parents")
                        .elements()
                        .map(Iterator::count)
                        == Some(1)
            });
            let mut parents: Vec<String> = if let Some(previous) = replaces {
                previous
                    .root()
                    .field("parents")
                    .elements()
                    .unwrap()
                    .map(|id| id.as_str().unwrap().to_owned())
                    .collect()
            } else {
                base.clone()
            };
            parents.sort();
            parents.dedup();
            let mut generation = 0;
            for parent in &parents {
                let parent = self.get(parent)?.context("This page's previous version is missing. Preserve the buffer and reopen the page.")?;
                ensure!(
                    parent.root().field("page").as_str() == Some(edit.page.as_str()),
                    "A save names a parent from another page. Reopen the page and retry."
                );
                generation =
                    generation.max(parent.root().field("generation").as_f64().unwrap() as u64 + 1);
            }
            let mut fields = json!({"page":edit.page,"parents":parents,"generation":generation,"path":edit.path,"text":edit.text,"deleted":edit.deleted,"created":edit.created,"device":self.device});
            fields["id"] = format::revision_id(&fields)?.into();
            let record = Json::from(&fields);
            format::validate_revision_ref(record.root())?;
            self.ensure_attachments(record.root().field("text"))?;
            let replaced =
                replaces.map(|previous| previous.root().field("id").as_str().unwrap().to_owned());
            if let Some(id) = &replaced {
                self.connection
                    .exec_bound("DELETE FROM outbox WHERE revision=?")?(
                    id.as_str()
                )?;
                self.connection
                    .exec_bound("DELETE FROM heads WHERE revision=?")?(id.as_str())?;
                self.connection
                    .exec_bound("DELETE FROM revisions WHERE id=?")?(id.as_str())?;
            }
            let id = record.root().field("id").as_str().unwrap();
            let changed = self.insert_revision(record.root())?;
            if changed {
                self.connection
                    .exec_bound("INSERT INTO outbox(revision) VALUES (?)")?(id)?;
                merged = self.merge_page(&page_key(&edit.page), edit.created)?;
            }
            let heads = self.heads(&edit.page)?;
            (record, heads, changed, replaced)
        };
        let id = record.root().field("id").as_str().unwrap();
        ensure!(
            record.root().field("page").as_str() == Some(edit.page.as_str()),
            "A save names a revision from another page. Keep the buffer and reopen the page before retrying."
        );
        if let Some(stamp) = &edit.recovery {
            // Keep the original request so retries from either side of its acknowledgment agree.
            if !is_replay {
                self.connection.exec_bound("INSERT INTO editor_saves(page,session,version,input_hash,revision,base) VALUES (?,?,?,?,?,?) ON CONFLICT(page,session) DO UPDATE SET version=excluded.version,input_hash=excluded.input_hash,revision=excluded.revision,base=excluded.base")?((page_key(&edit.page), stamp.session.as_str(), version(stamp)?, input_hash, id, serde_json::to_string(&edit.base)?))?;
            }
            if let Some(mut draft) = self.draft(&edit.page)? {
                if draft.stamp.session == stamp.session && draft.stamp.version <= stamp.version {
                    self.connection.exec_bound(
                        "DELETE FROM drafts WHERE page=? AND session=? AND version<=?",
                    )?((
                        page_key(&edit.page),
                        stamp.session.as_str(),
                        version(stamp)?,
                    ))?;
                } else if draft.stamp.session == stamp.session
                    && draft.stamp.version > stamp.version
                    && (draft.base == edit.base || draft.base == base)
                {
                    draft.base = vec![id.into()];
                    self.write_draft(&draft)?;
                }
            }
        }
        Ok(SaveAck {
            conflict: heads.len() > 1,
            revision: record,
            heads,
            changed,
            replaced,
            stamp: edit.recovery.clone(),
            overlaps: merged.as_ref().map_or(0, |merged| merged.overlaps),
            merged: merged.map(|merged| merged.revision),
        })
    }

    pub(super) fn insert_revision(&self, value: ValueRef<'_>) -> Result<bool> {
        format::validate_revision_ref(value)?;
        let id = value.field("id").as_str().unwrap();
        if self.get(id)?.is_some() {
            return Ok(false);
        }
        let page = value.field("page").canonical()?;
        let parents: Vec<_> = value
            .field("parents")
            .elements()
            .unwrap()
            .map(|p| p.as_str().unwrap())
            .collect();
        let mut generation = 0;
        for parent in &parents {
            let parent = self
                .get(parent)?
                .context("A revision's parent is missing. Reconcile the notebook and retry.")?;
            ensure!(
                parent.root().field("page").canonical()? == page,
                "A revision's parent belongs to another page. Preserve the change file and retry reconciliation."
            );
            generation =
                generation.max(parent.root().field("generation").as_f64().unwrap() as u64 + 1);
        }
        ensure!(
            value.field("generation").as_f64() == Some(generation as f64),
            "A revision has an incorrect generation. Preserve the change file and retry reconciliation."
        );
        let text = value.canonical()?;
        let (hash, body) = pack(text.as_bytes())?;
        let path = value.field("path");
        let path_key = path
            .as_str()
            .map(|path| path.nfc().collect::<String>().to_lowercase());
        self.connection.exec_bound("INSERT INTO revisions(id,page,path,path_key,generation,created,deleted,body_hash,body) VALUES (?,?,?,?,?,?,?,?,?)")?((id, page.as_str(), path.canonical()?, path_key, generation, value.field("created").as_f64().unwrap() as u64, value.field("deleted").canonical()? == "true", hash.as_str(), body))?;
        self.index_revision(value, &hash)?;
        for parent in parents {
            self.connection
                .exec_bound("INSERT INTO parents(child,parent) VALUES (?,?)")?((
                id, parent,
            ))?;
            self.connection
                .exec_bound("DELETE FROM heads WHERE page=? AND revision=?")?((
                page.as_str(),
                parent,
            ))?;
        }
        self.connection
            .exec_bound("INSERT INTO heads(page,revision) VALUES (?,?)")?((
            page.as_str(),
            id,
        ))?;
        Ok(true)
    }

    pub(super) fn ensure_attachments(&self, text: ValueRef<'_>) -> Result<()> {
        for name in referenced_attachments(
            text.as_js_string()
                .context("Revision text is not a string.")?
                .as_bytes(),
        ) {
            self.require_attachment(&name)?;
        }
        Ok(())
    }
}

fn inside(path: &str, folder: &str) -> bool {
    path.strip_prefix(folder)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn distinct_pages(edits: &[Edit]) -> Result<()> {
    let mut pages = HashSet::new();
    ensure!(
        edits.iter().all(|edit| pages.insert(&edit.page)),
        "A save contains several edits for one page. Combine them before saving."
    );
    Ok(())
}
