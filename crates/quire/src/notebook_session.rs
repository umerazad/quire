use crate::page_analysis::PageAnalysis;
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use gpui::{App, AppContext, Context, Entity, SharedString, Task};
use quire_store::{
    notebook::PageCheckout,
    worker::{QueueError, QueueLimits, StorageClient, StorageWorker},
};
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

type Ready = Shared<BoxFuture<'static, Result<(), Arc<str>>>>;

/// Own one session per opened notebook and pass its registered pages to every view.
pub struct NotebookSession {
    client: StorageClient,
    ready: Ready,
    pages: HashMap<String, Entity<OpenPage>>,
}

impl NotebookSession {
    pub fn open(path: PathBuf, notebook: String, writer: String) -> Result<Self, String> {
        let worker = StorageWorker::spawn(path, notebook, writer, QueueLimits::default())
            .map_err(|error| format!("{error:#}"))?;
        let ready = worker
            .ready
            .map(|result| result.map_err(|error| Arc::from(format!("{error:#}"))))
            .boxed()
            .shared();
        Ok(Self::from_worker(worker.client, ready))
    }

    fn from_worker(client: StorageClient, ready: Ready) -> Self {
        Self {
            client,
            ready,
            pages: HashMap::new(),
        }
    }

    pub fn open_page(&mut self, page: String, cx: &mut App) -> Entity<OpenPage> {
        self.pages
            .entry(page.clone())
            .or_insert_with(|| {
                cx.new(|cx| {
                    let mut page = OpenPage {
                        id: page,
                        client: self.client.clone(),
                        ready: self.ready.clone(),
                        state: LoadState::Loading,
                        status: "Loading page…".into(),
                        task: None,
                    };
                    page.load(cx);
                    page
                })
            })
            .clone()
    }
}

pub struct LoadedPage {
    pub analysis: Entity<PageAnalysis>,
    pub heads: Vec<String>,
    pub path: Option<String>,
    pub recovered: bool,
    pub deleted: bool,
    pub loaded_revision_bodies: usize,
    pub load_time: Duration,
}

enum LoadState {
    Loading,
    Failed { retryable: bool },
    Ready(LoadedPage),
}

pub struct OpenPage {
    id: String,
    client: StorageClient,
    ready: Ready,
    state: LoadState,
    status: SharedString,
    task: Option<Task<()>>,
}

impl OpenPage {
    pub fn is_loading(&self) -> bool {
        matches!(self.state, LoadState::Loading)
    }
    pub fn can_retry(&self) -> bool {
        matches!(self.state, LoadState::Failed { retryable: true })
    }
    pub fn status(&self) -> SharedString {
        self.status.clone()
    }
    pub fn loaded(&self) -> Option<&LoadedPage> {
        match &self.state {
            LoadState::Ready(page) => Some(page),
            _ => None,
        }
    }
    pub fn retry(&mut self, cx: &mut Context<Self>) {
        if self.can_retry() {
            self.load(cx);
        }
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        self.state = LoadState::Loading;
        self.status = "Loading page…".into();
        let started = Instant::now();
        let id = self.id.clone();
        let client = self.client.clone();
        let ready = self.ready.clone();
        let work = cx.background_spawn(async move {
            ready.await.map_err(|error| (error.to_string(), false))?;
            if id.is_empty() {
                return Err((
                    "Cannot open a page without an identity. Choose a page and retry.".into(),
                    false,
                ));
            }
            let reply = client.checkout(id, None).map_err(|error| {
                let retryable = error == QueueError::Busy;
                (error.to_string(), retryable)
            })?;
            let checkout = reply.await.map_err(|error| (format!("{error:#}"), true))?;
            PreparedPage::new(checkout).map_err(|error| (error, true))
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(page) => {
                        let source = crate::markdown::buffer(page.text, cx);
                        // Persistent editing is enabled only after recovery/save/close guards exist.
                        source.update(cx, |source, cx| {
                            source.set_capability(language::Capability::ReadOnly, cx)
                        });
                        let analysis = cx.new(|cx| PageAnalysis::new(source, cx));
                        this.status = page.status.into();
                        this.state = LoadState::Ready(LoadedPage {
                            analysis,
                            heads: page.heads,
                            path: page.path,
                            recovered: page.recovered,
                            deleted: page.deleted,
                            loaded_revision_bodies: page.loaded_revision_bodies,
                            load_time: started.elapsed(),
                        });
                    }
                    Err((error, retryable)) => {
                        this.status = error.into();
                        this.state = LoadState::Failed { retryable };
                    }
                }
                this.task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

struct PreparedPage {
    text: String,
    path: Option<String>,
    heads: Vec<String>,
    recovered: bool,
    deleted: bool,
    loaded_revision_bodies: usize,
    status: String,
}

impl PreparedPage {
    fn new(checkout: PageCheckout) -> Result<Self, String> {
        let loaded_revision_bodies = usize::from(checkout.selected.is_some());
        let deleted = checkout
            .selected
            .as_ref()
            .is_some_and(|record| record.root().field("deleted").as_bool() == Some(true));
        let recovered = checkout.draft.is_some();
        let (text, path) = if let Some(draft) = checkout.draft {
            (draft.text, Some(draft.path))
        } else if let Some(record) = checkout.selected {
            let text = record
                .root()
                .field("text")
                .as_str()
                .ok_or_else(unrepresentable)?;
            let path = record
                .root()
                .field("path")
                .as_str()
                .ok_or_else(unrepresentable)?;
            (text.to_owned(), Some(path.to_owned()))
        } else {
            (String::new(), None)
        };
        let mut status = "Read-only stored page preview".to_owned();
        if checkout.heads.is_empty() {
            status.push_str(" · page not saved");
        }
        if checkout.heads.len() > 1 {
            status.push_str(&format!(" · {} conflicting versions", checkout.heads.len()));
        }
        if deleted {
            status.push_str(" · selected version is deleted");
        }
        if recovered {
            status.push_str(" · recovered unsaved changes");
        }
        Ok(Self {
            text,
            path,
            heads: checkout.heads,
            recovered,
            deleted,
            loaded_revision_bodies,
            status,
        })
    }
}

fn unrepresentable() -> String {
    "This page contains UTF-16 code units the native editor cannot represent. Its original data is preserved. Open it with a compatible Quire version before editing.".into()
}

#[cfg(test)]
pub(crate) mod tests;
