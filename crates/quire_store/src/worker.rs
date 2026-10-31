use crate::notebook::{
    BatchImport, CheckedSave, CheckedSaves, Cursor, Draft, DraftStamp, Edit, MetadataLimits,
    NotebookStore, PageCheckout, PageMetadata, PendingPublication, PublicationAck,
    PublicationState, RescanCursor, RescanLimits, RescanReport, SaveAck,
};
use crate::sync::json::{JsString, Json};
use anyhow::{Context as _, Result, anyhow, ensure};
use futures::channel::oneshot;
use std::{
    fmt,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{
        Arc, Mutex, TryLockError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    task::{Context, Poll},
};

#[derive(Clone, Copy, Debug)]
pub struct QueueLimits {
    pub requests: usize,
    pub bytes: usize,
}
impl Default for QueueLimits {
    fn default() -> Self {
        Self {
            requests: 16,
            bytes: 64 * 1024 * 1024,
        }
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum QueueError {
    Busy,
    Stopped,
}
impl fmt::Display for QueueError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Busy => "Notebook storage queue is busy. Keep the buffer and retry after pending work finishes.",
            Self::Stopped => "Notebook storage stopped. Keep the buffer and reopen the notebook before retrying.",
        })
    }
}
impl std::error::Error for QueueError {}

pub struct Reply<T> {
    receiver: oneshot::Receiver<Result<T>>,
}
impl<T> Future for Reply<T> {
    type Output = Result<T>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.receiver).poll(cx).map(|result| result.unwrap_or_else(|_| Err(anyhow!("Notebook storage stopped before replying. Keep the buffer and reopen the notebook."))))
    }
}
#[derive(Clone, Copy, Debug)]
pub struct WorkerStats {
    pub pending_requests: usize,
    pub pending_bytes: usize,
    pub active: bool,
}
struct Budget {
    limit: usize,
    requests: AtomicUsize,
    bytes: AtomicUsize,
    active: AtomicBool,
}
struct Admission {
    budget: Arc<Budget>,
    bytes: usize,
}
impl Drop for Admission {
    fn drop(&mut self) {
        self.budget.bytes.fetch_sub(self.bytes, Ordering::Relaxed);
        self.budget.requests.fetch_sub(1, Ordering::Relaxed);
    }
}
impl Budget {
    fn reserve(self: &Arc<Self>, bytes: usize) -> std::result::Result<Admission, QueueError> {
        let mut used = self.bytes.load(Ordering::Relaxed);
        loop {
            let next = used.checked_add(bytes).ok_or(QueueError::Busy)?;
            // One oversized request may proceed alone; large atomic edits are not split.
            if used != 0 && next > self.limit {
                return Err(QueueError::Busy);
            }
            match self
                .bytes
                .compare_exchange_weak(used, next, Ordering::Relaxed, Ordering::Relaxed)
            {
                Ok(_) => break,
                Err(current) => used = current,
            }
        }
        self.requests.fetch_add(1, Ordering::Relaxed);
        Ok(Admission {
            budget: self.clone(),
            bytes,
        })
    }
}
type Work<S> = Box<dyn FnOnce(&S) + Send>;
struct Request<S> {
    run: Work<S>,
    admission: Admission,
}
struct Shared<S> {
    sender: Mutex<Option<SyncSender<Request<S>>>>,
    budget: Arc<Budget>,
}
pub struct Client<S> {
    shared: Arc<Shared<S>>,
}
impl<S> Clone for Client<S> {
    fn clone(&self) -> Self {
        Self {
            shared: self.shared.clone(),
        }
    }
}
pub type StorageClient = Client<NotebookStore>;
pub struct Worker<S> {
    pub client: Client<S>,
    pub ready: Reply<()>,
    pub stopped: Reply<()>,
}

pub type StorageWorker = Worker<NotebookStore>;

impl Worker<NotebookStore> {
    /// Starts without performing filesystem work on the calling thread.
    pub fn spawn(
        path: PathBuf,
        notebook: String,
        device: String,
        limits: QueueLimits,
    ) -> Result<Self> {
        Self::spawn_with("quire-storage", limits, move || {
            NotebookStore::open(&path, &notebook, &device)
        })
        .context(
            "Cannot start notebook storage worker. Keep the buffer and retry after freeing process resources.",
        )
    }
}

impl<S: 'static> Worker<S> {
    pub(crate) fn spawn_with(
        name: &str,
        limits: QueueLimits,
        open: impl FnOnce() -> Result<S> + Send + 'static,
    ) -> Result<Self> {
        ensure!(
            limits.requests > 0 && limits.bytes > 0,
            "Storage queue limits must be positive. Correct the limits before starting the worker."
        );
        let (sender, receiver) = mpsc::sync_channel::<Request<S>>(limits.requests);
        let budget = Arc::new(Budget {
            limit: limits.bytes,
            requests: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            active: AtomicBool::new(false),
        });
        let client = Client {
            shared: Arc::new(Shared {
                sender: Mutex::new(Some(sender)),
                budget: budget.clone(),
            }),
        };
        let (ready_send, ready) = oneshot::channel();
        let (stopped_send, stopped) = oneshot::channel();
        let name = name.to_owned();
        let thread = std::thread::Builder::new()
            .name(name.clone())
            .spawn(move || {
                match open() {
                    Ok(store) => {
                        let _ = ready_send.send(Ok(()));
                        for request in receiver {
                            budget.active.store(true, Ordering::Release);
                            (request.run)(&store);
                            budget.active.store(false, Ordering::Release);
                            drop(request.admission);
                        }
                        drop(store);
                    }
                    Err(error) => {
                        let _ = ready_send.send(Err(error));
                        drop(receiver);
                    }
                }
                let _ = stopped_send.send(Ok(()));
            })
            .with_context(|| {
                format!("Cannot start the {name} worker. Retry after freeing process resources.")
            })?;
        // Completion is reported after the connection is dropped, without joining on the UI thread.
        drop(thread);
        Ok(Self {
            client,
            ready: Reply { receiver: ready },
            stopped: Reply { receiver: stopped },
        })
    }
}

fn strings_size(values: &[String]) -> usize {
    values.iter().fold(
        values.len().saturating_mul(std::mem::size_of::<String>()),
        |size, value| size.saturating_add(value.capacity()),
    )
}
fn edits_size(edits: &[Edit]) -> usize {
    edits
        .iter()
        .fold(0usize, |size, edit| size.saturating_add(edit_size(edit)))
}

fn edit_size(edit: &Edit) -> usize {
    256usize
        .saturating_add(edit.page.capacity())
        .saturating_add(edit.path.capacity())
        .saturating_add(edit.text.capacity())
        .saturating_add(strings_size(&edit.base))
        .saturating_add(
            edit.recovery
                .as_ref()
                .map_or(0, |stamp| stamp.session.capacity()),
        )
}
fn gate_error<S>(
    error: TryLockError<std::sync::MutexGuard<'_, Option<SyncSender<Request<S>>>>>,
) -> QueueError {
    match error {
        TryLockError::WouldBlock => QueueError::Busy,
        TryLockError::Poisoned(_) => QueueError::Stopped,
    }
}
impl<S: 'static> Client<S> {
    pub(crate) fn request<T: Send + 'static>(
        &self,
        bytes: usize,
        work: impl FnOnce(&S) -> Result<T> + Send + 'static,
    ) -> std::result::Result<Reply<T>, QueueError> {
        let gate = self.shared.sender.try_lock().map_err(gate_error)?;
        let sender = gate.as_ref().ok_or(QueueError::Stopped)?;
        let admission = self.shared.budget.reserve(bytes.max(1))?;
        let (reply, receiver) = oneshot::channel();
        let request = Request {
            admission,
            run: Box::new(move |store| {
                let _ = reply.send(work(store));
            }),
        };
        sender.try_send(request).map_err(|error| match error {
            TrySendError::Full(_) => QueueError::Busy,
            TrySendError::Disconnected(_) => QueueError::Stopped,
        })?;
        Ok(Reply { receiver })
    }
    /// Stops admission atomically across all clones; accepted requests drain before `stopped` resolves.
    pub fn close(&self) -> std::result::Result<(), QueueError> {
        self.shared.sender.try_lock().map_err(gate_error)?.take();
        Ok(())
    }
    pub fn stats(&self) -> WorkerStats {
        WorkerStats {
            pending_requests: self.shared.budget.requests.load(Ordering::Relaxed),
            pending_bytes: self.shared.budget.bytes.load(Ordering::Relaxed),
            active: self.shared.budget.active.load(Ordering::Acquire),
        }
    }
}

impl Client<NotebookStore> {
    pub fn save(&self, edit: Edit) -> std::result::Result<Reply<SaveAck>, QueueError> {
        self.request(edit_size(&edit), move |store| store.save(&edit))
    }
    pub fn page_metadata(
        &self,
        after: Option<JsString>,
        limits: MetadataLimits,
    ) -> std::result::Result<Reply<Vec<PageMetadata>>, QueueError> {
        self.request(
            128usize.saturating_add(after.as_ref().map_or(0, |page| page.as_bytes().len())),
            move |store| store.page_metadata(after.as_ref(), limits),
        )
    }
    pub fn rebuild_metadata(&self) -> std::result::Result<Reply<usize>, QueueError> {
        self.request(128, NotebookStore::rebuild_metadata)
    }
    pub fn save_if_heads(
        &self,
        edit: Edit,
        expected: Vec<String>,
    ) -> std::result::Result<Reply<CheckedSave>, QueueError> {
        self.request(
            edit_size(&edit).saturating_add(strings_size(&expected)),
            move |store| store.save_if_heads(&edit, &expected),
        )
    }
    pub fn checkout(
        &self,
        page: String,
        selected: Option<String>,
    ) -> std::result::Result<Reply<PageCheckout>, QueueError> {
        self.request(
            128usize
                .saturating_add(page.capacity())
                .saturating_add(selected.as_ref().map_or(0, String::capacity)),
            move |store| store.checkout(&page, selected.as_deref()),
        )
    }
    pub fn save_many(
        &self,
        edits: Vec<Edit>,
    ) -> std::result::Result<Reply<Vec<SaveAck>>, QueueError> {
        self.request(edits_size(&edits), move |store| store.save_many(&edits))
    }
    pub fn save_many_if_heads(
        &self,
        edits: Vec<Edit>,
    ) -> std::result::Result<Reply<CheckedSaves>, QueueError> {
        self.request(edits_size(&edits), move |store| {
            store.save_many_if_heads(&edits)
        })
    }
    pub fn delete_if_heads(
        &self,
        edits: Vec<Edit>,
        folder: Option<String>,
    ) -> std::result::Result<Reply<CheckedSaves>, QueueError> {
        self.request(
            edits_size(&edits).saturating_add(folder.as_ref().map_or(0, String::capacity)),
            move |store| store.delete_if_heads(&edits, folder.as_deref()),
        )
    }
    pub fn put_draft(
        &self,
        draft: Draft,
        expected: Option<DraftStamp>,
    ) -> std::result::Result<Reply<DraftStamp>, QueueError> {
        let bytes = 256usize
            .saturating_add(draft.text.capacity())
            .saturating_add(draft.path.capacity())
            .saturating_add(draft.page.capacity())
            .saturating_add(draft.stamp.session.capacity())
            .saturating_add(strings_size(&draft.base))
            .saturating_add(
                expected
                    .as_ref()
                    .map_or(0, |stamp| stamp.session.capacity()),
            );
        self.request(bytes, move |store| {
            store.put_draft(&draft, expected.as_ref())
        })
    }
    pub fn draft(&self, page: String) -> std::result::Result<Reply<Option<Draft>>, QueueError> {
        self.request(128 + page.capacity(), move |store| store.draft(&page))
    }
    pub fn get(&self, id: String) -> std::result::Result<Reply<Option<Json>>, QueueError> {
        self.request(128 + id.capacity(), move |store| store.get(&id))
    }
    pub fn heads(&self, page: String) -> std::result::Result<Reply<Vec<String>>, QueueError> {
        self.request(128 + page.capacity(), move |store| store.heads(&page))
    }
    pub fn history(
        &self,
        page: String,
    ) -> std::result::Result<Reply<Vec<crate::notebook::SavedVersion>>, QueueError> {
        self.request(128 + page.capacity(), move |store| store.history(&page))
    }
    pub fn version_text(
        &self,
        page: String,
        id: String,
    ) -> std::result::Result<Reply<String>, QueueError> {
        self.request(128 + page.capacity() + id.capacity(), move |store| {
            store.version_text(&page, &id)
        })
    }
    pub fn attach(
        &self,
        data: Vec<u8>,
        original: String,
    ) -> std::result::Result<Reply<String>, QueueError> {
        self.request(
            128usize
                .saturating_add(data.capacity())
                .saturating_add(original.capacity()),
            move |store| store.attach(&data, &original),
        )
    }
    /// Attaches a file by streaming it from `source`; see `NotebookStore::attach_file`.
    pub fn attach_file(
        &self,
        source: PathBuf,
        original: String,
    ) -> std::result::Result<Reply<String>, QueueError> {
        self.request(256usize.saturating_add(original.capacity()), move |store| {
            store.attach_file(&source, &original)
        })
    }
    pub fn cache_attachment(
        &self,
        name: String,
        data: Vec<u8>,
    ) -> std::result::Result<Reply<()>, QueueError> {
        self.request(
            128usize
                .saturating_add(name.capacity())
                .saturating_add(data.capacity()),
            move |store| store.cache_attachment(&name, &data),
        )
    }
    pub fn prepare_publication(
        &self,
        created: u64,
    ) -> std::result::Result<Reply<Option<PendingPublication>>, QueueError> {
        self.request(128, move |store| store.prepare_publication(created))
    }
    pub fn complete_publication(
        &self,
        pending: PendingPublication,
    ) -> std::result::Result<Reply<Cursor>, QueueError> {
        self.request(
            256usize
                .saturating_add(pending.data.capacity())
                .saturating_add(pending.name.capacity())
                .saturating_add(pending.hash.capacity())
                .saturating_add(strings_size(&pending.revisions)),
            move |store| store.complete_publication(&pending),
        )
    }
    pub fn mark_reconciled(
        &self,
        cursor: Option<Cursor>,
    ) -> std::result::Result<Reply<()>, QueueError> {
        self.request(
            128 + cursor.as_ref().map_or(0, |cursor| cursor.hash.capacity()),
            move |store| store.mark_reconciled(cursor.as_ref()),
        )
    }
    pub fn publish_to(
        &self,
        root: PathBuf,
        created: u64,
    ) -> std::result::Result<Reply<Option<PublicationAck>>, QueueError> {
        self.request(128 + root.as_os_str().len(), move |store| {
            store.publish_to(&root, created)
        })
    }
    /// A message after each save that adds to the outbox; see [`NotebookStore::watch_outbox`].
    pub fn watch_outbox(
        &self,
    ) -> std::result::Result<Reply<futures::channel::mpsc::UnboundedReceiver<u64>>, QueueError>
    {
        self.request(128, |store| Ok(store.watch_outbox()))
    }
    pub fn publication_state(&self) -> std::result::Result<Reply<PublicationState>, QueueError> {
        self.request(128, NotebookStore::publication_state)
    }
    pub fn import_batch(
        &self,
        device: String,
        name: String,
        data: Vec<u8>,
    ) -> std::result::Result<Reply<BatchImport>, QueueError> {
        self.request(
            256usize
                .saturating_add(device.capacity())
                .saturating_add(name.capacity())
                .saturating_add(data.capacity()),
            move |store| store.import_batch(&device, &name, &data),
        )
    }
    pub fn pending_attachments(
        &self,
        after: Option<String>,
        limit: usize,
    ) -> std::result::Result<Reply<Vec<String>>, QueueError> {
        self.request(
            128 + after.as_ref().map_or(0, String::capacity),
            move |store| store.pending_attachments(after.as_deref(), limit),
        )
    }
    pub fn batch_hash(
        &self,
        device: String,
        seq: u32,
    ) -> std::result::Result<Reply<Option<String>>, QueueError> {
        self.request(128 + device.capacity(), move |store| {
            store.batch_hash(&device, seq)
        })
    }
    pub fn rescan(
        &self,
        root: PathBuf,
        limits: RescanLimits,
        resume: Option<RescanCursor>,
    ) -> std::result::Result<Reply<RescanReport>, QueueError> {
        let bytes = 256usize
            .saturating_add(root.as_os_str().len())
            .saturating_add(resume.as_ref().map_or(0, |cursor| {
                cursor
                    .after_device
                    .as_ref()
                    .map_or(0, String::capacity)
                    .saturating_add(cursor.after_attachment.as_ref().map_or(0, String::capacity))
            }));
        self.request(bytes, move |store| {
            store.rescan(&root, limits, resume.as_ref())
        })
    }
}
