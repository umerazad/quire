use crate::{
    page_analysis::PageAnalysis,
    page_persistence::{OpenedPage, PagePersistence},
    stored_source::{PreparedSource, StoredSource},
};
use futures::{
    FutureExt,
    future::{BoxFuture, Shared},
};
use gpui::{App, AppContext, Context, Entity, SharedString, Task};
use quire_store::{
    notebook::PageCheckout,
    sync::json::Json,
    worker::{QueueError, QueueLimits, StorageClient, StorageWorker},
};
use std::{
    cell::Cell,
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

type Ready = Shared<BoxFuture<'static, Result<(), Arc<str>>>>;

/// Own one session per opened notebook and pass its registered pages to every view.
pub struct NotebookSession {
    client: StorageClient,
    ready: Ready,
    pages: HashMap<String, Entity<OpenPage>>,
    editing: Rc<Cell<bool>>,
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
            editing: Rc::new(Cell::new(false)),
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
                        editing: self.editing.clone(),
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

    pub fn storage(&self) -> StorageClient {
        self.client.clone()
    }

    pub fn open_pages(&self) -> Vec<Entity<OpenPage>> {
        self.pages.values().cloned().collect()
    }

    /// Forgets a deleted page, so opening it again reads storage afresh.
    pub fn forget(&mut self, page: &str) -> Option<Entity<OpenPage>> {
        self.pages.remove(page)
    }
    fn controllers(&self, cx: &App) -> Vec<Entity<PagePersistence>> {
        self.pages
            .values()
            .filter_map(|page| page.read(cx).persistence().cloned())
            .collect()
    }
    pub fn enable_editing(&self, cx: &mut App) {
        self.editing.set(true);
        for page in self.controllers(cx) {
            page.update(cx, |page, cx| page.enable(cx));
        }
    }
    pub fn is_secured(&self, cx: &App) -> bool {
        self.controllers(cx)
            .iter()
            .all(|page| page.read(cx).is_secured(cx))
    }
    pub fn has_error(&self, cx: &App) -> bool {
        self.controllers(cx)
            .iter()
            .any(|page| page.read(cx).has_error() && !page.read(cx).is_secured(cx))
    }
    pub fn recover(&self, cx: &mut App) {
        for page in self.controllers(cx) {
            page.update(cx, |page, cx| page.recover(cx));
        }
    }
}

pub struct LoadedPage {
    pub analysis: Entity<PageAnalysis>,
    pub source: Arc<StoredSource>,
    pub persistence: Option<Entity<PagePersistence>>,
    pub heads: Vec<String>,
    pub path: Option<String>,
    pub recovered: bool,
    pub deleted: bool,
    pub loaded_revision_bodies: usize,
    pub load_time: Duration,
}

impl LoadedPage {
    /// The stored-source adapter for the buffer's current lineage, following merged versions.
    pub fn current_source(&self, cx: &App) -> Arc<StoredSource> {
        self.persistence
            .as_ref()
            .map_or_else(|| self.source.clone(), |page| page.read(cx).source())
    }
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
    editing: Rc<Cell<bool>>,
    state: LoadState,
    status: SharedString,
    task: Option<Task<()>>,
}

impl OpenPage {
    pub fn id(&self) -> &str {
        &self.id
    }
    pub fn is_loading(&self) -> bool {
        matches!(self.state, LoadState::Loading)
    }
    pub fn can_retry(&self) -> bool {
        matches!(self.state, LoadState::Failed { retryable: true })
    }
    pub fn status(&self, cx: &App) -> SharedString {
        if let Some(page) = self.persistence()
            && (page.read(cx).enabled() || page.read(cx).is_retired())
        {
            return page.read(cx).status(cx);
        }
        self.status.clone()
    }
    /// Where the page lives now: its saves follow renames made since it opened.
    pub fn path(&self, cx: &App) -> Option<String> {
        match self.persistence() {
            Some(page) => Some(page.read(cx).path().to_owned()),
            None => self.loaded()?.path.clone(),
        }
    }
    pub fn persistence(&self) -> Option<&Entity<PagePersistence>> {
        self.loaded().and_then(|page| page.persistence.as_ref())
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
            let reply = client.checkout(id.clone(), None).map_err(|error| {
                let retryable = error == QueueError::Busy;
                (error.to_string(), retryable)
            })?;
            let checkout = reply.await.map_err(|error| (format!("{error:#}"), true))?;
            let base = match checkout.draft.as_ref().map(|draft| draft.base.as_slice()) {
                Some([base]) if checkout.heads != [base.as_str()] => client
                    .get(base.clone())
                    .map_err(|error| (error.to_string(), error == QueueError::Busy))?
                    .await
                    .map_err(|error| (format!("{error:#}"), true))?,
                _ => None,
            };
            PreparedPage::new(id, checkout, base).map_err(|error| (error, true))
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(page) => {
                        let (source, stored_source) = page.text.install(cx);
                        source.update(cx, |source, cx| {
                            source.set_capability(language::Capability::ReadOnly, cx)
                        });
                        let stored_source = Arc::new(stored_source);
                        let persistence = page.opened.map(|opened| {
                            let persistence = cx.new(|cx| {
                                PagePersistence::new(
                                    source.clone(),
                                    stored_source.clone(),
                                    this.client.clone(),
                                    opened,
                                    cx,
                                )
                            });
                            cx.observe(&persistence, |_, _, cx| cx.notify()).detach();
                            if this.editing.get() {
                                persistence.update(cx, |page, cx| page.enable(cx));
                            }
                            persistence
                        });
                        let analysis = cx.new(|cx| PageAnalysis::new(source, cx));
                        this.status = page.status.into();
                        this.state = LoadState::Ready(LoadedPage {
                            analysis,
                            source: stored_source,
                            persistence,
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
    text: PreparedSource,
    path: Option<String>,
    heads: Vec<String>,
    recovered: bool,
    deleted: bool,
    loaded_revision_bodies: usize,
    status: String,
    opened: Option<OpenedPage>,
}

impl PreparedPage {
    /// `base` is the draft's own base revision when it differs from the page's current head.
    fn new(id: String, checkout: PageCheckout, base: Option<Json>) -> Result<Self, String> {
        let field =
            |record: &Json, name: &str| record.root().field(name).as_str().map(str::to_owned);
        let loaded_revision_bodies =
            usize::from(checkout.selected.is_some()) + usize::from(base.is_some());
        let deleted = checkout
            .selected
            .as_ref()
            .is_some_and(|record| record.root().field("deleted").as_bool() == Some(true));
        let live = !deleted && checkout.heads.len() == 1;
        let head_path = checkout
            .selected
            .as_ref()
            .and_then(|record| field(record, "path"));
        let head_text = checkout
            .selected
            .as_ref()
            .and_then(|record| field(record, "text"));
        let recovered = checkout.draft.is_some();
        let stamp = checkout.draft.as_ref().map(|draft| draft.stamp.clone());
        // A draft settles, saving from its own base so the store merges it, only when that base is
        // known and the page still lives at the draft's path. Every other draft stays for review.
        let (eligible, settle, base_ids, base_text) = match &checkout.draft {
            None => (live, false, checkout.heads.clone(), head_text.clone()),
            Some(draft) if draft.base == checkout.heads => (
                live,
                live && head_path.as_deref() == Some(draft.path.as_str()),
                draft.base.clone(),
                head_text.clone(),
            ),
            Some(draft) => {
                let known = base.as_ref().filter(|record| {
                    record.root().field("page").as_str() == Some(id.as_str())
                        && record.root().field("deleted").as_bool() == Some(false)
                });
                let settle =
                    live && known.is_some() && head_path.as_deref() == Some(draft.path.as_str());
                (
                    settle,
                    settle,
                    draft.base.clone(),
                    known.and_then(|record| field(record, "text")),
                )
            }
        };
        let (text, path) = if let Some(draft) = checkout.draft {
            (draft.text, Some(draft.path))
        } else if checkout.selected.is_some() {
            (
                head_text.ok_or_else(unrepresentable)?,
                Some(head_path.ok_or_else(unrepresentable)?),
            )
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
            if !eligible {
                status.push_str(" kept for review");
            }
        }
        let opened = eligible
            .then(|| {
                path.clone().map(|path| OpenedPage {
                    page: id,
                    path,
                    base: base_ids,
                    draft: stamp,
                    base_text: base_text.map(Arc::from),
                    settle,
                })
            })
            .flatten();
        Ok(Self {
            text: PreparedSource::new(text),
            path,
            heads: checkout.heads,
            recovered,
            deleted,
            loaded_revision_bodies,
            status,
            opened,
        })
    }
}

fn unrepresentable() -> String {
    "This page contains UTF-16 code units the native editor cannot represent. Its original data is preserved. Open it with a compatible Quire version before editing.".into()
}

#[cfg(test)]
pub(crate) mod tests;
