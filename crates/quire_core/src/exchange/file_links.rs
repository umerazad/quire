//! Links from a page to local files, for moving attachments between a notebook and a folder.
//! A target resolves against the page's folder as media do (the reference `relativeVaultPath`),
//! so `![](img/a.png)` in `Notes/Page.md` names `Notes/img/a.png`. Inline links and images, `![[…]]`
//! embeds, and `{{pdf: …}}` embeds are found outside fenced code; web, `data:`, and other scheme
//! targets are left alone, and a `?query` or `#fragment` such as `#page=2` is kept.

use crate::markdown::{JS_SPACE, copies::fenced_code, relative_vault_path};
use regex::Regex;
use std::{ops::Range, sync::LazyLock};

static INLINE: LazyLock<Regex> = LazyLock::new(|| {
    let space = &JS_SPACE[1..JS_SPACE.len() - 1];
    Regex::new(&format!(
        r#"\]\((?:<([^<>\n]+)>|([^()<>{space}]+))(?:[ \t]+"[^"\n]*")?\)"#
    ))
    .unwrap()
});
static EMBED: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[\[([^\]|\n]+)").unwrap());
static PDF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{\{(?:pdf|\[\[pdf\]\]):[ \t]*([^}\n]+?)[ \t]*\}\}").unwrap());
static SCHEME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[A-Za-z][A-Za-z0-9+.-]*:").unwrap());

/// One target to consider: its byte range and whether it may be wrapped in `<…>`.
struct Target {
    range: Range<usize>,
    inline: bool,
}

fn targets(text: &str) -> Vec<Target> {
    let code = fenced_code(text);
    let inside_code = |at: usize| code.iter().any(|range| range.contains(&at));
    let inline = INLINE.captures_iter(text).filter_map(|captures| {
        let found = captures.get(1).or_else(|| captures.get(2))?;
        Some(Target {
            range: found.range(),
            inline: captures.get(2).is_some(),
        })
    });
    let embeds = [&*EMBED, &*PDF].into_iter().flat_map(|pattern| {
        pattern.captures_iter(text).filter_map(|captures| {
            let found = captures.get(1)?;
            let trimmed = found.as_str().trim_end();
            Some(Target {
                range: found.start()..found.start() + trimmed.len(),
                inline: false,
            })
        })
    });
    let mut found: Vec<Target> = inline
        .chain(embeds)
        .filter(|target| !inside_code(target.range.start))
        .collect();
    found.sort_by_key(|target| target.range.start);
    found
}

/// The notebook path a link target names from the page at `page`, with its `?query` or
/// `#fragment`. None for web and other scheme targets, bare fragments, and paths above the root.
pub fn local_target(target: &str, page: &str) -> Option<(String, String)> {
    if SCHEME.is_match(target) || target.starts_with("//") || target.starts_with('#') {
        return None;
    }
    let split = target.find(['?', '#']).unwrap_or(target.len());
    let (file, suffix) = target.split_at(split);
    relative_vault_path(Some(page), file)
        .filter(|path| !path.is_empty())
        .map(|path| (path, suffix.to_owned()))
}

/// Replaces each local file target in `text` (a page at `page`) for which `replace` returns a new
/// target, keeping its `?query` or `#fragment`. `replace` sees the resolved notebook path. A plain
/// inline target that would need it is wrapped in `<…>`.
pub fn rewrite_file_links(
    text: &str,
    page: &str,
    mut replace: impl FnMut(&str) -> Option<String>,
) -> String {
    let mut result = String::with_capacity(text.len());
    let mut last = 0;
    for target in targets(text) {
        if target.range.start < last {
            continue;
        }
        let raw = &text[target.range.clone()];
        let Some((path, suffix)) = local_target(raw, page) else {
            continue;
        };
        let Some(new) = replace(&path) else {
            continue;
        };
        let new = format!("{new}{suffix}");
        result.push_str(&text[last..target.range.start]);
        if target.inline
            && new
                .contains(|character: char| character.is_whitespace() || "()<>".contains(character))
        {
            result.push_str(&format!("<{new}>"));
        } else {
            result.push_str(&new);
        }
        last = target.range.end;
    }
    result.push_str(&text[last..]);
    result
}

/// The local files `text` (a page at `page`) links to, as notebook paths, in text order.
pub fn linked_files(text: &str, page: &str) -> Vec<String> {
    let mut linked = Vec::new();
    rewrite_file_links(text, page, |path| {
        linked.push(path.to_owned());
        None
    });
    linked
}

/// The content name a resolved path gives a Quire notebook attachment: any folder's
/// `attachments/<name>` names the one stored file.
pub fn attachment_name(path: &str) -> Option<&str> {
    let mut parts = path.rsplit('/');
    let name = parts.next()?;
    (parts.next() == Some("attachments") && !name.is_empty()).then_some(name)
}

/// The target that reaches `attachments/<name>` at the root of an exported folder from `page`.
pub fn root_attachment_target(page: &str, name: &str) -> String {
    format!(
        "{}attachments/{name}",
        "../".repeat(page.matches('/').count())
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn targets_resolve_against_the_page_folder_and_keep_fragments() {
        let text = "- ![Chart](img/chart.png \"Title\") [doc](<../files/my doc.txt>)\n- ![[report.pdf#page=2]] ![[Page]] {{pdf: ../files/b.pdf}}\n- [web](https://example.com/a.png) ![d](data:image/png;base64,AA) [top](#heading) [up](../../out.png)\n```\n![](img/code.png)\n```\n";
        assert_eq!(
            linked_files(text, "Notes/Page.md"),
            [
                "Notes/img/chart.png",
                "files/my doc.txt",
                "Notes/report.pdf",
                "Notes/Page",
                "files/b.pdf"
            ]
        );
        let rewritten = rewrite_file_links(text, "Notes/Page.md", |path| match path {
            "Notes/img/chart.png" => Some("attachments/abc.png".into()),
            "files/my doc.txt" => Some("attachments/def.txt".into()),
            "Notes/report.pdf" => Some("attachments/0f.pdf".into()),
            "files/b.pdf" => Some("attachments/1a.pdf".into()),
            _ => None,
        });
        assert_eq!(
            rewritten,
            "- ![Chart](attachments/abc.png \"Title\") [doc](<attachments/def.txt>)\n- ![[attachments/0f.pdf#page=2]] ![[Page]] {{pdf: attachments/1a.pdf}}\n- [web](https://example.com/a.png) ![d](data:image/png;base64,AA) [top](#heading) [up](../../out.png)\n```\n![](img/code.png)\n```\n"
        );
    }

    #[test]
    fn plain_targets_that_need_brackets_get_them() {
        assert_eq!(
            rewrite_file_links("![](a.png)", "P.md", |_| Some("my files/a b.png".into())),
            "![](<my files/a b.png>)"
        );
        assert_eq!(
            rewrite_file_links("![](a%20b.png)", "P.md", |path| {
                assert_eq!(path, "a b.png");
                Some("attachments/x.png".into())
            }),
            "![](attachments/x.png)"
        );
    }

    #[test]
    fn attachment_names_and_root_targets() {
        assert_eq!(attachment_name("attachments/abc.png"), Some("abc.png"));
        assert_eq!(attachment_name("Deep/attachments/abc.png"), Some("abc.png"));
        assert_eq!(attachment_name("abc.png"), None);
        assert_eq!(attachment_name("attachments/"), None);
        assert_eq!(root_attachment_target("A.md", "x.png"), "attachments/x.png");
        assert_eq!(
            root_attachment_target("Deep/Er/B.md", "x.png"),
            "../../attachments/x.png"
        );
    }
}
