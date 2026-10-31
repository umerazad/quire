//! PDF and Word export through the AI CLI (reference `src/desktop/page-export.ts`): the page,
//! flattened as Markdown export flattens it and with its attachments pointed at files inside the
//! notebook, goes into a fresh folder as `page.md`; the CLI runs there with the writer's document
//! tools and writes `export.pdf` or `export.docx`; Quire opens that file, removes the folder, and
//! only then reports. Saving never replaces a file the writer did not agree to replace.

pub mod check;

use crate::ai::process::{EXPORT_TIMEOUT, Host, Mode, Outcome, Request, Transcript, Update};
use quire_core::ai::{
    Backend,
    export::{Format, PAGE_FILE, export_failed, export_prompt, invalid_file},
};
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

gpui::actions!(
    quire_exchange,
    [
        /// Formats the shown page as a PDF with the AI command line tool and your document
        /// tools, then saves it where you choose.
        ExportPagePdf,
        /// Formats the shown page as a Word document with the AI command line tool and your
        /// document tools, then saves it where you choose.
        ExportPageWord,
    ]
);

/// What an export run needs, read from the settings and the page before it starts.
#[derive(Clone, Debug)]
pub struct Job {
    pub backend: Backend,
    pub binary: String,
    pub model: String,
    pub format: Format,
    pub title: String,
    /// The flattened page with attachment links already pointing at local files.
    pub text: String,
    pub today: String,
    pub transcript: Option<Transcript>,
}

/// How an export run ended. The export folder is gone in every case.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Produced {
    /// The checked file's bytes, ready to save.
    File(Vec<u8>),
    Failed(String),
    Canceled,
}

/// The fresh folder an export runs in, removed with the run, even when the run is dropped.
struct ExportFolder(PathBuf);

impl Drop for ExportFolder {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

/// Runs one export to its checked file. `emit` sees the run's progress; `cancel` ends the CLI's
/// whole process tree. The export folder is removed before this returns.
pub async fn produce(
    host: &Host,
    job: Job,
    cancel: impl Future<Output = ()>,
    emit: impl FnMut(Update),
) -> Produced {
    let format = job.format;
    let folder = ExportFolder(
        std::env::temp_dir().join(format!("quire-export-{}", uuid::Uuid::new_v4().simple())),
    );
    let prepared = std::fs::create_dir(&folder.0)
        .and_then(|()| std::fs::write(folder.0.join(PAGE_FILE), &job.text));
    if let Err(error) = prepared {
        return Produced::Failed(export_failed(
            format,
            &format!(
                "Quire could not write the page into {} ({error}); check that the temporary folder is writable",
                folder.0.display()
            ),
        ));
    }
    let request = Request {
        backend: job.backend,
        mode: Mode::Export { today: job.today },
        binary: job.binary,
        model: job.model,
        prompt: export_prompt(&job.title, format),
        timeout: EXPORT_TIMEOUT,
        transcript: job.transcript,
    };
    let produced = match host.run_in(request, &folder.0, cancel, emit).await {
        Outcome::Canceled => Produced::Canceled,
        Outcome::Failed { message, .. } => Produced::Failed(message),
        Outcome::Done(_) => {
            let file = folder.0.join(format.file());
            let checked = std::fs::read(&file)
                .map_err(|_| None)
                .and_then(|bytes| check::check(format, &bytes).map(|()| bytes).map_err(Some));
            match checked {
                Ok(bytes) => Produced::File(bytes),
                Err(reason) => {
                    Produced::Failed(invalid_file(job.backend, format, reason.as_deref()))
                }
            }
        }
    };
    drop(folder);
    produced
}

/// What saving a finished file did.
#[derive(Debug, PartialEq, Eq)]
pub enum Saved {
    Written,
    /// The target appeared after the save dialog closed; the writer has not agreed to replace
    /// it, so nothing was written.
    Exists,
}

/// Writes `bytes` to `target`. With `replace`, the writer agreed in the save dialog to replace
/// an existing file, and the new file takes its place in one rename; without it, an existing
/// file is left alone.
pub fn save(target: &Path, bytes: &[u8], replace: bool) -> Result<Saved, String> {
    let failed = |error: std::io::Error| {
        format!(
            "Cannot write {}: {error}. Choose a folder you can write to and export again.",
            target.display()
        )
    };
    if !replace {
        let mut file = match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                return Ok(Saved::Exists);
            }
            Err(error) => return Err(failed(error)),
        };
        return match file.write_all(bytes).and_then(|()| file.sync_all()) {
            Ok(()) => Ok(Saved::Written),
            Err(error) => {
                drop(file);
                std::fs::remove_file(target).ok();
                Err(failed(error))
            }
        };
    }
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let staged = target.with_file_name(format!(".{name}.quire-{}", uuid::Uuid::new_v4().simple()));
    let written = std::fs::write(&staged, bytes).and_then(|()| std::fs::rename(&staged, target));
    if let Err(error) = written {
        std::fs::remove_file(&staged).ok();
        return Err(failed(error));
    }
    Ok(Saved::Written)
}

#[cfg(test)]
mod tests;
