use super::{
    Cursor, NotebookStore, PendingPublication, attachment,
    folder::{read_ordinary, verify_notebook},
};
use crate::{
    durable_files,
    sync::format::{self, Expected, Kind},
};
use anyhow::{Context as _, Result, ensure};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Write},
    path::Path,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicationAck {
    pub name: String,
    pub cursor: Cursor,
    pub revisions: usize,
    pub adopted: bool,
}

fn scan_writer(
    log: &Path,
    notebook: &str,
    device: &str,
    cursor: Option<&Cursor>,
    pending: Option<&PendingPublication>,
) -> Result<bool> {
    let mut names = BTreeMap::new();
    for entry in fs::read_dir(log)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(parsed) = format::parse_name(name, Kind::Batch) else {
            continue;
        };
        ensure!(
            parsed.seq > 0 && names.insert(parsed.seq, name.to_owned()).is_none(),
            "This writer has conflicting change files. Keep the folder unchanged and restore a distinct device identity before publishing."
        );
        ensure!(
            entry.file_type()?.is_file(),
            "A change file is not an ordinary file. Restore its original bytes before publishing."
        );
    }
    let at = cursor.map_or(0, |cursor| cursor.seq);
    if let Some(cursor) = cursor {
        let name = names.get(&cursor.seq).context("The last acknowledged change file is missing. Wait for sync or restore it before publishing.")?;
        let data = read_ordinary(&log.join(name), format::MAX_BATCH_BYTES + 1024 * 1024)?;
        let decoded = format::decode(name, &data, Kind::Batch, Expected { notebook, device })?;
        ensure!(
            decoded.hash == cursor.hash,
            "This writer's recorded change file has a different hash. Preserve both copies and restore a distinct device identity before publishing."
        );
    }
    let mut adopted = false;
    for (seq, name) in names.range((std::ops::Bound::Excluded(at), std::ops::Bound::Unbounded)) {
        let selected = pending.filter(|pending| pending.seq == *seq && pending.name == *name)
            .context("This writer has an unrecorded or conflicting change file. Keep it unchanged and import or reconcile its original writer before publishing.")?;
        let data = read_ordinary(&log.join(name), selected.data.len())?;
        ensure!(
            data == selected.data,
            "The pending change file differs from its frozen bytes. Preserve the file and wait for a complete verified copy before retrying."
        );
        adopted = true;
    }
    Ok(adopted)
}

impl NotebookStore {
    /// Publishes one frozen batch to an existing format 1 notebook folder.
    /// Acknowledges this local folder only, never cloud or peer delivery.
    pub fn publish_to(&self, root: &Path, created: u64) -> Result<Option<PublicationAck>> {
        self.publish_matching(root, created, None)
    }

    pub(super) fn adopt_written_publication(
        &self,
        root: &Path,
        expected: &PendingPublication,
    ) -> Result<PublicationAck> {
        self.publish_matching(root, 0, Some(expected))?.context(
            "The observed publication changed. Rescan this writer's folder before retrying.",
        )
    }

    fn publish_matching(
        &self,
        root: &Path,
        created: u64,
        expected: Option<&PendingPublication>,
    ) -> Result<Option<PublicationAck>> {
        self.require_reconciliation()?;
        let result = self.publish_to_inner(root, created, expected).context("Cannot publish notebook changes. Keep the locally saved versions and retry after resolving the folder or delivery error.");
        if let Err(error) = result {
            if let Err(state_error) = self.require_reconciliation() {
                return Err(error).context(format!("Publication's uncertain state could not be recorded ({state_error:#}). Preserve the database and reopen it before retrying."));
            }
            return Err(error);
        }
        result
    }

    pub(super) fn require_reconciliation(&self) -> Result<()> {
        self.transaction(|| {
            self.connection
                .exec("UPDATE meta SET uncertain=1 WHERE singleton=1")?()?;
            Ok(())
        })
    }

    fn publish_to_inner(
        &self,
        root: &Path,
        created: u64,
        expected: Option<&PendingPublication>,
    ) -> Result<Option<PublicationAck>> {
        verify_notebook(root, &self.notebook, &self.device)?;
        let devices = durable_files::directory_below(root, "devices")?;
        let writer = durable_files::directory_below(&devices, &self.device)?;
        let log = durable_files::directory_below(&writer, "log")?;
        let cursor = self.cursor(&self.device)?;
        let mut pending = self.pending_publication()?;
        ensure!(
            expected.is_none_or(|expected| pending.as_ref() == Some(expected)),
            "The frozen publication changed during rescan. Keep local saves and rescan the writer before retrying."
        );
        let adopted = scan_writer(
            &log,
            &self.notebook,
            &self.device,
            cursor.as_ref(),
            pending.as_ref(),
        )?;
        ensure!(
            expected.is_none() || adopted,
            "The observed publication file is no longer present. Keep local saves and retry after delivery finishes."
        );
        if pending.is_none() {
            self.mark_reconciled(cursor.as_ref())?;
            pending = self.prepare_publication(created)?;
        }
        let Some(pending) = pending else {
            return Ok(None);
        };
        let batch = format::decode(
            &pending.name,
            &pending.data,
            Kind::Batch,
            Expected {
                notebook: &self.notebook,
                device: &self.device,
            },
        )?;
        let names = batch.value.root().field("attachments").elements().unwrap();
        if names.len() > 0 {
            let directory = durable_files::directory_below(root, "attachments")?;
            for name in names {
                let name = name.as_str().unwrap();
                self.require_attachment(name)?;
                let source = self.directory.join("attachments").join(name);
                durable_files::install(
                    &directory.join(name),
                    |file| {
                        io::copy(&mut File::open(&source)?, file).context("Cannot copy a published attachment. Keep its local source and check folder permissions and space before retrying.")?;
                        Ok(())
                    },
                    |path| attachment::verify_file(path, name),
                )?;
            }
        }
        durable_files::install(
            &log.join(&pending.name),
            |file| {
                file.write_all(&pending.data).context("Cannot write a change file. Keep the local saved revision and check folder permissions and space before retrying.")
            },
            |path| {
                ensure!(
                    read_ordinary(path, pending.data.len())? == pending.data,
                    "The existing change file differs from its frozen publication. Preserve it and reconcile this writer before retrying."
                );
                Ok(())
            },
        )?;
        verify_notebook(root, &self.notebook, &self.device)?;
        scan_writer(
            &log,
            &self.notebook,
            &self.device,
            cursor.as_ref(),
            Some(&pending),
        )?;
        let cursor = self.complete_publication(&pending)?;
        Ok(Some(PublicationAck {
            name: pending.name,
            cursor,
            revisions: pending.revisions.len(),
            adopted,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notebook::Edit;

    #[test]
    fn adoption_requires_an_existing_file_and_the_exact_frozen_selection() -> Result<()> {
        let root = tempfile::tempdir()?;
        let folder = root.path().join("notebook");
        fs::create_dir(&folder)?;
        fs::write(
            folder.join("quire-notebook.json"),
            br#"{"format":1,"id":"nb","created":1}"#,
        )?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let edit = Edit {
            page: "p".into(),
            base: vec![],
            path: "A.md".into(),
            text: "one".into(),
            created: 1,
            deleted: false,
            recovery: None,
        };
        let first = store.save(&edit)?;
        store.mark_reconciled(None)?;
        let pending = store.prepare_publication(100)?.unwrap();
        assert!(store.adopt_written_publication(&folder, &pending).is_err());
        let log = folder.join("devices/office/log");
        assert_eq!(fs::read_dir(&log)?.count(), 0);
        assert_eq!(store.pending_publication()?, Some(pending.clone()));
        fs::write(log.join(&pending.name), &pending.data)?;
        store.publish_to(&folder, 200)?;
        let second = store.save(&Edit {
            base: vec![first.revision_id().into()],
            text: "two".into(),
            ..edit
        })?;
        let selected = store.prepare_publication(300)?.unwrap();
        assert!(store.adopt_written_publication(&folder, &pending).is_err());
        assert_eq!(store.pending_publication()?, Some(selected));
        assert_eq!(store.outbox()?, [second.revision_id()]);
        assert_eq!(store.cursor("office")?.unwrap().seq, 1);
        assert_eq!(fs::read_dir(&log)?.count(), 1);
        Ok(())
    }
}
