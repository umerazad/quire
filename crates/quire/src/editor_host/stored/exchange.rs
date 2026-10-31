//! The window's Markdown import and export commands: choosing folders and files, running the
//! work off the UI thread with progress and Cancel, and refreshing the notebook afterwards.

use super::StoredWindow;
use crate::{
    exchange::{
        BUSY, CancelExchange, Exchange, ExportNotebookMarkdown, ExportPageMarkdown, ExportRoamJson,
        ImportMarkdownFolder, ImportRoamData, flatten_page, now, roam,
    },
    notebook_backend::NotebookBackend,
    page_writes,
    query_index::ask,
};
use futures::{StreamExt as _, channel::mpsc};
use gpui::{AppContext as _, AsyncApp, Entity, PathPromptOptions, WeakEntity, Window};
use quire_core::exchange::{
    file_links::{attachment_name, rewrite_file_links, root_attachment_target},
    import::import_summary,
    notebook_export::{ExportReport, export_target, replace_question},
    page_export::{export_name, localize_attachments},
};
use quire_media::files::Files;
use quire_store::{
    markdown_import::{CANCELED, copy_files, read_source, remove_files},
    notebook::SourceFiles,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

/// Quire imports at most this many bytes of pages in one checked write.
const MAX_IMPORT_BYTES: usize = 48 * 1024 * 1024;

pub(super) const EXPORT_CANCELED: &str = "The export was canceled. Nothing was written.";

fn folder_prompt(prompt: &str) -> PathPromptOptions {
    PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(prompt.to_owned().into()),
    }
}

fn describe(error: anyhow::Error) -> String {
    format!("{error:#}")
}

/// The notebook file a `quire-vault:` link names: an ordinary file inside a folder notebook, or
/// a Quire notebook's stored attachment, as the reference resolves files under its root.
pub(super) fn notebook_file(files: &Files, relative: &str) -> Option<String> {
    let file = match files {
        Files::Folder { root } => {
            let base = fs::canonicalize(root).ok()?;
            let file = fs::canonicalize(base.join(relative)).ok()?;
            (file.starts_with(&base) && file.is_file()).then_some(file)?
        }
        Files::Store { .. } => files.locate(relative).ok()?,
    };
    Some(file.to_string_lossy().into_owned())
}

impl StoredWindow {
    pub(super) fn exchange_busy(&mut self, cx: &mut gpui::Context<Self>) -> bool {
        let busy = self.exchange.read(cx).is_running();
        if busy {
            self.nav.error = Some(BUSY.into());
            cx.notify();
        }
        busy
    }

    fn notebook_root(&self) -> Option<PathBuf> {
        self.session
            .borrow()
            .folder()
            .map(|(root, _)| root.to_path_buf())
    }

    /// Asks for a folder of Markdown pages and copies it into this notebook.
    pub(super) fn import_markdown_folder(&mut self, cx: &mut gpui::Context<Self>) {
        if self.exchange_busy(cx) {
            return;
        }
        let storage = self.session.borrow().storage();
        let root = self.notebook_root();
        let exchange = self.exchange.clone();
        let chosen = cx.prompt_for_paths(folder_prompt("Import"));
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(source))) = chosen
                .await
                .map(|chosen| chosen.map(|paths| paths.and_then(|paths| paths.into_iter().next())))
            else {
                return;
            };
            let result = import(source, root, storage, &exchange, cx).await;
            exchange.update(cx, |exchange, cx| {
                exchange.finish(result.map(|message| (message, None)), cx)
            });
            this.update(cx, |this, cx| this.notebook_changed(cx)).ok();
        })
        .detach();
    }

    /// Pages were added or files written: the index and page list catch up.
    fn notebook_changed(&mut self, cx: &mut gpui::Context<Self>) {
        if self.notebook_root().is_some()
            && let Some(index) = self.index_client(cx)
        {
            index
                .invalidate()
                .map(|reply| cx.background_spawn(reply).detach())
                .ok();
        }
        self.refresh_catalogue(cx);
    }

    /// Saves the shown page as standalone Markdown in a file the user chooses.
    pub(super) fn export_shown_page(&mut self, cx: &mut gpui::Context<Self>) {
        if self.exchange_busy(cx) {
            return;
        }
        let Some(page) = self.shown_page(cx) else {
            return;
        };
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Export needs the notebook index, which is still starting. Export again in a moment."
                    .into(),
            );
            cx.notify();
            return;
        };
        let storage = self.session.borrow().storage();
        let open = self.session.borrow().open_pages();
        let files = self.session.borrow().media_files();
        let exchange = self.exchange.clone();
        exchange.update(cx, |exchange, cx| {
            exchange.start("Exporting the page…", None, cx)
        });
        cx.spawn(async move |_, cx| {
            let result = export_page(page, open, storage, index, files, cx).await;
            exchange.update(cx, |exchange, cx| match result {
                Ok(None) => *exchange = Exchange::Idle,
                Ok(Some(done)) => exchange.finish(Ok(done), cx),
                Err(error) => exchange.finish(Err(error), cx),
            });
        })
        .detach();
    }

    /// Asks for a folder and writes every page and file of this notebook into it.
    pub(super) fn export_notebook(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.exchange_busy(cx) {
            return;
        }
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Export needs the notebook index, which is still starting. Export again in a moment."
                    .into(),
            );
            cx.notify();
            return;
        };
        let storage = self.session.borrow().storage();
        let files = self.session.borrow().media_files();
        let exchange = self.exchange.clone();
        let chosen = cx.prompt_for_paths(folder_prompt("Export"));
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(folder))) = chosen
                .await
                .map(|chosen| chosen.map(|paths| paths.and_then(|paths| paths.into_iter().next())))
            else {
                return;
            };
            let result = export_all(folder, storage, index, files, &exchange, this, cx).await;
            exchange.update(cx, |exchange, cx| {
                exchange.finish(result.map(|message| (message, None)), cx)
            });
        })
        .detach();
    }

    /// Asks for a Roam JSON or EDN export and a parent folder, imports the graph there as a new
    /// folder notebook, and opens it.
    pub(super) fn import_roam_data(&mut self, cx: &mut gpui::Context<Self>) {
        if self.exchange_busy(cx) {
            return;
        }
        let exchange = self.exchange.clone();
        let chosen = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Import".into()),
        });
        cx.spawn(async move |_, cx| {
            let Ok(Ok(Some(source))) = chosen
                .await
                .map(|chosen| chosen.map(|paths| paths.and_then(|paths| paths.into_iter().next())))
            else {
                return;
            };
            let chosen = cx.update(|cx| cx.prompt_for_paths(folder_prompt("Choose")));
            let Ok(Ok(Some(parent))) = chosen
                .await
                .map(|chosen| chosen.map(|paths| paths.and_then(|paths| paths.into_iter().next())))
            else {
                return;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            exchange.update(cx, |exchange, cx| {
                exchange.start(
                    "Importing Roam data: reading the export…",
                    Some(cancel.clone()),
                    cx,
                )
            });
            let result = roam::import(source, parent, cancel, &exchange, cx)
                .await
                .map(|(root, report)| {
                    let opened = cx.update(|cx| {
                        super::folder::open_folder_window(root.clone(), report[1..].to_vec(), cx)
                    });
                    let message = match opened {
                        Ok(()) => {
                            format!("{} Details are available in notebook notices.", report[0])
                        }
                        Err(error) => format!("{} {error}", report[0]),
                    };
                    (message, Some(root))
                });
            exchange.update(cx, |exchange, cx| exchange.finish(result, cx));
        })
        .detach();
    }

    /// Asks for a file and writes every page of this notebook into it as Roam JSON.
    pub(super) fn export_roam_json(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.exchange_busy(cx) {
            return;
        }
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Export needs the notebook index, which is still starting. Export again in a moment."
                    .into(),
            );
            cx.notify();
            return;
        };
        let storage = self.session.borrow().storage();
        let open = self.session.borrow().open_pages();
        let folder = self
            .session
            .borrow()
            .folder()
            .map(|(root, state)| (root.to_path_buf(), state.to_path_buf()));
        let exchange = self.exchange.clone();
        let chosen = cx.prompt_for_new_path(
            util::paths::home_dir(),
            Some(&quire_core::exchange::roam::export_file_name(now() as i64)),
        );
        cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(destination))) = chosen.await else {
                return;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            exchange.update(cx, |exchange, cx| {
                exchange.start(
                    "Exporting Roam JSON: reading pages…",
                    Some(cancel.clone()),
                    cx,
                )
            });
            let notebook = ExportedNotebook {
                storage,
                index,
                open,
                folder,
            };
            let result = export_roam(notebook, destination.clone(), cancel, &exchange, cx).await;
            let result = result.map(|report| {
                this.update(cx, |this, cx| {
                    this.folder
                        .notices
                        .extend(report[1..].iter().cloned().map(Into::into));
                    cx.notify();
                })
                .ok();
                (
                    format!(
                        "Saved Roam JSON to {}. {}",
                        destination.display(),
                        report[0]
                    ),
                    Some(destination),
                )
            });
            exchange.update(cx, |exchange, cx| exchange.finish(result, cx));
        })
        .detach();
    }

    pub(super) fn cancel_exchange(&mut self, cx: &mut gpui::Context<Self>) {
        self.exchange.update(cx, |exchange, cx| exchange.cancel(cx));
    }

    pub(super) fn register_exchange(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        use gpui::InteractiveElement as _;
        div.on_action(
            cx.listener(|this, _: &ImportMarkdownFolder, _, cx| this.import_markdown_folder(cx)),
        )
        .on_action(cx.listener(|this, _: &ExportPageMarkdown, _, cx| this.export_shown_page(cx)))
        .on_action(cx.listener(|this, _: &ExportNotebookMarkdown, window, cx| {
            this.export_notebook(window, cx)
        }))
        .on_action(cx.listener(|this, _: &ImportRoamData, _, cx| this.import_roam_data(cx)))
        .on_action(
            cx.listener(|this, _: &ExportRoamJson, window, cx| this.export_roam_json(window, cx)),
        )
        .on_action(cx.listener(|this, _: &CancelExchange, _, cx| this.cancel_exchange(cx)))
    }

    /// Export as Markdown for the shown page, and the notebook-wide import and export.
    pub(super) fn render_exchange_buttons(&self, cx: &mut gpui::Context<Self>) -> gpui::Div {
        use gpui::{ParentElement as _, Styled as _};
        use ui::{Button, Clickable as _, prelude::FluentBuilder as _};
        gpui::div()
            .flex()
            .gap_2()
            .when(self.shown_page(cx).is_some(), |div| {
                div.child(
                    Button::new("export-page-markdown", "Export as Markdown…")
                        .on_click(cx.listener(|this, _, _, cx| this.export_shown_page(cx))),
                )
                .children(self.render_document_export_buttons(cx))
            })
            .child(
                Button::new("import-markdown-folder", "Import a Markdown folder…")
                    .on_click(cx.listener(|this, _, _, cx| this.import_markdown_folder(cx))),
            )
            .child(
                Button::new("export-notebook-markdown", "Export notebook as Markdown…")
                    .on_click(cx.listener(|this, _, window, cx| this.export_notebook(window, cx))),
            )
            .child(
                Button::new("import-roam-data", "Import Roam data…")
                    .on_click(cx.listener(|this, _, _, cx| this.import_roam_data(cx))),
            )
            .child(
                Button::new("export-roam-json", "Export Roam JSON…")
                    .on_click(cx.listener(|this, _, window, cx| this.export_roam_json(window, cx))),
            )
    }
}

async fn import(
    source: PathBuf,
    root: Option<PathBuf>,
    storage: NotebookBackend,
    exchange: &Entity<Exchange>,
    cx: &mut AsyncApp,
) -> Result<String, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    exchange.update(cx, |exchange, cx| {
        exchange.start(
            "Importing pages: reading the folder…",
            Some(cancel.clone()),
            cx,
        )
    });
    let (counts, mut counted) = mpsc::unbounded();
    let reading = cx.background_spawn({
        let (source, root, cancel) = (source.clone(), root.clone(), cancel.clone());
        async move {
            read_source(&source, root.as_deref(), &cancel, |count| {
                counts.unbounded_send(count).ok();
            })
        }
    });
    while let Some(count) = counted.next().await {
        exchange.update(cx, |exchange, cx| {
            exchange.progress(format!("Importing pages: read {count} pages…"), true, cx)
        });
    }
    let folder = reading.await.map_err(describe)?;
    if cancel.load(Ordering::Relaxed) {
        return Err(CANCELED.into());
    }
    let bytes: usize = folder
        .pages
        .iter()
        .map(|page| 256 + page.path.len() + page.text.len())
        .sum();
    if bytes > MAX_IMPORT_BYTES {
        return Err(format!(
            "The folder holds {} MB of pages; Quire imports at most {} MB at a time. Split the folder and import each part; nothing was imported.",
            bytes.div_ceil(1024 * 1024),
            MAX_IMPORT_BYTES / (1024 * 1024)
        ));
    }
    exchange.update(cx, |exchange, cx| {
        exchange.progress(
            format!("Importing pages: saving {} pages…", folder.pages.len()),
            false,
            cx,
        )
    });
    let files = folder.files.len();
    match storage {
        NotebookBackend::Folder(client) => {
            let root =
                root.ok_or("This folder notebook has no folder. Reopen it and import again.")?;
            let copying = folder.files.clone();
            let copied = cx
                .background_spawn(async move { copy_files(&source, &root, &copying) })
                .await
                .map_err(describe)?;
            match ask(client.import_pages(folder.pages, now())).await {
                Ok(imported) => Ok(import_summary(
                    imported.saved.len(),
                    files,
                    imported.skipped.len(),
                )),
                Err(error) => {
                    cx.background_spawn(async move { remove_files(&copied) })
                        .await;
                    Err(error)
                }
            }
        }
        NotebookBackend::Store(client) => {
            let source = SourceFiles {
                root: source,
                files: folder.files,
            };
            let imported = ask(client.import_folder(folder.pages, source, now())).await?;
            let attached = imported.attached.len();
            let summary = import_summary(
                imported.pages.saved.len(),
                attached,
                imported.pages.skipped.len(),
            );
            Ok(match files - attached {
                0 => summary,
                unlinked => format!(
                    "{summary} {unlinked} other files no imported page links to stayed in the source folder: a Quire notebook keeps only the files its pages link to."
                ),
            })
        }
    }
}

async fn export_page(
    page: String,
    open: Vec<Entity<crate::notebook_session::OpenPage>>,
    storage: NotebookBackend,
    index: quire_store::index::IndexClient,
    files: Option<Files>,
    cx: &mut AsyncApp,
) -> Result<Option<(String, Option<PathBuf>)>, String> {
    let (title, text) = flatten_page(page, open, storage, index, cx).await?;
    let directory = util::paths::home_dir().clone();
    let chosen = cx
        .update(|cx| cx.prompt_for_new_path(&directory, Some(&export_name(&title, "md"))))
        .await
        .map_err(|_| "The save dialog closed unexpectedly. Export again.".to_owned())?
        .map_err(describe)?;
    let Some(target) = chosen else {
        return Ok(None);
    };
    let written = target.clone();
    cx.background_spawn(async move {
        let text = localize_attachments(&text, |relative| {
            files
                .as_ref()
                .and_then(|files| notebook_file(files, relative))
        });
        fs::write(&written, text).map_err(|error| {
            format!(
                "Cannot write {}: {error}. Choose a folder you can write to and export again.",
                written.display()
            )
        })
    })
    .await?;
    Ok(Some((
        format!("Exported {title} as Markdown."),
        Some(target),
    )))
}

/// The notebook a Roam export reads.
struct ExportedNotebook {
    storage: NotebookBackend,
    index: quire_store::index::IndexClient,
    open: Vec<gpui::Entity<crate::notebook_session::OpenPage>>,
    /// The folder notebook and Quire's state folder for it.
    folder: Option<(PathBuf, PathBuf)>,
}

/// Reads every page, refusing unsaved recovery as the reference does, and writes the notebook as
/// Roam JSON to `destination`.
async fn export_roam(
    notebook: ExportedNotebook,
    destination: PathBuf,
    cancel: Arc<AtomicBool>,
    exchange: &Entity<Exchange>,
    cx: &mut gpui::AsyncWindowContext,
) -> Result<Vec<String>, String> {
    let ExportedNotebook {
        storage,
        index,
        open,
        folder,
    } = notebook;
    let pages = ask(index.pages()).await?;
    let total = pages.len();
    let mut documents = Vec::with_capacity(total);
    for (count, listed) in pages.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(roam::EXPORT_CANCELED.into());
        }
        let page = listed.page.page.to_lossy_string();
        let Some(source) = page_writes::read(page.clone(), &open, &storage, cx).await? else {
            continue;
        };
        let draft = match &storage {
            NotebookBackend::Store(client) => ask(client.draft(page)).await?,
            NotebookBackend::Folder(client) => ask(client.draft(page)).await?,
        };
        if draft.is_some_and(|draft| draft.text != source.stored) {
            return Err(
                "Unsaved recovery data exists. Resolve recovered pages before exporting the graph."
                    .into(),
            );
        }
        documents.push(quire_core::exchange::roam::Document {
            path: source.path,
            text: source.stored,
        });
        exchange.update(cx, |exchange, cx| {
            exchange.progress(
                format!("Exporting Roam JSON: read {} of {total} pages…", count + 1),
                true,
                cx,
            )
        });
    }
    let documents = roam::sorted(documents, folder.is_some());
    let mut forbidden = vec![paths::data_dir().clone()];
    let root = folder.map(|(root, state)| {
        forbidden.extend([root.clone(), state]);
        root
    });
    roam::export(
        roam::Notebook { documents, root },
        destination,
        forbidden,
        cancel,
        exchange,
        cx,
    )
    .await
}

/// One file a notebook export writes, at its path inside the chosen folder.
enum Exported {
    Page(String),
    File(PathBuf),
}

async fn export_all(
    folder: PathBuf,
    storage: NotebookBackend,
    index: quire_store::index::IndexClient,
    files: Option<Files>,
    exchange: &Entity<Exchange>,
    window: WeakEntity<StoredWindow>,
    cx: &mut gpui::AsyncWindowContext,
) -> Result<String, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    exchange.update(cx, |exchange, cx| {
        exchange.start(
            "Exporting the notebook: listing pages…",
            Some(cancel.clone()),
            cx,
        )
    });
    let pages = ask(index.pages()).await?;
    let mut outputs: Vec<(String, Exported)> = Vec::new();
    let mut report = ExportReport {
        folder: folder.display().to_string(),
        ..ExportReport::default()
    };
    let total = pages.len();
    let open = window
        .update(cx, |this, _| this.session.borrow().open_pages())
        .map_err(|_| EXPORT_CANCELED.to_owned())?;
    for (count, listed) in pages.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(EXPORT_CANCELED.into());
        }
        let page = listed.page.page.to_lossy_string();
        let Some(source) = page_writes::read(page.clone(), &open, &storage, cx).await? else {
            continue;
        };
        if source.competing() {
            report.competing.push(source.path.clone());
        }
        let draft = match &storage {
            NotebookBackend::Store(client) => ask(client.draft(page)).await?,
            NotebookBackend::Folder(client) => ask(client.draft(page)).await?,
        };
        if draft.is_some() {
            report.drafts.push(source.path.clone());
        }
        outputs.push((export_target(&source.path)?, Exported::Page(source.stored)));
        exchange.update(cx, |exchange, cx| {
            exchange.progress(
                format!(
                    "Exporting the notebook: read {} of {total} pages…",
                    count + 1
                ),
                true,
                cx,
            )
        });
    }
    report.pages = outputs.len();
    match (&storage, files) {
        (NotebookBackend::Folder(client), Some(Files::Folder { root })) => {
            for file in ask(client.listing()).await?.others {
                outputs.push((export_target(&file)?, Exported::File(root.join(&file))));
                report.files += 1;
            }
        }
        (_, files) => {
            let (with_attachments, copied, uncopied) = cx
                .background_spawn(async move { gather_attachments(outputs, files.as_ref()) })
                .await;
            outputs = with_attachments;
            report.files = copied;
            report.uncopied = uncopied;
        }
    }
    let targets: Vec<String> = outputs.iter().map(|(target, _)| target.clone()).collect();
    let checked = folder.clone();
    let existing: Vec<String> = cx
        .background_spawn(async move {
            targets
                .into_iter()
                .filter(|target| fs::symlink_metadata(checked.join(target)).is_ok())
                .collect()
        })
        .await;
    if !existing.is_empty() {
        let (question, detail) = replace_question(&report.folder, &existing);
        let answer = window
            .update_in(cx, |_, window, cx| {
                window.prompt(
                    gpui::PromptLevel::Warning,
                    &question,
                    Some(&detail),
                    &["Replace", "Cancel"],
                    cx,
                )
            })
            .map_err(|_| EXPORT_CANCELED.to_owned())?;
        if answer.await != Ok(0) {
            return Err(EXPORT_CANCELED.into());
        }
    }
    exchange.update(cx, |exchange, cx| {
        exchange.progress(
            format!("Exporting the notebook: writing {} files…", outputs.len()),
            true,
            cx,
        )
    });
    let writing = cancel.clone();
    cx.background_spawn(async move { write_export(&folder, outputs, &writing) })
        .await?;
    Ok(report.summary())
}

/// A Quire notebook's attachments for an export: each page link to a stored `attachments/<name>`
/// is pointed at `attachments/<name>` in the export folder's root, which gets a copy of the file.
/// Returns the outputs with those files added, how many were copied, and the linked attachments
/// this computer does not have.
fn gather_attachments(
    outputs: Vec<(String, Exported)>,
    files: Option<&Files>,
) -> (Vec<(String, Exported)>, usize, Vec<String>) {
    let mut copied = BTreeMap::new();
    let mut uncopied = BTreeSet::new();
    let mut gathered: Vec<(String, Exported)> = outputs
        .into_iter()
        .map(|(target, output)| match output {
            Exported::Page(text) => {
                let text = rewrite_file_links(&text, &target, |path| {
                    let name = attachment_name(path)?;
                    match files.map(|files| files.locate(&format!("attachments/{name}"))) {
                        Some(Ok(file)) => {
                            copied.insert(name.to_owned(), file);
                            Some(root_attachment_target(&target, name))
                        }
                        _ => {
                            uncopied.insert(path.to_owned());
                            None
                        }
                    }
                });
                (target, Exported::Page(text))
            }
            file => (target, file),
        })
        .collect();
    let count = copied.len();
    gathered.extend(
        copied
            .into_iter()
            .map(|(name, file)| (format!("attachments/{name}"), Exported::File(file))),
    );
    (gathered, count, uncopied.into_iter().collect())
}

/// Writes every output into a hidden folder inside `folder`, then moves each into place, so a
/// canceled or failed export leaves nothing behind.
fn write_export(
    folder: &Path,
    outputs: Vec<(String, Exported)>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let staging = folder.join(format!(".quire-export-{}", now()));
    fs::create_dir(&staging).map_err(|error| {
        format!(
            "Cannot write into {}: {error}. Choose a folder you can write to and export again.",
            folder.display()
        )
    })?;
    let staged = (|| {
        for (target, output) in &outputs {
            if cancel.load(Ordering::Relaxed) {
                return Err(EXPORT_CANCELED.to_owned());
            }
            let path = staging.join(target);
            let failed = |error: std::io::Error| {
                format!(
                    "Cannot write {target}: {error}. Check free space and write access, then export again; nothing was written."
                )
            };
            fs::create_dir_all(path.parent().unwrap_or(&staging)).map_err(failed)?;
            match output {
                Exported::Page(text) => fs::write(&path, text).map_err(failed)?,
                Exported::File(source) => fs::copy(source, &path).map(drop).map_err(failed)?,
            }
        }
        Ok(())
    })();
    let moved = staged.and_then(|()| {
        outputs.iter().try_for_each(|(target, _)| {
            let path = folder.join(target);
            fs::create_dir_all(path.parent().unwrap_or(folder))
                .and_then(|()| fs::rename(staging.join(target), &path))
                .map_err(|error| {
                    format!("Cannot place {target} in {}: {error}. Check write access; files already placed stay.", folder.display())
                })
        })
    });
    fs::remove_dir_all(&staging).ok();
    moved
}
