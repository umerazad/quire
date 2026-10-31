//! An `rt` Quire notebook becomes a Quire database built from its published change log.
//!
//! The change log in the synced folder is the notebook; `rt`'s local `notebook.sqlite` is a
//! cache of it plus saves not yet published and drafts. Pages and history therefore come from
//! the log, read by Quire's own importer, which also covers stores from `rt` 0.9.49 and older
//! that can lack revisions their own change files hold. Quire writes under a new writer
//! identity, never one `rt` used. Saves `rt` has not published stop this notebook's migration
//! until `rt` publishes them, since Quire cannot publish another writer's saves. Drafts come
//! from a copy of `rt`'s database; one typed into a version Quire also has keeps that version
//! as its base, and any other stays for review.

use super::{
    Check, Migrated, NotebookKind, NotebookReport, Record, copy_database, report::summary,
    store_database,
};
use crate::{
    notebook::{Draft, DraftStamp, NotebookStore, RescanLimits},
    profiles::Notebook,
    sync::{
        format::{page_id_for_path, sha256},
        json::Json,
    },
};
use anyhow::{Context as _, Result, bail, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

/// A draft `rt` kept: `recovery` in its notebook database.
#[derive(Debug)]
pub(super) struct RtDraft {
    pub path: String,
    pub text: String,
    pub expected: Option<String>,
    pub saved_at: i64,
    pub base_known: bool,
}

/// What `rt`'s local database holds, read from a copy.
#[derive(Debug, Default)]
pub(super) struct RtStore {
    pub pages: BTreeMap<String, String>,
    pub revisions: Vec<String>,
    pub outbox: usize,
    pub drafts: Vec<RtDraft>,
}

pub(super) fn read_rt_store(database: &Path, scratch: &Path) -> Result<RtStore> {
    let copy = copy_database(database, scratch)?;
    let connection = &copy.connection;
    let has_base_known =
        connection.select::<String>("SELECT name FROM pragma_table_info('recovery')")?()?
            .iter()
            .any(|column| column == "base_known");
    let drafts = connection.select::<(String, String, Option<String>, i64, i64)>(&format!(
        "SELECT path,text,expected,saved_at,{} FROM recovery ORDER BY path",
        if has_base_known { "base_known" } else { "0" }
    ))?()?;
    Ok(RtStore {
        pages: connection
            .select::<(String, String)>("SELECT path,text FROM pages WHERE deleted=0")?(
        )?
        .into_iter()
        .map(|(path, text)| (path, sha256(text.as_bytes())))
        .collect(),
        revisions: connection.select::<String>("SELECT id FROM revisions")?()?,
        outbox: connection.select_row::<i64>("SELECT COUNT(*) FROM outbox")?()?.unwrap_or(0)
            as usize,
        drafts: drafts
            .into_iter()
            .map(|(path, text, expected, saved_at, base_known)| RtDraft {
                path,
                text,
                expected,
                saved_at,
                base_known: base_known != 0,
            })
            .collect(),
    })
}

/// The identity in `quire-notebook.json`, checked as `rt` checks it.
pub(super) fn notebook_identity(folder: &Path) -> Result<String> {
    let file = folder.join("quire-notebook.json");
    let identity = Json::parse(&fs::read(&file).with_context(|| {
        format!(
            "Cannot read {}. Wait for the sync tool to finish, then run the migration again",
            file.display()
        )
    })?)
    .with_context(|| {
        format!(
            "{} is damaged. Restore it from the sync tool's version history or a backup",
            file.display()
        )
    })?;
    let root = identity.root();
    let id = root.field("id").as_str().unwrap_or_default();
    ensure!(
        id.len() == 36 && id.bytes().all(|b| b.is_ascii_hexdigit() || b == b'-'),
        "{} is damaged. Restore it from the sync tool's version history or a backup",
        file.display()
    );
    ensure!(
        root.field("format").as_f64() == Some(1.),
        "{} was made by a newer version of rt. Keep using that version, or update Quire",
        folder.display()
    );
    Ok(id.to_owned())
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

/// A writer identity distinct from every one `rt` or another computer used.
fn new_writer(folder: &Path, rt_device: &str) -> String {
    let taken = writers(folder);
    loop {
        let writer = uuid::Uuid::new_v4().to_string();
        if !taken.contains(&writer) && rt_device.trim() != writer {
            return writer;
        }
    }
}

/// Reads the whole published log into `store`, in bounded passes.
fn import_log(store: &NotebookStore, folder: &Path) -> Result<Vec<String>> {
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

/// The key under which an imported draft is remembered.
fn draft_key(notebook: &str, draft: &RtDraft) -> String {
    let key = sha256(
        serde_json::to_string(&(
            notebook,
            &draft.path,
            &draft.text,
            &draft.expected,
            draft.saved_at,
        ))
        .unwrap_or_default(),
    );
    format!("draft:{key}")
}

/// Records `rt`'s drafts as Quire drafts, each once. Returns how many were imported now, before,
/// or left because Quire holds other unsaved work for the page.
fn import_drafts(
    store: &NotebookStore,
    notebook: &str,
    drafts: &[RtDraft],
    record: &mut Record,
    quire_data: &Path,
    review_only: bool,
    notes: &mut Vec<String>,
) -> Result<(usize, usize, usize)> {
    let (mut now, mut before, mut left) = (0, 0, 0);
    for draft in drafts {
        let key = draft_key(notebook, draft);
        if record.completed.contains(&key) {
            before += 1;
            continue;
        }
        let found = store.page_at(&draft.path)?;
        let page = found
            .as_ref()
            .map_or_else(|| page_id_for_path(&draft.path), |(page, _)| page.clone());
        let base = match (&found, &draft.expected) {
            (Some(_), Some(expected))
                if draft.base_known && !review_only && store.get(expected)?.is_some() =>
            {
                vec![expected.clone()]
            }
            _ => Vec::new(),
        };
        let current = store.draft(&page)?;
        if current
            .as_ref()
            .is_some_and(|current| current.text != draft.text)
        {
            left += 1;
            notes.push(format!(
                "rt's draft of {} was not imported because Quire holds other unsaved work for that page. rt still has it.",
                draft.path
            ));
            continue;
        }
        if current.is_none() {
            let saved_at = u64::try_from(draft.saved_at).unwrap_or_default();
            store.put_draft(
                &Draft {
                    page,
                    path: draft.path.clone(),
                    text: draft.text.clone(),
                    base,
                    stamp: DraftStamp {
                        session: "rt-migration".into(),
                        version: saved_at,
                    },
                    created: saved_at,
                },
                None,
            )?;
        }
        record.completed.insert(key);
        record.save(quire_data)?;
        now += 1;
    }
    Ok((now, before, left))
}

/// An `rt` Quire notebook to migrate.
pub(super) struct Source<'a> {
    /// The synced folder with `quire-notebook.json`.
    pub folder: &'a Path,
    /// `rt`'s local database for it, when there is one.
    pub local: Option<PathBuf>,
    /// `rt`'s own writer identity, never to be reused.
    pub rt_device: String,
    /// Import every draft for review, as after restoring a backup.
    pub review_only: bool,
}

impl<'a> Source<'a> {
    /// The notebook synced in `folder`, with the local database `rt` keeps in `rt_data`.
    pub fn installed(folder: &'a Path, rt_data: &Path) -> Result<Self> {
        let notebook = notebook_identity(folder)?;
        Ok(Self {
            folder,
            local: Some(
                rt_data
                    .join("notebooks")
                    .join(notebook)
                    .join("notebook.sqlite"),
            )
            .filter(|local| local.exists()),
            rt_device: fs::read_to_string(rt_data.join("device-id")).unwrap_or_default(),
            review_only: false,
        })
    }
}

/// Migrates the `rt` Quire notebook in `source`.
pub(super) fn migrate(
    source: &Source,
    quire_data: &Path,
    scratch: &Path,
    record: &mut Record,
) -> Result<(Option<Notebook>, NotebookReport)> {
    let folder = source.folder;
    let notebook = notebook_identity(folder)?;
    let rt = match &source.local {
        Some(local) => read_rt_store(local, scratch)?,
        None => RtStore::default(),
    };
    if rt.outbox > 0 {
        bail!(
            "rt holds {} saved {} that never reached the synced folder, and only rt can publish them. Open the notebook in rt and wait until it says its changes are published (quitting rt also publishes them), then run the migration again",
            rt.outbox,
            if rt.outbox == 1 { "change" } else { "changes" }
        );
    }
    let (database, writer) = match record.quire(folder) {
        Some(Migrated::Quire {
            database, writer, ..
        }) => (database.clone(), writer.clone()),
        _ => {
            let database = store_database(quire_data, &notebook);
            ensure!(
                !database.exists(),
                "Quire already has a notebook database at {} that this migration did not make. Move it aside, then run the migration again",
                database.display()
            );
            let writer = new_writer(folder, &source.rt_device);
            record.remember(Migrated::Quire {
                folder: folder.to_owned(),
                database: database.clone(),
                notebook: notebook.clone(),
                writer: writer.clone(),
            });
            record.save(quire_data)?;
            (database, writer)
        }
    };
    fs::create_dir_all(database.parent().unwrap_or(quire_data))?;
    let _lock =
        crate::notebook_lock::NotebookLock::acquire(&crate::notebook_lock::store_lock(&database))?;
    let store = NotebookStore::open(&database, &notebook, &writer)?;
    let mut notes = import_log(&store, folder)?;
    let (imported, before, left) = import_drafts(
        &store,
        &notebook,
        &rt.drafts,
        record,
        quire_data,
        source.review_only,
        &mut notes,
    )?;
    let census = store.census()?;
    let missing: Vec<&String> = rt
        .revisions
        .iter()
        .map(|id| store.get(id).map(|found| found.is_none().then_some(id)))
        .collect::<Result<Vec<_>>>()?
        .into_iter()
        .flatten()
        .collect();
    let undelivered = store.pending_attachments(None, 1)?;
    let folder_attachments = fs::read_dir(folder.join("attachments"))
        .map(|entries| entries.flatten().count())
        .unwrap_or(0);
    let review = rt
        .drafts
        .iter()
        .filter(|draft| source.review_only || !draft.base_known)
        .count();
    if review > 0 {
        notes.push(format!(
            "{review} of rt's drafts {}; they open for review and are never saved on their own.",
            if source.review_only {
                "come from a backup"
            } else {
                "did not record which version they were typed into"
            }
        ));
    }
    if !census.conflicted.is_empty() {
        notes.push(format!(
            "Pages with competing versions to resolve in Quire: {}.",
            census.conflicted.join(", ")
        ));
    }
    let checks = vec![
        Check::new(
            "Pages (count · hash of path and text)",
            summary(&rt.pages),
            summary(&census.pages),
            rt.pages.is_empty() || rt.pages == census.pages,
        ),
        Check::new(
            "Saved versions (history)",
            format!("{} in rt's copy", rt.revisions.len()),
            format!("{} ({} of rt's missing)", census.revisions, missing.len()),
            missing.is_empty(),
        ),
        Check::new(
            "Drafts",
            rt.drafts.len().to_string(),
            format!("{imported} imported now, {before} earlier, {left} left in rt"),
            imported + before + left == rt.drafts.len(),
        ),
        Check::new(
            "Saves rt has not published",
            rt.outbox.to_string(),
            "none to import".to_string(),
            rt.outbox == 0,
        ),
        Check::new(
            "Attachments",
            format!("{folder_attachments} in the synced folder"),
            format!(
                "{} copied, {} still to deliver",
                census.attachments,
                undelivered.len()
            ),
            undelivered.is_empty(),
        ),
    ];
    let report = NotebookReport {
        kind: NotebookKind::Quire,
        source: folder.to_owned(),
        destination: database.clone(),
        checks,
        notes,
    };
    Ok((
        Some(Notebook::Store {
            database: PathBuf::from(&database),
            notebook,
            writer,
        }),
        report,
    ))
}
