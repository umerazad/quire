//! Where Write's folds and caret-dependent syntax sit, in offsets kept current across edits, so a
//! caret move or a reparse refolds only what changed. Refolding a whole page cost about 140 ms on
//! a 1,500-line task page in a release build, since Zed resyncs its fold, wrap, and block maps
//! over every fold removed or added.

use multi_buffer::{MultiBufferOffset, MultiBufferSnapshot};
use std::ops::Range;
use text::Bias;

/// The buffer ranges a refresh refolds: Write's folds that strictly intersect them are removed,
/// and the wanted folds that strictly intersect them are added, as Zed's fold map matches
/// ranges. Kept sorted and merged.
#[derive(Clone, Default, Debug, PartialEq)]
pub(super) struct Scope(Vec<Range<MultiBufferOffset>>);

impl Scope {
    pub(super) fn whole(snapshot: &MultiBufferSnapshot) -> Self {
        Self(vec![MultiBufferOffset(0)..snapshot.len()])
    }

    pub(super) fn add(&mut self, range: Range<MultiBufferOffset>) {
        self.0.push(range);
    }

    pub(super) fn extend(&mut self, other: &Scope) {
        self.0.extend(other.0.iter().cloned());
    }

    /// Sorts and merges the ranges, so [`Scope::meets`] can search them.
    pub(super) fn normalize(&mut self) {
        self.0.sort_by_key(|range| (range.start, range.end));
        let mut merged: Vec<Range<MultiBufferOffset>> = Vec::with_capacity(self.0.len());
        for range in self.0.drain(..) {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        self.0 = merged;
    }

    pub(super) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub(super) fn ranges(&self) -> &[Range<MultiBufferOffset>] {
        &self.0
    }

    /// Whether `range` strictly intersects a range of the scope; a normalized scope is searched.
    pub(super) fn meets(&self, range: &Range<MultiBufferOffset>) -> bool {
        let first = self.0.partition_point(|scope| scope.end <= range.start);
        self.0
            .get(first)
            .is_some_and(|scope| range.start < scope.end && scope.start < range.end)
    }
}

/// A block's marker, checkbox, text start, hidden ID, and children, in offsets.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct BlockAt {
    pub start: MultiBufferOffset,
    pub checkbox: MultiBufferOffset,
    pub content: MultiBufferOffset,
    pub id: Option<Range<MultiBufferOffset>>,
    pub children: Option<MultiBufferOffset>,
}

/// Syntax that renders unless the caret is in its block: where it is, whether its text is still
/// the parsed source, and whether the last refresh rendered it.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SyntaxAt {
    pub range: Range<MultiBufferOffset>,
    pub block: Range<MultiBufferOffset>,
    pub intact: bool,
    pub shown: bool,
}

impl SyntaxAt {
    /// Whether the syntax renders with the caret at `caret`, as `shown` in the parent decides.
    pub(super) fn renders(&self, caret: Option<MultiBufferOffset>) -> bool {
        self.intact
            && !caret.is_some_and(|caret| self.block.start <= caret && caret <= self.block.end)
    }
}

/// The kinds of caret-dependent syntax, each listed in the order a refresh reads it.
#[derive(Clone, Copy)]
pub(super) enum Kind {
    Mark,
    Preview,
    List,
    Media,
    Toolbar,
    Embed,
    Link,
}

pub(super) const KINDS: [Kind; 7] = [
    Kind::Mark,
    Kind::Preview,
    Kind::List,
    Kind::Media,
    Kind::Toolbar,
    Kind::Embed,
    Kind::Link,
];

/// A fold Write placed: its range and a key for everything its placeholder shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Placed {
    pub range: Range<MultiBufferOffset>,
    pub key: u64,
}

impl Ord for Placed {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.range.start, self.range.end, self.key).cmp(&(
            other.range.start,
            other.range.end,
            other.key,
        ))
    }
}

impl PartialOrd for Placed {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A key no wanted fold has, for a placed fold whose range an edit touched.
pub(super) const STALE: u64 = u64::MAX;

pub(super) struct Layout {
    /// The blocks the layout was built for.
    pub source: std::sync::Arc<[super::AnchoredBlock]>,
    pub edit_count: usize,
    edits: text::Subscription<MultiBufferOffset>,
    pub blocks: Vec<BlockAt>,
    pub syntax: [Vec<SyntaxAt>; KINDS.len()],
    /// Write's folds, sorted.
    pub placed: Vec<Placed>,
    /// Counts refreshes, so folds other modules draw are placed anew by each.
    pub generation: u64,
}

impl Layout {
    pub(super) fn new(
        source: std::sync::Arc<[super::AnchoredBlock]>,
        edits: text::Subscription<MultiBufferOffset>,
        snapshot: &MultiBufferSnapshot,
    ) -> Self {
        // The snapshot already holds any edits published before the subscription was read.
        edits.consume();
        Self {
            source,
            edit_count: snapshot.edit_count(),
            edits,
            blocks: Vec::new(),
            syntax: Default::default(),
            placed: Vec::new(),
            generation: 0,
        }
    }

    pub(super) fn syntax(&self, kind: Kind) -> &[SyntaxAt] {
        &self.syntax[kind as usize]
    }

    pub(super) fn syntax_mut(&mut self, kind: Kind) -> &mut Vec<SyntaxAt> {
        &mut self.syntax[kind as usize]
    }

    /// Moves every offset through the edits since the last refresh, marks the folds they touched
    /// stale, and returns the edited lines, each widened by a character, which a refresh must
    /// read again. `None` when the edits cannot be followed and the view must be refolded whole.
    pub(super) fn follow(&mut self, snapshot: &MultiBufferSnapshot) -> Option<Scope> {
        let patch = self.edits.consume();
        if patch.is_empty() {
            return (self.edit_count == snapshot.edit_count()).then(Scope::default);
        }
        let edits = patch.edits();
        // Many edits at once (a replace-all) are cheaper to refold whole than to follow.
        if edits.len() > 64 {
            return None;
        }
        self.edit_count = snapshot.edit_count();
        let map = |offset: MultiBufferOffset| patch.old_to_new(offset);
        let range = |range: &Range<MultiBufferOffset>| map(range.start)..map(range.end);
        let touched = |range: &Range<MultiBufferOffset>| {
            edits
                .iter()
                .any(|edit| range.start <= edit.old.end && edit.old.start <= range.end)
        };
        for block in &mut self.blocks {
            block.start = map(block.start);
            block.checkbox = map(block.checkbox);
            block.content = map(block.content);
            block.id = block.id.as_ref().map(range);
            block.children = block.children.map(map);
        }
        for syntax in self.syntax.iter_mut().flatten() {
            syntax.range = range(&syntax.range);
            syntax.block = range(&syntax.block);
        }
        for placed in &mut self.placed {
            if touched(&placed.range) {
                placed.key = STALE;
            }
            placed.range = range(&placed.range);
        }
        self.placed.sort();
        let len = snapshot.len();
        let mut scope = Scope::default();
        for edit in edits {
            let first = snapshot.offset_to_point(edit.new.start).row;
            let last = snapshot.offset_to_point(edit.new.end).row;
            let start = snapshot.point_to_offset(language::Point::new(first, 0));
            let end = snapshot.point_to_offset(language::Point::new(
                last,
                snapshot.line_len(multi_buffer::MultiBufferRow(last)),
            ));
            scope.add(
                MultiBufferOffset(start.0.saturating_sub(1))
                    ..MultiBufferOffset((end.0 + 1).min(len.0)),
            );
        }
        scope.normalize();
        Some(scope)
    }

    /// Replaces the placed folds that meet `scope` with the wanted ones that do.
    pub(super) fn place(&mut self, scope: &Scope, wanted: impl IntoIterator<Item = Placed>) {
        self.placed.retain(|placed| !scope.meets(&placed.range));
        self.placed.extend(wanted);
        self.placed.sort();
    }

    /// The indices of the blocks whose folds can meet `scope`. A block's folds lie between its
    /// marker and the next block's, since its ID ends its text and its children follow.
    pub(super) fn blocks_meeting(&self, scope: &Scope) -> Vec<usize> {
        let mut found: Vec<usize> = scope
            .ranges()
            .iter()
            .flat_map(|range| {
                let first = self
                    .blocks
                    .partition_point(|block| block.start <= range.start)
                    .saturating_sub(1);
                let end = self.blocks.partition_point(|block| block.start < range.end);
                first..end.max(first)
            })
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }
}

/// `range` and the character on each side of it, so a fold an edit emptied there is matched too.
pub(super) fn widen(
    range: &Range<MultiBufferOffset>,
    snapshot: &MultiBufferSnapshot,
) -> Range<MultiBufferOffset> {
    let start = MultiBufferOffset(range.start.0.saturating_sub(1));
    let end = MultiBufferOffset((range.end.0 + 1).min(snapshot.len().0));
    snapshot.clip_offset(start, Bias::Left)..snapshot.clip_offset(end, Bias::Right)
}

/// The ranges where `placed` and `wanted` differ, each widened as `widen` says.
pub(super) fn difference(
    placed: &[Placed],
    wanted: &mut [Placed],
    widen: impl Fn(&Range<MultiBufferOffset>) -> Range<MultiBufferOffset>,
) -> Scope {
    wanted.sort();
    let mut scope = Scope::default();
    let mut differs = |range: &Range<MultiBufferOffset>| scope.add(widen(range));
    let (mut old, mut new) = (placed.iter().peekable(), wanted.iter().peekable());
    loop {
        match (old.peek(), new.peek()) {
            (Some(a), Some(b)) if a == b => {
                old.next();
                new.next();
            }
            (Some(a), Some(b)) if *a < *b => {
                differs(&a.range);
                old.next();
            }
            (_, Some(b)) => {
                differs(&b.range);
                new.next();
            }
            (Some(a), None) => {
                differs(&a.range);
                old.next();
            }
            (None, None) => break,
        }
    }
    scope.normalize();
    scope
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(range: Range<usize>) -> Range<MultiBufferOffset> {
        MultiBufferOffset(range.start)..MultiBufferOffset(range.end)
    }

    #[test]
    fn scope_meets_only_strictly_intersecting_ranges() {
        let mut scope = Scope::default();
        scope.add(at(20..30));
        scope.add(at(5..10));
        scope.add(at(8..12));
        scope.normalize();
        assert_eq!(scope.ranges(), &[at(5..12), at(20..30)]);
        assert!(scope.meets(&at(11..15)));
        assert!(scope.meets(&at(0..6)));
        assert!(scope.meets(&at(25..25)));
        assert!(!scope.meets(&at(12..20)));
        assert!(!scope.meets(&at(0..5)));
        assert!(!scope.meets(&at(30..40)));
    }

    #[test]
    fn difference_widens_the_folds_only_one_side_has() {
        let placed = |range, key| Placed {
            range: at(range),
            key,
        };
        let old = [
            placed(0..2, 1),
            placed(4..6, 1),
            placed(4..6, 1),
            placed(8..9, 2),
        ];
        let mut new = [placed(8..9, 3), placed(4..6, 1), placed(0..2, 1)];
        let scope = difference(&old, &mut new, |range| {
            MultiBufferOffset(range.start.0.saturating_sub(1))..MultiBufferOffset(range.end.0 + 1)
        });
        assert_eq!(scope.ranges(), &[at(3..10)]);
    }
}
