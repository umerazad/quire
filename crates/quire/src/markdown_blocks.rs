use language::{BufferSnapshot, ToTreeSitterPoint};
use std::ops::Range;

#[derive(Debug, PartialEq, Eq)]
pub struct Block {
    pub key: String,
    pub id: Option<String>,
    pub from: usize,
    pub content_from: usize,
    pub content_to: usize,
    pub subtree_to: usize,
    pub depth: usize,
    pub kind: &'static str,
    pub parent: Option<String>,
}

pub struct SyntaxSpan {
    pub marker: Range<usize>,
    pub id: Option<Range<usize>>,
}

pub fn syntax_spans(snapshot: &BufferSnapshot, blocks: &[Block]) -> Vec<SyntaxSpan> {
    blocks
        .iter()
        .filter_map(|block| {
            if block.kind != "list" && block.id.is_none() {
                return None;
            }
            let marker = if block.kind == "list" {
                block.from..block.content_from
            } else {
                block.content_from..block.content_from
            };
            let id = block.id.as_ref().and_then(|_| {
                let content: String = snapshot
                    .text_for_range(block.content_from..block.content_to)
                    .collect();
                id_tail(&content).map(|(range, _)| {
                    range.start + block.content_from..range.end + block.content_from
                })
            });
            Some(SyntaxSpan { marker, id })
        })
        .collect()
}

fn needs_eof_terminator(snapshot: &BufferSnapshot) -> bool {
    let point = snapshot.text.max_point();
    if point.column == 0 {
        return false;
    }
    let mut offset = snapshot
        .text
        .point_to_offset(language::Point::new(point.row, 0));
    let mut chars = snapshot.chars_at(offset).peekable();
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
        offset += 1;
    }
    match chars.next() {
        Some('-' | '+' | '*') => offset += 1,
        Some(c) if c.is_ascii_digit() => {
            offset += 1;
            let mut digits = 1;
            while chars.peek().is_some_and(char::is_ascii_digit) {
                chars.next();
                offset += 1;
                digits += 1;
            }
            if digits > 9 || !matches!(chars.next(), Some('.' | ')')) {
                return false;
            }
            offset += 1;
        }
        _ => return false,
    }
    if chars.peek().is_some_and(|c| !matches!(c, ' ' | '\t')) {
        return false;
    }
    while matches!(chars.peek(), Some(' ' | '\t')) {
        chars.next();
        offset += 1;
    }
    if chars.peek().is_none() {
        return true;
    }
    if !matches!(chars.peek(), Some('[' | '{')) {
        return false;
    }
    let prefix: String = snapshot
        .chars_at(offset)
        .take(crate::task_marker::PREFIX_CHAR_LIMIT)
        .collect();
    crate::task_marker::read(&prefix).is_some_and(|task| {
        snapshot
            .chars_at(offset + task.source.len())
            .all(|c| matches!(c, ' ' | '\t'))
    })
}

fn eof_tree(
    snapshot: &BufferSnapshot,
    layer: &language::SyntaxLayer<'_>,
) -> Option<language::Tree> {
    parse_tree(snapshot, Some(layer), true)
}

fn parse_tree(
    snapshot: &BufferSnapshot,
    layer: Option<&language::SyntaxLayer<'_>>,
    normalize_eof: bool,
) -> Option<language::Tree> {
    let mut tree = if let Some(layer) = layer {
        let owned = layer.to_owned();
        if owned.offset.0 != 0 {
            return None;
        }
        Some(owned.into_tree())
    } else {
        None
    };
    let len = snapshot.text.len();
    let point = snapshot.text.max_point().to_ts_point();
    // A newline alone can turn a nested empty bullet into a setext underline.
    // The virtual body forces list syntax; all returned ranges stay in real source.
    if normalize_eof && let Some(tree) = &mut tree {
        tree.edit(&tree_sitter::InputEdit {
            start_byte: len,
            old_end_byte: len,
            new_end_byte: len + 2,
            start_position: point,
            old_end_position: point,
            new_end_position: tree_sitter::Point::new(point.row + 1, 0),
        });
    }
    let mut parser = language::Parser::new();
    parser
        .set_language(&tree_sitter_quire_markdown::LANGUAGE.into())
        .ok()?;
    let mut chunks = snapshot.text.as_rope().chunks_in_range(0..len);
    parser.parse_with_options(
        &mut |offset, _| {
            if offset >= len {
                if normalize_eof {
                    b"x\n".get(offset - len..).unwrap_or_default()
                } else {
                    &[]
                }
            } else {
                chunks.seek(offset);
                chunks.peek_bytes().unwrap_or_default()
            }
        },
        tree.as_ref(),
        None,
    )
}

pub fn id_tail(text: &str) -> Option<(Range<usize>, &str)> {
    let tail = text.trim_end_matches([' ', '\t']);
    let caret = tail.rfind('^')?;
    if caret > 0 && !matches!(tail.as_bytes()[caret - 1], b' ' | b'\t') {
        return None;
    }
    let id = &tail[caret + 1..];
    if id.is_empty()
        || !id
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
    {
        return None;
    }
    let mut start = caret;
    while start > 0 && matches!(text.as_bytes()[start - 1], b' ' | b'\t') {
        start -= 1;
    }
    Some((start..text.len(), id))
}

fn js_space(c: char) -> bool {
    c.is_whitespace() && c != '\u{85}' || c == '\u{feff}'
}

fn trim_end(snapshot: &BufferSnapshot, start: usize, end: usize, whitespace: bool) -> usize {
    let removed: usize = snapshot
        .reversed_chars_at(end)
        .take_while(|c| {
            if whitespace {
                js_space(*c)
            } else {
                matches!(c, '\r' | '\n')
            }
        })
        .map(char::len_utf8)
        .sum();
    end.saturating_sub(removed).max(start)
}

pub fn blocks(snapshot: &BufferSnapshot) -> Option<Vec<Block>> {
    let layer = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")?;
    if layer.node().has_changes() {
        return None;
    }
    let normalized = if needs_eof_terminator(snapshot) {
        Some(eof_tree(snapshot, &layer)?)
    } else {
        None
    };
    let root = normalized
        .as_ref()
        .map_or_else(|| layer.node(), language::Tree::root_node);
    Some(collect_blocks(snapshot, root, normalized.is_some()))
}

pub fn current_blocks(snapshot: &BufferSnapshot) -> Option<Vec<Block>> {
    if snapshot
        .language()
        .is_none_or(|language| language.name().as_ref() != "Markdown")
    {
        return None;
    }
    if let Some(blocks) = blocks(snapshot) {
        return Some(blocks);
    }
    let layer = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown");
    let normalized = needs_eof_terminator(snapshot);
    let tree = parse_tree(snapshot, layer.as_ref(), normalized)?;
    Some(collect_blocks(snapshot, tree.root_node(), normalized))
}

fn collect_blocks(
    snapshot: &BufferSnapshot,
    root: language::Node<'_>,
    normalized: bool,
) -> Vec<Block> {
    let source_len = snapshot.text.len();
    let mut stack = vec![(root, 0, None::<String>)];
    let frontmatter = quire_core::markdown::frontmatter_length_chars(snapshot.chars_at(0));
    let mut result = Vec::<Block>::new();
    let mut merge_paragraph = false;
    while let Some((node, depth, parent)) = stack.pop() {
        if node.start_byte() >= source_len || node.end_byte() <= frontmatter {
            continue;
        }
        let mut cursor = node.walk();
        let children: Vec<_> = node.named_children(&mut cursor).collect();
        if matches!(node.kind(), "document" | "section" | "list") {
            stack.extend(
                children
                    .into_iter()
                    .rev()
                    .map(|child| (child, depth, parent.clone())),
            );
            continue;
        }
        if node.kind() == "thematic_break" {
            merge_paragraph = false;
            continue;
        }
        let kind = match node.kind() {
            "list_item" => "list",
            "atx_heading" => "heading",
            "fenced_code_block" | "indented_code_block" => "code",
            "block_quote" => "quote",
            _ => "paragraph",
        };
        let mut from = node.start_byte();
        if node.kind() == "indented_code_block" {
            let mut column = 0;
            for c in snapshot.chars_at(from) {
                if column >= 4 {
                    break;
                }
                match c {
                    ' ' => column += 1,
                    '\t' => column += 4 - column % 4,
                    _ => break,
                }
                from += c.len_utf8();
            }
        } else if kind == "paragraph" {
            from += snapshot
                .chars_at(from)
                .take_while(|c| matches!(c, ' ' | '\t'))
                .map(char::len_utf8)
                .sum::<usize>();
        }
        if kind == "paragraph"
            && from == '\u{feff}'.len_utf8()
            && frontmatter == 0
            && snapshot.chars_at(0).next() == Some('\u{feff}')
        {
            from = 0;
        }
        let mut content_from = from;
        let nested: Vec<_> = children
            .iter()
            .filter(|child| child.kind() == "list")
            .copied()
            .collect();
        if kind == "list"
            && let Some(marker) = children
                .iter()
                .find(|child| child.kind().starts_with("list_marker_"))
        {
            from = marker.start_byte();
            content_from = marker.end_byte().min(source_len);
            content_from += snapshot
                .chars_at(content_from)
                .take_while(|c| matches!(c, ' ' | '\t'))
                .map(char::len_utf8)
                .sum::<usize>();
        }
        let setext_rule = node.kind() == "setext_heading"
            && children.iter().any(|child| {
                if child.kind() != "setext_h2_underline" {
                    return false;
                }
                snapshot
                    .text_for_range(child.start_byte()..child.end_byte().min(source_len))
                    .flat_map(str::bytes)
                    .filter(|c| *c == b'-')
                    .count()
                    >= 3
            });
        let end = if setext_rule {
            children
                .iter()
                .find(|child| child.kind() == "paragraph")
                .map_or(node.end_byte(), |child| child.end_byte())
        } else {
            node.end_byte()
        };
        let subtree_to = trim_end(snapshot, from, end.min(source_len), false);
        let content_to = trim_end(
            snapshot,
            content_from,
            if kind == "list" {
                nested.first().map_or(subtree_to, |node| node.start_byte())
            } else {
                subtree_to
            },
            kind == "list",
        );
        let id = if matches!(kind, "code" | "paragraph") {
            None
        } else {
            let content: String = snapshot.text_for_range(content_from..content_to).collect();
            id_tail(&content).map(|(_, id)| id.to_string())
        };
        let key = id
            .clone()
            .unwrap_or_else(|| format!("at-{}", snapshot.text.offset_to_offset_utf16(from).0));
        // An empty item cannot interrupt a paragraph, even though the virtual body can.
        if normalized
            && kind == "list"
            && content_from == source_len
            && let Some(previous) = result.last_mut()
            && previous.kind == "paragraph"
            && previous.depth == depth
            && previous.parent == parent
        {
            let gap: String = snapshot.text_for_range(previous.content_to..from).collect();
            if gap.bytes().filter(|c| *c == b'\n').count() == 1
                && gap
                    .bytes()
                    .all(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
            {
                previous.content_to = source_len;
                previous.subtree_to = source_len;
                continue;
            }
        }
        if kind == "list" {
            stack.extend(
                nested
                    .into_iter()
                    .rev()
                    .map(|node| (node, depth + 1, Some(key.clone()))),
            );
        }
        if merge_paragraph
            && matches!(node.kind(), "paragraph" | "setext_heading")
            && let Some(previous) = result.last_mut()
            && previous.kind == "paragraph"
            && previous.depth == depth
            && previous.parent == parent
        {
            let gap: String = snapshot.text_for_range(previous.content_to..from).collect();
            if gap.bytes().filter(|c| *c == b'\n').count() == 1
                && gap
                    .bytes()
                    .all(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
            {
                previous.content_to = content_to;
                previous.subtree_to = subtree_to;
                merge_paragraph = !setext_rule;
                continue;
            }
        }
        merge_paragraph = node.kind() == "setext_heading" && !setext_rule;
        result.push(Block {
            key,
            id,
            from,
            content_from,
            content_to,
            subtree_to,
            depth,
            kind,
            parent,
        });
    }
    for block in &mut result {
        if block.kind == "paragraph" {
            let content: String = snapshot
                .text_for_range(block.content_from..block.content_to)
                .collect();
            block.id = id_tail(&content).map(|(_, id)| id.to_string());
            block.key = block.id.clone().unwrap_or_else(|| {
                format!("at-{}", snapshot.text.offset_to_offset_utf16(block.from).0)
            });
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use serde_json::{Value, json};

    #[gpui::test]
    fn current_block_analysis_matches_clean_parser_without_mutating_syntax(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let vectors: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/vectors.json"
        ))
        .unwrap();
        for vector in vectors {
            let buffer = cx.update(|cx| crate::markdown::buffer("", cx));
            cx.executor().run_until_parked();
            let text = vector["text"].as_str().unwrap();
            buffer.update(cx, |buffer, cx| {
                buffer.set_sync_parse_timeout(None);
                buffer.edit([(0..0, text)], None, cx);
            });
            let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
            let syntax = || {
                snapshot
                    .syntax_layers()
                    .map(|layer| {
                        let node = layer.node();
                        (
                            node.to_sexp(),
                            node.start_byte(),
                            node.end_byte(),
                            node.has_changes(),
                        )
                    })
                    .collect::<Vec<_>>()
            };
            let before = syntax();
            let current = current_blocks(&snapshot).unwrap();
            assert_eq!(
                syntax(),
                before,
                "derived analysis must leave shared syntax unchanged"
            );
            assert_eq!(snapshot.text(), text);
            cx.executor().run_until_parked();
            let clean = cx.update(|cx| buffer.read(cx).snapshot());
            assert_eq!(current, blocks(&clean).unwrap(), "{}", vector["name"]);
        }
    }

    #[gpui::test]
    fn native_blocks_match_frozen_markdown_vectors(cx: &mut TestAppContext) {
        cx.update(|cx| super::super::editor_host::init(cx).expect("editor initialization"));
        let vectors: Vec<Value> = serde_json::from_str(include_str!(
            "../../../tests/fixtures/markdown/vectors.json"
        ))
        .unwrap();
        let mut failures = Vec::new();
        for vector in vectors {
            let buffer =
                cx.update(|cx| crate::markdown::buffer(vector["source"].as_str().unwrap(), cx));
            cx.executor().run_until_parked();
            let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
            assert_eq!(snapshot.text(), vector["text"].as_str().unwrap());
            let native_blocks = blocks(&snapshot).unwrap();
            assert!(
                !snapshot
                    .syntax_layers()
                    .find(|layer| layer.language.name().as_ref() == "Markdown")
                    .unwrap()
                    .node()
                    .has_changes()
            );
            let spans: Vec<_> = syntax_spans(&snapshot, &native_blocks).into_iter().map(|span| {
                let to_utf16 = |offset| snapshot.text.offset_to_offset_utf16(offset).0;
                json!({"marker":[to_utf16(span.marker.start),to_utf16(span.marker.end)],"id":span.id.map(|id| [to_utf16(id.start),to_utf16(id.end)])})
            }).collect();
            if json!(spans) != vector["spans"] {
                let layer = snapshot
                    .syntax_layers()
                    .find(|layer| layer.language.name().as_ref() == "Markdown")
                    .unwrap();
                failures.push(format!(
                    "{} spans expected={} actual={} normalized={:?}",
                    vector["name"],
                    vector["spans"],
                    json!(spans),
                    eof_tree(&snapshot, &layer).map(|tree| tree.root_node().to_sexp())
                ));
            }
            let actual: Vec<_> = native_blocks.into_iter().map(|block| {
                let to_utf16 = |offset| snapshot.text.offset_to_offset_utf16(offset).0;
                let content: String = snapshot.text_for_range(block.content_from..block.content_to).collect();
                let text = id_tail(&content).map_or(content.as_str(), |(range, _)| &content[..range.start]).trim_end_matches(js_space);
                let mut value = json!({"key":block.key,"from":to_utf16(block.from),"to":to_utf16(block.content_to),"contentFrom":to_utf16(block.content_from),"contentTo":to_utf16(block.content_to),"subtreeTo":to_utf16(block.subtree_to),"depth":block.depth,"kind":block.kind,"text":text});
                if let Some(id) = block.id { value["id"] = json!(id); }
                if let Some(parent) = block.parent { value["parent"] = json!(parent); }
                value
            }).collect();
            if json!(actual) != vector["blocks"] {
                let tree = snapshot
                    .syntax_layers()
                    .find(|layer| layer.language.name().as_ref() == "Markdown")
                    .unwrap()
                    .node()
                    .to_sexp();
                failures.push(format!(
                    "{}\nexpected={}\nactual={}\ntree={tree}",
                    vector["name"],
                    vector["blocks"],
                    json!(actual)
                ));
            }
        }
        assert!(failures.is_empty(), "{}", failures.join("\n\n"));
    }
}
