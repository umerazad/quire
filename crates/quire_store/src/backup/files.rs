//! File work for backups: hashing while copying, durable folders, and walking a tree of ordinary
//! files with portable relative paths.

use crate::durable_files::sync_directory;
use anyhow::{Context as _, Result, bail};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// The SHA-256 and size of the file at `path`, read in chunks.
pub(super) fn hash_file(path: &Path) -> Result<(String, u64)> {
    let file = File::open(path).with_context(|| {
        format!(
            "Cannot read {}. Check that it exists and retry.",
            path.display()
        )
    })?;
    hash_reader(file)
        .with_context(|| format!("Cannot read {}. Check the drive and retry.", path.display()))
}

fn hash_reader(mut reader: impl Read) -> io::Result<(String, u64)> {
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut size = 0;
    loop {
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            return Ok((format!("{:x}", hash.finalize()), size));
        }
        hash.update(&buffer[..count]);
        size += count as u64;
    }
}

/// Copies `source` to a new file at `target`, synced to stable storage, and returns the hash and
/// size of the bytes copied. Never replaces an existing file.
pub(super) fn copy_hashed(source: &Path, target: &Path) -> Result<(String, u64)> {
    let metadata = fs::symlink_metadata(source).with_context(|| {
        format!(
            "Cannot read {}. Check that it exists and retry.",
            source.display()
        )
    })?;
    if !metadata.is_file() {
        bail!(
            "{} is not an ordinary file. Replace it with a local file and retry.",
            source.display()
        );
    }
    if let Some(parent) = target.parent() {
        create_dir_durable(parent)?;
    }
    let mut input = File::open(source).with_context(|| {
        format!(
            "Cannot read {}. Check its permissions and retry.",
            source.display()
        )
    })?;
    let mut output = File::options()
        .write(true)
        .create_new(true)
        .open(target)
        .with_context(|| {
            format!(
                "Cannot create {}. Check free space and write access there, then retry.",
                target.display()
            )
        })?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 1 << 16];
    let mut size = 0;
    loop {
        let count = input.read(&mut buffer).with_context(|| {
            format!(
                "Cannot read {}. Check the drive and retry.",
                source.display()
            )
        })?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
        output.write_all(&buffer[..count]).with_context(|| {
            format!(
                "Cannot write {}. Check free space on that drive and retry.",
                target.display()
            )
        })?;
        size += count as u64;
    }
    output.sync_all().with_context(|| {
        format!(
            "Cannot flush {} to the drive. Check the drive and retry.",
            target.display()
        )
    })?;
    Ok((format!("{:x}", hash.finalize()), size))
}

/// Creates `path` and any missing parents, syncing each new entry into its parent folder.
pub(super) fn create_dir_durable(path: &Path) -> Result<()> {
    let missing: Vec<PathBuf> = path
        .ancestors()
        .take_while(|ancestor| !ancestor.exists())
        .map(Path::to_path_buf)
        .collect();
    fs::create_dir_all(path).with_context(|| {
        format!(
            "Cannot create the folder {}. Check write access and free space, then retry.",
            path.display()
        )
    })?;
    for created in missing.iter().rev() {
        if let Some(parent) = created.parent() {
            sync(parent)?;
        }
    }
    Ok(())
}

pub(super) fn sync(directory: &Path) -> Result<()> {
    sync_directory(directory).with_context(|| {
        format!(
            "Cannot flush the folder {} to the drive. Use a local or supported drive and retry.",
            directory.display()
        )
    })
}

/// Syncs every folder below and including `root`, deepest first.
pub(super) fn sync_tree(root: &Path) -> Result<()> {
    let mut folders = vec![root.to_path_buf()];
    let mut index = 0;
    while let Some(folder) = folders.get(index).cloned() {
        for entry in fs::read_dir(&folder)? {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                folders.push(entry.path());
            }
        }
        index += 1;
    }
    folders.iter().rev().try_for_each(|folder| sync(folder))
}

/// A relative path in a manifest: `/`-separated, with no empty, `.`, or `..` part.
pub(super) fn valid_path(path: &str) -> bool {
    !path.is_empty()
        && !path.contains('\\')
        && !path.contains('\0')
        && path
            .split('/')
            .all(|part| !part.is_empty() && part != "." && part != "..")
}

/// Every ordinary file below `root` as a sorted relative path, leaving out entries `skip` names.
/// A symbolic link or special file fails the walk rather than being followed or dropped.
pub(super) fn walk(root: &Path, skip: &dyn Fn(&str) -> bool) -> Result<Vec<String>> {
    let mut files = Vec::new();
    let mut pending = vec![String::new()];
    while let Some(folder) = pending.pop() {
        let directory = if folder.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&folder)
        };
        let entries = fs::read_dir(&directory).with_context(|| {
            format!(
                "Cannot list {}. Check that it is available and readable, then retry.",
                directory.display()
            )
        })?;
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_str().with_context(|| {
                format!(
                    "{} has a file name that is not valid Unicode. Rename it and retry.",
                    entry.path().display()
                )
            })?;
            let relative = if folder.is_empty() {
                name.to_owned()
            } else {
                format!("{folder}/{name}")
            };
            if skip(&relative) {
                continue;
            }
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                bail!(
                    "Backup cannot include the symbolic link {relative}. Replace it with an ordinary file or folder and retry."
                );
            } else if kind.is_dir() {
                pending.push(relative);
            } else if kind.is_file() {
                files.push(relative);
            } else {
                bail!("Backup cannot read {relative}. Make it an ordinary local file and retry.");
            }
        }
    }
    files.sort();
    Ok(files)
}

/// `relative` below `root`, built from its `/`-separated parts.
pub(super) fn below(root: &Path, relative: &str) -> PathBuf {
    relative
        .split('/')
        .fold(root.to_path_buf(), |path, part| path.join(part))
}

/// Removes a folder this backup or restore created, reporting what it could not remove.
pub(super) fn discard(path: &Path) -> Option<String> {
    match fs::remove_dir_all(path) {
        Ok(()) => None,
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => Some(format!(
            " The unfinished copy at {} could not be removed ({error}); delete it by hand.",
            path.display()
        )),
    }
}
