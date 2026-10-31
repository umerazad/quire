use crate::{
    notebook_backend::NotebookBackend,
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
    folder_notebook::{DiskDocument, FolderChange, FolderWorker, Trash},
    notebook::PageCheckout,
    notebook_lock::{self, NotebookLock},
    profiles::Notebook,
    sync::json::Json,
    worker::{QueueError, QueueLimits, StorageWorker},
};
use std::{
    cell::Cell,
    collections::HashMap,
    path::{Path, PathBuf},
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

type Ready = Shared<BoxFuture<'static, Result<(), Arc<str>>>>;

/// Own one session per opened notebook and pass its registered pages to every view.
pub struct NotebookSession {
    client: NotebookBackend,
    ready: Ready,
    pages: HashMap<String, Entity<OpenPage>>,
    editing: Rc<Cell<bool>>,
    /// The notebook folder and Quire's state folder for it, when this is a folder notebook.
    folder: Option<(PathBuf, PathBuf)>,
    /// Where notebook-relative media paths resolve.
    media: Option<quire_media::files::Files>,
    /// Keeps every other session, in this process or another, out of this notebook's recovery.
    _lock: Option<NotebookLock>,
    /// Where the notebook lives, which names its profile.
    location: Option<Notebook>,
}

impl NotebookSession {
    #[cfg(test)]
    pub fn open(path: PathBuf, notebook: String, writer: String) -> Result<Self, String> {
        Self::open_synced(path, notebook, writer, None)
    }

    /// Opens a Quire notebook database whose saves publish to the synced `folder`.
    pub fn open_synced(
        path: PathBuf,
        notebook: String,
        writer: String,
        folder: Option<PathBuf>,
    ) -> Result<Self, String> {
        let lock = path
            .parent()
            .filter(|folder| folder.is_dir())
            .map(|_| NotebookLock::acquire(&notebook_lock::store_lock(&path)))
            .transpose()
            .map_err(|error| format!("{error:#}"))?;
        let media = quire_media::files::Files::for_store_database(&path);
        let location = Notebook::Store {
            database: path.clone(),
            notebook: notebook.clone(),
            writer: writer.clone(),
            folder,
        };
        let worker = StorageWorker::spawn(path, notebook, writer, QueueLimits::default())
            .map_err(|error| format!("{error:#}"))?;
        let ready = worker
            .ready
            .map(|result| result.map_err(|error| Arc::from(format!("{error:#}"))))
            .boxed()
            .shared();
        Ok(Self {
            media: Some(media),
            _lock: lock,
            location: Some(location),
            ..Self::from_worker(worker.client, ready)
        })
    }

    /// Opens the folder notebook at `root`, keeping its recovery, history, and index in `state`.
    /// Deleted notes go to `trash`; without one, deleting is refused.
    pub fn open_folder(
        root: PathBuf,
        state: PathBuf,
        trash: Option<Trash>,
    ) -> Result<Self, String> {
        let lock = std::fs::create_dir_all(&state)
            .map_err(anyhow::Error::from)
            .and_then(|()| NotebookLock::acquire(&notebook_lock::folder_lock(&state)))
            .map_err(|error| format!("{error:#}"))?;
        let worker =
            FolderWorker::spawn_folder(root.clone(), state.clone(), trash, QueueLimits::default())
                .map_err(|error| format!("{error:#}"))?;
        let ready = worker
            .ready
            .map(|result| result.map_err(|error| Arc::from(format!("{error:#}"))))
            .boxed()
            .shared();
        Ok(Self {
            media: Some(quire_media::files::Files::Folder { root: root.clone() }),
            location: Some(Notebook::Folder { root: root.clone() }),
            folder: Some((root, state)),
            _lock: Some(lock),
            ..Self::from_worker(worker.client, ready)
        })
    }

    fn from_worker(client: impl Into<NotebookBackend>, ready: Ready) -> Self {
        Self {
            client: client.into(),
            ready,
            pages: HashMap::new(),
            editing: Rc::new(Cell::new(false)),
            folder: None,
            media: None,
            _lock: None,
            location: None,
        }
    }

    /// Where this notebook lives, for profiles and reopening.
    pub fn location(&self) -> Option<Notebook> {
        self.location.clone()
    }

    /// Where this notebook keeps attachments that pages reference by relative path.
    pub fn media_files(&self) -> Option<quire_media::files::Files> {
        self.media.clone()
    }

    /// The notebook folder and its state folder, for a folder notebook.
    pub fn folder(&self) -> Option<(&Path, &Path)> {
        self.folder
            .as_ref()
            .map(|(root, state)| (root.as_path(), state.as_path()))
    }

    /// Rereads folder notebook files that changed outside Quire (`None`: every file this session
    /// read) and hands each open page its new contents. Clean pages reload; pages with unsaved
    /// edits enter a conflict. Resolves once every open page has been told.
    pub fn folder_changed(
        &self,
        paths: Option<Vec<String>>,
        cx: &mut App,
    ) -> Task<Result<(), String>> {
        let Some(folder) = self.client.folder() else {
            return Task::ready(Ok(()));
        };
        let request = match paths {
            Some(paths) => folder.refresh(paths),
            None => folder.rescan(),
        };
        let pages = self.pages.clone();
        cx.spawn(async move |cx| {
            let changes = crate::query_index::ask(request).await?;
            cx.update(|cx| {
                for change in changes {
                    let Some(persistence) = pages
                        .get(change.page())
                        .and_then(|page| page.read(cx).persistence().cloned())
                    else {
                        continue;
                    };
                    let disk = match change {
                        FolderChange::Changed { document, .. } => Some(document),
                        FolderChange::Removed { .. } => None,
                    };
                    persistence.update(cx, |persistence, cx| persistence.external(disk, cx));
                }
            });
            Ok(())
        })
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

    pub fn storage(&self) -> NotebookBackend {
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
    /// The path of a page in conflict with its file whose typing is not yet recorded, when no
    /// page has a storage error.
    pub fn unrecorded_conflict(&self, cx: &App) -> Option<String> {
        let failing: Vec<_> = self
            .controllers(cx)
            .into_iter()
            .filter(|page| page.read(cx).has_error() && !page.read(cx).is_secured(cx))
            .collect();
        if failing
            .iter()
            .any(|page| page.read(cx).conflict().is_none())
        {
            return None;
        }
        failing.first().map(|page| page.read(cx).path().to_owned())
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
    client: NotebookBackend,
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
            let folder = client.folder().is_some();
            let recovered_folder = folder
                .then(|| FolderRecovery::new(&id, &checkout))
                .flatten();
            let base = match checkout.draft.as_ref().map(|draft| draft.base.as_slice()) {
                Some([base]) if !folder && checkout.heads != [base.as_str()] => client
                    .get(base.clone())
                    .map_err(|error| (error.to_string(), error == QueueError::Busy))?
                    .await
                    .map_err(|error| (format!("{error:#}"), true))?,
                _ => None,
            };
            let page = PreparedPage::new(id, checkout, base).map_err(|error| (error, true))?;
            Ok(match recovered_folder {
                Some(recovery) => recovery.apply(page),
                None => page,
            })
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
                            if let Some(disk) = page.conflict.clone() {
                                persistence
                                    .update(cx, |page, cx| page.recovered_conflict(disk, cx));
                            }
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
    /// A folder notebook draft typed into another version than the file holds now: the file, or
    /// None when it is gone. The page opens in conflict.
    conflict: Option<Option<DiskDocument>>,
}

/// How a folder notebook page with a recovery draft opens. A draft made from the file's current
/// version, or a new note's draft while the file still does not exist, is saved at once. A draft
/// made from another version opens in conflict with the file, as in the reference app.
struct FolderRecovery {
    opened: OpenedPage,
    conflict: Option<Option<DiskDocument>>,
}

impl FolderRecovery {
    fn new(id: &str, checkout: &PageCheckout) -> Option<Self> {
        let draft = checkout.draft.as_ref()?;
        let current = draft.base == checkout.heads;
        if current && !checkout.heads.is_empty() {
            return None;
        }
        let disk = checkout.selected.as_ref().and_then(|record| {
            let field = |name| record.root().field(name).as_str().map(str::to_owned);
            Some(DiskDocument::new(field("path")?, field("text")?))
        });
        Some(Self {
            opened: OpenedPage {
                page: id.to_owned(),
                path: draft.path.clone(),
                base: draft.base.clone(),
                draft: Some(draft.stamp.clone()),
                base_text: None,
                settle: current,
            },
            conflict: (!current).then_some(disk),
        })
    }

    fn apply(self, mut page: PreparedPage) -> PreparedPage {
        page.status = if self.conflict.is_some() {
            "Recovered unsaved work. Compare it with the disk version, or save a separate copy."
                .into()
        } else {
            "Recovered an unsaved new note; saving it now.".into()
        };
        page.opened = Some(self.opened);
        page.conflict = self.conflict;
        page
    }
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
            conflict: None,
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
