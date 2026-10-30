use anyhow::{Context as _, Result, ensure};
use std::{
    fs::{self, File},
    io,
    path::{Path, PathBuf},
};

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
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

pub(crate) fn ordinary_directory(path: &Path) -> Result<()> {
    ensure!(
        fs::symlink_metadata(path)?.is_dir(),
        "Storage path {} is not an ordinary directory. Restore the original folder before retrying.",
        path.display()
    );
    Ok(())
}

pub(crate) fn directory_below(parent: &Path, name: &str) -> Result<PathBuf> {
    ordinary_directory(parent)?;
    let directory = parent.join(name);
    match fs::create_dir(&directory) {
        Ok(()) => {},
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {},
        Err(error) => return Err(error).context("Cannot create a storage folder. Check write permissions and available space before retrying."),
    }
    ordinary_directory(&directory)?;
    // Existing entries may come from an interrupted attempt that did not sync its parent.
    sync_directory(parent).context("Cannot synchronize a storage folder. Keep the source and retry on a supported local filesystem.")?;
    Ok(directory)
}

pub(crate) fn install<T>(
    path: &Path,
    write: impl FnOnce(&mut File) -> Result<()>,
    verify: impl Fn(&Path) -> Result<T>,
) -> Result<T> {
    let directory = path
        .parent()
        .context("Storage file has no parent folder. Choose an absolute notebook folder.")?;
    ordinary_directory(directory)?;
    match fs::symlink_metadata(path) {
        Ok(_) => {
            verify(path)?;
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let mut temporary = tempfile::Builder::new().prefix(".quire-").tempfile_in(directory)
                .context("Cannot create a temporary storage file. Check write permissions and available space before retrying.")?;
            write(temporary.as_file_mut())?;
            verify(temporary.path())?;
            temporary.as_file().sync_all().context("Cannot synchronize storage contents. Keep the source and retry on a supported local filesystem.")?;
            match temporary.persist_noclobber(path) {
                Ok(_) => {},
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => { verify(path)?; },
                Err(error) => return Err(error.error).context("Cannot install an immutable storage file. Keep the source and check folder permissions before retrying."),
            }
        }
        Err(error) => {
            return Err(error).context(
                "Cannot inspect a storage file. Check folder permissions before retrying.",
            );
        }
    }
    // Adopted files need synchronization too, before their database acknowledgment.
    File::open(path)?.sync_all().context("Cannot synchronize an installed storage file. Keep the pending data and retry before acknowledging it.")?;
    sync_directory(directory).context("Cannot synchronize the storage filename. Keep the pending data and retry before acknowledging it.")?;
    verify(path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn failed_writes_leave_no_destination_or_temporary_file_and_can_retry() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("immutable");
        let verify = |path: &Path| -> Result<()> {
            ensure!(
                fs::read(path)? == b"complete",
                "File contents differ. Retry the original write."
            );
            Ok(())
        };
        let result = install(
            &path,
            |file| {
                file.write_all(b"partial")?;
                Err(io::Error::new(io::ErrorKind::WriteZero, "injected interrupted write").into())
            },
            verify,
        );
        assert!(result.is_err());
        assert_eq!(fs::read_dir(directory.path())?.count(), 0);
        install(&path, |file| Ok(file.write_all(b"complete")?), verify)?;
        assert_eq!(fs::read(&path)?, b"complete");
        Ok(())
    }

    #[test]
    fn failed_verification_does_not_install_an_immutable_filename() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let path = directory.path().join("immutable");
        let result = install(
            &path,
            |file| Ok(file.write_all(b"partial")?),
            |file| {
                ensure!(
                    fs::read(file)? == b"complete",
                    "The copied source changed during the write. Retry from its original bytes."
                );
                Ok(())
            },
        );
        assert!(result.is_err());
        assert!(
            !path.exists(),
            "invalid bytes must never acquire an immutable filename"
        );
        assert_eq!(fs::read_dir(directory.path())?.count(), 0);
        Ok(())
    }
}
