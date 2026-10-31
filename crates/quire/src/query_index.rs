//! Keeps the notebook's query index current: pulls unanalyzed heads from the index worker,
//! parses them on the background executor, and stores the rows.

use crate::notebook_session::OpenPage;
use crate::page_index::{ANALYZER_VERSION, analyze, editor_text};
use gpui::{
    App, AppContext, Context, Entity, EntityId, SharedString, Subscription, Task, WeakEntity,
};
use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_core::{json::JsString, markdown::page_title_for_editor};
use quire_store::index::Overlay;
use quire_store::index::{IndexClient, IndexConfig, IndexWorker};
use std::collections::HashMap;
use std::{path::PathBuf, time::Duration};

const BATCH: usize = 16;
const IDLE: Duration = Duration::from_secs(1);
const RETRY: Duration = Duration::from_secs(5);
/// Coalesces typing into one overlay analysis.
const DEBOUNCE: Duration = Duration::from_millis(150);

pub struct IndexSync {
    client: IndexClient,
    remaining: usize,
    generation: u64,
    error: Option<SharedString>,
    tracked: HashMap<EntityId, Tracked>,
    _task: Task<()>,
}

/// The index answers queries differently than before: saved pages were analyzed or removed, or
/// an open page's unsaved text changed. Views showing query results re-run them.
pub struct IndexChanged;

impl gpui::EventEmitter<IndexChanged> for IndexSync {}

/// An open page feeding its live analysis to the index.
struct Tracked {
    page: WeakEntity<OpenPage>,
    buffer: Option<EntityId>,
    path: Option<String>,
    update: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

/// The index configuration for a notebook's daily layout, read from its settings page.
pub fn config(layout: DailyLayout, calendar: DailyCalendar) -> IndexConfig {
    IndexConfig {
        analyzer: ANALYZER_VERSION,
        layout,
        calendar,
    }
}

/// The standard daily layout, for notebooks without settings.
#[cfg(test)]
pub fn default_config(calendar: DailyCalendar) -> IndexConfig {
    config(DailyLayout::default(), calendar)
}

fn describe(error: impl std::fmt::Display) -> String {
    format!("{error:#}")
}

/// Sends a request to the index worker and waits for its reply, as one error message.
pub(crate) async fn ask<T>(
    request: Result<quire_store::worker::Reply<T>, quire_store::worker::QueueError>,
) -> Result<T, String> {
    request
        .map_err(|error| error.to_string())?
        .await
        .map_err(describe)
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
        Ok(Self::run(worker, cx))
    }

    /// Starts the index of the folder notebook at `root`, kept in `state` outside it.
    pub fn spawn_folder(
        root: PathBuf,
        state: PathBuf,
        config: IndexConfig,
        cx: &mut App,
    ) -> Result<Entity<Self>, String> {
        let worker = IndexWorker::spawn_folder(root, state, config).map_err(describe)?;
        Ok(Self::run(worker, cx))
    }

    fn run(worker: IndexWorker, cx: &mut App) -> Entity<Self> {
        cx.new(|cx| {
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
                    let (pause, progress, error) = match Self::step(&client, cx).await {
                        Ok((0, generation)) => (Some(IDLE), Some((0, generation)), None),
                        Ok(progress) => (None, Some(progress), None),
                        Err(error) => (Some(RETRY), None, Some(error)),
                    };
                    let updated = this.update(cx, |this, cx| {
                        this.set_error(error, cx);
                        if let Some((remaining, generation)) = progress {
                            if this.remaining != remaining {
                                this.remaining = remaining;
                                cx.notify();
                            }
                            if this.generation != generation {
                                this.generation = generation;
                                cx.emit(IndexChanged);
                            }
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
                generation: 0,
                error: None,
                tracked: HashMap::new(),
                _task: task,
            }
        })
    }

    /// Analyzes one batch and returns how many pages still need analysis, with the index
    /// generation after it.
    async fn step(client: &IndexClient, cx: &mut gpui::AsyncApp) -> Result<(usize, u64), String> {
        let pending = ask(client.pending(BATCH)).await?;
        for page in pending {
            let work = cx.update(|cx| analyze(editor_text(&page.text), cx));
            let Some(analysis) = cx.background_spawn(work).await else {
                return Err("Cannot analyze pages because the Markdown grammar is unavailable. Rebuild Quire with its bundled grammars.".into());
            };
            ask(client.put(page.revision, analysis)).await?;
        }
        let (generation, remaining) =
            ask(client.with_generation(|index| index.unanalyzed())).await?;
        Ok((remaining, generation))
    }

    fn set_error(&mut self, error: Option<String>, cx: &mut Context<Self>) {
        let error = error.map(SharedString::from);
        if self.error != error {
            self.error = error;
            cx.notify();
        }
    }

    pub fn client(&self) -> &IndexClient {
        &self.client
    }

    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Feeds an open page's live analysis to the index until the page entity is released, so
    /// queries see unsaved edits without counting the committed version as well.
    pub fn track(&mut self, page: Entity<OpenPage>, cx: &mut Context<Self>) {
        let id = page.entity_id();
        if self.tracked.contains_key(&id) {
            return;
        }
        let subscriptions = vec![
            cx.observe(&page, move |this, _, cx| this.attach(id, cx)),
            cx.observe_release(&page, move |this, page, cx| {
                this.tracked.remove(&id);
                let release = this.client.release_overlay(JsString::from(page.id()));
                cx.spawn(async move |this, cx| {
                    if ask(release).await.is_ok() {
                        this.update(cx, |_, cx| cx.emit(IndexChanged)).ok();
                    }
                })
                .detach();
            }),
        ];
        self.tracked.insert(
            id,
            Tracked {
                page: page.downgrade(),
                buffer: None,
                path: None,
                update: None,
                _subscriptions: subscriptions,
            },
        );
        self.attach(id, cx);
    }

    fn attach(&mut self, id: EntityId, cx: &mut Context<Self>) {
        let Some(tracked) = self.tracked.get_mut(&id) else {
            return;
        };
        let Some((buffer, path)) = tracked.page.upgrade().and_then(|page| {
            let page = page.read(cx);
            let buffer = page.loaded()?.analysis.read(cx).source.clone();
            Some((buffer, page.path(cx)))
        }) else {
            return;
        };
        if tracked.buffer == Some(buffer.entity_id()) {
            // A rename moves the page without editing it; its overlay must follow.
            if tracked.path != path {
                tracked.path = path;
                self.schedule(id, Duration::ZERO, cx);
            }
            return;
        }
        tracked.buffer = Some(buffer.entity_id());
        tracked.path = path;
        tracked
            ._subscriptions
            .push(cx.subscribe(&buffer, move |this, _, event, cx| {
                if matches!(event, language::BufferEvent::Edited { .. }) {
                    this.schedule(id, DEBOUNCE, cx);
                }
            }));
        self.schedule(id, Duration::ZERO, cx);
    }

    fn schedule(&mut self, id: EntityId, delay: Duration, cx: &mut Context<Self>) {
        let Some(tracked) = self.tracked.get_mut(&id) else {
            return;
        };
        let page = tracked.page.clone();
        let client = self.client.clone();
        tracked.update = Some(cx.spawn(async move |this: gpui::WeakEntity<Self>, cx| {
            if !delay.is_zero() {
                cx.background_executor().timer(delay).await;
            }
            let Ok(Some((key, path, text))) = page.read_with(cx, |page, cx| {
                page.loaded().map(|loaded| {
                    let text = loaded.analysis.read(cx).source.read(cx).text();
                    (
                        page.id().to_owned(),
                        page.path(cx).unwrap_or_default(),
                        text,
                    )
                })
            }) else {
                return;
            };
            let title = page_title_for_editor(&path.as_str().into(), &text.as_str().into())
                .to_lossy_string();
            let work = cx.update(|cx| analyze(text, cx));
            let Some(index) = cx.background_spawn(work).await else {
                return;
            };
            let result =
                ask(client
                    .set_overlay(JsString::from(key.as_str()), Overlay { path, title, index }))
                .await;
            this.update(cx, |this, cx| {
                if result.is_ok() {
                    cx.emit(IndexChanged);
                }
                this.set_error(result.err(), cx)
            })
            .ok();
        }));
    }

    /// Stops feeding a deleted page's live analysis, so queries see its stored deletion.
    pub fn forget(&mut self, page: &Entity<OpenPage>, cx: &mut Context<Self>) {
        if self.tracked.remove(&page.entity_id()).is_none() {
            return;
        }
        let release = self
            .client
            .release_overlay(JsString::from(page.read(cx).id()));
        cx.spawn(async move |this, cx| {
            let result = ask(release).await;
            this.update(cx, |this, cx| {
                if result.is_ok() {
                    cx.emit(IndexChanged);
                }
                this.set_error(result.err(), cx)
            })
            .ok();
        })
        .detach();
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
pub(crate) mod tests {
    use super::*;
    use gpui::TestAppContext;
    use quire_store::{
        notebook::{Edit, NotebookStore},
        sync::json::JsString,
    };

    pub(crate) async fn settle(client: &IndexClient, cx: &mut TestAppContext) {
        for _ in 0..100 {
            cx.executor().advance_clock(IDLE);
            cx.run_until_parked();
            if client.unanalyzed().unwrap().await.unwrap() == 0 {
                return;
            }
        }
        panic!(
            "indexing did not finish: {:?}",
            client.pending(4).unwrap().await.map(|pages| pages.len())
        );
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

#[cfg(test)]
mod overlay_tests {
    use super::*;
    use crate::notebook_session::NotebookSession;
    use gpui::TestAppContext;
    use quire_store::notebook::{Edit, NotebookStore};

    #[gpui::test]
    async fn unsaved_edits_reach_queries_and_closing_falls_back_to_saved_rows(
        cx: &mut TestAppContext,
    ) {
        cx.executor().allow_parking();
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        for (page, text) in [
            ("Target", ""),
            ("Other", ""),
            ("Source", "- see [[Target]]\n"),
        ] {
            store
                .save(&Edit {
                    page: page.into(),
                    path: format!("{page}.md"),
                    text: text.into(),
                    base: vec![],
                    deleted: false,
                    created: 1,
                    recovery: None,
                })
                .unwrap();
        }
        let sync = cx
            .update(|cx| {
                IndexSync::spawn(database.clone(), default_config(DailyCalendar::utc()), cx)
            })
            .unwrap();
        let client = cx.read(|cx| sync.read(cx).client().clone());
        super::tests::settle(&client, cx).await;
        let mut session =
            NotebookSession::open(database.clone(), "nb".into(), "local".into()).unwrap();
        let page = cx.update(|cx| session.open_page("Source".into(), cx));
        crate::notebook_session::tests::wait(&page, cx);
        sync.update(cx, |sync, cx| sync.track(page.clone(), cx));
        let backlinks = async |page: &str, cx: &mut TestAppContext| {
            cx.executor().advance_clock(DEBOUNCE);
            cx.run_until_parked();
            client
                .with_generation({
                    let page = JsString::from(page);
                    move |index| index.backlinks(&page)
                })
                .unwrap()
                .await
                .unwrap()
        };
        let (opened, links) = backlinks("Target", cx).await;
        assert_eq!(links.len(), 1, "an open page counts once");
        let buffer = cx.read(|cx| {
            page.read(cx)
                .loaded()
                .unwrap()
                .analysis
                .read(cx)
                .source
                .clone()
        });
        buffer.update(cx, |buffer, cx| {
            buffer.set_capability(language::Capability::ReadWrite, cx);
            buffer.edit([(8..14, "Other")], None, cx);
        });
        let (edited, links) = backlinks("Other", cx).await;
        assert!(edited > opened, "an overlay change moves the generation");
        assert_eq!(
            links[0].link.text, "see [[Other]]",
            "queries see unsaved text"
        );
        assert!(backlinks("Target", cx).await.1.is_empty());
        drop(session);
        drop(page);
        drop(buffer);
        cx.update(|_| {});
        cx.run_until_parked();
        assert_eq!(
            backlinks("Target", cx).await.1.len(),
            1,
            "a closed page without a save falls back to its committed rows"
        );
        assert!(backlinks("Other", cx).await.1.is_empty());
    }
}

#[cfg(test)]
mod reference_filter_tests {
    use super::*;
    use gpui::TestAppContext;
    use quire_core::reference_filter::ReferenceFilter;
    use quire_store::notebook::{Edit, NotebookStore};
    use serde_json::Value;

    #[gpui::test]
    async fn linked_reference_filters_match_the_shared_fixture(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../tests/fixtures/reference-filters.json"
        ))
        .unwrap();
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        for (path, text) in fixture["pages"].as_object().unwrap() {
            store
                .save(&Edit {
                    page: path.clone(),
                    path: path.clone(),
                    text: text.as_str().unwrap().into(),
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
        let shown = JsString::from(fixture["page"].as_str().unwrap());
        let references = client
            .linked_references(shown.clone(), false)
            .unwrap()
            .await
            .unwrap();
        for case in fixture["cases"].as_array().unwrap() {
            let source = case["filter"].as_str().unwrap();
            let filter = ReferenceFilter::parse(source);
            if let Some(error) = case["error"].as_str() {
                assert_eq!(filter, Err(error), "{source:?}");
                continue;
            }
            let filter = filter.unwrap();
            let names = client.filter_names(filter.clone()).unwrap().await.unwrap();
            let mut actual: Vec<_> = references
                .iter()
                .filter(|reference| filter.matches(&names, &reference.context))
                .map(|reference| reference.snippet.clone())
                .collect();
            let mut expected: Vec<String> = case["expect"]
                .as_array()
                .unwrap()
                .iter()
                .map(|value| value.as_str().unwrap().to_owned())
                .collect();
            actual.sort();
            expected.sort();
            assert_eq!(actual, expected, "filter {source:?}");
        }
        let contexts = references
            .iter()
            .map(|reference| reference.context.clone())
            .collect();
        let facets = client
            .reference_facets(shown, contexts, 8)
            .unwrap()
            .await
            .unwrap();
        let labels: Vec<_> = facets
            .iter()
            .map(|facet| (facet.label.as_str(), facet.count))
            .collect();
        assert!(labels.contains(&("Launch", 2)), "{labels:?}");
        assert!(
            !labels.iter().any(|(label, _)| *label == "Ana"),
            "the shown page is not a facet"
        );
    }
}

#[cfg(test)]
mod tag_edit_tests {
    use super::*;
    use gpui::TestAppContext;
    use quire_core::tag_edits::Retag;
    use quire_store::notebook::{Edit, NotebookStore};

    #[gpui::test]
    async fn bulk_tag_edits_plan_only_pages_the_index_says_they_touch(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("local.sqlite");
        let store = NotebookStore::open(&database, "nb", "local").unwrap();
        let pages = [
            ("Nested.md", "- #Area/home chores\n"),
            ("Front.md", "---\ntags: [area, other]\n---\n- text\n"),
            ("Longer.md", "- #areas is another tag\n"),
            ("Code.md", "- `#area` in code\n"),
        ];
        for (path, text) in pages {
            store
                .save(&Edit {
                    page: path.into(),
                    path: path.into(),
                    text: text.into(),
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

        let plan = async |rename: Option<&str>, cx: &mut TestAppContext| {
            let retag = Retag::new(&["#area"], rename).unwrap();
            let mut planned = Vec::new();
            for page in client.tag_edit_pages(retag.clone()).unwrap().await.unwrap() {
                let text = pages.iter().find(|(path, _)| *path == page.path).unwrap().1;
                let work =
                    cx.update(|cx| crate::page_index::edit_tags(text.into(), retag.clone(), cx));
                if let Some(edit) = cx.background_executor.spawn(work).await {
                    planned.push((page.path, edit.text));
                }
            }
            planned
        };
        assert_eq!(
            plan(Some("job"), cx).await,
            [
                (
                    "Front.md".into(),
                    "---\ntags: [job, other]\n---\n- text\n".into()
                ),
                ("Nested.md".into(), "- #job/home chores\n".into()),
            ]
        );
        assert_eq!(
            plan(None, cx).await,
            [(
                "Front.md".into(),
                "---\ntags: [other]\n---\n- text\n".into()
            )]
        );
    }
}

#[cfg(test)]
mod task_tests;
