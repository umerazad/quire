//! A Quire notebook's live sync in the window: saves publish to the synced folder on the
//! schedule in [`quire_store::live_sync`], other writers' change files import as they arrive,
//! and this writer's `status.json` announces it to every other Quire app. Publishing and
//! importing run on the storage worker; timers run on the executor, and heartbeats use the
//! injected [`Clock`], so tests drive both with a fake clock.

use crate::lifecycle::Activity;
use futures::StreamExt as _;
use gpui::{
    App, AppContext as _, Context, EventEmitter, IntoElement, ParentElement as _, Render,
    SharedString, Styled as _, Task, Window,
};
use quire_store::{
    live_sync::{
        self, DeviceStatus, Imports, ProblemMemory, PublishTimer, RESCAN_EVERY, RETRY_AFTER,
        StatusMemory, WATCH_DELAY,
    },
    notebook::{PublicationState, RescanLimits, RescanReport},
    worker::{QueueError, Reply, StorageClient},
};
use std::{
    path::PathBuf,
    rc::Rc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use ui::Label;

/// Milliseconds since the epoch, for heartbeats other computers compare with their own.
pub type Clock = Rc<dyn Fn() -> u64>;

pub fn system_clock() -> Clock {
    Rc::new(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as u64)
    })
}

/// Who this writer is in `status.json`.
#[derive(Clone, Debug)]
pub struct Identity {
    pub device: String,
    /// The computer: its host name, shared by every Quire app on it.
    pub label: String,
    pub app: String,
}

impl Identity {
    pub fn this_computer(device: String) -> Self {
        Self {
            device,
            label: live_sync::host_label(),
            app: concat!("quire-native ", env!("CARGO_PKG_VERSION")).into(),
        }
    }
}

/// Pages another writer changed, by page identity, for open pages to take in.
pub struct Imported(pub Vec<String>);

const PUBLISHING: &str = "Saved changes are being published to the synced folder.";

pub struct LiveSync {
    client: StorageClient,
    root: PathBuf,
    identity: Identity,
    clock: Clock,
    timer: PublishTimer,
    /// The scheduled publication, replaced by each save.
    publish_at: Option<Task<()>>,
    publishing: bool,
    publish_again: bool,
    /// The number of the latest save heard of, and of the latest one an outbox count includes.
    saves: u64,
    counted: u64,
    /// Whether this writer's status was written since the notebook opened.
    announced: bool,
    pending: u64,
    status: StatusMemory,
    nearby: bool,
    computers: Vec<DeviceStatus>,
    imports: Imports,
    problems: ProblemMemory,
    retry: Option<Task<()>>,
    /// Watcher events wait [`WATCH_DELAY`] for more before an import starts.
    debounce: Option<Task<()>>,
    publish_error: Option<(SharedString, Instant)>,
    import_error: Option<SharedString>,
    warnings: Vec<SharedString>,
    /// Holds each publication until released, so tests can see a slow folder.
    #[cfg(test)]
    pub hold: Option<futures::future::Shared<futures::channel::oneshot::Receiver<()>>>,
    /// Holds each import after its rescan, before its outcome is recorded.
    #[cfg(test)]
    pub hold_import: Option<futures::future::Shared<futures::channel::oneshot::Receiver<()>>>,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<Imported> for LiveSync {}

/// Every page a full rescan changed and every problem it hit, following continuations.
async fn rescan_all(client: StorageClient, root: PathBuf) -> Result<RescanReport, String> {
    let mut total = RescanReport::default();
    let mut resume = None;
    loop {
        let report = ask(client.rescan(root.clone(), RescanLimits::default(), resume)).await?;
        resume = report.continuation.clone();
        total.applied += report.applied;
        total.pages.extend(report.pages);
        total.merged.extend(report.merged);
        total.problems.extend(report.problems);
        if resume.is_none() {
            return Ok(total);
        }
    }
}

async fn ask<T>(reply: Result<Reply<T>, QueueError>) -> Result<T, String> {
    reply
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| format!("{error:#}"))
}

/// Publishes every saved change, oldest first, then reports what is left.
pub(crate) async fn publish_all(
    client: StorageClient,
    root: PathBuf,
    now: u64,
) -> (Result<(), String>, Option<PublicationState>) {
    let result = async {
        while ask(client.publication_state()).await?.outbox > 0 {
            if ask(client.publish_to(root.clone(), now)).await?.is_none() {
                break;
            }
        }
        Ok(())
    }
    .await;
    let state = ask(client.publication_state()).await.ok();
    (result, state)
}

impl LiveSync {
    /// Starts syncing the notebook in `root`: imports what other writers published, publishes
    /// saves left from an earlier session, announces this writer, and watches the folder.
    pub fn new(
        client: StorageClient,
        root: PathBuf,
        identity: Identity,
        clock: Clock,
        watch: bool,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            client,
            root,
            identity,
            clock,
            timer: PublishTimer::default(),
            publish_at: None,
            publishing: false,
            publish_again: false,
            saves: 0,
            counted: 0,
            announced: false,
            pending: 0,
            status: StatusMemory::default(),
            nearby: false,
            computers: Vec::new(),
            imports: Imports::default(),
            problems: ProblemMemory::default(),
            retry: None,
            debounce: None,
            publish_error: None,
            import_error: None,
            warnings: Vec::new(),
            #[cfg(test)]
            hold: None,
            #[cfg(test)]
            hold_import: None,
            _tasks: Vec::new(),
        };
        this._tasks.push(this.listen_for_saves(cx));
        this._tasks.push(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(RESCAN_EVERY).await;
                if this
                    .update(cx, |this, cx| {
                        this.import(cx);
                        this.publish(cx);
                    })
                    .is_err()
                {
                    break;
                }
            }
        }));
        if watch {
            this._tasks.push(this.watch(cx));
        }
        this.import(cx);
        this
    }

    fn listen_for_saves(&self, cx: &mut Context<Self>) -> Task<()> {
        let request = self.client.watch_outbox();
        cx.spawn(async move |this, cx| {
            let mut saves = match ask(request).await {
                Ok(saves) => saves,
                Err(error) => {
                    this.update(cx, |this, cx| {
                        this.publish_error = Some((format!("Quire cannot follow saves to publish them: {error}. Saved changes stay on this computer; reopen the notebook to publish them.").into(), cx.background_executor().now()));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };
            while let Some(number) = saves.next().await {
                let heard = this.update(cx, |this, cx| {
                    this.saves = this.saves.max(number);
                    this.saved(cx)
                });
                if heard.is_err() {
                    break;
                }
            }
        })
    }

    fn watch(&self, cx: &mut Context<Self>) -> Task<()> {
        let root = self.root.clone();
        let fs = ::fs::RealFs::new(None, cx.background_executor().clone());
        cx.spawn(async move |this, cx| {
            let roots: Vec<PathBuf> = std::fs::canonicalize(&root)
                .into_iter()
                .chain([root.clone()])
                .collect();
            let (mut events, _watcher) = ::fs::Fs::watch(&*fs, &root, WATCH_DELAY).await;
            while let Some(batch) = events.next().await {
                let paths: Vec<PathBuf> = batch
                    .into_iter()
                    .flat_map(|event| match event.kind {
                        Some(::fs::PathEventKind::Rescan) => {
                            roots.iter().map(|root| root.join("devices")).collect()
                        }
                        _ => vec![event.path],
                    })
                    .collect();
                if this
                    .update(cx, |this, cx| this.changed(&roots, paths, cx))
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// The folder watcher saw `paths` change. Files other writers deliver start an import within
    /// [`WATCH_DELAY`]; temporary files, whose names start with a dot, and everything outside
    /// `devices/` are ignored.
    pub fn changed(
        &mut self,
        roots: &[PathBuf],
        paths: impl IntoIterator<Item = PathBuf>,
        cx: &mut Context<Self>,
    ) {
        let wake = paths
            .into_iter()
            .any(|path| roots.iter().any(|root| live_sync::delivered(root, &path)));
        if wake {
            self.debounce = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(WATCH_DELAY).await;
                this.update(cx, |this, cx| this.import(cx)).ok();
            }));
        }
    }

    /// A save landed: publish it once the quiet spell or the deadline passes.
    fn saved(&mut self, cx: &mut Context<Self>) {
        let delay = self.timer.saved((self.clock)(), self.nearby);
        self.publish_at = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(Duration::from_millis(delay))
                .await;
            this.update(cx, |this, cx| this.publish(cx)).ok();
        }));
        cx.notify();
    }

    /// Publishes every saved change now, then refreshes this writer's status. Concurrent calls
    /// share one run, which goes round again for saves that landed meanwhile.
    pub fn publish(&mut self, cx: &mut Context<Self>) {
        self.publish_at = None;
        self.timer.started();
        if self.publishing {
            self.publish_again = true;
            return;
        }
        self.publishing = true;
        self.publish_again = false;
        cx.notify();
        let (client, root) = (self.client.clone(), self.root.clone());
        #[cfg(test)]
        let hold = self.hold.clone();
        let now = (self.clock)();
        cx.spawn(async move |this, cx| {
            #[cfg(test)]
            if let Some(hold) = hold {
                hold.await.ok();
            }
            let (result, state) = publish_all(client, root.clone(), now).await;
            let status = this
                .update(cx, |this, cx| this.published(result, state, cx))
                .ok()
                .flatten();
            if let Some(status) = status {
                let written = cx
                    .background_spawn(async move {
                        live_sync::write_status(&root, &status)
                            .map(|()| status)
                            .map_err(|error| format!("{error:#}"))
                    })
                    .await;
                this.update(cx, |this, cx| match written {
                    Ok(status) => this.status.wrote(status.outbox, status.heartbeat),
                    Err(error) => {
                        this.publish_error = Some((error.into(), cx.background_executor().now()));
                        cx.notify();
                    }
                })
                .ok();
            }
            this.update(cx, |this, cx| {
                this.publishing = false;
                cx.notify();
                if std::mem::take(&mut this.publish_again) {
                    this.publish(cx);
                }
            })
            .ok();
        })
        .detach();
    }

    /// Records a publication's outcome and returns the status to write, if one is due.
    fn published(
        &mut self,
        result: Result<(), String>,
        state: Option<PublicationState>,
        cx: &mut Context<Self>,
    ) -> Option<DeviceStatus> {
        match result {
            Ok(()) => self.publish_error = None,
            Err(error) => {
                self.publish_error = Some((
                    format!("Saved changes could not be published to the synced folder: {error}. They are kept on this computer and Quire will try again.").into(),
                    cx.background_executor().now(),
                ));
            }
        }
        let state = state?;
        self.record(state);
        cx.notify();
        let now = (self.clock)();
        self.status.due(state.outbox, now).then(|| DeviceStatus {
            format: 1,
            app: self.identity.app.clone(),
            device: self.identity.device.clone(),
            label: self.identity.label.clone(),
            last_seq: state.last_seq,
            outbox: state.outbox,
            heartbeat: now,
        })
    }

    fn record(&mut self, state: PublicationState) {
        self.pending = state.outbox;
        self.counted = self.counted.max(state.saves);
    }

    /// Whether saved changes wait to be published, counted or only heard of.
    fn waiting(&self) -> bool {
        self.pending > 0 || self.saves > self.counted
    }

    /// Imports other writers' changes. A request while one runs makes it run again once it
    /// ends, since it may have listed the folder before the new file landed.
    pub fn import(&mut self, cx: &mut Context<Self>) {
        self.retry = None;
        if !self.imports.request() {
            return;
        }
        cx.notify();
        let (client, root) = (self.client.clone(), self.root.clone());
        cx.spawn(async move |this, cx| {
            loop {
                let report = rescan_all(client.clone(), root.clone()).await;
                #[cfg(test)]
                if let Some(hold) = this
                    .read_with(cx, |this, _| this.hold_import.clone())
                    .ok()
                    .flatten()
                {
                    hold.await.ok();
                }
                let folder = root.clone();
                let statuses = cx
                    .background_spawn(async move { live_sync::read_statuses(&folder) })
                    .await;
                let state = ask(client.publication_state()).await.ok();
                let again = this.update(cx, |this, cx| {
                    this.imported(report, statuses, state, cx);
                    this.imports.finished()
                });
                if !again.unwrap_or(false) {
                    return;
                }
            }
        })
        .detach();
    }

    fn imported(
        &mut self,
        report: Result<RescanReport, String>,
        statuses: Vec<DeviceStatus>,
        state: Option<PublicationState>,
        cx: &mut Context<Self>,
    ) {
        let now = (self.clock)();
        self.nearby =
            live_sync::nearby(&statuses, &self.identity.device, &self.identity.label, now);
        self.computers = statuses
            .into_iter()
            .filter(|status| status.device != self.identity.device)
            .collect();
        self.computers.sort_by(|a, b| a.label.cmp(&b.label));
        match report {
            Err(error) => {
                self.import_error = Some(format!("Changes from other computers could not be imported: {error}. Quire will try again.").into());
            }
            Ok(report) => {
                self.import_error = None;
                for warning in self.problems.survived(&report.problems) {
                    self.warn(warning);
                }
                for merged in report.merged.iter().filter(|merged| merged.overlaps > 0) {
                    let path = merged
                        .revision
                        .root()
                        .field("path")
                        .as_str()
                        .map(str::to_owned);
                    self.warn(format!("{} was edited in two places at once and some of the same lines changed both ways. Quire kept both versions of those lines next to each other; check the page and remove the one you don't want.", path.unwrap_or_else(|| "A page".into())));
                }
                if !report.problems.is_empty() {
                    self.retry = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(RETRY_AFTER).await;
                        this.update(cx, |this, cx| this.import(cx)).ok();
                    }));
                }
                let pages: Vec<String> = report
                    .pages
                    .iter()
                    .map(|page| &page.page)
                    .chain(report.merged.iter().map(|merged| &merged.page))
                    .filter_map(|page| page.as_str().map(str::to_owned))
                    .collect();
                if !pages.is_empty() {
                    cx.emit(Imported(pages));
                }
            }
        }
        if let Some(state) = state {
            self.record(state);
        }
        if !std::mem::replace(&mut self.announced, true) {
            // Saves left from the last session go out, and other apps on this computer see
            // this one is open and publish at once.
            self.publish(cx);
        } else if self.waiting() && self.publish_at.is_none() && !self.publishing {
            self.saved(cx);
        }
        cx.notify();
    }

    fn warn(&mut self, warning: String) {
        let warning = SharedString::from(warning);
        if !self.warnings.contains(&warning) {
            self.warnings.push(warning);
        }
    }

    /// A close or quit began at `since`: publish what is saved now rather than on schedule. A
    /// failure from before this close is retried once; one during it stays, so the close asks.
    pub fn prepare_close(&mut self, since: Instant, cx: &mut Context<Self>) {
        let stale = self
            .publish_error
            .as_ref()
            .is_none_or(|(_, at)| *at < since);
        if !self.publishing && self.waiting() && stale {
            self.publish_error = None;
            self.publish(cx);
        }
    }

    /// What a close waits for: publishing saved changes, or the reason they could not be.
    pub fn activity(&self) -> Activity {
        let waiting = self.waiting();
        match &self.publish_error {
            _ if self.publishing => Activity::Busy(PUBLISHING.into()),
            Some((error, _)) if waiting => Activity::Failed(
                format!("{error} Closing anyway keeps them saved on this computer; Quire publishes them the next time this notebook opens.").into(),
            ),
            None if waiting => Activity::Busy(PUBLISHING.into()),
            _ => Activity::Idle,
        }
    }

    #[cfg(test)]
    pub fn is_importing(&self) -> bool {
        self.imports.running()
    }

    #[cfg(test)]
    pub fn is_publishing(&self) -> bool {
        self.publishing
    }

    /// The status line, as the reference app's: whether saves have reached the synced folder,
    /// then when each other computer was last seen there.
    pub fn line(&self, time: &dyn Fn(i64) -> String) -> String {
        let mut line = if self.pending > 0 {
            format!(
                "Saved here, {} to publish",
                plural(self.pending, "change", "changes")
            )
        } else if self.waiting() || self.publishing {
            "Saved here, publishing".into()
        } else {
            "Published".into()
        };
        for computer in &self.computers {
            line.push_str(&format!(
                " · {}: seen {}",
                computer.label,
                time(computer.heartbeat as i64)
            ));
            if computer.outbox > 0 {
                line.push_str(&format!(
                    ", {} not yet published",
                    plural(computer.outbox, "change", "changes")
                ));
            }
        }
        line
    }

    /// What the line means and anything that went wrong, one sentence each.
    pub fn details(&self) -> Vec<SharedString> {
        let summary: SharedString = if self.waiting() {
            "Saved on this computer. Quire publishes to the synced folder within a few seconds."
                .into()
        } else {
            "Every saved change is in the synced folder.".into()
        };
        std::iter::once(summary)
            .chain(self.publish_error.iter().map(|(error, _)| error.clone()))
            .chain(self.import_error.clone())
            .chain(self.warnings.iter().cloned())
            .collect()
    }
}

fn plural(count: u64, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

/// Shows the status line with what it means; times use the window's clock label.
pub struct LiveSyncView {
    pub sync: gpui::Entity<LiveSync>,
    pub time: Rc<dyn Fn(i64) -> String>,
}

impl LiveSyncView {
    pub fn new(
        sync: gpui::Entity<LiveSync>,
        time: Rc<dyn Fn(i64) -> String>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&sync, |_, _, cx| cx.notify()).detach();
        Self { sync, time }
    }
}

impl Render for LiveSyncView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sync = self.sync.read(cx);
        gpui::div()
            .px_2()
            .flex()
            .flex_col()
            .child(Label::new(sync.line(&*self.time)))
            .children(sync.details().into_iter().skip(1).map(Label::new))
    }
}

/// Starts live sync for a Quire notebook with a known synced folder.
pub fn start(
    client: StorageClient,
    root: PathBuf,
    device: String,
    cx: &mut App,
) -> gpui::Entity<LiveSync> {
    cx.new(|cx| {
        LiveSync::new(
            client,
            root,
            Identity::this_computer(device),
            system_clock(),
            true,
            cx,
        )
    })
}

#[cfg(test)]
mod tests;
