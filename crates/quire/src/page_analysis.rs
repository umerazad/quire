use gpui::{App, AppContext, Context, Entity, Subscription, Task};
use language::{Buffer, BufferEvent, BufferSnapshot};
use quire_core::markdown::Reference;
use std::sync::Arc;

pub struct ParsedBlocks {
    pub snapshot: BufferSnapshot,
    pub blocks: Arc<[crate::markdown_blocks::Block]>,
}

pub struct PageAnalysis {
    pub source: Entity<Buffer>,
    parsed: Option<ParsedBlocks>,
    references: Option<(BufferSnapshot, Arc<[Reference]>)>,
    reference_task: Option<(BufferSnapshot, Task<()>)>,
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
            parsed: None,
            references: None,
            reference_task: None,
            _subscription: subscription,
        };
        this.refresh(cx);
        this
    }

    pub fn current(&self, cx: &App) -> Option<&ParsedBlocks> {
        let parsed = self.parsed.as_ref()?;
        let source = self.source.read(cx);
        let snapshot = source.snapshot();
        // Blocks come from the pinned grammar over these bytes, independently of highlighting.
        same_source(&snapshot, &parsed.snapshot).then_some(parsed)
    }

    pub fn current_references(&self, cx: &App) -> Option<&Arc<[Reference]>> {
        let (snapshot, references) = self.references.as_ref()?;
        let source = self.source.read(cx);
        (*source.parse_status().borrow() == language::ParseStatus::Idle
            && same_syntax(snapshot, &source.snapshot()))
        .then_some(references)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.current(cx).is_none() {
            let snapshot = self.source.read(cx).snapshot();
            let blocks = crate::markdown_blocks::current_blocks(&snapshot);
            self.parsed = blocks.map(|blocks| ParsedBlocks {
                snapshot,
                blocks: blocks.into(),
            });
        }
        self.refresh_references(cx);
        cx.notify();
    }

    fn refresh_references(&mut self, cx: &mut Context<Self>) {
        if self.current_references(cx).is_some() {
            return;
        }
        self.references = None;
        let source = self.source.read(cx);
        if *source.parse_status().borrow() != language::ParseStatus::Idle
            || source
                .language()
                .is_none_or(|language| language.name().as_ref() != "Markdown")
        {
            self.reference_task = None;
            return;
        }
        let snapshot = source.snapshot();
        if self
            .reference_task
            .as_ref()
            .is_some_and(|(pending, _)| same_syntax(pending, &snapshot))
        {
            return;
        }
        self.reference_task = None;
        let input = snapshot.clone();
        let work = cx.background_spawn(async move { analyze_references(&input) });
        let pending = snapshot.clone();
        let task = cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                if let Some(references) = result {
                    this.finish_references(snapshot, references, cx);
                } else if this
                    .reference_task
                    .as_ref()
                    .is_some_and(|(pending, _)| same_syntax(pending, &snapshot))
                {
                    this.reference_task = None;
                }
            })
            .ok();
        });
        self.reference_task = Some((pending, task));
    }

    fn finish_references(
        &mut self,
        snapshot: BufferSnapshot,
        references: Arc<[Reference]>,
        cx: &mut Context<Self>,
    ) {
        let source = self.source.read(cx);
        if *source.parse_status().borrow() != language::ParseStatus::Idle
            || !same_syntax(&snapshot, &source.snapshot())
        {
            return;
        }
        self.references = Some((snapshot, references));
        self.reference_task = None;
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

fn analyze_references(snapshot: &BufferSnapshot) -> Option<Arc<[Reference]>> {
    let mut code = Vec::new();
    let mut primary = false;
    for layer in snapshot.syntax_layers().filter(|layer| {
        matches!(
            layer.language.name().as_ref(),
            "Markdown" | "Markdown-Inline"
        )
    }) {
        primary |= layer.language.name().as_ref() == "Markdown";
        let root = layer.node();
        if root.has_changes() {
            return None;
        }
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
    primary.then(|| quire_core::markdown::references(&snapshot.text(), &code).into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::{Value, json};

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
        let (old_snapshot, old_references) = cx.update(|cx| {
            (
                source.read(cx).snapshot(),
                page.read(cx).current_references(cx).unwrap().clone(),
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
            assert!(!Arc::ptr_eq(&old_references, &references));
            references
        });
        page.update(cx, |page, cx| {
            page.finish_references(old_snapshot, old_references, cx)
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
            (source.read(cx).snapshot(), references)
        });
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit([(2..5, "مرحبا🙂")], None, cx);
        });
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        page.update(cx, |page, cx| {
            page.finish_references(old.0.clone(), old.1.clone(), cx)
        });
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        cx.executor().run_until_parked();
        let current = cx.update(|cx| {
            let references = page.read(cx).current_references(cx).unwrap().clone();
            assert_eq!(references.len(), 1);
            assert_eq!(references[0].target, "مرحبا🙂");
            references
        });
        page.update(cx, |page, cx| page.finish_references(old.0, old.1, cx));
        cx.update(|cx| {
            assert!(Arc::ptr_eq(
                &current,
                page.read(cx).current_references(cx).unwrap()
            ))
        });
        let before_language_change = cx.update(|cx| (source.read(cx).snapshot(), current));
        source.update(cx, |source, cx| source.set_language(None, cx));
        cx.update(|cx| assert!(page.read(cx).current_references(cx).is_none()));
        page.update(cx, |page, cx| {
            page.finish_references(before_language_change.0, before_language_change.1, cx)
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
            let mut start = previous
                .bytes()
                .zip(text.bytes())
                .take_while(|(a, b)| a == b)
                .count();
            while !previous.is_char_boundary(start) || !text.is_char_boundary(start) {
                start -= 1;
            }
            let mut suffix = previous[start..]
                .bytes()
                .rev()
                .zip(text[start..].bytes().rev())
                .take_while(|(a, b)| a == b)
                .count();
            while !previous.is_char_boundary(previous.len() - suffix)
                || !text.is_char_boundary(text.len() - suffix)
            {
                suffix -= 1;
            }
            incremental.update(cx, |buffer, cx| {
                buffer.set_sync_parse_timeout(None);
                buffer.edit(
                    [(
                        start..previous.len() - suffix,
                        &text[start..text.len() - suffix],
                    )],
                    None,
                    cx,
                );
            });
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
