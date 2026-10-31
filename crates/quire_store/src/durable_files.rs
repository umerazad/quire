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

/// The I/O steps of [`install`], in order, for fault injection in tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    Create,
    Write,
    SyncContents,
    Rename,
    SyncInstalled,
    SyncDirectory,
}

/// Test-only fault injection for [`install`]: one armed step either fails as its system call
/// would, leaving `install`'s own cleanup to run, or "crashes" right after it completes, leaving on
/// disk whatever that step wrote, as a killed process would.
#[cfg(test)]
pub(crate) mod faults {
    use super::Step;
    use std::cell::Cell;

    thread_local! {
        static ARMED: Cell<Option<(Step, bool)>> = const { Cell::new(None) };
    }

    /// Arms one fault on this thread; it fires at the next `install` reaching `step`.
    pub(crate) fn arm(step: Step, crash: bool) {
        ARMED.with(|armed| armed.set(Some((step, crash))));
    }

    /// Whether the armed fault has not fired yet.
    pub(crate) fn pending() -> bool {
        ARMED.with(|armed| armed.get().is_some())
    }

    pub(super) fn fire(step: Step, crash: bool) -> bool {
        ARMED.with(|armed| {
            let fires = armed.get() == Some((step, crash));
            if fires {
                armed.set(None);
            }
            fires
        })
    }
}

#[cfg(test)]
fn fail(step: Step) -> Result<()> {
    if faults::fire(step, false) {
        anyhow::bail!("Injected {step:?} failure");
    }
    Ok(())
}

#[cfg(not(test))]
fn fail(_: Step) -> Result<()> {
    Ok(())
}

#[cfg(test)]
fn crash(step: Step) -> bool {
    faults::fire(step, true)
}

#[cfg(not(test))]
fn crash(_: Step) -> bool {
    false
}

const CRASHED: &str = "Injected crash";

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
            fail(Step::Create)?;
            let mut temporary = tempfile::Builder::new().prefix(".quire-").tempfile_in(directory)
                .context("Cannot create a temporary storage file. Check write permissions and available space before retrying.")?;
            if crash(Step::Create) {
                temporary.keep()?;
                anyhow::bail!(CRASHED);
            }
            fail(Step::Write)?;
            write(temporary.as_file_mut())?;
            verify(temporary.path())?;
            if crash(Step::Write) {
                temporary.keep()?;
                anyhow::bail!(CRASHED);
            }
            fail(Step::SyncContents)?;
            temporary.as_file().sync_all().context("Cannot synchronize storage contents. Keep the source and retry on a supported local filesystem.")?;
            if crash(Step::SyncContents) {
                temporary.keep()?;
                anyhow::bail!(CRASHED);
            }
            fail(Step::Rename)?;
            match temporary.persist_noclobber(path) {
                Ok(_) => {},
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => { verify(path)?; },
                Err(error) => return Err(error.error).context("Cannot install an immutable storage file. Keep the source and check folder permissions before retrying."),
            }
            if crash(Step::Rename) {
                anyhow::bail!(CRASHED);
            }
        }
        Err(error) => {
            return Err(error).context(
                "Cannot inspect a storage file. Check folder permissions before retrying.",
            );
        }
    }
    // Adopted files need synchronization too, before their database acknowledgment.
    fail(Step::SyncInstalled)?;
    File::open(path)?.sync_all().context("Cannot synchronize an installed storage file. Keep the pending data and retry before acknowledging it.")?;
    if crash(Step::SyncInstalled) {
        anyhow::bail!(CRASHED);
    }
    fail(Step::SyncDirectory)?;
    sync_directory(directory).context("Cannot synchronize the storage filename. Keep the pending data and retry before acknowledging it.")?;
    if crash(Step::SyncDirectory) {
        anyhow::bail!(CRASHED);
    }
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
