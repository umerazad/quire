//! Derives query-index rows from page text with the editor's own Markdown analysis, so indexed
//! queries and the open editor can never disagree about blocks, references, or tags.

use crate::{
    markdown_blocks::{self, Block},
    page_analysis::analyze_inline,
};
use gpui::App;
use language::BufferSnapshot;
use quire_core::{json::JsString, markdown::frontmatter_tags};
use quire_store::index::{IndexedBlock, IndexedLink, IndexedTag, PageIndex, tag_key};
use std::{collections::HashSet, future::Future};

/// Bump whenever analysis output changes; the index then rebuilds itself.
pub const ANALYZER_VERSION: i64 = 2;

/// The editor's view of stored text: every line break becomes `\n`. Unpaired surrogates become
/// U+FFFD, which keeps UTF-16 offsets unchanged.
pub fn editor_text(text: &JsString) -> String {
    let text = text.to_lossy_string();
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text
    }
}

/// Analyzes one page. Parsing happens in the returned future, which belongs on the background
/// executor. Returns None if the Markdown grammar is unavailable.
pub fn analyze(
    text: String,
    cx: &mut App,
) -> impl Future<Output = Option<PageIndex>> + Send + use<> {
    let snapshot = crate::markdown::detached_snapshot(&text, cx);
    async move { index_snapshot(&snapshot.await, &text) }
}

fn block_at(blocks: &[IndexedBlock], position: u32) -> Option<&IndexedBlock> {
    let index = blocks.partition_point(|block| block.from <= position);
    let block = blocks.get(index.checked_sub(1)?)?;
    (position <= block.to).then_some(block)
}

pub fn index_snapshot(snapshot: &BufferSnapshot, text: &str) -> Option<PageIndex> {
    let utf16 = |offset: usize| snapshot.text.offset_to_offset_utf16(offset).0 as u32;
    let blocks: Vec<IndexedBlock> = markdown_blocks::blocks(snapshot)?
        .iter()
        .map(|block: &Block| IndexedBlock {
            key: block.key.clone(),
            id: block.id.clone(),
            from: utf16(block.from),
            to: utf16(block.content_to),
            content_from: utf16(block.content_from),
            content_to: utf16(block.content_to),
            subtree_to: utf16(block.subtree_to),
            depth: block.depth as u32,
            kind: block.kind.into(),
            parent: block.parent.clone(),
            text: markdown_blocks::block_text(snapshot, block),
        })
        .collect();
    let inline = analyze_inline(snapshot)?;
    let links = inline
        .references
        .iter()
        .map(|reference| {
            let from = utf16(reference.range.start);
            IndexedLink {
                kind: reference.kind.as_str().into(),
                syntax: reference.syntax.as_str().into(),
                target: reference.target.clone(),
                label: reference.label.clone(),
                media: reference.media.map(|media| media.as_str().into()),
                from,
                to: utf16(reference.range.end),
                text: block_at(&blocks, from).map_or_else(
                    || text[reference.range.clone()].to_owned(),
                    |block| block.text.clone(),
                ),
            }
        })
        .collect();
    let mut seen = HashSet::new();
    let tagged = inline
        .tags
        .iter()
        .filter_map(|tag| {
            let from = utf16(tag.range.start);
            let block = block_at(&blocks, from);
            seen.insert((tag_key(&tag.text), block.map_or(from, |block| block.from)))
                .then(|| IndexedTag {
                    tag: tag.text.clone(),
                    from: block.map_or(from, |block| block.content_from),
                    block_id: block.and_then(|block| block.id.clone()),
                    snippet: block.map_or_else(|| tag.text.clone(), |block| block.text.clone()),
                })
        })
        .collect();
    Some(PageIndex {
        blocks,
        links,
        tagged,
        front_tags: frontmatter_tags(text),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use serde_json::{Value, json};

    fn rows(index: &PageIndex) -> Value {
        json!({
            "blocks": index.blocks.iter().map(|b| json!({"key":b.key,"id":b.id,"from":b.from,"to":b.to,"contentFrom":b.content_from,"contentTo":b.content_to,"subtreeTo":b.subtree_to,"depth":b.depth,"kind":b.kind,"parent":b.parent,"text":b.text})).collect::<Vec<_>>(),
            "links": index.links.iter().map(|l| json!({"kind":l.kind,"syntax":l.syntax,"target":l.target,"label":l.label,"media":l.media,"from":l.from,"to":l.to,"text":l.text})).collect::<Vec<_>>(),
            "tagged": index.tagged.iter().map(|t| json!({"tag":t.tag,"from":t.from,"blockId":t.block_id,"snippet":t.snippet})).collect::<Vec<_>>(),
            "frontTags": index.front_tags,
        })
    }

    #[gpui::test]
    async fn detached_analysis_matches_the_frozen_page_index_vectors(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let vectors: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/page-index-vectors.json"
        ))
        .unwrap();
        // Known native block-boundary gaps (NR10.8). This list may only shrink: a listed vector
        // that starts matching, or a new divergence, fails the test.
        let known: std::collections::BTreeSet<String> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/page-index-block-divergences.json"
        ))
        .unwrap();
        let mut failures = Vec::new();
        let mut diverged = std::collections::BTreeSet::new();
        for vector in &vectors {
            let name = vector["name"].as_str().unwrap().to_owned();
            let source = JsString::from(vector["source"].as_str().unwrap());
            let text = editor_text(&source);
            let work = cx.update(|cx| analyze(text, cx));
            let actual = rows(&cx.background_executor.spawn(work).await.unwrap());
            let expected = json!({
                "blocks": vector["blocks"], "links": vector["links"],
                "tagged": vector["tagged"], "frontTags": vector["frontTags"],
            });
            if actual["blocks"] != expected["blocks"] {
                diverged.insert(name);
            } else if actual != expected {
                failures.push(format!("{name}\nexpected={expected}\nactual={actual}"));
            }
        }
        assert!(
            failures.is_empty(),
            "links, tags, or front tags differ despite matching blocks:\n{}",
            failures.join("\n\n")
        );
        assert_eq!(
            diverged.difference(&known).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "new block divergences"
        );
        assert_eq!(
            known.difference(&diverged).collect::<Vec<_>>(),
            Vec::<&String>::new(),
            "fixed divergences: remove them from page-index-block-divergences.json"
        );
    }
}
