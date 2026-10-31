mod syntax;

use gpui::{App, AppContext, Context, Entity, Subscription, Task};
use language::{Buffer, BufferEvent, BufferSnapshot};
use quire_core::markdown::{Reference, TagToken};
use std::{cell::RefCell, sync::Arc};

pub struct ParsedBlocks {
    pub snapshot: BufferSnapshot,
    pub blocks: Arc<[crate::markdown_blocks::Block]>,
}

#[derive(Clone)]
pub struct InlineAnalysis {
    pub references: Arc<[Reference]>,
    pub tags: Arc<[TagToken]>,
    pub tag_summary: gpui::SharedString,
}

pub struct PageAnalysis {
    pub source: Entity<Buffer>,
    parsed: RefCell<Option<Arc<ParsedBlocks>>>,
    inline: Option<(BufferSnapshot, InlineAnalysis)>,
    inline_task: Option<(BufferSnapshot, Task<()>)>,
    _subscription: Subscription,
}

impl PageAnalysis {
    pub fn new(source: Entity<Buffer>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&source, |this, _, event, cx| {
            if matches!(
                event,
                BufferEvent::Edited { .. }
                    | BufferEvent::Reparsed
                    | BufferEvent::LanguageChanged(_)
            ) {
                this.refresh(cx);
            }
        });
        let mut this = Self {
            source,
            parsed: RefCell::new(None),
            inline: None,
            inline_task: None,
            _subscription: subscription,
        };
        this.refresh(cx);
        this
    }

    /// Computes block analysis lazily and shares it for the current source version.
    /// Metadata rendering uses current_inline instead to avoid foreground block work.
    /// Whether blocks were parsed for some version of the page.
    #[cfg(test)]
    pub fn blocks_parsed(&self) -> bool {
        self.parsed.borrow().is_some()
    }

    pub fn current(&self, cx: &App) -> Option<Arc<ParsedBlocks>> {
        let source = self.source.read(cx);
        let snapshot = source.snapshot();
        if let Some(parsed) = self.parsed.borrow().as_ref()
            && same_source(&snapshot, &parsed.snapshot)
        {
            return Some(parsed.clone());
        }
        let parsed = crate::markdown_blocks::current_blocks(&snapshot).map(|blocks| {
            Arc::new(ParsedBlocks {
                snapshot,
                blocks: blocks.into(),
            })
        });
        *self.parsed.borrow_mut() = parsed.clone();
        parsed
    }

    pub fn current_references(&self, cx: &App) -> Option<&Arc<[Reference]>> {
        self.current_inline(cx).map(|inline| &inline.references)
    }

    pub fn current_inline(&self, cx: &App) -> Option<&InlineAnalysis> {
        let (snapshot, inline) = self.inline.as_ref()?;
        let source = self.source.read(cx);
        (*source.parse_status().borrow() == language::ParseStatus::Idle
            && same_syntax(snapshot, &source.snapshot()))
        .then_some(inline)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.source.read(cx).snapshot();
        if self
            .parsed
            .get_mut()
            .as_ref()
            .is_some_and(|parsed| !same_source(&snapshot, &parsed.snapshot))
        {
            *self.parsed.get_mut() = None;
        }
        self.refresh_inline(cx);
        cx.notify();
    }

    fn refresh_inline(&mut self, cx: &mut Context<Self>) {
        if self.current_references(cx).is_some() {
            return;
        }
        self.inline = None;
        let source = self.source.read(cx);
        if *source.parse_status().borrow() != language::ParseStatus::Idle
            || source
                .language()
                .is_none_or(|language| language.name().as_ref() != "Markdown")
        {
            self.inline_task = None;
            return;
        }
        let snapshot = source.snapshot();
        if self
            .inline_task
            .as_ref()
            .is_some_and(|(pending, _)| same_syntax(pending, &snapshot))
        {
            return;
        }
        self.inline_task = None;
        let input = snapshot.clone();
        let work = cx.background_spawn(async move { analyze_inline(&input) });
        let pending = snapshot.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if let Some(inline) = result {
                    this.finish_inline(snapshot, inline, cx);
                } else if this
                    .inline_task
                    .as_ref()
                    .is_some_and(|(pending, _)| same_syntax(pending, &snapshot))
                {
                    this.inline_task = None;
                }
            })
            .ok();
        });
        self.inline_task = Some((pending, task));
    }

    fn finish_inline(
        &mut self,
        snapshot: BufferSnapshot,
        inline: InlineAnalysis,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.read(cx);
        if *source.parse_status().borrow() != language::ParseStatus::Idle
            || !same_syntax(&snapshot, &source.snapshot())
        {
            return;
        }
        self.inline = Some((snapshot, inline));
        self.inline_task = None;
        cx.notify();
    }
}

fn same_source(a: &BufferSnapshot, b: &BufferSnapshot) -> bool {
    a.version() == b.version()
        && a.language()
            .zip(b.language())
            .is_some_and(|(a, b)| Arc::ptr_eq(a, b))
}

fn same_syntax(a: &BufferSnapshot, b: &BufferSnapshot) -> bool {
    same_source(a, b) && a.syntax_update_count() == b.syntax_update_count()
}

/// What task analysis reads from the syntax tree.
pub(crate) struct TaskSyntax {
    /// Ranges where tags are not recognized.
    pub tags: Vec<std::ops::Range<usize>>,
    /// Fenced and indented code blocks, bounded as the reference parser bounds them.
    pub code_blocks: Vec<std::ops::Range<usize>>,
}

pub(crate) fn task_syntax(snapshot: &BufferSnapshot, text: &str) -> Option<TaskSyntax> {
    syntax::exclusions(snapshot, text).map(|excluded| TaskSyntax {
        tags: excluded.tags,
        code_blocks: excluded.code_blocks,
    })
}

/// The page's wiki and Roam references outside code, without tag analysis.
pub(crate) fn references(snapshot: &BufferSnapshot) -> Option<Vec<Reference>> {
    let text = snapshot.text();
    let excluded = syntax::exclusions(snapshot, &text)?;
    Some(quire_core::markdown::references(&text, &excluded.code))
}

/// References and Markdown URL ranges, the links a rename or move rewrites, outside code.
pub(crate) fn analyze_links(
    snapshot: &BufferSnapshot,
) -> Option<(Vec<Reference>, Vec<std::ops::Range<usize>>)> {
    let text = snapshot.text();
    let mut excluded = syntax::exclusions(snapshot, &text)?;
    excluded.urls.sort_by_key(|url| (url.start, url.end));
    Some((
        quire_core::markdown::references(&text, &excluded.code),
        excluded.urls,
    ))
}

pub(crate) fn analyze_inline(snapshot: &BufferSnapshot) -> Option<InlineAnalysis> {
    let text = snapshot.text();
    let excluded = syntax::exclusions(snapshot, &text)?;
    let references = quire_core::markdown::references(&text, &excluded.code);
    let tags = quire_core::markdown::tags(&text, &excluded.tags, &references);
    let mut inline = InlineAnalysis {
        references: references.into(),
        tags: tags.into(),
        tag_summary: gpui::SharedString::default(),
    };
    let mut seen = std::collections::HashSet::new();
    let labels: Vec<_> = inline
        .tags
        .iter()
        .filter_map(|tag| seen.insert(tag.text.as_str()).then_some(tag.text.as_str()))
        .collect();
    inline.tag_summary = if labels.is_empty() {
        gpui::SharedString::default()
    } else {
        format!("Inline tags: {}", labels.join(" · ")).into()
    };
    Some(inline)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::{Value, json};

    #[gpui::test]
    fn inline_metadata_is_atomic_current_and_does_not_eagerly_parse_blocks(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let before = "#old [[Old]] `prefix #hidden`";
        let after = "#new [[New]] `prefix #hidden`";
        let source = cx.update(|cx| crate::markdown::buffer(before, cx));
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        cx.update(|cx| assert!(page.read(cx).current_inline(cx).is_none()));
        cx.executor().run_until_parked();
        let (old_snapshot, old) = cx.update(|cx| {
            let page = page.read(cx);
            let data = page.current_inline(cx).unwrap().clone();
            assert_eq!(data.references.len(), 1);
            assert_eq!(data.references[0].target, "Old");
            assert_eq!(data.tags.len(), 1);
            assert_eq!(data.tags[0].text, "#old");
            assert!(page.parsed.borrow().is_none());
            let blocks = page.current(cx).unwrap();
            assert!(Arc::ptr_eq(&blocks, &page.current(cx).unwrap()));
            (source.read(cx).snapshot(), data)
        });
        edit_to(&source, before, after, cx);
        cx.update(|cx| assert!(page.read(cx).current_inline(cx).is_none()));
        page.update(cx, |page, cx| {
            page.finish_inline(old_snapshot, old.clone(), cx)
        });
        cx.update(|cx| assert!(page.read(cx).current_inline(cx).is_none()));
        cx.executor().run_until_parked();
        let (snapshot, current) = cx.update(|cx| {
            let page = page.read(cx);
            let data = page.current_inline(cx).unwrap().clone();
            assert_eq!(data.references[0].target, "New");
            assert_eq!(data.tags[0].text, "#new");
            assert!(!Arc::ptr_eq(&data.tags, &old.tags));
            assert!(page.parsed.borrow().is_none());
            (source.read(cx).snapshot(), data)
        });
        source.update(cx, |source, cx| source.reparse(cx, false));
        cx.update(|cx| assert!(page.read(cx).current_inline(cx).is_none()));
        cx.executor().run_until_parked();
        let ready = cx.update(|cx| page.read(cx).current_inline(cx).unwrap().clone());
        assert!(!Arc::ptr_eq(&current.tags, &ready.tags));
        page.update(cx, |page, cx| page.finish_inline(snapshot, current, cx));
        cx.update(|cx| {
            assert!(Arc::ptr_eq(
                &ready.tags,
                &page.read(cx).current_inline(cx).unwrap().tags
            ))
        });
        let (snapshot, language) = cx.update(|cx| {
            (
                source.read(cx).snapshot(),
                source.read(cx).language().unwrap().clone(),
            )
        });
        source.update(cx, |source, cx| source.set_language(None, cx));
        page.update(cx, |page, cx| page.finish_inline(snapshot, ready, cx));
        cx.executor().run_until_parked();
        cx.update(|cx| assert!(page.read(cx).current_inline(cx).is_none()));
        source.update(cx, |source, cx| source.set_language(Some(language), cx));
        cx.executor().run_until_parked();
        cx.update(|cx| {
            let data = page.read(cx).current_inline(cx).unwrap();
            assert_eq!(data.tags[0].text, "#new");
            assert_eq!(data.references[0].target, "New");
        });
    }

    pub(crate) fn edit_to(
        source: &Entity<Buffer>,
        before: &str,
        after: &str,
        cx: &mut TestAppContext,
    ) {
        let mut start = before
            .bytes()
            .zip(after.bytes())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(start) || !after.is_char_boundary(start) {
            start -= 1;
        }
        let mut suffix = before[start..]
            .bytes()
            .rev()
            .zip(after[start..].bytes().rev())
            .take_while(|(a, b)| a == b)
            .count();
        while !before.is_char_boundary(before.len() - suffix)
            || !after.is_char_boundary(after.len() - suffix)
        {
            suffix -= 1;
        }
        source.update(cx, |buffer, cx| {
            buffer.set_sync_parse_timeout(None);
            buffer.edit(
                [(
                    start..before.len() - suffix,
                    &after[start..after.len() - suffix],
                )],
                None,
                cx,
            );
        });
    }

    #[gpui::test]
    fn native_tag_exclusions_match_the_frozen_oracle(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/tag-vectors.json"
        ))
        .unwrap();
        let mut failures = Vec::new();
        let incremental = cx.update(|cx| crate::markdown::buffer("", cx));
        let incremental_page = cx.new(|cx| PageAnalysis::new(incremental.clone(), cx));
        let mut previous = String::new();
        for case in cases {
            let vector = &case["editor"];
            let text = vector["text"].as_str().unwrap();
            edit_to(&incremental, &previous, text, cx);
            previous = text.to_owned();
            let fresh = cx.update(|cx| crate::markdown::buffer(text, cx));
            let fresh_page = cx.new(|cx| PageAnalysis::new(fresh.clone(), cx));
            for (mode, source, page) in [
                ("fresh", &fresh, &fresh_page),
                ("incremental", &incremental, &incremental_page),
            ] {
                cx.executor().run_until_parked();
                let (snapshot, references, tags) = cx.update(|cx| {
                    assert_eq!(
                        *source.read(cx).parse_status().borrow(),
                        language::ParseStatus::Idle
                    );
                    (
                        source.read(cx).snapshot(),
                        page.read(cx).current_references(cx).unwrap().clone(),
                        page.read(cx).current_inline(cx).unwrap().tags.clone(),
                    )
                });
                assert_eq!(snapshot.text(), text);
                let excluded = syntax::exclusions(&snapshot, text).unwrap().tags;
                let actual: Vec<_> = tags
                    .iter()
                    .map(|tag| {
                        json!({
                            "from": text[..tag.range.start].encode_utf16().count(),
                            "to": text[..tag.range.end].encode_utf16().count(),
                            "text": tag.text,
                        })
                    })
                    .collect();
                let mut issue = (json!(actual) != vector["tags"])
                    .then(|| format!("tags expected={} actual={}", vector["tags"], json!(actual)));
                let mut bytes = vec![None; text.encode_utf16().count() + 1];
                let mut units = 0;
                for (byte, character) in text.char_indices() {
                    bytes[units] = Some(byte);
                    units += character.len_utf16();
                }
                bytes[units] = Some(text.len());
                for completion in vector["completions"].as_array().unwrap() {
                    let position = completion[0].as_u64().unwrap() as usize;
                    let actual = quire_core::markdown::tag_completion_range(
                        text,
                        bytes[position].unwrap(),
                        &excluded,
                        &references,
                    )
                    .map(|range| {
                        json!([
                            text[..range.start].encode_utf16().count(),
                            text[..range.end].encode_utf16().count(),
                        ])
                    })
                    .unwrap_or(Value::Null);
                    if actual != completion[1] {
                        issue.get_or_insert_with(String::new).push_str(&format!(
                            " completion cursor={position} expected={} actual={actual}",
                            completion[1]
                        ));
                        break;
                    }
                }
                if let Some(issue) = issue {
                    let trees: Vec<_> = snapshot
                        .syntax_layers()
                        .filter(|layer| {
                            matches!(
                                layer.language.name().as_ref(),
                                "Markdown" | "Markdown-Inline"
                            )
                        })
                        .map(|layer| {
                            format!("{} {}", layer.language.name(), layer.node().to_sexp())
                        })
                        .collect();
                    failures.push(format!(
                        "{mode} {} {issue} excluded={excluded:?} trees={trees:?}",
                        case["name"]
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} native tag mismatches:\n{}",
            failures.len(),
            failures
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[gpui::test]
    fn pending_reference_work_follows_rapid_edits_and_releases_closed_pages(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer("[[Obsolete]]", cx));
        cx.executor().run_until_parked();
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit([(0..12, "```md\n[[Obsolete]]\n```\n")], None, cx);
            let end = source.len();
            source.edit([(end..end, "\n[[Final🙂]]")], None, cx);
        });
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        cx.executor().run_until_parked();
        cx.update(|cx| {
            let references = page.read(cx).current_references(cx).unwrap();
            assert_eq!(references.len(), 1);
            assert_eq!(references[0].target, "Final🙂");
        });
        source.update(cx, |source, cx| {
            source.edit([(0..0, "[[Later]]\n")], None, cx);
        });
        let weak = page.downgrade();
        drop(page);
        cx.executor().run_until_parked();
        assert!(weak.upgrade().is_none());
        let reopened = cx.new(|cx| PageAnalysis::new(source, cx));
        cx.executor().run_until_parked();
        cx.update(|cx| {
            let references = reopened.read(cx).current_references(cx).unwrap();
            assert_eq!(references.len(), 2);
            assert_eq!(references[0].target, "Later");
            assert_eq!(references[1].target, "Final🙂");
        });
    }

    #[gpui::test]
    fn reference_results_include_the_native_syntax_generation(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer("[[Page]] `[[Code]]`", cx));
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        cx.executor().run_until_parked();
        let (old_snapshot, old_inline) = cx.update(|cx| {
            (
                source.read(cx).snapshot(),
                page.read(cx).current_inline(cx).unwrap().clone(),
            )
        });
        source.update(cx, |source, cx| source.reparse(cx, false));
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        cx.executor().run_until_parked();
        let current = cx.update(|cx| {
            let snapshot = source.read(cx).snapshot();
            assert_eq!(snapshot.version(), old_snapshot.version());
            assert_ne!(
                snapshot.syntax_update_count(),
                old_snapshot.syntax_update_count()
            );
            let references = page.read(cx).current_references(cx).unwrap().clone();
            assert!(!Arc::ptr_eq(&old_inline.references, &references));
            references
        });
        page.update(cx, |page, cx| {
            page.finish_inline(old_snapshot, old_inline, cx)
        });
        page.update(cx, |page, cx| page.refresh(cx));
        cx.executor().run_until_parked();
        cx.update(|cx| {
            assert!(Arc::ptr_eq(
                &current,
                page.read(cx).current_references(cx).unwrap()
            ))
        });
    }

    #[gpui::test]
    fn reference_results_follow_current_source_and_reject_late_completions(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer("[[Old]] `[[Hidden]]`", cx));
        cx.executor().run_until_parked();
        let markdown = cx.update(|cx| source.read(cx).language().unwrap().clone());
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        cx.executor().run_until_parked();
        let old = cx.update(|cx| {
            let page = page.read(cx);
            let references = page.current_references(cx).unwrap().clone();
            assert_eq!(references.len(), 1);
            assert_eq!(references[0].target, "Old");
            (
                source.read(cx).snapshot(),
                page.current_inline(cx).unwrap().clone(),
            )
        });
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit([(2..5, "مرحبا🙂")], None, cx);
        });
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        page.update(cx, |page, cx| {
            page.finish_inline(old.0.clone(), old.1.clone(), cx)
        });
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        cx.executor().run_until_parked();
        let current = cx.update(|cx| {
            let references = page.read(cx).current_references(cx).unwrap().clone();
            assert_eq!(references.len(), 1);
            assert_eq!(references[0].target, "مرحبا🙂");
            references
        });
        page.update(cx, |page, cx| page.finish_inline(old.0, old.1, cx));
        cx.update(|cx| {
            assert!(Arc::ptr_eq(
                &current,
                page.read(cx).current_references(cx).unwrap()
            ))
        });
        let before_language_change = cx.update(|cx| {
            (
                source.read(cx).snapshot(),
                page.read(cx).current_inline(cx).unwrap().clone(),
            )
        });
        source.update(cx, |source, cx| source.set_language(None, cx));
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        page.update(cx, |page, cx| {
            page.finish_inline(before_language_change.0, before_language_change.1, cx)
        });
        cx.executor().run_until_parked();
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        source.update(cx, |source, cx| source.set_language(Some(markdown), cx));
        cx.executor().run_until_parked();
        cx.update(|cx| {
            assert_eq!(
                page.read(cx).current_references(cx).unwrap()[0].target,
                "مرحبا🙂"
            )
        });
    }

    #[gpui::test]
    fn native_grammar_reference_exclusions_match_the_frozen_oracle(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/reference-vectors.json"
        ))
        .unwrap();
        let mut failures = Vec::new();
        let incremental = cx.update(|cx| crate::markdown::buffer("", cx));
        let incremental_page = cx.new(|cx| PageAnalysis::new(incremental.clone(), cx));
        let mut previous = String::new();
        for case in cases {
            let vector = &case["editor"];
            let text = vector["text"].as_str().unwrap();
            edit_to(&incremental, &previous, text, cx);
            previous = text.to_owned();
            let fresh = cx.update(|cx| crate::markdown::buffer(text, cx));
            let fresh_page = cx.new(|cx| PageAnalysis::new(fresh.clone(), cx));
            for (mode, buffer, page) in [
                ("fresh", &fresh, &fresh_page),
                ("incremental", &incremental, &incremental_page),
            ] {
                cx.executor().run_until_parked();
                let (snapshot, references) = cx.update(|cx| {
                    let buffer = buffer.read(cx);
                    assert_eq!(*buffer.parse_status().borrow(), language::ParseStatus::Idle);
                    (
                        buffer.snapshot(),
                        page.read(cx).current_references(cx).unwrap().clone(),
                    )
                });
                assert_eq!(snapshot.text(), text);
                let mut code = Vec::new();
                for layer in snapshot.syntax_layers().filter(|layer| {
                    matches!(
                        layer.language.name().as_ref(),
                        "Markdown" | "Markdown-Inline"
                    )
                }) {
                    let root = layer.node();
                    assert!(!root.has_changes());
                    let mut stack = vec![root];
                    while let Some(node) = stack.pop() {
                        if matches!(
                            node.kind(),
                            "fenced_code_block" | "indented_code_block" | "code_span"
                        ) {
                            code.push(node.byte_range());
                            continue;
                        }
                        let mut cursor = node.walk();
                        stack.extend(node.named_children(&mut cursor));
                    }
                }
                let actual: Vec<_> = references
                    .iter()
                    .map(|reference| {
                        let mut value = json!({
                            "kind": reference.kind.as_str(),
                            "syntax": reference.syntax.as_str(),
                            "target": reference.target,
                            "from": text[..reference.range.start].encode_utf16().count(),
                            "to": text[..reference.range.end].encode_utf16().count(),
                        });
                        if let Some(label) = &reference.label {
                            value["label"] = label.clone().into();
                        }
                        if let Some(media) = reference.media {
                            value["media"] = media.as_str().into();
                        }
                        value
                    })
                    .collect();
                if json!(actual) != vector["references"] {
                    let trees = snapshot
                        .syntax_layers()
                        .filter(|layer| {
                            matches!(
                                layer.language.name().as_ref(),
                                "Markdown" | "Markdown-Inline"
                            )
                        })
                        .map(|layer| {
                            format!("{} {}", layer.language.name(), layer.node().to_sexp())
                        })
                        .collect::<Vec<_>>();
                    failures.push(format!(
                        "{mode} {} expected={} actual={} native_code={:?} trees={:?}",
                        case["name"],
                        vector["references"],
                        json!(actual),
                        code,
                        trees
                    ));
                }
            }
        }
        assert!(
            failures.is_empty(),
            "{} native reference mismatches:\n{}",
            failures.len(),
            failures
                .iter()
                .take(8)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        );
    }

    #[test]
    fn quire_and_upstream_markdown_policies_coexist() {
        let text = "heading\n===\n";
        let parse = |language: tree_sitter::Language| {
            let mut parser = tree_sitter::Parser::new();
            parser.set_language(&language).unwrap();
            parser.parse(text, None).unwrap().root_node().to_sexp()
        };
        assert!(parse(tree_sitter_md::LANGUAGE.into()).contains("setext_heading"));
        let quire = parse(tree_sitter_quire_markdown::LANGUAGE.into());
        assert!(quire.contains("paragraph"));
        assert!(!quire.contains("setext_heading"));
        assert!(!quire.contains("ERROR"));
    }

    #[gpui::test]
    fn background_completion_reuses_the_shared_current_analysis(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source =
            cx.update(|cx| crate::markdown::buffer("- Parent ^parent\n  - child ^child\n", cx));
        cx.executor().run_until_parked();
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        let original = cx.update(|cx| page.read(cx).current(cx).unwrap().blocks.clone());
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit([(0..0, "مرحبا🙂\n\n")], None, cx);
        });
        let current = cx.update(|cx| {
            let parsed = page.read(cx).current(cx).unwrap();
            assert_eq!(
                parsed.snapshot.version(),
                source.read(cx).snapshot().version()
            );
            assert!(!Arc::ptr_eq(&original, &parsed.blocks));
            parsed.blocks.clone()
        });
        cx.executor().run_until_parked();
        cx.update(|cx| {
            assert!(Arc::ptr_eq(
                &current,
                &page.read(cx).current(cx).unwrap().blocks
            ));
        });
    }
}
