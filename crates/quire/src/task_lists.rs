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
    App, ClickEvent, Context, Entity, FocusHandle, Focusable as _, KeyBinding, SharedString,
    Subscription, WeakEntity, Window, prelude::*,
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
        let view = cx.new(|cx| TaskListView::new(host, &query, cx));
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
    rich: crate::rich_text::RichCache,
    focus: FocusHandle,
    /// The row the keys act on while the list has focus.
    highlighted: usize,
    /// The due date being typed for one task.
    date: Option<(TaskRef, Entity<Editor>)>,
}

const NO_INDEX: &str =
    "Task lists need the notebook index, which has not started. Wait for indexing to finish.";

/// The key context of a focused task list, inside its page editor.
const LIST_CONTEXT: &str = "QuireTaskQueryList";
/// The contexts a focused list sets: it is a list, and a control the page editor and Vim leave alone.
const LIST_KEYS: &str = "QuireControl QuireList QuireTaskQueryList";
/// The key context around a list's date field.
const DATE_CONTEXT: &str = "QuireTaskQueryDate";

/// Keys for task lists in page editors. Cmd/Ctrl-Enter on a query line moves focus into its
/// list, which answers the list keys, Cmd/Ctrl-Enter to tick, Cmd/Ctrl-Shift-Enter to move to
/// the next state, D for the due date, and Escape to return to the page. The list carries
/// `QuireControl`, so page editor and Vim keys stay with the page; its date field binds the
/// few editing keys a date needs.
pub(crate) fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        editor
            .register_action(move |_: &crate::tasks::ToggleTask, window, cx| {
                let list = handle
                    .upgrade()
                    .and_then(|editor| list_at_caret(editor.read(cx), cx));
                match list {
                    Some(view) => view.update(cx, |view, cx| view.enter(window, cx)),
                    None => cx.propagate(),
                }
            })
            .detach();
    })
    .detach();
    let [tick, cycle, all] = if cfg!(target_os = "macos") {
        ["cmd-enter", "cmd-shift-enter", "cmd-a"]
    } else {
        ["ctrl-enter", "ctrl-shift-enter", "ctrl-a"]
    };
    let date = format!("{DATE_CONTEXT} > Editor");
    let date = Some(date.as_str());
    cx.bind_keys([
        KeyBinding::new(tick, crate::tasks::ToggleTask, Some(LIST_CONTEXT)),
        KeyBinding::new(cycle, crate::tasks::CycleTask, Some(LIST_CONTEXT)),
        KeyBinding::new(
            "d",
            crate::navigation::tasks::EditDueDate,
            Some(LIST_CONTEXT),
        ),
        KeyBinding::new("escape", menu::Cancel, Some(LIST_CONTEXT)),
        KeyBinding::new("enter", menu::Confirm, date),
        KeyBinding::new("escape", menu::Cancel, date),
        KeyBinding::new("backspace", editor::actions::Backspace, date),
        KeyBinding::new("delete", editor::actions::Delete, date),
        KeyBinding::new("left", editor::actions::MoveLeft, date),
        KeyBinding::new("right", editor::actions::MoveRight, date),
        KeyBinding::new(all, editor::actions::SelectAll, date),
    ]);
}

/// The list whose query block holds the newest caret.
fn list_at_caret(editor: &Editor, cx: &App) -> Option<Entity<TaskListView>> {
    let lists = editor.addon::<TaskLists>()?;
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let head = editor
        .selections
        .newest_anchor()
        .head()
        .to_offset(&snapshot);
    lists
        .shown
        .iter()
        .find(|shown| {
            let block = shown.spot.block.start.to_offset(&snapshot)
                ..shown.spot.block.end.to_offset(&snapshot);
            block.start <= head && head <= block.end
        })
        .map(|shown| shown.view.clone())
}

impl TaskListView {
    fn new(editor: WeakEntity<Editor>, source: &str, cx: &mut Context<Self>) -> Self {
        let trimmed = source.trim();
        Self {
            focus: cx.focus_handle(),
            highlighted: 0,
            date: None,
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
            rich: Default::default(),
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

    /// Opens what a link in a task's text names through the page editor's notebook.
    fn on_link(&self, cx: &App) -> crate::rich_text::OnLink {
        let notebook = self
            .editor
            .upgrade()
            .and_then(|editor| crate::references::host(editor.read(cx)))
            .map(|host| host.read(cx).notebook().clone());
        std::rc::Rc::new(move |link, beside, window, cx| {
            if let Some(notebook) = &notebook {
                link.open(beside, notebook.as_ref(), window, cx)
            }
        })
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
        self.write(task_ref, TaskChange::State(action), cx);
    }

    /// Writes a change to a listed task's page through the multi-page writes the Tasks browser
    /// uses, then lists the tasks again.
    fn write(&mut self, task: TaskRef, change: TaskChange, cx: &mut Context<Self>) {
        let writers = self
            .editor
            .upgrade()
            .and_then(|editor| crate::references::host(editor.read(cx)))
            .and_then(|host| host.read(cx).notebook().writers(cx));
        let Some((open, storage)) = writers else {
            self.notice = Some(NO_INDEX.into());
            cx.notify();
            return;
        };
        let work = crate::task_edit::update(vec![task], change, open, storage, cx);
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

    /// The rows the keys move between: tasks, page group labels, and Show more.
    fn stops(rows: &[Row]) -> Vec<usize> {
        rows.iter()
            .enumerate()
            .filter(|(_, row)| matches!(row, Row::Task(_) | Row::Group(_, Some(_)) | Row::More))
            .map(|(index, _)| index)
            .collect()
    }

    /// Moves focus into the list with the first task highlighted.
    fn enter(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.highlighted = Self::stops(&self.rows()).first().copied().unwrap_or(0);
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let stops = Self::stops(&self.rows());
        let Some(last) = stops.len().checked_sub(1) else {
            return;
        };
        let at = stops
            .iter()
            .position(|stop| *stop >= self.highlighted)
            .unwrap_or(last);
        let next = (at as isize).saturating_add(delta).clamp(0, last as isize) as usize;
        self.highlighted = stops[next];
        cx.notify();
    }

    fn highlighted_task(&self) -> Option<TaskItem> {
        match self.rows().get(self.highlighted) {
            Some(Row::Task(task)) => Some(*task.clone()),
            _ => None,
        }
    }

    /// Opens the highlighted task or page group, or shows more tasks.
    fn confirm(&mut self, beside: bool, window: &mut Window, cx: &mut Context<Self>) {
        match self.rows().get(self.highlighted).cloned() {
            Some(Row::Task(task)) => self.open(
                &task.source.page.to_lossy_string(),
                Some(crate::navigation::Jump::at(task.id.clone(), task.from)),
                beside,
                window,
                cx,
            ),
            Some(Row::Group(_, Some(page))) => self.open(&page, None, beside, window, cx),
            Some(Row::More) => self.show_more(cx),
            _ => {}
        }
    }

    fn show_more(&mut self, cx: &mut Context<Self>) {
        self.extra += DEFAULT_LIMIT;
        self.query(cx);
    }

    /// Returns focus to the page, with the caret where it was on the query line.
    fn leave(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(editor) = self.editor.upgrade() {
            window.focus(&editor.focus_handle(cx), cx);
        }
    }

    /// Opens the due date field of the task at row `index`.
    fn edit_due(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Row::Task(task)) = self.rows().get(index).cloned() else {
            return;
        };
        let field = crate::navigation::tasks::field_editor(task.due.as_deref(), window, cx);
        window.focus(&field.focus_handle(cx), cx);
        self.highlighted = index;
        self.date = Some((TaskRef::from(task.as_ref()), field));
        cx.notify();
    }

    /// Sets, changes, or with an empty field clears the typed due date.
    fn apply_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((task, field)) = self.date.take() else {
            return;
        };
        let typed = field.read(cx).text(cx);
        match quire_core::tasks::date_phrase(&typed, self.today.as_deref().unwrap_or_default()) {
            Ok(date) => {
                self.notice = None;
                self.write(task, TaskChange::Date(DateField::Due, date), cx);
                window.focus(&self.focus, cx);
            }
            Err(error) => {
                self.notice = Some(error.into());
                self.date = Some((task, field));
            }
        }
        cx.notify();
    }

    fn cancel_due(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.date = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    fn render_task(&self, index: usize, task: &TaskItem, cx: &mut Context<Self>) -> gpui::Div {
        let done = task.state == TaskState::Done;
        let today = self.today.clone().unwrap_or_default();
        let next = next_state(task.state);
        let due = crate::navigation::tasks::date_chip(task, DateField::Due, &today);
        let overdue = due.as_deref().is_some_and(|due| due.starts_with("Overdue"));
        let field = self
            .date
            .as_ref()
            .filter(|(editing, _)| *editing == TaskRef::from(task))
            .map(|(_, field)| field.clone());
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
                    .child(crate::rich_text::line(
                        ("task-list-line", index),
                        &self.rich.get(&task.text),
                        done,
                        Some(self.on_link(cx)),
                        cx,
                    ))
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
            .child(match field {
                Some(field) => {
                    div()
                        .key_context(DATE_CONTEXT)
                        .debug_selector(|| "quire-task-list-date".into())
                        .w(gpui::px(220.))
                        .px_1()
                        .border_1()
                        .border_color(cx.theme().colors().border_focused)
                        .on_action(cx.listener(|this, _: &menu::Confirm, window, cx| {
                            this.apply_due(window, cx)
                        }))
                        .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                            this.cancel_due(window, cx)
                        }))
                        .child(field)
                        .into_any_element()
                }
                None => {
                    div()
                        .debug_selector(move || format!("quire-task-list-due-{index}"))
                        .child(
                            Button::new(
                                ("task-list-due", index),
                                due.unwrap_or_else(|| "Set due".to_owned()),
                            )
                            .label_size(LabelSize::Small)
                            .color(if overdue { Color::Error } else { Color::Muted })
                            .tooltip(ui::Tooltip::text(
                                "Type a due date · D when the list has focus",
                            ))
                            .on_click(cx.listener(
                                move |this, _, window, cx| this.edit_due(index, window, cx),
                            )),
                        )
                        .into_any_element()
                }
            })
    }
}

/// The state Cmd/Ctrl-Shift-Enter and the state button move a task to.
fn next_state(state: TaskState) -> TaskState {
    match state {
        TaskState::Inbox => TaskState::Next,
        TaskState::Next => TaskState::Waiting,
        TaskState::Waiting => TaskState::Someday,
        TaskState::Someday | TaskState::Done => TaskState::Inbox,
    }
}

impl Render for TaskListView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows();
        let label = format!("Tasks matching {}", self.shown_query);
        let focused = self.focus.contains_focused(window, cx);
        let highlight = cx.theme().colors().element_selected;
        let root = v_flex()
            .id("quire-task-list-rows")
            .key_context(LIST_KEYS)
            .track_focus(&self.focus)
            .role(gpui::Role::List)
            .aria_label(label)
            .size_full()
            .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_action(
                cx.listener(|this, _: &crate::tasks::ToggleTask, window, cx| {
                    if let Some(task) = this.highlighted_task() {
                        this.change(&task, Action::Toggle, window, cx)
                    }
                }),
            )
            .on_action(
                cx.listener(|this, _: &crate::tasks::CycleTask, window, cx| {
                    if let Some(task) = this.highlighted_task() {
                        let next = next_state(task.state);
                        this.change(&task, Action::Set(next), window, cx)
                    }
                }),
            )
            .on_action(cx.listener(
                |this, _: &crate::navigation::tasks::EditDueDate, window, cx| {
                    let index = this.highlighted;
                    this.edit_due(index, window, cx)
                },
            ))
            .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| this.leave(window, cx)));
        crate::navigation::list_keys(
            root,
            cx,
            |this, delta, cx| this.step(delta, cx),
            |this, beside, window, cx| this.confirm(beside, window, cx),
        )
        .children(rows.into_iter().enumerate().map(|(index, row)| {
            let line = h_flex()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .when(focused && index == self.highlighted, |line| {
                    line.bg(highlight)
                });
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
                                .on_click(cx.listener(|this, _, _, cx| this.show_more(cx))),
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
