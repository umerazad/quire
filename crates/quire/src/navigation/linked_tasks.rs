//! Tasks elsewhere that link to the shown page, above its linked references, as the reference
//! `LinkedTasks` shows them: All keeps the references; Open, Due, and Done list the matching
//! tasks to tick off or open.

use super::{Jump, list_keys, live::Live, tag_edits::Writers, tasks::date_chip};
use crate::task_edit::{self, TaskChange, TaskRef};
use crate::tasks::ToggleTask;
use gpui::{
    App, ClickEvent, Context, EventEmitter, FocusHandle, Focusable, ScrollStrategy, Task,
    UniformListScrollHandle, Window, px, uniform_list,
};
use quire_core::{
    tasks::{Action, DateField, TaskState},
    workspace::TaskStatus,
};
use quire_store::index::{IndexClient, TaskItem};
use ui::{
    Button, ButtonStyle, Checkbox, Color, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing,
    ToggleState, Toggleable, prelude::*, v_flex,
};

/// Tasks listed before "Show more tasks".
const PAGE: usize = 40;

pub enum LinkedTasksEvent {
    Open {
        page: String,
        jump: Jump,
        beside: bool,
    },
}

pub struct LinkedTasksView {
    page: String,
    pub(crate) tasks: Live<Vec<TaskItem>>,
    pub(crate) status: TaskStatus,
    limit: usize,
    today: String,
    highlighted: usize,
    writers: Writers,
    work: Option<Task<()>>,
    error: Option<SharedString>,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
}

impl EventEmitter<LinkedTasksEvent> for LinkedTasksView {}

impl Focusable for LinkedTasksView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl LinkedTasksView {
    pub fn new(page: String, writers: Writers, cx: &mut Context<Self>) -> Self {
        Self {
            page,
            tasks: Live::default(),
            status: TaskStatus::All,
            limit: PAGE,
            today: String::new(),
            highlighted: 0,
            writers,
            work: None,
            error: None,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
        }
    }

    /// Whether the linked references give way to a task list.
    pub fn lists_tasks(&self) -> bool {
        self.status != TaskStatus::All && !self.tasks.value.is_empty()
    }

    pub fn refresh(&mut self, client: &IndexClient, today: String, cx: &mut Context<Self>) {
        self.today = today;
        let page = self.page.clone();
        let answer = client.with_generation(move |index| index.linked_tasks(&page));
        self.tasks.request(
            answer,
            cx,
            |this| &mut this.tasks,
            |error| {
                format!(
                    "Cannot list linked tasks: {error}. Wait for indexing to finish, then retry."
                )
            },
        );
    }

    pub fn shown(&self) -> Vec<TaskItem> {
        self.tasks
            .value
            .iter()
            .filter(|task| task.has_status(self.status, &self.today))
            .take(self.limit)
            .cloned()
            .collect()
    }

    fn count(&self, status: TaskStatus) -> usize {
        self.tasks
            .value
            .iter()
            .filter(|task| task.has_status(status, &self.today))
            .count()
    }

    pub fn set_status(&mut self, status: TaskStatus, cx: &mut Context<Self>) {
        self.status = status;
        self.limit = PAGE;
        self.highlighted = 0;
        cx.notify();
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.shown().len();
        if count == 0 {
            return;
        }
        self.highlighted = (self.highlighted as isize)
            .saturating_add(delta)
            .clamp(0, count as isize - 1) as usize;
        self.scroll
            .scroll_to_item(self.highlighted, ScrollStrategy::Top);
        cx.notify();
    }

    fn open(&mut self, index: usize, beside: bool, cx: &mut Context<Self>) {
        if let Some(task) = self.shown().get(index) {
            self.highlighted = index;
            cx.emit(LinkedTasksEvent::Open {
                page: task.source.page.to_lossy_string(),
                jump: Jump::at(task.id.clone(), task.from),
                beside,
            });
        }
    }

    /// Ticks the task at `index`: completes it, or reopens a done one.
    pub fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(task) = self.shown().get(index).map(TaskRef::from) else {
            return;
        };
        let (open, storage) = (self.writers)(cx);
        let work = task_edit::update(
            vec![task],
            TaskChange::State(Action::Toggle),
            open,
            storage,
            cx,
        );
        self.error = None;
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.work = None;
                if let Err(error) = result {
                    this.error = Some(error.into());
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn render_task(&self, index: usize, task: &TaskItem, cx: &mut Context<Self>) -> ListItem {
        let done = task.state == TaskState::Done;
        let due = date_chip(task, DateField::Due, &self.today);
        ListItem::new(("linked-task", index))
            .spacing(ListItemSpacing::Dense)
            .toggle_state(index == self.highlighted)
            .start_slot(
                Checkbox::new(
                    ("linked-task-tick", index),
                    if done {
                        ToggleState::Selected
                    } else {
                        ToggleState::Unselected
                    },
                )
                .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx))),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new(task.text.clone()).truncate().color(if done {
                        Color::Muted
                    } else {
                        Color::Default
                    }))
                    .child(
                        Label::new(task.source.title.clone())
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .children(
                        due.map(|due| Label::new(due).size(LabelSize::Small).color(Color::Muted)),
                    ),
            )
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.open(index, event.modifiers().shift, cx)
            }))
    }
}

impl Render for LinkedTasksView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.tasks.value.is_empty() {
            return div().into_any_element();
        }
        let statuses = TaskStatus::ALL.map(|status| {
            let label = match status {
                TaskStatus::All => status.label().to_owned(),
                _ => format!("{} {}", status.label(), self.count(status)),
            };
            Button::new(
                SharedString::from(format!("linked-status-{status:?}")),
                label,
            )
            .style(ButtonStyle::Subtle)
            .toggle_state(self.status == status)
            .on_click(cx.listener(move |this, _, _, cx| this.set_status(status, cx)))
        });
        let shown = self.shown();
        let total = self.count(self.status);
        let empty = match self.status {
            TaskStatus::Done => "No linked tasks are done yet.",
            TaskStatus::Due => "No linked tasks are due.",
            _ => "Every linked task is done.",
        };
        let root = v_flex()
            .id("linked-tasks")
            .key_context("QuireList QuireLinkedTasks")
            .track_focus(&self.focus)
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .p_1()
            .gap_1()
            .on_action(cx.listener(|this, _: &ToggleTask, _, cx| {
                let index = this.highlighted;
                this.toggle(index, cx)
            }))
            .when(self.lists_tasks() && !shown.is_empty(), |root| {
                root.h(px(240.))
            });
        list_keys(
            root,
            cx,
            |this, delta, cx| this.move_highlight(delta, cx),
            |this, beside, _, cx| {
                let index = this.highlighted;
                this.open(index, beside, cx)
            },
        )
        .child(
            h_flex()
                .gap_1()
                .child(Label::new("Linked tasks").size(LabelSize::Small))
                .children(statuses),
        )
        .when_some(
            self.tasks.error.clone().or(self.error.clone()),
            |root, error| root.child(Label::new(error).color(Color::Error)),
        )
        .when(self.lists_tasks(), |root| {
            root.when(shown.is_empty(), |root| {
                root.child(Label::new(empty).color(Color::Muted))
            })
            .when(!shown.is_empty(), |root| {
                root.child(
                    uniform_list(
                        "linked-task-rows",
                        shown.len(),
                        cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                            range
                                .filter_map(|index| {
                                    Some(this.render_task(index, shown.get(index)?, cx))
                                })
                                .collect()
                        }),
                    )
                    .track_scroll(&self.scroll)
                    .flex_1(),
                )
            })
            .when(self.limit < total, |root| {
                root.child(
                    Button::new("linked-tasks-more", "Show more tasks")
                        .style(ButtonStyle::Subtle)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.limit += PAGE;
                            cx.notify();
                        })),
                )
            })
        })
        .into_any_element()
    }
}
