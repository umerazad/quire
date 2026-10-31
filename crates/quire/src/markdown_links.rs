use crate::references::Target;
use language::BufferSnapshot;
use quire_core::markdown::{Reference, ReferenceKind};
use std::ops::Range;

/// Where a rendered link goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Destination {
    Web(String),
    Reference(Target),
}

pub struct Link {
    pub range: Range<usize>,
    pub label_range: Range<usize>,
    pub block_range: Range<usize>,
    pub label: String,
    pub destination: Destination,
    pub source: String,
}

/// An unlabeled block reference inside other text, shown as the text of the block it names.
pub struct Preview {
    pub range: Range<usize>,
    pub block_range: Range<usize>,
    pub target: String,
    pub source: String,
}

pub fn links(
    snapshot: &BufferSnapshot,
    blocks: &[crate::markdown_blocks::Block],
) -> Option<Vec<Link>> {
    let primary = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")?;
    let primary = primary.node();
    let mut links = Vec::new();
    for layer in snapshot
        .syntax_layers()
        .filter(|layer| layer.language.name().as_ref() == "Markdown-Inline")
    {
        let root = layer.node();
        if root.has_changes() {
            return None;
        }
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            if matches!(node.kind(), "image" | "code_span" | "html_tag") {
                continue;
            }
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if node.kind() != "inline_link" {
                stack.extend(children.into_iter().rev());
                continue;
            }
            let range = node.byte_range();
            let Some(block) = blocks
                .get(blocks.partition_point(|block| block.content_to <= range.start))
                .filter(|block| {
                    block.kind != "code"
                        && block.content_from <= range.start
                        && range.end <= block.content_to
                })
            else {
                continue;
            };
            let mut ancestor = primary.descendant_for_byte_range(range.start, range.end);
            let mut code = false;
            while let Some(node) = ancestor {
                if matches!(node.kind(), "fenced_code_block" | "indented_code_block") {
                    code = true;
                    break;
                }
                ancestor = node.parent();
            }
            if code {
                continue;
            }
            let Some(label) = children.iter().find(|node| node.kind() == "link_text") else {
                continue;
            };
            let Some(destination) = children
                .iter()
                .find(|node| node.kind() == "link_destination")
            else {
                continue;
            };
            if label.named_child_count() != 0 || destination.named_child_count() != 0 {
                continue;
            }
            let label_range = label.byte_range();
            let label: String = snapshot.text_for_range(label_range.clone()).collect();
            if label.is_empty() || label.contains(['\n', '\r']) {
                continue;
            }
            let destination: String = snapshot.text_for_range(destination.byte_range()).collect();
            let url = destination
                .strip_prefix('<')
                .and_then(|url| url.strip_suffix('>'))
                .unwrap_or(&destination);
            let Ok(parsed) = http_client::Url::parse(url) else {
                continue;
            };
            if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
                continue;
            }
            links.push(Link {
                source: snapshot.text_for_range(range.clone()).collect(),
                range,
                label_range,
                block_range: block.from..block.content_to,
                label,
                destination: Destination::Web(url.to_owned()),
            });
        }
    }
    links.sort_by_key(|link| link.range.start);
    Some(links)
}

/// Whether a reference stands alone on its line, after at most a list marker and before at
/// most a block ID, so it shows the whole embedded block or page there.
pub fn alone_on_line(text: &str, range: &Range<usize>) -> bool {
    let line_start = text[..range.start].rfind('\n').map_or(0, |at| at + 1);
    let line_end = text[range.end..]
        .find('\n')
        .map_or(text.len(), |at| range.end + at);
    let before = text[line_start..range.start].trim_matches([' ', '\t']);
    let before_ok = before.is_empty()
        || matches!(before, "-" | "+" | "*")
        || before
            .strip_suffix(['.', ')'])
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    let after = text[range.end..line_end].trim_matches([' ', '\t']);
    let after_ok = after.is_empty()
        || after.strip_prefix('^').is_some_and(|id| {
            !id.is_empty()
                && id
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        });
    before_ok && after_ok
}

/// Whether a reference shows a whole block or page in place, as the reference `fullEmbed`.
pub fn is_full_embed(text: &str, reference: &Reference) -> bool {
    !crate::media::is_media_embed(reference)
        && (reference.kind == ReferenceKind::Embed
            || (reference.kind == ReferenceKind::Block && reference.label.is_none()))
        && alone_on_line(text, &reference.range)
}

/// Rendered page and labeled block links, and previews of unlabeled block references, for the
/// references outside full embeds. `references` comes from the same snapshot.
pub fn reference_links(
    snapshot: &BufferSnapshot,
    blocks: &[crate::markdown_blocks::Block],
    references: &[Reference],
) -> (Vec<Link>, Vec<Preview>) {
    let text = snapshot.text();
    let mut links = Vec::new();
    let mut previews = Vec::new();
    for reference in references {
        if is_full_embed(&text, reference) {
            continue;
        }
        let Some(target) = Target::of(reference) else {
            continue;
        };
        let range = reference.range.clone();
        let Some(block) = blocks
            .get(blocks.partition_point(|block| block.content_to <= range.start))
            .filter(|block| block.content_from <= range.start && range.end <= block.content_to)
        else {
            continue;
        };
        let source = text[range.clone()].to_owned();
        let block_range = block.from..block.content_to;
        let opening = source.find(reference.target.as_str()).unwrap_or(2);
        let label_range = match &reference.label {
            Some(label) => {
                let start = range.start + opening + reference.target.len() + 1;
                start..start + label.len()
            }
            None if matches!(target, Target::Page(_)) => {
                let start = range.start + opening;
                start..start + reference.target.len()
            }
            None => {
                previews.push(Preview {
                    range,
                    block_range,
                    target: reference.target.clone(),
                    source,
                });
                continue;
            }
        };
        if label_range.is_empty() {
            continue;
        }
        links.push(Link {
            label: text[label_range.clone()].to_owned(),
            range,
            label_range,
            block_range,
            destination: Destination::Reference(target),
            source,
        });
    }
    (links, previews)
}

#[cfg(test)]
mod tests {
    use gpui::TestAppContext;

    #[gpui::test]
    fn links_follow_inline_syntax_and_exclude_metadata_code_and_unsupported_forms(
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "---\ntitle: [Metadata](https://ignored.example)\n---\n\n`[Code](https://ignored.example)`\n\n![Image](https://ignored.example)\n\n[Rich **label**](https://ignored.example) [Local](./page) [Script](javascript:alert)\n\n- Parent\n  ```md\n  [Nested code](https://ignored.example)\n  ```\n  - [مرحبا🙂](https://example.com/child)\n\n[Angle](<https://example.com/angle>) [Title](https://example.com/title \"title\")\n";
        let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.executor().run_until_parked();
        let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
        let links = super::links(
            &snapshot,
            &crate::markdown_blocks::blocks(&snapshot).unwrap(),
        )
        .unwrap();
        let actual: Vec<_> = links
            .iter()
            .map(|link| match &link.destination {
                super::Destination::Web(url) => (link.label.as_str(), url.as_str()),
                super::Destination::Reference(target) => panic!("unexpected {target:?}"),
            })
            .collect();
        assert_eq!(
            actual,
            [
                ("مرحبا🙂", "https://example.com/child"),
                ("Angle", "https://example.com/angle"),
                ("Title", "https://example.com/title")
            ]
        );
        for link in links {
            assert_eq!(&text[link.range], link.source);
        }
        let label = format!("{}🙂", "a".repeat(512));
        let text = format!("[{label}](https://example.com/long)");
        let buffer = cx.update(|cx| crate::markdown::buffer(&text, cx));
        cx.executor().run_until_parked();
        let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
        assert_eq!(
            super::links(
                &snapshot,
                &crate::markdown_blocks::blocks(&snapshot).unwrap()
            )
            .unwrap()[0]
                .label,
            label
        );
    }

    #[gpui::test]
    fn references_render_as_labels_previews_or_full_embeds(cx: &mut TestAppContext) {
        use super::{Destination, Target};
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "- see [[Plan]], [[Plan|the plan]], ((x)), ((y|why)) `[[code]]`\n- ((whole)) ^id\n1. {{embed: [[Page]]}}\n- ![[Also]] trailing\n";
        let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.executor().run_until_parked();
        let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
        let blocks = crate::markdown_blocks::blocks(&snapshot).unwrap();
        let references = crate::page_analysis::references(&snapshot).unwrap();
        let (links, previews) = super::reference_links(&snapshot, &blocks, &references);
        let links: Vec<_> = links
            .iter()
            .map(|link| (link.label.as_str(), link.destination.clone()))
            .collect();
        let page = |name: &str| Destination::Reference(Target::Page(name.into()));
        assert_eq!(
            links,
            [
                ("Plan", page("Plan")),
                ("the plan", page("Plan")),
                ("why", Destination::Reference(Target::Block("y".into()))),
                ("Also", page("Also")),
            ]
        );
        let previews: Vec<_> = previews
            .iter()
            .map(|preview| preview.target.as_str())
            .collect();
        assert_eq!(previews, ["x"], "((whole)) and the page embed stand alone");
        let full: Vec<_> = references
            .iter()
            .filter(|reference| super::is_full_embed(text, reference))
            .map(|reference| reference.target.as_str())
            .collect();
        assert_eq!(full, ["whole", "Page"]);
    }
}
