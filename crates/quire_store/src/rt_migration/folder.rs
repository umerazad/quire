//! An `rt` folder notebook stays where it is: both apps edit the same Markdown files. Adopting it
//! opens it once with Quire's state folder, which imports `rt`'s drafts and saved versions from
//! copies of its `.quire` databases (NR13), then compares the notes as `rt` reads them (raw file
//! bytes) with the notes as Quire reads them.

use super::{Check, Migrated, NotebookKind, NotebookReport, Record, report::summary};
use crate::{
    folder_notebook::{FolderNotebook, hashed_listing, read, state_folder_in},
    notebook_lock::{NotebookLock, folder_lock},
    profiles::Notebook,
    sync::format::sha256,
};
use anyhow::Result;
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

/// Files in each `.quire/devices/<device>/<kind>` folder.
fn device_files(root: &Path, kind: &str) -> usize {
    fs::read_dir(root.join(".quire/devices"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|device| {
            fs::read_dir(device.path().join(kind))
                .map(|files| files.flatten().count())
                .unwrap_or(0)
        })
        .sum()
}

pub(super) fn adopt(
    root: &Path,
    quire_data: &Path,
    record: &mut Record,
) -> Result<(Option<Notebook>, NotebookReport)> {
    let root = fs::canonicalize(root)?;
    let state = state_folder_in(quire_data, &root);
    fs::create_dir_all(&state)?;
    let _lock = NotebookLock::acquire(&folder_lock(&state))?;
    record.remember(Migrated::Folder {
        root: root.clone(),
        state: state.clone(),
    });
    record.save(quire_data)?;
    let (pages, others) = hashed_listing(&root)?;
    let notebook = FolderNotebook::open(&root, &state)?;
    let tally = notebook.reference_tally()?;
    let census = notebook.census()?;
    let read_by_quire: BTreeMap<String, String> = pages
        .keys()
        .map(|path| {
            let text = read(&root, path)?.map(|document| sha256(document.text.as_bytes()));
            Ok((path.clone(), text.unwrap_or_default()))
        })
        .collect::<Result<_>>()?;
    let settings = root.join(".quire/notebook.json");
    let settings = fs::read(&settings)
        .map(|bytes| sha256(bytes)[..12].to_owned())
        .unwrap_or_else(|_| "none".into());
    let journals = device_files(&root, "transactions");
    let retired = device_files(&root, "retired");
    let mut notes = notebook.notices();
    notes.extend(notebook.warnings());
    if retired > 0 {
        notes.push(format!(
            "{retired} originals of notes rt moved stay in .quire/devices/*/retired, where rt's history finds them."
        ));
    }
    let checks = vec![
        Check::new(
            "Notes (count · hash of path and text)",
            summary(&pages),
            summary(&read_by_quire),
            pages == read_by_quire,
        ),
        Check::new(
            "Attachments and other files",
            summary(&others),
            summary(&census.others),
            others == census.others,
        ),
        Check::new(
            "Drafts",
            tally.drafts.to_string(),
            format!("{} imported or already in the file", tally.drafts_accounted),
            tally.drafts == tally.drafts_accounted,
        ),
        Check::new(
            "Saved versions (history)",
            tally.history.to_string(),
            format!("{} of them in Quire's history", tally.history_present),
            tally.history == tally.history_present,
        ),
        Check::new(
            "Daily note layout (.quire/notebook.json)",
            settings.clone(),
            format!("{settings}, read in place"),
            true,
        ),
        Check::new(
            "rt's interrupted multi-note changes",
            journals.to_string(),
            if journals == 0 {
                "none".to_string()
            } else {
                "not finished by Quire; open the notebook in rt once so it finishes them".into()
            },
            journals == 0,
        ),
    ];
    Ok((
        Some(Notebook::Folder { root: root.clone() }),
        NotebookReport {
            kind: NotebookKind::Folder,
            source: root,
            destination: PathBuf::from(&state),
            checks,
            notes,
        },
    ))
}
