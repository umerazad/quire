use super::{APPLICATION_ID, NotebookStore, SCHEMA_VERSION, revision::referenced_attachments};
use crate::schema;
use anyhow::{Context as _, Result, ensure};

impl NotebookStore {
    /// Offline rollback of the additive upgrades to schema 1. Stop other storage workers first.
    /// Consumes this handle and refuses to discard metadata unavailable in schema 1.
    pub fn rollback_schema_upgrade(self) -> Result<()> {
        self.transaction(|| {
            let version = self.preflight()?;
            ensure!(version >= 2, "This rollback requires an upgraded schema. Preserve the database and use the matching Quire version.");
            if version >= 3 {
                let exposed = self.connection.select_row::<i64>("SELECT COUNT(*) FROM observed_revisions r JOIN outbox o ON o.revision=r.revision WHERE o.batch IS NULL")?()?;
                ensure!(exposed == Some(0), "Peer-observed saves are still pending publication. Publish them durably before rolling back this schema upgrade.");
                self.connection.exec("DROP TABLE observed_revisions")?()?;
                self.connection.exec("DELETE FROM schema_history WHERE version=3")?()?;
            }
            let requests = self.connection.select_row::<i64>("SELECT COUNT(*) FROM attachment_needs")?()?;
            ensure!(requests == Some(0), "Attachment requests are still pending. Finish their verified delivery before rolling back this schema upgrade.");
            let history = self.connection.select_row::<i64>("SELECT COUNT(*) FROM batch_hashes b LEFT JOIN cursors c ON c.device=b.device AND c.seq=b.seq AND c.hash=b.hash WHERE c.device IS NULL")?()?;
            ensure!(history == Some(0), "Historical batch hashes cannot be represented in schema 1. Keep the upgraded database and use a compatible Quire version.");
            self.connection.exec("DROP TABLE attachment_needs")?()?;
            self.connection.exec("DROP TABLE batch_hashes")?()?;
            self.connection.exec("DELETE FROM schema_history WHERE version=2")?()?;
            self.connection.exec("PRAGMA user_version=1")?()?;
            ensure!(self.preflight()? == 1, "Schema rollback verification failed. Preserve the database and restore a verified backup.");
            Ok(())
        })
    }

    /// Runs inside the open transaction, after exact schema and identity preflight.
    pub(super) fn migrate(&self, from: i32) -> Result<()> {
        for version in from + 1..=SCHEMA_VERSION {
            for (_, sql) in schema::migration(version) {
                self.connection.exec(sql)?()?;
            }
            if version == 1 {
                self.connection.exec_bound(
                    "INSERT INTO meta(singleton,notebook,device,uncertain) VALUES (1,?,?,1)",
                )?((self.notebook.as_str(), self.device.as_str()))?;
                self.connection
                    .exec(&format!("PRAGMA application_id={APPLICATION_ID}"))?()?;
            } else if version == 2 {
                self.connection.exec(
                    "INSERT INTO batch_hashes(device,seq,hash) SELECT device,seq,hash FROM cursors",
                )?()?;
                let mut after = String::new();
                loop {
                    let ids: Vec<String> = self.connection.select_bound(
                        "SELECT id FROM revisions WHERE id>? ORDER BY id LIMIT 128",
                    )?(after.as_str())?;
                    if ids.is_empty() {
                        break;
                    }
                    for id in &ids {
                        let record = self
                            .get(id)?
                            .context("A legacy revision disappeared during migration. Preserve the database and restore a verified backup.")?;
                        for name in referenced_attachments(
                            record
                                .root()
                                .field("text")
                                .as_js_string()
                                .unwrap()
                                .as_bytes(),
                        ) {
                            self.queue_attachment(&name)?;
                        }
                    }
                    after = ids.last().unwrap().clone();
                }
            } else if version == 3 {
                // Older stores did not record peer observation. Protect every pending save.
                self.connection.exec(
                    "INSERT INTO observed_revisions(revision) SELECT revision FROM outbox",
                )?()?;
            }
            self.connection
                .exec_bound("INSERT INTO schema_history(version,checksum) VALUES (?,?)")?(
                (
                version,
                schema::checksum(version),
            )
            )?;
            self.connection
                .exec(&format!("PRAGMA user_version={version}"))?()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        database,
        notebook::{Draft, DraftStamp, Edit},
        sync::{format, json::Json},
    };
    use std::{cell::Cell, collections::BTreeSet, path::Path};

    fn legacy_store(path: &Path) -> Result<NotebookStore> {
        assert_eq!(
            schema::checksum(1),
            "92d718278b41f834c594bbce9683f9af1a02017500cb3c9ae9a71e7e45fdb25f",
            "schema 1 must remain byte-compatible with the committed storage format"
        );
        let connection = database::open_writable(path)?;
        for (_, sql) in schema::SCHEMA_V1 {
            connection.exec(sql)?()?;
        }
        connection.exec_bound("INSERT INTO schema_history(version,checksum) VALUES (1,?)")?(
            schema::checksum(1),
        )?;
        connection.exec(
            "INSERT INTO meta(singleton,notebook,device,uncertain) VALUES (1,'nb','office',1)",
        )?()?;
        connection.exec(&format!("PRAGMA application_id={APPLICATION_ID}"))?()?;
        connection.exec("PRAGMA user_version=1")?()?;
        Ok(NotebookStore {
            connection,
            notebook: "nb".into(),
            device: "office".into(),
            poisoned: Cell::new(false),
            directory: path.parent().unwrap().into(),
        })
    }

    fn edit(base: Vec<String>, text: &str) -> Edit {
        Edit {
            page: "p-one".into(),
            base,
            path: "A.md".into(),
            text: text.into(),
            created: 1,
            deleted: false,
            recovery: None,
        }
    }

    fn schema_two_store(path: &Path) -> Result<NotebookStore> {
        assert_eq!(
            schema::checksum(2),
            "0d2e8502c075f2879626b7b1149b32cab02f4d74f748e10650d84158c4293f76"
        );
        let store = legacy_store(path)?;
        for (_, sql) in schema::SCHEMA_V2 {
            store.connection.exec(sql)?()?;
        }
        store
            .connection
            .exec_bound("INSERT INTO schema_history(version,checksum) VALUES (2,?)")?(
            schema::checksum(2),
        )?;
        store.connection.exec("PRAGMA user_version=2")?()?;
        Ok(store)
    }

    #[test]
    fn schema_two_upgrade_protects_existing_unselected_saves_and_refuses_unsafe_rollback()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.sqlite");
        let store = schema_two_store(&path)?;
        let saved = store.save(&edit(vec![], "legacy pending save"))?;
        drop(store);
        let store = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(
            store
                .connection
                .select::<String>("SELECT revision FROM observed_revisions")?()?,
            [saved.revision_id()]
        );
        assert!(store.rollback_schema_upgrade().is_err());
        let store = NotebookStore::open(&path, "nb", "office")?;
        store.mark_reconciled(None)?;
        let next = store.save(&edit(vec![saved.revision_id().into()], "new contents"))?;
        assert!(next.replaced.is_none());
        assert_eq!(store.outbox()?, [saved.revision_id(), next.revision_id()]);
        assert_eq!(
            store.get(saved.revision_id())?.unwrap().canonical()?,
            saved.revision.canonical()?
        );
        let pending = store.prepare_publication(2)?.unwrap();
        store.rollback_schema_upgrade()?;
        let store = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(store.pending_publication()?, Some(pending));
        assert_eq!(store.outbox()?, [saved.revision_id(), next.revision_id()]);
        assert_eq!(
            store
                .connection
                .select_row::<i32>("SELECT COUNT(*) FROM observed_revisions")?()?,
            Some(2)
        );
        Ok(())
    }

    #[test]
    fn failed_schema_two_upgrade_rolls_back_observation_backfill_and_can_retry() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.sqlite");
        let store = schema_two_store(&path)?;
        let saved = store.save(&edit(vec![], "keep pending"))?;
        store.connection.exec("CREATE TEMP TRIGGER fail_upgrade BEFORE INSERT ON schema_history WHEN NEW.version=3 BEGIN SELECT RAISE(ABORT,'injected schema history failure'); END")?()?;
        assert!(store.transaction(|| store.migrate(2)).is_err());
        assert_eq!(store.preflight()?, 2);
        assert_eq!(
            store.connection.select_row::<i32>(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name='observed_revisions'"
            )?()?,
            Some(0)
        );
        assert_eq!(store.outbox()?, [saved.revision_id()]);
        assert_eq!(
            store.get(saved.revision_id())?.unwrap().canonical()?,
            saved.revision.canonical()?
        );
        drop(store);
        let store = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(
            store
                .connection
                .select::<String>("SELECT revision FROM observed_revisions")?()?,
            [saved.revision_id()]
        );
        Ok(())
    }

    #[test]
    fn populated_schema_one_upgrade_preserves_local_state_and_pages_attachment_reconstruction()
    -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.sqlite");
        let store = legacy_store(&path)?;
        let first = store.save(&edit(vec![], "first مرحبا 🙂"))?;
        store.mark_reconciled(None)?;
        let pending = store.prepare_publication(100)?.unwrap();
        let second = store.save(&edit(vec![first.revision_id().into()], "later save"))?;
        let draft = Draft {
            page: "p-one".into(),
            path: "A.md".into(),
            text: "unsaved میٹنگ".into(),
            base: vec![second.revision_id().into()],
            created: 2,
            stamp: DraftStamp {
                session: "editor".into(),
                version: 1,
            },
        };
        store.put_draft(&draft, None)?;
        let known = "a".repeat(64);
        store
            .connection
            .exec_bound("INSERT INTO cursors(device,seq,hash) VALUES ('peer',7,?)")?(
            known.as_str(),
        )?;
        let mut expected = BTreeSet::new();
        let mut last_record = None;
        store.transaction(|| {
            for index in 0..130 {
                let name = format::attachment_file_name(format!("image {index}").as_bytes(), "a.png");
                let fields = Json::parse(format!(r#"{{"page":"p-{index}","path":"P{index}.md","text":"attachments/{name} \ud800","parents":[],"generation":0,"deleted":false,"created":1,"device":"peer","future":"\udfff"}}"#).as_bytes())?;
                let id = format::revision_id(&fields)?;
                let fields = fields.canonical()?;
                let record = Json::parse(format!("{{\"id\":\"{id}\",{}", &fields[1..]).as_bytes())?;
                store.insert_revision(record.root())?;
                expected.insert(name);
                last_record = Some((id, record.canonical()?));
            }
            Ok(())
        })?;
        let cached = format::attachment_file_name(b"image 0", "a.png");
        std::fs::create_dir(store.directory.join("attachments"))?;
        std::fs::write(
            store.directory.join("attachments").join(&cached),
            b"image 0",
        )?;
        store
            .connection
            .exec_bound("INSERT INTO attachments(name,hash,size) VALUES (?,?,?)")?((
            cached.as_str(),
            format::sha256(b"image 0"),
            7i64,
        ))?;
        drop(store);
        let store = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(store.preflight()?, SCHEMA_VERSION);
        assert_eq!(store.pending_publication()?, Some(pending.clone()));
        assert_eq!(store.outbox()?, [first.revision_id(), second.revision_id()]);
        assert_eq!(store.draft("p-one")?, Some(draft));
        store.require_attachment(&cached)?;
        assert_eq!(store.batch_hash("peer", 7)?, Some(known));
        assert!(
            store.batch_hash("peer", 6)?.is_none(),
            "unknown history must not be invented"
        );
        assert_eq!(
            store.pending_attachments(None, 2000)?,
            expected.into_iter().collect::<Vec<_>>()
        );
        let (id, record) = last_record.unwrap();
        assert_eq!(store.get(&id)?.unwrap().canonical()?, record);
        assert_eq!(
            store.get(first.revision_id())?.unwrap().canonical()?,
            first.revision.canonical()?
        );
        assert_eq!(
            store.get(second.revision_id())?.unwrap().canonical()?,
            second.revision.canonical()?
        );
        drop(store);
        let reopened = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(reopened.pending_publication()?, Some(pending));
        assert_eq!(
            reopened
                .connection
                .select_row::<i32>("SELECT COUNT(*) FROM schema_history")?()?,
            Some(SCHEMA_VERSION)
        );
        Ok(())
    }

    #[test]
    fn sqlite_full_mid_migration_leaves_schema_one_intact_and_can_retry_after_reopen() -> Result<()>
    {
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.sqlite");
        let store = legacy_store(&path)?;
        let saved = store.save(&edit(vec![], "acknowledged"))?;
        let pages = store.connection.select_row::<i64>("PRAGMA page_count")?()?.unwrap();
        store
            .connection
            .exec(&format!("PRAGMA max_page_count={}", pages + 2))?()?;
        let error = store
            .transaction(|| store.migrate(store.preflight()?))
            .unwrap_err();
        assert!(
            format!("{error:#}").to_lowercase().contains("full"),
            "{error:#}"
        );
        assert_eq!(store.preflight()?, 1);
        assert_eq!(
            store.get(saved.revision_id())?.unwrap().canonical()?,
            saved.revision.canonical()?
        );
        assert_eq!(store.connection.select_row::<i32>("SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('batch_hashes','attachment_needs')")?()?, Some(0));
        drop(store);
        let reopened = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(reopened.preflight()?, SCHEMA_VERSION);
        assert_eq!(reopened.outbox()?, [saved.revision_id()]);
        Ok(())
    }

    #[test]
    fn damaged_legacy_body_aborts_upgrade_without_repairing_or_discarding_it() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.sqlite");
        let store = legacy_store(&path)?;
        let saved = store.save(&edit(vec![], "keep original"))?;
        store
            .connection
            .exec("UPDATE revisions SET body_hash='damaged'")?()?;
        let before = store
            .connection
            .select_row::<(String, Vec<u8>)>("SELECT body_hash,body FROM revisions")?(
        )?
        .unwrap();
        drop(store);
        assert!(NotebookStore::open(&path, "nb", "office").is_err());
        let connection = database::open_writable(&path)?;
        assert_eq!(
            connection.select_row::<i32>("PRAGMA user_version")?()?,
            Some(1)
        );
        assert_eq!(
            connection.select_row::<(String, Vec<u8>)>("SELECT body_hash,body FROM revisions")?()?
                .unwrap(),
            before
        );
        assert_eq!(
            connection.select::<String>("SELECT revision FROM outbox")?()?,
            [saved.revision_id()]
        );
        assert_eq!(connection.select_row::<i32>("SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('batch_hashes','attachment_needs')")?()?, Some(0));
        Ok(())
    }

    #[test]
    fn reversible_upgrade_preserves_local_work_through_down_and_up() -> Result<()> {
        let root = tempfile::tempdir()?;
        let path = root.path().join("local.sqlite");
        let store = legacy_store(&path)?;
        let saved = store.save(&edit(vec![], "keep local work"))?;
        store.mark_reconciled(None)?;
        let pending = store.prepare_publication(100)?.unwrap();
        drop(store);
        NotebookStore::open(&path, "nb", "office")?.rollback_schema_upgrade()?;
        let connection = database::open_writable(&path)?;
        assert_eq!(
            connection.select_row::<i32>("PRAGMA user_version")?()?,
            Some(1)
        );
        drop(connection);
        let store = NotebookStore::open(&path, "nb", "office")?;
        assert_eq!(
            store.get(saved.revision_id())?.unwrap().canonical()?,
            saved.revision.canonical()?
        );
        assert_eq!(store.outbox()?, [saved.revision_id()]);
        assert_eq!(store.pending_publication()?, Some(pending));
        Ok(())
    }

    #[test]
    fn rollback_refuses_to_lose_attachment_requests_or_noncurrent_batch_hashes() -> Result<()> {
        for with_request in [true, false] {
            let root = tempfile::tempdir()?;
            let path = root.path().join("local.sqlite");
            let store = NotebookStore::open(&path, "nb", "office")?;
            let name = format::attachment_file_name(b"late", "a.bin");
            store.transaction(|| {
                if with_request {
                    store.queue_attachment(&name)?;
                } else {
                    store.record_batch_hash("peer", 1, &"a".repeat(64))?;
                    store.record_batch_hash("peer", 2, &"b".repeat(64))?;
                    store
                        .connection
                        .exec_bound("INSERT INTO cursors(device,seq,hash) VALUES ('peer',2,?)")?(
                        "b".repeat(64),
                    )?;
                }
                Ok(())
            })?;
            assert!(store.rollback_schema_upgrade().is_err());
            let store = NotebookStore::open(&path, "nb", "office")?;
            assert_eq!(store.preflight()?, SCHEMA_VERSION);
            if with_request {
                assert_eq!(store.pending_attachments(None, 100)?, [name]);
            } else {
                assert_eq!(store.batch_hash("peer", 1)?, Some("a".repeat(64)));
                assert_eq!(store.cursor("peer")?.unwrap().seq, 2);
            }
        }
        Ok(())
    }
}
