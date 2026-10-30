//! Bulk tag rename and delete across the notebook: plan from each page's current text, preview,
//! then apply as one all-or-nothing save.

use crate::{notebook_session::OpenPage, page_persistence::PagePersistence, query_index::ask};
use futures::{StreamExt as _, channel::mpsc};
use gpui::{App, AsyncApp, Entity, Task, WeakEntity};
use quire_core::{
    source_text::SourceText,
    tag_edits::{Retag, TagChange, check_page_count},
};
use quire_store::{
    index::IndexClient,
    notebook::{CheckedSaves, Edit},
    worker::StorageClient,
};
use std::{
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

pub const STALE_PREVIEW: &str =
    "A page changed after the preview. Preview again before applying it.";

/// An open page's state when the plan read it; any change since makes the preview stale.
struct Observed {
    persistence: WeakEntity<PagePersistence>,
    snapshot: language::BufferSnapshot,
}

pub struct PlannedPage {
    pub page: String,
    pub path: String,
    pub changes: Vec<TagChange>,
    base: Vec<String>,
    text: Arc<str>,
    open: Option<Observed>,
}

pub struct TagEditPlan {
    pub pages: Vec<PlannedPage>,
}

/// One page's text to plan from: an open page's live buffer, unsaved typing included, or the
/// stored head of a closed page.
struct Source {
    page: String,
    path: String,
    base: Vec<String>,
    stored: String,
    open: Option<Observed>,
}

fn open_source(page: &Entity<OpenPage>, cx: &App) -> Option<Result<Source, String>> {
    let open = page.read(cx);
    let persistence = open.persistence()?;
    let state = persistence.read(cx);
    let snapshot = state.buffer().read(cx).snapshot();
    let stored = state.source().render(&snapshot);
    Some(stored.map(|stored| Source {
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

async fn stored_source(storage: &StorageClient, page: String) -> Result<Option<Source>, String> {
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
    Ok(Some(Source {
        page,
        path,
        base: checkout.heads,
        stored,
        open: None,
    }))
}

/// Plans the edit over every page whose tags it touches, refusing more than one apply may write.
pub fn plan(
    retag: Retag,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<TagEditPlan, String>> {
    cx.spawn(async move |cx| {
        let candidates = ask(index.tag_edit_pages(retag.clone())).await?;
        let mut pages = Vec::new();
        for candidate in candidates {
            let id = candidate.page.to_lossy_string();
            let open = cx.update(|cx| {
                open_pages
                    .iter()
                    .find(|page| page.read(cx).id() == id)
                    .and_then(|page| open_source(page, cx))
            });
            let source = match open {
                Some(source) => source?,
                None => match stored_source(&storage, id).await? {
                    Some(source) => source,
                    None => continue,
                },
            };
            let (original, normalized) = SourceText::prepare(source.stored);
            let work = cx.update(|cx| crate::page_index::edit_tags(normalized, retag.clone(), cx));
            let Some(edit) = work.await else {
                continue;
            };
            let text = original.apply(edit.edits).map_err(|error| {
                format!(
                    "Could not rewrite tags in {}: {error}. Preview again.",
                    source.path
                )
            })?;
            pages.push(PlannedPage {
                page: source.page,
                path: source.path,
                changes: edit.changes,
                base: source.base,
                text,
                open: source.open,
            });
        }
        check_page_count(pages.len())?;
        Ok(TagEditPlan { pages })
    })
}

/// Whether every open page still shows the text the preview read.
fn unchanged(plan: &TagEditPlan, cx: &App) -> bool {
    plan.pages.iter().all(|page| {
        page.open.as_ref().is_none_or(|open| {
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

/// Writes every planned page or none, refusing when any page changed after the preview. Open
/// pages save first and hold further saves meanwhile, then take in their new version.
pub fn apply(
    plan: TagEditPlan,
    storage: StorageClient,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    if !unchanged(&plan, cx) {
        return Task::ready(Err(STALE_PREVIEW.into()));
    }
    let open: Vec<_> = plan
        .pages
        .iter()
        .filter_map(|page| page.open.as_ref()?.persistence.upgrade())
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
        let saved = match cx.update(|cx| edits(&plan, cx)) {
            Some(edits) => match ask(storage.save_many_if_heads(edits)).await {
                Ok(CheckedSaves::Saved(acks)) => Ok(acks),
                Ok(CheckedSaves::HeadsChanged { .. }) => Err(STALE_PREVIEW.to_owned()),
                Err(error) => Err(error),
            },
            None => Err(STALE_PREVIEW.to_owned()),
        };
        cx.update(|cx| {
            if let Ok(acks) = &saved {
                for (page, ack) in plan.pages.iter().zip(acks) {
                    if let Some(persistence) = page
                        .open
                        .as_ref()
                        .and_then(|open| open.persistence.upgrade())
                    {
                        persistence.update(cx, |persistence, cx| {
                            persistence.absorb(ack.revision_id().to_owned(), page.text.clone(), cx)
                        });
                    }
                }
            }
            for persistence in &open {
                persistence.update(cx, |persistence, cx| persistence.release(cx));
            }
        });
        saved.map(|acks| acks.len())
    })
}

/// The saves to make, with each open page's current base: its own save may have landed since
/// the preview without changing its text.
fn edits(plan: &TagEditPlan, cx: &App) -> Option<Vec<Edit>> {
    if !unchanged(plan, cx) {
        return None;
    }
    let created = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        });
    plan.pages
        .iter()
        .map(|page| {
            let base = match &page.open {
                Some(open) => open.persistence.upgrade()?.read(cx).base().to_vec(),
                None => page.base.clone(),
            };
            Some(Edit {
                page: page.page.clone(),
                base,
                path: page.path.clone(),
                text: page.text.to_string(),
                deleted: false,
                created,
                recovery: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests;
