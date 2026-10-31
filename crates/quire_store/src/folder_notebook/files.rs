//! Ordinary Markdown files inside a notebook folder: listing, validated paths, strict reads, and
//! replacements that readers see whole or not at all.

use crate::sync::format::sha256;
use anyhow::{Context as _, Result, bail, ensure};
use std::{
    fs::{self, File},
    io::{self, Write as _},
    path::{Path, PathBuf},
};

/// The reference note size limit.
pub const MAX_NOTE_BYTES: u64 = 10 * 1024 * 1024;

/// Build output and tool folders the reference walk never lists.
const SKIPPED: [&str; 3] = ["node_modules", "dist", "out"];

/// One file's contents as read from disk. `revision` is the SHA-256 of `text`, as in the
/// reference folder notebook.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskDocument {
    pub path: String,
    pub text: String,
    pub revision: String,
}

impl DiskDocument {
    pub fn new(path: String, text: String) -> Self {
        Self {
            revision: revision(&text),
            path,
            text,
        }
    }
}

pub fn revision(text: &str) -> String {
    sha256(text)
}

/// Every Markdown file and folder below the root, skipping dot entries, build output, and
/// symbolic links, so listed paths stay inside the notebook.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Listing {
    pub files: Vec<ListedFile>,
    pub folders: Vec<String>,
    /// Ordinary files that are not notes, such as attachments.
    pub others: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedFile {
    pub path: String,
    pub size: u64,
    pub modified: Option<std::time::SystemTime>,
}

pub fn list(root: &Path) -> Listing {
    let mut listing = Listing::default();
    walk(root, "", &mut listing);
    listing.files.sort_by(|a, b| a.path.cmp(&b.path));
    listing.folders.sort();
    listing.others.sort();
    listing
}

fn walk(root: &Path, folder: &str, listing: &mut Listing) {
    let directory = root.join(folder);
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) => {
            listing.warnings.push(format!(
                "Could not read {}: {error}. Make this folder available locally and rescan.",
                directory.display()
            ));
            return;
        }
    };
    for entry in entries.flatten() {
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            listing.warnings.push(format!(
                "Skipped {} because its name is not valid Unicode. Rename it to open it in Quire.",
                entry.path().display()
            ));
            continue;
        };
        if name.starts_with('.') || SKIPPED.contains(&name.as_str()) {
            continue;
        }
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let relative = if folder.is_empty() {
            name.clone()
        } else {
            format!("{folder}/{name}")
        };
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            listing.folders.push(relative.clone());
            walk(root, &relative, listing);
        } else if kind.is_file() && is_markdown(&name) {
            let metadata = entry.metadata().ok();
            listing.files.push(ListedFile {
                path: relative,
                size: metadata.as_ref().map_or(0, |metadata| metadata.len()),
                modified: metadata.and_then(|metadata| metadata.modified().ok()),
            });
        } else if kind.is_file() {
            listing.others.push(relative);
        }
    }
}

pub fn is_markdown(name: &str) -> bool {
    name.len() > 3
        && name
            .get(name.len() - 3..)
            .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".md"))
}

/// The reference `safePath` shape check: a relative, slash-separated Markdown path with no empty,
/// parent, or hidden segments.
pub fn validate(relative: &str) -> Result<()> {
    ensure!(
        !relative.is_empty()
            && !relative.starts_with('/')
            && !relative.contains('\\')
            && !relative
                .split('/')
                .any(|part| part.is_empty() || part == ".." || part.starts_with('.'))
            && is_markdown(relative),
        "Invalid note path {relative:?}. Choose a Markdown file inside the open notebook."
    );
    Ok(())
}

/// Resolves `relative` below `root`, refusing paths that leave the notebook through links.
/// With `create`, missing parent folders are created one at a time and must be ordinary folders.
pub fn resolve(root: &Path, relative: &str, create: bool) -> Result<PathBuf> {
    validate(relative)?;
    let destination = root.join(relative);
    let parent = destination
        .parent()
        .context("A note path has no folder. Choose a Markdown file inside the notebook.")?;
    if create {
        let mut current = root.to_owned();
        for part in relative
            .split('/')
            .collect::<Vec<_>>()
            .split_last()
            .map_or(&[][..], |(_, folders)| folders)
        {
            let above = current.clone();
            current.push(part);
            match fs::create_dir(&current) {
                Ok(()) => sync_directory(&above).with_context(|| format!("Created {} but could not make it durable. Check the folder's disk and retry the save.", current.display()))?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(error).with_context(|| format!("Cannot create folder {}. Check write access to the notebook and retry.", current.display())),
            }
            let metadata = fs::symlink_metadata(&current)?;
            ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "A note directory is a link or is unavailable. Choose a directory within the notebook."
            );
        }
    }
    let real_root = fs::canonicalize(root).with_context(|| {
        format!(
            "Cannot open notebook folder {}. Make it available locally and reopen it.",
            root.display()
        )
    })?;
    let real_parent = fs::canonicalize(parent).with_context(|| {
        format!("Cannot find the folder for {relative}. Make it available locally and retry.")
    })?;
    ensure!(
        real_parent.starts_with(&real_root),
        "The note {relative} resolves outside this notebook. Move it into the notebook before editing."
    );
    match fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("The note {relative} is a symbolic link. Open its actual notebook to edit it.")
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Cannot inspect {relative}. Check folder permissions and retry.")
            });
        }
    }
    Ok(destination)
}

/// Reads one note exactly as stored, refusing invalid UTF-8 and notes over the size limit.
/// Returns None when the file does not exist.
pub fn read(root: &Path, relative: &str) -> Result<Option<DiskDocument>> {
    let target = resolve(root, relative, false)?;
    read_at(relative, &target)
}

pub(crate) fn read_at(relative: &str, target: &Path) -> Result<Option<DiskDocument>> {
    let metadata = match fs::metadata(target) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Cannot read {relative}. Make the file available locally and retry.")
            });
        }
    };
    ensure!(
        metadata.is_file(),
        "{relative} is not an ordinary file. Rename the folder or file so each note is a Markdown file."
    );
    ensure!(
        metadata.len() <= MAX_NOTE_BYTES,
        "{relative} exceeds the 10 MB note limit. Split it into smaller Markdown files to edit it."
    );
    let bytes = match fs::read(target) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Cannot read {relative}. Make the file available locally and retry.")
            });
        }
    };
    let text = String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!(
            "{relative} is not valid UTF-8 text. Convert it to UTF-8 in another editor before opening it in Quire."
        )
    })?;
    Ok(Some(DiskDocument::new(relative.to_owned(), text)))
}

/// A replacement written beside its destination and made durable, not yet visible.
pub(crate) struct Staged {
    file: tempfile::NamedTempFile,
    destination: PathBuf,
}

/// Writes `text` to a temporary file in the destination's folder, with the existing file's
/// permissions, and syncs it to disk.
pub(crate) fn stage(destination: &Path, text: &str) -> Result<Staged> {
    stage_with(destination, text, full_sync)
}

/// Like [`stage`], but only [`flush`]es the file, for a batch that calls [`barrier`] once
/// before publishing any of its files.
pub(crate) fn stage_flushed(destination: &Path, text: &str) -> Result<Staged> {
    stage_with(destination, text, flush)
}

fn stage_with(destination: &Path, text: &str, sync: fn(&File) -> io::Result<()>) -> Result<Staged> {
    let folder = destination
        .parent()
        .context("A note path has no folder. Choose a Markdown file inside the notebook.")?;
    let mut file = tempfile::Builder::new()
        .prefix(".quire-")
        .suffix(".tmp")
        .tempfile_in(folder)
        .with_context(|| format!("Cannot create a temporary file in {}. Check write access and free space, then retry the save.", folder.display()))?;
    let permissions = match fs::metadata(destination) {
        Ok(metadata) => Some(metadata.permissions()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => default_permissions(),
        Err(error) => {
            return Err(error)
                .context("Cannot read the note's permissions. Check folder access and retry.");
        }
    };
    if let Some(permissions) = permissions {
        file.as_file().set_permissions(permissions).context(
            "Cannot copy the note's permissions to its replacement. Check folder access and retry.",
        )?;
    }
    file.write_all(text.as_bytes())
        .and_then(|()| sync(file.as_file()))
        .with_context(|| format!("Cannot write the replacement for {}. Check free space and retry; your text is kept in recovery.", destination.display()))?;
    Ok(Staged {
        file,
        destination: destination.to_owned(),
    })
}

#[cfg(unix)]
fn default_permissions() -> Option<fs::Permissions> {
    use std::os::unix::fs::PermissionsExt as _;
    Some(fs::Permissions::from_mode(0o644))
}

#[cfg(not(unix))]
fn default_permissions() -> Option<fs::Permissions> {
    None
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Published {
    Done,
    /// The destination appeared while creating a new note; nothing was replaced.
    Exists,
}

impl Staged {
    /// Makes the staged text visible in one rename. A new note never replaces a file that another
    /// application created meanwhile.
    pub(crate) fn publish(self, replace: bool) -> Result<Published> {
        let folder = self
            .destination
            .parent()
            .map(Path::to_owned)
            .context("A note path has no folder.")?;
        let destination = self.destination.clone();
        if self.place(replace)? == Published::Exists {
            return Ok(Published::Exists);
        }
        sync_directory(&folder).with_context(|| {
            format!("Saved {} but could not make its name durable. Retry saving to be sure it survives a power loss.", destination.display())
        })?;
        Ok(Published::Done)
    }

    /// Renames the staged file into place without syncing its folder, for a batch that syncs
    /// each folder once afterwards.
    pub(crate) fn place(self, replace: bool) -> Result<Published> {
        let destination = self.destination;
        if replace {
            self.file.persist(&destination).map_err(|error| error.error).with_context(|| {
                format!("Cannot replace {}. Check write access and retry; your text is kept in recovery.", destination.display())
            })?;
        } else {
            match self.file.persist_noclobber(&destination) {
                Ok(_) => {}
                Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => {
                    return Ok(Published::Exists);
                }
                Err(error) => {
                    return Err(error.error).with_context(|| {
                        format!("Cannot create {}. Check write access and retry; your text is kept in recovery.", destination.display())
                    });
                }
            }
        }
        Ok(Published::Done)
    }
}

#[cfg(test)]
thread_local! {
    /// Syncs on this thread that reach stable storage, `F_FULLFSYNC` on macOS.
    pub(crate) static FULL_SYNCS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Syncs a file to stable storage, on macOS flushing the drive's whole cache.
pub(crate) fn full_sync(file: &File) -> io::Result<()> {
    #[cfg(test)]
    FULL_SYNCS.with(|count| count.set(count.get() + 1));
    file.sync_all()
}

#[cfg(unix)]
pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    full_sync(&File::open(path)?)
}

#[cfg(not(unix))]
pub(crate) fn sync_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Sends a file's data and attributes to its drive. On macOS this is `fsync`, which leaves them
/// in the drive's volatile cache, where `sync_all`'s `F_FULLFSYNC` would also flush that whole
/// cache; a batch flushes each file and then calls [`barrier`] once per drive. Elsewhere
/// `fsync` already reaches stable storage.
#[cfg(target_vendor = "apple")]
pub(crate) fn flush(file: &File) -> io::Result<()> {
    use std::os::fd::AsRawFd as _;
    // SAFETY: `fsync` only reads the descriptor, which `file` keeps open for the call.
    match unsafe { libc::fsync(file.as_raw_fd()) } {
        0 => Ok(()),
        _ => Err(io::Error::last_os_error()),
    }
}

#[cfg(not(target_vendor = "apple"))]
pub(crate) fn flush(file: &File) -> io::Result<()> {
    full_sync(file)
}

/// [`flush`] for a folder, making the names created or removed in it reach the drive.
#[cfg(unix)]
pub(crate) fn flush_directory(path: &Path) -> io::Result<()> {
    flush(&File::open(path)?)
}

#[cfg(not(unix))]
pub(crate) fn flush_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

/// Makes everything already [`flush`]ed to the drives holding `paths` durable, with one full
/// sync per drive: `F_FULLFSYNC` asks the drive to write its whole cache to stable storage.
#[cfg(target_vendor = "apple")]
pub(crate) fn barrier<'a>(paths: impl IntoIterator<Item = &'a Path>) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt as _;
    let mut drives = std::collections::HashSet::new();
    for path in paths {
        if drives.insert(fs::metadata(path)?.dev()) {
            full_sync(&File::open(path)?)?;
        }
    }
    Ok(())
}

#[cfg(not(target_vendor = "apple"))]
pub(crate) fn barrier<'a>(_paths: impl IntoIterator<Item = &'a Path>) -> io::Result<()> {
    Ok(())
}
