//! Keeps the notebook's query index current: pulls unanalyzed heads from the index worker,
//! parses them on the background executor, and stores the rows.

use crate::page_index::{ANALYZER_VERSION, analyze, editor_text};
use gpui::{App, AppContext, Context, Entity, SharedString, Task};
use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_store::index::{IndexClient, IndexConfig, IndexWorker};
use std::{path::PathBuf, time::Duration};

const BATCH: usize = 16;
const IDLE: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(5);

pub struct IndexSync {
    client: IndexClient,
    remaining: usize,
    error: Option<SharedString>,
    _task: Task<()>,
}

/// The standard daily layout until notebook settings reach the app (NR16).
pub fn default_config(calendar: DailyCalendar) -> IndexConfig {
    IndexConfig {
        analyzer: ANALYZER_VERSION,
        layout: DailyLayout::default(),
        calendar,
    }
}

fn describe(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

impl IndexSync {
    /// Starts the index worker and its analysis loop. Indexing never blocks editing; failures
    /// are reported through `error` and retried.
    pub fn spawn(
        database: PathBuf,
        config: IndexConfig,
        cx: &mut App,
    ) -> Result<Entity<Self>, String> {
        let worker = IndexWorker::spawn(database, config).map_err(describe)?;
        Ok(cx.new(|cx| {
            let task = cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
                let Ok(client) = this.read_with(cx, |this, _| this.client.clone()) else {
                    return;
                };
                if let Err(error) = worker.ready.await {
                    this.update(cx, |this, cx| this.set_error(Some(describe(error)), cx))
                        .ok();
                    return;
                }
                loop {
                    let (pause, remaining, error) = match Self::step(&client, cx).await {
                        Ok(0) => (Some(IDLE), Some(0), None),
                        Ok(remaining) => (None, Some(remaining), None),
                        Err(error) => (Some(RETRY), None, Some(error)),
                    };
                    let updated = this.update(cx, |this, cx| {
                        this.set_error(error, cx);
                        if let Some(remaining) = remaining
                            && this.remaining != remaining
                        {
                            this.remaining = remaining;
                            cx.notify();
                        }
                    });
                    if updated.is_err() {
                        return;
                    }
                    if let Some(pause) = pause {
                        cx.background_executor().timer(pause).await;
                    }
                }
            });
            Self {
                client: worker.client.clone(),
                remaining: 0,
                error: None,
                _task: task,
            }
        }))
    }

    /// Analyzes one batch and returns how many pages still need analysis.
    async fn step(client: &IndexClient, cx: &mut gpui::AsyncApp) -> Result<usize, String> {
        let pending = client
            .pending(BATCH)
            .map_err(|error| error.to_string())?
            .await
            .map_err(describe)?;
        for page in pending {
            let work = cx.update(|cx| analyze(editor_text(&page.text), cx));
            let Some(analysis) = cx.background_spawn(work).await else {
                return Err("Cannot analyze pages because the Markdown grammar is unavailable. Rebuild Quire with its bundled grammars.".into());
            };
            client
                .put(page.revision, analysis)
                .map_err(|error| error.to_string())?
                .await
                .map_err(describe)?;
        }
        client
            .unanalyzed()
            .map_err(|error| error.to_string())?
            .await
            .map_err(describe)
    }

    fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        let error = error.map(SharedString::from);
        if self.error != error {
            self.error = error;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub fn client(&self) -> &IndexClient {
        &self.client
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Progress for the window status, while pages are still being indexed.
    pub fn status(&self) -> Option<SharedString> {
        (self.remaining > 0).then(|| match self.remaining {
            1 => "Indexing notebook: 1 page left".into(),
            remaining => format!("Indexing notebook: {remaining} pages left").into(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use quire_store::{
        notebook::{Edit, NotebookStore},
        sync::json::JsString,
    };

    pub(super) async fn settle(client: &IndexClient, cx: &mut TestAppContext) {
        for _ in 0..100 {
            cx.executor().advance_clock(IDLE);
            cx.run_until_parked();
            if client.unanalyzed().unwrap().await.unwrap() == 0 {
                return;
            }
        }
        panic!("indexing did not finish");
    }

    fn save(store: &NotebookStore, page: &str, text: &str) {
        let base = store.heads(page).unwrap();
        store
            .save(&Edit {
                page: page.into(),
                path: format!("{page}.md"),
                text: text.into(),
                base,
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
    }

    #[gpui::test]
    async fn stored_pages_become_queryable_and_follow_later_saves(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        save(&store, "Target", "- goal ^g1\n");
        save(&store, "Source", "- see [[Target]] and ((g1)) #Plan\n");
        let sync = cx
            .update(|cx| {
                IndexSync::spawn(database.clone(), default_config(DailyCalendar::utc()), cx)
            })
            .unwrap();
        let client = cx.read(|cx| sync.read(cx).client().clone());
        settle(&client, cx).await;
        let links = client
            .backlinks(JsString::from("Target"))
            .unwrap()
            .await
            .unwrap();
        assert_eq!(
            links
                .iter()
                .map(|link| (link.link.kind.as_str(), link.link.text.as_str()))
                .collect::<Vec<_>>(),
            [
                ("page", "see [[Target]] and ((g1)) #Plan"),
                ("block", "see [[Target]] and ((g1)) #Plan")
            ]
        );
        save(&store, "Source", "- nothing here\n");
        settle(&client, cx).await;
        assert!(
            client
                .backlinks(JsString::from("Target"))
                .unwrap()
                .await
                .unwrap()
                .is_empty()
        );
        assert!(client.tag_names().unwrap().await.unwrap().is_empty());
        assert!(cx.read(|cx| sync.read(cx).error().is_none()));
    }
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use gpui::TestAppContext;
    use quire_store::notebook::{Edit, NotebookStore};
    use serde_json::Value;
    use std::collections::BTreeMap;

    #[gpui::test]
    async fn search_matches_the_frozen_reference_index(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let fixture: Value =
            serde_json::from_str(include_str!("../../../tests/fixtures/search-vectors.json"))
                .unwrap();
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        for page in fixture["pages"].as_array().unwrap() {
            let path = page["path"].as_str().unwrap();
            store
                .save(&Edit {
                    page: path.into(),
                    path: path.into(),
                    text: page["text"].as_str().unwrap().into(),
                    base: vec![],
                    deleted: false,
                    created: 1,
                    recovery: None,
                })
                .unwrap();
        }
        let sync = cx
            .update(|cx| IndexSync::spawn(database, default_config(DailyCalendar::utc()), cx))
            .unwrap();
        let client = cx.read(|cx| sync.read(cx).client().clone());
        super::tests::settle(&client, cx).await;
        let mut failures = Vec::new();
        for vector in fixture["queries"].as_array().unwrap() {
            let query = vector["query"].as_str().unwrap();
            let expected = vector["hits"].as_array().unwrap();
            let actual = client.search(query.into()).unwrap().await.unwrap();
            let key = |path: &str, title: &str, snippet: &str, block: Option<&str>| {
                format!(
                    "{path}\u{1}{title}\u{1}{snippet}\u{1}{}",
                    block.unwrap_or("-")
                )
            };
            if actual.len() != expected.len() {
                failures.push(format!(
                    "{query:?}: {} hits, expected {}",
                    actual.len(),
                    expected.len()
                ));
                continue;
            }
            // Equal bm25 scores have no defined order; compare each tie run as a multiset. A run cut
            // by the 40-hit limit can hold different members of the tie, so only its size counts.
            let mut start = 0;
            while start < expected.len() {
                let score = expected[start]["score"].as_f64().unwrap();
                let end = (start..expected.len())
                    .find(|&i| (expected[i]["score"].as_f64().unwrap() - score).abs() > 1e-9)
                    .unwrap_or(expected.len());
                if !(end == 40 && start > 0) {
                    let count = |keys: Vec<String>| {
                        keys.into_iter().fold(BTreeMap::new(), |mut map, key| {
                            *map.entry(key).or_insert(0) += 1;
                            map
                        })
                    };
                    let want = count(
                        expected[start..end]
                            .iter()
                            .map(|hit| {
                                key(
                                    hit["path"].as_str().unwrap(),
                                    hit["title"].as_str().unwrap(),
                                    hit["snippet"].as_str().unwrap(),
                                    hit["blockId"].as_str(),
                                )
                            })
                            .collect(),
                    );
                    let got = count(
                        actual[start..end]
                            .iter()
                            .map(|hit| {
                                key(
                                    &hit.source.path,
                                    &hit.source.title,
                                    &hit.snippet,
                                    hit.block_id.as_deref(),
                                )
                            })
                            .collect(),
                    );
                    if want != got {
                        failures.push(format!(
                            "{query:?} hits {start}..{end}:\nexpected {want:?}\nactual   {got:?}"
                        ));
                    }
                }
                start = end;
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }
}
