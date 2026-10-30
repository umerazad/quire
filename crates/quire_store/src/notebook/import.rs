use super::{Cursor, NotebookStore};
use crate::sync::{
    format::{self, Expected, Kind},
    json::{JsString, ValueRef},
};
use anyhow::Result;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, PartialEq)]
pub enum BatchImport {
    Applied(ImportedBatch),
    Duplicate,
    WaitingForSequence {
        expected: u32,
    },
    MissingParents {
        ids: Vec<String>,
    },
    OwnPublicationPending,
    Rejected {
        reason: String,
        newer_format: Option<f64>,
    },
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImportedPage {
    pub page: JsString,
    pub heads: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ImportedBatch {
    pub cursor: Cursor,
    pub applied: usize,
    pub pages: Vec<ImportedPage>,
    /// Content-named dependencies that the folder adapter must check/download separately.
    pub attachments: Vec<String>,
}

fn rejected(reason: impl Into<String>) -> BatchImport {
    BatchImport::Rejected {
        reason: reason.into(),
        newer_format: None,
    }
}

impl NotebookStore {
    /// Applies an incoming batch and its cursor in one local transaction.
    /// Does not acknowledge folder durability or consume a frozen own publication.
    /// Known historical hashes detect forks; older unknown batches retain duplicate semantics.
    pub fn import_batch(&self, device: &str, name: &str, data: &[u8]) -> Result<BatchImport> {
        let decoded = match format::decode(
            name,
            data,
            Kind::Batch,
            Expected {
                notebook: &self.notebook,
                device,
            },
        ) {
            Ok(decoded) => decoded,
            Err(error) => {
                return Ok(BatchImport::Rejected {
                    reason: error.to_string(),
                    newer_format: error.newer_format(),
                });
            }
        };
        self.transaction(|| {
            let batch = decoded.value.root();
            let seq = batch.field("seq").as_f64().unwrap() as u32;
            let at = self.cursor(device)?;
            let next = at.as_ref().map_or(1, |cursor| cursor.seq + 1);
            if seq < next {
                let known = if let Some(cursor) = &at && cursor.seq == seq { Some(cursor.hash.clone()) } else { self.batch_hash(device, seq)? };
                if known.is_some_and(|hash| hash != decoded.hash) {
                    return Ok(rejected("This device has conflicting change files. Preserve both copies and restore a distinct writer identity before importing."));
                }
                return Ok(BatchImport::Duplicate);
            }
            if seq > next { return Ok(BatchImport::WaitingForSequence { expected: next }); }
            if batch.field("previous").as_str() != at.as_ref().map(|cursor| cursor.hash.as_str()) {
                return Ok(rejected("The device's change files do not form one chain. Keep them unchanged and retry after delivery finishes."));
            }
            if device == self.device {
                if let Some(pending) = self.pending_publication()? {
                    return Ok(if pending.name == name && pending.data == data {
                        BatchImport::OwnPublicationPending
                    } else {
                        rejected("This device has a different frozen publication. Preserve both copies and restore a distinct writer identity before importing.")
                    });
                }
                if !self.outbox()?.is_empty() {
                    return Ok(rejected("This device has unrecorded change files and local pending saves. Preserve both copies and restore a distinct writer identity before importing."));
                }
            }
            if let Some(outcome) = self.check_import_graph(batch)? { return Ok(outcome); }
            let mut applied = 0;
            let mut changed = BTreeMap::new();
            for revision in batch.field("revisions").elements().unwrap() {
                self.connection.exec_bound("INSERT OR IGNORE INTO observed_revisions(revision) SELECT revision FROM outbox WHERE revision=?")?(revision.field("id").as_str().unwrap())?;
                if self.insert_revision(revision)? {
                    applied += 1;
                    let page = revision.field("page");
                    changed.insert(page.as_js_string().unwrap().clone(), page.canonical()?);
                }
            }
            let pages = changed.into_iter().map(|(page, key)| {
                Ok(ImportedPage { page, heads: self.heads_key(&key)? })
            }).collect::<Result<Vec<_>>>()?;
            let cursor = Cursor { seq, hash: decoded.hash.clone() };
            self.record_batch_hash(device, seq, &cursor.hash)?;
            self.connection.exec_bound("INSERT INTO cursors(device,seq,hash) VALUES (?,?,?) ON CONFLICT(device) DO UPDATE SET seq=excluded.seq,hash=excluded.hash")?((device, seq, cursor.hash.as_str()))?;
            let attachments: Vec<String> = batch.field("attachments").elements().unwrap()
                .map(|name| name.as_str().unwrap().to_owned()).collect::<BTreeSet<_>>().into_iter().collect();
            for name in &attachments { self.queue_attachment(name)?; }
            Ok(BatchImport::Applied(ImportedBatch { cursor, applied, pages, attachments }))
        })
    }

    fn check_import_graph(&self, batch: ValueRef<'_>) -> Result<Option<BatchImport>> {
        let mut available = BTreeMap::<String, (String, u64)>::new();
        let mut missing = BTreeSet::new();
        let mut invalid = None;
        for revision in batch.field("revisions").elements().unwrap() {
            let page = revision.field("page").canonical()?;
            let mut generation = 0;
            for parent in revision.field("parents").elements().unwrap() {
                let id = parent.as_str().unwrap();
                let metadata = match available.get(id) {
                    Some(metadata) => Some(metadata.clone()),
                    None => self.connection.select_row_bound::<_, (String, u64)>(
                        "SELECT page,generation FROM revisions WHERE id=?",
                    )?(id)?,
                };
                if let Some((parent_page, parent_generation)) = metadata {
                    if parent_page != page {
                        invalid = Some(
                            "A revision's parent belongs to another page. Preserve the change file and restore its original graph.",
                        );
                    }
                    generation = generation.max(parent_generation + 1);
                } else {
                    missing.insert(id.to_owned());
                }
            }
            let actual_generation = revision.field("generation").as_f64().unwrap() as u64;
            if generation != actual_generation {
                invalid = Some(
                    "A revision has an incorrect generation. Preserve the change file and restore its original graph.",
                );
            }
            available.insert(
                revision.field("id").as_str().unwrap().to_owned(),
                (page, actual_generation),
            );
        }
        if !missing.is_empty() {
            return Ok(Some(BatchImport::MissingParents {
                ids: missing.into_iter().collect(),
            }));
        }
        Ok(invalid.map(rejected))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{notebook::Edit, sync::json::Json};
    use serde_json::json;

    #[test]
    fn failed_peer_import_does_not_leave_observation_metadata_and_retry_protects_the_parent()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
        store.mark_reconciled(None)?;
        let first = store.save(&Edit {
            page: "p".into(),
            base: vec![],
            path: "A.md".into(),
            text: "first".into(),
            created: 1,
            deleted: false,
            recovery: None,
        })?;
        let record = first.revision.canonical()?;
        let mut observed: serde_json::Value = serde_json::from_str(&record)?;
        observed["device"] = "peer".into();
        let batch = Json::from(
            &json!({"notebook":"nb","device":"peer","seq":1,"previous":null,"created":1,"revisions":[observed],"attachments":[]}),
        );
        let encoded = format::encode(&batch, Kind::Batch)?;
        store.connection.exec("CREATE TEMP TRIGGER fail_cursor BEFORE INSERT ON cursors BEGIN SELECT RAISE(ABORT,'injected cursor failure'); END")?()?;
        assert!(
            store
                .import_batch("peer", &encoded.name, &encoded.data)
                .is_err()
        );
        assert_eq!(
            store
                .connection
                .select_row::<i64>("SELECT COUNT(*) FROM observed_revisions")?()?,
            Some(0)
        );
        assert!(store.cursor("peer")?.is_none());
        assert_eq!(store.outbox()?, [first.revision_id()]);
        store.connection.exec("DROP TRIGGER fail_cursor")?()?;
        assert!(matches!(
            store.import_batch("peer", &encoded.name, &encoded.data)?,
            BatchImport::Applied(_)
        ));
        assert_eq!(
            store
                .connection
                .select::<String>("SELECT revision FROM observed_revisions")?()?,
            [first.revision_id()]
        );
        let next = store.save(&Edit {
            page: "p".into(),
            base: vec![first.revision_id().into()],
            path: "A.md".into(),
            text: "next".into(),
            created: 2,
            deleted: false,
            recovery: None,
        })?;
        assert!(next.replaced.is_none());
        assert_eq!(
            store.get(first.revision_id())?.unwrap().canonical()?,
            record
        );
        Ok(())
    }

    #[test]
    fn metadata_failure_rolls_back_the_entire_import_and_allows_retry() -> Result<()> {
        for table in ["cursors", "attachment_needs"] {
            let root = tempfile::tempdir()?;
            let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local")?;
            let mut revision = json!({"page":"p","text":"one","path":"A.md","deleted":false,"parents":[],"generation":0,"created":1,"device":"office"});
            revision["id"] = format::revision_id(&revision)?.into();
            let id = revision["id"].as_str().unwrap().to_owned();
            let name = format::attachment_file_name(b"late", "a.bin");
            let batch = Json::from(
                &json!({"notebook":"nb","device":"office","seq":1,"previous":null,"created":1,"revisions":[revision],"attachments":[name]}),
            );
            let encoded = format::encode(&batch, Kind::Batch)?;
            store.connection.exec(&format!("CREATE TEMP TRIGGER fail_import BEFORE INSERT ON {table} BEGIN SELECT RAISE(ABORT,'injected metadata failure'); END"))?()?;
            assert!(
                store
                    .import_batch("office", &encoded.name, &encoded.data)
                    .is_err()
            );
            assert!(store.get(&id)?.is_none());
            assert!(store.heads("p")?.is_empty());
            assert!(store.cursor("office")?.is_none());
            assert!(store.batch_hash("office", 1)?.is_none());
            assert!(store.pending_attachments(None, 100)?.is_empty());
            store.connection.exec("DROP TRIGGER fail_import")?()?;
            assert!(matches!(
                store.import_batch("office", &encoded.name, &encoded.data)?,
                BatchImport::Applied(_)
            ));
            assert_eq!(store.pending_attachments(None, 100)?, [name]);
        }
        Ok(())
    }
}
