use super::{BatchImport, ImportedPage, MergedPage, NotebookStore};
use crate::{
    durable_files,
    sync::{
        format,
        json::{JsString, Json},
    },
};
use anyhow::{Context as _, Result, ensure};
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug)]
pub struct RescanLimits {
    pub batches: usize,
    pub attachments: usize,
    pub batch_bytes: u64,
    pub attachment_bytes: u64,
}
impl Default for RescanLimits {
    fn default() -> Self {
        Self {
            batches: 64,
            attachments: 32,
            batch_bytes: 64 * 1024 * 1024,
            attachment_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RescanCursor {
    pub after_device: Option<String>,
    pub after_attachment: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RescanWait {
    Sequence {
        device: String,
        expected: u32,
    },
    File {
        device: String,
        file: String,
    },
    Parents {
        device: String,
        file: String,
        ids: Vec<String>,
    },
    Attachment {
        name: String,
    },
}

#[derive(Debug)]
pub struct RescanProblem {
    pub device: Option<String>,
    pub file: Option<String>,
    pub reason: String,
    pub newer_format: Option<f64>,
}

#[derive(Debug, Default)]
pub struct RescanReport {
    pub applied: usize,
    pub batches: usize,
    pub adopted: usize,
    pub attachments_completed: usize,
    pub batch_bytes: u64,
    pub attachment_bytes: u64,
    pub pages: Vec<ImportedPage>,
    pub waiting: Vec<RescanWait>,
    pub problems: Vec<RescanProblem>,
    pub continuation: Option<RescanCursor>,
    /// Merges made after a complete graph pass; warn about pages with overlaps.
    pub merged: Vec<MergedPage>,
}

fn problem(device: Option<&str>, file: Option<&str>, reason: String) -> RescanProblem {
    RescanProblem {
        device: device.map(str::to_owned),
        file: file.map(str::to_owned),
        reason,
        newer_format: None,
    }
}

fn valid_device(device: &str) -> bool {
    !device.is_empty()
        && device
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn ordinary_if_present(path: &Path) -> Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            durable_files::ordinary_directory(path)?;
            Ok(true)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context(
            "Cannot inspect a notebook folder. Check delivery and permissions before retrying.",
        ),
    }
}

struct DeviceLog {
    device: String,
    directory: PathBuf,
    files: BTreeMap<u32, (String, String)>,
}

impl NotebookStore {
    fn device_log(&self, root: &Path, device: &str) -> Result<DeviceLog> {
        durable_files::ordinary_directory(&root.join("devices").join(device))?;
        let directory = root.join("devices").join(device).join("log");
        let mut files = BTreeMap::new();
        if ordinary_if_present(&directory)? {
            for entry in fs::read_dir(&directory)? {
                let entry = entry?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else {
                    continue;
                };
                let Some(parsed) = format::parse_name(name, format::Kind::Batch) else {
                    continue;
                };
                ensure!(
                    parsed.seq > 0 && entry.file_type()?.is_file(),
                    "A change filename is invalid or linked. Preserve it and restore its original ordinary file before retrying."
                );
                if let Some(known) = self.batch_hash(device, parsed.seq)? {
                    ensure!(
                        known == parsed.hash,
                        "This device has a conflicting historical change file. Preserve both copies and restore a distinct writer identity before syncing."
                    );
                }
                ensure!(
                    files
                        .insert(parsed.seq, (name.to_owned(), parsed.hash.to_owned()))
                        .is_none(),
                    "This device has several files for one sequence. Preserve them and restore a distinct writer identity before syncing."
                );
            }
        }
        Ok(DeviceLog {
            device: device.into(),
            directory,
            files,
        })
    }

    /// Imports a bounded delivered prefix; each batch commits independently.
    /// Earlier commits remain applied if a later storage error is returned.
    /// Byte budgets count payload sizes, allowing one oversized item alone in each phase.
    pub fn rescan(
        &self,
        root: &Path,
        limits: RescanLimits,
        resume: Option<&RescanCursor>,
    ) -> Result<RescanReport> {
        ensure!(
            limits.batches > 0
                && limits.attachments > 0
                && limits.batch_bytes > 0
                && limits.attachment_bytes > 0,
            "Rescan limits must be positive. Allow at least one batch and attachment before retrying."
        );
        self.require_reconciliation()?;
        verify_notebook(root, &self.notebook, &self.device)?;
        let mut report = RescanReport::default();
        let mut logs = Vec::new();
        let devices = root.join("devices");
        if ordinary_if_present(&devices)? {
            for entry in fs::read_dir(&devices)? {
                let entry = entry?;
                let raw = entry.file_name();
                let device = raw.to_string_lossy();
                if device.starts_with('.') {
                    continue;
                }
                let kind = entry.file_type()?;
                if kind.is_file() {
                    continue;
                }
                if !valid_device(&device) || kind.is_symlink() || !kind.is_dir() {
                    report.problems.push(problem(Some(&device),None,"Device metadata must use an ordinary folder with its original safe writer identity. Restore that folder before retrying.".into()));
                    continue;
                }
                match self.device_log(root, &device) {
                    Ok(log) => logs.push(log),
                    Err(error) => {
                        report
                            .problems
                            .push(problem(Some(&device), None, format!("{error:#}")))
                    }
                }
            }
        }
        logs.sort_by(|a, b| a.device.cmp(&b.device));
        let start = resume
            .and_then(|cursor| cursor.after_device.as_ref())
            .map_or(0, |after| logs.partition_point(|log| &log.device <= after));
        let order: Vec<_> = (start..logs.len()).chain(0..start).collect();
        let mut waiting = BTreeMap::new();
        let mut problems = BTreeMap::new();
        let mut pages = BTreeMap::<JsString, ImportedPage>::new();
        let mut next = RescanCursor::default();
        let mut graph_more = false;
        let mut last_device = None;
        'passes: loop {
            let mut progressed = false;
            for &index in &order {
                let log = &logs[index];
                waiting.remove(&log.device);
                problems.remove(&log.device);
                loop {
                    let at = self.cursor(&log.device)?.map_or(0, |cursor| cursor.seq);
                    let expected = at + 1;
                    let Some((name, _)) = log.files.get(&expected) else {
                        if log
                            .files
                            .last_key_value()
                            .is_some_and(|(seq, _)| *seq > expected)
                        {
                            waiting.insert(
                                log.device.clone(),
                                RescanWait::Sequence {
                                    device: log.device.clone(),
                                    expected,
                                },
                            );
                        }
                        break;
                    };
                    if report.batches >= limits.batches {
                        graph_more = true;
                        next.after_device = last_device.clone();
                        break 'passes;
                    }
                    let candidate = (|| -> Result<Option<Vec<u8>>> {
                        durable_files::ordinary_directory(root)?;
                        durable_files::ordinary_directory(&devices)?;
                        durable_files::ordinary_directory(log.directory.parent().unwrap())?;
                        durable_files::ordinary_directory(&log.directory)?;
                        let size = fs::symlink_metadata(log.directory.join(name))?.len();
                        if report.batch_bytes > 0
                            && report.batch_bytes.saturating_add(size) > limits.batch_bytes
                        {
                            return Ok(None);
                        }
                        read_ordinary(
                            &log.directory.join(name),
                            format::MAX_BATCH_BYTES + 1024 * 1024,
                        )
                        .map(Some)
                    })();
                    let data = match candidate {
                        Ok(None) => {
                            graph_more = true;
                            next.after_device = last_device.clone();
                            break 'passes;
                        }
                        Ok(Some(data)) => data,
                        Err(error) => {
                            if error.downcast_ref::<io::Error>().is_some_and(|error| {
                                matches!(
                                    error.kind(),
                                    io::ErrorKind::NotFound
                                        | io::ErrorKind::PermissionDenied
                                        | io::ErrorKind::WouldBlock
                                )
                            }) {
                                waiting.insert(
                                    log.device.clone(),
                                    RescanWait::File {
                                        device: log.device.clone(),
                                        file: name.clone(),
                                    },
                                );
                            } else {
                                problems.insert(
                                    log.device.clone(),
                                    problem(Some(&log.device), Some(name), format!("{error:#}")),
                                );
                            }
                            break;
                        }
                    };
                    last_device = Some(log.device.clone());
                    report.batch_bytes = report.batch_bytes.saturating_add(data.len() as u64);
                    match self.import_batch(&log.device, name, &data)? {
                        BatchImport::Applied(imported) => {
                            report.applied += imported.applied;
                            report.batches += 1;
                            for page in imported.pages {
                                pages.insert(page.page.clone(), page);
                            }
                            progressed = true;
                        }
                        BatchImport::OwnPublicationPending => {
                            let pending = self.pending_publication()?.context("The frozen publication changed. Rescan the writer before retrying.")?;
                            if pending.name != *name || pending.data != data {
                                problems.insert(log.device.clone(),problem(Some(&log.device),Some(name),"The frozen publication changed after this file was read. Keep local saves and rescan before adopting it.".into()));
                                break;
                            }
                            match self.adopt_written_publication(root, &pending) {
                                Ok(_) => {
                                    report.batches += 1;
                                    report.adopted += 1;
                                    progressed = true;
                                }
                                Err(error) => {
                                    problems.insert(
                                        log.device.clone(),
                                        problem(
                                            Some(&log.device),
                                            Some(name),
                                            format!("{error:#}"),
                                        ),
                                    );
                                    break;
                                }
                            }
                        }
                        BatchImport::Duplicate => {
                            progressed = true;
                        }
                        BatchImport::WaitingForSequence { expected } => {
                            waiting.insert(
                                log.device.clone(),
                                RescanWait::Sequence {
                                    device: log.device.clone(),
                                    expected,
                                },
                            );
                            break;
                        }
                        BatchImport::MissingParents { ids } => {
                            waiting.insert(
                                log.device.clone(),
                                RescanWait::Parents {
                                    device: log.device.clone(),
                                    file: name.clone(),
                                    ids,
                                },
                            );
                            break;
                        }
                        BatchImport::Rejected {
                            reason,
                            newer_format,
                        } => {
                            problems.insert(
                                log.device.clone(),
                                RescanProblem {
                                    device: Some(log.device.clone()),
                                    file: Some(name.clone()),
                                    reason,
                                    newer_format,
                                },
                            );
                            break;
                        }
                    }
                }
            }
            if !progressed {
                break;
            }
        }
        report.waiting.extend(waiting.into_values());
        report.problems.extend(problems.into_values());
        let attachment_more = self.rescan_attachments(
            root,
            limits,
            resume.and_then(|cursor| cursor.after_attachment.as_deref()),
            &mut report,
            &mut next,
        )?;
        let root_valid = match verify_notebook(root, &self.notebook, &self.device) {
            Ok(()) => true,
            Err(error) => {
                report.problems.push(problem(
                    Some(&self.device),
                    Some("quire-notebook.json"),
                    format!("{error:#}"),
                ));
                false
            }
        };
        if root_valid
            && !report
                .problems
                .iter()
                .any(|problem| problem.device.as_deref() == Some(&self.device))
            && self.pending_publication()?.is_none()
        {
            let cursor = self.cursor(&self.device)?;
            let own = logs.iter().find(|log| log.device == self.device);
            let at = cursor.as_ref().map_or(0, |cursor| cursor.seq);
            let current_present = cursor.as_ref().is_none_or(|cursor| {
                own.is_some_and(|log| {
                    log.files
                        .get(&cursor.seq)
                        .is_some_and(|(_, hash)| *hash == cursor.hash)
                })
            });
            if current_present
                && own
                    .is_none_or(|log| log.files.last_key_value().is_none_or(|(seq, _)| *seq <= at))
            {
                self.mark_reconciled(cursor.as_ref())?;
                // Merge only once every own batch is imported; see `merge_conflicts`.
                if !graph_more {
                    let merges = self.merge_conflicts()?;
                    for merged in merges.merged {
                        let heads = self.heads_key(&merged.page.to_json())?;
                        pages.insert(
                            merged.page.clone(),
                            ImportedPage {
                                page: merged.page.clone(),
                                heads,
                            },
                        );
                        report.merged.push(merged);
                    }
                    report.problems.extend(
                        merges
                            .failed
                            .into_iter()
                            .map(|failure| problem(None, None, failure.reason)),
                    );
                }
            } else if !current_present && let Some(cursor) = cursor {
                report.waiting.push(RescanWait::File {
                    device: self.device.clone(),
                    file: format!("{:09}-{}.qlog.zst", cursor.seq, cursor.hash),
                });
            }
        }
        report.pages = pages.into_values().collect();
        if graph_more || attachment_more {
            report.continuation = Some(next);
        }
        Ok(report)
    }

    fn rescan_attachments(
        &self,
        root: &Path,
        limits: RescanLimits,
        after: Option<&str>,
        report: &mut RescanReport,
        next: &mut RescanCursor,
    ) -> Result<bool> {
        let local = self.directory.join("attachments");
        let remote = root.join("attachments");
        for directory in [&local, &remote] {
            if let Err(error) = ordinary_if_present(directory) {
                report
                    .problems
                    .push(problem(None, Some("attachments"), format!("{error:#}")));
                return Ok(false);
            }
        }
        let mut after = after.map(str::to_owned);
        let mut attempted = 0;
        while attempted < limits.attachments {
            let names = self.pending_attachments(
                after.as_deref(),
                (limits.attachments - attempted).min(2000),
            )?;
            if names.is_empty() {
                break;
            }
            for name in names {
                let local_file = local.join(&name);
                let source = if fs::symlink_metadata(&local_file).is_ok() {
                    local_file
                } else {
                    remote.join(&name)
                };
                let size = fs::symlink_metadata(&source).map(|metadata| metadata.len());
                if let Ok(size) = size
                    && report.attachment_bytes > 0
                    && report.attachment_bytes.saturating_add(size) > limits.attachment_bytes
                {
                    next.after_attachment = after;
                    return Ok(true);
                }
                attempted += 1;
                match size {
                    Ok(size) => {
                        report.attachment_bytes = report.attachment_bytes.saturating_add(size);
                        match self.cache_attachment_file(&name,&source) {
                            Ok(()) => report.attachments_completed += 1,
                            Err(error) if self.poisoned.get() => return Err(error),
                            Err(error) => report.problems.push(problem(None,Some(&format!("attachments/{name}")),format!("{error:#}"))),
                        }
                    },
                    Err(error) if matches!(error.kind(),io::ErrorKind::NotFound|io::ErrorKind::PermissionDenied|io::ErrorKind::WouldBlock) => report.waiting.push(RescanWait::Attachment {name:name.clone()}),
                    Err(error) => report.problems.push(problem(None,Some(&format!("attachments/{name}")),format!("Cannot inspect attachment delivery ({error}). Check folder permissions and retry."))),
                }
                after = Some(name);
            }
        }
        if !self.pending_attachments(after.as_deref(), 1)?.is_empty() {
            next.after_attachment = after;
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

pub(super) fn read_ordinary(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).with_context(|| {
        format!(
            "Cannot read {}. Wait for delivery or restore the original file before retrying.",
            path.display()
        )
    })?;
    ensure!(
        metadata.is_file() && metadata.len() <= limit as u64,
        "Storage file {} is linked, nonordinary, or too large. Preserve it and restore the original file before retrying.",
        path.display()
    );
    let mut data = Vec::new();
    File::open(path)?
        .take(limit as u64 + 1)
        .read_to_end(&mut data)?;
    ensure!(
        data.len() <= limit,
        "Storage file grew while reading it. Wait for delivery to finish before retrying."
    );
    Ok(data)
}

pub(super) fn verify_notebook(root: &Path, notebook: &str, device: &str) -> Result<()> {
    ensure!(
        root.is_absolute(),
        "Notebook publication needs an absolute folder path. Choose the notebook folder and retry."
    );
    ensure!(
        valid_device(device),
        "Writer identity is not a safe folder name. Restore its original local identity before publishing."
    );
    durable_files::ordinary_directory(root)?;
    let identity = Json::parse(&read_ordinary(
        &root.join("quire-notebook.json"),
        1024 * 1024,
    )?)?;
    let identity = identity.root();
    ensure!(
        identity.field("format").as_f64() == Some(1.),
        "Notebook format is unsupported or newer than this app. Preserve its files and use a compatible Quire version."
    );
    ensure!(
        identity.field("id").as_str() == Some(notebook),
        "This folder belongs to another notebook. Choose the original notebook folder before publishing."
    );
    Ok(())
}
