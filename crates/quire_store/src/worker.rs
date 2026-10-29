use crate::notebook::{
    Cursor, Draft, DraftStamp, Edit, NotebookStore, PendingPublication, SaveAck,
};
use crate::sync::json::Json;
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
type Work = Box<dyn FnOnce(&NotebookStore) + Send>;
struct Request {
    run: Work,
    admission: Admission,
}
struct Shared {
    sender: Mutex<Option<SyncSender<Request>>>,
    budget: Arc<Budget>,
}
#[derive(Clone)]
pub struct StorageClient {
    shared: Arc<Shared>,
}
pub struct StorageWorker {
    pub client: StorageClient,
    pub ready: Reply<()>,
    pub stopped: Reply<()>,
}

impl StorageWorker {
    /// Starts without performing filesystem work on the calling thread.
    pub fn spawn(
        path: PathBuf,
        notebook: String,
        device: String,
        limits: QueueLimits,
    ) -> Result<Self> {
        ensure!(
            limits.requests > 0 && limits.bytes > 0,
            "Storage queue limits must be positive. Correct the limits before starting the worker."
        );
        let (sender, receiver) = mpsc::sync_channel::<Request>(limits.requests);
        let budget = Arc::new(Budget {
            limit: limits.bytes,
            requests: AtomicUsize::new(0),
            bytes: AtomicUsize::new(0),
            active: AtomicBool::new(false),
        });
        let client = StorageClient {
            shared: Arc::new(Shared {
                sender: Mutex::new(Some(sender)),
                budget: budget.clone(),
            }),
        };
        let (ready_send, ready) = oneshot::channel();
        let (stopped_send, stopped) = oneshot::channel();
        let thread = std::thread::Builder::new().name("quire-storage".into()).spawn(move || {
            match NotebookStore::open(&path, &notebook, &device) {
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
        }).context("Cannot start notebook storage worker. Keep the buffer and retry after freeing process resources.")?;
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
fn gate_error(
    error: TryLockError<std::sync::MutexGuard<'_, Option<SyncSender<Request>>>>,
) -> QueueError {
    match error {
        TryLockError::WouldBlock => QueueError::Busy,
        TryLockError::Poisoned(_) => QueueError::Stopped,
    }
}
impl StorageClient {
    fn request<T: Send + 'static>(
        &self,
        bytes: usize,
        work: impl FnOnce(&NotebookStore) -> Result<T> + Send + 'static,
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
    pub fn save(&self, edit: Edit) -> std::result::Result<Reply<SaveAck>, QueueError> {
        self.request(edit_size(&edit), move |store| store.save(&edit))
    }
    pub fn save_many(
        &self,
        edits: Vec<Edit>,
    ) -> std::result::Result<Reply<Vec<SaveAck>>, QueueError> {
        self.request(
            edits
                .iter()
                .fold(0usize, |size, edit| size.saturating_add(edit_size(edit))),
            move |store| store.save_many(&edits),
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
}
