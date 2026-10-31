//! Live task lists in Write: a block holding only `{{tasks: QUERY}}` or `{{[[tasks]]: QUERY}}`
//! shows the tasks its query matches in a Zed custom block below its line, the way full embeds
//! show their source. The query folds into a header while the caret is outside the block and
//! shows as written while the caret is in it. Results come from the query index with unsaved
//! edits, refresh when the index changes, and never let an older answer replace a newer one.
//! Ticking or cycling a task on this page edits the page's buffer as one undo step; a task on
//! another page changes through the all-or-nothing page writes the Tasks browser uses.

use crate::navigation::{live::Live, tasks::state_label};
use crate::task_edit::{TaskChange, TaskRef};
use editor::{
    Addon, Editor, FoldPlaceholder,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId, RenderBlock},
};
use gpui::{
    App, ClickEvent, Context, Entity, SharedString, Subscription, WeakEntity, Window, prelude::*,
};
use multi_buffer::{Anchor, ToOffset};
use quire_core::task_query::{DEFAULT_LIMIT, TaskQuery};
use quire_core::tasks::{Action, DateField, TaskState};
use quire_store::index::{IndexClient, TaskItem, TaskList, TaskListQuery};
use std::{any::TypeId, collections::HashMap, ops::Range, sync::Arc};
use ui::{Button, Checkbox, Color, Label, LabelSize, ToggleState, prelude::*};

/// A task query block in the view, in view anchors.
#[derive(Clone)]
pub(crate) struct Spot {
    /// The `{{...}}` text.
    pub range: Range<Anchor>,
    pub block: Range<Anchor>,
    pub source: String,
    pub query: String,
}

struct Shown {
    spot: Spot,
    view: Entity<TaskListView>,
    block: CustomBlockId,
    rows: u32,
    label: SharedString,
    _subscription: Subscription,
}

/// The task lists of one Write view.
#[derive(Default)]
pub(crate) struct TaskLists {
    shown: Vec<Shown>,
}

impl Addon for TaskLists {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

fn state(editor: &mut Editor) -> &mut TaskLists {
    if editor.addon::<TaskLists>().is_none() {
        editor.register_addon(TaskLists::default());
    }
    editor
        .addon_mut::<TaskLists>()
        .expect("registered task lists")
}

/// Shows a list for every query block of the current parse, keeping lists whose query and
/// place are unchanged.
pub(crate) fn sync(
    editor: &mut Editor,
    spots: Vec<Spot>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    if spots.is_empty() && editor.addon::<TaskLists>().is_none() {
        return;
    }
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let mut old = std::mem::take(&mut state(editor).shown);
    let mut kept = Vec::new();
    let mut fresh = Vec::new();
    for spot in spots {
        let at = spot.range.start.to_offset(&snapshot);
        let found = old.iter().position(|shown| {
            shown.spot.range.start.to_offset(&snapshot) == at && shown.spot.query == spot.query
        });
        if let Some(index) = found {
            let mut shown = old.remove(index);
            shown.spot = spot;
            kept.push(shown);
            continue;
        }
        let host = cx.entity().downgrade();
        let query = spot.query.clone();
        let view = cx.new(|_| TaskListView::new(host, &query));
        let rows = view.read(cx).rows().len() as u32;
        let label = view.read(cx).header();
        let block = editor.insert_blocks(
            [BlockProperties {
                placement: BlockPlacement::Below(spot.range.start),
                height: Some(rows),
                style: BlockStyle::Flex,
                render: render(view.clone()),
                priority: 0,
            }],
            None,
            cx,
        )[0];
        let subscription = cx.observe_in(&view, window, |editor, view, window, cx| {
            let (rows, label) = {
                let view = view.read(cx);
                (view.rows().len() as u32, view.header())
            };
            let lists = state(editor);
            let Some(shown) = lists.shown.iter_mut().find(|shown| shown.view == view) else {
                return;
            };
            let block = shown.block;
            let resized = shown.rows != rows;
            let relabeled = shown.label != label;
            shown.rows = rows;
            shown.label = label;
            if resized {
                editor.resize_blocks(HashMap::from_iter([(block, rows)]), None, cx);
            }
            if relabeled {
                crate::editor_projection::refold(editor, window, cx);
            }
        });
        fresh.push(view.clone());
        kept.push(Shown {
            spot,
            view,
            block,
            rows,
            label,
            _subscription: subscription,
        });
    }
    let removed = old.into_iter().map(|shown| shown.block).collect();
    editor.remove_blocks(removed, None, cx);
    kept.sort_by_key(|shown| shown.spot.range.start.to_offset(&snapshot));
    state(editor).shown = kept;
    for view in fresh {
        ask(editor, &view, cx);
    }
}

/// The index answers differently: list every query again.
pub(crate) fn requery(editor: &mut Editor, cx: &mut Context<Editor>) {
    let views: Vec<_> = editor
        .addon::<TaskLists>()
        .map(|lists| lists.shown.iter().map(|shown| shown.view.clone()).collect())
        .unwrap_or_default();
    for view in views {
        ask(editor, &view, cx);
    }
}

/// Removes every list, for Source view.
pub(crate) fn clear(editor: &mut Editor, cx: &mut Context<Editor>) {
    let Some(lists) = editor.addon_mut::<TaskLists>() else {
        return;
    };
    let blocks = lists.shown.drain(..).map(|shown| shown.block).collect();
    editor.remove_blocks(blocks, None, cx);
}

/// Sends a list's query with this editor's page as `here`.
fn ask(editor: &Editor, view: &Entity<TaskListView>, cx: &mut Context<Editor>) {
    let Some(host) = crate::references::host(editor) else {
        return;
    };
    let client = host.read(cx).index().cloned();
    let here = editor
        .buffer()
        .read(cx)
        .all_buffers_iter()
        .next()
        .and_then(|buffer| host.read(cx).notebook().page_id(&buffer, cx));
    let today = cx.global::<crate::tasks::TaskContext>().today();
    view.update(cx, |view, cx| {
        view.here = here;
        view.request(client, today, cx)
    });
}

/// Whether the page at `path` is the Templates page, where task queries belong to templates
/// and stay as written.
pub(crate) fn is_templates_page(path: &str) -> bool {
    path.to_lowercase() == "templates.md"
}

fn render(view: Entity<TaskListView>) -> RenderBlock {
    Arc::new(move |block| {
        use theme::ActiveTheme;
        let colors = block.app.theme().colors();
        gpui::div()
            .debug_selector(|| "quire-task-list".into())
            .ml(block.anchor_x)
            .pl_2()
            .w_full()
            .h(block.line_height * block.height as f32)
            .border_l_2()
            .border_color(colors.border)
            .child(view.clone())
            .into_any_element()
    })
}

/// A header for each list, to fold its query into. Clicking it puts the caret in the block,
/// showing the query for editing.
pub(crate) fn headers(
    editor: &Editor,
    host: WeakEntity<Editor>,
    type_tag: TypeId,
) -> Vec<(Spot, FoldPlaceholder)> {
    let Some(lists) = editor.addon::<TaskLists>() else {
        return Vec::new();
    };
    lists
        .shown
        .iter()
        .enumerate()
        .map(|(ordinal, shown)| {
            let label = shown.label.clone();
            let host = host.clone();
            let placeholder = FoldPlaceholder {
                collapsed_text: Some("☰".into()),
                constrain_width: false,
                merge_adjacent: false,
                type_tag: Some(type_tag),
                render: Arc::new(move |_, range, cx| {
                    use theme::ActiveTheme;
                    let colors = cx.theme().colors();
                    let host = host.clone();
                    crate::editor_projection::inline_control(
                        ("quire-task-list-header", ordinal),
                        host.clone(),
                    )
                    .debug_selector(|| "quire-task-list-header".into())
                    .role(gpui::Role::Button)
                    .aria_label(label.clone())
                    .cursor_pointer()
                    .text_color(colors.text_muted)
                    .child(label.clone())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        let start = range.start;
                        host.update(cx, |editor, cx| {
                            window.focus(&gpui::Focusable::focus_handle(editor, cx), cx);
                            editor.change_selections(
                                Default::default(),
                                window,
                                cx,
                                |selections| selections.select_anchor_ranges([start..start]),
                            );
                        })
                        .ok();
                    })
                    .into_any_element()
                }),
            };
            (shown.spot.clone(), placeholder)
        })
        .collect()
}

/// One line of a list.
#[derive(Clone, Debug, PartialEq)]
enum Row {
    Error(SharedString),
    Loading,
    Empty,
    Group(SharedString, Option<String>),
    Task(Box<TaskItem>),
    More,
    Notice(SharedString),
}

/// The tasks one query block lists.
pub(crate) struct TaskListView {
    editor: WeakEntity<Editor>,
    query: Result<TaskQuery, String>,
    shown_query: SharedString,
    /// The page holding the query, as `here` names it.
    here: Option<String>,
    today: Option<String>,
    client: Option<IndexClient>,
    /// Tasks shown beyond the query's limit after Show more.
    extra: usize,
    list: Live<Option<TaskList>>,
    notice: Option<SharedString>,
    work: Option<gpui::Task<()>>,
}

const NO_INDEX: &str =
    "Task lists need the notebook index, which has not started. Wait for indexing to finish.";

impl TaskListView {
    fn new(editor: WeakEntity<Editor>, source: &str) -> Self {
        let trimmed = source.trim();
        Self {
            editor,
            query: TaskQuery::parse(source),
            shown_query: if trimmed.is_empty() { "here" } else { trimmed }
                .to_owned()
                .into(),
            here: None,
            today: None,
            client: None,
            extra: 0,
            list: Live::default(),
            notice: None,
            work: None,
        }
    }

    fn request(
        &mut self,
        client: Option<IndexClient>,
        today: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.client = client;
        self.today = today;
        self.query(cx);
    }

    fn query(&mut self, cx: &mut Context<Self>) {
        let Ok(query) = &self.query else {
            return;
        };
        let Some(today) = self.today.clone() else {
            self.list.error = Some(crate::tasks::NO_TIME_ZONE.into());
            cx.notify();
            return;
        };
        let Some(client) = self.client.clone() else {
            self.list.error = Some(NO_INDEX.into());
            cx.notify();
            return;
        };
        let mut query = query.clone();
        query.limit = query.limit.saturating_add(self.extra);
        let list = TaskListQuery {
            query,
            here: self.here.clone().unwrap_or_default(),
            today,
        };
        let answer = client.with_generation(move |index| index.task_list(&list).map(Some));
        self.list.request(
            answer,
            cx,
            |this| &mut this.list,
            |error| format!("Cannot list tasks: {error}. Wait for indexing to finish, then retry."),
        );
    }

    /// The header the query folds into: the query and how many tasks it shows.
    fn header(&self) -> SharedString {
        let count = match (&self.query, &self.list.value) {
            (Err(_), _) => " · query error".to_owned(),
            (Ok(_), Some(list)) if list.shown() == list.total => format!(" · {}", list.total),
            (Ok(_), Some(list)) => format!(" · {} of {}", list.shown(), list.total),
            (Ok(_), None) => String::new(),
        };
        format!("☰ Tasks · {}{count}", self.shown_query).into()
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        if let Some(notice) = &self.notice {
            rows.push(Row::Notice(notice.clone()));
        }
        match (&self.query, &self.list.value) {
            (Err(error), _) => rows.push(Row::Error(error.clone().into())),
            (Ok(_), None) => rows.push(match &self.list.error {
                Some(error) => Row::Error(error.clone()),
                None => Row::Loading,
            }),
            (Ok(_), Some(list)) => {
                if let Some(error) = &self.list.error {
                    rows.push(Row::Error(error.clone()));
                }
                if list.total == 0 {
                    rows.push(Row::Empty);
                }
                for group in &list.groups {
                    if !group.label.is_empty() {
                        rows.push(Row::Group(group.label.clone().into(), group.page.clone()));
                    }
                    rows.extend(
                        group
                            .tasks
                            .iter()
                            .map(|task| Row::Task(Box::new(task.clone()))),
                    );
                }
                if list.shown() < list.total {
                    rows.push(Row::More);
                }
            }
        }
        rows
    }

    /// Opens `page`, at `jump` when a task row names its block.
    fn open(
        &self,
        page: &str,
        jump: Option<crate::navigation::Jump>,
        beside: bool,
        window: &mut Window,
        cx: &mut App,
    ) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        if let Some(host) = crate::references::host(editor.read(cx)) {
            let notebook = host.read(cx).notebook().clone();
            match jump {
                Some(jump) => notebook.show_page_at(page, jump, beside, window, cx),
                None => notebook.show_page(page, beside, window, cx),
            }
        }
    }

    /// Ticks or moves a listed task: through this page's editor when the task is here, as one
    /// undo step, otherwise by writing its page.
    fn change(
        &mut self,
        task: &TaskItem,
        action: Action,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(editor) = self.editor.upgrade() else {
            return;
        };
        self.notice = None;
        let task_ref = TaskRef::from(task);
        if self.here.as_deref() == Some(task_ref.page.as_str()) {
            let changed = editor.update(cx, |editor, cx| {
                crate::tasks::change_listed(editor, &task_ref, action, window, cx)
            });
            match changed {
                Ok(true) => {
                    cx.notify();
                    return;
                }
                Ok(false) => {}
                Err(error) => {
                    self.notice = Some(error.into());
                    cx.notify();
                    return;
                }
            }
        }
        let writers = crate::references::host(editor.read(cx))
            .and_then(|host| host.read(cx).notebook().writers(cx));
        let Some((open, storage)) = writers else {
            self.notice = Some(NO_INDEX.into());
            cx.notify();
            return;
        };
        let work =
            crate::task_edit::update(vec![task_ref], TaskChange::State(action), open, storage, cx);
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.work = None;
                if let Err(error) = result {
                    this.notice = Some(error.into());
                }
                this.query(cx);
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn render_task(&self, index: usize, task: &TaskItem, cx: &mut Context<Self>) -> gpui::Div {
        let done = task.state == TaskState::Done;
        let today = self.today.clone().unwrap_or_default();
        let next = match task.state {
            TaskState::Inbox => TaskState::Next,
            TaskState::Next => TaskState::Waiting,
            TaskState::Waiting => TaskState::Someday,
            TaskState::Someday | TaskState::Done => TaskState::Inbox,
        };
        let due = crate::navigation::tasks::date_chip(task, DateField::Due, &today);
        let ticked = task.clone();
        let cycled = task.clone();
        let page = task.source.page.to_lossy_string();
        let jump = crate::navigation::Jump::at(task.id.clone(), task.from);
        h_flex()
            .gap_2()
            .child(
                div()
                    .debug_selector(|| format!("quire-task-list-tick-{index}"))
                    .child(
                        Checkbox::new(
                            ("task-list-tick", index),
                            if done {
                                ToggleState::Selected
                            } else {
                                ToggleState::Unselected
                            },
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                this.change(&ticked, Action::Toggle, window, cx)
                            },
                        )),
                    ),
            )
            .child(
                div()
                    .id(("task-list-text", index))
                    .cursor_pointer()
                    .overflow_hidden()
                    .child(Label::new(task.text.clone()).truncate().color(if done {
                        Color::Muted
                    } else {
                        Color::Default
                    }))
                    .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                        this.open(
                            &page,
                            Some(jump.clone()),
                            event.modifiers().shift,
                            window,
                            cx,
                        )
                    })),
            )
            .child(
                div()
                    .debug_selector(|| format!("quire-task-list-state-{index}"))
                    .child(
                        Button::new(("task-list-state", index), state_label(task.state))
                            .label_size(LabelSize::Small)
                            .color(Color::Accent)
                            .on_click(cx.listener(move |this, _, window, cx| {
                                this.change(&cycled, Action::Set(next), window, cx)
                            })),
                    ),
            )
            .child(
                Label::new(task.source.title.clone())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .children(due.map(|due| Label::new(due).size(LabelSize::Small).color(Color::Muted)))
    }
}

impl Render for TaskListView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let label = format!("Tasks matching {}", self.shown_query);
        v_flex()
            .id("quire-task-list-rows")
            .role(gpui::Role::List)
            .aria_label(label)
            .size_full()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(rows.into_iter().enumerate().map(|(index, row)| {
                let line = h_flex().flex_1().min_h_0().overflow_hidden();
                match row {
                    Row::Error(error) | Row::Notice(error) => line.child(
                        Label::new(format!("⚠ {error}"))
                            .size(LabelSize::Small)
                            .color(Color::Error),
                    ),
                    Row::Loading => line.child(
                        Label::new("Loading tasks…")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                    Row::Empty => line.child(
                        Label::new("No tasks match.")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                    Row::Group(label, page) => line.child(match page {
                        Some(page) => Button::new(("task-list-group", index), label)
                            .label_size(LabelSize::Small)
                            .end_icon(Icon::new(IconName::ArrowUpRight).size(IconSize::XSmall))
                            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                                this.open(&page, None, event.modifiers().shift, window, cx)
                            }))
                            .into_any_element(),
                        None => Label::new(label)
                            .size(LabelSize::Small)
                            .color(Color::Muted)
                            .into_any_element(),
                    }),
                    Row::Task(task) => line.child(self.render_task(index, &task, cx)),
                    Row::More => line.child(
                        div()
                            .debug_selector(|| "quire-task-list-more".into())
                            .child(
                                Button::new("task-list-more", "Show more tasks")
                                    .label_size(LabelSize::Small)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.extra += DEFAULT_LIMIT;
                                        this.query(cx);
                                    })),
                            ),
                    ),
                }
            }))
    }
}

/// The header of each list in this view, in page order.
#[cfg(test)]
pub(crate) fn header_labels(editor: &Editor) -> Vec<String> {
    editor
        .addon::<TaskLists>()
        .map(|lists| {
            lists
                .shown
                .iter()
                .map(|shown| shown.label.to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// The lines each list in this view shows, as plain text, in page order.
#[cfg(test)]
pub(crate) fn lines(editor: &Editor, cx: &App) -> Vec<Vec<String>> {
    editor
        .addon::<TaskLists>()
        .map(|lists| {
            lists
                .shown
                .iter()
                .map(|shown| {
                    shown
                        .view
                        .read(cx)
                        .rows()
                        .into_iter()
                        .map(|row| match row {
                            Row::Error(text) => format!("error: {text}"),
                            Row::Notice(text) => format!("notice: {text}"),
                            Row::Loading => "Loading tasks…".into(),
                            Row::Empty => "No tasks match.".into(),
                            Row::Group(label, _) => format!("## {label}"),
                            Row::Task(task) => format!(
                                "{} {} · {} · {}",
                                if task.state == TaskState::Done {
                                    "[x]"
                                } else {
                                    "[ ]"
                                },
                                task.text,
                                state_label(task.state),
                                task.source.title
                            ),
                            Row::More => "Show more tasks".into(),
                        })
                        .collect()
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether every list in this view shows the answer to its latest query and no change is
/// being written.
#[cfg(test)]
pub(crate) fn settled(editor: &Editor, cx: &App) -> bool {
    editor.addon::<TaskLists>().is_none_or(|lists| {
        lists.shown.iter().all(|shown| {
            let view = shown.view.read(cx);
            view.list.settled() && view.work.is_none()
        })
    })
}
