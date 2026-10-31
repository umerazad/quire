//! Where a page opens: at a block for a search hit, tag row, task row, reference row, or followed
//! block link, which puts the caret on its block and centers it as the reference `PageJump`
//! does; otherwise at the reading position each pane remembers for the page.

use super::*;
use crate::navigation::Jump;
use editor::{
    DisplayPoint, Editor, EditorEvent, SelectionEffects,
    display_map::{DisplayRow, ToDisplayPoint},
    scroll::Autoscroll,
};
use multi_buffer::MultiBufferOffset;
use quire_core::workspace::position_key;

/// A caret move waiting for its page's editor, or for the page's blocks to parse.
pub(in crate::editor_host::stored) struct PendingJump {
    page: Entity<OpenPage>,
    /// The jump is for the side pane's view of the page, which can show the main page too.
    side: bool,
    jump: Jump,
    _parsed: Option<gpui::Subscription>,
}

enum Reveal {
    At(usize),
    Waiting,
    Missing,
}

/// Where `jump` lands in the page `analysis` reads: the block with its ID once blocks are
/// parsed, else the indexed offset, clamped to the text.
fn locate(analysis: &crate::page_analysis::PageAnalysis, jump: &Jump, cx: &App) -> Reveal {
    let parsed = jump.block.as_ref().and_then(|_| analysis.current(cx));
    let by_id = parsed.as_ref().and_then(|parsed| {
        parsed
            .blocks
            .iter()
            .find(|block| block.id == jump.block)
            .map(|block| block.content_from)
    });
    if let Some(offset) = by_id {
        return Reveal::At(offset);
    }
    if let Some(from) = jump.from {
        let snapshot = analysis.source.read(cx).snapshot();
        let end = snapshot.offset_to_offset_utf16(snapshot.len());
        let utf16 = text::OffsetUtf16((from as usize).min(end.0));
        return Reveal::At(snapshot.offset_utf16_to_offset(utf16));
    }
    match (&jump.block, parsed) {
        (Some(_), None) => Reveal::Waiting,
        _ => Reveal::Missing,
    }
}

impl StoredWindow {
    /// Opens `page` and puts the caret where `jump` says once its editor can show it.
    pub(in crate::editor_host::stored) fn open_page_at(
        &mut self,
        page: String,
        jump: Option<Jump>,
        beside: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.open_page(page, beside, window, cx);
        let side = beside && self.nav.side.is_some();
        self.nav.jump = jump.map(|jump| PendingJump {
            page: match (&self.nav.side, side) {
                (Some(pane), true) => pane.page.clone(),
                _ => self.page.clone(),
            },
            side,
            jump,
            _parsed: None,
        });
        self.apply_jump(window, cx);
    }

    /// Puts the caret where `jump` says in the page the main pane shows, without visiting it.
    pub(in crate::editor_host::stored) fn reveal_in_shown(
        &mut self,
        jump: Jump,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.nav.jump = Some(PendingJump {
            page: self.page.clone(),
            side: false,
            jump,
            _parsed: None,
        });
        self.apply_jump(window, cx);
    }

    /// Puts the caret where `jump` says in the side pane's page.
    pub(in crate::editor_host::stored) fn reveal_in_side(
        &mut self,
        jump: Jump,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(page) = self.nav.side.as_ref().map(|side| side.page.clone()) else {
            return;
        };
        self.nav.jump = Some(PendingJump {
            page,
            side: true,
            jump,
            _parsed: None,
        });
        self.apply_jump(window, cx);
    }

    /// Moves the caret for a waiting jump when its page's editor is showing and can place it.
    pub(in crate::editor_host::stored) fn apply_jump(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(pending) = &self.nav.jump else {
            return;
        };
        let (shown, editor) = match pending.side {
            true => self
                .nav
                .side
                .as_ref()
                .filter(|side| side.page == pending.page)
                .map_or((false, None), |side| (true, side.editor.clone())),
            false => (self.page == pending.page, self.editor.clone()),
        };
        let Some(editor) = editor.filter(|_| shown) else {
            if !shown {
                self.nav.jump = None;
            }
            return;
        };
        let (analysis, source) = {
            let view = editor.read(cx);
            (view.analysis.clone(), view.source.clone())
        };
        match locate(analysis.read(cx), &pending.jump, cx) {
            Reveal::At(offset) => {
                self.nav.jump = None;
                source.update(cx, |editor, cx| {
                    editor.change_selections(
                        SelectionEffects::scroll(Autoscroll::center()),
                        window,
                        cx,
                        |selections| {
                            selections.select_ranges([
                                MultiBufferOffset(offset)..MultiBufferOffset(offset)
                            ])
                        },
                    )
                });
                window.focus(&source.focus_handle(cx), cx);
            }
            Reveal::Missing => self.nav.jump = None,
            Reveal::Waiting => {
                let parsed = cx.observe_in(&analysis, window, |this, _, window, cx| {
                    this.apply_jump(window, cx)
                });
                if let Some(pending) = &mut self.nav.jump {
                    pending._parsed = Some(parsed);
                }
            }
        }
    }
}

/// The source line at the top of an editor's view.
fn top_line(editor: &mut Editor, cx: &mut gpui::Context<Editor>) -> u32 {
    let row = editor.scroll_position(cx).y.max(0.).floor() as u32;
    DisplayPoint::new(DisplayRow(row), 0)
        .to_point(&editor.display_snapshot(cx))
        .row
}

impl StoredWindow {
    /// Shows the page in a new editor from its remembered line, unless a jump is about to place
    /// it, and remembers the line whenever the view scrolls.
    pub(in crate::editor_host::stored) fn keep_reading_position(
        &mut self,
        view: &Entity<NotebookWindow>,
        page: &Entity<OpenPage>,
        side: bool,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let key = position_key(side, page.read(cx).id());
        let source = view.read(cx).source.clone();
        let jumping = self
            .nav
            .jump
            .as_ref()
            .is_some_and(|pending| pending.side == side && &pending.page == page);
        if let Some(line) = self.nav.workspace.positions.get(&key).copied()
            && !jumping
        {
            source.update(cx, |editor, cx| {
                let snapshot = editor.display_snapshot(cx);
                let row = text::Point::new(line, 0).to_display_point(&snapshot).row();
                editor.set_scroll_top_row(row, window, cx);
            });
        }
        cx.subscribe(&source, move |this, source, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::ScrollPositionChanged { .. }) {
                let line = source.update(cx, top_line);
                let key = key.clone();
                this.remember(
                    |workspace| match line {
                        0 => _ = workspace.positions.remove(&key),
                        line => _ = workspace.positions.insert(key, line),
                    },
                    cx,
                );
            }
        })
        .detach();
    }
}
