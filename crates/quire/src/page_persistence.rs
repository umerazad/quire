use crate::stored_source::StoredSource;
use gpui::{App, AppContext, Context, Entity, SharedString, Subscription, Task};
use language::{Buffer, BufferEvent, BufferSnapshot};
use quire_store::{
    notebook::{CheckedSave, Draft, DraftStamp, Edit, SaveAck},
    worker::StorageClient,
};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub struct OpenedPage {
    pub page: String,
    pub path: String,
    pub base: Vec<String>,
    pub draft: Option<DraftStamp>,
}

#[derive(Clone)]
struct Frozen {
    snapshot: BufferSnapshot,
    stamp: DraftStamp,
    created: u64,
    base: Vec<String>,
    expected: Option<DraftStamp>,
    save: bool,
}

struct Completion {
    recovered: bool,
    heads_changed: bool,
    result: Result<Option<SaveAck>, String>,
}

pub struct PagePersistence {
    buffer: Entity<Buffer>,
    source: Arc<StoredSource>,
    client: StorageClient,
    opened: OpenedPage,
    session: String,
    version: u64,
    seen: BufferSnapshot,
    saved: Option<BufferSnapshot>,
    durable: Option<BufferSnapshot>,
    enabled: bool,
    blocked: bool,
    error: Option<SharedString>,
    pending: Option<Frozen>,
    queued_save: Option<BufferSnapshot>,
    recover_requested: bool,
    task: Option<Task<()>>,
    debounce: Option<Task<()>>,
    autosave: Option<Task<()>>,
    _subscription: Subscription,
}

impl PagePersistence {
    pub fn new(
        buffer: Entity<Buffer>,
        source: Arc<StoredSource>,
        client: StorageClient,
        opened: OpenedPage,
        cx: &mut Context<Self>,
    ) -> Self {
        let snapshot = buffer.read(cx).snapshot();
        let saved = (opened.draft.is_none() && !opened.base.is_empty()).then(|| snapshot.clone());
        let durable = (saved.is_some() || opened.draft.is_some()).then(|| snapshot.clone());
        let subscription = cx.subscribe(&buffer, |this, _, event, cx| {
            if matches!(
                event,
                BufferEvent::Edited { .. }
                    | BufferEvent::Operation {
                        operation: language::Operation::UpdateLineEnding { .. },
                        ..
                    }
            ) {
                this.changed(cx);
            }
        });
        Self {
            buffer,
            source,
            client,
            opened,
            session: uuid::Uuid::new_v4().to_string(),
            version: 0,
            seen: snapshot,
            saved,
            durable,
            enabled: false,
            blocked: false,
            error: None,
            pending: None,
            queued_save: None,
            recover_requested: false,
            task: None,
            debounce: None,
            autosave: None,
            _subscription: subscription,
        }
    }

    pub fn enable(&mut self, cx: &mut Context<Self>) {
        self.enabled = true;
        self.buffer.update(cx, |buffer, cx| {
            buffer.set_capability(language::Capability::ReadWrite, cx)
        });
        if !self.is_secured(cx) {
            self.schedule_recovery(cx);
        }
        cx.notify();
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn has_error(&self) -> bool {
        self.error.is_some()
    }
    pub fn is_dirty(&self, cx: &App) -> bool {
        let current = self.buffer.read(cx).snapshot();
        self.saved
            .as_ref()
            .is_none_or(|saved| !same(saved, &current))
            || !self.secured(&current)
    }
    pub fn is_secured(&self, cx: &App) -> bool {
        let current = self.buffer.read(cx).snapshot();
        self.secured(&current)
    }
    fn secured(&self, current: &BufferSnapshot) -> bool {
        if self
            .pending
            .as_ref()
            .is_some_and(|request| !same(&request.snapshot, current))
            || self
                .queued_save
                .as_ref()
                .is_some_and(|queued| !same(queued, current))
        {
            return false;
        }
        self.durable
            .as_ref()
            .is_some_and(|durable| same(durable, current))
    }
    pub fn status(&self, cx: &App) -> SharedString {
        if let Some(error) = &self.error {
            return error.clone();
        }
        if self.task.is_some() {
            return if self.pending.as_ref().is_some_and(|request| request.save) {
                "Saving captured version…"
            } else {
                "Recording recovery…"
            }
            .into();
        }
        if self.blocked {
            return "This page changed elsewhere. Your text is kept for recovery; review its versions before saving.".into();
        }
        if !self.is_dirty(cx) {
            "Saved locally".into()
        } else if self.is_secured(cx) {
            "Recorded for recovery · not saved locally".into()
        } else {
            "Unsaved changes".into()
        }
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.request(true, cx);
    }
    pub fn recover(&mut self, cx: &mut Context<Self>) {
        self.request(false, cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        let current = self.buffer.read(cx).snapshot();
        if same(&current, &self.seen) {
            return;
        }
        self.seen = current;
        if self.enabled {
            self.schedule_recovery(cx);
            self.schedule_autosave(cx);
        }
        cx.notify();
    }

    fn schedule_recovery(&mut self, cx: &mut Context<Self>) {
        let delay = cx.background_executor().timer(Duration::from_millis(100));
        self.debounce = Some(cx.spawn(async move |this, cx| {
            delay.await;
            this.update(cx, |this, cx| {
                this.debounce = None;
                this.recover(cx);
            })
            .ok();
        }));
    }

    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        let delay = cx.background_executor().timer(Duration::from_millis(450));
        self.autosave = Some(cx.spawn(async move |this, cx| {
            delay.await;
            this.update(cx, |this, cx| {
                this.autosave = None;
                this.save(cx);
            })
            .ok();
        }));
    }

    fn request(&mut self, save: bool, cx: &mut Context<Self>) {
        if !self.enabled {
            return;
        }
        self.debounce = None;
        if save {
            self.autosave = None;
        }
        if save && self.blocked {
            self.error = Some("This page changed elsewhere. Keep the buffer and review the current versions before saving.".into());
            cx.notify();
            return;
        }
        if (save && !self.is_dirty(cx)) || (!save && self.is_secured(cx)) {
            return;
        }
        let snapshot = self.buffer.read(cx).snapshot();
        if self.pending.is_some() {
            if save {
                self.queued_save = Some(snapshot);
            } else {
                self.recover_requested = true;
            }
            if self.task.is_none() {
                self.dispatch(cx);
            }
            return;
        }
        self.start(snapshot, save, cx);
    }

    fn start(&mut self, snapshot: BufferSnapshot, save: bool, cx: &mut Context<Self>) {
        let Some(version) = self
            .version
            .checked_add(1)
            .filter(|version| *version <= i64::MAX as u64)
        else {
            self.error = Some("This editor exhausted its recovery sequence. Keep the buffer and reopen the notebook before retrying.".into());
            cx.notify();
            return;
        };
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|time| u64::try_from(time.as_millis()).ok())
            .filter(|time| *time <= 9_007_199_254_740_991);
        let Some(created) = created else {
            self.error = Some(
                "Cannot timestamp this save. Keep the buffer, correct the system clock, and retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        self.version = version;
        self.pending = Some(Frozen {
            snapshot,
            stamp: DraftStamp {
                session: self.session.clone(),
                version,
            },
            created,
            base: self.opened.base.clone(),
            expected: self.opened.draft.clone(),
            save,
        });
        self.dispatch(cx);
    }

    fn dispatch(&mut self, cx: &mut Context<Self>) {
        let request = self
            .pending
            .as_ref()
            .expect("pending persistence request")
            .clone();
        let source = self.source.clone();
        let client = self.client.clone();
        let page = self.opened.page.clone();
        let path = self.opened.path.clone();
        let recovered = self.opened.draft.as_ref() == Some(&request.stamp);
        self.error = None;
        let work = cx.background_spawn(async move {
            persist(client, source, page, path, request, recovered).await
        });
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| this.finish(result, cx)).ok();
        }));
        cx.notify();
    }

    fn finish(&mut self, completion: Completion, cx: &mut Context<Self>) {
        self.task = None;
        let request = self.pending.take().expect("completed persistence request");
        if completion.recovered {
            self.opened.draft = Some(request.stamp.clone());
            self.durable = Some(request.snapshot.clone());
        }
        let succeeded = completion.result.is_ok();
        match completion.result {
            Ok(Some(saved)) => {
                self.opened.base = vec![saved.revision_id().to_owned()];
                self.opened.draft = None;
                self.saved = Some(request.snapshot.clone());
                self.buffer.update(cx, |buffer, cx| {
                    buffer.did_save(request.snapshot.version().clone(), None, cx)
                });
                self.durable = Some(request.snapshot);
            }
            Ok(None) => {}
            Err(error) => {
                self.error = Some(error.into());
                if completion.heads_changed {
                    self.blocked = true;
                    self.queued_save = None;
                } else {
                    self.pending = Some(request);
                }
            }
        }
        cx.notify();
        if succeeded {
            if let Some(snapshot) = self.queued_save.take() {
                self.start(snapshot, true, cx);
            } else if self.recover_requested {
                self.recover_requested = false;
                self.recover(cx);
            } else if !self.is_secured(cx) && self.debounce.is_none() {
                self.schedule_recovery(cx);
            }
        } else if completion.heads_changed && !self.is_secured(cx) {
            // The checked save definitely made no revision; newer typing can still be recovered.
            self.recover_requested = false;
            self.recover(cx);
        }
    }
}

fn same(left: &BufferSnapshot, right: &BufferSnapshot) -> bool {
    left.remote_id() == right.remote_id()
        && left.version() == right.version()
        && left.line_ending() == right.line_ending()
}

async fn persist(
    client: StorageClient,
    source: Arc<StoredSource>,
    page: String,
    path: String,
    request: Frozen,
    mut recovered: bool,
) -> Completion {
    let result = async {
        let text = source.render(&request.snapshot)?;
        if !recovered {
            client
                .put_draft(
                    Draft {
                        page: page.clone(),
                        path: path.clone(),
                        text: text.to_string(),
                        base: request.base.clone(),
                        stamp: request.stamp.clone(),
                        created: request.created,
                    },
                    request.expected.clone(),
                )
                .map_err(|error| error.to_string())?
                .await
                .map_err(|error| format!("{error:#}"))?;
            recovered = true;
        }
        if !request.save {
            return Ok(None);
        }
        let saved = client
            .save_if_heads(
                Edit {
                    page,
                    path,
                    text: text.to_string(),
                    base: request.base.clone(),
                    deleted: false,
                    created: request.created,
                    recovery: Some(request.stamp),
                },
                request.base,
            )
            .map_err(|error| error.to_string())?
            .await
            .map_err(|error| format!("{error:#}"))?;
        Ok(Some(saved))
    }
    .await;
    match result {
        Ok(Some(CheckedSave::HeadsChanged { .. })) => Completion { recovered, heads_changed: true, result: Err("This page changed elsewhere before the save. Your captured text is recorded for recovery; review its versions before saving.".into()) },
        Ok(Some(CheckedSave::Saved(saved))) => Completion { recovered, heads_changed: false, result: Ok(Some(saved)) },
        Ok(None) => Completion { recovered, heads_changed: false, result: Ok(None) },
        Err(error) => Completion { recovered, heads_changed: false, result: Err(error) },
    }
}

#[cfg(test)]
pub(crate) mod tests;
