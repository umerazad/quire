//! A hover preview of the page a notebook link names: its title and opening lines, read from the
//! index when the pointer rests on the link, shown in Zed's hoverable tooltip.

use crate::query_index::ask;
use gpui::{
    Context, InteractiveElement as _, IntoElement, ParentElement, Render, SharedString, Styled,
    Task, Window, px,
};
use quire_store::index::{IndexClient, PagePreview};
use ui::{Color, Label, LabelCommon as _, LabelSize};

/// How many blocks a preview shows.
const LINES: usize = 6;

pub(crate) struct PagePreviewTooltip {
    target: SharedString,
    answer: Option<Result<Option<PagePreview>, String>>,
    _load: Task<()>,
}

impl PagePreviewTooltip {
    pub(crate) fn new(index: Option<&IndexClient>, target: String, cx: &mut Context<Self>) -> Self {
        let request = index.map(|index| index.page_preview(target.clone(), LINES));
        let load = cx.spawn(async move |this, cx| {
            let answer = match request {
                Some(request) => ask(request).await,
                None => Err("the notebook index has not started".into()),
            };
            this.update(cx, |this, cx| {
                this.answer = Some(answer);
                cx.notify();
            })
            .ok();
        });
        Self {
            target: target.into(),
            answer: None,
            _load: load,
        }
    }

    /// The title and lines the preview shows now.
    pub(crate) fn content(&self) -> (SharedString, Vec<SharedString>) {
        let note = |text: String| vec![SharedString::from(text)];
        match &self.answer {
            None => (self.target.clone(), note("Loading…".into())),
            Some(Ok(Some(preview))) if preview.lines.is_empty() => (
                preview.page.title.clone().into(),
                note("This page is empty.".into()),
            ),
            Some(Ok(Some(preview))) => (
                preview.page.title.clone().into(),
                preview.lines.iter().cloned().map(Into::into).collect(),
            ),
            Some(Ok(None)) => (
                self.target.clone(),
                note("No page has this name yet. Follow the link to create it.".into()),
            ),
            Some(Err(error)) => (
                self.target.clone(),
                note(format!(
                    "Cannot preview this page: {error}. Wait for indexing, then hover again."
                )),
            ),
        }
    }
}

impl Render for PagePreviewTooltip {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (title, lines) = self.content();
        ui::tooltip_container(cx, |container, _| {
            container
                .debug_selector(|| "quire-page-preview".into())
                .max_w(px(360.))
                .gap_1()
                .child(Label::new(title))
                .children(lines.into_iter().map(|line| {
                    Label::new(line)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate()
                }))
        })
    }
}
