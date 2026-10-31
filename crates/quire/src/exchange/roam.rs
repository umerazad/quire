//! Roam import and export: reading a Roam JSON or EDN export into a new folder notebook, and
//! writing a notebook as Roam JSON. Conversion comes from `quire_core::exchange::roam` and
//! `edn`, with references and blocks parsed by the editor's Markdown grammar off the UI thread;
//! files go through `quire_store::roam_files`.

use super::Exchange;
use gpui::{AppContext as _, AsyncApp, Entity};
use quire_core::{
    exchange::{
        edn::{import_roam_edn, roam_pages_from_edn},
        js_json,
        roam::{
            Document, RoamMarkdown, TextBlock, export_file_text, export_roam_json,
            import_folder_name, import_roam_json, reference_texts,
        },
    },
    markdown::Reference,
};
use quire_store::roam_files::{self, Format, IMPORT_CANCELED, ImportedGraph};
use serde_json::Value;
use std::{
    cell::RefCell,
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Texts parsed per trip to the UI thread, which only starts each parse.
const PARSE_BATCH: usize = 256;

pub const EXPORT_CANCELED: &str = "The Roam export was canceled. Nothing was written.";

fn describe(error: anyhow::Error) -> String {
    format!("{error:#}")
}

fn new_id() -> String {
    format!("b-{}", uuid::Uuid::new_v4().simple())
}

/// Whether the reference parser could find a reference in `text`.
fn may_reference(text: &str) -> bool {
    text.contains("((") || text.contains("[[") || text.contains("{{pdf:")
}

/// The editor's parses of the texts a conversion reads, recording the texts it has not parsed.
#[derive(Default)]
struct Parsed {
    references: HashMap<String, Vec<Reference>>,
    blocks: HashMap<String, Vec<TextBlock>>,
    missing: RefCell<BTreeSet<String>>,
}

impl RoamMarkdown for Parsed {
    fn references(&self, text: &str) -> Vec<Reference> {
        if !may_reference(text) {
            return Vec::new();
        }
        self.references.get(text).cloned().unwrap_or_else(|| {
            self.missing.borrow_mut().insert(text.to_owned());
            Vec::new()
        })
    }

    fn blocks(&self, text: &str) -> Vec<TextBlock> {
        self.blocks.get(text).cloned().unwrap_or_default()
    }
}

/// The positions in `text` with `\r\n` line breaks that its `\n` form drops, as offsets into that
/// form, so offsets the editor reports map back.
fn dropped_returns(text: &str) -> Vec<usize> {
    text.match_indices("\r\n")
        .enumerate()
        .map(|(dropped, (index, _))| index - dropped)
        .collect()
}

fn restore(offset: usize, dropped: &[usize]) -> usize {
    offset + dropped.partition_point(|&position| position < offset)
}

/// The reference `blockText`: a list block's continuation lines lose the indentation of its
/// content column.
fn reference_block_text(text: &str, block: &crate::markdown_blocks::Block, own: String) -> String {
    if block.kind != "list" {
        return own;
    }
    let line_from = text[..block.from].rfind('\n').map_or(0, |index| index + 1);
    let indent = block.content_from - line_from;
    own.split('\n')
        .enumerate()
        .map(|(index, line)| {
            if index == 0 {
                return line;
            }
            let space = line.len() - line.trim_start_matches([' ', '\t']).len();
            &line[indent.min(space)..]
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parses each text with the editor's Markdown grammar, a batch at a time, and returns its
/// references (and, with `blocks`, its blocks). Stops when `cancel` is set.
async fn parse(
    texts: Vec<String>,
    blocks: bool,
    cancel: &AtomicBool,
    exchange: &Entity<Exchange>,
    label: impl Fn(usize) -> String,
    cx: &mut AsyncApp,
) -> Result<Vec<(String, Vec<Reference>, Vec<TextBlock>)>, String> {
    const UNAVAILABLE: &str = "The Markdown grammar is unavailable. Restart Quire, then retry.";
    let mut parsed = Vec::with_capacity(texts.len());
    for batch in texts.chunks(PARSE_BATCH) {
        if cancel.load(Ordering::Relaxed) {
            return Ok(parsed);
        }
        let snapshots = cx.update(|cx| {
            batch
                .iter()
                .map(|text| {
                    let normalized = text.replace("\r\n", "\n");
                    (
                        text.clone(),
                        crate::markdown::detached_snapshot(&normalized, cx),
                    )
                })
                .collect::<Vec<_>>()
        });
        let results = cx
            .background_spawn(async move {
                let mut results = Vec::with_capacity(snapshots.len());
                for (text, snapshot) in snapshots {
                    let snapshot = snapshot.await;
                    let dropped = dropped_returns(&text);
                    let references = crate::page_analysis::references(&snapshot)
                        .ok_or(UNAVAILABLE)?
                        .into_iter()
                        .map(|mut reference| {
                            reference.range = restore(reference.range.start, &dropped)
                                ..restore(reference.range.end, &dropped);
                            reference
                        })
                        .collect();
                    let found = if blocks {
                        let normalized = snapshot.text();
                        crate::markdown_blocks::blocks(&snapshot)
                            .ok_or(UNAVAILABLE)?
                            .into_iter()
                            .map(|block| {
                                let own = crate::markdown_blocks::block_text(&snapshot, &block);
                                TextBlock {
                                    text: reference_block_text(&normalized, &block, own),
                                    key: block.key,
                                    id: block.id,
                                    parent: block.parent,
                                    from: block.from,
                                    subtree_to: block.subtree_to,
                                }
                            })
                            .collect()
                    } else {
                        Vec::new()
                    };
                    results.push((text, references, found));
                }
                Ok::<_, &str>(results)
            })
            .await?;
        parsed.extend(results);
        let label = label(parsed.len());
        progress(exchange, label, true, cx);
    }
    Ok(parsed)
}

fn progress(exchange: &Entity<Exchange>, label: String, cancellable: bool, cx: &mut AsyncApp) {
    exchange.update(cx, |exchange, cx| exchange.progress(label, cancellable, cx));
}

/// A Roam export read and rebuilt as Roam pages.
enum Graph {
    Json(Value),
    Edn(quire_core::exchange::edn::EdnGraph),
}

impl Graph {
    fn pages(&self) -> &Value {
        match self {
            Self::Json(pages) => pages,
            Self::Edn(graph) => &graph.pages,
        }
    }
}

/// Imports the Roam export at `source` as a new folder notebook inside `parent`. Returns the
/// notebook folder and the import report.
pub async fn import(
    source: PathBuf,
    parent: PathBuf,
    cancel: Arc<AtomicBool>,
    exchange: &Entity<Exchange>,
    cx: &mut AsyncApp,
) -> Result<(PathBuf, Vec<String>), String> {
    let canceled = || cancel.load(Ordering::Relaxed);
    let (source, graph, texts) = cx
        .background_spawn(async move {
            let source = roam_files::read_source(&source).map_err(describe)?;
            let graph = match source.format {
                Format::Json => Graph::Json(js_json::parse(&source.text).map_err(|error| {
                    if error == js_json::TOO_DEEP {
                        error
                    } else {
                        "The Roam export is not valid UTF-8 JSON. Extract its JSON or EDN file from the Roam export ZIP, then select that file.".to_owned()
                    }
                })?),
                Format::Edn => Graph::Edn(roam_pages_from_edn(&source.text)?),
            };
            let texts: BTreeSet<String> = reference_texts(graph.pages()).into_iter().collect();
            Ok::<_, String>((source, graph, texts.into_iter().collect::<Vec<_>>()))
        })
        .await?;
    if canceled() {
        return Err(IMPORT_CANCELED.into());
    }
    let total = texts.len();
    progress(
        exchange,
        format!("Importing Roam data: reading references in {total} blocks…"),
        true,
        cx,
    );
    let parsed = parse(
        texts,
        false,
        &cancel,
        exchange,
        |count| format!("Importing Roam data: read references in {count} of {total} blocks…"),
        cx,
    )
    .await?;
    if canceled() {
        return Err(IMPORT_CANCELED.into());
    }
    progress(
        exchange,
        "Importing Roam data: converting pages…".into(),
        true,
        cx,
    );
    let imported = cx
        .background_spawn(async move {
            let markdown = Parsed {
                references: parsed
                    .into_iter()
                    .map(|(text, references, _)| (text, references))
                    .collect(),
                ..Parsed::default()
            };
            let imported = match graph {
                Graph::Json(pages) => import_roam_json(pages, &markdown, &mut new_id),
                Graph::Edn(graph) => import_roam_edn(graph, &markdown, &mut new_id),
            }?;
            let metadata = js_json::stringify(&imported.metadata);
            Ok::<_, String>((imported.documents, imported.report, metadata))
        })
        .await?;
    let (documents, report, metadata) = imported;
    let total = documents.len();
    progress(
        exchange,
        format!("Importing Roam data: writing {total} pages…"),
        true,
        cx,
    );
    let name = import_folder_name(
        super::now() as i64,
        &uuid::Uuid::new_v4().simple().to_string()[..8],
    );
    let (written, mut counted) = futures::channel::mpsc::unbounded();
    let publishing = cx.background_spawn({
        let cancel = cancel.clone();
        async move {
            roam_files::publish_import(
                &parent,
                &name,
                ImportedGraph {
                    documents: &documents,
                    metadata: &metadata,
                    source: &source,
                    report,
                },
                &cancel,
                |count| {
                    written.unbounded_send(count).ok();
                },
            )
            .map_err(describe)
        }
    });
    while let Some(count) = futures::StreamExt::next(&mut counted).await {
        progress(
            exchange,
            format!("Importing Roam data: wrote {count} of {total} pages…"),
            true,
            cx,
        );
    }
    publishing.await
}

/// What a Roam export reads from the notebook.
pub struct Notebook {
    pub documents: Vec<Document>,
    /// The folder notebook whose `.quire/roam.json` holds imported Roam metadata.
    pub root: Option<PathBuf>,
}

/// Writes `notebook` as Roam JSON to `destination`, refusing a destination inside `forbidden`.
/// Returns the report.
pub async fn export(
    notebook: Notebook,
    destination: PathBuf,
    forbidden: Vec<PathBuf>,
    cancel: Arc<AtomicBool>,
    exchange: &Entity<Exchange>,
    cx: &mut AsyncApp,
) -> Result<Vec<String>, String> {
    let canceled = || cancel.load(Ordering::Relaxed);
    let Notebook { documents, root } = notebook;
    let metadata = cx
        .background_spawn(async move {
            root.as_deref()
                .map(roam_files::read_metadata)
                .transpose()
                .map(Option::flatten)
                .map_err(describe)
        })
        .await?;
    let texts: Vec<String> = documents
        .iter()
        .map(|document| document.text.replace("\r\n", "\n"))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let total = documents.len();
    progress(
        exchange,
        format!("Exporting Roam JSON: reading blocks in {total} pages…"),
        true,
        cx,
    );
    let mut markdown = Parsed::default();
    for (text, references, blocks) in parse(
        texts,
        true,
        &cancel,
        exchange,
        |count| format!("Exporting Roam JSON: read blocks in {count} of {total} pages…"),
        cx,
    )
    .await?
    {
        markdown.references.insert(text.clone(), references);
        markdown.blocks.insert(text, blocks);
    }
    if canceled() {
        return Err(EXPORT_CANCELED.into());
    }
    let documents = Arc::new(documents);
    let metadata = Arc::new(metadata);
    // A first pass finds the block texts whose references the export rewrites; the second
    // converts with every one parsed.
    let (mut markdown, missing) = cx
        .background_spawn({
            let (documents, metadata) = (documents.clone(), metadata.clone());
            async move {
                export_roam_json(
                    &documents,
                    metadata.as_ref().as_ref(),
                    &markdown,
                    &mut new_id,
                )?;
                let missing = std::mem::take(&mut *markdown.missing.borrow_mut());
                Ok::<_, String>((markdown, missing))
            }
        })
        .await?;
    let missing: Vec<String> = missing.into_iter().collect();
    let blocks = missing.len();
    progress(
        exchange,
        format!("Exporting Roam JSON: reading references in {blocks} blocks…"),
        true,
        cx,
    );
    for (text, references, _) in parse(
        missing,
        false,
        &cancel,
        exchange,
        |count| format!("Exporting Roam JSON: read references in {count} of {blocks} blocks…"),
        cx,
    )
    .await?
    {
        markdown.references.insert(text, references);
    }
    if canceled() {
        return Err(EXPORT_CANCELED.into());
    }
    progress(
        exchange,
        format!("Exporting Roam JSON: writing {total} pages…"),
        false,
        cx,
    );
    cx.background_spawn(async move {
        let exported = export_roam_json(
            &documents,
            metadata.as_ref().as_ref(),
            &markdown,
            &mut new_id,
        )?;
        let text = export_file_text(exported.pages);
        roam_files::write_export(&destination, &forbidden, &text, exported.report).map_err(describe)
    })
    .await
}

/// The export's documents in the reference order: by path as the folder notebook's
/// `localeCompare` sorts them, or by UTF-16 code units for a Quire notebook.
pub fn sorted(mut documents: Vec<Document>, folder: bool) -> Vec<Document> {
    if folder {
        documents.sort_by(|a, b| quire_core::templates::collate(&a.path, &b.path));
    } else {
        documents.sort_by(|a, b| a.path.encode_utf16().cmp(b.path.encode_utf16()));
    }
    documents
}

/// Whether `root` holds an imported Roam graph's metadata.
pub fn is_roam_notebook(root: &Path) -> bool {
    root.join(".quire")
        .join(roam_files::METADATA_FILE)
        .is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_map_back_across_dropped_returns() {
        let text = "a\r\nb ((x))\r\n((y))";
        let dropped = dropped_returns(text);
        let normalized = text.replace("\r\n", "\n");
        let start = normalized.find("((y))").unwrap();
        assert_eq!(&text[restore(start, &dropped)..], "((y))");
        let start = normalized.find("((x))").unwrap();
        assert_eq!(
            &text[restore(start, &dropped)..restore(start + 5, &dropped)],
            "((x))"
        );
    }
}
