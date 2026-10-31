//! What a page's header, status bar, and details panel say about it: list blocks, words and
//! reading time, headings to jump to, and blocks with an ID. The figures are worked out off the
//! main thread whenever the page's analysis catches up with its text, and the last figures stay
//! on screen meanwhile, so typing never waits for them.

use crate::notebook_session::OpenPage;
use crate::page_analysis::PageAnalysis;
use gpui::{AppContext as _, Context, Entity, IntoElement, Render, Subscription, Task, Window};
use language::BufferSnapshot;
use ui::{Color, Icon, IconName, IconSize, Label, LabelCommon as _, LabelSize, prelude::*};

/// A heading on the page, and where the caret goes when it is chosen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Heading {
    pub text: String,
    pub block: Option<String>,
    /// The heading text's start, in UTF-16 code units as jumps measure it.
    pub from: u32,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PageFacts {
    pub words: usize,
    /// List items, which the reference counts as the page's blocks.
    pub blocks: usize,
    pub headings: Vec<Heading>,
    pub ids: usize,
}

/// The figures for one version of a page, or None when it is not Markdown.
pub fn facts(snapshot: &BufferSnapshot) -> Option<PageFacts> {
    let blocks = crate::markdown_blocks::current_blocks(snapshot)?;
    let text = snapshot.text();
    let headings = blocks
        .iter()
        .filter(|block| block.kind == "heading")
        .map(|block| {
            let content = block.content_from..block.content_to;
            let end = quire_core::markdown::block_id_range(&text, content.clone())
                .map_or(content.end, |id| id.start.saturating_sub(1));
            let line = text.get(content.start..end).unwrap_or_default();
            Heading {
                text: line.trim_start_matches('#').trim().to_owned(),
                block: block.id.clone(),
                from: snapshot.offset_to_offset_utf16(block.content_from).0 as u32,
            }
        })
        .collect();
    Some(PageFacts {
        words: quire_core::markdown::count_words(&text),
        blocks: blocks.iter().filter(|block| block.kind == "list").count(),
        headings,
        ids: blocks.iter().filter(|block| block.id.is_some()).count(),
    })
}

/// A page's figures, kept current, drawn as the end of its editor's status bar.
pub struct PageInfo {
    page: Entity<OpenPage>,
    analysis: Entity<PageAnalysis>,
    facts: Option<PageFacts>,
    /// The version the figures describe, or are being worked out for.
    counted: Option<BufferSnapshot>,
    _counting: Option<Task<()>>,
    _analysis: Subscription,
}

impl PageInfo {
    pub fn new(
        page: Entity<OpenPage>,
        analysis: Entity<PageAnalysis>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscription = cx.observe(&analysis, |this, _, cx| this.count(cx));
        let mut this = Self {
            page,
            analysis,
            facts: None,
            counted: None,
            _counting: None,
            _analysis: subscription,
        };
        this.count(cx);
        this
    }

    pub fn facts(&self) -> Option<&PageFacts> {
        self.facts.as_ref()
    }

    /// Counts again once the analysis has caught up with a version not yet counted.
    fn count(&mut self, cx: &mut Context<Self>) {
        let analysis = self.analysis.read(cx);
        if analysis.current_inline(cx).is_none() {
            return;
        }
        let snapshot = analysis.source.read(cx).snapshot();
        if self
            .counted
            .as_ref()
            .is_some_and(|counted| counted.version() == snapshot.version())
        {
            return;
        }
        self.counted = Some(snapshot.clone());
        let work = cx.background_spawn(async move { facts(&snapshot) });
        self._counting = Some(cx.spawn(async move |this, cx| {
            let facts = work.await;
            this.update(cx, |this, cx| {
                this.facts = facts;
                cx.notify();
            })
            .ok();
        }));
    }
}

impl Render for PageInfo {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let path = self.page.read(cx).path(cx);
        let meta = self.facts.as_ref().map(|facts| {
            format!(
                "{} · Markdown · UTF-8",
                quire_core::markdown::plural(facts.words, "word")
            )
        });
        h_flex()
            .id("page-status")
            .gap_3()
            .min_w_0()
            .when_some(path, |row, path| {
                row.child(
                    h_flex()
                        .gap_1()
                        .min_w_0()
                        .child(
                            Icon::new(IconName::FileTextOutlined)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .child(
                            Label::new(path)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        ),
                )
            })
            .when_some(meta, |row, meta| {
                row.child(
                    Label::new(meta)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .single_line(),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn facts_count_list_blocks_words_headings_and_ids(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let text = "---\ntitle: Plan\n---\n# Plan for آج\n- one idea ^b-1\n- two\n  - nested\n## Next steps ^next\nA closing line.\n";
        let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.run_until_parked();
        let facts = cx.update(|cx| facts(&buffer.read(cx).snapshot()).unwrap());
        assert_eq!(facts.blocks, 3);
        assert_eq!(facts.ids, 2);
        assert_eq!(
            facts.words,
            quire_core::markdown::count_words(text),
            "the header counts what count_words counts"
        );
        let headings: Vec<_> = facts
            .headings
            .iter()
            .map(|heading| (heading.text.as_str(), heading.block.as_deref()))
            .collect();
        assert_eq!(
            headings,
            [("Plan for آج", None), ("Next steps", Some("next"))]
        );
        let first = text.find("# Plan").unwrap();
        let utf16 = text[..first].encode_utf16().count() as u32;
        assert!(facts.headings[0].from >= utf16);
    }

    #[gpui::test]
    fn plain_text_has_no_facts(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let buffer = cx.new(|cx| language::Buffer::local("plain", cx));
        cx.update(|cx| assert!(facts(&buffer.read(cx).snapshot()).is_none()));
    }
}
