//! Moving a block with its children to another page, as one all-or-nothing change: the block
//! leaves its page, lands at the end of the destination, and every `Page#^id` reference to a
//! moved block follows it. `((id))` references need no change because block IDs are unique in
//! the notebook. Pages are read from their live buffers when open and written through
//! `page_writes`, so a failure leaves every page as it was.

use crate::{
    markdown_blocks::Block,
    notebook_backend::NotebookBackend,
    notebook_session::OpenPage,
    page_writes::{self, PageSource, PageWrite},
    query_index::ask,
};
use editor::Editor;
use gpui::{App, AppContext as _, Entity, Global, Task, WeakEntity, actions};
use multi_buffer::ToOffset as _;
use quire_core::{
    markdown::{Reference, page_name},
    source_text::{SourceEdit, SourceText},
};
use quire_store::index::IndexClient;
use std::collections::{BTreeSet, HashMap};

actions!(
    quire_block_move,
    [
        /// Moves the list item at the caret, with its children, to a page chosen next.
        MoveBlockToPage
    ]
);

pub const NOT_IN_BLOCK: &str = "Place the caret in a list item, then move it to a page.";
pub const READ_ONLY: &str =
    "This page is read-only, so its blocks cannot move. Reopen the page, then try again.";

/// The block a page editor asked to move, for the window that picks the destination.
pub struct Request {
    pub buffer: WeakEntity<language::Buffer>,
    pub id: String,
}

struct Pending(Option<Request>);
impl Global for Pending {}

/// Takes the block the last `MoveBlockToPage` named.
pub fn take_request(cx: &mut App) -> Option<Request> {
    cx.try_global::<Pending>()?;
    cx.global_mut::<Pending>().0.take()
}

/// Lets each editor name the list item at its caret, giving it an ID when it has none, and then
/// passes the action on to the window, which asks for the destination.
pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        editor
            .register_action(move |_: &MoveBlockToPage, _, cx| {
                if let Some(editor) = handle.upgrade() {
                    match editor.update(cx, request) {
                        Ok(request) => cx.set_global(Pending(Some(request))),
                        Err(message) => crate::outline::show_notice(message, cx),
                    }
                }
                cx.propagate();
            })
            .detach();
    })
    .detach();
}

fn request(editor: &mut Editor, cx: &mut gpui::Context<Editor>) -> Result<Request, &'static str> {
    let buffer = crate::outline::source_buffer(editor, cx).ok_or(NOT_IN_BLOCK)?;
    let multi = editor.buffer().read(cx).snapshot(cx);
    let head = editor.selections.newest_anchor().head().to_offset(&multi);
    let (_, head) = multi.point_to_buffer_offset(head).ok_or(NOT_IN_BLOCK)?;
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let block = crate::markdown_blocks::current_blocks(&snapshot)
        .ok_or(NOT_IN_BLOCK)?
        .into_iter()
        .rfind(|block| {
            block.kind == "list"
                && line_start(&text, block.from) <= head.0
                && head.0 <= block.content_to
        })
        .ok_or(NOT_IN_BLOCK)?;
    if editor.read_only(cx) {
        return Err(READ_ONLY);
    }
    let id = match block.id {
        Some(id) => id,
        None => {
            let id = crate::references::new_block_id();
            let at = block.content_to;
            let gap = if block.content_from == at { "" } else { " " };
            buffer.update(cx, |buffer, cx| {
                buffer.edit([(at..at, format!("{gap}^{id}"))], None, cx)
            });
            id
        }
    };
    Ok(Request {
        buffer: buffer.downgrade(),
        id,
    })
}

pub const SAME_PAGE: &str = "The block is already on that page. Choose a different destination.";
pub const MISSING_SOURCE: &str =
    "The block's page is no longer in the notebook. Reopen it, then move the block again.";
pub const MISSING_DESTINATION: &str =
    "The destination page is missing. Create it, then move the block again.";
pub const MISSING_BLOCK: &str =
    "The block is no longer on its page. Place the caret in it and move it again.";
pub const STALE_MOVE: &str =
    "A page changed while the block was moving. Nothing moved; move the block again.";
pub const NO_GRAMMAR: &str =
    "Markdown parsing is unavailable, so the block cannot move. Restart Quire and try again.";

/// The source and destination texts after the cut, and the IDs of every moved block.
#[derive(Debug, PartialEq, Eq)]
pub struct Cut {
    pub source: String,
    pub destination: String,
    pub ids: BTreeSet<String>,
}

fn line_start(text: &str, position: usize) -> usize {
    text[..position].rfind('\n').map_or(0, |index| index + 1)
}

fn indent(line: &str) -> usize {
    line.len() - line.trim_start().len()
}

/// Takes the block `id` and its children out of `text` and appends them, outdented to the left
/// margin, to `destination` after a blank line.
pub fn cut(text: &str, blocks: &[Block], id: &str, destination: &str) -> Result<Cut, &'static str> {
    let block = blocks
        .iter()
        .find(|block| block.id.as_deref() == Some(id))
        .ok_or(MISSING_BLOCK)?;
    let from = line_start(text, block.from);
    let to = block.subtree_to + usize::from(text[block.subtree_to..].starts_with('\n'));
    let lines = &text[from..block.subtree_to];
    let margin = indent(lines);
    let moved = lines
        .split('\n')
        .map(|line| &line[margin.min(indent(line))..])
        .collect::<Vec<_>>()
        .join("\n");
    let separator = if destination.is_empty() || destination.ends_with("\n\n") {
        ""
    } else if destination.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let ids = blocks
        .iter()
        .filter(|item| item.from >= block.from && item.subtree_to <= block.subtree_to)
        .filter_map(|item| item.id.clone())
        .collect();
    Ok(Cut {
        source: format!("{}{}", &text[..from], &text[to..]),
        destination: format!("{destination}{separator}{moved}\n"),
        ids,
    })
}

/// The edits that point `Page#^id` references to moved blocks at `destination`, where `Page`
/// resolves to the source page's path.
pub fn retarget(
    text: &str,
    references: &[Reference],
    ids: &BTreeSet<String>,
    destination: &str,
    is_source: impl Fn(&str) -> bool,
) -> Vec<SourceEdit<String>> {
    references
        .iter()
        .filter_map(|reference| {
            let (base, id) = reference.target.split_once("#^")?;
            if !ids.contains(id) || !is_source(base) {
                return None;
            }
            let target = format!("{}#^{id}", page_name(destination));
            Some(SourceEdit {
                range: reference.range.clone(),
                replacement: text[reference.range.clone()].replacen(&reference.target, &target, 1),
            })
        })
        .collect()
}

/// A page read for the move, with its text in the editor's line breaks.
struct Read {
    source: PageSource,
    original: SourceText,
    text: String,
}

fn prepare(source: PageSource) -> Read {
    let (original, text) = SourceText::prepare(source.stored.clone());
    Read {
        source,
        original,
        text,
    }
}

/// What moving a block writes, and the page it goes to.
pub struct Plan {
    pub writes: Vec<PageWrite>,
    pub destination: String,
}

/// Plans moving block `id` of page `source` to the end of page `destination`.
pub fn plan(
    source: String,
    id: String,
    destination: String,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<Plan, String>> {
    cx.spawn(async move |cx| {
        if source == destination {
            return Err(SAME_PAGE.into());
        }
        let read = async |page: String, missing: &str, cx: &mut gpui::AsyncApp| {
            page_writes::read(page, &open_pages, &storage, cx)
                .await?
                .map(prepare)
                .ok_or_else(|| missing.to_owned())
        };
        let from = read(source.clone(), MISSING_SOURCE, cx).await?;
        let to = read(destination.clone(), MISSING_DESTINATION, cx).await?;
        let snapshot = cx.update(|cx| crate::markdown::detached_snapshot(&from.text, cx));
        let blocks = cx
            .background_spawn(
                async move { crate::markdown_blocks::current_blocks(&snapshot.await) },
            )
            .await
            .ok_or(NO_GRAMMAR)?;
        let cut = cut(&from.text, &blocks, &id, &to.text)?;
        let source_path = from.source.path.clone();
        let destination_path = to.source.path.clone();
        let facts = ask(index.relocation(source.as_str().into()))
            .await?
            .ok_or(MISSING_SOURCE)?;
        let mut pages = vec![(from, Some(cut.source)), (to, Some(cut.destination))];
        for candidate in facts.candidates {
            let page = candidate.page.to_lossy_string();
            if page == source || page == destination {
                continue;
            }
            if let Some(found) = page_writes::read(page, &open_pages, &storage, cx).await? {
                pages.push((prepare(found), None));
            }
        }
        let mut analyzed = Vec::new();
        for (read, planned) in pages {
            let text = planned.clone().unwrap_or_else(|| read.text.clone());
            let work = cx.update(|cx| crate::page_index::relocation_links(text.clone(), cx));
            let (references, _) = cx.background_spawn(work).await.ok_or(NO_GRAMMAR)?;
            analyzed.push((read, planned, text, references));
        }
        let bases: Vec<String> = analyzed
            .iter()
            .flat_map(|(.., references)| references.iter())
            .filter_map(|reference| Some(reference.target.split_once("#^")?.0.to_owned()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let (_, resolved) = ask(index.with_generation(move |index| {
            bases
                .into_iter()
                .map(|base| Ok((base.clone(), index.find_page(&base)?.map(|page| page.path))))
                .collect::<Result<HashMap<_, _>, _>>()
        }))
        .await?;
        let is_source = |base: &str| {
            resolved.get(base).cloned().flatten().as_deref() == Some(source_path.as_str())
        };
        let mut writes = Vec::new();
        for (read, planned, text, references) in analyzed {
            let followed = retarget(&text, &references, &cut.ids, &destination_path, is_source);
            if planned.is_none() && followed.is_empty() {
                continue;
            }
            let rewritten = apply(&text, followed);
            let written = read
                .original
                .apply([SourceEdit {
                    range: 0..read.text.len(),
                    replacement: rewritten,
                }])
                .map_err(|error| {
                    format!(
                        "Could not rewrite {} for the move: {error}. Move the block again.",
                        read.source.path
                    )
                })?;
            writes.push(read.source.write(written));
        }
        Ok(Plan {
            writes,
            destination,
        })
    })
}

fn apply(text: &str, mut edits: Vec<SourceEdit<String>>) -> String {
    edits.sort_by_key(|edit| std::cmp::Reverse(edit.range.start));
    edits.into_iter().fold(text.to_owned(), |mut text, edit| {
        text.replace_range(edit.range, &edit.replacement);
        text
    })
}

/// Writes every page of the move or none.
pub fn write(plan: Plan, storage: NotebookBackend, cx: &mut App) -> Task<Result<usize, String>> {
    page_writes::apply(plan.writes, storage, STALE_MOVE, cx)
}

#[cfg(test)]
#[path = "block_move_tests.rs"]
mod tests;
