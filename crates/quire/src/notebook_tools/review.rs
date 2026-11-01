//! The AI change list (reference `AiChangeList`): the changes the AI proposed through the
//! notebook tools for some runs, each with its diff, Apply, Reject, and Undo, and Apply all. The
//! answer pane shows one under each question and one for "Changes from a terminal"; the AI
//! changes window shows one for every run.

use super::changes::{self, Change, Request, Status};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, SharedString, Task, Window, WindowOptions,
    prelude::*, px,
};
use quire_core::ai::tools::{LineChange, diff_lines};
use theme::ActiveTheme as _;
use ui::{
    Button, ButtonCommon as _, ButtonStyle, Clickable as _, Color, Label, LabelCommon as _, h_flex,
    v_flex,
};

pub const CONTEXT: &str = "QuireAiChanges";
/// The pane's list of changes from terminal sessions.
pub const TERMINAL_TITLE: &str = "Changes from a terminal";
/// The list under a question.
pub const RUN_TITLE: &str = "Changes";
const WINDOW_TITLE: &str = "AI changes";
const NONE_YET: &str =
    "No AI changes yet. Changes a question or a terminal session proposes wait here for review.";

/// The reference `label`: what the change does, in a line.
pub fn label(request: &Request) -> String {
    let page = format!("[[{}]]", request.title());
    match request {
        Request::Append { .. } => format!("Add to {page}"),
        Request::Create { .. } => format!("Create {page}"),
        Request::Replace { .. } => format!("Edit a block on {page}"),
        Request::TaskState { state, .. } => format!("Mark a task {} on {page}", state.as_str()),
        Request::Capture { .. } => format!("Capture a task in {page}"),
    }
}

/// The diff rows the list shows: `+`, `−` or a space, then the line. A change with no text
/// before shows only what it adds.
pub fn diff(request: &Request) -> Vec<String> {
    let (before, after) = request.texts();
    diff_lines(before, after)
        .into_iter()
        .filter(|(kind, _)| !before.is_empty() || *kind != LineChange::Remove)
        .map(|(kind, line)| {
            let mark = match kind {
                LineChange::Add => "+",
                LineChange::Remove => "\u{2212}",
                LineChange::Same => " ",
            };
            format!("{mark} {line}")
        })
        .collect()
}

/// The status line of a change that is no longer staged.
pub fn status_text(change: &Change) -> &'static str {
    match change.status {
        Status::Applied if created(change) => "Applied as a new page",
        Status::Applied => "Applied",
        Status::Rejected => "Rejected",
        Status::Undone => "Undone",
        Status::Staged => "",
    }
}

fn created(change: &Change) -> bool {
    change
        .applied
        .as_ref()
        .is_some_and(|applied| applied.created)
}

/// The changes of some runs, or of every run, with their actions.
pub struct ChangeList {
    runs: Option<Vec<String>>,
    title: SharedString,
    /// Shown when there are no changes; without it an empty list draws nothing.
    empty: Option<SharedString>,
    pub error: Option<String>,
    _pending: Option<Task<()>>,
}

impl ChangeList {
    pub fn new(
        runs: Option<Vec<String>>,
        title: impl Into<SharedString>,
        empty: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.observe(&changes::ledger(cx), |_, _, cx| cx.notify())
            .detach();
        Self {
            runs,
            title: title.into(),
            empty,
            error: None,
            _pending: None,
        }
    }

    /// The list under a question, or the pane's list of terminal changes.
    pub fn for_run(run: &str, title: &'static str, cx: &mut App) -> Entity<Self> {
        let runs = Some(vec![run.to_owned()]);
        cx.new(|cx| Self::new(runs, title, None, cx))
    }

    pub fn changes(&self, cx: &App) -> Vec<Change> {
        let mut listed = changes::list(None, cx);
        if let Some(runs) = &self.runs {
            listed.retain(|change| runs.contains(&change.run));
        }
        listed
    }

    /// Runs `action`, showing its error under the title when it fails.
    fn attempt(
        &mut self,
        action: impl FnOnce(&mut App) -> Task<Result<(), String>>,
        cx: &mut Context<Self>,
    ) {
        self.error = None;
        let task = action(cx);
        self._pending = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.error = result.err();
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn apply(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.attempt(move |cx| changes::apply(&id, cx), cx);
    }

    pub fn undo(&mut self, id: &str, cx: &mut Context<Self>) {
        let id = id.to_owned();
        self.attempt(move |cx| changes::undo(&id, cx), cx);
    }

    /// Applies every staged change in the list, one run after another, and reports how many
    /// failed.
    pub fn apply_all(&mut self, cx: &mut Context<Self>) {
        let staged: Vec<Change> = self
            .changes(cx)
            .into_iter()
            .filter(|change| change.status == Status::Staged)
            .collect();
        let mut runs: Vec<String> = staged.iter().map(|change| change.run.clone()).collect();
        runs.sort_unstable();
        runs.dedup();
        let total = staged.len();
        self.attempt(
            move |cx| {
                let applying: Vec<_> = runs.iter().map(|run| changes::apply_all(run, cx)).collect();
                cx.spawn(async move |_| {
                    let mut failed = Vec::new();
                    for task in applying {
                        failed.extend(task.await);
                    }
                    match failed.first() {
                        None => Ok(()),
                        Some((_, first)) => Err(format!(
                            "{} of {total} changes did not apply: {first}",
                            failed.len()
                        )),
                    }
                })
            },
            cx,
        );
    }

    fn row(&self, change: &Change, cx: &mut Context<Self>) -> gpui::AnyElement {
        let colors = cx.theme().colors();
        let id = change.id.clone();
        let actions = match change.status {
            Status::Staged => h_flex()
                .gap_1()
                .child(
                    Button::new(format!("apply-{id}"), "Apply")
                        .style(ButtonStyle::Filled)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| this.apply(&id, cx)
                        })),
                )
                .child(Button::new(format!("reject-{id}"), "Reject").on_click({
                    let id = id.clone();
                    move |_, _, cx| changes::reject(&id, cx)
                })),
            _ => h_flex()
                .gap_1()
                .child(Label::new(status_text(change)).color(Color::Muted))
                .when(
                    change.status == Status::Applied && !created(change),
                    |this| {
                        this.child(
                            Button::new(format!("undo-{id}"), "Undo")
                                .on_click(cx.listener(move |this, _, _, cx| this.undo(&id, cx))),
                        )
                    },
                ),
        };
        v_flex()
            .gap_1()
            .pt_2()
            .child(Label::new(label(&change.request)))
            .child(
                v_flex()
                    .p_2()
                    .rounded_sm()
                    .bg(colors.editor_background)
                    .font_family("Lilex")
                    .children(diff(&change.request).into_iter().map(|line| {
                        let color = match line.chars().next() {
                            Some('+') => colors.version_control_added,
                            Some('\u{2212}') => colors.version_control_deleted,
                            _ => colors.text,
                        };
                        gpui::div().text_color(color).child(line)
                    })),
            )
            .child(actions)
            .into_any_element()
    }
}

impl Render for ChangeList {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let list = self.changes(cx);
        if list.is_empty() && self.empty.is_none() {
            return gpui::div().into_any_element();
        }
        let staged = list
            .iter()
            .filter(|change| change.status == Status::Staged)
            .count();
        let rows: Vec<_> = list.iter().map(|change| self.row(change, cx)).collect();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(Label::new(format!("{} \u{b7} {}", self.title, list.len())))
                    .when(staged > 1, |this| {
                        this.child(
                            Button::new("apply-all", "Apply all")
                                .style(ButtonStyle::Filled)
                                .on_click(cx.listener(|this, _, _, cx| this.apply_all(cx))),
                        )
                    }),
            )
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).color(Color::Error))
            })
            .children(rows)
            .when(list.is_empty(), |this| {
                this.children(
                    self.empty
                        .clone()
                        .map(|empty| Label::new(empty).color(Color::Muted)),
                )
            })
            .into_any_element()
    }
}

/// The AI changes window: every run's changes in one list.
pub struct AiChangesWindow {
    focus: FocusHandle,
    pub list: Entity<ChangeList>,
}

/// Opens the AI changes window, or brings the open one forward.
pub fn open(cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<AiChangesWindow>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(px(640.), px(720.)),
            cx,
        ))),
        ..crate::title_bar::base_options()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_window_title(WINDOW_TITLE);
        cx.new(|cx| AiChangesWindow::new(window, cx))
    });
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Cannot open AI changes: {error}. Check desktop access, then retry."),
            cx,
        );
    }
}

impl AiChangesWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        crate::preferences::follow_system_appearance(window, cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let list = cx.new(|cx| ChangeList::new(None, WINDOW_TITLE, Some(NONE_YET.into()), cx));
        Self { focus, list }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }
}

impl Focusable for AiChangesWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for AiChangesWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .child(
                v_flex()
                    .id("ai-changes")
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .overflow_y_scroll()
                    .child(self.list.clone()),
            )
    }
}
