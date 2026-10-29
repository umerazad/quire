use editor::Editor;
use gpui::{
    App, AppContext, Context, Entity, Focusable, Global, KeyBinding, WeakEntity, Window, actions,
    prelude::*,
};

use std::collections::VecDeque;

actions!(
    quire,
    [
        OpenCommand,
        SubmitCommand,
        DismissCommand,
        PreviousCommand,
        NextCommand
    ]
);

#[derive(Default)]
struct CommandHistory(VecDeque<String>);
impl Global for CommandHistory {}

// Adapted from Zed's command_palette::QueryHistory at the vendored revision.
#[derive(Default)]
struct HistoryCursor {
    selection: Option<usize>,
    prefix: Option<String>,
}

impl HistoryCursor {
    fn navigate(
        &mut self,
        history: &CommandHistory,
        current: &str,
        previous: bool,
    ) -> Option<String> {
        if self
            .selection
            .is_some_and(|index| history.0.get(index).map(String::as_str) != Some(current))
        {
            *self = Self::default();
        }
        if self.selection.is_none() {
            self.prefix = Some(current.to_string());
        }
        let prefix = self.prefix.get_or_insert_with(|| current.to_string());
        let selected = if previous {
            let start = self.selection.unwrap_or(history.0.len());
            (0..start)
                .rev()
                .find(|&i| history.0[i].starts_with(prefix.as_str()))
        } else {
            let start = self.selection? + 1;
            (start..history.0.len()).find(|&i| history.0[i].starts_with(prefix.as_str()))
        };
        if let Some(index) = selected {
            self.selection = Some(index);
            return history.0.get(index).cloned();
        }
        if !previous {
            self.selection = None;
            return self.prefix.take();
        }
        None
    }
}

pub struct CommandPrompt {
    pub(super) input: Entity<Editor>,
    pub(super) error: Option<String>,
    target: Option<WeakEntity<Editor>>,
    epoch: u64,
    history_cursor: HistoryCursor,
}

impl CommandPrompt {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| {
            let mut input = Editor::single_line(window, cx);
            input.set_placeholder_text("Vim command", window, cx);
            input
        });
        cx.subscribe(&input, |this, _, event, cx| {
            if matches!(event, editor::EditorEvent::Edited { .. }) {
                this.epoch = this.epoch.wrapping_add(1);
                this.error = None;
                cx.notify();
            }
        })
        .detach();
        Self {
            input,
            error: None,
            target: None,
            epoch: 0,
            history_cursor: HistoryCursor::default(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.target.is_some()
    }

    pub fn open(&mut self, editor: &Entity<Editor>, window: &mut Window, cx: &mut Context<Self>) {
        let prefix = vim::command_prefix(editor, window, cx);
        self.epoch = self.epoch.wrapping_add(1);
        self.target = Some(editor.downgrade());
        self.error = None;
        self.history_cursor = HistoryCursor::default();
        self.input.update(cx, |input, cx| {
            input.set_text(prefix.clone(), window, cx);
            input.change_selections(Default::default(), window, cx, |selections| {
                let end = editor::MultiBufferOffset(prefix.len());
                selections.select_ranges([end..end]);
            });
        });
        window.focus(&self.input.focus_handle(cx), cx);
        cx.notify();
    }

    fn dismiss(&mut self, _: &DismissCommand, window: &mut Window, cx: &mut Context<Self>) {
        self.remember(cx);
        self.epoch = self.epoch.wrapping_add(1);
        if let Some(editor) = self.target.take().and_then(|target| target.upgrade()) {
            window.focus(&editor.focus_handle(cx), cx);
        }
        self.error = None;
        cx.notify();
    }

    fn submit(&mut self, _: &SubmitCommand, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text(cx);
        self.remember(cx);
        let epoch = self.epoch;
        let parsed = vim::editor_command(&query, cx);
        cx.spawn_in(window, async move |this, cx| {
            let result = parsed.await;
            this.update_in(cx, |this, window, cx| {
                if this.epoch != epoch || this.input.read(cx).text(cx) != query {
                    return;
                }
                match result {
                    Err(error) => this.error = Some(error),
                    Ok(action) => {
                        let Some(editor) = this.target.as_ref().and_then(|target| target.upgrade())
                        else {
                            this.error = Some(
                                "The command's editor closed. Open the page and retry.".into(),
                            );
                            cx.notify();
                            return;
                        };
                        if let Err(error) =
                            vim::validate_editor_command(action.as_ref(), &editor, window, cx)
                        {
                            this.error = Some(error);
                            cx.notify();
                            return;
                        }
                        this.target = None;
                        this.error = None;
                        this.epoch = this.epoch.wrapping_add(1);
                        window.focus(&editor.focus_handle(cx), cx);
                        cx.defer_in(window, move |_, window, cx| {
                            window.dispatch_action(action, cx)
                        });
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn remember(&mut self, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text(cx);
        if !query.is_empty() {
            let history = &mut cx.default_global::<CommandHistory>().0;
            if let Some(index) = history.iter().position(|entry| entry == &query) {
                history.remove(index);
            }
            history.push_back(query);
            if history.len() > 1000 {
                history.pop_front();
            }
        }
        self.history_cursor = HistoryCursor::default();
    }

    fn navigate_history(&mut self, previous: bool, window: &mut Window, cx: &mut Context<Self>) {
        let query = self.input.read(cx).text(cx);
        let Some(query) =
            self.history_cursor
                .navigate(cx.default_global::<CommandHistory>(), &query, previous)
        else {
            return;
        };
        self.input.update(cx, |input, cx| {
            input.set_text(query.clone(), window, cx);
            input.change_selections(Default::default(), window, cx, |selections| {
                let end = editor::MultiBufferOffset(query.len());
                selections.select_ranges([end..end]);
            });
        });
        cx.notify();
    }
}

impl Render for CommandPrompt {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        gpui::div()
            .key_context("QuireEx")
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::dismiss))
            .on_action(cx.listener(|this, _: &PreviousCommand, window, cx| {
                this.navigate_history(true, window, cx)
            }))
            .on_action(cx.listener(|this, _: &NextCommand, window, cx| {
                this.navigate_history(false, window, cx)
            }))
            .p_2()
            .flex()
            .flex_col()
            .child(
                gpui::div()
                    .flex()
                    .gap_2()
                    .child(":")
                    .child(gpui::div().flex_1().h_6().child(self.input.clone())),
            )
            .children(self.error.clone())
    }
}

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(":", OpenCommand, Some("vim_mode == normal")),
        KeyBinding::new(":", OpenCommand, Some("vim_mode == visual")),
        KeyBinding::new("enter", SubmitCommand, Some("QuireEx > Editor")),
        KeyBinding::new("escape", DismissCommand, Some("QuireEx > Editor")),
        KeyBinding::new("ctrl-c", DismissCommand, Some("QuireEx > Editor")),
        KeyBinding::new("up", PreviousCommand, Some("QuireEx > Editor")),
        KeyBinding::new("down", NextCommand, Some("QuireEx > Editor")),
    ]);
}
