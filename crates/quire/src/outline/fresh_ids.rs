//! A block that an edit copies, such as a paste, Vim `yyp`, or a duplicated line, is a new
//! block, so it gets a fresh ID when its copied ID already names a block on this page or on
//! another page. A block an edit moves keeps its ID: when no open page still has it and the
//! index finds it only on this page or on an open page that no longer has it, the edit put it
//! back after a cut, Vim delete, or undo, and references follow it.

use editor::Editor;
use gpui::{App, Context, Entity};
use language::Buffer;
use quire_core::markdown::Addresses;
use std::ops::Range;
use text::ToOffset as _;
use text::TransactionId;

/// The block IDs `inserted` brought into `text`, as byte ranges after their `^`, split into
/// those already on the page and those to check against the rest of the notebook. Of several
/// inserted copies, the first stays a candidate and the later ones are duplicates.
pub(super) fn inserted_ids(
    text: &str,
    inserted: &[Range<usize>],
) -> (Vec<Range<usize>>, Vec<Range<usize>>) {
    if !inserted
        .iter()
        .any(|range| text.get(range.clone()).is_some_and(|it| it.contains('^')))
    {
        return (Vec::new(), Vec::new());
    }
    let ids = Addresses::scan(text).ids;
    let is_inserted = |id: &Range<usize>| {
        inserted
            .iter()
            .any(|range| range.start < id.start && id.end <= range.end)
    };
    ids.iter()
        .filter(|id| is_inserted(id))
        .cloned()
        .partition(|id| {
            ids.iter().any(|other| {
                other != id
                    && text[other.clone()] == text[id.clone()]
                    && (!is_inserted(other) || other.start < id.start)
            })
        })
}

/// Whether `text` has a block with this ID.
pub(super) fn holds(text: &str, id: &str) -> bool {
    text.contains(id)
        && Addresses::scan(text)
            .ids
            .into_iter()
            .any(|range| &text[range] == id)
}

/// The notebook's open pages by path, with their live buffers, which the index may not have
/// read yet. The editor's own page is among them.
pub(super) fn open_pages(editor: &Editor, cx: &App) -> Vec<(Option<String>, Entity<Buffer>)> {
    let mut pages = crate::references::host(editor)
        .map(|host| open_in(host.read(cx), cx))
        .unwrap_or_default();
    if let Some(own) =
        super::source_buffer(editor, cx).filter(|own| pages.iter().all(|(_, page)| page != own))
    {
        pages.push((None, own));
    }
    pages
}

fn open_in(
    host: &crate::references::ReferenceHost,
    cx: &App,
) -> Vec<(Option<String>, Entity<Buffer>)> {
    let Some((pages, _)) = host.notebook().writers(cx) else {
        return Vec::new();
    };
    pages
        .iter()
        .filter_map(|page| {
            let loaded = page.read(cx).loaded()?;
            Some((loaded.path.clone(), loaded.analysis.read(cx).source.clone()))
        })
        .collect()
}

/// Gives inserted copies of IDs already on this page or on another open page fresh IDs as part
/// of the inserting transaction `into`, and asks the index about the rest, replacing those that
/// a page which is not open, or still has them, already has once it answers.
pub(super) fn follow(
    editor: &Editor,
    buffer: &Entity<Buffer>,
    inserted: &[Range<usize>],
    into: Option<TransactionId>,
    cx: &mut Context<Editor>,
) {
    let snapshot = buffer.read(cx).snapshot();
    let text = snapshot.text();
    let (duplicates, candidates) = inserted_ids(&text, inserted);
    if duplicates.is_empty() && candidates.is_empty() {
        return;
    }
    let elsewhere: Vec<String> = open_pages(editor, cx)
        .into_iter()
        .filter(|(_, page)| page != buffer)
        .map(|(_, page)| page.read(cx).text())
        .collect();
    let (held, candidates): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|range| {
        elsewhere
            .iter()
            .any(|page| holds(page, &text[range.clone()]))
    });
    let fresh: Vec<_> = duplicates
        .into_iter()
        .chain(held)
        .map(|range| (range, crate::references::new_block_id()))
        .collect();
    super::amend(buffer, &fresh, into, cx);
    let Some(host) = crate::references::host(editor) else {
        return;
    };
    let (Some(index), Some(own)) = (
        host.read(cx).index().cloned(),
        host.read(cx).notebook().page_path(buffer, cx),
    ) else {
        return;
    };
    let snapshot = buffer.read(cx).snapshot();
    for range in candidates {
        let id = text[range.clone()].to_owned();
        let anchors = snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end);
        let answer = index.resolve_block(id.clone());
        let buffer = buffer.downgrade();
        let host = host.downgrade();
        let own = own.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Some(found)) = crate::query_index::ask(answer).await else {
                return;
            };
            if found.path == own {
                return;
            }
            cx.update(|cx| {
                let (Some(buffer), Some(host)) = (buffer.upgrade(), host.upgrade()) else {
                    return;
                };
                let moved = open_in(host.read(cx), cx).into_iter().any(|(path, page)| {
                    path.as_ref() == Some(&found.path) && !holds(&page.read(cx).text(), &id)
                });
                let snapshot = buffer.read(cx).snapshot();
                let range = anchors.start.to_offset(&snapshot)..anchors.end.to_offset(&snapshot);
                if !moved && snapshot.text_for_range(range.clone()).collect::<String>() == id {
                    super::amend_later(
                        &buffer,
                        &[(range, crate::references::new_block_id())],
                        into,
                        cx,
                    );
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::{holds, inserted_ids};

    type Found<'a> = (Vec<&'a str>, Vec<&'a str>);

    fn ids(text: &str, inserted: std::ops::Range<usize>) -> Found<'_> {
        let (duplicates, candidates) = inserted_ids(text, std::slice::from_ref(&inserted));
        let names = |ranges: Vec<std::ops::Range<usize>>| {
            ranges.into_iter().map(|range| &text[range]).collect()
        };
        (names(duplicates), names(candidates))
    }

    #[test]
    fn inserted_copies_of_page_ids_are_duplicates() {
        let text = "- One ^a\n- One ^a\n- Two ^b\n";
        assert_eq!(ids(text, 9..18), (vec!["a"], vec![]));
        assert_eq!(ids(text, 18..27), (vec![], vec!["b"]));
        assert_eq!(ids(text, 0..9), (vec!["a"], vec![]), "either copy");
    }

    #[test]
    fn repeated_inserted_copies_keep_the_first_as_a_candidate() {
        let text = "- X ^c\n- Y ^c\n";
        assert_eq!(ids(text, 0..14), (vec!["c"], vec!["c"]));
    }

    #[test]
    fn typing_and_code_are_left_alone() {
        let text = "- One ^ab\n```\n- Code ^ab\n```\n";
        assert_eq!(ids(text, 8..9), (vec![], vec![]), "typing an ID");
        assert_eq!(ids(text, 14..25), (vec![], vec![]), "fenced code");
    }

    #[test]
    fn a_page_holds_only_its_block_ids() {
        let text = "- One ^ab\n- see ((abc)) ^abc-d\n```\n- Code ^xy\n```\n";
        assert!(holds(text, "ab"));
        assert!(holds(text, "abc-d"));
        assert!(!holds(text, "abc"), "a reference is not a block");
        assert!(!holds(text, "xy"), "fenced code is literal");
    }
}
