//! A generated folder notebook for comparing cheapened queries with the straightforward versions
//! they replaced: shared words and prefixes, so ranks tie and searches match many rows; dated and
//! linked tasks in every state; tags spelled in different cases; project pages; a title shared by
//! two pages; and open pages whose unsaved analysis shadows their saved rows.

use super::{
    IndexConfig, IndexedBlock, IndexedLink, IndexedTag, IndexedTask, Overlay, PageIndex, QueryIndex,
};
use crate::sync::{format::page_id_for_path, json::JsString};
use anyhow::Result;
use quire_core::{
    daily::{DailyCalendar, DailyLayout},
    tasks::DateSources,
};

pub(super) struct TestNotebook {
    pub root: tempfile::TempDir,
    pub index: QueryIndex,
    /// Page paths, in the order they were written.
    pub paths: Vec<String>,
}

/// A deterministic xorshift generator, so failures reproduce.
pub(super) struct Random(u32);

impl Random {
    pub fn new(seed: u32) -> Self {
        Self(seed.max(1))
    }

    pub fn below(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0 as usize % bound
    }

    pub fn chance(&mut self, percent: usize) -> bool {
        self.below(100) < percent
    }

    pub fn pick<'a>(&mut self, items: &'a [String]) -> &'a str {
        &items[self.below(items.len())]
    }
}

pub(super) const WORDS: [&str; 12] = [
    "budget",
    "budgets",
    "budgeting",
    "plan",
    "review",
    "draft",
    "design",
    "storage",
    "note",
    "meeting",
    "ملاحظات",
    "quarter",
];
const STATES: [&str; 5] = ["inbox", "next", "waiting", "someday", "done"];
pub(super) const TODAY: &str = "2024-02-10";

fn config() -> IndexConfig {
    IndexConfig {
        analyzer: 1,
        layout: DailyLayout {
            folder: "daily".into(),
            title: "{YYYY}-{MM}-{DD}".into(),
        },
        calendar: DailyCalendar::utc(),
    }
}

fn day(offset: usize) -> String {
    format!("2024-{:02}-{:02}", 1 + offset / 28, 1 + offset % 28)
}

fn title_of(path: &str) -> String {
    let name = path.rsplit('/').next().unwrap_or(path);
    name.strip_suffix(".md").unwrap_or(name).to_owned()
}

/// Every page's path: daily pages, people, two project pages, two pages sharing a title, and
/// topics.
fn page_paths(pages: usize) -> Vec<String> {
    let daily = pages / 4;
    let people = pages / 10;
    let mut paths: Vec<String> = (0..daily)
        .map(|i| format!("daily/{}.md", day(i)))
        .chain((0..people).map(|i| format!("People/Person {i}.md")))
        .chain([
            "Projects/Launch.md".to_owned(),
            "Garden.md".to_owned(),
            "One/Same.md".to_owned(),
            "Two/Same.md".to_owned(),
        ])
        .collect();
    let topics = pages.saturating_sub(paths.len());
    paths.extend((0..topics).map(|i| format!("Topic {i}.md")));
    paths
}

/// A link target naming `path` the ways people write them: by title, by path, with a heading,
/// in another case, or as a date.
fn target(random: &mut Random, path: &str) -> String {
    let title = title_of(path);
    match random.below(6) {
        0 => path.strip_suffix(".md").unwrap_or(path).to_owned(),
        1 => format!("{title}#Notes"),
        2 => title.to_uppercase(),
        3 => path.to_owned(),
        _ => title,
    }
}

/// Analysis for one page, as the app's analyzer would produce it for generated text.
pub(super) fn analysis(
    random: &mut Random,
    path: &str,
    paths: &[String],
    words: usize,
) -> PageIndex {
    let mut index = PageIndex::default();
    if path == "Projects/Launch.md" {
        index.front_tags = vec!["project/launch".into()];
    }
    let blocks = 3 + random.below(10);
    let mut offset = 0u32;
    let mut parent_links: Vec<String> = Vec::new();
    for ordinal in 0..blocks {
        let mut text: Vec<String> = (0..1 + random.below(words))
            .map(|_| WORDS[random.below(WORDS.len())].to_owned())
            .collect();
        let mut links = Vec::new();
        if random.chance(40) {
            let linked = random.pick(paths).to_owned();
            links.push(target(random, &linked));
        }
        if random.chance(10) {
            links.push(day(random.below(60)));
        }
        if random.chance(5) {
            links.push("Same".into());
        }
        let from = offset;
        for link in &links {
            index.links.push(IndexedLink {
                kind: "page".into(),
                syntax: "wiki".into(),
                target: link.clone(),
                from: offset + 2,
                to: offset + 6 + link.len() as u32,
                text: format!("{} [[{link}]]", text.join(" ")),
                ..Default::default()
            });
            text.push(format!("[[{link}]]"));
        }
        let mut tags = Vec::new();
        if random.chance(30) {
            let number = random.below(12);
            let tag = match random.below(3) {
                0 => format!("#Topic-{number}"),
                _ => format!("#topic-{number}"),
            };
            tags.push(tag);
        }
        if random.chance(4) {
            tags.push("#project/launch".into());
        }
        if path == "Garden.md" && ordinal == 0 {
            tags.push("#project".into());
        }
        for tag in &tags {
            index.tagged.push(IndexedTag {
                tag: tag.clone(),
                from: offset + 1,
                block_id: None,
                snippet: text.join(" "),
            });
            text.push(tag.clone());
        }
        let id = random
            .chance(10)
            .then(|| format!("b{}-{ordinal}", paths.len()));
        let text = text.join(" ");
        if random.chance(35) {
            let state = STATES[random.below(STATES.len())];
            let mut dates = DateSources::default();
            if random.chance(30) {
                dates.due = Some(day(random.below(60)));
            }
            let mut task_links = links.clone();
            if random.chance(50) {
                task_links.extend(parent_links.iter().cloned());
            }
            dates.links = task_links.clone();
            index.tasks.push(IndexedTask {
                key: format!("k{ordinal}"),
                id: id.clone(),
                from,
                raw: format!("[ ] {text}"),
                text: text.clone(),
                state: state.into(),
                tags: tags.clone(),
                links: task_links,
                dates,
                completed: (state == "done").then(|| day(random.below(60))),
            });
        }
        parent_links = links;
        let to = from + text.len() as u32;
        index.blocks.push(IndexedBlock {
            key: format!("k{ordinal}"),
            id,
            from,
            to,
            content_from: from,
            content_to: to,
            subtree_to: to,
            depth: 0,
            kind: "list".into(),
            parent: None,
            content: text.clone(),
            text,
        });
        offset = to + 1;
    }
    index
}

impl TestNotebook {
    /// Writes `pages` generated pages and analyzes them all.
    pub fn new(pages: usize, seed: u32) -> Result<Self> {
        let root = tempfile::tempdir()?;
        let notebook = root.path().join("notebook");
        let paths = page_paths(pages);
        for (number, path) in paths.iter().enumerate() {
            let file = notebook.join(path);
            std::fs::create_dir_all(file.parent().expect("pages sit in the notebook"))?;
            std::fs::write(file, format!("- page {number}\n"))?;
        }
        let index = QueryIndex::open_folder(&notebook, &root.path().join("state"), config())?;
        let notebook = Self { root, index, paths };
        notebook.analyze(seed)?;
        Ok(notebook)
    }

    /// Analyzes every pending page, with analysis generated from `seed` and its path.
    pub fn analyze(&self, seed: u32) -> Result<()> {
        loop {
            let pending = self.index.pending(256)?;
            if pending.is_empty() {
                return Ok(());
            }
            let analyses: Vec<(String, PageIndex)> = pending
                .iter()
                .map(|page| {
                    let path = page.path.to_lossy_string();
                    let mut random = Random::new(seed ^ hash(&path));
                    (
                        page.revision.clone(),
                        analysis(&mut random, &path, &self.paths, 12),
                    )
                })
                .collect();
            let borrowed: Vec<(&str, &PageIndex)> = analyses
                .iter()
                .map(|(revision, analysis)| (revision.as_str(), analysis))
                .collect();
            self.index.put_many(&borrowed)?;
        }
    }

    /// Rewrites `path` so it needs analysis again, as an autosave does.
    pub fn save(&self, path: &str, text: &str) -> Result<()> {
        std::fs::write(self.root.path().join("notebook").join(path), text)?;
        self.index.invalidate();
        Ok(())
    }

    /// Opens `path` with unsaved analysis generated from `seed`.
    pub fn open(&self, path: &str, seed: u32) -> Result<()> {
        let mut random = Random::new(seed ^ hash(path));
        self.index.set_overlay(
            &JsString::from(page_id_for_path(path).as_str()),
            &Overlay {
                path: path.into(),
                title: title_of(path),
                index: analysis(&mut random, path, &self.paths, 12),
            },
        )
    }

    pub fn page(&self, path: &str) -> String {
        page_id_for_path(path)
    }
}

fn hash(text: &str) -> u32 {
    text.bytes().fold(2_166_136_261u32, |hash, byte| {
        (hash ^ byte as u32).wrapping_mul(16_777_619)
    })
}
