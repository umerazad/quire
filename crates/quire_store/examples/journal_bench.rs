//! Times journaled multi-note writes in a folder notebook on this machine's disk.
//!
//! `cargo run --release -p quire_store --example journal_bench -- [notes] [folder]`
//! imports `notes` new pages (default 1000) and then edits all of them in one change, in a fresh
//! notebook under `folder` (default: the system temporary folder), and prints the cost of the
//! sync primitives each write uses.

use anyhow::{Context as _, Result, ensure};
use quire_store::{
    database::open_writable,
    folder_notebook::FolderNotebook,
    markdown_import::SourcePage,
    notebook::{CheckedSaves, Edit},
    sync::format::page_id_for_path,
};
use std::{
    fs::File,
    io::Write as _,
    path::PathBuf,
    time::{Duration, Instant},
};

fn timed<T>(label: &str, count: usize, work: impl FnOnce() -> Result<T>) -> Result<T> {
    let started = Instant::now();
    let value = work()?;
    let elapsed = started.elapsed();
    println!(
        "{label}: {count} in {:.2} s, {:.2} ms each",
        elapsed.as_secs_f64(),
        elapsed.as_secs_f64() * 1000.0 / count.max(1) as f64
    );
    Ok(value)
}

fn average(count: u32, mut work: impl FnMut(u32) -> Result<()>) -> Result<Duration> {
    let started = Instant::now();
    (0..count).try_for_each(&mut work)?;
    Ok(started.elapsed() / count)
}

fn primitives(folder: &std::path::Path) -> Result<()> {
    let samples = 20;
    let file_sync = average(samples, |index| {
        let mut file = File::create(folder.join(format!("probe-{index}")))?;
        file.write_all(b"probe")?;
        Ok(file.sync_all()?)
    })?;
    let directory_sync = average(samples, |_| Ok(File::open(folder)?.sync_all()?))?;
    #[cfg(target_vendor = "apple")]
    {
        use std::os::fd::AsRawFd as _;
        let plain = average(samples, |index| {
            let mut file = File::create(folder.join(format!("plain-{index}")))?;
            file.write_all(b"probe")?;
            // SAFETY: `fsync` only reads the descriptor, which `file` keeps open for the call.
            ensure!(
                unsafe { libc::fsync(file.as_raw_fd()) } == 0,
                "fsync failed: {}. Check the scratch folder's disk and retry.",
                std::io::Error::last_os_error()
            );
            Ok(())
        })?;
        println!("primitives: file fsync without F_FULLFSYNC {plain:?}");
    }
    let connection = open_writable(&folder.join("probe.sqlite"))?;
    connection.exec("CREATE TABLE probe (value INTEGER)")?()?;
    let commit = average(samples, |index| {
        connection.exec_bound("INSERT INTO probe VALUES (?)")?(i64::from(index))
    })?;
    println!(
        "primitives: file sync_all {file_sync:?}, folder sync_all {directory_sync:?}, SQLite autocommit {commit:?}"
    );
    Ok(())
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let count: usize = args
        .next()
        .map(|count| count.parse())
        .transpose()
        .context("The note count must be a whole number, such as 1000.")?
        .unwrap_or(1000);
    let parent = args.next().map_or_else(std::env::temp_dir, PathBuf::from);
    let scratch = tempfile::tempdir_in(&parent).with_context(|| {
        format!(
            "Cannot create a scratch folder in {}. Pass a writable folder as the second argument.",
            parent.display()
        )
    })?;
    let root = scratch.path().join("notebook");
    let state = scratch.path().join("state");
    std::fs::create_dir_all(&root)?;
    primitives(scratch.path())?;

    let notebook = FolderNotebook::open(&root, &state)?;
    let pages: Vec<SourcePage> = (0..count)
        .map(|index| SourcePage {
            path: format!("Imported/Folder {}/Note {index}.md", index % 10),
            text: format!("- Note {index}\n  - [[Note {}]]\n", (index + 1) % count),
        })
        .collect();
    let imported = timed("import", count, || notebook.import_pages(&pages, 1))?;
    ensure!(
        imported.saved.len() == count,
        "The import saved {} of {count} notes. Check the scratch folder's free space and retry.",
        imported.saved.len()
    );

    let edits: Vec<Edit> = pages
        .iter()
        .map(|page| {
            let id = page_id_for_path(&page.path);
            Ok(Edit {
                base: notebook.heads(&id)?,
                page: id,
                path: page.path.clone(),
                text: format!("{}  - edited\n", page.text),
                deleted: false,
                created: 2,
                recovery: None,
            })
        })
        .collect::<Result<_>>()?;
    let saved = timed("edit existing", count, || {
        notebook.save_many_if_heads(&edits)
    })?;
    ensure!(
        matches!(saved, CheckedSaves::Saved(ref acks) if acks.len() == count),
        "The bulk edit did not save every note. Rerun the benchmark in a fresh folder."
    );
    Ok(())
}
