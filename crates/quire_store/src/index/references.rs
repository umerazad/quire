//! Linked references for a page: the referring blocks, their filter contexts, and suggested
//! filter facets. Ported from rt `42de3c8949e1` (`src/ui/LinkedReferences.tsx` `referenceBlocks`
//! and `orderReferences`, `src/core/reference-filter.ts`, `src/core/projects.ts`).

use super::{
    PageRef, QueryIndex,
    query::{PAGE_COLUMNS, PageRow, page_ref},
    strip_markdown,
};
use crate::sync::json::JsString;
use anyhow::Result;
use quire_core::reference_filter::{FilterNames, ReferenceContext, ReferenceFilter};
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashMap, HashSet},
};

/// One referring block, once per block, with what filters test against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReferenceBlock {
    pub source: PageRef,
    /// The block's content start, or the link itself when no block holds it.
    pub from: u32,
    pub block_id: Option<String>,
    pub snippet: String,
    pub context: ReferenceContext,
    pub outline: Outline,
}

/// Where a block sits, for showing it with its surroundings as the reference tag and
/// reference results do.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Outline {
    /// The block's parents, outermost first, at most four.
    pub ancestors: Vec<String>,
    /// The first blocks nested under it, with their depth below it (1 for a direct child).
    pub children: Vec<(u32, String)>,
    /// How many more nested blocks follow `children`.
    pub more_children: usize,
}

/// Nested blocks listed under a block before the rest are counted instead.
pub const SHOWN_CHILDREN: usize = 8;
const SHOWN_ANCESTORS: usize = 4;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FacetKind {
    Tag,
    Page,
    Project,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Facet {
    pub kind: FacetKind,
    pub label: String,
    /// The filter term a click adds.
    pub term: String,
    pub count: usize,
}

struct Block {
    key: String,
    parent: Option<String>,
    from: u32,
    content_from: u32,
    content_to: u32,
    subtree_to: u32,
    id: Option<String>,
    content: String,
}

/// The rows of one referring page that contexts read.
struct Page {
    blocks: Vec<Block>,
    /// Block positions by key, for walking up to ancestors.
    by_key: HashMap<String, usize>,
    links: Vec<(String, u32)>,
    tags: Vec<(String, u32)>,
    front_tags: Vec<String>,
}

fn block_at(blocks: &[Block], position: u32) -> Option<&Block> {
    let index = blocks.partition_point(|block| block.from <= position);
    let block = blocks.get(index.checked_sub(1)?)?;
    (position <= block.content_to).then_some(block)
}

/// The names a page is linked by: its title and its path without `.md`.
fn page_names(page: &PageRef) -> [String; 2] {
    [
        page.title.to_lowercase(),
        strip_markdown(&page.path).to_lowercase(),
    ]
}

/// A block's parents (nearest four, outermost first) and its first nested blocks with their
/// depth below it, plus how many nested blocks are left out.
fn outline(page: &Page, block: &Block, shown: usize) -> Outline {
    let parent = |item: &Block| Some(&page.blocks[*page.by_key.get(item.parent.as_deref()?)?]);
    let mut ancestors = Vec::new();
    let mut up = parent(block);
    while let Some(ancestor) = up.filter(|_| ancestors.len() < SHOWN_ANCESTORS) {
        ancestors.insert(0, ancestor.content.clone());
        up = parent(ancestor);
    }
    let depth = |item: &Block| {
        let mut depth = 0;
        let mut up = Some(item);
        while let Some(current) = up.filter(|current| current.key != block.key) {
            depth += 1;
            up = parent(current);
        }
        depth
    };
    let nested: Vec<&Block> = page
        .blocks
        .iter()
        .filter(|item| item.from > block.from && item.from < block.subtree_to)
        .collect();
    let children = nested
        .iter()
        .take(shown)
        .map(|item| (depth(item), item.content.clone()))
        .collect();
    Outline {
        ancestors,
        children,
        more_children: nested.len().saturating_sub(shown),
    }
}

impl QueryIndex {
    fn page_rows(&self, revision: &str) -> Result<Page> {
        type BlockRow = (
            (String, Option<String>, u32, u32),
            (u32, u32, Option<String>, String),
        );
        let blocks: Vec<BlockRow> = self.connection.select_bound(
            "SELECT key,parent,start,content_from,content_to,subtree_to,id,content FROM live_blocks WHERE revision=? ORDER BY ordinal",
        )?(revision)?;
        let blocks: Vec<Block> = blocks
            .into_iter()
            .map(
                |((key, parent, from, content_from), (content_to, subtree_to, id, content))| {
                    Block {
                        key,
                        parent,
                        from,
                        content_from,
                        content_to,
                        subtree_to,
                        id,
                        content,
                    }
                },
            )
            .collect();
        Ok(Page {
            by_key: blocks
                .iter()
                .enumerate()
                .map(|(index, block)| (block.key.clone(), index))
                .collect(),
            blocks,
            links: self.connection.select_bound(
                "SELECT target,start FROM live_links WHERE revision=? AND kind='page' ORDER BY ordinal",
            )?(revision)?,
            tags: self.connection.select_bound(
                "SELECT tag,start FROM live_tagged WHERE revision=? ORDER BY ordinal",
            )?(revision)?,
            front_tags: self.connection.select_bound(
                "SELECT tag FROM live_front_tags WHERE revision=? ORDER BY ordinal",
            )?(revision)?,
        })
    }

    /// The reference `referenceContexts` for one block of a page.
    fn context(source: &PageRef, page: &Page, from: u32) -> ReferenceContext {
        let mut inside: Vec<&Block> = Vec::new();
        if let Some(block) = block_at(&page.blocks, from) {
            let parent = |item: &Block| {
                let index = *page.by_key.get(item.parent.as_deref()?)?;
                Some(&page.blocks[index])
            };
            let mut up = parent(block);
            while let Some(ancestor) = up {
                inside.push(ancestor);
                up = parent(ancestor);
            }
            inside.extend(
                page.blocks
                    .iter()
                    .filter(|item| item.from >= block.from && item.from < block.subtree_to),
            );
        }
        let within = |at: u32| {
            inside
                .iter()
                .any(|item| at >= item.content_from && at < item.content_to)
        };
        let mut links: BTreeSet<String> = page_names(source).into();
        links.extend(
            page.links
                .iter()
                .filter(|(_, at)| within(*at))
                .map(|(target, _)| {
                    target
                        .split('#')
                        .next()
                        .unwrap_or_default()
                        .trim()
                        .to_lowercase()
                }),
        );
        let mut tags: BTreeSet<String> = page
            .front_tags
            .iter()
            .map(|tag| format!("#{}", tag.to_lowercase()))
            .collect();
        tags.extend(
            page.tags
                .iter()
                .filter(|(_, at)| within(*at))
                .map(|(tag, _)| tag.to_lowercase()),
        );
        let text = std::iter::once(source.title.as_str())
            .chain(inside.iter().map(|item| item.content.as_str()))
            .collect::<Vec<_>>()
            .join("\n")
            .to_lowercase();
        ReferenceContext { tags, links, text }
    }

    /// The outline of the block holding each `(revision, position)`, empty where no block does.
    pub fn outlines(&self, at: &[(String, u32)]) -> Result<Vec<Outline>> {
        let at: Vec<_> = at
            .iter()
            .map(|(revision, position)| (revision.clone(), *position, SHOWN_CHILDREN))
            .collect();
        self.outlines_with(&at)
    }

    /// The outline of the block holding each `(revision, position, shown)`, listing up to
    /// `shown` nested blocks, so a row can load more of them in place.
    pub fn outlines_with(&self, at: &[(String, u32, usize)]) -> Result<Vec<Outline>> {
        let mut pages: HashMap<String, Page> = HashMap::new();
        at.iter()
            .map(|(revision, position, shown)| {
                if !pages.contains_key(revision) {
                    pages.insert(revision.clone(), self.page_rows(revision)?);
                }
                let rows = &pages[revision];
                Ok(block_at(&rows.blocks, *position)
                    .map(|block| outline(rows, block, *shown))
                    .unwrap_or_default())
            })
            .collect()
    }

    /// The page's referring blocks, each once: daily pages newest first (or oldest first), then
    /// other pages by path, keeping block order within a page.
    pub fn linked_references(
        &self,
        page: &JsString,
        oldest_first: bool,
    ) -> Result<Vec<ReferenceBlock>> {
        let backlinks = self.backlinks(page)?;
        let mut pages: HashMap<String, Page> = HashMap::new();
        let mut seen = HashSet::new();
        let mut blocks = Vec::new();
        for link in backlinks {
            let revision = link.source.revision.clone();
            if !pages.contains_key(&revision) {
                pages.insert(revision.clone(), self.page_rows(&revision)?);
            }
            let rows = &pages[&revision];
            let block = block_at(&rows.blocks, link.link.from);
            let from = block.map_or(link.link.from, |block| block.content_from);
            if !seen.insert((revision, from)) {
                continue;
            }
            blocks.push(ReferenceBlock {
                context: Self::context(&link.source, rows, from),
                outline: block
                    .map(|block| outline(rows, block, SHOWN_CHILDREN))
                    .unwrap_or_default(),
                block_id: block.and_then(|block| block.id.clone()),
                snippet: link.link.text,
                source: link.source,
                from,
            });
        }
        let dates: HashMap<String, Option<String>> = blocks
            .iter()
            .map(|block| {
                (
                    block.source.revision.clone(),
                    self.date(&block.source.title),
                )
            })
            .collect();
        let mut ordered: Vec<(usize, ReferenceBlock)> = blocks.into_iter().enumerate().collect();
        ordered.sort_by(|(a_index, a), (b_index, b)| {
            let (left, right) = (&dates[&a.source.revision], &dates[&b.source.revision]);
            match (left, right) {
                (Some(left), Some(right)) if left != right => {
                    if oldest_first {
                        left.cmp(right)
                    } else {
                        right.cmp(left)
                    }
                }
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                _ if a.source.path == b.source.path => a_index.cmp(b_index),
                _ => self.collator.compare(&a.source.path, &b.source.path),
            }
        });
        Ok(ordered.into_iter().map(|(_, block)| block).collect())
    }

    /// The reference `projectPages`: a `#project` block, a `Projects/` title, or a front matter
    /// tag of `project` or `project/<name>`, ordered by title.
    pub fn project_pages(&self) -> Result<Vec<PageRef>> {
        let rows: Vec<PageRow> = self.connection.select(&format!(
            "SELECT {PAGE_COLUMNS} FROM live_pages p WHERE p.deleted=0 AND (
               EXISTS (SELECT 1 FROM live_tagged t WHERE t.revision=p.revision AND t.tag_key='project')
               OR p.title_key LIKE 'projects/%'
               OR EXISTS (SELECT 1 FROM live_front_tags f WHERE f.revision=p.revision
                 AND (lower(f.tag)='project' OR lower(f.tag) LIKE 'project/_%')))"
        ))?()?;
        let mut pages = rows.into_iter().map(page_ref).collect::<Result<Vec<_>>>()?;
        pages.sort_by(|a, b| self.collator.compare(&a.title, &b.title));
        Ok(pages)
    }

    /// Resolves each `[[page]]` and `project:name` term to the lowercase names that link to it.
    pub fn filter_names(&self, filter: &ReferenceFilter) -> Result<FilterNames> {
        let mut names = FilterNames::default();
        for name in filter.pages.iter().chain(&filter.not_pages) {
            let mut linked = vec![name.to_lowercase()];
            if let Some(page) = self.resolve_page(name)? {
                linked.extend(page_names(&page));
            }
            names.pages.insert(name.clone(), linked);
        }
        if !filter.projects.is_empty() || !filter.not_projects.is_empty() {
            let projects = self.project_pages()?;
            for name in filter.projects.iter().chain(&filter.not_projects) {
                let linked = projects
                    .iter()
                    .filter(|page| {
                        let title = page.title.to_lowercase();
                        &title == name || title.strip_prefix("projects/") == Some(name)
                    })
                    .flat_map(page_names)
                    .collect();
                names.projects.insert(name.clone(), linked);
            }
        }
        Ok(names)
    }

    /// The reference `referenceFacets`: the most frequent tags and linked pages among these
    /// contexts, leaving out ones in every context and the page being shown.
    pub fn reference_facets(
        &self,
        shown: &JsString,
        contexts: &[ReferenceContext],
        limit: usize,
    ) -> Result<Vec<Facet>> {
        let own: HashSet<String> = self
            .pages_where("p.page=?", &shown.to_json())?
            .first()
            .map(|page| page_names(page).into_iter().collect())
            .unwrap_or_default();
        let mut tag_count: HashMap<String, usize> = HashMap::new();
        let mut link_count: HashMap<String, usize> = HashMap::new();
        for context in contexts {
            for tag in &context.tags {
                *tag_count.entry(tag.clone()).or_default() += 1;
            }
            for link in context.links.iter().filter(|link| !own.contains(*link)) {
                *link_count.entry(link.clone()).or_default() += 1;
            }
        }
        let ranked = |counts: HashMap<String, usize>| {
            let mut ranked: Vec<_> = counts
                .into_iter()
                .filter(|(_, count)| *count < contexts.len() || contexts.len() == 1)
                .collect();
            ranked.sort_by(|a, b| {
                b.1.cmp(&a.1)
                    .then_with(|| self.collator.compare(&a.0, &b.0))
            });
            ranked
        };
        let mut facets: Vec<Facet> = ranked(tag_count)
            .into_iter()
            .take(limit)
            .map(|(tag, count)| Facet {
                kind: FacetKind::Tag,
                label: tag.clone(),
                term: tag,
                count,
            })
            .collect();
        let projects: HashSet<String> = self
            .project_pages()?
            .into_iter()
            .map(|page| page.revision)
            .collect();
        let mut seen = HashSet::new();
        for (name, count) in ranked(link_count).into_iter().take(limit * 3) {
            if seen.len() >= limit {
                break;
            }
            let Some(page) = self.resolve_page(&name)? else {
                continue;
            };
            if !seen.insert(page.revision.clone()) {
                continue;
            }
            facets.push(Facet {
                kind: if projects.contains(&page.revision) {
                    FacetKind::Project
                } else {
                    FacetKind::Page
                },
                term: format!("[[{}]]", page.title),
                label: page.title,
                count,
            });
        }
        Ok(facets)
    }
}
