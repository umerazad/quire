//! Roam exchange files, as the reference `roam-files.ts` handles them: reading a Roam export
//! within its size limit, publishing an imported graph as a new folder notebook in one rename
//! with its metadata, original archive, and report under `.quire`, reading that metadata back for
//! an export, and writing an export without replacing an earlier one. The export file is only
//! read.

use crate::folder_notebook::{METADATA, barrier, flush, flush_directory};
use anyhow::{Context as _, Result, bail};
use quire_core::exchange::{js_json, roam::Document};
use serde_json::Value;
use std::{
    fs::{self, File},
    io::{self, Write as _},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

/// The largest Roam export Quire reads.
pub const MAX_SOURCE_BYTES: u64 = 100 * 1024 * 1024;
/// The metadata file an import writes and an export reads, inside `.quire`.
pub const METADATA_FILE: &str = "roam.json";
pub const IMPORT_CANCELED: &str =
    "The Roam import was canceled. Nothing was written, and the original export is unchanged.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Json,
    Edn,
}

impl Format {
    fn extension(self) -> &'static str {
        match self {
            Self::Json => "json",
            Self::Edn => "edn",
        }
    }
}

/// A Roam export read whole: its text, which is also its exact bytes, and its format.
#[derive(Debug)]
pub struct RoamSource {
    pub text: String,
    pub format: Format,
}

/// Reads a Roam export: EDN when its extension is `.edn`, otherwise JSON. Refuses exports over
/// 100 MB and text that is not UTF-8.
pub fn read_source(path: &Path) -> Result<RoamSource> {
    let unreadable = || {
        format!(
            "Cannot read {}. Check that the file exists and Quire can read it, then import again.",
            path.display()
        )
    };
    if fs::metadata(path).with_context(unreadable)?.len() > MAX_SOURCE_BYTES {
        bail!("This Roam export exceeds 100 MB. Split the graph before importing.");
    }
    let bytes = fs::read(path).with_context(unreadable)?;
    let format = match path.extension().and_then(|extension| extension.to_str()) {
        Some(extension) if extension.eq_ignore_ascii_case("edn") => Format::Edn,
        _ => Format::Json,
    };
    let text = String::from_utf8(bytes).map_err(|_| {
        anyhow::anyhow!(
            "The Roam export is not valid UTF-8 text. Export again as JSON or EDN and select the extracted file."
        )
    })?;
    Ok(RoamSource { text, format })
}

/// The graph an import publishes.
pub struct ImportedGraph<'a> {
    pub documents: &'a [Document],
    /// The metadata as `JSON.stringify` writes it.
    pub metadata: &'a str,
    pub source: &'a RoamSource,
    pub report: Vec<String>,
}

fn create(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::options().write(true).create_new(true).open(path)?;
    set_private(&file)?;
    file.write_all(bytes)?;
    flush(&file)
}

#[cfg(unix)]
fn set_private(file: &File) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    file.set_permissions(fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private(_file: &File) -> io::Result<()> {
    Ok(())
}

/// Publishes an imported graph as the folder notebook `<parent>/<name>`: every page, then
/// `.quire/roam.json`, `.quire/source.<json|edn>` holding the export's exact bytes, and
/// `.quire/import-report.txt`, written into a hidden staging folder that one rename turns into the
/// notebook. A failure or `cancel` removes the staging folder, so nothing is left behind. Returns
/// the notebook folder and the report, which gains the archive line.
pub fn publish_import(
    parent: &Path,
    name: &str,
    graph: ImportedGraph<'_>,
    cancel: &AtomicBool,
    mut progress: impl FnMut(usize),
) -> Result<(PathBuf, Vec<String>)> {
    let parent = fs::canonicalize(parent).with_context(|| {
        format!(
            "Cannot open {}. Choose an existing parent folder and retry.",
            parent.display()
        )
    })?;
    if !name.starts_with("Roam import ") || Path::new(name).components().count() != 1 {
        bail!("The import folder is invalid. Choose a parent folder and retry.");
    }
    let root = parent.join(name);
    let staging = parent.join(format!(".{name}.partial"));
    fs::create_dir(&staging).with_context(|| {
        format!(
            "Cannot create the imported notebook in {}. Check free space and write access, then import again.",
            parent.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(&staging, fs::Permissions::from_mode(0o700))?;
    }
    let written = (|| {
        let mut report = graph.report;
        for (count, document) in graph.documents.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                bail!(IMPORT_CANCELED);
            }
            create(&staging.join(&document.path), document.text.as_bytes()).with_context(|| {
                format!(
                    "Cannot write {} in the imported notebook. Check free space and write access, then import again; nothing was imported.",
                    document.path
                )
            })?;
            progress(count + 1);
        }
        let metadata = staging.join(METADATA);
        fs::create_dir(&metadata)?;
        create(&metadata.join(METADATA_FILE), graph.metadata.as_bytes())?;
        let archive = format!("source.{}", graph.source.format.extension());
        create(&metadata.join(&archive), graph.source.text.as_bytes())?;
        report.push(format!(
            "Exact original export archived in .quire/{archive}."
        ));
        create(
            &metadata.join("import-report.txt"),
            format!("{}\n", report.join("\n")).as_bytes(),
        )?;
        flush_directory(&metadata)?;
        flush_directory(&staging)?;
        barrier([staging.as_path()])?;
        if cancel.load(Ordering::Relaxed) {
            bail!(IMPORT_CANCELED);
        }
        fs::rename(&staging, &root).with_context(|| {
            format!(
                "Cannot publish the imported notebook as {}. Check write access to {}, then import again; nothing was imported.",
                root.display(),
                parent.display()
            )
        })?;
        flush_directory(&parent)?;
        barrier([parent.as_path()])?;
        Ok(report)
    })();
    match written {
        Ok(report) => Ok((root, report)),
        Err(error) => {
            fs::remove_dir_all(&staging).ok();
            Err(error)
        }
    }
}

/// The Roam metadata of the folder notebook at `root`, or None when it has none.
pub fn read_metadata(root: &Path) -> Result<Option<Value>> {
    const UNREADABLE: &str = "The Roam metadata file could not be read. Restore .quire/roam.json from a backup before exporting.";
    match fs::read(root.join(METADATA).join(METADATA_FILE)) {
        Ok(bytes) => js_json::parse(&String::from_utf8_lossy(&bytes))
            .map(Some)
            .map_err(|_| anyhow::anyhow!(UNREADABLE)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => bail!(UNREADABLE),
    }
}

/// Whether `path` is or lies inside `folder`, comparing real paths.
fn overlaps(path: &Path, folder: &Path) -> bool {
    let real = |path: &Path| fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let (path, folder) = (real(path), real(folder));
    path.starts_with(&folder) || folder.starts_with(&path)
}

/// Writes a Roam export to `destination` without replacing any file, then its report beside it
/// as `<destination>.report.txt`. Refuses a destination inside one of `forbidden`, the live
/// notebook and Quire's data folders. Returns the report, which notes a report file it could not
/// write.
pub fn write_export(
    destination: &Path,
    forbidden: &[PathBuf],
    text: &str,
    mut report: Vec<String>,
) -> Result<Vec<String>> {
    let folder = destination
        .parent()
        .context("Choose a file inside a folder for the export.")?;
    let name = destination
        .file_name()
        .context("Choose a file name for the export.")?;
    let destination = fs::canonicalize(folder)
        .with_context(|| {
            format!(
                "Cannot open {}. Choose an existing folder for the export.",
                folder.display()
            )
        })?
        .join(name);
    if forbidden
        .iter()
        .any(|folder| overlaps(&destination, folder))
    {
        bail!("Choose an export filename outside live notebooks and Quire's app-data folder.");
    }
    match create(&destination, text.as_bytes()) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            bail!("The export file already exists. Choose a new filename to keep previous exports.")
        }
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Cannot write {}. Check free space and write access, then export again.",
                    destination.display()
                )
            });
        }
    }
    let mut report_file = destination.into_os_string();
    report_file.push(".report.txt");
    if create(
        Path::new(&report_file),
        format!("{}\n", report.join("\n")).as_bytes(),
    )
    .is_err()
    {
        report.push(
            "The JSON export was saved, but its report file could not be written. Review these notices in Quire."
                .to_owned(),
        );
    }
    if let Some(folder) = Path::new(&report_file).parent() {
        flush_directory(folder).ok();
        barrier([folder]).ok();
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn graph<'a>(documents: &'a [Document], source: &'a RoamSource) -> ImportedGraph<'a> {
        ImportedGraph {
            documents,
            metadata: r#"{"version":1,"pages":{}}"#,
            source,
            report: vec!["Imported 2 pages and 0 blocks. Original export unchanged.".into()],
        }
    }

    fn documents() -> Vec<Document> {
        ["One", "Two"]
            .iter()
            .map(|title| Document {
                path: format!("{title}.md"),
                text: format!("---\ntitle: \"{title}\"\n---\n\n"),
            })
            .collect()
    }

    #[test]
    fn an_import_publishes_pages_metadata_archive_and_report_in_one_folder() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let source = RoamSource {
            text: r#"[{"title":"One"},{"title":"Two"}]"#.into(),
            format: Format::Json,
        };
        let documents = documents();
        let mut counted = Vec::new();
        let (root, report) = publish_import(
            parent.path(),
            "Roam import x",
            graph(&documents, &source),
            &AtomicBool::new(false),
            |count| counted.push(count),
        )?;
        assert_eq!(root, fs::canonicalize(parent.path())?.join("Roam import x"));
        assert_eq!(counted, [1, 2]);
        assert_eq!(fs::read_to_string(root.join("One.md"))?, documents[0].text);
        assert_eq!(
            fs::read_to_string(root.join(".quire/source.json"))?,
            source.text
        );
        assert_eq!(
            fs::read_to_string(root.join(".quire/roam.json"))?,
            r#"{"version":1,"pages":{}}"#
        );
        assert_eq!(
            report.last().map(String::as_str),
            Some("Exact original export archived in .quire/source.json.")
        );
        assert_eq!(
            fs::read_to_string(root.join(".quire/import-report.txt"))?,
            format!("{}\n", report.join("\n"))
        );
        assert_eq!(
            fs::read_dir(parent.path())?.count(),
            1,
            "no staging folder is left"
        );
        Ok(())
    }

    #[test]
    fn a_failed_or_canceled_import_leaves_the_parent_as_it_was() -> Result<()> {
        let parent = tempfile::tempdir()?;
        let source = RoamSource {
            text: "#datascript/DB {}".into(),
            format: Format::Edn,
        };
        let mut documents = documents();
        documents[1].path = "One.md".into();
        let error = publish_import(
            parent.path(),
            "Roam import x",
            graph(&documents, &source),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert!(
            format!("{error:#}").contains("Cannot write One.md"),
            "{error:#}"
        );
        assert_eq!(fs::read_dir(parent.path())?.count(), 0);

        let cancel = AtomicBool::new(false);
        let error = publish_import(
            parent.path(),
            "Roam import y",
            graph(&documents[..1], &source),
            &cancel,
            |_| cancel.store(true, Ordering::Relaxed),
        )
        .unwrap_err();
        assert_eq!(error.to_string(), IMPORT_CANCELED);
        assert_eq!(fs::read_dir(parent.path())?.count(), 0);

        let error = publish_import(
            parent.path(),
            "../Roam import z",
            graph(&documents[..1], &source),
            &AtomicBool::new(false),
            |_| {},
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "The import folder is invalid. Choose a parent folder and retry."
        );
        Ok(())
    }

    #[test]
    fn sources_are_read_within_the_limit_as_utf8() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let edn = folder.path().join("graph.EDN");
        fs::write(&edn, "#datascript/DB {}")?;
        assert_eq!(read_source(&edn)?.format, Format::Edn);
        let bad = folder.path().join("bad.json");
        fs::write(&bad, [0xff, 0xfe])?;
        assert_eq!(
            read_source(&bad).unwrap_err().to_string(),
            "The Roam export is not valid UTF-8 text. Export again as JSON or EDN and select the extracted file."
        );
        let large = folder.path().join("large.json");
        File::create(&large)?.set_len(MAX_SOURCE_BYTES + 1)?;
        assert_eq!(
            read_source(&large).unwrap_err().to_string(),
            "This Roam export exceeds 100 MB. Split the graph before importing."
        );
        Ok(())
    }

    #[test]
    fn exports_never_replace_a_file_and_stay_outside_notebooks() -> Result<()> {
        let folder = tempfile::tempdir()?;
        let notebook = folder.path().join("notebook");
        fs::create_dir(&notebook)?;
        let destination = folder.path().join("out.json");
        let report = write_export(
            &destination,
            std::slice::from_ref(&notebook),
            "[]\n",
            vec!["Done.".into()],
        )?;
        assert_eq!(report, ["Done."]);
        assert_eq!(fs::read_to_string(&destination)?, "[]\n");
        assert_eq!(
            fs::read_to_string(folder.path().join("out.json.report.txt"))?,
            "Done.\n"
        );
        let error = write_export(&destination, &[], "{}", Vec::new()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "The export file already exists. Choose a new filename to keep previous exports."
        );
        assert_eq!(fs::read_to_string(&destination)?, "[]\n");
        let error = write_export(
            &notebook.join("export.json"),
            std::slice::from_ref(&notebook),
            "[]",
            Vec::new(),
        )
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Choose an export filename outside live notebooks and Quire's app-data folder."
        );
        fs::write(folder.path().join("taken.json.report.txt"), "")?;
        let report = write_export(
            &folder.path().join("taken.json"),
            &[],
            "[]",
            vec!["A".into()],
        )?;
        assert_eq!(
            report.len(),
            2,
            "a report file that cannot be written is noted"
        );
        Ok(())
    }

    #[test]
    fn metadata_is_absent_or_read_whole() -> Result<()> {
        let root = tempfile::tempdir()?;
        assert!(read_metadata(root.path())?.is_none());
        fs::create_dir(root.path().join(METADATA))?;
        fs::write(root.path().join(".quire/roam.json"), "invalid")?;
        assert_eq!(
            read_metadata(root.path()).unwrap_err().to_string(),
            "The Roam metadata file could not be read. Restore .quire/roam.json from a backup before exporting."
        );
        fs::write(
            root.path().join(".quire/roam.json"),
            r#"{"version":1,"pages":{}}"#,
        )?;
        assert_eq!(
            read_metadata(root.path())?.map(|value| js_json::stringify(&value)),
            Some(r#"{"version":1,"pages":{}}"#.to_owned())
        );
        Ok(())
    }
}
