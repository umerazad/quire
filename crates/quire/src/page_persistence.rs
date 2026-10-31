use crate::notebook_backend::{NotebookBackend, folder_conflict};
use crate::stored_source::StoredSource;
use gpui::{App, AppContext, Context, Entity, SharedString, Subscription, Task};
use language::{Buffer, BufferEvent, BufferSnapshot};
use quire_core::{merge::merge_text, source_text::SourceText};
use quire_store::{
    folder_notebook::{DiskDocument, FolderConflict},
    notebook::{Draft, DraftStamp, Edit},
    sync::json::Json,
};
use std::{
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RECOVERY_DELAY: Duration = Duration::from_millis(100);
const SAVE_IDLE: Duration = Duration::from_millis(450);
const SAVE_LATEST: Duration = Duration::from_secs(2);

pub struct OpenedPage {
    pub page: String,
    pub path: String,
    pub base: Vec<String>,
    pub draft: Option<DraftStamp>,
    /// Stored text of `base`, when the checkout read it; fetched on demand otherwise.
    pub base_text: Option<Arc<str>>,
    /// Save the restored draft as soon as editing is enabled, merging it into newer versions.
    pub settle: bool,
}

#[derive(Clone)]
struct Frozen {
    snapshot: BufferSnapshot,
    source: Arc<StoredSource>,
    stamp: DraftStamp,
    created: u64,
    base: Vec<String>,
    expected: Option<DraftStamp>,
    save: bool,
}

struct Saved {
    revision: String,
    text: Arc<str>,
    /// The merge revision and its text, when this save left several versions.
    merged: Option<(String, Arc<str>)>,
    overlaps: usize,
    /// The page's only head is this save's result.
    settled: bool,
}

struct Completion {
    recovered: bool,
    result: Result<Option<Saved>, String>,
    /// A folder notebook refused the save because its file is not the version it was made from.
    conflict: Option<FolderConflict>,
}

/// A folder notebook page whose file changed or disappeared while it had unsaved edits, or
/// whose save was refused for that reason. Saving stops until the user chooses a version;
/// recovery keeps recording the buffer meanwhile.
#[derive(Clone, Debug)]
pub struct Conflict {
    /// The file as it is now, or None when it no longer exists.
    pub disk: Option<DiskDocument>,
    pub message: SharedString,
}

/// A version to merge into the buffer: the edits made since `base` merge into `text`.
#[derive(Clone)]
struct Known {
    revision: String,
    base: Arc<str>,
    text: Arc<str>,
}

enum Incoming {
    /// The store moved on; read its current version. A matching hint avoids reading the body.
    Store {
        hint: Option<(String, Arc<str>)>,
    },
    Known(Known),
}

struct Plan {
    known: Known,
    snapshot: BufferSnapshot,
    diff: language::Diff,
    original: SourceText,
    clean: bool,
    overlaps: usize,
}

enum Resolution {
    Current,
    Removed,
    Merge(Box<Plan>),
}

pub struct PagePersistence {
    buffer: Entity<Buffer>,
    source: Arc<StoredSource>,
    client: NotebookBackend,
    opened: OpenedPage,
    session: String,
    conflict: Option<Conflict>,
    /// An outside change that arrived while a save or merge was in flight.
    deferred: Option<Option<DiskDocument>>,
    version: u64,
    seen: BufferSnapshot,
    saved: Option<BufferSnapshot>,
    durable: Option<BufferSnapshot>,
    enabled: bool,
    error: Option<SharedString>,
    warning: Option<SharedString>,
    removed: bool,
    retired: bool,
    pending: Option<Frozen>,
    queued_save: Option<BufferSnapshot>,
    recover_requested: bool,
    stale: bool,
    held: bool,
    hint: Option<(String, Arc<str>)>,
    save_by: Option<Instant>,
    task: Option<Task<()>>,
    merge: Option<Task<()>>,
    debounce: Option<Task<()>>,
    autosave: Option<Task<()>>,
    _subscription: Subscription,
}

impl PagePersistence {
    pub fn new(
        buffer: Entity<Buffer>,
        source: Arc<StoredSource>,
        client: impl Into<NotebookBackend>,
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
            client: client.into(),
            opened,
            session: uuid::Uuid::new_v4().to_string(),
            conflict: None,
            deferred: None,
            version: 0,
            seen: snapshot,
            saved,
            durable,
            enabled: false,
            error: None,
            warning: None,
            removed: false,
            retired: false,
            pending: None,
            queued_save: None,
            recover_requested: false,
            stale: false,
            held: false,
            hint: None,
            save_by: None,
            task: None,
            merge: None,
            debounce: None,
            autosave: None,
            _subscription: subscription,
        }
    }

    pub fn enable(&mut self, cx: &mut Context<Self>) {
        if self.retired {
            return;
        }
        self.enabled = true;
        self.buffer.update(cx, |buffer, cx| {
            buffer.set_capability(language::Capability::ReadWrite, cx)
        });
        if !self.is_secured(cx) {
            self.schedule_recovery(cx);
        }
        if std::mem::take(&mut self.opened.settle) {
            self.save(cx);
        }
        cx.notify();
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }
    pub fn has_error(&self) -> bool {
        self.error.is_some() || self.conflict.is_some()
    }
    /// The status says more than how far the text is saved: an error, a conflict, a deletion,
    /// or a warning that the reader needs to see without hovering.
    pub fn has_notice(&self) -> bool {
        self.has_error() || self.retired || self.removed || self.warning.is_some()
    }
    /// The folder conflict waiting for the user to choose a version, if any.
    pub fn conflict(&self) -> Option<&Conflict> {
        self.conflict.as_ref()
    }
    /// The stored-source adapter for the buffer's current lineage; it follows merged versions.
    pub fn source(&self) -> Arc<StoredSource> {
        self.source.clone()
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
        if let Some(conflict) = &self.conflict {
            let recorded = if self.is_secured(cx) {
                "Your edits are recorded locally."
            } else {
                "Recording your edits locally…"
            };
            return format!("{} {recorded}", conflict.message).into();
        }
        let state = if self.task.is_some() {
            if self.pending.as_ref().is_some_and(|request| request.save) {
                "Saving captured version…"
            } else {
                "Recording recovery…"
            }
        } else if self.merge.is_some() {
            "Merging changes made elsewhere…"
        } else if !self.is_dirty(cx) {
            "Saved locally"
        } else if self.is_secured(cx) {
            "Recorded for recovery · not saved locally"
        } else {
            "Unsaved changes"
        };
        let removed = if self.retired {
            Some("This page was deleted. Its text stays open here, read-only.")
        } else {
            self.removed.then_some(
                "This page was deleted elsewhere. Your text stays open here; edit and save it to restore the page.",
            )
        };
        [Some(state), removed, self.warning.as_deref()]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ")
            .into()
    }

    pub fn save(&mut self, cx: &mut Context<Self>) {
        self.request(true, cx);
    }
    pub fn recover(&mut self, cx: &mut Context<Self>) {
        self.request(false, cx);
    }

    /// Another writer stored `text` as `revision` of this page, for example a bulk tag edit or
    /// an imported change. Edits made here since the last saved text merge into it as one buffer
    /// edit outside undo history; a page without unsaved changes simply shows the new version.
    ///
    /// While a save is on its way the merge waits: that save's acknowledgment can already include
    /// this version, or this version can already include that save, and only the store knows which.
    /// Once the save is acknowledged the page merges the store's current version instead of
    /// `text`. The same reconciliation runs whenever an acknowledgment shows that the page moved
    /// on, so a late or reordered call never applies a version older than the saved one.
    pub fn absorb(&mut self, revision: String, text: impl Into<Arc<str>>, cx: &mut Context<Self>) {
        if self.opened.base == [revision.as_str()] {
            return;
        }
        self.hint = Some((revision, text.into()));
        self.stale = true;
        self.reconcile(cx);
    }

    /// Another writer's version of this page arrived in the store, for example by an import:
    /// merge it as [`Self::absorb`] does, reading the version from the store.
    pub fn store_moved(&mut self, cx: &mut Context<Self>) {
        self.stale = true;
        self.reconcile(cx);
    }

    fn saving(&self) -> bool {
        self.task.is_some() && self.pending.as_ref().is_some_and(|request| request.save)
    }

    fn reconcile(&mut self, cx: &mut Context<Self>) {
        if !self.stale || self.merge.is_some() || self.saving() || self.conflict.is_some() {
            return;
        }
        self.stale = false;
        let hint = self.hint.take();
        self.start_merge(Incoming::Store { hint }, cx);
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
        let delay = cx.background_executor().timer(RECOVERY_DELAY);
        self.debounce = Some(cx.spawn(async move |this, cx| {
            delay.await;
            this.update(cx, |this, cx| {
                this.debounce = None;
                this.recover(cx);
            })
            .ok();
        }));
    }

    /// Saves after 450 ms without typing, and no later than 2 s after the first unsaved edit.
    fn schedule_autosave(&mut self, cx: &mut Context<Self>) {
        let now = cx.background_executor().now();
        let save_by = *self.save_by.get_or_insert(now + SAVE_LATEST);
        let delay = cx
            .background_executor()
            .timer(SAVE_IDLE.min(save_by.saturating_duration_since(now)));
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
        if save && self.conflict.is_some() {
            self.autosave = None;
            self.save_by = None;
            return;
        }
        self.debounce = None;
        if save {
            self.autosave = None;
            self.save_by = None;
        }
        if (save && !self.is_dirty(cx)) || (!save && self.is_secured(cx)) {
            return;
        }
        let snapshot = self.buffer.read(cx).snapshot();
        if save && (self.merge.is_some() || self.held) {
            self.queued_save = Some(snapshot);
            return;
        }
        if let Some(pending) = &self.pending {
            let blocked = pending.save && self.merge.is_some();
            if save {
                self.queued_save = Some(snapshot);
            } else {
                self.recover_requested = true;
            }
            if self.task.is_none() && !blocked {
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
            source: self.source.clone(),
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
        let client = self.client.clone();
        let page = self.opened.page.clone();
        let path = self.opened.path.clone();
        let recovered = self.opened.draft.as_ref() == Some(&request.stamp);
        self.error = None;
        let work = cx
            .background_spawn(async move { persist(client, page, path, request, recovered).await });
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
        let succeeded = completion.result.is_ok() || completion.conflict.is_some();
        match (completion.result, completion.conflict) {
            (Ok(Some(saved)), _) => self.saved(request, saved, cx),
            (Ok(None), _) => {}
            (Err(_), Some(refused)) => {
                // The refusal read the disk after any change queued meanwhile.
                self.deferred = None;
                self.enter_conflict(refused.disk, refused.message.into(), cx)
            }
            (Err(error), None) => {
                self.error = Some(error.into());
                self.pending = Some(request);
            }
        }
        cx.notify();
        if succeeded {
            if self.queued_save.is_some() && self.merge.is_none() && !self.held {
                let snapshot = self.queued_save.take().expect("queued save");
                self.start(snapshot, true, cx);
            } else if self.recover_requested {
                self.recover_requested = false;
                self.recover(cx);
            } else if !self.is_secured(cx) && self.debounce.is_none() {
                self.schedule_recovery(cx);
            }
        }
        self.reconcile(cx);
        self.resume_external(cx);
    }

    fn saved(&mut self, request: Frozen, saved: Saved, cx: &mut Context<Self>) {
        self.opened.draft = None;
        self.opened.base = vec![saved.revision];
        self.opened.base_text = Some(saved.text.clone());
        self.saved = Some(request.snapshot.clone());
        self.buffer.update(cx, |buffer, cx| {
            buffer.did_save(request.snapshot.version().clone(), None, cx)
        });
        self.durable = Some(request.snapshot);
        if saved.overlaps > 0 {
            self.warning = Some(overlap_warning(&self.opened.path));
        }
        if saved.settled {
            self.removed = false;
        } else {
            self.stale = true;
        }
        match saved.merged {
            Some((revision, text)) if text == saved.text => self.opened.base = vec![revision],
            Some((revision, text)) => self.start_merge(
                Incoming::Known(Known {
                    revision,
                    base: saved.text,
                    text,
                }),
                cx,
            ),
            None => {}
        }
    }

    fn start_merge(&mut self, incoming: Incoming, cx: &mut Context<Self>) {
        let snapshot = self.buffer.read(cx).snapshot();
        let source = self.source.clone();
        let client = self.client.clone();
        let page = self.opened.page.clone();
        let base = self.opened.base.clone();
        let base_text = self.opened.base_text.clone();
        let issued = base.clone();
        let work = cx.background_spawn(async move {
            let known = match incoming {
                Incoming::Known(known) => known,
                Incoming::Store { hint } => {
                    match current(client, page, base, base_text, hint).await? {
                        Ok(known) => known,
                        Err(resolution) => return Ok(resolution),
                    }
                }
            };
            plan(snapshot, source, known).map(|plan| Resolution::Merge(Box::new(plan)))
        });
        self.merge = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| this.merged(issued, result, cx))
                .ok();
        }));
        cx.notify();
    }

    fn merged(
        &mut self,
        issued: Vec<String>,
        result: Result<Resolution, String>,
        cx: &mut Context<Self>,
    ) {
        self.merge = None;
        match result {
            Err(error) => self.error = Some(error.into()),
            Ok(Resolution::Current) => {}
            Ok(Resolution::Removed) => self.removed = true,
            // A save landed meanwhile: merge the version again against the new base.
            Ok(Resolution::Merge(plan)) if self.opened.base != issued => {
                self.absorb(plan.known.revision, plan.known.text, cx)
            }
            // Typing landed meanwhile: recompute rather than drop conflicting hunks.
            Ok(Resolution::Merge(plan))
                if !same(&plan.snapshot, &self.buffer.read(cx).snapshot()) =>
            {
                self.start_merge(Incoming::Known(plan.known), cx)
            }
            Ok(Resolution::Merge(plan)) => self.apply(*plan, cx),
        }
        if self.queued_save.take().is_some() {
            self.save(cx);
        }
        if std::mem::take(&mut self.recover_requested) {
            self.recover(cx);
        }
        self.reconcile(cx);
        self.resume_external(cx);
        cx.notify();
    }

    /// Applies a merge computed against the current snapshot as one edit outside undo history.
    fn apply(&mut self, plan: Plan, cx: &mut Context<Self>) {
        let snapshot = self.buffer.update(cx, |buffer, cx| {
            buffer.finalize_last_transaction();
            if let Some(transaction) = buffer.apply_diff(plan.diff, cx) {
                buffer.forget_transaction(transaction);
            }
            buffer.snapshot()
        });
        self.source = Arc::new(StoredSource::rebased(plan.original, snapshot.clone()));
        self.opened.base = vec![plan.known.revision];
        self.opened.base_text = Some(plan.known.text);
        self.seen = snapshot.clone();
        self.removed = false;
        if plan.overlaps > 0 {
            self.warning = Some(overlap_warning(&self.opened.path));
        }
        if plan.clean {
            self.buffer.update(cx, |buffer, cx| {
                buffer.did_save(snapshot.version().clone(), None, cx)
            });
            self.saved = Some(snapshot.clone());
            self.durable = Some(snapshot);
            self.autosave = None;
            self.save_by = None;
            if let (Some(folder), Some(text)) = (self.client.folder(), &self.opened.base_text) {
                // A draft that already equals the file has nothing left to recover.
                folder
                    .clear_draft(self.opened.page.clone(), text.to_string())
                    .map(|reply| cx.background_spawn(reply).detach())
                    .ok();
            }
        } else {
            self.saved = None;
            if self.enabled {
                self.schedule_recovery(cx);
                self.schedule_autosave(cx);
            }
        }
    }
}

const CHANGED_OUTSIDE: &str =
    "This file changed outside Quire. Compare versions before continuing.";
const REMOVED_OUTSIDE: &str =
    "This file was removed outside Quire. Save a copy to keep this page, or accept the deletion.";
const RECOVERED: &str =
    "Recovered unsaved work. Compare it with the disk version, or save a separate copy.";
const ACCEPTED_REMOVAL: &str =
    "This file was removed outside Quire. Open another page, or create this one again.";

/// Outside changes to a folder notebook's files, and choosing a version after a conflict.
impl PagePersistence {
    /// The page's file changed (`Some`) or disappeared (`None`) outside Quire. A page without
    /// unsaved changes shows the new version; one with changes keeps them and enters a conflict
    /// until the user chooses a version, as the reference folder notebook does. A change that
    /// arrives during a save or merge waits for it, so its outcome decides.
    pub fn external(&mut self, disk: Option<DiskDocument>, cx: &mut Context<Self>) {
        if disk
            .as_ref()
            .is_some_and(|disk| self.opened.base == [disk.revision.as_str()])
        {
            // The file is back at the version these edits were made from, so saving can resume.
            if self.conflict.take().is_some() {
                if self.enabled && self.is_dirty(cx) {
                    self.schedule_autosave(cx);
                }
                cx.notify();
            }
            return;
        }
        if self.task.is_some() || self.merge.is_some() {
            self.deferred = Some(disk);
            return;
        }
        let local = match self.source.render(&self.buffer.read(cx).snapshot()) {
            Ok(local) => local,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        match disk {
            Some(disk)
                if *local == *disk.text || (self.conflict.is_none() && !self.is_dirty(cx)) =>
            {
                self.conflict = None;
                if self.warning.as_deref() == Some(ACCEPTED_REMOVAL) {
                    self.warning = None;
                    self.buffer.update(cx, |buffer, cx| {
                        buffer.set_capability(language::Capability::ReadWrite, cx)
                    });
                }
                self.show(disk, local, cx);
            }
            Some(disk) => self.enter_conflict(Some(disk), CHANGED_OUTSIDE.into(), cx),
            None => self.enter_conflict(None, REMOVED_OUTSIDE.into(), cx),
        }
    }

    fn resume_external(&mut self, cx: &mut Context<Self>) {
        if self.task.is_none()
            && self.merge.is_none()
            && let Some(disk) = self.deferred.take()
        {
            self.external(disk, cx);
        }
    }

    /// Replaces the buffer with `disk` as one edit outside undo history and marks it saved.
    fn show(&mut self, disk: DiskDocument, local: Arc<str>, cx: &mut Context<Self>) {
        self.start_merge(
            Incoming::Known(Known {
                revision: disk.revision,
                base: local,
                text: disk.text.into(),
            }),
            cx,
        );
    }

    fn enter_conflict(
        &mut self,
        disk: Option<DiskDocument>,
        message: SharedString,
        cx: &mut Context<Self>,
    ) {
        self.conflict = Some(Conflict { disk, message });
        self.error = None;
        self.autosave = None;
        self.save_by = None;
        self.queued_save = None;
        self.recover(cx);
        cx.notify();
    }

    /// Opens a recovered folder draft that was typed into another version than the file holds.
    pub fn recovered_conflict(&mut self, disk: Option<DiskDocument>, cx: &mut Context<Self>) {
        self.conflict = Some(Conflict {
            disk,
            message: RECOVERED.into(),
        });
        cx.notify();
    }

    /// The page's current text in stored form, for saving it as a copy.
    pub fn local_text(&self, cx: &App) -> Result<Arc<str>, String> {
        self.source.render(&self.buffer.read(cx).snapshot())
    }

    /// Resolves a conflict with the version on disk: the buffer shows the file, or becomes
    /// read-only when the file was removed, and this page's recovery draft is discarded. Callers
    /// keep the unsaved text first when the user asked to, through a saved copy.
    pub fn use_disk(&mut self, cx: &mut Context<Self>) {
        let Some(conflict) = self.conflict.take() else {
            return;
        };
        self.debounce = None;
        let local = match self.local_text(cx) {
            Ok(local) => local,
            Err(error) => {
                self.conflict = Some(conflict);
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        match conflict.disk {
            Some(disk) => self.show(disk, local, cx),
            None => {
                self.opened.base.clear();
                self.opened.base_text = None;
                self.warning = Some(ACCEPTED_REMOVAL.into());
                let snapshot = self.buffer.update(cx, |buffer, cx| {
                    buffer.set_capability(language::Capability::ReadOnly, cx);
                    buffer.snapshot()
                });
                self.saved = Some(snapshot.clone());
                self.durable = Some(snapshot);
            }
        }
        if let Some(folder) = self.client.folder() {
            let discard = folder.discard_draft(self.opened.page.clone());
            self.opened.draft = None;
            cx.spawn(async move |this, cx| {
                let result = crate::query_index::ask(discard).await;
                if let Err(error) = result {
                    this.update(cx, |this, cx| {
                        this.error = Some(format!("Showing the disk version, but its old recovery copy could not be removed: {error}. It will reopen for review; choose the disk version again then.").into());
                        cx.notify();
                    })
                    .ok();
                }
            })
            .detach();
        }
        cx.notify();
    }
}

/// Coordination with other writers of an open page, used by bulk tag edits.
impl PagePersistence {
    pub fn buffer(&self) -> &Entity<Buffer> {
        &self.buffer
    }

    /// The revisions the next save replaces.
    pub fn base(&self) -> &[String] {
        &self.opened.base
    }

    pub fn path(&self) -> &str {
        &self.opened.path
    }

    /// Defers this page's saves while another writer saves it.
    pub fn hold(&mut self) {
        self.held = true;
        self.autosave = None;
    }

    /// No save or merge is in flight.
    pub fn is_idle(&self) -> bool {
        self.task.is_none() && self.merge.is_none()
    }

    pub fn release(&mut self, cx: &mut Context<Self>) {
        self.held = false;
        if self.queued_save.take().is_some() {
            self.save(cx);
        }
    }

    /// Records that the page now lives at `path`, so later saves keep it there.
    pub fn relocate(&mut self, path: String, cx: &mut Context<Self>) {
        if self.opened.path != path {
            self.opened.path = path;
            cx.notify();
        }
    }

    /// Stops saving for good because the page was deleted here: no later save or recovery may
    /// revive it. The buffer keeps its text, read-only.
    pub fn retire(&mut self, cx: &mut Context<Self>) {
        self.retired = true;
        self.enabled = false;
        self.held = false;
        self.queued_save = None;
        self.recover_requested = false;
        self.autosave = None;
        self.debounce = None;
        self.save_by = None;
        self.buffer.update(cx, |buffer, cx| {
            buffer.set_capability(language::Capability::ReadOnly, cx)
        });
        cx.notify();
    }

    pub fn is_retired(&self) -> bool {
        self.retired
    }
}

fn same(left: &BufferSnapshot, right: &BufferSnapshot) -> bool {
    left.remote_id() == right.remote_id()
        && left.version() == right.version()
        && left.line_ending() == right.line_ending()
}

fn overlap_warning(path: &str) -> SharedString {
    format!("{path} was edited in two places at once and some of the same lines changed both ways. Quire kept both versions of those lines next to each other; check the page and remove the one you don't want.").into()
}

fn revision_text(record: &Json) -> Result<Arc<str>, String> {
    record
        .root()
        .field("text")
        .as_str()
        .map(Arc::from)
        .ok_or_else(|| "A newer version of this page contains UTF-16 code units the native editor cannot represent. Your text is kept here; open the page with a compatible Quire version to combine them.".into())
}

fn revision_id(record: &Json) -> String {
    record
        .root()
        .field("id")
        .as_str()
        .expect("validated revision id")
        .to_owned()
}

async fn read(client: &NotebookBackend, id: String) -> Result<Json, String> {
    client
        .get(id)
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| format!("{error:#}"))?
        .ok_or_else(|| "A version of this page is missing from storage. Your text is kept here; preserve the database and reopen the notebook.".to_owned())
}

/// Reads the page's single current version. Every version stored since `base` descends from it,
/// so the edits made here since `base` merge into it safely.
async fn current(
    client: NotebookBackend,
    page: String,
    base: Vec<String>,
    base_text: Option<Arc<str>>,
    hint: Option<(String, Arc<str>)>,
) -> Result<Result<Known, Resolution>, String> {
    let heads = client
        .heads(page.clone())
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| format!("{error:#}"))?;
    let head = match heads.as_slice() {
        [] => return Ok(Err(Resolution::Current)),
        [head] if base == [head.as_str()] => return Ok(Err(Resolution::Current)),
        [head] => head.clone(),
        _ => return Err("This page has competing versions that could not be merged. Your text is kept here; reopen the notebook to retry the merge, then save again.".into()),
    };
    let text = match hint.filter(|(revision, _)| *revision == head) {
        Some((_, text)) => text,
        None => {
            let record = read(&client, head.clone()).await?;
            if record.root().field("deleted").as_bool() == Some(true) {
                return Ok(Err(Resolution::Removed));
            }
            revision_text(&record)?
        }
    };
    let base = match (base_text, base.as_slice()) {
        (Some(text), _) => text,
        (None, [id]) => revision_text(&read(&client, id.clone()).await?)?,
        (None, _) => return Err("This page was opened without one saved version to merge from. Your text is kept here; save it, then reopen the page.".into()),
    };
    Ok(Ok(Known {
        revision: head,
        base,
        text,
    }))
}

/// Merges the edits made in `snapshot` since `known.base` into `known.text`, in stored form so
/// original line endings survive, and returns the buffer edits that reach the merged text.
fn plan(snapshot: BufferSnapshot, source: Arc<StoredSource>, known: Known) -> Result<Plan, String> {
    let local = source.render(&snapshot)?;
    let units = |text: &str| text.encode_utf16().collect::<Vec<_>>();
    let merged = merge_text(&units(&known.base), &units(&known.text), &units(&local));
    let text = String::from_utf16(&merged.text).map_err(|_| {
        "Merging this page's versions split a Unicode character. Your text is kept here; save it again to retry.".to_owned()
    })?;
    let clean = *text == *known.text;
    let (original, normalized) = SourceText::prepare(text);
    let diff = language::Diff {
        base_version: snapshot.version().clone(),
        line_ending: snapshot.line_ending(),
        edits: language::text_diff(&snapshot.text(), &normalized),
    };
    Ok(Plan {
        known,
        snapshot,
        diff,
        original,
        clean,
        overlaps: merged.overlaps,
    })
}

/// Records recovery, then saves from the captured base. A base that is no longer the page's
/// head is saved anyway: the store merges it with the newer versions in the same transaction.
async fn persist(
    client: NotebookBackend,
    page: String,
    path: String,
    request: Frozen,
    mut recovered: bool,
) -> Completion {
    let mut conflict = None;
    let result = async {
        let text = request.source.render(&request.snapshot)?;
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
        let ack = client
            .save(Edit {
                page,
                path,
                text: text.to_string(),
                base: request.base,
                deleted: false,
                created: request.created,
                recovery: Some(request.stamp),
            })
            .map_err(|error| error.to_string())?
            .await
            .map_err(|error| {
                conflict = folder_conflict(&error).cloned();
                format!("{error:#}")
            })?;
        let merged = ack
            .merged
            .as_ref()
            .map(|record| Ok::<_, String>((revision_id(record), revision_text(record)?)))
            .transpose()?;
        let head = merged
            .as_ref()
            .map_or(ack.revision_id(), |(id, _)| id.as_str());
        Ok(Some(Saved {
            settled: ack.heads == [head],
            revision: ack.revision_id().to_owned(),
            text,
            merged,
            overlaps: ack.overlaps,
        }))
    }
    .await;
    Completion {
        recovered,
        result,
        conflict,
    }
}

#[cfg(test)]
pub(crate) mod tests;

#[cfg(test)]
pub(crate) mod folder_tests;
