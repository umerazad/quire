//! Keep in today's note (reference `keepInToday` in `AiPage.tsx`): appends an answer to today's
//! daily page as one block, creating the page where the daily layout puts it when missing. A
//! quoted citation becomes a block reference only now, marking the quoted block with an ID when
//! it has none; blocks that cannot take one, such as code, stay cited by page.

use super::context::Writers;
use crate::query_index::ask;
use gpui::{App, Task};
use quire_core::ai::{
    ask::{answer_block, resolve_quotes},
    commands::marker_of,
};
use quire_store::index::IndexClient;
use std::{collections::HashMap, sync::Arc};

pub const STALE: &str =
    "A page changed while the answer was kept. Press Keep in today's note again.";

/// Today's daily page: its date, and the title and folder a new one gets.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Today {
    pub date: String,
    pub title: String,
    pub folder: String,
}

/// The ID of the first block outside code that quotes `quote`, marking it in `text` when it has
/// no ID yet.
fn mark(text: &mut String, quote: &str) -> Option<String> {
    let mut fenced = false;
    let mut at = 0;
    for line in text.split_inclusive('\n') {
        let start = at;
        at += line.len();
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            fenced = !fenced;
            continue;
        }
        if fenced || !line.contains(quote) {
            continue;
        }
        if let Some(id) = marker_of(line.trim_end()) {
            return Some(id.to_owned());
        }
        let id = crate::references::new_block_id();
        let end = start + line.trim_end().len();
        text.insert_str(end, &format!(" ^{id}"));
        return Some(id);
    }
    None
}

fn append(text: &str, block: &str) -> String {
    let gap = if text.is_empty() || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    format!("{text}{gap}{block}\n")
}

/// Keeps `answer` in today's page and returns that page's title.
pub fn keep(
    answer: String,
    today: Today,
    index: IndexClient,
    writers: Writers,
    cx: &mut App,
) -> Task<Result<String, String>> {
    let (open, storage) = writers;
    cx.spawn(async move |cx| {
        let mut quotes = Vec::new();
        resolve_quotes(&answer, |title, quote| {
            quotes.push((title.to_owned(), quote.to_owned()));
            None
        });
        let mut sources = HashMap::new();
        let mut minted = HashMap::new();
        for (title, quote) in quotes {
            let Some(page) = ask(index.find_page(title.clone())).await? else {
                continue;
            };
            let id = page.page.to_lossy_string();
            if !sources.contains_key(&id) {
                let Some(source) = crate::page_writes::read(id.clone(), &open, &storage, cx).await?
                else {
                    continue;
                };
                let text = source.stored.clone();
                sources.insert(id.clone(), (source, text));
            }
            let (_, text) = sources.get_mut(&id).expect("the source was read above");
            if let Some(block) = mark(text, &quote) {
                minted.insert((title, quote), block);
            }
        }
        let resolved = resolve_quotes(&answer, |title, quote| {
            minted.get(&(title.to_owned(), quote.to_owned())).cloned()
        });
        let block = answer_block(&resolved);
        let date = today.date.clone();
        let (_, day) = ask(index.with_generation(move |index| index.daily_page(&date))).await?;
        let day = day.map(|page| page.page.to_lossy_string());
        if let Some(day) = &day
            && !sources.contains_key(day)
        {
            let source = crate::page_writes::read(day.clone(), &open, &storage, cx)
                .await?
                .ok_or_else(|| {
                    format!(
                        "Today's page {} could not be read. Open it, then press Keep in today's note again.",
                        today.title
                    )
                })?;
            let text = source.stored.clone();
            sources.insert(day.clone(), (source, text));
        }
        let writes: Vec<_> = sources
            .into_iter()
            .filter_map(|(id, (source, text))| {
                let text = if Some(&id) == day.as_ref() {
                    append(&text, &block)
                } else if text != source.stored {
                    text
                } else {
                    return None;
                };
                Some(source.write(Arc::from(text)))
            })
            .collect();
        if !writes.is_empty() {
            let written =
                cx.update(|cx| crate::page_writes::apply(writes, storage.clone(), STALE, cx));
            written.await?;
        }
        if day.is_none() {
            crate::navigation::create_page_with(
                storage,
                index,
                today.title.clone(),
                today.folder,
                append("", &block),
            )
            .await?;
        }
        Ok(today.title)
    })
}

#[cfg(test)]
mod tests {
    use super::mark;

    #[test]
    fn marks_the_quoted_block_once_and_skips_code() {
        let mut text = "```\nfenced words here\n```\n- good and tasty apples\n".to_owned();
        assert_eq!(mark(&mut text, "fenced words here"), None);
        let id = mark(&mut text, "good and tasty").unwrap();
        assert_eq!(
            text,
            format!("```\nfenced words here\n```\n- good and tasty apples ^{id}\n")
        );
        assert_eq!(mark(&mut text, "good and tasty"), Some(id));
    }
}
