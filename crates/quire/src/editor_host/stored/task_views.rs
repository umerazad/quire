//! Tasks inside the stored window: the Tasks browser, capture from anywhere with Cmd-Shift-T,
//! and the notebook's clock, time zone, daily layout, and page titles for task commands.

use super::*;
use crate::navigation::tasks::{TasksEvent, TasksView};
use crate::tasks::TaskContext;
use quire_core::{
    markdown::page_title_for_editor,
    workspace::{BrowserView, Destination},
};

#[derive(Default)]
pub(in crate::editor_host::stored) struct TaskViews {
    pub(in crate::editor_host::stored) list: Option<Entity<TasksView>>,
}

impl StoredWindow {
    /// Gives task commands this notebook's clock, time zone, daily layout, and page titles, so
    /// a task made on today's daily page needs no created stamp and stamps link daily pages by
    /// the configured title.
    pub(in crate::editor_host::stored) fn install_task_context(&self, cx: &mut App) {
        let session = Rc::downgrade(&self.session);
        cx.set_global(TaskContext {
            now: self.nav.daily.now.clone(),
            calendar: self.nav.daily.calendar.clone(),
            layout: self.nav.daily.layout.clone(),
            title: Rc::new(move |buffer, cx| {
                let session = session.upgrade()?;
                let pages = session.try_borrow().ok()?.open_pages();
                pages.iter().find_map(|page| {
                    let loaded = page.read(cx).loaded()?;
                    (loaded.analysis.read(cx).source == *buffer).then(|| {
                        let path = loaded.path.clone().unwrap_or_default();
                        let text = buffer.read(cx).text();
                        page_title_for_editor(&path.as_str().into(), &text.as_str().into())
                            .to_lossy_string()
                    })
                })
            }),
        });
    }

    fn tasks_view(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Entity<TasksView> {
        if let Some(list) = self.nav.tasks.list.clone() {
            return list;
        }
        let session = self.session.clone();
        let writers: crate::navigation::tag_edits::Writers = Rc::new(move |_| {
            let session = session.borrow();
            (session.open_pages(), session.storage())
        });
        let choices = self.nav.workspace.tasks.clone();
        let list = cx.new(|cx| TasksView::new(choices, writers, window, cx));
        cx.subscribe_in(
            &list,
            window,
            |this, _, event: &TasksEvent, window, cx| match event {
                TasksEvent::Open { page, beside } => {
                    this.open_page(page.clone(), *beside, window, cx)
                }
                TasksEvent::Remember(choices) => {
                    this.nav.workspace.tasks = choices.clone();
                    this.persist(cx);
                }
            },
        )
        .detach();
        self.nav.tasks.list = Some(list.clone());
        list
    }

    pub(in crate::editor_host::stored) fn show_tasks(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let list = self.tasks_view(window, cx);
        self.refresh_tasks(cx);
        window.focus(&list.focus_handle(cx), cx);
    }

    /// Cmd-Shift-T: shows Tasks and puts the caret in the capture field.
    pub(in crate::editor_host::stored) fn capture_task(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        self.visit(
            Destination::Browser {
                view: BrowserView::Tasks,
            },
            window,
            cx,
        );
        if let Some(list) = self.nav.tasks.list.clone() {
            list.update(cx, |list, cx| list.focus_capture(window, cx));
        }
    }

    /// Re-runs the Tasks query while Tasks is shown.
    pub(in crate::editor_host::stored) fn refresh_tasks(&mut self, cx: &mut gpui::Context<Self>) {
        if self.nav.view != Some(BrowserView::Tasks) {
            return;
        }
        let (Some(client), Some(list)) = (self.index_client(cx), self.nav.tasks.list.clone())
        else {
            return;
        };
        let today = self.today().unwrap_or_default();
        list.update(cx, |list, cx| list.refresh(&client, today, cx));
    }

    pub(in crate::editor_host::stored) fn task_content(&self) -> Option<gpui::AnyElement> {
        (self.nav.view == Some(BrowserView::Tasks))
            .then(|| self.nav.tasks.list.clone())
            .flatten()
            .map(|list| list.into_any_element())
    }
}
