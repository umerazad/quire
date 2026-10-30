use language::BufferSnapshot;
use std::ops::Range;

pub struct Link {
    pub range: Range<usize>,
    pub label: String,
    pub url: String,
    pub source: String,
}

pub fn links(snapshot: &BufferSnapshot) -> Option<Vec<Link>> {
    let blocks = crate::markdown_blocks::blocks(snapshot)?;
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
            let block = blocks.get(blocks.partition_point(|block| block.content_to <= range.start));
            if !block.is_some_and(|block| {
                block.kind != "code"
                    && block.content_from <= range.start
                    && range.end <= block.content_to
            }) {
                continue;
            }
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
            let label: String = snapshot.text_for_range(label.byte_range()).collect();
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
                label,
                url: url.to_owned(),
            });
        }
    }
    links.sort_by_key(|link| link.range.start);
    Some(links)
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
        let links = super::links(&snapshot).unwrap();
        let actual: Vec<_> = links
            .iter()
            .map(|link| (link.label.as_str(), link.url.as_str()))
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
        assert_eq!(super::links(&snapshot).unwrap()[0].label, label);
    }
}
