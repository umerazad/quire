//! All-or-nothing writes to several pages, open or closed: bulk tag edits, task updates, renames,
//! and deletions. Each
//! page's text is read from its live buffer when open, unsaved typing included, or from its
//! stored head. Applying refuses when any open page changed after it was read; otherwise open
//! pages save first and hold further saves, every page is written in one checked save, and open
//! pages then take in their new version.

use crate::{
    notebook_backend::NotebookBackend, notebook_session::OpenPage,
    page_persistence::PagePersistence, query_index::ask,
};
use futures::{StreamExt as _, channel::mpsc};
use gpui::{App, AsyncApp, Entity, Task, WeakEntity};
use quire_store::notebook::{CheckedSaves, Edit};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

/// An open page's state when it was read; any change since makes a write stale.
struct Observed {
    persistence: WeakEntity<PagePersistence>,
    snapshot: language::BufferSnapshot,
}

/// One page's text as read: an open page's live buffer, or the stored head of a closed page.
pub struct PageSource {
    pub page: String,
    pub path: String,
    /// The stored form of the text, with the page's own line breaks.
    pub stored: String,
    base: Vec<String>,
    open: Option<Observed>,
}

impl PageSource {
    /// Whether the page had competing versions when it was read.
    pub fn competing(&self) -> bool {
        self.base.len() > 1
    }

    /// The write that replaces this page's text with `text`.
    pub fn write(self, text: Arc<str>) -> PageWrite {
        let path = self.path.clone();
        self.write_at(path, text)
    }

    /// The write that moves this page to `path` with `text`, keeping its identity and history.
    pub fn write_at(self, path: String, text: Arc<str>) -> PageWrite {
        PageWrite {
            page: self.page,
            path,
            base: self.base,
            text,
            open: self.open,
        }
    }
}

pub struct PageWrite {
    pub page: String,
    pub path: String,
    pub text: Arc<str>,
    base: Vec<String>,
    open: Option<Observed>,
}

fn open_source(page: &Entity<OpenPage>, cx: &App) -> Option<Result<PageSource, String>> {
    let open = page.read(cx);
    let persistence = open.persistence()?;
    let state = persistence.read(cx);
    let snapshot = state.buffer().read(cx).snapshot();
    let stored = state.source().render(&snapshot);
    Some(stored.map(|stored| PageSource {
        page: open.id().to_owned(),
        path: state.path().to_owned(),
        base: state.base().to_vec(),
        stored: stored.to_string(),
        open: Some(Observed {
            persistence: persistence.downgrade(),
            snapshot,
        }),
    }))
}

async fn stored_source(
    storage: &NotebookBackend,
    page: String,
) -> Result<Option<PageSource>, String> {
    let checkout = ask(storage.checkout(page.clone(), None)).await?;
    let Some(head) = checkout.selected else {
        return Ok(None);
    };
    let field = |name: &str| head.root().field(name).as_str().map(str::to_owned);
    if head.root().field("deleted").as_bool() == Some(true) {
        return Ok(None);
    }
    let (Some(path), Some(stored)) = (field("path"), field("text")) else {
        return Err(format!(
            "Page {page} has an unreadable stored version. Open it, save it again, and retry."
        ));
    };
    Ok(Some(PageSource {
        page,
        path,
        base: checkout.heads,
        stored,
        open: None,
    }))
}

/// Reads `page` from its open buffer when one of `open_pages` holds it, else from storage.
/// None when the page is deleted or missing.
pub async fn read(
    page: String,
    open_pages: &[Entity<OpenPage>],
    storage: &NotebookBackend,
    cx: &mut AsyncApp,
) -> Result<Option<PageSource>, String> {
    let open = cx.update(|cx| {
        open_pages
            .iter()
            .find(|open| open.read(cx).id() == page)
            .and_then(|open| open_source(open, cx))
    });
    match open {
        Some(source) => source.map(Some),
        None => stored_source(storage, page).await,
    }
}

/// Whether every open page still shows the text it was read with.
fn unchanged(writes: &[PageWrite], cx: &App) -> bool {
    writes.iter().all(|write| {
        write.open.as_ref().is_none_or(|open| {
            open.persistence.upgrade().is_some_and(|persistence| {
                persistence.read(cx).buffer().read(cx).version() == *open.snapshot.version()
            })
        })
    })
}

/// Resolves once no open page has a save or merge in flight.
async fn idle(open: &[Entity<PagePersistence>], cx: &mut AsyncApp) {
    loop {
        let (notify, mut notified) = mpsc::unbounded();
        let watching = cx.update(|cx| {
            if open
                .iter()
                .all(|persistence| persistence.read(cx).is_idle())
            {
                return None;
            }
            Some(
                open.iter()
                    .map(|persistence| {
                        let notify = notify.clone();
                        cx.observe(persistence, move |_, _| {
                            notify.unbounded_send(()).ok();
                        })
                    })
                    .collect::<Vec<_>>(),
            )
        });
        if watching.is_none() {
            return;
        }
        notified.next().await;
    }
}

/// Writes every page or none, refusing with `stale` when an open page changed after it was read
/// or another writer saved a page meanwhile. Open pages take in their new text and path. Returns
/// how many pages were written.
pub fn apply(
    writes: Vec<PageWrite>,
    storage: NotebookBackend,
    stale: &'static str,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    write(writes, None, storage, stale, cx)
}

/// Deletes every page or none, as `apply` writes them: tombstones in a Quire notebook, the system
/// Trash in a folder notebook, where `folder` goes whole. With `folder`, also refuses when a page
/// appeared inside it after the pages were chosen. Open pages stop saving, so nothing typed later
/// can revive them; their text stays in the buffer.
pub fn delete(
    writes: Vec<PageWrite>,
    folder: Option<String>,
    storage: NotebookBackend,
    stale: &'static str,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    write(writes, Some(folder), storage, stale, cx)
}

fn write(
    writes: Vec<PageWrite>,
    deletion: Option<Option<String>>,
    storage: NotebookBackend,
    stale: &'static str,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    if !unchanged(&writes, cx) {
        return Task::ready(Err(stale.into()));
    }
    let open: Vec<_> = writes
        .iter()
        .filter_map(|write| write.open.as_ref()?.persistence.upgrade())
        .collect();
    // Saving each open page first makes its saved text equal its buffer, so taking in the new
    // version afterwards has nothing left to merge.
    for persistence in &open {
        persistence.update(cx, |persistence, cx| {
            persistence.save(cx);
            persistence.hold();
        });
    }
    cx.spawn(async move |cx| {
        idle(&open, cx).await;
        let saved = match cx.update(|cx| edits(&writes, deletion.is_some(), cx)) {
            Some(edits) => {
                let answer = match (&deletion, &storage) {
                    (None, _) => ask(storage.save_many_if_heads(edits)).await,
                    (Some(folder), NotebookBackend::Store(client)) => {
                        ask(client.delete_if_heads(edits, folder.clone())).await
                    }
                    (Some(folder), NotebookBackend::Folder(client)) => {
                        ask(client.delete_if_heads(edits, folder.clone())).await
                    }
                };
                match answer {
                    Ok(CheckedSaves::Saved(acks)) => Ok(acks),
                    Ok(CheckedSaves::HeadsChanged { .. }) => Err(stale.to_owned()),
                    Ok(CheckedSaves::Appeared { path }) => Err(appeared(&path, &deletion)),
                    Err(error) => Err(error),
                }
            }
            None => Err(stale.to_owned()),
        };
        cx.update(|cx| {
            let written = saved.is_ok();
            for (write, ack) in writes.iter().zip(saved.iter().flatten()) {
                let Some(persistence) = write
                    .open
                    .as_ref()
                    .and_then(|open| open.persistence.upgrade())
                else {
                    continue;
                };
                persistence.update(cx, |persistence, cx| {
                    if deletion.is_some() {
                        persistence.retire(cx);
                    } else {
                        persistence.relocate(write.path.clone(), cx);
                        persistence.absorb(ack.revision_id().to_owned(), write.text.clone(), cx)
                    }
                });
            }
            if !(written && deletion.is_some()) {
                for persistence in &open {
                    persistence.update(cx, |persistence, cx| persistence.release(cx));
                }
            }
        });
        saved.map(|acks| acks.len())
    })
}

fn appeared(path: &str, deletion: &Option<Option<String>>) -> String {
    match deletion {
        Some(Some(folder)) => format!(
            "{path} appeared in {folder} after you chose to delete it. Nothing was deleted; try again."
        ),
        _ => format!(
            "{path} changed before the change could be applied. Nothing was changed; review the page and try again."
        ),
    }
}

/// The saves to make, with each open page's current base: its own save may have landed since
/// it was read without changing its text.
fn edits(writes: &[PageWrite], deleted: bool, cx: &App) -> Option<Vec<Edit>> {
    if !unchanged(writes, cx) {
        return None;
    }
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        });
    writes
        .iter()
        .map(|write| {
            let base = match &write.open {
                Some(open) => open.persistence.upgrade()?.read(cx).base().to_vec(),
                None => write.base.clone(),
            };
            Some(Edit {
                page: write.page.clone(),
                base,
                path: write.path.clone(),
                text: write.text.to_string(),
                deleted,
                created,
                recovery: None,
            })
        })
        .collect()
}
