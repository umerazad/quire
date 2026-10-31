//! Restoring a verified backup as a new notebook. The open notebook is backed up first and is
//! never written: the backup is copied into a hidden folder beside the chosen location, given its
//! new identity, checked, and only then renamed into place. Any failure removes what the restore
//! created and leaves the current notebook and every backup as they were.

use super::{
    BackupInfo, Kind, Manifest, STATE_DATABASE, STORE_DATABASE, Source, create, database,
    files::{below, copy_hashed, create_dir_durable, discard, hash_file, sync, sync_tree},
    outside_live, overlaps, real, unique, verify,
};
use crate::{
    folder_notebook::FolderNotebook,
    notebook::{Draft, NotebookStore, adopt_restored},
};
use anyhow::{Context as _, Result, ensure};
use std::{
    fs,
    path::{Path, PathBuf},
};

/// A point in a restore, reported once the step before it is complete and durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreStep {
    /// The current notebook was backed up.
    SafetyBackup,
    /// The backup's files were copied into the hidden restore folder.
    Copied,
    /// The copy took its new identity.
    Adopted,
    /// The copy passed its checks.
    Verified,
    /// The copy was renamed into place and opens as a notebook.
    Swapped,
}

pub struct RestoreRequest<'a> {
    pub backup: &'a Path,
    /// The folder that receives the restored notebook, in a new `Restored <time>` folder.
    pub parent: &'a Path,
    /// The open notebook and its backup folder. It is backed up before anything is restored, and
    /// the restore must stay outside its data.
    pub current: Option<(&'a Source, &'a Path)>,
    /// Quire's state folder for a folder notebook at a given root.
    pub state_for: &'a dyn Fn(&Path) -> PathBuf,
    /// Milliseconds since the Unix epoch.
    pub created: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Location {
    /// A restored Quire notebook, to open with this identity and writer.
    Store {
        database: PathBuf,
        notebook: String,
        writer: String,
    },
    /// A restored folder notebook and its state folder.
    Folder { root: PathBuf, state: PathBuf },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Restored {
    pub location: Location,
    /// The backup of the current notebook made first.
    pub safety: Option<BackupInfo>,
}

/// Restores `request.backup` into a new notebook. `step` runs after each step; an error from it
/// stops the restore there, as an interruption would.
pub fn restore(
    request: &RestoreRequest<'_>,
    step: &mut dyn FnMut(RestoreStep) -> Result<()>,
) -> Result<Restored> {
    let manifest = verify(request.backup)?;
    let parent = fs::canonicalize(request.parent).with_context(|| {
        format!(
            "Cannot open {}. Choose an existing folder for the restored notebook.",
            request.parent.display()
        )
    })?;
    ensure!(
        parent.is_dir(),
        "{} is not a folder. Choose a folder for the restored notebook.",
        parent.display()
    );
    ensure!(
        !overlaps(&parent, &real(request.backup)),
        "The restore folder {} overlaps the backup. Choose a folder outside the backup.",
        parent.display()
    );
    if let Some((source, _)) = request.current {
        outside_live(source, &parent, "restore folder")?;
    }
    let safety = request
        .current
        .map(|(source, destination)| {
            create(source, destination, request.created).context(
                "Quire could not back up the current notebook first, so nothing was restored. Fix the backup problem and restore again.",
            )
        })
        .transpose()?;
    step(RestoreStep::SafetyBackup)
        .context("The restore did not finish and the current notebook is unchanged.")?;
    let id = unique(request.created);
    let target = parent.join(format!("Restored {id}"));
    ensure!(
        !target.exists(),
        "{} already exists. Restore again to use a new name.",
        target.display()
    );
    let mut created = Vec::new();
    let result = match manifest.kind {
        Kind::Store => restore_store(
            request.backup,
            &manifest,
            &parent,
            &id,
            &target,
            &mut created,
            step,
        ),
        Kind::Folder => restore_folder(
            request,
            &manifest,
            &parent,
            &id,
            &target,
            &mut created,
            step,
        ),
    };
    match result {
        Ok(location) => Ok(Restored { location, safety }),
        Err(error) => {
            let notes: String = created
                .iter()
                .rev()
                .filter_map(|path| discard(path))
                .collect();
            Err(error.context(format!(
                "The restore did not finish and the current notebook is unchanged.{notes}"
            )))
        }
    }
}

/// Copies every file of `manifest` whose path starts with `prefix` into `to`, checking each copy
/// against the manifest's hash.
fn copy_files(backup: &Path, manifest: &Manifest, prefix: &str, to: &Path) -> Result<()> {
    for entry in &manifest.files {
        let Some(relative) = entry.path.strip_prefix(prefix) else {
            continue;
        };
        let copied = copy_hashed(&below(backup, &entry.path), &below(to, relative))?;
        ensure!(
            copied == (entry.hash.clone(), entry.size),
            "{} changed while it was restored. Choose another backup or retry.",
            entry.path
        );
    }
    Ok(())
}

fn rename(from: &Path, to: &Path) -> Result<()> {
    fs::rename(from, to).with_context(|| {
        format!(
            "Cannot move the restored notebook into place at {}. Check write access there and retry.",
            to.display()
        )
    })
}

/// The tables whose rows a restore keeps exactly. Publication state is reset for the new identity.
const KEPT: [&str; 5] = ["revisions", "parents", "heads", "drafts", "attachments"];

fn restore_store(
    backup: &Path,
    manifest: &Manifest,
    parent: &Path,
    id: &str,
    target: &Path,
    created: &mut Vec<PathBuf>,
    step: &mut dyn FnMut(RestoreStep) -> Result<()>,
) -> Result<Location> {
    let staging = parent.join(format!(".restoring-{id}"));
    fs::create_dir(&staging).with_context(|| {
        format!(
            "Cannot create the restored notebook in {}. Check write access and free space there, then retry.",
            parent.display()
        )
    })?;
    created.push(staging.clone());
    copy_files(backup, manifest, "", &staging)?;
    sync_tree(&staging)?;
    step(RestoreStep::Copied)?;
    let file = staging.join(STORE_DATABASE);
    let (old, writer) = database::read_only(&file)?.select_row::<(String, String)>(&format!(
        "SELECT notebook,device FROM {}.meta",
        database::ATTACHED
    ))?()?
    .context("The backed-up notebook has no identity. Choose another backup.")?;
    drop(NotebookStore::open(&file, &old, &writer)?);
    let notebook = uuid::Uuid::new_v4().to_string();
    adopt_restored(&file, &notebook, &writer)?;
    step(RestoreStep::Adopted)?;
    drop(NotebookStore::open(&file, &notebook, &writer)?);
    let counts = database::check(&file)?;
    let recorded = &manifest.databases[STORE_DATABASE];
    for table in KEPT {
        ensure!(
            counts.get(table) == recorded.get(table),
            "The restored notebook's {table} differ from the backup. The backup may be damaged; choose another backup."
        );
    }
    ensure!(
        counts.get("outbox") == counts.get("revisions"),
        "The restored notebook's history is not ready to publish. Choose another backup."
    );
    for entry in manifest
        .files
        .iter()
        .filter(|entry| entry.path != STORE_DATABASE)
    {
        ensure!(
            hash_file(&below(&staging, &entry.path))? == (entry.hash.clone(), entry.size),
            "The restored attachment {} differs from the backup. Choose another backup.",
            entry.path
        );
    }
    sync_tree(&staging)?;
    step(RestoreStep::Verified)?;
    rename(&staging, target)?;
    created.pop();
    created.push(target.to_path_buf());
    sync(parent)?;
    step(RestoreStep::Swapped)?;
    Ok(Location::Store {
        database: target.join(STORE_DATABASE),
        notebook,
        writer,
    })
}

fn restore_folder(
    request: &RestoreRequest<'_>,
    manifest: &Manifest,
    parent: &Path,
    id: &str,
    target: &Path,
    created: &mut Vec<PathBuf>,
    step: &mut dyn FnMut(RestoreStep) -> Result<()>,
) -> Result<Location> {
    let state = (request.state_for)(target);
    ensure!(
        !state.exists(),
        "Quire already keeps state at {} for a notebook at {}. Restore again to use a new name.",
        state.display(),
        target.display()
    );
    let state_parent = state
        .parent()
        .context("Quire's state folder has no parent folder.")?
        .to_path_buf();
    create_dir_durable(&state_parent)?;
    let staging = parent.join(format!(".restoring-{id}"));
    let staging_state = state_parent.join(format!(".restoring-{id}"));
    for folder in [&staging, &staging_state] {
        fs::create_dir(folder).with_context(|| {
            format!(
                "Cannot create {}. Check write access and free space there, then retry.",
                folder.display()
            )
        })?;
        created.push(folder.clone());
    }
    copy_files(request.backup, manifest, "notebook/", &staging)?;
    copy_files(request.backup, manifest, "state/", &staging_state)?;
    sync_tree(&staging)?;
    sync_tree(&staging_state)?;
    step(RestoreStep::Copied)?;
    let file = staging_state.join("folder.sqlite");
    adopt_folder(&file, target)?;
    step(RestoreStep::Adopted)?;
    let counts = database::check(&file)?;
    let recorded = &manifest.databases[STATE_DATABASE];
    for table in ["drafts", "history", "contents"] {
        ensure!(
            counts.get(table) == recorded.get(table),
            "The restored notebook's {table} differ from the backup. The backup may be damaged; choose another backup."
        );
    }
    sync_tree(&staging_state)?;
    step(RestoreStep::Verified)?;
    rename(&staging, target)?;
    created.retain(|path| path != &staging);
    created.push(target.to_path_buf());
    rename(&staging_state, &state)?;
    created.retain(|path| path != &staging_state);
    created.push(state.clone());
    sync(parent)?;
    sync(&state_parent)?;
    drop(
        FolderNotebook::open(target, &state)
            .context("The restored folder notebook does not open. Choose another backup.")?,
    );
    step(RestoreStep::Swapped)?;
    Ok(Location::Folder {
        root: target.to_path_buf(),
        state,
    })
}

/// Points the restored state at its new folder and leaves every recovery draft for review.
fn adopt_folder(file: &Path, root: &Path) -> Result<()> {
    let connection = crate::database::open_writable(file)?;
    crate::database::immediate(&connection, || {
        connection.exec_bound("UPDATE meta SET root=? WHERE singleton=1")?(
            root.to_string_lossy().as_ref(),
        )?;
        for (page, body) in
            connection.select::<(String, String)>("SELECT page,body FROM drafts")?()?
        {
            let mut draft: Draft = serde_json::from_str(&body)
                .context("A recovery draft in the backup is damaged. Choose another backup.")?;
            draft.base.clear();
            connection.exec_bound("UPDATE drafts SET body=? WHERE page=?")?((
                serde_json::to_string(&draft)?.as_str(),
                page.as_str(),
            ))?;
        }
        Ok(())
    })
}
