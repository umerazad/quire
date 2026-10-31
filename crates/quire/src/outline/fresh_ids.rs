//! A pasted or duplicated block is a new block, so it gets a fresh ID when its copied ID
//! already names a block on this page or on another page. References keep pointing at the
//! original, and the copy stays addressable.

use editor::Editor;
use gpui::{Context, Entity};
use language::Buffer;
use quire_core::markdown::Addresses;
use std::ops::Range;
use text::ToOffset as _;
use text::TransactionId;

fn new_id() -> String {
    format!("b-{}", uuid::Uuid::new_v4().simple())
}

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

/// Gives inserted copies of IDs already on the page fresh IDs as part of the inserting
/// transaction `into`, and asks the index about the rest, replacing those another page already
/// has once it answers.
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
    let candidates: Vec<_> = candidates
        .into_iter()
        .map(|range| {
            let id = text[range.clone()].to_owned();
            (
                id,
                snapshot.anchor_before(range.start)..snapshot.anchor_after(range.end),
            )
        })
        .collect();
    let fresh: Vec<_> = duplicates
        .into_iter()
        .map(|range| (range, new_id()))
        .collect();
    super::amend(buffer, &fresh, into, cx);
    let Some(host) = crate::references::host(editor) else {
        return;
    };
    let host = host.read(cx);
    let (Some(index), Some(own)) = (host.index().cloned(), host.notebook().page_path(buffer, cx))
    else {
        return;
    };
    for (id, anchors) in candidates {
        let answer = index.resolve_block(id.clone());
        let buffer = buffer.downgrade();
        let own = own.clone();
        cx.spawn(async move |_, cx| {
            let Ok(Some(found)) = crate::query_index::ask(answer).await else {
                return;
            };
            if found.path == own {
                return;
            }
            cx.update(|cx| {
                let Some(buffer) = buffer.upgrade() else {
                    return;
                };
                let snapshot = buffer.read(cx).snapshot();
                let range = anchors.start.to_offset(&snapshot)..anchors.end.to_offset(&snapshot);
                if snapshot.text_for_range(range.clone()).collect::<String>() == id {
                    super::amend(&buffer, &[(range, new_id())], into, cx);
                }
            });
        })
        .detach();
    }
}

#[cfg(test)]
mod tests {
    use super::inserted_ids;

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
}
