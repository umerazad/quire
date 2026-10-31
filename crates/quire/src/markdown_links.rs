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

/// Inline nodes whose text is never a rendered web link: code, HTML, images, and the link forms
/// Write leaves as source.
const OPAQUE: [&str; 7] = [
    "image",
    "code_span",
    "html_tag",
    "email_autolink",
    "full_reference_link",
    "collapsed_reference_link",
    "shortcut_link",
];

/// Plain-label inline links, `<...>` autolinks, and bare URLs to HTTP(S) addresses, outside
/// code and metadata. `None` while the inline parse is stale.
pub fn links(
    snapshot: &BufferSnapshot,
    blocks: &[crate::markdown_blocks::Block],
) -> Option<Vec<Link>> {
    let primary = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")?;
    let primary = primary.node();
    let block_of = |range: &Range<usize>| {
        let block = blocks
            .get(blocks.partition_point(|block| block.content_to <= range.start))
            .filter(|block| {
                block.kind != "code"
                    && block.content_from <= range.start
                    && range.end <= block.content_to
            })?;
        let mut ancestor = primary.descendant_for_byte_range(range.start, range.end);
        while let Some(node) = ancestor {
            if matches!(node.kind(), "fenced_code_block" | "indented_code_block") {
                return None;
            }
            ancestor = node.parent();
        }
        Some(block.from..block.content_to)
    };
    let web_link = |range: Range<usize>, label_range: Range<usize>, url: &str| {
        let block_range = block_of(&range)?;
        let parsed = http_client::Url::parse(url).ok()?;
        if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
            return None;
        }
        Some(Link {
            source: snapshot.text_for_range(range.clone()).collect(),
            label: snapshot.text_for_range(label_range.clone()).collect(),
            range,
            label_range,
            block_range,
            destination: Destination::Web(url.to_owned()),
        })
    };
    let mut links = Vec::new();
    for layer in snapshot
        .syntax_layers()
        .filter(|layer| layer.language.name().as_ref() == "Markdown-Inline")
    {
        let root = layer.node();
        if root.has_changes() {
            return None;
        }
        let mut covered = Vec::new();
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let range = node.byte_range();
            if OPAQUE.contains(&node.kind()) {
                covered.push(range);
                continue;
            }
            if node.kind() == "uri_autolink" {
                covered.push(range.clone());
                let inner = range.start + 1..range.end - 1;
                let url: String = snapshot.text_for_range(inner.clone()).collect();
                links.extend(web_link(range, inner, &url));
                continue;
            }
            let mut cursor = node.walk();
            let children: Vec<_> = node.named_children(&mut cursor).collect();
            if node.kind() != "inline_link" {
                stack.extend(children.into_iter().rev());
                continue;
            }
            covered.push(range.clone());
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
            links.extend(web_link(range, label_range, url));
        }
        let start = root.start_byte();
        let text: String = snapshot.text_for_range(root.byte_range()).collect();
        for url in bare_urls(&text) {
            let range = start + url.start..start + url.end;
            if !covered
                .iter()
                .any(|covered| covered.start < range.end && range.start < covered.end)
            {
                links.extend(web_link(range.clone(), range, &text[url]));
            }
        }
    }
    links.sort_by_key(|link| link.range.start);
    links.dedup_by_key(|link| link.range.clone());
    Some(links)
}

/// Bare `http://` and `https://` addresses in prose, as GitHub Flavored Markdown's extended
/// autolinks find them: at a line start or after a space or one of `*_~(`, up to a space or `<`,
/// without trailing punctuation, quotes, or an unmatched closing bracket.
pub fn bare_urls(text: &str) -> Vec<Range<usize>> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(at) = text[from..].find("http").map(|at| from + at) {
        from = at + "http".len();
        let rest = &text[at..];
        let Some(scheme) = ["https://", "http://"]
            .into_iter()
            .find(|scheme| rest.starts_with(scheme))
        else {
            continue;
        };
        let after_boundary = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace() || matches!(c, '*' | '_' | '~' | '('));
        if !after_boundary {
            continue;
        }
        let mut end = at
            + rest
                .find(|c: char| c.is_whitespace() || c == '<')
                .unwrap_or(rest.len());
        while let Some(last) = text[at..end].chars().next_back() {
            let url = &text[at..end];
            let unmatched = |open, close| {
                last == close && url.matches(close).count() > url.matches(open).count()
            };
            let trailing = matches!(
                last,
                '?' | '!' | '.' | ',' | ':' | ';' | '*' | '_' | '~' | '\'' | '"'
            );
            if !(trailing || unmatched('(', ')') || unmatched('[', ']')) {
                break;
            }
            end -= last.len_utf8();
        }
        if end > at + scheme.len() {
            found.push(at..end);
            from = end;
        }
    }
    found
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

    #[test]
    fn bare_urls_end_before_trailing_punctuation_and_unmatched_brackets() {
        let found = |text: &'static str| -> Vec<&'static str> {
            super::bare_urls(text)
                .into_iter()
                .map(|range| &text[range])
                .collect()
        };
        assert_eq!(
            found(
                "see https://example.org/path, (http://a.example/x_(y)) and \"https://quoted.example\" (https://q.example/?a=1)."
            ),
            [
                "https://example.org/path",
                "http://a.example/x_(y)",
                "https://q.example/?a=1"
            ]
        );
        assert_eq!(
            found(
                "xhttps://glued.example *https://em.example* https://مثال.example/🙂 https:// ftp://no.example"
            ),
            ["https://em.example", "https://مثال.example/🙂"]
        );
        assert_eq!(
            found("[x https://in.example] https://end.example<b>"),
            ["https://in.example", "https://end.example"]
        );
    }

    #[gpui::test]
    fn bare_urls_and_autolinks_are_web_links_outside_code_and_other_links(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "---\nsource: https://meta.example\n---\n\n- bare https://bare.example/a. and <https://auto.example> <mailto:a@b.example>\n- `https://code.example` [label](https://inline.example) ![i](https://image.example) [ref][https://ref.example]\n\n```\nhttps://fenced.example\n```\n";
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
            .map(|link| {
                (
                    &text[link.range.clone()],
                    link.label.as_str(),
                    link.destination.clone(),
                )
            })
            .collect();
        let web = |url: &str| super::Destination::Web(url.into());
        assert_eq!(
            actual,
            [
                (
                    "https://bare.example/a",
                    "https://bare.example/a",
                    web("https://bare.example/a")
                ),
                (
                    "<https://auto.example>",
                    "https://auto.example",
                    web("https://auto.example")
                ),
                (
                    "[label](https://inline.example)",
                    "label",
                    web("https://inline.example")
                ),
            ]
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
