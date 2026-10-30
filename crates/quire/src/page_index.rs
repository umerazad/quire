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
pub const ANALYZER_VERSION: i64 = 4;

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
            content: snapshot
                .text_for_range(block.content_from..block.content_to)
                .collect(),
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
        let mut failures = Vec::new();
        for vector in &vectors {
            let source = JsString::from(vector["source"].as_str().unwrap());
            let work = cx.update(|cx| analyze(editor_text(&source), cx));
            let actual = rows(&cx.background_executor.spawn(work).await.unwrap());
            let expected = json!({
                "blocks": vector["blocks"], "links": vector["links"],
                "tagged": vector["tagged"], "frontTags": vector["frontTags"],
            });
            if actual != expected {
                failures.push(format!(
                    "{} source={}\nexpected={expected}\nactual={actual}",
                    vector["name"], vector["source"]
                ));
            }
        }
        assert!(
            failures.is_empty(),
            "{} of {} page-index vectors differ from the reference parseBlocks:\n{}",
            failures.len(),
            vectors.len(),
            failures.join("\n\n")
        );
    }

    #[gpui::test]
    fn incrementally_edited_blocks_match_the_frozen_page_index_vectors(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let vectors: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/page-index-vectors.json"
        ))
        .unwrap();
        let buffer = cx.update(|cx| crate::markdown::buffer("", cx));
        let mut previous = String::new();
        let mut failures = Vec::new();
        for vector in &vectors {
            let text = editor_text(&JsString::from(vector["source"].as_str().unwrap()));
            crate::page_analysis::tests::edit_to(&buffer, &previous, &text, cx);
            cx.executor().run_until_parked();
            let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
            assert_eq!(snapshot.text(), text);
            let actual = rows(&index_snapshot(&snapshot, &text).unwrap());
            if actual["blocks"] != vector["blocks"] {
                failures.push(format!(
                    "{} after editing from {previous:?}\nexpected={}\nactual={}",
                    vector["name"], vector["blocks"], actual["blocks"]
                ));
            }
            previous = text;
        }
        assert!(
            failures.is_empty(),
            "{} of {} incrementally parsed pages differ from the reference parseBlocks:\n{}",
            failures.len(),
            vectors.len(),
            failures.join("\n\n")
        );
    }
}

/// Plans a bulk tag edit for one page using the editor's own tag recognition, so tags in code
/// and links stay. Parsing happens in the returned future.
#[cfg(test)]
pub fn edit_tags(
    text: String,
    retag: quire_core::tag_edits::Retag,
    cx: &mut App,
) -> impl Future<Output = Option<quire_core::tag_edits::PageTagEdit>> + Send + use<> {
    let snapshot = crate::markdown::detached_snapshot(&text, cx);
    async move {
        let inline = analyze_inline(&snapshot.await)?;
        quire_core::tag_edits::edit_page(&text, &inline.tags, &retag)
    }
}

#[cfg(test)]
mod tag_edit_tests {
    use super::*;
    use gpui::TestAppContext;
    use quire_core::tag_edits::{INVALID_NAME, NOTHING_CHOSEN, Retag, TagChange};

    async fn plan(
        pages: &[(&str, &str)],
        chosen: &[&str],
        rename: Option<&str>,
        cx: &mut TestAppContext,
    ) -> Vec<(String, String, Vec<TagChange>)> {
        let retag = Retag::new(chosen, rename).unwrap();
        let mut edits = Vec::new();
        for (path, text) in pages {
            let work = cx.update(|cx| edit_tags((*text).into(), retag.clone(), cx));
            if let Some(edit) = cx.background_executor.spawn(work).await {
                edits.push(((*path).to_owned(), edit.text, edit.changes));
            }
        }
        edits
    }

    fn texts(edits: Vec<(String, String, Vec<TagChange>)>) -> Vec<(String, String)> {
        edits
            .into_iter()
            .map(|(path, text, _)| (path, text))
            .collect()
    }

    #[gpui::test]
    async fn renames_nested_tags_and_merges_but_leaves_code_and_links(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let pages = [
            ("A.md", "- call #Area and #area/home\n- #areas stays\n"),
            (
                "B.md",
                "- `#area` in code and [[#area]] stay\n- #work too\n",
            ),
            ("C.md", "- nothing here\n"),
        ];
        assert_eq!(
            texts(plan(&pages, &["#area", "#work"], Some("#job"), cx).await),
            [
                (
                    "A.md".into(),
                    "- call #job and #job/home\n- #areas stays\n".into()
                ),
                (
                    "B.md".into(),
                    "- `#area` in code and [[#area]] stay\n- #job too\n".into()
                ),
            ]
        );
    }

    #[gpui::test]
    async fn deletes_only_chosen_tags_keeping_words_and_markers_apart(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let pages = [(
            "A.md",
            "- task #next\n- #next first\n- a #next b\n- #next\n- [ ] #next\n- #next/soon stays\n",
        )];
        assert_eq!(
            texts(plan(&pages, &["#next"], None, cx).await),
            [(
                "A.md".into(),
                "- task\n- first\n- a b\n- \n- [ ] \n- #next/soon stays\n".into()
            )]
        );
    }

    #[gpui::test]
    async fn rewrites_front_matter_tags_in_the_form_the_page_uses(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let pages = [
            ("Inline.md", "---\ntags: [project, area]\n---\n- text\n"),
            ("Plain.md", "---\ntags: project, area\n---\n"),
            (
                "List.md",
                "---\ntitle: List\ntags:\n  - project\n  - other\n---\n",
            ),
            ("Only.md", "---\ntags: [project]\n---\n"),
        ];
        assert_eq!(
            texts(plan(&pages, &["#project"], Some("work/project"), cx).await),
            [
                (
                    "Inline.md".into(),
                    "---\ntags: [work/project, area]\n---\n- text\n".into()
                ),
                (
                    "Plain.md".into(),
                    "---\ntags: work/project, area\n---\n".into()
                ),
                (
                    "List.md".into(),
                    "---\ntitle: List\ntags:\n  - work/project\n  - other\n---\n".into()
                ),
                ("Only.md".into(), "---\ntags: [work/project]\n---\n".into()),
            ]
        );
        assert_eq!(
            texts(plan(&pages[3..], &["#project"], None, cx).await),
            [("Only.md".into(), "---\ntags: []\n---\n".into())]
        );
    }

    #[gpui::test]
    async fn refuses_invalid_names_and_reports_each_change(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        assert_eq!(
            Retag::new(&["#old"], Some("two words")).unwrap_err(),
            INVALID_NAME
        );
        assert_eq!(
            Retag::new(&[] as &[&str], None).unwrap_err(),
            NOTHING_CHOSEN
        );
        let edits = plan(
            &[("A.md", "- #old one\n"), ("B.md", "- #old two\n")],
            &["#old"],
            Some("new"),
            cx,
        )
        .await;
        let changes: Vec<_> = edits
            .into_iter()
            .flat_map(|(path, _, changes)| {
                changes
                    .into_iter()
                    .map(move |change| (path.clone(), change))
            })
            .collect();
        let expected = |path: &str| {
            (
                path.to_owned(),
                TagChange {
                    before: "#old".into(),
                    after: "#new".into(),
                },
            )
        };
        assert_eq!(changes, [expected("A.md"), expected("B.md")]);
    }
}
