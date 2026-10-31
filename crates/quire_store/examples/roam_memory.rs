//! Measures a Roam import's peak memory on a generated graph, with no user data.
//!
//! `cargo run -p quire_store --example roam_memory -- generate <file.json|file.edn> <megabytes>`
//! writes a synthetic JSON export or EDN database. `... -- import <file> <parent>` imports it as a
//! folder notebook inside `<parent>` the way the app does: JSON in two streamed passes a page at a
//! time, EDN rebuilt whole and then converted a page at a time. `... -- import-whole <file>
//! <parent>` imports it the earlier way, holding the whole graph, every document, and the
//! metadata text before writing. References are answered as plain text in both. Each prints the
//! elapsed time and the process's peak resident size.

use anyhow::{Context as _, Result, bail};
use quire_core::{
    exchange::{
        edn::{import_roam_edn, roam_pages_from_edn},
        js_json,
        roam::{RoamImport, RoamMarkdown, TextBlock, import_roam_json, page_reference_texts},
    },
    markdown::Reference,
};
use quire_store::roam_files::{self, Format, ImportStaging, RoamSource};
use serde_json::Value;
use std::{
    collections::BTreeSet,
    fs::File,
    io::{BufWriter, Write as _},
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
    time::Instant,
};

struct Plain;

impl RoamMarkdown for Plain {
    fn references(&self, _: &str) -> Vec<Reference> {
        Vec::new()
    }

    fn blocks(&self, _: &str) -> Vec<TextBlock> {
        Vec::new()
    }
}

const CHILD: &str = "with **bold** text";
const PROSE: &str = "some ordinary prose to give the block a realistic length";

fn generate_json(out: &mut impl std::io::Write, target: usize) -> Result<usize> {
    let (mut written, mut page) = (2, 0usize);
    out.write_all(b"[")?;
    while written < target {
        let blocks: Vec<String> = (0..40)
            .map(|block| {
                let children: Vec<String> = (0..3)
                    .map(|child| {
                        format!(
                            r#"{{"uid":"p{page}b{block}c{child}","string":"Child {child} of block {block} links [[Page {}]] and ((p{}b0)) {CHILD}","create-time":1700000000000,"edit-time":1700000000000,":create/user":{{":user/uid":"u1"}}}}"#,
                            page + 1,
                            page.saturating_sub(1)
                        )
                    })
                    .collect();
                format!(
                    r#"{{"uid":"p{page}b{block}","string":"Block {block} on page {page}: {PROSE}","children":[{}]}}"#,
                    children.join(",")
                )
            })
            .collect();
        let text = format!(
            r#"{}{{"title":"Page {page}","uid":"p{page}","children":[{}]}}"#,
            if page == 0 { "" } else { "," },
            blocks.join(",")
        );
        out.write_all(text.as_bytes())?;
        written += text.len();
        page += 1;
    }
    out.write_all(b"]")?;
    Ok(page)
}

fn generate_edn(out: &mut impl std::io::Write, target: usize) -> Result<usize> {
    let header = "#datascript/DB {\n  :schema {:block/children {:db/cardinality :db.cardinality/many} :block/refs {:db/cardinality :db.cardinality/many}}\n  :datoms [";
    out.write_all(header.as_bytes())?;
    let (mut written, mut page, mut entity) = (header.len() + 3, 0usize, 1usize);
    while written < target {
        let page_entity = entity;
        entity += 1;
        let mut datoms = format!(
            "[{page_entity} :node/title \"Page {page}\" 100] [{page_entity} :block/uid \"p{page}\" 100]\n"
        );
        for block in 0..40 {
            let block_entity = entity;
            entity += 1;
            datoms.push_str(&format!(
                "[{page_entity} :block/children {block_entity} 100] [{block_entity} :block/uid \"p{page}b{block}\" 100] [{block_entity} :block/string \"Block {block} on page {page}: {PROSE}\" 100] [{block_entity} :block/order {block} 100]\n"
            ));
            for child in 0..3 {
                let child_entity = entity;
                entity += 1;
                datoms.push_str(&format!(
                    "[{block_entity} :block/children {child_entity} 100] [{child_entity} :block/uid \"p{page}b{block}c{child}\" 100] [{child_entity} :block/string \"Child {child} of block {block} links [[Page {}]] and ((p{}b0)) {CHILD}\" 100] [{child_entity} :block/order {child} 100] [{child_entity} :create/time 1700000000000 100]\n",
                    page + 1,
                    page.saturating_sub(1)
                ));
            }
        }
        out.write_all(datoms.as_bytes())?;
        written += datoms.len();
        page += 1;
    }
    out.write_all(b"]}\n")?;
    Ok(page)
}

fn generate(path: &Path, megabytes: usize) -> Result<()> {
    let mut out = BufWriter::new(File::create(path)?);
    let target = megabytes * 1024 * 1024;
    let pages = if path.extension().is_some_and(|extension| extension == "edn") {
        generate_edn(&mut out, target)?
    } else {
        generate_json(&mut out, target)?
    };
    out.flush()?;
    println!(
        "wrote {pages} pages, {} bytes",
        std::fs::metadata(path)?.len()
    );
    Ok(())
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn stopped(error: js_json::Streamed) -> anyhow::Error {
    anyhow::anyhow!("The export did not import: {error:?}")
}

/// The app's import: two streamed passes for JSON, pages converted one at a time for EDN.
fn streamed(source: &RoamSource, parent: &Path) -> Result<(PathBuf, Vec<String>, usize)> {
    let mut import = RoamImport::new();
    let mut texts = BTreeSet::new();
    let mut scan = |page: &Value| {
        import.scan(page, &mut new_id)?;
        let mut found = Vec::new();
        page_reference_texts(page, &mut found);
        texts.extend(found);
        Ok::<_, String>(())
    };
    let edn = match source.format {
        Format::Json => {
            js_json::each_element(&source.text, |page| scan(&page)).map_err(stopped)?;
            None
        }
        Format::Edn => {
            let graph = roam_pages_from_edn(&source.text).map_err(anyhow::Error::msg)?;
            for page in graph.pages.as_array().into_iter().flatten() {
                scan(page).map_err(anyhow::Error::msg)?;
            }
            Some(graph)
        }
    };
    let cancel = AtomicBool::new(false);
    let mut staging = ImportStaging::create(parent, "Roam import memory")?;
    let convert = |page: Value| {
        let (document, page) = import.convert(page, &Plain)?;
        staging
            .add(&document, &page, &cancel)
            .map_err(|error| format!("{error:#}"))
    };
    let lines = match edn {
        None => {
            js_json::each_element(&source.text, convert).map_err(stopped)?;
            Vec::new()
        }
        Some(mut graph) => {
            let lines = graph.report();
            let Value::Array(pages) = graph.pages.take() else {
                bail!("The EDN database rebuilt no pages.");
            };
            pages
                .into_iter()
                .try_for_each(convert)
                .map_err(anyhow::Error::msg)?;
            lines
        }
    };
    let mut report = import.finish();
    report.splice(1..1, lines);
    let (root, report) = staging.publish(source, report, &cancel)?;
    Ok((root, report, texts.len()))
}

/// The earlier import: the whole graph, every document, and the metadata text in memory first.
fn whole(source: &RoamSource, parent: &Path) -> Result<(PathBuf, Vec<String>, usize)> {
    let (imported, texts) = match source.format {
        Format::Json => {
            let pages = js_json::parse(&source.text).map_err(anyhow::Error::msg)?;
            let texts = quire_core::exchange::roam::reference_texts(&pages).len();
            (import_roam_json(pages, &Plain, &mut new_id), texts)
        }
        Format::Edn => {
            let graph = roam_pages_from_edn(&source.text).map_err(anyhow::Error::msg)?;
            let texts = quire_core::exchange::roam::reference_texts(&graph.pages).len();
            (import_roam_edn(graph, &Plain, &mut new_id), texts)
        }
    };
    let imported = imported.map_err(anyhow::Error::msg)?;
    let metadata = js_json::stringify(&imported.metadata);
    let cancel = AtomicBool::new(false);
    let mut staging = ImportStaging::create(parent, "Roam import memory")?;
    let pages = &imported.metadata["pages"];
    for document in &imported.documents {
        staging.add(document, &pages[&document.path], &cancel)?;
    }
    let (root, report) = staging.publish(source, imported.report, &cancel)?;
    assert_eq!(
        std::fs::read_to_string(root.join(".quire/roam.json"))?,
        metadata
    );
    Ok((root, report, texts))
}

fn import(source: &Path, parent: &Path, streaming: bool) -> Result<()> {
    let started = Instant::now();
    let source = roam_files::read_source(source)?;
    let (root, report, texts) = if streaming {
        streamed(&source, parent)?
    } else {
        whole(&source, parent)?
    };
    println!(
        "{} ({texts} reference texts) into {} in {:?}; peak resident {} MB",
        report[0],
        root.display(),
        started.elapsed(),
        peak_resident() / (1024 * 1024)
    );
    Ok(())
}

#[cfg(unix)]
fn peak_resident() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: getrusage writes a complete rusage into the pointer it is given.
    let usage = unsafe {
        libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr());
        usage.assume_init()
    };
    let max = usage.ru_maxrss as u64;
    if cfg!(target_os = "macos") {
        max
    } else {
        max * 1024
    }
}

#[cfg(not(unix))]
fn peak_resident() -> u64 {
    0
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["generate", path, megabytes] => generate(
            Path::new(path),
            megabytes
                .parse()
                .context("Give the export size in whole megabytes.")?,
        ),
        ["import", source, parent] => import(Path::new(source), Path::new(parent), true),
        ["import-whole", source, parent] => import(Path::new(source), Path::new(parent), false),
        _ => bail!(
            "Usage: roam_memory generate <file.json|file.edn> <megabytes> | import <file> <parent> | import-whole <file> <parent>"
        ),
    }
}
