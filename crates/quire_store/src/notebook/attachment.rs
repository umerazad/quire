use super::NotebookStore;
use crate::{
    durable_files::{self, ordinary_directory},
    sync::format,
};
use anyhow::{Context as _, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
};

fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 65536];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub(super) fn verify_file(path: &Path, name: &str) -> Result<(String, i64)> {
    let expected = format::attachment_hash(name)
        .context("Invalid attachment name. Use a content-hashed attachment filename.")?;
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("Attachment {name} is missing. Download it before retrying."))?;
    ensure!(
        metadata.is_file(),
        "Attachment {name} is not an ordinary file. Restore its original bytes before retrying."
    );
    let hash = hash_file(path)?;
    ensure!(
        hash == expected,
        "Attachment {name} failed its hash check. Preserve the damaged file and restore its original bytes before retrying."
    );
    Ok((hash, i64::try_from(metadata.len())?))
}

impl NotebookStore {
    /// Returns a Markdown reference after file synchronization and catalog commit.
    /// Folder publication is a separate operation.
    pub fn attach(&self, data: &[u8], original: &str) -> Result<String> {
        let name = format::attachment_file_name(data, original);
        self.cache_attachment(&name, data)?;
        Ok(format!("attachments/{name}"))
    }

    /// Attaches an ordinary file by streaming it, so its size never sits in memory. Returns the
    /// same Markdown reference as [`Self::attach`].
    pub fn attach_file(&self, source: &Path, original: &str) -> Result<String> {
        self.require_usable()?;
        let source = &fs::canonicalize(source).with_context(|| {
            format!(
                "Cannot find {}. Check that the file still exists, then attach it again.",
                source.display()
            )
        })?;
        let metadata = fs::metadata(source).with_context(|| {
            format!(
                "Cannot read {}. Check that the file exists and is readable, then attach it again.",
                source.display()
            )
        })?;
        ensure!(
            metadata.is_file(),
            "{} is not an ordinary file. Choose a file, not a folder or device.",
            source.display()
        );
        let name = format::attachment_name_for_hash(&hash_file(source)?, original);
        self.cache_attachment_file(&name, source)?;
        Ok(format!("attachments/{name}"))
    }

    /// Verifies downloaded bytes against their protocol name before writing them.
    /// Existing damaged files are preserved; an unregistered valid file may be adopted.
    pub fn cache_attachment(&self, name: &str, data: &[u8]) -> Result<()> {
        self.require_usable()?;
        let expected = format::attachment_hash(name)
            .context("Invalid attachment name. Use a content-hashed attachment filename.")?;
        ensure!(
            format::sha256(data) == expected,
            "Attachment {name} does not match its content hash. Keep the source and retry the download."
        );
        self.install_cached_attachment(name, i64::try_from(data.len())?, |file| {
            file.write_all(data).context("Cannot write the local attachment. Keep the source and check available space before retrying.")
        })
    }

    /// Verifies and streams an ordinary source file into the durable local cache.
    pub fn cache_attachment_file(&self, name: &str, source: &Path) -> Result<()> {
        self.require_usable()?;
        let (_, size) = verify_file(source, name)?;
        self.install_cached_attachment(name, size, |file| {
            io::copy(&mut File::open(source)?, file).context("Cannot copy the downloaded attachment. Keep its source and check cache permissions and space before retrying.")?;
            Ok(())
        })
    }

    fn install_cached_attachment(
        &self,
        name: &str,
        expected_size: i64,
        write: impl FnOnce(&mut File) -> Result<()>,
    ) -> Result<()> {
        let directory = durable_files::directory_below(&self.directory, "attachments")?;
        let path = directory.join(name);
        let (hash, size) = durable_files::install(&path, write, |path| verify_file(path, name))?;
        ensure!(
            size == expected_size,
            "Attachment size changed during installation. Preserve the source and retry."
        );
        self.transaction(|| {
            if let Some((stored_hash, stored_size)) = self.attachment_metadata(name)? {
                ensure!(stored_hash == hash && stored_size == size, "Attachment catalog metadata changed. Preserve the database and restore a verified backup.");
            } else {
                self.connection.exec_bound("INSERT INTO attachments(name,hash,size) VALUES (?,?,?)")?((name, hash.as_str(), size))?;
            }
            self.connection.exec_bound("DELETE FROM attachment_needs WHERE name=?")?(name)?;
            Ok(())
        })
    }

    fn attachment_metadata(&self, name: &str) -> Result<Option<(String, i64)>> {
        self.connection
            .select_row_bound("SELECT hash,size FROM attachments WHERE name=?")?(name)
    }

    pub(super) fn require_attachment(&self, name: &str) -> Result<()> {
        let expected = self.attachment_metadata(name)?.with_context(|| {
            format!(
                "Attachment {name} has no durable local copy. Download or attach it before saving."
            )
        })?;
        let directory = self.directory.join("attachments");
        ordinary_directory(&directory)?;
        let path = directory.join(name);
        ensure!(
            verify_file(&path, name)? == expected,
            "Local attachment {name} differs from its catalog. Preserve the database and restore its original bytes before saving."
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uncertain_commit_blocks_cache_requests_before_file_io() -> Result<()> {
        let root = tempfile::tempdir()?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        store.poisoned.set(true);
        let name = format::attachment_file_name(b"bytes", "a.bin");
        for error in [
            store.cache_attachment(&name, b"bytes").unwrap_err(),
            store
                .cache_attachment_file(&name, &root.path().join("not-downloaded"))
                .unwrap_err(),
        ] {
            assert!(
                error.to_string().contains("commit is uncertain"),
                "{error:#}"
            );
        }
        assert!(!root.path().join("attachments").exists());
        Ok(())
    }
}
