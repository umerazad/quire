//! Copying a folder of Markdown pages and their files into a notebook. Reading the source and
//! copying its other files happen here; each backend creates the pages in one checked write
//! (`NotebookStore::import_pages`, `FolderNotebook::import_pages`), so a page import lands whole
//! or not at all. The source folder is only read.

use crate::{durable_files::sync_directory, notebook::SaveAck};
use anyhow::{Context as _, Result, bail};
use quire_core::exchange::import::{SourceEntry, source_entry};
use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// A page read from the source folder, at its path relative to the folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourcePage {
    pub path: String,
    pub text: String,
}

/// Everything an import reads from the source folder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SourceFolder {
    pub pages: Vec<SourcePage>,
    /// Other ordinary files, relative to the folder.
    pub files: Vec<String>,
}

/// What a page import wrote.
#[derive(Debug)]
pub struct PagesImported {
    pub saved: Vec<SaveAck>,
    /// Source paths of pages left alone because the notebook already has their path.
    pub skipped: Vec<String>,
}

pub const CANCELED: &str = "The import was canceled before anything was written.";

fn canceled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!(CANCELED);
    }
    Ok(())
}

/// Reads the source folder the way the reference walk does, in name order: entries whose names
/// start with a dot and symbolic links are skipped, `.md` files are read as pages (invalid UTF-8
/// becomes U+FFFD, as the reference reads them), and other ordinary files are listed. `progress`
/// hears how many pages have been read. Refuses a source inside `notebook`.
pub fn read_source(
    source: &Path,
    notebook: Option<&Path>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize),
) -> Result<SourceFolder> {
    let root = fs::canonicalize(source).with_context(|| {
        format!(
            "Cannot open {} to import. Check that the folder exists and Quire can read it, then import again.",
            source.display()
        )
    })?;
    if let Some(notebook) = notebook.and_then(|notebook| fs::canonicalize(notebook).ok())
        && (root.starts_with(&notebook) || notebook.starts_with(&root))
    {
        bail!("Choose a folder outside this notebook to import.");
    }
    let mut folder = SourceFolder::default();
    walk(&root, "", &mut folder, cancel, &mut progress)?;
    Ok(folder)
}

/// The reference walk: depth first, each folder's entries in name order.
fn walk(
    root: &Path,
    relative: &str,
    folder: &mut SourceFolder,
    cancel: &AtomicBool,
    progress: &mut impl FnMut(usize),
) -> Result<()> {
    let directory = root.join(relative);
    let mut entries = fs::read_dir(&directory)
        .and_then(|entries| entries.collect::<io::Result<Vec<_>>>())
        .with_context(|| {
            format!(
                "Cannot list {} to import. Check that Quire can read it, then import again.",
                directory.display()
            )
        })?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        canceled(cancel)?;
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            bail!(
                "Cannot import {}: its name is not valid Unicode. Rename it in the source folder and import again; nothing was imported.",
                entry.path().display()
            );
        };
        let kind = entry.file_type()?;
        let path = if relative.is_empty() {
            name.clone()
        } else {
            format!("{relative}/{name}")
        };
        match source_entry(&name, kind.is_dir(), kind.is_file()) {
            SourceEntry::Skipped => {}
            SourceEntry::Folder => walk(root, &path, folder, cancel, progress)?,
            SourceEntry::File => folder.files.push(path),
            SourceEntry::Page => {
                let bytes = fs::read(entry.path()).with_context(|| {
                    format!("Cannot read {path} to import. Check that Quire can read it, then import again; nothing was imported.")
                })?;
                folder.pages.push(SourcePage {
                    path,
                    text: String::from_utf8_lossy(&bytes).into_owned(),
                });
                progress(folder.pages.len());
            }
        }
    }
    Ok(())
}

/// Copies `files` from `source` into the folder notebook at `root`, at the same relative paths.
/// A file already there is left alone, as the reference does. Each copy is written to a
/// temporary file and installed without replacing anything. Returns the files this call created,
/// so a failed import can remove them.
pub fn copy_files(source: &Path, root: &Path, files: &[String]) -> Result<Vec<PathBuf>> {
    let mut created = Vec::new();
    let result = files.iter().try_for_each(|file| {
        let target = root.join(file);
        let directory = target.parent().context("An imported file has no folder. Report this as a bug.")?;
        let mut folder = root.to_path_buf();
        for part in Path::new(file).parent().into_iter().flat_map(Path::components) {
            folder.push(part);
            if fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_symlink()) {
                bail!(
                    "Cannot copy {file}: {} in the notebook is a link, and Quire copies files only into ordinary folders. Replace the link with a folder, then import again.",
                    folder.display()
                );
            }
        }
        fs::create_dir_all(directory).with_context(|| {
            format!("Cannot create the folder for {file}. Check free space and write access to the notebook, then import again.")
        })?;
        if fs::symlink_metadata(&target).is_ok() {
            return Ok(());
        }
        let mut temporary = tempfile::Builder::new()
            .prefix(".quire-")
            .tempfile_in(directory)
            .with_context(|| format!("Cannot copy {file}. Check free space and write access to the notebook, then import again."))?;
        io::copy(&mut fs::File::open(source.join(file))?, temporary.as_file_mut())
            .with_context(|| format!("Cannot copy {file}. Check that Quire can read it and that the notebook has free space, then import again."))?;
        temporary.as_file().sync_all()?;
        match temporary.persist_noclobber(&target) {
            Ok(_) => {
                sync_directory(directory).ok();
                created.push(target);
                Ok(())
            }
            Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
            Err(error) => Err(error.error).with_context(|| format!("Cannot copy {file} into the notebook. Check write access, then import again.")),
        }
    });
    match result {
        Ok(()) => Ok(created),
        Err(error) => {
            remove_files(&created);
            Err(error)
        }
    }
}

/// Removes files an import created before it failed.
pub fn remove_files(created: &[PathBuf]) {
    for file in created {
        fs::remove_file(file).ok();
    }
}
