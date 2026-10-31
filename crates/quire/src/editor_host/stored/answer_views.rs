//! The answer pane beside the main page (NR30): opened by `:ai` without a range, the page menu,
//! presets, and Synthesize under linked references; it keeps answers in today's page and runs
//! Tidy in the page's editor.

use super::*;
use crate::ai::answer::{AnswerEvent, AnswerPane, Place, Seed};
use quire_core::daily::format_daily;

#[derive(Default)]
pub(in crate::editor_host::stored) struct AnswerState {
    pub(in crate::editor_host::stored) pane: Option<Entity<AnswerPane>>,
    keeping: Option<gpui::Task<()>>,
}

impl StoredWindow {
    /// The ISO date `title` names under the notebook's daily layout.
    pub(in crate::editor_host::stored) fn daily_date_of(&self, title: &str) -> Option<String> {
        let calendar = self.nav.daily.calendar.as_ref()?;
        quire_core::daily::daily_date(title, &self.nav.daily.layout, calendar)
    }

    /// The daily page title for an ISO date under the notebook's daily layout.
    pub(in crate::editor_host::stored) fn daily_title_of(&self, date: &str) -> Option<String> {
        let calendar = self.nav.daily.calendar.as_ref()?;
        format_daily(&self.nav.daily.layout.title, date, calendar).ok()
    }

    fn page_writers(&self) -> Option<crate::ai::context::Writers> {
        let session = self.session.try_borrow().ok()?;
        Some((session.open_pages(), session.storage()))
    }

    /// Opens the answer pane with `seed`, replacing any open one and ending its run.
    pub(in crate::editor_host::stored) fn open_answer(
        &mut self,
        seed: Seed,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let page = self
            .nav
            .view
            .is_none()
            .then(|| self.nav.title(self.page.read(cx).id()).map(str::to_owned))
            .flatten();
        let place = Place {
            daily: page
                .as_deref()
                .is_some_and(|page| self.daily_date_of(page).is_some()),
            page,
            today: self.today().unwrap_or_default(),
        };
        let host = self.links.host.clone();
        let writers = self.page_writers();
        let pane = cx.new(|cx| AnswerPane::new(host, writers, seed, place, window, cx));
        cx.subscribe_in(
            &pane,
            window,
            |this, pane, event: &AnswerEvent, window, cx| match event {
                AnswerEvent::Close => this.close_answer(window, cx),
                AnswerEvent::Keep { index, answer } => {
                    this.keep_answer(pane.clone(), *index, answer.clone(), cx)
                }
                AnswerEvent::Tidy => this.tidy_shown(window, cx),
            },
        )
        .detach();
        self.nav.answer = AnswerState {
            pane: Some(pane),
            keeping: None,
        };
        cx.notify();
    }

    pub(in crate::editor_host::stored) fn close_answer(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.nav.answer = AnswerState::default();
        self.focus_active(window, cx);
        cx.notify();
    }

    fn keep_answer(
        &mut self,
        pane: Entity<AnswerPane>,
        index: usize,
        answer: String,
        cx: &mut gpui::Context<Self>,
    ) {
        let prepared = self
            .today_daily(|error| {
                format!(
                    "Cannot keep the answer in today's page: {error}. Fix the daily layout in Daily notes, then retry."
                )
            })
            .and_then(|today| {
            let client = self.index_client(cx).ok_or_else(|| {
                "Keeping an answer needs the notebook index, which has not started. Wait for indexing, then retry.".to_owned()
            })?;
            let writers = self.page_writers().ok_or_else(|| {
                "The notebook is busy. Press Keep in today's note again.".to_owned()
            })?;
            Ok((today, client, writers))
        });
        let (today, client, writers) = match prepared {
            Ok(prepared) => prepared,
            Err(message) => {
                pane.update(cx, |pane, cx| pane.show_notice(message, cx));
                return;
            }
        };
        let kept = crate::ai::keep::keep(answer, today, client, writers, cx);
        self.nav.answer.keeping = Some(cx.spawn(async move |this, cx| {
            let result = kept.await;
            pane.update(cx, |pane, cx| match result {
                Ok(title) => pane.kept(index, title, cx),
                Err(message) => pane.show_notice(message, cx),
            });
            this.update(cx, |this, cx| {
                this.nav.answer.keeping = None;
                cx.notify();
            })
            .ok();
        }));
    }

    /// Tidies the page in the main pane, outside this window's update so the editor can ask
    /// the notebook about the day.
    pub(in crate::editor_host::stored) fn tidy_shown(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(editor) = self
            .editor
            .as_ref()
            .map(|view| view.read(cx).source.clone())
        else {
            return;
        };
        window.focus(&editor.focus_handle(cx), cx);
        window.defer(cx, move |window, cx| {
            crate::ai::proposal::tidy(&editor, window, cx)
        });
    }
}
