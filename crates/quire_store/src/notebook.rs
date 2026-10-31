use crate::{database, schema};
use anyhow::{Context as _, Result, bail, ensure};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use sqlez::connection::Connection;
use std::{
    cell::{Cell, RefCell},
    path::{Path, PathBuf},
};

mod attachment;
mod census;
mod checkout;
mod delivery;
mod folder;
mod history;
mod import;
mod markdown_import;
mod merge;
mod metadata;
mod migration;
mod publication;
mod publisher;
mod recovery;
mod restored;
mod revision;
pub use census::StoreCensus;
pub use checkout::PageCheckout;
pub use folder::{RescanCursor, RescanLimits, RescanProblem, RescanReport, RescanWait};
pub use history::{SavedVersion, VERSION_UNAVAILABLE};
pub use import::{BatchImport, ImportedBatch, ImportedPage};
pub(crate) use markdown_import::pages_size;
pub use markdown_import::{FolderImported, SourceFiles};
pub use merge::{MergeFailure, MergeReport, MergedPage};
pub(crate) use metadata::revision_metadata;
pub use metadata::{MetadataLimits, PageMetadata};
pub use publication::PendingPublication;
pub use publisher::PublicationAck;
pub(crate) use recovery::unpack;
pub use recovery::{Draft, DraftStamp};
pub(crate) use restored::adopt_restored;
pub(crate) use revision::read_revision;
pub use revision::{CheckedSave, CheckedSaves, Cursor, Edit, SaveAck};

pub use crate::schema::{APPLICATION_ID, SCHEMA_VERSION};

pub struct NotebookStore {
    connection: Connection,
    notebook: String,
    device: String,
    poisoned: Cell<bool>,
    directory: PathBuf,
    clock: Clock,
    /// Told whenever a save adds to the outbox, so a scheduler can publish it.
    outbox_watchers: RefCell<Vec<UnboundedSender<u64>>>,
    /// How many saves have added to the outbox since this store opened.
    saves: Cell<u64>,
}

/// What a writer's status reports about its publication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PublicationState {
    pub outbox: u64,
    pub last_seq: u64,
    /// The number carried by the last outbox message when this was read: a save with a larger
    /// number is not counted in `outbox` yet.
    pub saves: u64,
}

/// Milliseconds stamped on merge revisions that rescans make.
pub type Clock = Box<dyn Fn() -> u64>;

fn system_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

impl NotebookStore {
    pub fn open(path: &Path, notebook: &str, device: &str) -> Result<Self> {
        Self::open_with_clock(path, notebook, device, Box::new(system_clock))
    }

    /// Opens the store with the clock that stamps merges made by later rescans.
    pub fn open_with_clock(
        path: &Path,
        notebook: &str,
        device: &str,
        clock: Clock,
    ) -> Result<Self> {
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
            clock,
            outbox_watchers: RefCell::default(),
            saves: Cell::new(0),
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

    /// A numbered message after each save that adds to the outbox, until the receiver is dropped.
    /// Messages can arrive before the save commits; act on them with a later request to this
    /// store, whose [`PublicationState::saves`] says which saves it counted.
    pub fn watch_outbox(&self) -> UnboundedReceiver<u64> {
        let (sender, receiver) = unbounded();
        self.outbox_watchers.borrow_mut().push(sender);
        receiver
    }

    fn outbox_grew(&self) {
        let saves = self.saves.get() + 1;
        self.saves.set(saves);
        self.outbox_watchers
            .borrow_mut()
            .retain(|watcher| watcher.unbounded_send(saves).is_ok());
    }

    /// How many saves wait to publish and the last sequence this writer published.
    pub fn publication_state(&self) -> Result<PublicationState> {
        let outbox = self
            .connection
            .select_row::<i64>("SELECT COUNT(*) FROM outbox")?()?
        .unwrap_or(0);
        Ok(PublicationState {
            outbox: u64::try_from(outbox).unwrap_or(0),
            last_seq: self
                .cursor(&self.device)?
                .map_or(0, |cursor| u64::from(cursor.seq)),
            saves: self.saves.get(),
        })
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

/// What a chosen Quire notebook database is, read without writing to it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DatabaseIdentity {
    pub notebook: String,
    pub writer: String,
    /// The live page with the first path, to show when the notebook has no remembered page.
    pub first_page: Option<String>,
}

impl NotebookStore {
    /// Reads the notebook and writer identities of the database at `path` read-only, so a
    /// notebook chosen in a file dialog opens with its original identities.
    pub fn identity(path: &Path) -> Result<DatabaseIdentity> {
        let text = path.to_str().context(
            "The notebook database path is not valid Unicode. Move the notebook to a Unicode path and choose it again.",
        )?;
        ensure!(
            path.is_file(),
            "{} is not a Quire notebook database. Choose the notebook's local.sqlite file.",
            path.display()
        );
        let connection = Connection::open_memory(None);
        connection.exec_bound("ATTACH DATABASE ? AS chosen")?(format!(
            "file:{}?mode=ro",
            crate::index::uri_path(text)
        ))
        .with_context(|| {
            format!(
                "{} is not a readable Quire notebook database. Choose the notebook's local.sqlite file.",
                path.display()
            )
        })?;
        let application = connection
            .select_row::<i32>("PRAGMA chosen.application_id")
            .and_then(|mut read| read())
            .ok()
            .flatten();
        ensure!(
            application == Some(APPLICATION_ID),
            "{} is not a Quire notebook database. Choose the local.sqlite file of a Quire notebook.",
            path.display()
        );
        let (notebook, writer) = connection.select_row::<(String, String)>(
            "SELECT notebook,device FROM chosen.meta WHERE singleton=1",
        )?()?
        .with_context(|| {
            format!(
                "{} has no notebook identity. Open it once with --notebook-db and its identities, or choose another notebook.",
                path.display()
            )
        })?;
        let first_page = connection.select_row::<String>(
            "SELECT r.page FROM chosen.heads h JOIN chosen.revisions r ON r.id=h.revision WHERE r.deleted=0 ORDER BY r.path_key, r.path, r.page LIMIT 1",
        )?()?
        .map(|page| serde_json::from_str::<String>(&page))
        .transpose()
        .context("A page identity in the notebook database is damaged. Restore a verified backup.")?;
        Ok(DatabaseIdentity {
            notebook,
            writer,
            first_page,
        })
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

    #[test]
    fn identity_reads_a_chosen_database_without_writing_and_refuses_other_files() -> Result<()> {
        let root = tempfile::tempdir()?;
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "office")?;
        assert_eq!(
            NotebookStore::identity(&database)?,
            DatabaseIdentity {
                notebook: "nb".into(),
                writer: "office".into(),
                first_page: None,
            }
        );
        for (page, path, deleted) in [("p-b", "B.md", false), ("p-a", "A.md", true)] {
            store.save(&Edit {
                page: page.into(),
                base: vec![],
                path: path.into(),
                text: "text".into(),
                deleted,
                created: 1,
                recovery: None,
            })?;
        }
        drop(store);
        let before = std::fs::read(&database)?;
        let identity = NotebookStore::identity(&database)?;
        assert_eq!(identity.first_page.as_deref(), Some("p-b"));
        assert_eq!(std::fs::read(&database)?, before);
        let other = root.path().join("other.sqlite");
        Connection::open_file(other.to_str().unwrap()).exec("CREATE TABLE t(x)")?()?;
        let error = NotebookStore::identity(&other).unwrap_err();
        assert!(
            format!("{error:#}").contains("is not a Quire notebook database"),
            "{error:#}"
        );
        let error = NotebookStore::identity(&root.path().join("missing.sqlite")).unwrap_err();
        assert!(format!("{error:#}").contains("Choose the notebook's local.sqlite file"));
        Ok(())
    }
}
