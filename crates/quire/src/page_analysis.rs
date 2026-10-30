use gpui::{App, Context, Entity, Subscription};
use language::{Buffer, BufferEvent, BufferSnapshot};
use std::sync::Arc;

pub struct ParsedBlocks {
    pub snapshot: BufferSnapshot,
    pub blocks: Arc<[crate::markdown_blocks::Block]>,
}

pub struct PageAnalysis {
    pub source: Entity<Buffer>,
    parsed: Option<ParsedBlocks>,
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
        (snapshot.version() == parsed.snapshot.version()
            && snapshot
                .language()
                .zip(parsed.snapshot.language())
                .is_some_and(|(a, b)| Arc::ptr_eq(a, b)))
        .then_some(parsed)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.current(cx).is_some() {
            return;
        }
        let source = self.source.read(cx);
        let snapshot = source.snapshot();
        let blocks = crate::markdown_blocks::current_blocks(&snapshot);
        self.parsed = blocks.map(|blocks| ParsedBlocks {
            snapshot,
            blocks: blocks.into(),
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};
    use serde_json::{Value, json};

    #[gpui::test]
    fn native_grammar_reference_exclusions_match_the_frozen_oracle(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let cases: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/reference-vectors.json"
        ))
        .unwrap();
        let mut failures = Vec::new();
        let incremental = cx.update(|cx| crate::markdown::buffer("", cx));
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
            for (mode, buffer) in [("fresh", &fresh), ("incremental", &incremental)] {
                cx.executor().run_until_parked();
                let snapshot = cx.update(|cx| {
                    let buffer = buffer.read(cx);
                    assert_eq!(*buffer.parse_status().borrow(), language::ParseStatus::Idle);
                    buffer.snapshot()
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
                let actual: Vec<_> = quire_core::markdown::references(text, &code)
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
