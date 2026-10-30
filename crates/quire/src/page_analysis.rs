use gpui::{App, Context, Entity, Subscription};
use language::{Buffer, BufferEvent, BufferSnapshot};
use std::sync::Arc;

pub struct ParsedBlocks {
    pub snapshot: BufferSnapshot,
    pub blocks: Arc<[crate::markdown_blocks::Block]>,
}

pub struct PageAnalysis {
    pub source: Entity<Buffer>,
    parsed: Option<ParsedBlocks>,
    _subscription: Subscription,
}

impl PageAnalysis {
    pub fn new(source: Entity<Buffer>, cx: &mut Context<Self>) -> Self {
        let subscription = cx.subscribe(&source, |this, _, event, cx| {
            if matches!(
                event,
                BufferEvent::Edited { .. }
                    | BufferEvent::Reparsed
                    | BufferEvent::LanguageChanged(_)
            ) {
                this.refresh(cx);
            }
        });
        let mut this = Self {
            source,
            parsed: None,
            _subscription: subscription,
        };
        this.refresh(cx);
        this
    }

    pub fn current(&self, cx: &App) -> Option<&ParsedBlocks> {
        let parsed = self.parsed.as_ref()?;
        let source = self.source.read(cx);
        let snapshot = source.snapshot();
        // Blocks come from the pinned grammar over these bytes, independently of highlighting.
        (snapshot.version() == parsed.snapshot.version()
            && snapshot
                .language()
                .zip(parsed.snapshot.language())
                .is_some_and(|(a, b)| Arc::ptr_eq(a, b)))
        .then_some(parsed)
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.current(cx).is_some() {
            return;
        }
        let source = self.source.read(cx);
        let snapshot = source.snapshot();
        let blocks = crate::markdown_blocks::current_blocks(&snapshot);
        self.parsed = blocks.map(|blocks| ParsedBlocks {
            snapshot,
            blocks: blocks.into(),
        });
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext, TestAppContext};

    #[gpui::test]
    fn background_completion_reuses_the_shared_current_analysis(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source =
            cx.update(|cx| crate::markdown::buffer("- Parent ^parent\n  - child ^child\n", cx));
        cx.executor().run_until_parked();
        let page = cx.new(|cx| PageAnalysis::new(source.clone(), cx));
        let original = cx.update(|cx| page.read(cx).current(cx).unwrap().blocks.clone());
        source.update(cx, |source, cx| {
            source.set_sync_parse_timeout(None);
            source.edit([(0..0, "مرحبا🙂\n\n")], None, cx);
        });
        let current = cx.update(|cx| {
            let parsed = page.read(cx).current(cx).unwrap();
            assert_eq!(
                parsed.snapshot.version(),
                source.read(cx).snapshot().version()
            );
            assert!(!Arc::ptr_eq(&original, &parsed.blocks));
            parsed.blocks.clone()
        });
        cx.executor().run_until_parked();
        cx.update(|cx| {
            assert!(Arc::ptr_eq(
                &current,
                &page.read(cx).current(cx).unwrap().blocks
            ));
        });
    }
}
