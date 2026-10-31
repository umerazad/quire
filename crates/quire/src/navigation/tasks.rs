//! The Tasks browser: capture, the Due, Upcoming, Inbox, Next, Waiting, Someday, and Done lists,
//! and Projects with their next actions. Rows tick, change state, take dates, and change in bulk
//! through `task_edit`, which writes every touched page or none.

use super::{list_keys, live::Live, tag_edits::Writers};
use crate::task_edit::{self, CaptureProject, CaptureTo, TaskChange, TaskRef};
use crate::tasks::{CycleTask, ToggleTask};
use editor::{Editor, EditorEvent};
use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    SharedString, Subscription, Task, UniformListScrollHandle, Window, uniform_list,
};
use quire_core::{
    tasks::{Action, DateField, TaskState, date_phrase, short_date},
    workspace::{ProjectStatus, TaskChoices, TaskView},
};
use quire_store::index::{IndexClient, ProjectSummary, TaskBoard, TaskItem, TaskQuery};
use ui::{
    Button, ButtonStyle, Checkbox, Color, ContextMenu, Disableable, DropdownMenu, Label,
    LabelCommon, LabelSize, ListItem, ListItemSpacing, PopoverMenu, ToggleState, Toggleable,
    prelude::*, v_flex,
};

gpui::actions!(
    quire_tasks_view,
    [
        /// Adds the typed task to Inbox, or to today's page when Settings says so.
        SubmitCapture,
        /// Adds the highlighted task to the selection, or takes it out.
        ToggleTaskSelected,
        /// Selects every task from the last chosen one to the highlighted one.
        SelectTaskRange,
        ClearTaskSelection,
        MoveToInbox,
        MoveToNext,
        MoveToWaiting,
        MoveToSomeday,
        MoveToDone,
        EditDueDate,
        EditCreatedDate,
        EditCompletedDate,
        NextTaskView,
        PreviousTaskView,
        FocusTaskFilter,
        /// Lists the highlighted project's actions.
        ReviewProject,
    ]
);

/// Rows listed before "Show more".
pub const PAGE: usize = 40;
pub const NEW_PROJECT_TEXT: &str = "- #project\n- [ ] Define the next action #next\n";
const PROJECTS_FOLDER: &str = "Projects";
const CAPTURE_PLACEHOLDER: &str = if cfg!(target_os = "macos") {
    "What needs your attention? (⌘⇧T)"
} else {
    "What needs your attention? (Ctrl-Shift-T)"
};
const LIST_KEYS: &str = if cfg!(target_os = "macos") {
    "Space selects · 1–5 move to Inbox, Next, Waiting, Someday, Done · ⌘Enter ticks · D, C, ⇧D set dates · Enter opens · [ and ] switch lists"
} else {
    "Space selects · 1–5 move to Inbox, Next, Waiting, Someday, Done · Ctrl-Enter ticks · D, C, Shift-D set dates · Enter opens · [ and ] switch lists"
};

pub enum TasksEvent {
    Open {
        page: String,
        jump: Option<super::Jump>,
        beside: bool,
    },
    /// The list, a filter, the rows shown, or the capture draft changed and should be remembered.
    Remember(TaskChoices),
    /// A link in a task's text was clicked.
    Link {
        link: crate::rich_text::RowLink,
        beside: bool,
    },
}

/// The empty-list text for each list, from the reference Tasks page.
fn empty_text(view: TaskView) -> &'static str {
    match view {
        TaskView::Due => "Nothing is due. Link a task to a daily page to give it a date.",
        TaskView::Upcoming => "Nothing is due in the next 7 days.",
        TaskView::Inbox => "Your inbox is clear. Capture whatever needs your attention.",
        TaskView::Next => "Choose a small, concrete next action from your inbox.",
        TaskView::Waiting => "Nothing is waiting on someone else.",
        TaskView::Someday => "Keep possibilities here until you choose to act.",
        TaskView::Done => "Completed actions will appear here.",
        TaskView::Projects => {
            "Give an outcome a home. Create a project, or mark a page with #project."
        }
    }
}

pub fn state_label(state: TaskState) -> &'static str {
    match state {
        TaskState::Inbox => "Inbox",
        TaskState::Next => "Next",
        TaskState::Waiting => "Waiting",
        TaskState::Someday => "Someday",
        TaskState::Done => "Done",
    }
}

fn plural(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        count => format!("{count} {many}"),
    }
}

/// What a project's next-action line says, and whether it needs attention.
pub fn next_action(project: &ProjectSummary) -> (String, bool) {
    if project.next > 0 {
        (plural(project.next, "next action", "next actions"), false)
    } else if project.open == 0 && project.done > 0 {
        ("All actions complete".into(), false)
    } else if project.open > 0 {
        ("No next action · Clarify one small step".into(), true)
    } else {
        ("No next action".into(), true)
    }
}

/// A task's identity in a list: its page and block.
fn identity(task: &TaskItem) -> (String, String) {
    (task.source.page.to_lossy_string(), task.key.clone())
}

/// The date being typed for one task.
struct DateEdit {
    task: TaskRef,
    field: DateField,
    editor: Entity<Editor>,
}

pub struct TasksView {
    pub(crate) board: Live<TaskBoard>,
    pub(crate) choices: TaskChoices,
    pub(crate) filter: Entity<Editor>,
    pub(crate) capture: Entity<Editor>,
    capture_project: Option<String>,
    date: Option<DateEdit>,
    pub(crate) new_project: Option<Entity<Editor>>,
    pub(crate) highlighted: usize,
    selected: Vec<(String, String)>,
    anchor: Option<(String, String)>,
    pub(crate) limit: usize,
    pub(crate) notice: Option<SharedString>,
    pub(crate) error: Option<SharedString>,
    work: Option<Task<()>>,
    today: String,
    client: Option<IndexClient>,
    writers: Writers,
    destination: Destination,
    pub(crate) focus: FocusHandle,
    scroll: UniformListScrollHandle,
    rich: crate::rich_text::RichCache,
    _subscriptions: Vec<Subscription>,
}

/// Where the next capture goes, read when it is submitted so a Settings change applies at once.
pub type Destination = std::rc::Rc<dyn Fn(&App) -> Result<CaptureTo, String>>;

impl EventEmitter<TasksEvent> for TasksView {}

impl Focusable for TasksView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn field(placeholder: &str, window: &mut Window, cx: &mut App) -> Entity<Editor> {
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_placeholder_text(placeholder, window, cx);
        editor
    })
}

impl TasksView {
    pub fn new(
        choices: TaskChoices,
        writers: Writers,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = field("Filter actions or pages…", window, cx);
        let capture = field(CAPTURE_PLACEHOLDER, window, cx);
        filter.update(cx, |filter, cx| {
            filter.set_text(choices.query.as_str(), window, cx)
        });
        capture.update(cx, |capture, cx| {
            capture.set_text(choices.draft.as_str(), window, cx)
        });
        let filtered = cx.subscribe(&filter, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.highlighted = 0;
                this.limit = PAGE;
                this.remember(cx);
                this.query(cx);
            }
        });
        let drafted = cx.subscribe(&capture, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.remember(cx);
            }
        });
        let limit = choices.limit.max(PAGE);
        Self {
            board: Live::default(),
            choices,
            filter,
            capture,
            capture_project: None,
            date: None,
            new_project: None,
            highlighted: 0,
            selected: Vec::new(),
            anchor: None,
            limit,
            notice: None,
            error: None,
            work: None,
            today: String::new(),
            client: None,
            writers,
            destination: std::rc::Rc::new(|_| Ok(CaptureTo::Inbox)),
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            rich: Default::default(),
            _subscriptions: vec![filtered, drafted],
        }
    }

    /// Re-reads the board for `today`, after the index changed or the view was shown.
    pub fn refresh(&mut self, client: &IndexClient, today: String, cx: &mut Context<Self>) {
        self.client = Some(client.clone());
        self.today = today;
        self.query(cx);
    }

    fn query(&mut self, cx: &mut Context<Self>) {
        let Some(client) = self.client.clone() else {
            return;
        };
        let query = TaskQuery {
            view: self.choices.view,
            today: self.today.clone(),
            filter: self.filter.read(cx).text(cx),
            project: self.choices.project.clone(),
            limit: self.limit,
        };
        let answer = client.with_generation(move |index| index.task_board(&query));
        self.board.request(
            answer,
            cx,
            |this| &mut this.board,
            |error| format!("Cannot list tasks: {error}. Wait for indexing to finish, then retry."),
        );
    }

    pub fn set_destination(&mut self, destination: Destination) {
        self.destination = destination;
    }

    pub fn focus_capture(&self, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.capture.focus_handle(cx), cx);
    }

    /// The reference status line: whether the last change to a task is still being written.
    pub fn save_status(&self) -> &'static str {
        match (&self.work, &self.error) {
            (Some(_), _) => "Saving locally…",
            (None, Some(_)) => "Not saved; see the message above",
            (None, None) => "Saved locally",
        }
    }

    fn remember(&mut self, cx: &mut Context<Self>) {
        self.choices.query = self.filter.read(cx).text(cx);
        self.choices.draft = self.capture.read(cx).text(cx);
        self.choices.limit = if self.limit > PAGE { self.limit } else { 0 };
        cx.emit(TasksEvent::Remember(self.choices.clone()));
    }

    pub fn choose(&mut self, view: TaskView, cx: &mut Context<Self>) {
        self.choices.view = view;
        self.limit = PAGE;
        self.highlighted = 0;
        self.notice = None;
        self.selected.clear();
        self.remember(cx);
        self.query(cx);
        cx.notify();
    }

    fn step_view(&mut self, delta: isize, cx: &mut Context<Self>) {
        let views = TaskView::ALL;
        let at = views
            .iter()
            .position(|view| *view == self.choices.view)
            .unwrap_or(0) as isize;
        let next = (at + delta).rem_euclid(views.len() as isize) as usize;
        self.choose(views[next], cx);
    }

    pub fn set_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        self.choices.project = project;
        self.limit = PAGE;
        self.highlighted = 0;
        self.remember(cx);
        self.query(cx);
    }

    pub fn set_status(&mut self, status: ProjectStatus, cx: &mut Context<Self>) {
        self.choices.project_status = status;
        self.highlighted = 0;
        self.remember(cx);
        cx.notify();
    }

    pub fn set_capture_project(&mut self, project: Option<String>, cx: &mut Context<Self>) {
        self.capture_project = project;
        cx.notify();
    }

    /// The chosen project filter, when it still names a project.
    fn project(&self) -> Option<&ProjectSummary> {
        let chosen = self.choices.project.as_deref()?;
        self.board
            .value
            .projects
            .iter()
            .find(|project| project.page.page.as_str() == Some(chosen))
    }

    /// Projects matching the status and the typed name.
    pub fn shown_projects(&self, cx: &App) -> Vec<ProjectSummary> {
        let needle = self.filter.read(cx).text(cx).trim().to_lowercase();
        self.board
            .value
            .projects
            .iter()
            .filter(|project| match self.choices.project_status {
                ProjectStatus::All => true,
                ProjectStatus::Active => project.is_active(),
                ProjectStatus::Inactive => !project.is_active(),
            })
            .filter(|project| project.page.title.to_lowercase().contains(&needle))
            .cloned()
            .collect()
    }

    pub fn status_count(&self, status: ProjectStatus) -> usize {
        let projects = &self.board.value.projects;
        match status {
            ProjectStatus::All => projects.len(),
            ProjectStatus::Active => projects.iter().filter(|p| p.is_active()).count(),
            ProjectStatus::Inactive => projects.iter().filter(|p| !p.is_active()).count(),
        }
    }

    pub fn rows(&self) -> &[TaskItem] {
        &self.board.value.rows
    }

    fn row_count(&self, cx: &App) -> usize {
        match self.choices.view {
            TaskView::Projects => self.shown_projects(cx).len(),
            _ => self.rows().len(),
        }
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.row_count(cx);
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

    fn highlighted_task(&self) -> Option<&TaskItem> {
        (self.choices.view != TaskView::Projects)
            .then(|| self.rows().get(self.highlighted))
            .flatten()
    }

    fn open(&mut self, index: usize, beside: bool, cx: &mut Context<Self>) {
        self.highlighted = index;
        let target = match self.choices.view {
            TaskView::Projects => self
                .shown_projects(cx)
                .get(index)
                .map(|project| (project.page.page.to_lossy_string(), None)),
            _ => self.rows().get(index).map(|task| {
                (
                    task.source.page.to_lossy_string(),
                    Some(super::Jump::at(task.id.clone(), task.from)),
                )
            }),
        };
        if let Some((page, jump)) = target {
            cx.emit(TasksEvent::Open { page, jump, beside });
        }
    }

    /// Lists a project's actions: its next actions, or else the list of its first open one.
    pub fn review(
        &mut self,
        project: &ProjectSummary,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let view = match (project.next, project.first_open) {
            (0, Some(TaskState::Inbox) | None) => TaskView::Inbox,
            (0, Some(TaskState::Waiting)) => TaskView::Waiting,
            (0, Some(TaskState::Someday)) => TaskView::Someday,
            _ => TaskView::Next,
        };
        self.filter
            .update(cx, |filter, cx| filter.clear(window, cx));
        self.choices.project = Some(project.page.page.to_lossy_string());
        self.choose(view, cx);
        window.focus(&self.focus, cx);
    }

    fn review_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(project) = self.shown_projects(cx).get(self.highlighted).cloned() {
            self.review(&project, window, cx);
        }
    }

    pub fn is_selected(&self, task: &TaskItem) -> bool {
        self.selected.contains(&identity(task))
    }

    pub fn selected_count(&self) -> usize {
        self.rows()
            .iter()
            .filter(|task| self.is_selected(task))
            .count()
    }

    /// Adds a task to the selection or takes it out; `range` selects every task from the last
    /// chosen one.
    pub fn select(&mut self, index: usize, range: bool, cx: &mut Context<Self>) {
        let Some(task) = self.rows().get(index) else {
            return;
        };
        let key = identity(task);
        let from = self
            .anchor
            .as_ref()
            .and_then(|anchor| self.rows().iter().position(|row| identity(row) == *anchor));
        match from.filter(|_| range) {
            Some(from) => {
                let keys: Vec<_> = self.rows()[from.min(index)..=from.max(index)]
                    .iter()
                    .map(identity)
                    .collect();
                for key in keys {
                    if !self.selected.contains(&key) {
                        self.selected.push(key);
                    }
                }
            }
            None => match self.selected.iter().position(|item| *item == key) {
                Some(at) => {
                    self.selected.remove(at);
                }
                None => self.selected.push(key.clone()),
            },
        }
        self.anchor = Some(key);
        cx.notify();
    }

    fn clear_selection(&mut self, cx: &mut Context<Self>) {
        self.selected.clear();
        cx.notify();
    }

    /// The tasks a command applies to: the selection, else the highlighted task.
    fn targets(&self) -> Vec<TaskRef> {
        let chosen: Vec<TaskRef> = self
            .rows()
            .iter()
            .filter(|task| self.is_selected(task))
            .map(TaskRef::from)
            .collect();
        if chosen.is_empty() {
            self.highlighted_task()
                .map(TaskRef::from)
                .into_iter()
                .collect()
        } else {
            chosen
        }
    }

    fn run(
        &mut self,
        tasks: Vec<TaskRef>,
        change: TaskChange,
        done: impl FnOnce(usize) -> String + 'static,
        cx: &mut Context<Self>,
    ) {
        if tasks.is_empty() {
            return;
        }
        let (open, storage) = (self.writers)(cx);
        let work = task_edit::update(tasks, change, open, storage, cx);
        self.error = None;
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(count) => {
                        this.selected.clear();
                        this.notice = Some(done(count).into());
                        this.query(cx);
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Ticks the task at `index`: completes it, or reopens a done one.
    pub fn toggle(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(task) = self.rows().get(index) else {
            return;
        };
        let reopen = task.state == TaskState::Done;
        let task = TaskRef::from(task);
        self.run(
            vec![task],
            TaskChange::State(Action::Toggle),
            move |_| {
                if reopen {
                    "Reopened task."
                } else {
                    "Completed task."
                }
                .into()
            },
            cx,
        );
    }

    fn cycle(&mut self, cx: &mut Context<Self>) {
        let Some(task) = self.highlighted_task() else {
            return;
        };
        let next = match task.state {
            TaskState::Inbox => TaskState::Next,
            TaskState::Next => TaskState::Waiting,
            TaskState::Waiting => TaskState::Someday,
            TaskState::Someday | TaskState::Done => TaskState::Inbox,
        };
        let task = TaskRef::from(task);
        self.run(
            vec![task],
            TaskChange::State(Action::Set(next)),
            move |_| format!("Moved to {}.", state_label(next)),
            cx,
        );
    }

    /// Moves the task at `index` to `state`, as each row's state menu does (rt's per-row state
    /// select), leaving any other selected tasks where they are.
    pub fn set_state(&mut self, index: usize, state: TaskState, cx: &mut Context<Self>) {
        let Some(task) = self.rows().get(index) else {
            return;
        };
        if task.state == state {
            return;
        }
        let task = TaskRef::from(task);
        self.highlighted = index;
        self.run(
            vec![task],
            TaskChange::State(Action::Set(state)),
            move |_| format!("Moved to {}.", state_label(state)),
            cx,
        );
    }

    /// Moves the selected tasks, or the highlighted one, to `state`.
    pub fn move_to(&mut self, state: TaskState, cx: &mut Context<Self>) {
        let tasks = self.targets();
        self.run(
            tasks,
            TaskChange::State(Action::Set(state)),
            move |count| {
                format!(
                    "Moved {} to {}.",
                    plural(count, "task", "tasks"),
                    state_label(state)
                )
            },
            cx,
        )
    }

    /// Opens the date field for the highlighted task's created, due, or completed date.
    pub fn edit_date(
        &mut self,
        index: usize,
        field: DateField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(task) = self.rows().get(index) else {
            return;
        };
        let current = match field {
            DateField::Created => task.created.clone(),
            DateField::Due => task.due.clone(),
            DateField::Completed => task.completed.clone(),
        };
        let task = TaskRef::from(task);
        let editor = field_editor(current.as_deref(), window, cx);
        window.focus(&editor.focus_handle(cx), cx);
        self.highlighted = index;
        self.date = Some(DateEdit {
            task,
            field,
            editor,
        });
        cx.notify();
    }

    fn apply_date(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(edit) = self.date.take() else {
            return;
        };
        let typed = edit.editor.read(cx).text(cx);
        match date_phrase(&typed, &self.today) {
            Ok(date) => {
                let name = field_name(edit.field);
                let cleared = date.is_none();
                self.run(
                    vec![edit.task],
                    TaskChange::Date(edit.field, date),
                    move |_| {
                        if cleared {
                            format!("Cleared the {name} date.")
                        } else {
                            format!("Set the {name} date.")
                        }
                    },
                    cx,
                );
                window.focus(&self.focus, cx);
            }
            Err(error) => {
                self.error = Some(error.into());
                self.date = Some(edit);
            }
        }
        cx.notify();
    }

    fn cancel_field(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.date = None;
        self.new_project = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Captures the typed task where Settings sends it, then shows the list it went to.
    pub fn submit_capture(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let value = self.capture.read(cx).text(cx);
        let Some(client) = self.client.clone() else {
            return;
        };
        let project = self
            .capture_project
            .as_deref()
            .and_then(|chosen| {
                self.board
                    .value
                    .projects
                    .iter()
                    .find(|project| project.page.page.as_str() == Some(chosen))
            })
            .map(|project| CaptureProject {
                page: project.page.page.to_lossy_string(),
                title: project.page.title.clone(),
                path: project.page.path.clone(),
            });
        let to = match (self.destination)(cx) {
            Ok(to) => to,
            Err(error) => {
                self.error = Some(error.into());
                cx.notify();
                return;
            }
        };
        let (open, storage) = (self.writers)(cx);
        let work = task_edit::capture(value, project, to, open, client, storage, cx);
        self.error = None;
        self.work = Some(cx.spawn_in(window, async move |this, cx| {
            let result = work.await;
            this.update_in(cx, |this, window, cx| {
                this.work = None;
                match result {
                    Ok(captured) => {
                        this.capture
                            .update(cx, |editor, cx| editor.clear(window, cx));
                        this.filter
                            .update(cx, |editor, cx| editor.clear(window, cx));
                        this.choices.project = None;
                        let view = match captured.state {
                            TaskState::Inbox => TaskView::Inbox,
                            TaskState::Next => TaskView::Next,
                            TaskState::Waiting => TaskView::Waiting,
                            TaskState::Someday => TaskView::Someday,
                            TaskState::Done => TaskView::Done,
                        };
                        this.choose(view, cx);
                        this.notice = Some("Task captured.".into());
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn start_new_project(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let editor = field("Project name", window, cx);
        window.focus(&editor.focus_handle(cx), cx);
        self.new_project = Some(editor);
        cx.notify();
    }

    /// Creates a project page in Projects with a first next action, and opens it.
    fn create_project(&mut self, cx: &mut Context<Self>) {
        let (Some(editor), Some(client)) = (self.new_project.clone(), self.client.clone()) else {
            return;
        };
        let name = editor.read(cx).text(cx);
        let (_, storage) = (self.writers)(cx);
        let work = super::create_page_with(
            storage,
            client,
            name,
            PROJECTS_FOLDER.into(),
            NEW_PROJECT_TEXT.into(),
        );
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.work = None;
                match result {
                    Ok(page) => {
                        this.new_project = None;
                        this.error = None;
                        cx.emit(TasksEvent::Open {
                            page,
                            jump: None,
                            beside: false,
                        });
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    #[cfg(test)]
    pub fn idle(&self) -> bool {
        self.work.is_none() && self.board.settled()
    }
}

fn field_name(field: DateField) -> &'static str {
    match field {
        DateField::Created => "created",
        DateField::Due => "due",
        DateField::Completed => "completed",
    }
}

pub(crate) fn field_editor(
    current: Option<&str>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<Editor> {
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_placeholder_text("YYYY-MM-DD, today, tomorrow, +3, or a weekday", window, cx);
        if let Some(current) = current {
            editor.set_text(current, window, cx);
            editor.select_all(&editor::actions::SelectAll, window, cx);
        }
        editor
    })
}

/// The chip text for one of a task's dates, as the reference date inputs label them.
pub fn date_chip(task: &TaskItem, field: DateField, today: &str) -> Option<String> {
    let value = match field {
        DateField::Created => task.created.as_deref(),
        DateField::Due => task.due.as_deref(),
        DateField::Completed => task.completed.as_deref(),
    }?;
    Some(match field {
        DateField::Due if task.is_open() && value < today => {
            format!("Overdue · {}", short_date(value, today))
        }
        DateField::Due if value == today => "Today".into(),
        _ => short_date(value, today),
    })
}

impl TasksView {
    fn render_task_rows(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let rows = self.rows().to_vec();
        let today = self.today.clone();
        uniform_list(
            "task-rows",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let task = rows.get(index)?;
                        Some(this.render_task(index, task, &today, cx))
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
    }

    fn render_task(
        &self,
        index: usize,
        task: &TaskItem,
        today: &str,
        cx: &mut Context<Self>,
    ) -> ListItem {
        let done = task.state == TaskState::Done;
        let fields: &[DateField] = if done {
            &[DateField::Created, DateField::Due, DateField::Completed]
        } else {
            &[DateField::Created, DateField::Due]
        };
        let state = self.state_menu(index, task.state, cx);
        let chips = fields.iter().map(|field| {
            let field = *field;
            let label = date_chip(task, field, today).map_or_else(
                || format!("Set {}", field_name(field)),
                |chip| match field {
                    DateField::Created => format!("Created {chip}"),
                    DateField::Due => chip,
                    DateField::Completed => format!("Done {chip}"),
                },
            );
            let overdue = field == DateField::Due && label.starts_with("Overdue");
            Button::new(
                SharedString::from(format!("task-date-{index}-{}", field_name(field))),
                label,
            )
            .label_size(LabelSize::Small)
            .color(if overdue { Color::Error } else { Color::Muted })
            .on_click(
                cx.listener(move |this, _, window, cx| this.edit_date(index, field, window, cx)),
            )
        });
        ListItem::new(("task", index))
            .spacing(ListItemSpacing::Sparse)
            .toggle_state(index == self.highlighted)
            .start_slot(
                h_flex()
                    .gap_1()
                    .child(
                        Checkbox::new(
                            ("select-task", index),
                            if self.is_selected(task) {
                                ToggleState::Selected
                            } else {
                                ToggleState::Unselected
                            },
                        )
                        .on_click(cx.listener(
                            move |this, _, window, cx| {
                                let shift = window.modifiers().shift;
                                this.select(index, shift, cx)
                            },
                        )),
                    )
                    .child(
                        Checkbox::new(
                            ("tick-task", index),
                            if done {
                                ToggleState::Selected
                            } else {
                                ToggleState::Unselected
                            },
                        )
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle(index, cx))),
                    ),
            )
            .child(
                v_flex()
                    .min_w_0()
                    .child(crate::rich_text::line(
                        ("task-text", index),
                        &self.rich.get(&task.text),
                        done,
                        Some(crate::rich_text::emitter(cx, |link, beside| {
                            TasksEvent::Link { link, beside }
                        })),
                        cx,
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Label::new(task.source.title.clone())
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(state),
                    ),
            )
            .end_slot(h_flex().gap_1().children(chips))
            .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                this.open(index, event.modifiers().shift, cx)
            }))
    }

    /// The row's state, as a menu that moves the task to another state. The menu is built only
    /// when opened, so long lists build none.
    fn state_menu(
        &self,
        index: usize,
        current: TaskState,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let view = cx.entity().downgrade();
        let menu = PopoverMenu::new(("task-state", index))
            .trigger(
                Button::new(("task-state-button", index), state_label(current))
                    .label_size(LabelSize::Small)
                    .color(Color::Accent)
                    .end_icon(Icon::new(IconName::ChevronDown).size(IconSize::XSmall)),
            )
            .menu(move |window, cx| {
                let view = view.clone();
                let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
                    for state in TaskState::ALL {
                        let view = view.clone();
                        menu = menu.toggleable_entry(
                            state_label(state),
                            state == current,
                            IconPosition::Start,
                            None,
                            move |_, cx| {
                                view.update(cx, |this, cx| this.set_state(index, state, cx))
                                    .ok();
                            },
                        );
                    }
                    menu
                });
                menu.update(cx, |menu, cx| menu.select_toggled_or_first(window, cx));
                Some(menu)
            });
        gpui::div()
            .debug_selector(move || format!("quire-task-state-{index}"))
            .child(menu)
    }

    fn render_projects(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let projects = self.shown_projects(cx);
        uniform_list(
            "project-rows",
            projects.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let project = projects.get(index)?.clone();
                        let (line, missing) = next_action(&project);
                        let review = project.clone();
                        Some(
                            ListItem::new(("project", index))
                                .spacing(ListItemSpacing::Sparse)
                                .toggle_state(index == this.highlighted)
                                .child(
                                    v_flex()
                                        .child(Label::new(project.page.title.clone()))
                                        .child(
                                            h_flex()
                                                .gap_2()
                                                .child(
                                                    Label::new(format!(
                                                        "{} open · {} done",
                                                        project.open, project.done
                                                    ))
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                                )
                                                .child(
                                                    Label::new(line).size(LabelSize::Small).color(
                                                        if missing {
                                                            Color::Warning
                                                        } else {
                                                            Color::Success
                                                        },
                                                    ),
                                                ),
                                        ),
                                )
                                .end_slot(
                                    Button::new(("review-project", index), "Review actions →")
                                        .label_size(LabelSize::Small)
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.review(&review, window, cx)
                                        })),
                                )
                                .on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                    this.open(index, event.modifiers().shift, cx)
                                })),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
    }

    fn project_menu(
        &self,
        id: &'static str,
        chosen: Option<&str>,
        none: &'static str,
        set: fn(&mut Self, Option<String>, &mut Context<Self>),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DropdownMenu {
        let projects: Vec<(String, String)> = self
            .board
            .value
            .projects
            .iter()
            .map(|project| {
                (
                    project.page.page.to_lossy_string(),
                    project.page.title.clone(),
                )
            })
            .collect();
        let label = chosen
            .and_then(|chosen| projects.iter().find(|(page, _)| page == chosen))
            .map_or_else(|| none.to_owned(), |(_, title)| title.clone());
        let view = cx.entity().downgrade();
        let menu = ContextMenu::build(window, cx, move |mut menu, _, _| {
            let entries = std::iter::once((None, none.to_owned())).chain(
                projects
                    .into_iter()
                    .map(|(page, title)| (Some(page), title)),
            );
            for (page, title) in entries {
                let view = view.clone();
                menu = menu.entry(title, None, move |_, cx| {
                    let page = page.clone();
                    view.update(cx, |this, cx| set(this, page, cx)).ok();
                });
            }
            menu
        });
        DropdownMenu::new(id, label, menu)
    }

    fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let selected = self.selected_count();
        h_flex()
            .gap_1()
            .child(Label::new(format!("{selected} selected")).size(LabelSize::Small))
            .children(TaskState::ALL.into_iter().map(|state| {
                Button::new(
                    SharedString::from(format!("bulk-{}", state.as_str())),
                    state_label(state),
                )
                .label_size(LabelSize::Small)
                .on_click(cx.listener(move |this, _, _, cx| this.move_to(state, cx)))
            }))
            .child(
                Button::new("bulk-clear", "Clear")
                    .label_size(LabelSize::Small)
                    .on_click(cx.listener(|this, _, _, cx| this.clear_selection(cx))),
            )
    }
}

impl Render for TasksView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let view = self.choices.view;
        let counts = self.board.value.counts.clone();
        let projects_view = view == TaskView::Projects;
        let border = cx.theme().colors().border;
        let boxed = |child: Entity<Editor>| {
            div()
                .flex_1()
                .border_1()
                .border_color(border)
                .p_1()
                .child(child)
        };
        let tabs = h_flex()
            .gap_1()
            .flex_wrap()
            .children(TaskView::ALL.into_iter().map(|item| {
                Button::new(
                    SharedString::from(format!("task-view-{}", item.label())),
                    format!("{} {}", item.label(), counts.of(item)),
                )
                .toggle_state(item == view)
                .on_click(cx.listener(move |this, _, _, cx| this.choose(item, cx)))
            }));
        let capture_menu = self.project_menu(
            "capture-project",
            self.capture_project.as_deref(),
            "No project",
            Self::set_capture_project,
            window,
            cx,
        );
        let capture = h_flex()
            .key_context("QuireTaskField")
            .gap_2()
            .on_action(
                cx.listener(|this, _: &menu::Confirm, window, cx| this.submit_capture(window, cx)),
            )
            .on_action(
                cx.listener(|this, _: &menu::Cancel, window, cx| this.cancel_field(window, cx)),
            )
            .child(boxed(self.capture.clone()))
            .child(capture_menu)
            .child(
                Button::new("capture", "Capture ↵")
                    .disabled(self.work.is_some())
                    .on_click(cx.listener(|this, _, window, cx| this.submit_capture(window, cx))),
            );
        let filters = h_flex()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .key_context("QuireList QuireTaskFilter")
                    .on_action(cx.listener(|this, _: &menu::Cancel, window, cx| {
                        this.cancel_field(window, cx)
                    }))
                    .child(boxed(self.filter.clone())),
            )
            .when(projects_view, |row| {
                row.children(ProjectStatus::ALL.into_iter().map(|status| {
                    Button::new(
                        SharedString::from(format!("status-{}", status.label())),
                        format!("{} ({})", status.label(), self.status_count(status)),
                    )
                    .toggle_state(status == self.choices.project_status)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_status(status, cx)))
                }))
                .child(Button::new("new-project", "New project").on_click(
                    cx.listener(|this, _, window, cx| this.start_new_project(window, cx)),
                ))
            })
            .when(!projects_view, |row| {
                let chosen = self
                    .project()
                    .map(|project| project.page.page.to_lossy_string());
                row.child(self.project_menu(
                    "filter-project",
                    chosen.as_deref(),
                    "All projects",
                    Self::set_project,
                    window,
                    cx,
                ))
            });
        let heading = if projects_view {
            plural(self.shown_projects(cx).len(), "project", "projects")
        } else {
            plural(self.board.value.matching, "action", "actions")
        };
        let rows = self.row_count(cx);
        let empty = (rows == 0).then(|| {
            let filtered = !self.filter.read(cx).text(cx).trim().is_empty();
            match (projects_view, filtered) {
                (true, true) => "No matching projects.".to_owned(),
                (true, false) if !self.board.value.projects.is_empty() => format!(
                    "No {} projects. Choose All to see every project.",
                    self.choices.project_status.label().to_lowercase()
                ),
                (false, true) => "No matching actions. Try another word or project.".to_owned(),
                _ => empty_text(view).to_owned(),
            }
        });
        let more = if projects_view {
            0
        } else {
            self.board.value.matching.saturating_sub(self.limit)
        };
        let list_root = v_flex()
            .id("task-list")
            .key_context("QuireList QuireTaskList")
            .track_focus(&self.focus)
            .flex_1()
            .min_h_0();
        let list = list_keys(
            list_root,
            cx,
            |this, delta, cx| this.move_highlight(delta, cx),
            |this, beside, _, cx| {
                let index = this.highlighted;
                this.open(index, beside, cx)
            },
        )
        .on_action(cx.listener(|this, _: &ToggleTaskSelected, _, cx| {
            let index = this.highlighted;
            this.select(index, false, cx)
        }))
        .on_action(cx.listener(|this, _: &SelectTaskRange, _, cx| {
            let index = this.highlighted;
            this.select(index, true, cx)
        }))
        .on_action(cx.listener(|this, _: &ClearTaskSelection, _, cx| this.clear_selection(cx)))
        .on_action(
            cx.listener(|this, _: &ReviewProject, window, cx| this.review_highlighted(window, cx)),
        )
        .on_action(cx.listener(|this, _: &EditDueDate, window, cx| {
            let index = this.highlighted;
            this.edit_date(index, DateField::Due, window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditCreatedDate, window, cx| {
            let index = this.highlighted;
            this.edit_date(index, DateField::Created, window, cx)
        }))
        .on_action(cx.listener(|this, _: &EditCompletedDate, window, cx| {
            let index = this.highlighted;
            this.edit_date(index, DateField::Completed, window, cx)
        }))
        .when(!projects_view && self.selected_count() > 0, |list| {
            list.child(self.render_toolbar(cx))
        })
        .when_some(empty, |list, empty| {
            list.child(Label::new(empty).color(Color::Muted))
        })
        .map(|list| {
            if projects_view {
                list.child(self.render_projects(cx))
            } else {
                list.child(self.render_task_rows(cx))
            }
        })
        .when(more > 0, |list| {
            list.child(
                Button::new("more-tasks", "Show more actions").on_click(cx.listener(
                    |this, _, _, cx| {
                        this.limit += PAGE;
                        this.remember(cx);
                        this.query(cx);
                    },
                )),
            )
        });
        let date_field = self.date.as_ref().map(|edit| {
            h_flex()
                .key_context("QuireTaskField")
                .gap_2()
                .on_action(
                    cx.listener(|this, _: &menu::Confirm, window, cx| this.apply_date(window, cx)),
                )
                .on_action(
                    cx.listener(|this, _: &menu::Cancel, window, cx| this.cancel_field(window, cx)),
                )
                .child(Label::new(format!(
                    "{} date for “{}”",
                    match edit.field {
                        DateField::Created => "Created",
                        DateField::Due => "Due",
                        DateField::Completed => "Completed",
                    },
                    edit.task.raw
                )))
                .child(boxed(edit.editor.clone()))
                .child(
                    Label::new("Enter sets it · empty clears · Escape cancels")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
        });
        let new_project = self.new_project.clone().map(|editor| {
            h_flex()
                .key_context("QuireTaskField")
                .gap_2()
                .on_action(cx.listener(|this, _: &menu::Confirm, _, cx| this.create_project(cx)))
                .on_action(
                    cx.listener(|this, _: &menu::Cancel, window, cx| this.cancel_field(window, cx)),
                )
                .child(boxed(editor))
                .child(
                    Button::new("create-project", "Create project")
                        .on_click(cx.listener(|this, _, _, cx| this.create_project(cx))),
                )
        });
        v_flex()
            .id("tasks")
            .key_context("QuireTasks")
            .flex_1()
            .min_h_0()
            .p_2()
            .gap_2()
            .on_action(cx.listener(|this, _: &ToggleTask, _, cx| {
                let index = this.highlighted;
                if this.highlighted_task().is_some() {
                    this.toggle(index, cx)
                }
            }))
            .on_action(cx.listener(|this, _: &CycleTask, _, cx| this.cycle(cx)))
            .on_action(
                cx.listener(|this, _: &MoveToInbox, _, cx| this.move_to(TaskState::Inbox, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveToNext, _, cx| this.move_to(TaskState::Next, cx)))
            .on_action(
                cx.listener(|this, _: &MoveToWaiting, _, cx| this.move_to(TaskState::Waiting, cx)),
            )
            .on_action(
                cx.listener(|this, _: &MoveToSomeday, _, cx| this.move_to(TaskState::Someday, cx)),
            )
            .on_action(cx.listener(|this, _: &MoveToDone, _, cx| this.move_to(TaskState::Done, cx)))
            .on_action(cx.listener(|this, _: &NextTaskView, _, cx| this.step_view(1, cx)))
            .on_action(cx.listener(|this, _: &PreviousTaskView, _, cx| this.step_view(-1, cx)))
            .on_action(cx.listener(|this, _: &FocusTaskFilter, window, cx| {
                window.focus(&this.filter.focus_handle(cx), cx)
            }))
            .child(
                h_flex()
                    .gap_2()
                    .child(Label::new("Tasks"))
                    .child(
                        Label::new(format!("{} open actions", counts.open()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("tasks-help", "How tasks work")
                            .style(ButtonStyle::Subtle)
                            .on_click(|_, window, cx| {
                                window.dispatch_action(Box::new(crate::help::OpenHelp), cx)
                            }),
                    ),
            )
            .child(capture)
            .child(tabs)
            .child(filters)
            .when_some(new_project, |root, row| root.child(row))
            .child(
                h_flex().gap_2().child(Label::new(view.label())).child(
                    Label::new(heading)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
            .when_some(date_field, |root, row| root.child(row))
            .when_some(self.notice.clone(), |root, notice| {
                root.child(
                    Label::new(notice)
                        .size(LabelSize::Small)
                        .color(Color::Success),
                )
            })
            .when_some(
                self.error.clone().or(self.board.error.clone()),
                |root, error| root.child(Label::new(error).color(Color::Error)),
            )
            .child(list)
            .child(
                Label::new(if projects_view {
                    "Enter opens a project · R reviews its actions · [ and ] switch lists"
                } else {
                    LIST_KEYS
                })
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(format!("{} · Markdown tasks", self.save_status()))
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(div().flex_1())
                    .child(
                        Label::new("Capture → Clarify → Do → Review")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            )
    }
}
