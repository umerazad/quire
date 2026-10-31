//! The nine read tools, ported from the reference `notebook-tools.ts` handlers. Lists come from
//! the query index, which holds the unsaved text of open pages; page text comes from an open
//! page's buffer or the stored head through `page_writes::read`. Result fields, order, summaries,
//! and messages follow the reference.

use super::live::LiveNotebook;
use crate::{page_index, page_writes, query_index::ask};
use gpui::{AppContext as _, AsyncApp};
use quire_core::ai::tools::{
    TEXT_CAP, block_key, compact, hashed, line_at, outside_pages, plural, take,
};
use quire_store::index::{IndexedBlock, PageIndex, PageRef, QueryIndex, TaskItem};
use serde_json::{Value, json};
use std::collections::HashMap;

/// Why a tool call failed: a message the model can act on, or an unexpected failure.
#[derive(Debug, PartialEq)]
pub enum Failure {
    Tool(String),
    Internal(String),
}

impl From<String> for Failure {
    fn from(error: String) -> Self {
        Self::Internal(error)
    }
}

pub type Outcome = Result<Value, Failure>;
type Answer<T> = anyhow::Result<Result<T, String>>;

const NOT_READY: &str =
    "A page could not be read as Markdown. Rebuild Quire with its bundled grammars.";

pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest prefix of `text` within `units` UTF-16 units.
pub fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, character) in text.char_indices() {
        used += character.len_utf16();
        if used > units {
            return &text[..at];
        }
    }
    text
}

/// The byte offset of a UTF-16 offset in `text`.
pub fn byte_at(text: &str, units: u32) -> usize {
    let mut used = 0;
    for (at, character) in text.char_indices() {
        if used >= units as usize {
            return at;
        }
        used += character.len_utf16();
    }
    text.len()
}

fn text_of(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).map(str::to_owned)
}

fn number_of(input: &Value, name: &str) -> Option<u64> {
    input
        .get(name)
        .and_then(Value::as_f64)
        .map(|value| value as u64)
}

/// Runs a query on the index thread.
pub async fn query<T: Send + 'static>(
    notebook: &LiveNotebook,
    run: impl FnOnce(&QueryIndex) -> Answer<T> + Send + 'static,
) -> Result<T, Failure> {
    let (_, answer) = ask(notebook.index.with_generation(run)).await?;
    answer.map_err(Failure::Tool)
}

/// A page's text as the editor reads it, unsaved typing included; None when it is gone.
pub async fn page_text(
    notebook: &LiveNotebook,
    page: &PageRef,
    cx: &mut AsyncApp,
) -> Result<Option<String>, Failure> {
    let source = page_writes::read(
        page.page.to_lossy_string(),
        &notebook.open_pages,
        &notebook.storage,
        cx,
    )
    .await?;
    Ok(source.map(|source| page_index::editor_text(&source.stored.as_str().into())))
}

pub async fn analyze(text: &str, cx: &mut AsyncApp) -> Result<PageIndex, Failure> {
    let work = cx.update(|cx| page_index::analyze(text.to_owned(), cx));
    cx.background_spawn(work)
        .await
        .ok_or_else(|| Failure::Internal(NOT_READY.into()))
}

/// The reference `find`: "today", a path, or a title; refuses targets outside the pages.
pub fn find(index: &QueryIndex, target: &str, today: &str) -> Answer<Option<PageRef>> {
    let value = target.trim();
    if value.eq_ignore_ascii_case("today") {
        return Ok(Ok(index.daily_page(today)?));
    }
    if let Some(error) = outside_pages(value) {
        return Ok(Err(error));
    }
    Ok(Ok(index.find_page(value)?))
}

/// The reference `require`: `find`, or the message that names the fix.
pub fn require(index: &QueryIndex, target: &str, today: &str) -> Answer<PageRef> {
    Ok(match find(index, target, today)? {
        Ok(Some(page)) => Ok(page),
        Ok(None) if target.trim().eq_ignore_ascii_case("today") => Err(format!(
            "Today's daily page ({today}) does not exist yet. Use append_block with page \"today\" to start it."
        )),
        Ok(None) => Err(format!(
            "No page named \"{target}\". Use list_pages or search_notebook to find its title."
        )),
        Err(error) => Err(error),
    })
}

/// The block containing a UTF-16 position, as the reference `blockAt` finds it.
pub fn block_at(blocks: &[IndexedBlock], position: u32) -> Option<&IndexedBlock> {
    let index = blocks.partition_point(|block| block.from <= position);
    let block = blocks.get(index.checked_sub(1)?)?;
    (position <= block.to).then_some(block)
}

fn day(value: Option<String>, today: &str) -> Option<String> {
    value.map(|value| {
        if value == "today" {
            today.to_owned()
        } else {
            value
        }
    })
}

fn today(notebook: &LiveNotebook) -> Result<String, Failure> {
    notebook.today.clone().map_err(Failure::Internal)
}

pub async fn run(name: &str, input: &Value, notebook: &LiveNotebook, cx: &mut AsyncApp) -> Outcome {
    match name {
        "search_notebook" => search_notebook(input, notebook).await,
        "list_pages" => list_pages(input, notebook).await,
        "read_page" => read_page(input, notebook, cx).await,
        "read_block" => read_block(input, notebook, cx).await,
        "linked_references" => linked_references(input, notebook).await,
        "tagged_blocks" => tagged_blocks(input, notebook).await,
        "tasks" => tasks(input, notebook).await,
        "daily_pages" => daily_pages(input, notebook, cx).await,
        "mentions" => mentions(input, notebook).await,
        other => Err(Failure::Internal(format!("{other} is not a read tool"))),
    }
    .map(compact)
}

async fn search_notebook(input: &Value, notebook: &LiveNotebook) -> Outcome {
    let query_text = text_of(input, "query").unwrap_or_default();
    let limit = number_of(input, "limit");
    let wanted = query_text.to_lowercase();
    let searched = query_text.clone();
    let hits = query(notebook, move |index| {
        let live = index
            .live_blocks()?
            .into_iter()
            .filter(|(_, block)| block.text.to_lowercase().contains(&wanted))
            .map(|(page, block)| (page, block.text, block.id));
        let indexed = index
            .search(&searched)?
            .into_iter()
            .map(|hit| (hit.source, hit.snippet, hit.block_id));
        let mut seen = std::collections::HashSet::new();
        let hits: Vec<_> = live
            .chain(indexed)
            .filter(|(page, snippet, id)| {
                seen.insert(format!(
                    "{}\0{}",
                    page.path,
                    id.as_deref().unwrap_or(snippet)
                ))
            })
            .map(|(page, snippet, id)| {
                json!({
                    "page": page.title,
                    "path": page.path,
                    "block_id": id,
                    "daily": index.date(&page.title),
                    "snippet": utf16_prefix(&snippet, 400),
                })
            })
            .collect();
        Ok(Ok(hits))
    })
    .await?;
    let total = hits.len();
    let (results, truncated) = take(hits, limit);
    Ok(json!({
        "summary": format!("Searched {query_text} · {}", plural(total, "block")),
        "results": results,
        "truncated": truncated,
    }))
}

struct Row {
    title: String,
    path: String,
    page: PageRef,
    daily: Option<String>,
    modified: i64,
}

async fn list_pages(input: &Value, notebook: &LiveNotebook) -> Outcome {
    let today = today(notebook)?;
    let after = day(text_of(input, "modified_after"), &today);
    let from = day(text_of(input, "daily_from"), &today);
    let to = day(text_of(input, "daily_to"), &today);
    let namespace = text_of(input, "namespace").map(|value| value.to_lowercase());
    let contains = text_of(input, "contains").map(|value| value.to_lowercase());
    let prefix = text_of(input, "prefix").map(|value| value.to_lowercase());
    let sort = text_of(input, "sort").unwrap_or_else(|| "title".into());
    let limit = number_of(input, "limit");
    let reader = notebook.clone();
    let (rows, total) = query(notebook, move |index| {
        let modified = index.modified_times()?;
        let mut task_counts: HashMap<String, usize> = HashMap::new();
        for task in index.tasks()? {
            *task_counts
                .entry(task.source.page.to_lossy_string())
                .or_default() += 1;
        }
        let mut rows: Vec<Row> = index
            .pages()?
            .into_iter()
            .map(|listed| Row {
                title: listed.page.title.clone(),
                path: listed.page.path.clone(),
                modified: modified
                    .get(&listed.page.page.to_lossy_string())
                    .copied()
                    .unwrap_or(0),
                daily: listed.date,
                page: listed.page,
            })
            .filter(|row| {
                let changed = reader.day_of(row.modified);
                let folder = row.path.rsplit_once('/').map_or("", |(folder, _)| folder);
                namespace.as_ref().is_none_or(|namespace| {
                    [quire_core::workspace::namespace_of(&row.title), folder]
                        .iter()
                        .any(|candidate| candidate.to_lowercase() == *namespace)
                }) && contains
                    .as_ref()
                    .is_none_or(|contains| row.title.to_lowercase().contains(contains))
                    && prefix
                        .as_ref()
                        .is_none_or(|prefix| row.title.to_lowercase().starts_with(prefix))
                    && from
                        .as_ref()
                        .is_none_or(|from| row.daily.as_ref().is_some_and(|daily| daily >= from))
                    && to
                        .as_ref()
                        .is_none_or(|to| row.daily.as_ref().is_some_and(|daily| daily <= to))
                    && after
                        .as_ref()
                        .is_none_or(|after| changed.is_some_and(|changed| changed >= *after))
            })
            .collect();
        match sort.as_str() {
            "date" => rows.sort_by(|a, b| b.daily.cmp(&a.daily)),
            "modified" => rows.sort_by_key(|row| std::cmp::Reverse(row.modified)),
            _ => {}
        }
        let total = rows.len();
        let (rows, truncated) = take(rows, limit);
        let pages = rows
            .into_iter()
            .map(|row| {
                Ok(json!({
                    "title": row.title,
                    "path": row.path,
                    "daily": row.daily,
                    "modified": reader.day_of(row.modified),
                    "tasks": task_counts.get(&row.page.page.to_lossy_string()).copied().unwrap_or(0),
                    "references": index.backlinks(&row.page.page)?.len(),
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Ok(((pages, truncated), total)))
    })
    .await?;
    let (pages, truncated) = rows;
    Ok(json!({
        "summary": format!("Listed {}", plural(total, "page")),
        "pages": pages,
        "truncated": truncated,
    }))
}

/// A page found for a read: its reference, daily date, incoming reference count, and change time.
struct Found {
    page: PageRef,
    daily: Option<String>,
    references: usize,
    modified: i64,
}

async fn found(notebook: &LiveNotebook, target: String) -> Result<Found, Failure> {
    let today = today(notebook)?;
    query(notebook, move |index| {
        let page = match require(index, &target, &today)? {
            Ok(page) => page,
            Err(error) => return Ok(Err(error)),
        };
        let modified = index
            .modified_times()?
            .get(&page.page.to_lossy_string())
            .copied()
            .unwrap_or(0);
        Ok(Ok(Found {
            daily: index.date(&page.title),
            references: index.backlinks(&page.page)?.len(),
            modified,
            page,
        }))
    })
    .await
}

fn gone(page: &PageRef) -> Failure {
    Failure::Tool(format!(
        "No page named \"{}\". Use list_pages or search_notebook to find its title.",
        page.title
    ))
}

async fn read_page(input: &Value, notebook: &LiveNotebook, cx: &mut AsyncApp) -> Outcome {
    let found = found(notebook, text_of(input, "page").unwrap_or_default()).await?;
    let source = page_text(notebook, &found.page, cx)
        .await?
        .ok_or_else(|| gone(&found.page))?;
    let analysis = analyze(&source, cx).await?;
    let lines: Vec<&str> = source.split('\n').collect();
    let first = number_of(input, "from_line").unwrap_or(1).max(1) as usize;
    let mut last = number_of(input, "to_line")
        .map_or(lines.len(), |to| to as usize)
        .min(lines.len());
    let mut text = lines
        .get(first - 1..last.max(first - 1))
        .map(|window| window.join("\n"))
        .unwrap_or_default();
    let mut truncated = None;
    if utf16_len(&text) > TEXT_CAP {
        let head = utf16_prefix(&text, TEXT_CAP);
        let cut = head
            .rfind('\n')
            .filter(|cut| *cut > 0)
            .unwrap_or(head.len());
        text.truncate(cut);
        last = first + text.split('\n').count() - 1;
        truncated = Some(format!(
            "Cut at line {last} of {}. Call read_page again with from_line {}.",
            lines.len(),
            last + 1
        ));
    }
    let blocks: Vec<Value> = analysis
        .blocks
        .iter()
        .map(|block| {
            let line = line_at(&source, block.content_from as usize);
            (line, block_key(line, &block.text))
        })
        .filter(|(line, _)| *line >= first && *line <= last)
        .take(1000)
        .map(|(line, key)| json!({ "line": line, "key": key }))
        .collect();
    let mut tags: Vec<&str> = Vec::new();
    for tag in &analysis.tagged {
        if !tags.contains(&tag.tag.as_str()) {
            tags.push(&tag.tag);
        }
    }
    let title = &found.page.title;
    Ok(json!({
        "summary": format!(
            "Read [[{title}]]{}",
            found.daily.as_ref().map(|daily| format!(" {daily}")).unwrap_or_default()
        ),
        "title": title,
        "path": found.page.path,
        "daily": found.daily,
        "modified": notebook.day_of(found.modified),
        "tags": tags,
        "tasks": analysis.tasks.len(),
        "references": found.references,
        "lines": { "from": first, "to": last, "total": lines.len() },
        "text": text,
        "blocks": blocks,
        "truncated": truncated,
    }))
}

/// The page holding the block `id` names, or the reference `resolve` message.
pub async fn resolve(
    notebook: &LiveNotebook,
    target: String,
) -> Result<(PageRef, String), Failure> {
    query(notebook, move |index| {
        let id = target
            .find("#^")
            .map_or(target.as_str(), |at| &target[at + 2..])
            .to_owned();
        Ok(match index.resolve_block(&target)? {
            Some(page) => Ok((page, id)),
            None => {
                let uses = index
                    .live_blocks()?
                    .iter()
                    .filter(|(_, block)| block.id.as_deref() == Some(id.as_str()))
                    .count();
                Err(if uses > 1 {
                    format!(
                        "Multiple blocks use {id}. Give the copied block a new identifier in source view."
                    )
                } else {
                    format!("Block {id} is missing. Restore its source or update this reference.")
                })
            }
        })
    })
    .await
}

async fn read_block(input: &Value, notebook: &LiveNotebook, cx: &mut AsyncApp) -> Outcome {
    let target = text_of(input, "id").unwrap_or_default();
    let target = target.strip_prefix('^').unwrap_or(&target).to_owned();
    let (page, id) = resolve(notebook, target).await?;
    let text = page_text(notebook, &page, cx)
        .await?
        .ok_or_else(|| gone(&page))?;
    let analysis = analyze(&text, cx).await?;
    let block = analysis
        .blocks
        .iter()
        .find(|block| block.id.as_deref() == Some(id.as_str()))
        .ok_or_else(|| {
            Failure::Tool(format!(
                "Block {id} is missing. Restore its source or update this reference."
            ))
        })?;
    let from = byte_at(&text, block.from);
    let line_start = text[..from].rfind('\n').map_or(0, |at| at + 1);
    let end = byte_at(&text, block.subtree_to);
    let mut ancestors = Vec::new();
    let mut parent = block.parent.clone();
    while let Some(key) = parent {
        let Some(found) = analysis.blocks.iter().find(|item| item.key == key) else {
            break;
        };
        ancestors.insert(0, found.text.clone());
        parent = found.parent.clone();
    }
    let daily = query(notebook, {
        let title = page.title.clone();
        move |index| Ok(Ok(index.date(&title)))
    })
    .await?;
    Ok(json!({
        "summary": format!("Read block ^{id} on [[{}]]", page.title),
        "page": page.title,
        "path": page.path,
        "daily": daily,
        "ancestors": ancestors,
        "text": text[line_start..end.max(line_start)].trim_end_matches('\n'),
    }))
}

fn newest_first(a: &Value, b: &Value, date: &str) -> std::cmp::Ordering {
    let key = |row: &Value, field: &str| row[field].as_str().unwrap_or("").to_owned();
    key(b, date)
        .cmp(&key(a, date))
        .then_with(|| key(a, "page").cmp(&key(b, "page")))
}

async fn linked_references(input: &Value, notebook: &LiveNotebook) -> Outcome {
    let today = today(notebook)?;
    let target = text_of(input, "page").unwrap_or_default();
    let (title, mut rows) = query(notebook, move |index| {
        let page = match require(index, &target, &today)? {
            Ok(page) => page,
            Err(error) => return Ok(Err(error)),
        };
        let mut blocks: HashMap<String, Vec<IndexedBlock>> = HashMap::new();
        let rows = index
            .backlinks(&page.page)?
            .into_iter()
            .map(|link| {
                let key = link.source.page.to_lossy_string();
                if !blocks.contains_key(&key) {
                    blocks.insert(key.clone(), index.page_blocks(&link.source.page)?);
                }
                let id = block_at(&blocks[&key], link.link.from).and_then(|block| block.id.clone());
                Ok(json!({
                    "page": link.source.title,
                    "path": link.source.path,
                    "daily": index.date(&link.source.title),
                    "block_id": id,
                    "text": link.link.text,
                }))
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Ok((page.title, rows)))
    })
    .await?;
    rows.sort_by(|a, b| newest_first(a, b, "daily"));
    let total = rows.len();
    let (references, truncated) = take(rows, number_of(input, "limit"));
    Ok(json!({
        "summary": format!("Linked references to [[{title}]] · {total}"),
        "references": references,
        "truncated": truncated,
    }))
}

fn task_open(task: &TaskItem) -> bool {
    task.state != quire_core::tasks::TaskState::Done
}

async fn tagged_blocks(input: &Value, notebook: &LiveNotebook) -> Outcome {
    let today = today(notebook)?;
    let tag = hashed(&text_of(input, "tag").unwrap_or_default());
    let from = day(text_of(input, "from"), &today);
    let to = day(text_of(input, "to"), &today);
    let status = text_of(input, "status").unwrap_or_else(|| "all".into());
    let reader = notebook.clone();
    let searched = tag.clone();
    let mut rows = query(notebook, move |index| {
        let modified = index.modified_times()?;
        let mut tasks: HashMap<String, Vec<TaskItem>> = HashMap::new();
        for task in index.tasks()? {
            tasks
                .entry(task.source.page.to_lossy_string())
                .or_default()
                .push(task);
        }
        let mut blocks: HashMap<String, Vec<IndexedBlock>> = HashMap::new();
        let mut rows = Vec::new();
        for found in index.tagged_blocks(&searched)? {
            let page = found.source.page.to_lossy_string();
            let task = match tasks.get(&page) {
                Some(on_page) => {
                    if !blocks.contains_key(&page) {
                        blocks.insert(page.clone(), index.page_blocks(&found.source.page)?);
                    }
                    blocks[&page]
                        .iter()
                        .find(|block| block.content_from == found.tag.from)
                        .and_then(|block| on_page.iter().find(|task| task.key == block.key))
                }
                None => None,
            };
            let date = index
                .date(&found.source.title)
                .or_else(|| reader.day_of(modified.get(&page).copied().unwrap_or(0)));
            let keep = from
                .as_ref()
                .is_none_or(|from| date.as_ref().is_some_and(|date| date >= from))
                && to
                    .as_ref()
                    .is_none_or(|to| date.as_ref().is_some_and(|date| date <= to))
                && match status.as_str() {
                    "done" => task.is_some_and(|task| !task_open(task)),
                    "open" => task.is_some_and(task_open),
                    "due" => task.is_some_and(|task| {
                        task_open(task) && task.due.as_ref().is_some_and(|due| *due <= today)
                    }),
                    _ => true,
                };
            if keep {
                rows.push(json!({
                    "page": found.source.title,
                    "path": found.source.path,
                    "date": date,
                    "block_id": found.tag.block_id,
                    "task": task.map(|task| task.state.as_str()),
                    "due": task.and_then(|task| task.due.clone()),
                    "text": found.tag.snippet,
                }));
            }
        }
        Ok(Ok(rows))
    })
    .await?;
    rows.sort_by(|a, b| newest_first(a, b, "date"));
    let total = rows.len();
    let (blocks, truncated) = take(rows, number_of(input, "limit"));
    Ok(json!({
        "summary": format!("Tagged {tag} · {}", plural(total, "block")),
        "blocks": blocks,
        "truncated": truncated,
    }))
}

/// A task as the tools list it.
pub fn task_row(task: &TaskItem) -> Value {
    json!({
        "key": task.key,
        "text": task.text,
        "state": task.state.as_str(),
        "due": task.due,
        "completed": task.completed,
        "tags": task.tags,
        "page": task.source.title,
        "path": task.source.path,
        "block_id": task.id,
    })
}

async fn tasks(input: &Value, notebook: &LiveNotebook) -> Outcome {
    let today = today(notebook)?;
    let due = text_of(input, "due");
    let page = text_of(input, "page");
    if due.as_deref() == Some("range") && (input.get("from").is_none() || input.get("to").is_none())
    {
        return Err(Failure::Tool(
            "due \"range\" needs from and to dates, such as from \"2026-11-01\" and to \"2026-11-07\"."
                .into(),
        ));
    }
    let (from, to) = if due.as_deref() == Some("week") {
        (Some(today.clone()), quire_core::tasks::add_days(&today, 6))
    } else {
        (
            day(text_of(input, "from"), &today),
            day(text_of(input, "to"), &today),
        )
    };
    let state = text_of(input, "state");
    let project = text_of(input, "project").map(|value| value.to_lowercase());
    let tag = text_of(input, "tag").map(|tag| hashed(&tag).to_lowercase());
    let words = text_of(input, "text").map(|value| value.to_lowercase());
    let (now, wanted, range) = (today.clone(), due.clone(), (from.clone(), to.clone()));
    let mut rows = query(notebook, move |index| {
        let path = match &page {
            Some(target) => match find(index, target, &now)? {
                Ok(Some(found)) => Some(found.page.to_lossy_string()),
                Ok(None) => {
                    return Ok(Err(format!(
                        "No page named \"{target}\". Use list_pages to find its title."
                    )));
                }
                Err(error) => return Ok(Err(error)),
            },
            None => None,
        };
        let rows: Vec<TaskItem> = index
            .tasks()?
            .into_iter()
            .filter(|task| {
                let state_ok = match state.as_deref() {
                    Some("open") => task_open(task),
                    Some(state) => task.state.as_str() == state,
                    None => true,
                };
                let due_ok = match wanted.as_deref() {
                    None => true,
                    Some(kind) => match (task_open(task), task.due.as_deref()) {
                        (true, Some(due)) => match kind {
                            "today" => due == now,
                            "overdue" => due < now.as_str(),
                            _ => {
                                let (from, to) = (&range.0, &range.1);
                                from.as_deref().is_some_and(|from| due >= from)
                                    && to.as_deref().is_some_and(|to| due <= to)
                            }
                        },
                        _ => false,
                    },
                };
                state_ok
                    && due_ok
                    && path
                        .as_ref()
                        .is_none_or(|path| task.source.page.to_lossy_string() == *path)
                    && project.as_ref().is_none_or(|project| {
                        task.source.title.to_lowercase() == *project
                            || task
                                .links
                                .iter()
                                .any(|link| link.to_lowercase() == *project)
                    })
                    && tag
                        .as_ref()
                        .is_none_or(|tag| task.tags.iter().any(|item| item.to_lowercase() == *tag))
                    && words
                        .as_ref()
                        .is_none_or(|words| task.text.to_lowercase().contains(words))
            })
            .collect();
        Ok(Ok(rows))
    })
    .await?;
    rows.sort_by(|a, b| {
        a.due
            .as_deref()
            .unwrap_or("9999")
            .cmp(b.due.as_deref().unwrap_or("9999"))
    });
    let total = rows.len();
    let label = match due.as_deref() {
        Some("today") => "due today".to_owned(),
        Some("overdue") => "overdue".to_owned(),
        Some("week") => "due this week".to_owned(),
        Some(_) => format!(
            "due {} to {}",
            from.unwrap_or_default(),
            to.unwrap_or_default()
        ),
        None => text_of(input, "state").unwrap_or_else(|| "all".into()),
    };
    let (rows, truncated) = take(rows, number_of(input, "limit"));
    Ok(json!({
        "summary": format!("Tasks {label} · {total}"),
        "today": today,
        "tasks": rows.iter().map(task_row).collect::<Vec<_>>(),
        "truncated": truncated,
    }))
}

async fn daily_pages(input: &Value, notebook: &LiveNotebook, cx: &mut AsyncApp) -> Outcome {
    let today = today(notebook)?;
    let from = day(text_of(input, "from"), &today).unwrap_or_default();
    let to = day(text_of(input, "to"), &today).unwrap_or_default();
    let (first, last) = (from.clone(), to.clone());
    let days = query(notebook, move |index| {
        let mut dates: Vec<String> = index
            .pages()?
            .into_iter()
            .filter_map(|listed| listed.date)
            .filter(|date| *date >= first && *date <= last)
            .collect();
        dates.sort();
        dates.dedup();
        let days = dates
            .into_iter()
            .map(|date| Ok(index.daily_page(&date)?.map(|page| (date, page))))
            .collect::<anyhow::Result<Vec<_>>>()?;
        Ok(Ok(days.into_iter().flatten().collect::<Vec<_>>()))
    })
    .await?;
    let mut total = 0;
    let mut truncated = None;
    let mut rows = Vec::new();
    for (date, page) in days {
        let text = page_text(notebook, &page, cx).await?.unwrap_or_default();
        let length = utf16_len(&text);
        if total + length > TEXT_CAP {
            truncated = Some(format!(
                "Stopped at {date} after {TEXT_CAP} characters. Call daily_pages again from {date}."
            ));
            break;
        }
        total += length;
        rows.push(json!({ "date": date, "title": page.title, "path": page.path, "text": text }));
    }
    Ok(json!({
        "summary": format!("Daily pages {from} to {to} · {}", rows.len()),
        "pages": rows,
        "truncated": truncated,
    }))
}

async fn mentions(input: &Value, notebook: &LiveNotebook) -> Outcome {
    let today = today(notebook)?;
    let target = text_of(input, "page").unwrap_or_default();
    let reader = notebook.clone();
    let (title, mut rows) = query(notebook, move |index| {
        let page = match require(index, &target, &today)? {
            Ok(page) => page,
            Err(error) => return Ok(Err(error)),
        };
        let modified = index.modified_times()?;
        let mut counts: Vec<(PageRef, usize)> = Vec::new();
        for link in index.backlinks(&page.page)? {
            match counts
                .iter_mut()
                .find(|(source, _)| source.path == link.source.path)
            {
                Some((_, count)) => *count += 1,
                None => counts.push((link.source, 1)),
            }
        }
        let rows = counts
            .into_iter()
            .map(|(source, count)| {
                let changed = modified
                    .get(&source.page.to_lossy_string())
                    .copied()
                    .unwrap_or(0);
                json!({
                    "page": source.title,
                    "path": source.path,
                    "daily": index.date(&source.title),
                    "modified": reader.day_of(changed),
                    "count": count,
                })
            })
            .collect::<Vec<_>>();
        Ok(Ok((page.title, rows)))
    })
    .await?;
    let date = |row: &Value| {
        row["daily"]
            .as_str()
            .or(row["modified"].as_str())
            .unwrap_or("")
            .to_owned()
    };
    rows.sort_by(|a, b| {
        date(b).cmp(&date(a)).then_with(|| {
            a["page"]
                .as_str()
                .unwrap_or("")
                .cmp(b["page"].as_str().unwrap_or(""))
        })
    });
    let total = rows.len();
    let (pages, truncated) = take(rows, number_of(input, "limit"));
    Ok(json!({
        "summary": format!("Pages mentioning [[{title}]] · {total}"),
        "pages": pages,
        "truncated": truncated,
    }))
}
