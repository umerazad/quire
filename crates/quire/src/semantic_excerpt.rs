use crate::page_analysis::PageAnalysis;
use editor::{Addon, Editor};
use gpui::{App, Context, Entity, Subscription, Window};
use multi_buffer::{ExcerptRange, MultiBuffer, PathKey};
use text::ToPoint;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExcerptState {
    Pending,
    Ready,
    Missing,
    Ambiguous,
}

pub struct SemanticExcerpt {
    pub page: Entity<PageAnalysis>,
    pub buffer: Entity<MultiBuffer>,
    pub state: ExcerptState,
    pub id: String,
    _subscription: Subscription,
}

impl SemanticExcerpt {
    pub fn message(&self) -> Option<String> {
        match self.state {
            ExcerptState::Ready => None,
            ExcerptState::Pending => {
                Some("Updating embedded block. Wait for parsing to finish.".into())
            }
            ExcerptState::Missing => Some(format!(
                "Block ^{} no longer exists. Restore its ID in Source or open another block.",
                self.id
            )),
            ExcerptState::Ambiguous => Some(format!(
                "Multiple blocks use ^{}. Give them distinct IDs in Source to edit this embed.",
                self.id
            )),
        }
    }

    pub fn new(page: Entity<PageAnalysis>, id: String, cx: &mut Context<Self>) -> Self {
        let source = page.read(cx).source.clone();
        let buffer = crate::editor_excerpt::buffer(source, 0..0, cx)
            .expect("An empty seed excerpt must have a valid source range");
        let subscription = cx.observe(&page, |this, _, cx| this.refresh(cx));
        let mut this = Self {
            page,
            buffer,
            state: ExcerptState::Pending,
            id,
            _subscription: subscription,
        };
        this.refresh(cx);
        this
    }

    /// Whether the shown text still ends a line with the block's ID. A new empty child cannot
    /// interrupt its parent's paragraph in Markdown, so until it gets text the parser reads the
    /// parent's ID as plain text; the view keeps its range rather than losing the block.
    fn still_holds_id(&self, cx: &App) -> bool {
        if self.state != ExcerptState::Ready {
            return false;
        }
        let marker = format!("^{}", self.id);
        self.buffer
            .read(cx)
            .snapshot(cx)
            .text()
            .lines()
            .any(|line| {
                line.trim_end()
                    .strip_suffix(marker.as_str())
                    .is_some_and(|before| before.ends_with([' ', '\t']))
            })
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let page = self.page.read(cx);
        let source = page.source.clone();
        let parsed = page.current(cx);
        let (state, ranges) = if let Some(parsed) = parsed {
            let mut matching = parsed
                .blocks
                .iter()
                .filter(|block| block.id.as_deref() == Some(self.id.as_str()));
            match (matching.next(), matching.next()) {
                (Some(block), None) => (
                    ExcerptState::Ready,
                    vec![ExcerptRange::new(
                        block.from.to_point(&parsed.snapshot)
                            ..block.subtree_to.to_point(&parsed.snapshot),
                    )],
                ),
                (None, _) => (ExcerptState::Missing, Vec::new()),
                _ => (ExcerptState::Ambiguous, Vec::new()),
            }
        } else {
            (ExcerptState::Pending, Vec::new())
        };
        if state == ExcerptState::Missing && self.still_holds_id(cx) {
            cx.notify();
            return;
        }
        let snapshot = source.read(cx).snapshot();
        self.buffer.update(cx, |buffer, cx| {
            buffer.set_excerpt_ranges_for_path(PathKey::sorted(0), source, &snapshot, ranges, cx);
        });
        self.state = state;
        cx.notify();
    }
}

struct BoundExcerpt {
    model: Entity<SemanticExcerpt>,
    _subscription: Subscription,
    saved_selections: Option<std::sync::Arc<[language::Selection<multi_buffer::Anchor>]>>,
}

/// The ID of the block an excerpt view shows, while the view shows it.
pub fn root(editor: &Editor, cx: &App) -> Option<String> {
    let model = editor.addon::<BoundExcerpt>()?.model.read(cx);
    (model.state == ExcerptState::Ready).then(|| model.id.clone())
}

impl Addon for BoundExcerpt {
    fn extend_key_context(&self, context: &mut gpui::KeyContext, _: &App) {
        context.add("QuireExcerpt");
    }

    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn synchronize(
    editor: &mut Editor,
    model: &Entity<SemanticExcerpt>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let model = model.read(cx);
    let page = model.page.read(cx);
    let source = page.source.clone();
    let blocks = page.current(cx).map(|parsed| parsed.blocks.clone());
    editor.set_read_only(model.state != ExcerptState::Ready);
    if model.state != ExcerptState::Ready {
        let selections = editor.selections.disjoint_anchors_arc();
        editor
            .addon_mut::<BoundExcerpt>()
            .unwrap()
            .saved_selections
            .get_or_insert(selections);
        editor.change_selections(editor::SelectionEffects::no_scroll(), window, cx, |s| {
            s.select_ranges([multi_buffer::Anchor::Min..multi_buffer::Anchor::Min]);
        });
    } else if let Some(saved) = editor
        .addon_mut::<BoundExcerpt>()
        .unwrap()
        .saved_selections
        .take()
    {
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let selections: Vec<_> = saved
            .iter()
            .copied()
            .filter(|s| snapshot.can_resolve(&s.start) && snapshot.can_resolve(&s.end))
            .collect();
        if !selections.is_empty() {
            editor.change_selections(editor::SelectionEffects::no_scroll(), window, cx, |s| {
                s.select(selections)
            });
        }
    }
    crate::editor_projection::view_range_changed(editor, &source, blocks.as_deref(), window, cx);
    cx.notify();
}

pub fn refresh_view(
    editor: &Entity<Editor>,
    model: &Entity<SemanticExcerpt>,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| synchronize(editor, model, window, cx));
}

pub fn attach(
    editor: &Entity<Editor>,
    model: &Entity<SemanticExcerpt>,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| {
        assert_eq!(editor.buffer(), &model.read(cx).buffer, "Construct the semantic editor from its model's shared-history buffer before attaching it");
        assert!(editor.addon::<BoundExcerpt>().is_none(), "A semantic editor must attach to exactly one block model");
        let subscription = cx.observe_in(model, window, |editor, model, window, cx| synchronize(editor, &model, window, cx));
        editor.register_addon(BoundExcerpt { model: model.clone(), _subscription: subscription, saved_selections: None });
        synchronize(editor, model, window, cx);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::page_analysis::PageAnalysis;
    use gpui::{AppContext, TestAppContext};
    use multi_buffer::MultiBufferOffset;

    const NOTE: &str = "before\n\n- Parent ^parent\n  - child ^child\n- outside ^outside\n";

    #[gpui::test]
    fn two_excerpts_follow_subtree_changes_and_earlier_unicode_edits(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer(NOTE, cx));
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        let first = cx.new(|cx| SemanticExcerpt::new(page.clone(), "parent".into(), cx));
        let second = cx.new(|cx| SemanticExcerpt::new(page.clone(), "parent".into(), cx));
        cx.executor().run_until_parked();
        for model in [&first, &second] {
            cx.update(|cx| {
                let model = model.read(cx);
                assert_eq!(model.state, ExcerptState::Ready);
                assert_eq!(
                    model.buffer.read(cx).read(cx).text(),
                    "- Parent ^parent\n  - child ^child"
                );
            });
        }
        let indent = NOTE.find("  - child").unwrap();
        source.update(cx, |source, cx| {
            source.edit([(indent..indent + 2, "")], None, cx)
        });
        cx.executor().run_until_parked();
        for model in [&first, &second] {
            cx.update(|cx| {
                assert_eq!(
                    model.read(cx).buffer.read(cx).read(cx).text(),
                    "- Parent ^parent"
                )
            });
        }
        let end = NOTE.find("\n  - child").unwrap();
        source.update(cx, |source, cx| {
            source.edit(
                [(0..0, "مرحبا🙂\n"), (end..end, "\n  - new ^new")],
                None,
                cx,
            )
        });
        cx.executor().run_until_parked();
        for model in [&first, &second] {
            cx.update(|cx| {
                assert_eq!(
                    model.read(cx).buffer.read(cx).read(cx).text(),
                    "- Parent ^parent\n  - new ^new"
                )
            });
        }
        let start = cx.update(|cx| source.read(cx).text().find("\n  - new ^new").unwrap());
        source.update(cx, |source, cx| {
            source.edit([(start..start + "\n  - new ^new".len(), "")], None, cx)
        });
        cx.executor().run_until_parked();
        for model in [&first, &second] {
            cx.update(|cx| {
                assert_eq!(
                    model.read(cx).buffer.read(cx).read(cx).text(),
                    "- Parent ^parent"
                )
            });
        }
    }

    #[gpui::test]
    fn missing_or_duplicate_identity_cannot_edit_surrounding_source(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer(NOTE, cx));
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        let model = cx.new(|cx| SemanticExcerpt::new(page, "parent".into(), cx));
        cx.executor().run_until_parked();
        for (note, expected) in [
            (NOTE.replace("^parent", "^renamed"), ExcerptState::Missing),
            (
                format!("{NOTE}- duplicate ^parent\n"),
                ExcerptState::Ambiguous,
            ),
        ] {
            source.update(cx, |source, cx| {
                let len = source.len();
                source.edit([(0..len, note.as_str())], None, cx);
            });
            cx.executor().run_until_parked();
            cx.update(|cx| {
                assert_eq!(model.read(cx).state, expected);
                let buffer = model.read(cx).buffer.clone();
                assert_eq!(buffer.read(cx).read(cx).text(), "");
                buffer.update(cx, |buffer, cx| {
                    buffer.edit(
                        [(MultiBufferOffset(0)..MultiBufferOffset(0), "escape")],
                        None,
                        cx,
                    )
                });
                assert_eq!(source.read(cx).text(), note);
            });
            source.update(cx, |source, cx| {
                source.undo(cx).expect("source edit is undoable");
            });
            cx.executor().run_until_parked();
            cx.update(|cx| {
                assert_eq!(source.read(cx).text(), NOTE);
                assert_eq!(model.read(cx).state, ExcerptState::Ready);
                assert_eq!(
                    model.read(cx).buffer.read(cx).read(cx).text(),
                    "- Parent ^parent\n  - child ^child"
                );
            });
        }
    }
}
