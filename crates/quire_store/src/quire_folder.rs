//! A Quire notebook found by its synced folder: the folder that holds `quire-notebook.json` and
//! every writer's change files. Choosing that folder on a computer opens the notebook there,
//! building this computer's database from the change files the first time, as `rt` does.

use crate::{
    durable_files::sync_directory,
    notebook::{NotebookStore, RescanLimits},
    notebook_lock::{NotebookLock, store_lock},
    profiles::Notebook,
    rt_migration::store_database,
    sync::json::Json,
};
use anyhow::{Context as _, Result, ensure};
use std::{collections::BTreeSet, fs, io::Write as _, path::Path};

/// The file that makes a folder a Quire notebook.
pub const IDENTITY_FILE: &str = "quire-notebook.json";
/// The newest notebook format this app reads and writes.
const FORMAT: u64 = 1;

/// Whether `folder` holds a Quire notebook rather than Markdown notes.
pub fn is_quire_notebook(folder: &Path) -> bool {
    folder.join(IDENTITY_FILE).exists()
}

/// The notebook identity in `folder`'s `quire-notebook.json`, checked as `rt` checks it.
pub fn read_identity(folder: &Path) -> Result<String> {
    let file = folder.join(IDENTITY_FILE);
    let bytes = fs::read(&file).with_context(|| {
        format!(
            "{} is not a Quire notebook, or its {IDENTITY_FILE} has not finished syncing. Wait for the sync tool, or choose the notebook's folder",
            folder.display()
        )
    })?;
    let damaged = || {
        format!(
            "{IDENTITY_FILE} in {} is damaged. Restore it from the sync tool's version history or a backup",
            folder.display()
        )
    };
    let identity = Json::parse(&bytes).with_context(damaged)?;
    let root = identity.root();
    let id = root.field("id").as_str().unwrap_or_default();
    let format = root.field("format").as_f64();
    ensure!(
        id.len() == 36
            && id
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b) || b == b'-')
            && format.is_some(),
        "{}",
        damaged()
    );
    ensure!(
        format.is_some_and(|format| format <= FORMAT as f64),
        "This notebook was made by a newer version of Quire. Update Quire to open it"
    );
    Ok(id.to_owned())
}

/// Makes `folder`, which must hold no files other than hidden ones, a new Quire notebook and
/// returns its identity. `now` is milliseconds since the epoch. The identity file is written
/// once and never replaced, so a sync tool never has two versions of it to choose between.
pub fn create(folder: &Path, now: u64) -> Result<String> {
    fs::create_dir_all(folder).with_context(|| {
        format!(
            "Cannot create {}. Choose a folder you can write to",
            folder.display()
        )
    })?;
    let occupied = fs::read_dir(folder)?
        .flatten()
        .any(|entry| !entry.file_name().to_string_lossy().starts_with('.'));
    ensure!(
        !occupied,
        "Choose an empty folder for a new Quire notebook. The folder you chose already has files in it"
    );
    let id = uuid::Uuid::new_v4().to_string();
    let identity = serde_json::to_string_pretty(
        &serde_json::json!({"format": FORMAT, "id": id, "created": now}),
    )?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(folder.join(IDENTITY_FILE))
        .with_context(|| {
            format!(
                "Cannot write {IDENTITY_FILE} in {}. Choose an empty folder you can write to",
                folder.display()
            )
        })?;
    file.write_all(format!("{identity}\n").as_bytes())?;
    file.sync_all()?;
    sync_directory(folder).ok();
    Ok(id)
}

/// The Quire notebook synced in `folder`, as this computer opens it. Its database lives under
/// `quire_data`; the first time, it is made under a writer identity no other computer used and
/// filled from the folder's change files. Returns the notebook and any change file that could
/// not be read yet, which later rescans retry.
pub fn open(folder: &Path, quire_data: &Path) -> Result<(Notebook, Vec<String>)> {
    let folder = fs::canonicalize(folder).with_context(|| {
        format!(
            "Cannot open {}. Choose an existing notebook folder",
            folder.display()
        )
    })?;
    let notebook = read_identity(&folder)?;
    let database = store_database(quire_data, &notebook);
    let (writer, problems) = if database.exists() {
        let identity = NotebookStore::identity(&database)?;
        ensure!(
            identity.notebook == notebook,
            "{} belongs to another notebook than {}. Move it aside, then choose the folder again",
            database.display(),
            folder.display()
        );
        (identity.writer, Vec::new())
    } else {
        let directory = database.parent().unwrap_or(quire_data);
        fs::create_dir_all(directory).with_context(|| {
            format!(
                "Cannot create {}. Check that Quire's data folder is writable, then choose the folder again",
                directory.display()
            )
        })?;
        let writer = new_writer(&folder);
        let _lock = NotebookLock::acquire(&store_lock(&database))?;
        let store = NotebookStore::open(&database, &notebook, &writer)?;
        let problems = import_log(&store, &folder)?;
        (writer, problems)
    };
    Ok((
        Notebook::Store {
            database,
            notebook,
            writer,
            folder: Some(folder),
        },
        problems,
    ))
}

/// Every writer identity the synced folder holds.
fn writers(folder: &Path) -> BTreeSet<String> {
    fs::read_dir(folder.join("devices"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// A writer identity distinct from every one in the folder and from `taken`.
pub(crate) fn new_writer_except(folder: &Path, taken: &str) -> String {
    let used = writers(folder);
    loop {
        let writer = uuid::Uuid::new_v4().to_string();
        if !used.contains(&writer) && taken.trim() != writer {
            return writer;
        }
    }
}

fn new_writer(folder: &Path) -> String {
    new_writer_except(folder, "")
}

/// Reads the whole published log into `store`, in bounded passes. Returns the change files that
/// could not be read or still wait for earlier ones.
pub(crate) fn import_log(store: &NotebookStore, folder: &Path) -> Result<Vec<String>> {
    let mut problems = Vec::new();
    let mut cursor = None;
    loop {
        let report = store.rescan(folder, RescanLimits::default(), cursor.as_ref())?;
        problems.extend(report.problems.iter().map(|problem| {
            format!(
                "{}{}: {}",
                problem.device.as_deref().unwrap_or("folder"),
                problem
                    .file
                    .as_deref()
                    .map(|file| format!("/{file}"))
                    .unwrap_or_default(),
                problem.reason
            )
        }));
        problems.extend(
            report
                .waiting
                .iter()
                .map(|wait| format!("Waiting for delivery: {wait:?}")),
        );
        match report.continuation {
            Some(next) => cursor = Some(next),
            None => return Ok(problems),
        }
    }
}
