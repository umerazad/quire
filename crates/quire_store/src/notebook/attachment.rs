use super::NotebookStore;
use crate::sync::format;
use anyhow::{Context as _, Result, ensure};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::Path,
};

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(path)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Durable directory synchronization is not implemented on this platform",
        ))
    }
}

fn ordinary_directory(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Attachment cache is not an ordinary directory. Preserve its contents and restore the local cache directory before retrying."
    );
    Ok(())
}

fn verify_file(path: &Path, name: &str) -> Result<(String, i64)> {
    let expected = format::attachment_hash(name)
        .context("Invalid attachment name. Use a content-hashed attachment filename.")?;
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!("Local attachment {name} is missing. Download it before saving.")
    })?;
    ensure!(
        metadata.is_file(),
        "Local attachment {name} is not an ordinary file. Restore its original bytes before saving."
    );
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
    let hash = format!("{:x}", hash.finalize());
    ensure!(
        hash == expected,
        "Local attachment {name} failed its hash check. Preserve the damaged file and restore its original bytes before saving."
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

    /// Verifies downloaded bytes against their protocol name before writing them.
    /// Existing damaged files are preserved; an unregistered valid file may be adopted.
    pub fn cache_attachment(&self, name: &str, data: &[u8]) -> Result<()> {
        let expected = format::attachment_hash(name)
            .context("Invalid attachment name. Use a content-hashed attachment filename.")?;
        ensure!(
            format::sha256(data) == expected,
            "Attachment {name} does not match its content hash. Keep the source and retry the download."
        );
        let directory = self.directory.join("attachments");
        match fs::create_dir(&directory) {
            Ok(()) => {},
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
            Err(error) => return Err(error).context("Cannot create the local attachment cache. Check folder permissions and available space before retrying."),
        }
        ordinary_directory(&directory)?;
        sync_directory(&self.directory).context("Cannot synchronize the attachment cache directory. Keep the source and retry on a supported local filesystem.")?;
        let path = directory.join(name);
        match fs::symlink_metadata(&path) {
            Ok(_) => {
                verify_file(&path, name)?;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut temporary = tempfile::Builder::new().prefix(".quire-attachment-").tempfile_in(&directory)
                    .context("Cannot create a temporary attachment. Check available space and cache permissions before retrying.")?;
                temporary.write_all(data).context("Cannot write the local attachment. Keep the source and check available space before retrying.")?;
                temporary.as_file().sync_all().context("Cannot synchronize the local attachment. Keep the source and retry on a supported local filesystem.")?;
                match temporary.persist_noclobber(&path) {
                    Ok(_) => {},
                    Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => { verify_file(&path, name)?; },
                    Err(error) => return Err(error.error).context("Cannot install the local attachment. Keep the source and check cache permissions before retrying."),
                }
            }
            Err(error) => {
                return Err(error).context(
                    "Cannot inspect the local attachment. Check cache permissions before retrying.",
                );
            }
        }
        // Re-sync adopted files too: a prior attempt may have stopped before its catalog commit.
        File::open(&path)?.sync_all().context("Cannot synchronize the installed attachment. Keep the source and retry before saving its reference.")?;
        sync_directory(&directory).context("Cannot synchronize the attachment filename. Keep the source and retry before saving its reference.")?;
        let (hash, size) = verify_file(&path, name)?;
        ensure!(
            size == i64::try_from(data.len())?,
            "Attachment size changed during installation. Preserve the source and retry."
        );
        self.transaction(|| {
            if let Some((stored_hash, stored_size)) = self.attachment_metadata(name)? {
                ensure!(stored_hash == hash && stored_size == size, "Attachment catalog metadata changed. Preserve the database and restore a verified backup.");
            } else {
                self.connection.exec_bound("INSERT INTO attachments(name,hash,size) VALUES (?,?,?)")?((name, hash.as_str(), size))?;
            }
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
