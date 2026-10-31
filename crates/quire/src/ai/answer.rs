//! The answer pane (reference `AiPage.tsx`): questions about the notebook beside the page, each
//! answered with the excerpts it was sent listed under it, the model's tool activity, and the
//! answer rendered as Markdown whose citations open the page or block they name. A synthesis of
//! linked references starts by editing its instruction and runs only on Command-Enter.

use super::{
    context::{Writers, gather},
    process::{self, Host, Mode, Outcome, Request, Transcript, Update},
    settings::{self, AiSettings},
};
use crate::references::{Follow, ReferenceHost, Target};
use ::settings::Settings as _;
use editor::Editor;
use futures::{StreamExt as _, channel::oneshot};
use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollHandle, SharedString, Task,
    Window, prelude::*,
};
use markdown::{Markdown, MarkdownElement, MarkdownFont, MarkdownStyle};
use quire_core::ai::{
    AiRange,
    ask::{
        Cited, DEFAULT_SYNTHESIS, Gathered, NO_CONTEXT, Needs, PRESETS, Preset, Source,
        TAGGED_DRAFT, Turn, ask_context_summary, build_ask_prompt, link_citations, parse_citation,
    },
    backend::{AI_OFF, CANCELED, PROMPT_LIMIT, PROMPT_TOO_LONG},
};
use std::collections::HashMap;
use ui::{
    Button, ButtonCommon as _, ButtonStyle, Clickable as _, Color, Disableable as _, Label,
    LabelCommon as _, LabelSize, h_flex, v_flex,
};

pub const CONTEXT: &str = "QuireAnswer";
pub const INPUT_CONTEXT: &str = "QuireAnswerInput";
pub const SYNTHESIS_CONTEXT: &str = "QuireSynthesis";
const THINKING: &str = "Thinking…";
const READING: &str = "Reading the notebook…";

/// What the pane starts with.
#[derive(Clone, Debug, Default)]
pub struct Seed {
    /// The header, such as "Linked references to Atlas".
    pub title: Option<String>,
    /// A question to ask at once.
    pub question: Option<String>,
    /// Text to start the question box with.
    pub draft: Option<String>,
    /// Selected text the first question is about.
    pub range: Option<AiRange>,
    /// Where the first question's excerpts come from.
    pub source: Option<Source>,
}

impl Seed {
    /// Linked references with no question make a synthesis, which waits for its instruction.
    fn synthesis(&self) -> bool {
        self.question.as_deref().is_none_or(|q| q.trim().is_empty())
            && matches!(self.source, Some(Source::References(_)))
    }

    /// The pane for a preset: its question and excerpts, or the Tagged with… draft.
    pub fn preset(preset: &Preset, value: &str) -> Self {
        let source = match (preset.key, preset.needs) {
            ("due", _) => Some(Source::Due),
            ("week", _) => Some(Source::Week),
            ("yesterday", _) => Some(Source::Yesterday),
            ("last", _) => Some(Source::Mentions(value.to_owned())),
            (_, Needs::Tag) if !value.trim().is_empty() => {
                Some(Source::Tagged(value.trim().to_owned()))
            }
            _ => None,
        };
        match (preset.needs, value.trim().is_empty()) {
            (Needs::Tag, true) => Self {
                draft: Some(TAGGED_DRAFT.into()),
                ..Self::default()
            },
            _ => Self {
                question: Some(preset.question(value)),
                source,
                ..Self::default()
            },
        }
    }
}

/// The page the pane sits beside, and the notebook's date.
#[derive(Clone, Debug, Default)]
pub struct Place {
    pub page: Option<String>,
    pub daily: bool,
    pub today: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AnswerEvent {
    Close,
    /// Keep turn `index`'s answer in today's page.
    Keep {
        index: usize,
        answer: String,
    },
    /// Tidy the daily page the pane sits beside.
    Tidy,
}

/// One tool call: what it is doing, then its summary once it returns.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolRow {
    pub id: Option<String>,
    pub label: String,
    pub done: bool,
    pub failed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    Reading,
    Streaming,
    Done,
    Failed { message: String, retry: bool },
}

pub struct TurnView {
    pub question: String,
    first: bool,
    pub context: Vec<String>,
    pub tools: Vec<ToolRow>,
    pub answer: String,
    pub status: Status,
    pub kept: Option<String>,
    labels: HashMap<String, String>,
    markdown: Entity<Markdown>,
    cancel: Option<oneshot::Sender<()>>,
    _run: Option<Task<()>>,
}

pub struct AnswerPane {
    host: Entity<ReferenceHost>,
    writers: Option<Writers>,
    seed: Seed,
    place: Place,
    pub turns: Vec<TurnView>,
    /// Where the current conversation starts; a revised synthesis starts a fresh one.
    since: usize,
    pub input: Entity<Editor>,
    pub editing_synthesis: bool,
    pub notice: Option<SharedString>,
    focus: FocusHandle,
    scroll: ScrollHandle,
}

impl EventEmitter<AnswerEvent> for AnswerPane {}

impl Focusable for AnswerPane {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// Opens what a citation link names; other links open in the browser.
fn open_link(host: &Entity<ReferenceHost>, url: &str, window: &mut Window, cx: &mut App) {
    let target = match parse_citation(url) {
        Some(Cited::Page(page)) => Target::Page(page),
        Some(Cited::Block(block)) => Target::Block(block),
        None => return cx.open_url(url),
    };
    let notebook = host.read(cx).notebook().clone();
    notebook.follow(
        Follow {
            target,
            beside: false,
            backlinks: false,
        },
        window,
        cx,
    );
}

/// Why questions cannot run now, or None.
pub fn blocked(cx: &App) -> Option<&'static str> {
    (!settings::is_on(cx)).then(|| {
        if settings::is_tested(cx) {
            AI_OFF
        } else {
            settings::UNTESTED
        }
    })
}

impl AnswerPane {
    pub fn new(
        host: Entity<ReferenceHost>,
        writers: Option<Writers>,
        seed: Seed,
        place: Place,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let synthesis = seed.synthesis();
        let input = cx.new(|cx| {
            let mut editor = Editor::auto_height(if synthesis { 4 } else { 2 }, 12, window, cx);
            editor.set_placeholder_text("Ask about your notes…", window, cx);
            let draft = seed
                .draft
                .clone()
                .or_else(|| synthesis.then(|| settings::synthesis(cx)));
            if let Some(draft) = draft {
                editor.set_text(draft, window, cx);
            }
            editor
        });
        let mut pane = Self {
            host,
            writers,
            seed,
            place,
            turns: Vec::new(),
            since: 0,
            input,
            editing_synthesis: synthesis,
            notice: blocked(cx).map(SharedString::from),
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
        };
        let question = pane
            .seed
            .question
            .clone()
            .filter(|question| !question.trim().is_empty());
        let input = pane.input.focus_handle(cx);
        window.focus(&input, cx);
        match question {
            Some(question) if pane.notice.is_none() => pane.ask(question, true, window, cx),
            _ if synthesis => pane.input.update(cx, |editor, cx| {
                editor.select_all(&editor::actions::SelectAll, window, cx)
            }),
            _ => {}
        }
        pane
    }

    pub fn title(&self) -> String {
        self.seed
            .title
            .clone()
            .unwrap_or_else(|| "Ask the notebook".into())
    }

    fn running(&self) -> bool {
        self.turns
            .iter()
            .any(|turn| matches!(turn.status, Status::Reading | Status::Streaming))
    }

    /// Where a question's excerpts come from: the seed's for the first question, Tagged with…
    /// for a typed tag question, and otherwise the blocks matching its words.
    fn source_for(&self, question: &str, first: bool) -> Option<Source> {
        if first && (self.seed.source.is_some() || self.seed.range.is_some()) {
            return self.seed.source.clone();
        }
        if let Some(tag) = question.strip_prefix(TAGGED_DRAFT) {
            let tag: String = tag
                .chars()
                .take_while(|c| !c.is_whitespace() && *c != ',')
                .collect();
            if !tag.is_empty() {
                return Some(Source::Tagged(tag));
            }
        }
        Some(Source::Search)
    }

    /// Asks `question`. The first question of a conversation carries the seed's range and
    /// excerpts; every question carries the earlier turns of its conversation.
    pub fn ask(
        &mut self,
        question: String,
        first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.running() {
            return;
        }
        if let Some(reason) = blocked(cx) {
            self.notice = Some(reason.into());
            cx.notify();
            return;
        }
        self.notice = None;
        let range = first.then(|| self.seed.range.clone()).flatten();
        let source = self.source_for(&question, first);
        let earlier: Vec<Turn> = self.turns[self.since..]
            .iter()
            .filter(|turn| turn.status == Status::Done)
            .map(|turn| Turn {
                question: turn.question.clone(),
                answer: turn.answer.clone(),
            })
            .collect();
        let index = self.turns.len();
        let markdown = cx.new(|cx| Markdown::new(SharedString::default(), None, None, cx));
        self.turns.push(TurnView {
            question: question.clone(),
            first,
            context: ask_context_summary(range.as_ref(), None),
            tools: Vec::new(),
            answer: String::new(),
            status: Status::Reading,
            kept: None,
            labels: HashMap::new(),
            markdown,
            cancel: None,
            _run: None,
        });
        let index_client = self.host.read(cx).index().cloned();
        let writers = self.writers.clone();
        let today = self.place.today.clone();
        let settings = AiSettings::get_global(cx).clone();
        let runner = cx.global::<Host>().clone();
        let (cancel, canceled) = oneshot::channel::<()>();
        self.turns[index].cancel = Some(cancel);
        let run = cx.spawn_in(window, async move |this, cx| {
            let gathered: Option<Gathered> = match (source, index_client) {
                (Some(source), Some(client)) => {
                    match gather(client, source, &question, &today, writers, cx).await {
                        Ok(gathered) => Some(gathered),
                        Err(error) => {
                            this.update(cx, |this, cx| {
                                this.fail(index, format!("Quire could not read the notebook for this question: {error}. Wait for indexing to finish, then press Retry."), true, cx)
                            })
                            .ok();
                            return;
                        }
                    }
                }
                _ => None,
            };
            let prompt = build_ask_prompt(&question, range.as_ref(), gathered.as_ref(), &earlier);
            let ready = this.update(cx, |this, cx| {
                let turn = &mut this.turns[index];
                turn.context = ask_context_summary(range.as_ref(), gathered.as_ref());
                if turn.context.is_empty() {
                    turn.context.push(NO_CONTEXT.into());
                }
                turn.labels = gathered
                    .iter()
                    .flat_map(|gathered| &gathered.excerpts)
                    .filter_map(|excerpt| Some((excerpt.id.clone()?, excerpt.text.clone())))
                    .collect();
                if prompt.len() > PROMPT_LIMIT {
                    this.fail(index, PROMPT_TOO_LONG.into(), false, cx);
                    return false;
                }
                if turn.status != Status::Reading {
                    return false;
                }
                turn.status = Status::Streaming;
                cx.notify();
                true
            });
            if !matches!(ready, Ok(true)) {
                return;
            }
            // Read here rather than when asked: the notebook window may be mid-update then.
            let transcripts = this
                .update(cx, |this, cx| this.host.read(cx).notebook().transcripts(cx))
                .ok()
                .flatten();
            let request = Request {
                backend: settings.backend,
                mode: Mode::Ask { today },
                binary: settings.binary(settings.backend).to_owned(),
                model: settings.model.clone(),
                prompt,
                timeout: process::ASK_TIMEOUT,
                transcript: transcripts.map(|folder| Transcript {
                    folder,
                    id: format!("ask-{}", uuid::Uuid::new_v4().simple()),
                    page: String::new(),
                }),
            };
            let (updates, mut received) = futures::channel::mpsc::unbounded();
            let running = cx.background_spawn(async move {
                runner
                    .run(
                        request,
                        async {
                            canceled.await.ok();
                        },
                        move |update| {
                            updates.unbounded_send(update).ok();
                        },
                    )
                    .await
            });
            while let Some(update) = received.next().await {
                if this
                    .update(cx, |this, cx| this.progress(index, update, cx))
                    .is_err()
                {
                    return;
                }
            }
            let outcome = running.await;
            this.update(cx, |this, cx| this.finish(index, outcome, cx))
                .ok();
        });
        self.turns[index]._run = Some(run);
        cx.notify();
    }

    fn render_answer(&self, index: usize, cx: &mut Context<Self>) {
        let turn = &self.turns[index];
        let labels = turn.labels.clone();
        let linked = link_citations(&turn.answer, |id| labels.get(id).cloned());
        turn.markdown
            .update(cx, |markdown, cx| markdown.replace(linked, cx));
    }

    fn progress(&mut self, index: usize, update: Update, cx: &mut Context<Self>) {
        let turn = &mut self.turns[index];
        if turn.status != Status::Streaming {
            return;
        }
        match update {
            Update::Token(text) => {
                turn.answer.push_str(&text);
                self.render_answer(index, cx);
            }
            Update::Tool { id, name, detail } => turn.tools.push(ToolRow {
                id,
                label: format!("{name} {detail}").trim().to_owned(),
                done: false,
                failed: false,
            }),
            Update::ToolDone {
                id,
                summary,
                failed,
            } => {
                let row = turn
                    .tools
                    .iter_mut()
                    .find(|row| row.id.is_some() && row.id == id);
                match row {
                    Some(row) => {
                        if !summary.is_empty() {
                            row.label = summary;
                        }
                        row.done = true;
                        row.failed = failed;
                    }
                    None => turn.tools.push(ToolRow {
                        id,
                        label: summary,
                        done: true,
                        failed,
                    }),
                }
            }
        }
        cx.notify();
    }

    fn fail(&mut self, index: usize, message: String, retry: bool, cx: &mut Context<Self>) {
        let turn = &mut self.turns[index];
        turn.status = Status::Failed { message, retry };
        turn.cancel = None;
        cx.notify();
    }

    fn finish(&mut self, index: usize, outcome: Outcome, cx: &mut Context<Self>) {
        if self.turns[index].status != Status::Streaming {
            return;
        }
        match outcome {
            Outcome::Done(text) => {
                let turn = &mut self.turns[index];
                turn.answer = text;
                turn.status = Status::Done;
                turn.cancel = None;
                self.render_answer(index, cx);
            }
            Outcome::Failed { message, retry } => self.fail(index, message, retry, cx),
            Outcome::Canceled => self.fail(index, CANCELED.into(), true, cx),
        }
        cx.notify();
    }

    /// Ends the running question at once; later events from it are dropped.
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if let Some(index) = self
            .turns
            .iter()
            .position(|turn| matches!(turn.status, Status::Reading | Status::Streaming))
        {
            self.fail(index, CANCELED.into(), true, cx);
        }
    }

    fn retry(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let (question, first) = (self.turns[index].question.clone(), self.turns[index].first);
        self.ask(question, first, window, cx);
    }

    /// Sends the question box: a question, or a synthesis instruction, which is remembered and
    /// starts a fresh conversation over the same references.
    pub fn submit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let question = self.input.read(cx).text(cx).trim().to_owned();
        if question.is_empty() || self.running() || blocked(cx).is_some() {
            if let Some(reason) = blocked(cx) {
                self.notice = Some(reason.into());
                cx.notify();
            }
            return;
        }
        let first = if self.editing_synthesis {
            settings::remember_synthesis(&question, cx);
            self.editing_synthesis = false;
            self.since = self.turns.len();
            true
        } else {
            self.turns.len() == self.since
        };
        self.input
            .update(cx, |editor, cx| editor.set_text("", window, cx));
        self.ask(question, first, window, cx);
        let input = self.input.focus_handle(cx);
        window.focus(&input, cx);
    }

    /// Returns to the instruction of a synthesis, to revise it and run again.
    pub fn revise(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let question = self.turns[index].question.clone();
        self.editing_synthesis = true;
        self.input
            .update(cx, |editor, cx| editor.set_text(question, window, cx));
        let input = self.input.focus_handle(cx);
        window.focus(&input, cx);
        cx.notify();
    }

    fn use_default(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.input.update(cx, |editor, cx| {
            editor.set_text(DEFAULT_SYNTHESIS, window, cx)
        });
        let input = self.input.focus_handle(cx);
        window.focus(&input, cx);
    }

    fn choose_preset(&mut self, preset: &Preset, window: &mut Window, cx: &mut Context<Self>) {
        match preset.needs {
            Needs::Daily => cx.emit(AnswerEvent::Tidy),
            Needs::Tag => {
                self.input
                    .update(cx, |editor, cx| editor.set_text(TAGGED_DRAFT, window, cx));
                let input = self.input.focus_handle(cx);
                window.focus(&input, cx);
                self.input.update(cx, |editor, cx| {
                    editor.move_to_end(&editor::actions::MoveToEnd, window, cx)
                });
            }
            _ => {
                let page = self.place.page.clone().unwrap_or_default();
                self.seed.source = Seed::preset(preset, &page).source;
                self.ask(preset.question(&page), true, window, cx);
            }
        }
    }

    /// The suggested questions offered beside the open page, with their labels.
    pub fn presets(&self) -> Vec<(&'static Preset, String)> {
        let page = self.place.page.as_deref();
        PRESETS
            .iter()
            .filter(|preset| preset.offered(page, self.place.daily))
            .map(|preset| (preset, preset.label_for(page)))
            .collect()
    }

    /// Opens what a link in an answer names, as a click on it does.
    #[cfg(test)]
    pub fn click_link(pane: &Entity<Self>, url: &str, window: &mut Window, cx: &mut App) {
        let host = pane.read(cx).host.clone();
        open_link(&host, url, window, cx);
    }

    /// The Markdown source an answer renders, with its citations as links.
    #[cfg(test)]
    pub fn rendered(&self, index: usize, cx: &App) -> String {
        self.turns[index].markdown.read(cx).source().to_string()
    }

    /// Marks turn `index` as kept in the page titled `title`.
    pub fn kept(&mut self, index: usize, title: String, cx: &mut Context<Self>) {
        if let Some(turn) = self.turns.get_mut(index) {
            turn.kept = Some(title);
            cx.notify();
        }
    }

    pub fn show_notice(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice = Some(message.into());
        cx.notify();
    }

    fn render_turn(
        &self,
        index: usize,
        turn: &TurnView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let synthesis = self.seed.synthesis();
        let running = self.running();
        let body = match &turn.status {
            Status::Failed { message, .. } => Label::new(SharedString::from(message.clone()))
                .color(Color::Error)
                .into_any_element(),
            _ if turn.answer.is_empty() => Label::new(if turn.status == Status::Reading {
                READING
            } else {
                THINKING
            })
            .color(Color::Muted)
            .into_any_element(),
            _ => {
                let pane = cx.entity().downgrade();
                MarkdownElement::new(
                    turn.markdown.clone(),
                    MarkdownStyle::themed(MarkdownFont::Agent, window, cx),
                )
                .on_url_click(move |url, window, cx| {
                    if let Some(pane) = pane.upgrade() {
                        let host = pane.read(cx).host.clone();
                        open_link(&host, &url, window, cx);
                    }
                })
                .into_any_element()
            }
        };
        let id = |name: &str| SharedString::from(format!("ai-turn-{index}-{name}"));
        let actions = h_flex()
            .gap_2()
            .when(
                synthesis
                    && !self.editing_synthesis
                    && !matches!(turn.status, Status::Reading | Status::Streaming)
                    && turn.first,
                |row| {
                    row.child(
                        Button::new(id("revise"), "Revise instruction")
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.revise(index, window, cx)
                            })),
                    )
                },
            )
            .when(
                matches!(turn.status, Status::Failed { retry: true, .. }),
                |row| {
                    row.child(
                        Button::new(id("retry"), "Retry")
                            .style(ButtonStyle::Filled)
                            .disabled(running)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.retry(index, window, cx)
                            })),
                    )
                },
            )
            .when(
                matches!(turn.status, Status::Reading | Status::Streaming),
                |row| {
                    row.child(
                        Button::new(id("cancel"), "Cancel")
                            .on_click(cx.listener(|this, _, _, cx| this.cancel(cx))),
                    )
                },
            )
            .when(turn.status == Status::Done, |row| match &turn.kept {
                Some(title) => row.child(
                    Label::new(format!("Kept in {title}"))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
                None => {
                    let answer = turn.answer.clone();
                    row.child(Button::new(id("keep"), "Keep in today's note").on_click(
                        cx.listener(move |_, _, _, cx| {
                            cx.emit(AnswerEvent::Keep {
                                index,
                                answer: answer.clone(),
                            })
                        }),
                    ))
                }
            });
        v_flex()
            .gap_1()
            .pb_3()
            .child(Label::new(SharedString::from(turn.question.clone())))
            .child(
                Label::new(format!("Context sent: {}", turn.context.join(" · ")))
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .children(turn.tools.iter().map(|row| {
                Label::new(if row.done {
                    row.label.clone()
                } else {
                    format!("{}…", row.label)
                })
                .size(LabelSize::Small)
                .color(if row.failed {
                    Color::Error
                } else {
                    Color::Muted
                })
            }))
            .child(body)
            .child(actions)
    }
}

impl Render for AnswerPane {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        use theme::ActiveTheme as _;
        let colors = cx.theme().colors().clone();
        let synthesis = self.seed.synthesis();
        let blocked = blocked(cx).is_some();
        let presets = (!blocked && !synthesis && self.turns.is_empty()).then(|| {
            h_flex()
                .flex_wrap()
                .gap_1()
                .children(self.presets().into_iter().map(|(preset, label)| {
                    Button::new(
                        SharedString::from(format!("ai-preset-{}", preset.key)),
                        label,
                    )
                    .style(ButtonStyle::Subtle)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.choose_preset(preset, window, cx)
                    }))
                }))
        });
        let turns: Vec<_> = self
            .turns
            .iter()
            .enumerate()
            .map(|(index, turn)| self.render_turn(index, turn, window, cx).into_any_element())
            .collect();
        let draft = self.input.read(cx).text(cx);
        let composer = if self.editing_synthesis {
            let source = match &self.seed.source {
                Some(Source::References(page)) => format!("Sends the linked references to {page}."),
                _ => String::new(),
            };
            let shortcut = if cfg!(target_os = "macos") {
                "⌘↩ to run"
            } else {
                "Ctrl-Enter to run"
            };
            v_flex()
                .gap_1()
                .child(Label::new("Instruction").size(LabelSize::Small))
                .child(
                    gpui::div()
                        .key_context(SYNTHESIS_CONTEXT)
                        .p_1()
                        .border_1()
                        .border_color(colors.border)
                        .child(self.input.clone()),
                )
                .child(
                    Label::new(source)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .when(draft.trim() != DEFAULT_SYNTHESIS, |row| {
                            row.child(Button::new("ai-synthesis-default", "Use default").on_click(
                                cx.listener(|this, _, window, cx| this.use_default(window, cx)),
                            ))
                        })
                        .child(
                            Label::new(shortcut)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(
                            Button::new("ai-synthesize", "Synthesize")
                                .style(ButtonStyle::Filled)
                                .disabled(draft.trim().is_empty() || self.running() || blocked)
                                .on_click(
                                    cx.listener(|this, _, window, cx| this.submit(window, cx)),
                                ),
                        ),
                )
                .into_any_element()
        } else {
            h_flex()
                .gap_2()
                .items_end()
                .child(
                    gpui::div()
                        .key_context(INPUT_CONTEXT)
                        .flex_1()
                        .p_1()
                        .border_1()
                        .border_color(colors.border)
                        .child(self.input.clone()),
                )
                .child(
                    Button::new("ai-ask", "Ask")
                        .style(ButtonStyle::Filled)
                        .disabled(blocked || draft.trim().is_empty())
                        .on_click(cx.listener(|this, _, window, cx| this.submit(window, cx))),
                )
                .into_any_element()
        };
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &super::SubmitQuestion, window, cx| {
                if !this.editing_synthesis {
                    this.submit(window, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &super::RunSynthesis, window, cx| {
                if this.editing_synthesis {
                    this.submit(window, cx)
                }
            }))
            .size_full()
            .p_2()
            .gap_2()
            .bg(colors.panel_background)
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(self.title()))
                    .child(gpui::div().flex_1())
                    .child(
                        Button::new("ai-close", "Close")
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(AnswerEvent::Close))),
                    ),
            )
            .when_some(self.notice.clone(), |pane, notice| {
                pane.child(Label::new(notice).color(Color::Warning))
            })
            .child(
                v_flex()
                    .id("ai-answers")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(presets)
                    .children(turns),
            )
            .child(composer)
    }
}
