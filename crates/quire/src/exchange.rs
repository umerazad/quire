//! Markdown import and export: the commands, the status line that shows a running import or
//! export with its Cancel button, and the notebook reads a page export flattens over. Planning
//! comes from `quire_core::exchange`; writes go through `quire_store::markdown_import`.

use crate::{
    notebook_backend::NotebookBackend, notebook_session::OpenPage, page_writes, query_index::ask,
};
use gpui::{
    AsyncApp, Context, Entity, IntoElement, ParentElement, Render, SharedString, Styled, Window,
};
use quire_core::{
    exchange::page_export::{ExportNotebook, FoundBlock, export_markdown},
    markdown::{Reference, page_title},
};
use quire_store::index::IndexClient;
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};
use ui::{Button, Clickable, Label, prelude::FluentBuilder as _};

gpui::actions!(
    quire_exchange,
    [
        /// Copies a folder of Markdown pages and their files into this notebook.
        ImportMarkdownFolder,
        /// Saves the shown page as standalone Markdown, with links read as text and embeds
        /// written out.
        ExportPageMarkdown,
        /// Writes every page and file of this notebook into a folder as Markdown.
        ExportNotebookMarkdown,
        /// Stops the running import or export before it writes anything.
        CancelExchange,
    ]
);

pub const BUSY: &str =
    "An import or export is already running. Wait for it to finish, or cancel it.";

/// The running or finished import or export.
#[derive(Default)]
pub enum Exchange {
    #[default]
    Idle,
    Running {
        label: SharedString,
        /// Present while stopping leaves nothing behind.
        cancel: Option<Arc<AtomicBool>>,
    },
    Finished {
        message: SharedString,
        /// A written file to show in Finder or Explorer.
        file: Option<PathBuf>,
    },
    Failed(SharedString),
}

impl Exchange {
    pub fn is_running(&self) -> bool {
        matches!(self, Self::Running { .. })
    }

    /// The text the status line shows.
    pub fn message(&self) -> Option<SharedString> {
        match self {
            Self::Idle => None,
            Self::Running { label, .. } => Some(label.clone()),
            Self::Finished { message, .. } => Some(message.clone()),
            Self::Failed(message) => Some(message.clone()),
        }
    }

    pub fn start(
        &mut self,
        label: impl Into<SharedString>,
        cancel: Option<Arc<AtomicBool>>,
        cx: &mut Context<Self>,
    ) {
        *self = Self::Running {
            label: label.into(),
            cancel,
        };
        cx.notify();
    }

    /// Updates a running label; `cancel` None means the write has begun and can no longer stop.
    pub fn progress(
        &mut self,
        label: impl Into<SharedString>,
        cancellable: bool,
        cx: &mut Context<Self>,
    ) {
        if let Self::Running {
            label: shown,
            cancel,
        } = self
        {
            *shown = label.into();
            if !cancellable {
                *cancel = None;
            }
            cx.notify();
        }
    }

    pub fn finish(
        &mut self,
        result: Result<(String, Option<PathBuf>), String>,
        cx: &mut Context<Self>,
    ) {
        *self = match result {
            Ok((message, file)) => Self::Finished {
                message: message.into(),
                file,
            },
            Err(error) => Self::Failed(error.into()),
        };
        cx.notify();
    }

    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Self::Running {
            cancel: Some(cancel),
            label,
        } = self
        {
            cancel.store(true, Ordering::Relaxed);
            *label = "Canceling…".into();
            cx.notify();
        }
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) {
        if !self.is_running() {
            *self = Self::Idle;
            cx.notify();
        }
    }
}

impl Render for Exchange {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(message) = self.message() else {
            return gpui::div();
        };
        let cancellable = matches!(
            self,
            Self::Running {
                cancel: Some(_),
                ..
            }
        );
        let file = match self {
            Self::Finished { file, .. } => file.clone(),
            _ => None,
        };
        let running = self.is_running();
        gpui::div()
            .p_2()
            .flex()
            .items_center()
            .gap_2()
            .child(Label::new(message))
            .when(cancellable, |div| {
                div.child(
                    Button::new("cancel-exchange", "Cancel")
                        .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                )
            })
            .when_some(file, |div, file| {
                div.child(
                    Button::new("show-exported-file", "Show file")
                        .on_click(move |_, _, cx| cx.reveal_path(&file)),
                )
            })
            .when(!running, |div| {
                div.child(
                    Button::new("dismiss-exchange", "Dismiss")
                        .on_click(cx.listener(|this, _, _, cx| this.dismiss(cx))),
                )
            })
    }
}

/// What a page export has read from the notebook so far, and what it still needs.
#[derive(Default)]
struct Gathered {
    texts: HashMap<String, String>,
    pages: HashMap<String, Option<String>>,
    blocks: HashMap<String, Option<FoundBlock>>,
    parses: HashMap<String, Vec<Reference>>,
    missing: RefCell<Missing>,
}

#[derive(Default)]
struct Missing {
    pages: HashSet<String>,
    blocks: HashSet<String>,
    parses: HashSet<String>,
}

impl Missing {
    fn is_empty(&self) -> bool {
        self.pages.is_empty() && self.blocks.is_empty() && self.parses.is_empty()
    }
}

impl ExportNotebook for Gathered {
    fn text(&self, path: &str) -> String {
        self.texts.get(path).cloned().unwrap_or_default()
    }

    fn title(&self, path: &str) -> String {
        page_title(path, &self.text(path)).to_lossy_string()
    }

    fn find_page(&self, target: &str) -> Option<String> {
        self.pages.get(target).cloned().unwrap_or_else(|| {
            self.missing.borrow_mut().pages.insert(target.to_owned());
            None
        })
    }

    fn resolve(&self, target: &str) -> Option<FoundBlock> {
        self.blocks.get(target).cloned().unwrap_or_else(|| {
            self.missing.borrow_mut().blocks.insert(target.to_owned());
            None
        })
    }

    fn references(&self, text: &str) -> Vec<Reference> {
        self.parses.get(text).cloned().unwrap_or_else(|| {
            self.missing.borrow_mut().parses.insert(text.to_owned());
            Vec::new()
        })
    }
}

/// Reads a page's editor text, from its open buffer when open, else its stored head.
async fn page_text(
    page: String,
    open: &[Entity<OpenPage>],
    storage: &NotebookBackend,
    cx: &mut AsyncApp,
) -> Result<Option<(String, String)>, String> {
    Ok(page_writes::read(page, open, storage, cx)
        .await?
        .map(|source| (source.path, editor_text(&source.stored))))
}

/// The editor's view of stored text: every line break becomes `\n`, as the reference notebook
/// holds it.
fn editor_text(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    }
}

async fn parse(text: String, cx: &mut AsyncApp) -> Result<Vec<Reference>, String> {
    let snapshot = cx
        .update(|cx| crate::markdown::detached_snapshot(&text, cx))
        .await;
    crate::page_analysis::references(&snapshot).ok_or_else(|| {
        "The Markdown grammar is unavailable. Restart Quire, then export again.".to_owned()
    })
}

/// The block `target` names inside the page holding `text` at `path`, parsed natively.
async fn find_block(
    target: &str,
    path: String,
    text: &str,
    cx: &mut AsyncApp,
) -> Option<FoundBlock> {
    let id = target.rsplit_once("#^").map_or(target, |(_, id)| id);
    let snapshot = cx
        .update(|cx| crate::markdown::detached_snapshot(text, cx))
        .await;
    let blocks = crate::markdown_blocks::blocks(&snapshot)?;
    let mut matches = blocks
        .iter()
        .filter(|block| block.id.as_deref() == Some(id));
    let block = matches.next()?;
    matches.next().is_none().then(|| FoundBlock {
        path,
        from: block.from,
        content_from: block.content_from,
        subtree_to: block.subtree_to,
        text: crate::markdown_blocks::block_text(&snapshot, block),
    })
}

/// Rounds of reading before a page export gives up; each round loads what the last one found
/// missing, and embeds nest at most three deep.
const MAX_ROUNDS: usize = 32;

/// The page as standalone Markdown and its title, flattened as the reference export does over
/// the notebook's open buffers and stored pages.
pub async fn flatten_page(
    page: String,
    open: Vec<Entity<OpenPage>>,
    storage: NotebookBackend,
    index: IndexClient,
    cx: &mut AsyncApp,
) -> Result<(String, String), String> {
    let (path, text) = page_text(page, &open, &storage, cx).await?.ok_or_else(|| {
        "This page is no longer in the notebook. Choose another page and export again.".to_owned()
    })?;
    let mut gathered = Gathered::default();
    gathered.texts.insert(path.clone(), text);
    for _ in 0..MAX_ROUNDS {
        let exported = export_markdown(&gathered, &path).map_err(|error| error.to_string())?;
        let missing = std::mem::take(&mut *gathered.missing.borrow_mut());
        if missing.is_empty() {
            return Ok((gathered.title(&path), exported));
        }
        for text in missing.parses {
            let references = parse(text.clone(), cx).await?;
            gathered.parses.insert(text, references);
        }
        for target in missing.pages {
            let found = match ask(index.find_page(target.clone())).await? {
                Some(found) => page_text(found.page.to_lossy_string(), &open, &storage, cx).await?,
                None => None,
            };
            let path = found.map(|(path, text)| {
                gathered.texts.insert(path.clone(), text);
                path
            });
            gathered.pages.insert(target, path);
        }
        for target in missing.blocks {
            let found = match ask(index.resolve_block(target.clone())).await? {
                Some(found) => {
                    match page_text(found.page.to_lossy_string(), &open, &storage, cx).await? {
                        Some((path, text)) => {
                            let block = find_block(&target, path.clone(), &text, cx).await;
                            gathered.texts.insert(path, text);
                            block
                        }
                        None => None,
                    }
                }
                None => None,
            };
            gathered.blocks.insert(target, found);
        }
    }
    Err("The page embeds more than Quire can follow in one export. Remove some embeds, then export again.".into())
}

/// The milliseconds since the Unix epoch, for new revisions.
pub fn now() -> u64 {
    u64::try_from(crate::navigation::now_millis()).unwrap_or_default()
}
