//! The notebook excerpts sent with a question, read from the query index, which includes unsaved
//! edits of open pages. Each source stands in for the reference's notebook tools: tasks for
//! what is due, mentions for the last occurrence, tagged blocks, linked references for a
//! synthesis, and search for anything else.

use crate::query_index::ask;
use quire_core::ai::ask::{Excerpt, Gathered, Source, search_terms};
use quire_core::ai::commands::marker_of;
use quire_core::tasks::add_days;
use quire_store::index::IndexClient;
use std::collections::HashMap;

/// The open pages and the store, for reading a whole page with its unsaved edits.
pub type Writers = (
    Vec<gpui::Entity<crate::notebook_session::OpenPage>>,
    crate::notebook_backend::NotebookBackend,
);

fn excerpt(title: &str, id: Option<&String>, text: &str) -> Excerpt {
    Excerpt {
        title: title.to_owned(),
        id: id.cloned(),
        text: text.to_owned(),
    }
}

/// A page's top-level blocks, each with its nested lines, as excerpts.
pub fn page_excerpts(title: &str, text: &str) -> Vec<Excerpt> {
    let mut blocks: Vec<Vec<&str>> = Vec::new();
    for line in text.lines() {
        let top = !line.starts_with([' ', '\t']) && !line.trim().is_empty();
        match blocks.last_mut() {
            Some(block) if !top => block.push(line),
            _ => blocks.push(vec![line]),
        }
    }
    blocks
        .into_iter()
        .map(|lines| Excerpt {
            title: title.to_owned(),
            id: lines
                .first()
                .and_then(|line| marker_of(line))
                .map(str::to_owned),
            text: lines.join("\n"),
        })
        .collect()
}

/// Gathers the excerpts for `source`. `today` is the notebook's date.
pub async fn gather(
    index: IndexClient,
    source: Source,
    question: &str,
    today: &str,
    writers: Option<Writers>,
    cx: &mut gpui::AsyncApp,
) -> Result<Gathered, String> {
    let excerpts: Vec<Excerpt> = match &source {
        Source::References(title) => {
            let Some(page) = ask(index.find_page(title.clone())).await? else {
                return Ok(Gathered::bounded(source, []));
            };
            ask(index.linked_references(page.page, false))
                .await?
                .into_iter()
                .map(|block| {
                    let children =
                        block.outline.children.iter().map(|(depth, text)| {
                            format!("{}- {text}", "  ".repeat(*depth as usize))
                        });
                    let text = std::iter::once(block.snippet.clone())
                        .chain(children)
                        .collect::<Vec<_>>()
                        .join("\n");
                    excerpt(&block.source.title, block.block_id.as_ref(), &text)
                })
                .collect()
        }
        Source::Due | Source::Week => {
            let until = match source {
                Source::Week => add_days(today, 7).unwrap_or_else(|| today.to_owned()),
                _ => today.to_owned(),
            };
            let (_, mut tasks) = ask(index.with_generation(|index| index.tasks())).await?;
            tasks.retain(|task| {
                task.is_open() && task.due.as_deref().is_some_and(|due| due <= until.as_str())
            });
            tasks.sort_by(|a, b| a.due.cmp(&b.due));
            tasks
                .iter()
                .map(|task| {
                    let due = task.due.clone().unwrap_or_default();
                    let when = if due.as_str() < today {
                        format!("overdue since {due}")
                    } else {
                        format!("due {due}")
                    };
                    excerpt(
                        &task.source.title,
                        task.id.as_ref(),
                        &format!("{} ({when})", task.raw.trim()),
                    )
                })
                .collect()
        }
        Source::Mentions(title) => ask(index.mentions(title.clone()))
            .await?
            .into_iter()
            .map(|link| excerpt(&link.source.title, None, &link.link.text))
            .collect(),
        Source::Tagged(tag) => ask(index.tagged_blocks(tag.trim_start_matches('#').to_owned()))
            .await?
            .into_iter()
            .map(|tagged| {
                excerpt(
                    &tagged.source.title,
                    tagged.tag.block_id.as_ref(),
                    &tagged.tag.snippet,
                )
            })
            .collect(),
        Source::Yesterday => {
            let yesterday = add_days(today, -1).unwrap_or_default();
            let day = yesterday.clone();
            let (_, page) = ask(index.with_generation(move |index| index.daily_page(&day))).await?;
            let mut found = Vec::new();
            if let (Some(page), Some((open, storage))) = (page, writers) {
                let id = page.page.to_lossy_string();
                if let Some(source) = crate::page_writes::read(id, &open, &storage, cx).await? {
                    found.extend(page_excerpts(&page.title, &source.stored));
                }
            }
            let (_, tasks) = ask(index.with_generation(|index| index.tasks())).await?;
            found.extend(tasks.iter().filter_map(|task| {
                let label = if task.completed.as_deref() == Some(yesterday.as_str()) {
                    "completed"
                } else if task.created.as_deref() == Some(yesterday.as_str()) {
                    "added"
                } else {
                    return None;
                };
                Some(excerpt(
                    &task.source.title,
                    task.id.as_ref(),
                    &format!("{} ({label} {yesterday})", task.raw.trim()),
                ))
            }));
            found
        }
        Source::Search => {
            let mut scored: HashMap<(String, String), (usize, usize, Excerpt)> = HashMap::new();
            for (order, term) in search_terms(question).into_iter().enumerate() {
                for hit in ask(index.search(term)).await? {
                    let key = (hit.source.title.clone(), hit.snippet.clone());
                    let entry = scored.entry(key).or_insert_with(|| {
                        (
                            0,
                            order,
                            excerpt(&hit.source.title, hit.block_id.as_ref(), &hit.snippet),
                        )
                    });
                    entry.0 += 1;
                }
            }
            let mut hits: Vec<_> = scored.into_values().collect();
            hits.sort_by(|a, b| {
                b.0.cmp(&a.0)
                    .then(a.1.cmp(&b.1))
                    .then(a.2.title.cmp(&b.2.title))
            });
            hits.into_iter().map(|(_, _, excerpt)| excerpt).collect()
        }
    };
    Ok(Gathered::bounded(source, excerpts))
}
