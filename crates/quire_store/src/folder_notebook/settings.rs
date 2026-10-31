//! The notebook settings the reference app keeps in `.quire/notebook.json`, read so a folder
//! opens with its configured daily layout and replaced in one rename when the layout is saved.

use super::files::{DiskDocument, sync_directory};
use anyhow::{Context as _, Result, bail, ensure};
use std::{
    fs, io,
    io::Write as _,
    path::{Path, PathBuf},
};

pub const METADATA: &str = ".quire";
const FILE: &str = "notebook.json";
const DISPLAY: &str = ".quire/notebook.json";
/// The reference writer refuses larger settings.
const MAX_BYTES: usize = 64 * 1024;

/// The metadata folder Quire used while it was called Threadnote, before 0.9.23.
const LEGACY_METADATA: &str = ".threadnote";

/// Renames `root`'s `.threadnote` folder to `.quire` the first time Quire opens the notebook, as
/// the reference app does, so its settings and history stay with the notebook. Nothing is
/// overwritten: when both folders exist, an older Threadnote on another computer still writes
/// the old one, and the returned notice says so. Also returns a notice when the rename fails.
pub fn adopt_legacy_metadata(root: &Path) -> Option<String> {
    let from = root.join(LEGACY_METADATA);
    let legacy = fs::symlink_metadata(&from).ok()?;
    if !legacy.is_dir() || legacy.file_type().is_symlink() {
        return None;
    }
    let to = root.join(METADATA);
    if fs::symlink_metadata(&to).is_ok() {
        return Some(format!(
            "This notebook has a {LEGACY_METADATA} folder from an older Threadnote beside {METADATA}. Update every computer to Quire; Quire does not read {LEGACY_METADATA}, so that computer's recovery data and history stay there until it updates."
        ));
    }
    match fs::rename(&from, &to) {
        Ok(()) => {
            sync_directory(root).ok();
            None
        }
        Err(error) => Some(format!(
            "Cannot rename this notebook's {LEGACY_METADATA} folder to {METADATA}: {error}. Its settings and history stay in {LEGACY_METADATA}; check the folder's permissions and reopen the notebook."
        )),
    }
}

/// `.quire` below `root`, which must be an ordinary folder when it exists.
fn metadata(root: &Path) -> Result<Option<PathBuf>> {
    let folder = root.join(METADATA);
    match fs::symlink_metadata(&folder) {
        Ok(found) if found.is_dir() && !found.file_type().is_symlink() => Ok(Some(folder)),
        Ok(_) => bail!(
            "The notebook's .quire entry is a link or a file. Replace it with an ordinary folder, then reopen the notebook."
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context(
            "Cannot inspect the notebook's .quire folder. Check folder permissions and reopen the notebook.",
        ),
    }
}

/// The settings file's text and revision, or None when the notebook has none.
pub fn read(root: &Path) -> Result<Option<DiskDocument>> {
    let Some(folder) = metadata(root)? else {
        return Ok(None);
    };
    let file = folder.join(FILE);
    match fs::symlink_metadata(&file) {
        Ok(found) => ensure!(
            found.is_file() && !found.file_type().is_symlink(),
            "{DISPLAY} is not an ordinary file. Replace it with a file, then reopen the notebook."
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!("Cannot read {DISPLAY}. Check folder permissions and reopen the notebook.")
            });
        }
    }
    let bytes = fs::read(&file).with_context(|| {
        format!("Cannot read {DISPLAY}. Check folder permissions and reopen the notebook.")
    })?;
    ensure!(
        bytes.len() <= MAX_BYTES,
        "{DISPLAY} is larger than 64 KB. Fix or delete it, then reopen the notebook."
    );
    let text = String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!("{DISPLAY} is not UTF-8 text. Fix or delete it, then reopen the notebook.")
    })?;
    Ok(Some(DiskDocument::new(DISPLAY.into(), text)))
}

/// Replaces the settings file with `text` in one rename when it still has revision `base` (None:
/// no file yet), creating `.quire` if needed. Returns the new revision.
pub fn write(root: &Path, text: &str, base: Option<&str>) -> Result<String> {
    ensure!(
        text.len() <= MAX_BYTES && serde_json::from_str::<serde_json::Value>(text).is_ok(),
        "Notebook settings are invalid. Reopen Settings and try again."
    );
    let stale = || {
        anyhow::anyhow!(
            "{DISPLAY} changed in another application after Quire read it. Reopen the notebook to see the new settings, then save the layout again."
        )
    };
    let current = read(root)?;
    if current.as_ref().map(|document| document.revision.as_str()) != base {
        return Err(stale());
    }
    let folder = match metadata(root)? {
        Some(folder) => folder,
        None => {
            let folder = root.join(METADATA);
            match fs::create_dir(&folder) {
                Ok(()) => sync_directory(root).with_context(|| {
                    format!("Created {} but could not make it durable. Check the notebook's disk and save the layout again.", folder.display())
                })?,
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("Cannot create {}. Check write access to the notebook and save the layout again.", folder.display())
                    });
                }
            }
            metadata(root)?.context(
                "The notebook's .quire folder disappeared while saving. Save the layout again.",
            )?
        }
    };
    let mut staged = tempfile::Builder::new()
        .prefix("notebook.json.")
        .suffix(".tmp")
        .tempfile_in(&folder)
        .with_context(|| format!("Cannot create a temporary file in {}. Check write access and free space, then save the layout again.", folder.display()))?;
    staged
        .write_all(text.as_bytes())
        .and_then(|()| staged.as_file().sync_all())
        .with_context(|| {
            format!("Cannot write {DISPLAY}. Check free space, then save the layout again.")
        })?;
    if read(root)?
        .as_ref()
        .map(|document| document.revision.as_str())
        != base
    {
        return Err(stale());
    }
    staged
        .persist(folder.join(FILE))
        .map_err(|error| error.error)
        .with_context(|| {
            format!("Cannot replace {DISPLAY}. Check write access, then save the layout again.")
        })?;
    sync_directory(&folder).with_context(|| {
        format!("Saved {DISPLAY} but could not make it durable. Save the layout again to be sure it survives a power loss.")
    })?;
    Ok(DiskDocument::new(DISPLAY.into(), text.into()).revision)
}
