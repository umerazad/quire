use crate::{database, schema};
use anyhow::{Context as _, Result, bail, ensure};
use sqlez::connection::Connection;
use std::{
    cell::Cell,
    path::{Path, PathBuf},
};

mod attachment;
mod checkout;
mod delivery;
mod folder;
mod import;
mod metadata;
mod migration;
mod publication;
mod publisher;
mod recovery;
mod revision;
pub use checkout::PageCheckout;
pub use folder::{RescanCursor, RescanLimits, RescanProblem, RescanReport, RescanWait};
pub use import::{BatchImport, ImportedBatch, ImportedPage};
pub(crate) use metadata::revision_metadata;
pub use metadata::{MetadataLimits, PageMetadata};
pub use publication::PendingPublication;
pub use publisher::PublicationAck;
pub use recovery::{Draft, DraftStamp};
pub(crate) use revision::read_revision;
pub use revision::{CheckedSave, Cursor, Edit, SaveAck};

pub use crate::schema::{APPLICATION_ID, SCHEMA_VERSION};

pub struct NotebookStore {
    connection: Connection,
    notebook: String,
    device: String,
    poisoned: Cell<bool>,
    directory: PathBuf,
}

impl NotebookStore {
    pub fn open(path: &Path, notebook: &str, device: &str) -> Result<Self> {
        ensure!(
            !notebook.is_empty() && !device.is_empty(),
            "Notebook and writer identities are required. Choose a notebook and retry."
        );
        let connection = database::open_unconfigured(path)?;
        let store = Self {
            connection,
            notebook: notebook.into(),
            device: device.into(),
            poisoned: Cell::new(false),
            directory: path
                .parent()
                .context("Database path has no parent folder.")?
                .to_owned(),
        };
        store.preflight().context("Cannot open this notebook database. Preserve it and use a compatible Quire version or a verified backup.")?;
        database::configure(&store.connection).context("Cannot configure durable notebook storage. Keep the editor contents and check the database folder's write permissions and available space.")?;
        store.transaction(|| {
            store.migrate(store.preflight()?)?;
            store
                .connection
                .exec("UPDATE meta SET uncertain=1 WHERE singleton=1")?()?;
            Ok(())
        })?;
        Ok(store)
    }

    pub fn notebook_id(&self) -> &str {
        &self.notebook
    }
    pub fn device_id(&self) -> &str {
        &self.device
    }

    fn preflight(&self) -> Result<i32> {
        let application = self.connection.select_row::<i32>("PRAGMA application_id")?()?
            .context("Database has no application identity.")?;
        let version = self.connection.select_row::<i32>("PRAGMA user_version")?()?
            .context("Database has no schema version.")?;
        if application == 0 && version == 0 {
            let count = self.connection.select_row::<i64>(
                "SELECT COUNT(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
            )?()?;
            ensure!(
                count == Some(0),
                "An unrecognized database already has data. Choose a new native database path."
            );
            return Ok(0);
        }
        ensure!(
            application == APPLICATION_ID,
            "This database belongs to another application. Choose the native Quire database."
        );
        ensure!(
            version <= SCHEMA_VERSION,
            "This database uses schema {version}. Update Quire before opening it."
        );
        ensure!(
            version >= 1,
            "This database has an unsupported schema. Preserve it and use a compatible Quire version."
        );
        let history = self.connection.select::<(i32, String)>(
            "SELECT version,checksum FROM schema_history ORDER BY version",
        )?()?;
        ensure!(
            history
                == (1..=version)
                    .map(|version| (version, schema::checksum(version)))
                    .collect::<Vec<_>>(),
            "Database schema history changed. Preserve the database and restore a verified backup."
        );
        let actual = self.connection.select::<(String, String)>(
            "SELECT name,sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY name",
        )?()?;
        let mut expected: Vec<_> = (1..=version)
            .flat_map(|version| schema::migration(version).iter())
            .map(|(name, sql)| (name.to_string(), sql.to_string()))
            .collect();
        expected.sort_by(|a, b| a.0.cmp(&b.0));
        ensure!(
            actual == expected,
            "Database tables or indexes differ from its recorded schema. Preserve it and use a verified backup."
        );
        let identity = self.connection.select_row::<(String, String)>(
            "SELECT notebook,device FROM meta WHERE singleton=1",
        )?()?
        .context("Database notebook identity is missing.")?;
        ensure!(
            identity.0 == self.notebook && identity.1 == self.device,
            "This database belongs to another notebook or writer. Open it on its original device or migrate its local work into a new database."
        );
        if version >= 2 {
            let mismatches = self.connection.select_row::<i64>("SELECT COUNT(*) FROM cursors c LEFT JOIN batch_hashes b ON b.device=c.device AND b.seq=c.seq WHERE b.hash IS NULL OR b.hash<>c.hash")?()?;
            ensure!(
                mismatches == Some(0),
                "Notebook cursors disagree with their recorded batch history. Preserve the database and restore a verified backup before syncing."
            );
        }
        Ok(version)
    }

    pub(super) fn require_usable(&self) -> Result<()> {
        ensure!(
            !self.poisoned.get(),
            "A previous commit is uncertain. Keep the editor contents and reopen the notebook before retrying."
        );
        Ok(())
    }

    fn transaction<T>(&self, run: impl FnOnce() -> Result<T>) -> Result<T> {
        self.transaction_with("BEGIN IMMEDIATE", run)
    }

    fn transaction_with<T>(&self, begin: &str, run: impl FnOnce() -> Result<T>) -> Result<T> {
        self.require_usable()?;
        self.connection.exec(begin).and_then(|mut begin| begin())
            .context("Cannot begin a notebook transaction. Keep the editor contents, check available space and write permissions, and retry after other writers finish.")?;
        match run() {
            Ok(value) => {
                if let Err(error) = self
                    .connection
                    .exec("COMMIT")
                    .and_then(|mut commit| commit())
                {
                    self.poisoned.set(true);
                    let _ = self
                        .connection
                        .exec("ROLLBACK")
                        .and_then(|mut rollback| rollback());
                    return Err(error).context("Notebook commit result is uncertain. Keep the editor contents and reopen the notebook before retrying.");
                }
                Ok(value)
            }
            Err(error) => {
                if let Err(rollback) = self
                    .connection
                    .exec("ROLLBACK")
                    .and_then(|mut rollback| rollback())
                {
                    self.poisoned.set(true);
                    bail!(
                        "Notebook transaction failed ({error}) and rollback failed ({rollback}). Keep the editor contents and reopen the notebook."
                    );
                }
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::format::sha256;

    #[test]
    fn sqlite_full_preserves_the_last_ack_and_recovery_copy() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let edit = Edit {
            page: "p-one".into(),
            base: vec![],
            path: "A.md".into(),
            text: "acknowledged".into(),
            deleted: false,
            created: 1,
            recovery: None,
        };
        let saved = store.save(&edit)?;
        let text = (0..3000)
            .map(|i| sha256(i.to_string()))
            .collect::<Vec<_>>()
            .join("\n");
        let draft = Draft {
            page: edit.page.clone(),
            path: edit.path.clone(),
            text: text.clone(),
            base: vec![saved.revision_id().into()],
            created: 2,
            stamp: DraftStamp {
                session: "editor".into(),
                version: 2,
            },
        };
        store.put_draft(&draft, None)?;
        let pages = store.connection.select_row::<i64>("PRAGMA page_count")?()?.unwrap();
        store
            .connection
            .exec(&format!("PRAGMA max_page_count={pages}"))?()?;
        let result = store.save(&Edit {
            text,
            base: draft.base.clone(),
            recovery: Some(draft.stamp.clone()),
            ..edit
        });
        let error = result.expect_err("the SQLite page limit must reject the save");
        assert!(
            format!("{error:#}").to_lowercase().contains("full"),
            "{error:#}"
        );
        assert_eq!(store.heads("p-one")?, vec![saved.revision_id()]);
        assert_eq!(store.outbox()?, vec![saved.revision_id()]);
        assert_eq!(store.draft("p-one")?, Some(draft));
        assert_eq!(
            store
                .get(saved.revision_id())?
                .unwrap()
                .root()
                .field("text")
                .as_str(),
            Some("acknowledged")
        );
        Ok(())
    }
}
