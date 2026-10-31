//! Opening a page at a block: a search hit, tag row, task row, reference row, or followed block
//! link puts the caret on its block and centers it, as the reference `PageJump` does.

use super::*;
use crate::navigation::Jump;
use editor::{SelectionEffects, scroll::Autoscroll};
use multi_buffer::MultiBufferOffset;

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
