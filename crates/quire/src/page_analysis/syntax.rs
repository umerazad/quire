use language::{BufferSnapshot, ToTreeSitterPoint};
use quire_core::markdown::{angle_autolink_end, bare_url_end};
use std::ops::Range;

#[derive(Default)]
pub(super) struct Exclusions {
    pub code: Vec<Range<usize>>,
    pub tags: Vec<Range<usize>>,
    #[cfg(test)]
    pub inline_reparses: usize,
}

#[derive(Clone, Copy)]
enum Kind {
    Code,
    Excluded,
    Opaque,
    InvalidAutolink,
}

struct Span {
    range: Range<usize>,
    kind: Kind,
}

fn html_kind(source: &str) -> Kind {
    let source = source.trim_start_matches([' ', '\t']);
    if source.starts_with("<!--") || source.starts_with("<?") {
        Kind::Opaque
    } else {
        Kind::Excluded
    }
}

fn inline_spans(tree: &language::Tree, text: &str) -> Vec<Span> {
    let mut result = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        let range = node.byte_range();
        let kind = match node.kind() {
            "code_span" => Some(Kind::Code),
            "inline_link"
            | "full_reference_link"
            | "collapsed_reference_link"
            | "shortcut_link"
            | "image"
            | "backslash_escape" => Some(Kind::Excluded),
            "html_tag" => Some(html_kind(&text[range.clone()])),
            "uri_autolink" | "email_autolink" => Some(
                if angle_autolink_end(text, range.start) == Some(range.end) {
                    Kind::Excluded
                } else {
                    Kind::InvalidAutolink
                },
            ),
            _ => None,
        };
        if let Some(kind) = kind {
            result.push(Span { range, kind });
        } else {
            let mut cursor = node.walk();
            stack.extend(node.named_children(&mut cursor));
        }
    }
    result.sort_unstable_by_key(|span| span.range.start);
    result
}

fn subtract(ranges: &[Range<usize>], holes: &[Range<usize>]) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    for range in ranges {
        let mut start = range.start;
        for hole in holes
            .iter()
            .filter(|hole| hole.start < range.end && hole.end > range.start)
        {
            if start < hole.start {
                result.push(start..hole.start);
            }
            start = start.max(hole.end);
        }
        if start < range.end {
            result.push(start..range.end);
        }
    }
    result
}

fn inline_exclusions(
    snapshot: &BufferSnapshot,
    source: &str,
    layer: &language::SyntaxLayer<'_>,
    result: &mut Exclusions,
) -> Option<()> {
    let owned = layer.to_owned();
    let base = owned.offset.0;
    let base_point = snapshot.text.offset_to_point(base);
    let original = owned.into_tree();
    let extent = original.root_node().end_byte();
    let text = &source[base..base + extent];
    let ranges: Vec<_> = original
        .included_ranges()
        .iter()
        .map(|range| range.start_byte..range.end_byte.min(extent))
        .filter(|range| range.start < range.end)
        .collect();
    let mut tree = Some(original);
    let mut spans = inline_spans(tree.as_ref()?, text);
    let mut parser = None;
    let mut holes = Vec::new();
    let mut position = 0;
    while position < extent {
        let Some(range) = ranges.get(ranges.partition_point(|range| range.end <= position)) else {
            break;
        };
        position = position.max(range.start);
        if position >= extent {
            break;
        }
        let index = spans.partition_point(|span| span.range.start <= position);
        let span = index.checked_sub(1).and_then(|index| spans.get(index));
        if let Some(span) = span
            && position < span.range.end
            && !matches!(span.kind, Kind::InvalidAutolink)
        {
            position = span.range.end;
            continue;
        }
        let available = &text[..range.end];
        let angle = angle_autolink_end(available, position);
        let invalid_opener = span.is_some_and(|span| {
            span.range.start == position && matches!(span.kind, Kind::InvalidAutolink)
        });
        let end = angle
            .or_else(|| bare_url_end(available, position, false))
            .or_else(|| invalid_opener.then_some(position + 1));
        if let Some(end) = end {
            let first = spans.partition_point(|span| span.range.end <= position);
            let needs_reparse = spans[first..]
                .iter()
                .take_while(|span| span.range.start < end)
                .any(|span| {
                    span.range.start < position
                        || end < span.range.end
                        || matches!(span.kind, Kind::InvalidAutolink)
                });
            if angle.is_some() {
                result.tags.push(base + position..base + end);
            }
            holes.push(position..end);
            position = end;
            if !needs_reparse {
                continue;
            }
            let remaining = subtract(&ranges, &holes);
            if remaining.is_empty() {
                tree = None;
                break;
            }
            let included: Vec<_> = remaining
                .into_iter()
                .map(|range| tree_sitter::Range {
                    start_byte: range.start,
                    end_byte: range.end,
                    start_point: (snapshot.text.offset_to_point(base + range.start) - base_point)
                        .to_ts_point(),
                    end_point: (snapshot.text.offset_to_point(base + range.end) - base_point)
                        .to_ts_point(),
                })
                .collect();
            if parser.is_none() {
                let mut inline = language::Parser::new();
                inline
                    .set_language(&tree_sitter_quire_markdown::INLINE_LANGUAGE.into())
                    .ok()?;
                parser = Some(inline);
            }
            let parser = parser.as_mut()?;
            parser.set_included_ranges(&included).ok()?;
            tree = Some(parser.parse(text, tree.as_ref())?);
            #[cfg(test)]
            {
                result.inline_reparses += 1;
            }
            spans = inline_spans(tree.as_ref()?, text);
        } else {
            position += text[position..].chars().next()?.len_utf8();
        }
    }
    if let Some(tree) = tree {
        for span in inline_spans(&tree, text) {
            let index = holes.partition_point(|hole| hole.start <= span.range.start);
            if index > 0 && span.range.start < holes[index - 1].end {
                continue;
            }
            let range = base + span.range.start..base + span.range.end;
            match span.kind {
                Kind::Code => {
                    result.code.push(range.clone());
                    result.tags.push(range);
                }
                Kind::Excluded => result.tags.push(range),
                Kind::Opaque | Kind::InvalidAutolink => {}
            }
        }
    }
    Some(())
}

pub(super) fn exclusions(snapshot: &BufferSnapshot, text: &str) -> Option<Exclusions> {
    let mut result = Exclusions::default();
    let mut primary = false;
    for layer in snapshot
        .syntax_layers()
        .filter(|layer| layer.language.name().as_ref() == "Markdown")
    {
        primary = true;
        let root = layer.node();
        if root.has_changes() {
            return None;
        }
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let range = node.byte_range();
            match node.kind() {
                "fenced_code_block" | "indented_code_block" => {
                    result.code.push(range.clone());
                    result.tags.push(range);
                }
                "html_block" => {
                    if matches!(html_kind(&text[range.clone()]), Kind::Excluded) {
                        result.tags.push(range);
                    }
                }
                "link_reference_definition" => result.tags.push(range),
                _ => {
                    let mut cursor = node.walk();
                    stack.extend(node.named_children(&mut cursor));
                }
            }
        }
    }
    if !primary {
        return None;
    }
    for layer in snapshot
        .syntax_layers()
        .filter(|layer| layer.language.name().as_ref() == "Markdown-Inline")
    {
        let root = layer.node();
        if root.has_changes() {
            return None;
        }
        if result
            .code
            .iter()
            .any(|range| range.start <= root.start_byte() && root.end_byte() <= range.end)
        {
            continue;
        }
        inline_exclusions(snapshot, text, &layer, &mut result)?;
    }
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn ordinary_urls_reuse_syntax_and_boundary_repairs_scale_by_affected_paragraphs(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        for count in [20, 100] {
            for (source, reparses) in [
                ("https://example.test/[#tag] ".repeat(count), 0),
                (
                    "https://example.test/`code #one` now `#two`\n\n".repeat(count),
                    count,
                ),
            ] {
                let buffer = cx.update(|cx| crate::markdown::buffer(&source, cx));
                cx.executor().run_until_parked();
                let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
                let excluded = exclusions(&snapshot, &source).unwrap();
                assert_eq!(excluded.inline_reparses, reparses);
                let tags = quire_core::markdown::tags(&source, &excluded.tags, &[]);
                assert_eq!(tags.len(), count);
                assert!(
                    tags.iter()
                        .all(|tag| tag.text == if reparses == 0 { "#tag" } else { "#one" })
                );
            }
        }
    }
}
