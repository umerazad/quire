use super::NotebookStore;
use crate::sync::format;
use anyhow::{Result, ensure};

impl NotebookStore {
    /// Returns at most 2,000 requests after an exclusive filename cursor.
    pub fn pending_attachments(&self, after: Option<&str>, limit: usize) -> Result<Vec<String>> {
        let names: Vec<String> = self.connection.select_bound(
            "SELECT name FROM attachment_needs WHERE name>? ORDER BY name LIMIT ?",
        )?((after.unwrap_or(""), limit.min(2000) as u32))?;
        ensure!(
            names
                .iter()
                .all(|name| format::attachment_hash(name).is_some()),
            "A queued attachment name is damaged. Preserve the database and restore a verified backup before downloading files."
        );
        Ok(names)
    }

    pub fn batch_hash(&self, device: &str, seq: u32) -> Result<Option<String>> {
        let hash: Option<String> = self
            .connection
            .select_row_bound("SELECT hash FROM batch_hashes WHERE device=? AND seq=?")?(
            (
            device, seq,
        )
        )?;
        ensure!(
            hash.as_ref()
                .is_none_or(|hash| hash.len() == 64 && format::attachment_hash(hash).is_some()),
            "Stored batch history is damaged. Preserve the database and restore a verified backup before syncing."
        );
        Ok(hash)
    }

    pub(super) fn record_batch_hash(&self, device: &str, seq: u32, hash: &str) -> Result<()> {
        ensure!(
            (1..=999_999_999).contains(&seq)
                && hash.len() == 64
                && format::attachment_hash(hash).is_some(),
            "Cannot record invalid batch history. Preserve the change file and retry reconciliation."
        );
        ensure!(
            self.batch_hash(device, seq)?
                .is_none_or(|prior| prior == hash),
            "A device has conflicting historical batches. Preserve both copies and restore a distinct writer identity before syncing."
        );
        self.connection
            .exec_bound("INSERT OR IGNORE INTO batch_hashes(device,seq,hash) VALUES (?,?,?)")?(
            (
            device, seq, hash,
        )
        )?;
        Ok(())
    }

    pub(super) fn queue_attachment(&self, name: &str) -> Result<()> {
        ensure!(
            format::attachment_hash(name).is_some(),
            "Cannot queue an invalid attachment name. Preserve the change file and retry reconciliation."
        );
        self.connection
            .exec_bound("INSERT OR IGNORE INTO attachment_needs(name) VALUES (?)")?(name)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queued_requests_are_paged_without_starving_later_names() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        store.transaction(|| {
            for index in 0..2003 {
                store.queue_attachment(&format!("{index:064x}.bin"))?;
            }
            Ok(())
        })?;
        assert!(store.pending_attachments(None, 0)?.is_empty());
        let first = store.pending_attachments(None, usize::MAX)?;
        assert_eq!(first.len(), 2000);
        let second = store.pending_attachments(first.last().map(String::as_str), usize::MAX)?;
        assert_eq!(second.len(), 3);
        assert!(
            store
                .pending_attachments(second.last().map(String::as_str), 100)?
                .is_empty()
        );
        assert_eq!(
            store.pending_attachments(None, 2000)?,
            first,
            "reading requests must not acknowledge missing files"
        );
        Ok(())
    }

    #[test]
    fn catalog_failure_retains_the_request_until_verified_file_retry_commits() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let data = b"late attachment";
        let name = format::attachment_file_name(data, "a.bin");
        store.transaction(|| store.queue_attachment(&name))?;
        store.connection.exec("CREATE TEMP TRIGGER fail_delivery BEFORE DELETE ON attachment_needs BEGIN SELECT RAISE(ABORT,'injected delivery commit failure'); END")?()?;
        assert!(store.cache_attachment(&name, data).is_err());
        assert_eq!(
            store.pending_attachments(None, 100)?,
            std::slice::from_ref(&name)
        );
        assert_eq!(
            std::fs::read(store.directory.join("attachments").join(&name))?,
            data
        );
        assert_eq!(
            store
                .connection
                .select_row::<i32>("SELECT COUNT(*) FROM attachments")?()?,
            Some(0)
        );
        store.connection.exec("DROP TRIGGER fail_delivery")?()?;
        store.cache_attachment(&name, data)?;
        assert!(store.pending_attachments(None, 100)?.is_empty());
        Ok(())
    }
}
