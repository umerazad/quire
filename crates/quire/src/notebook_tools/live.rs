//! The notebook the tools answer for: whichever window registered last, read live through its
//! open pages, storage, and query index, never through a second view of the files.

use crate::{notebook_backend::NotebookBackend, notebook_session::OpenPage};
use gpui::{App, Entity, Global};
use quire_core::daily::DailyCalendar;
use quire_store::index::IndexClient;
use std::rc::Rc;

/// One open notebook as the tools see it.
#[derive(Clone)]
pub struct LiveNotebook {
    pub open_pages: Vec<Entity<OpenPage>>,
    pub storage: NotebookBackend,
    pub index: IndexClient,
    /// Today's date, or why it cannot be read.
    pub today: Result<String, String>,
    pub calendar: Option<DailyCalendar>,
}

impl LiveNotebook {
    /// The local date of a time in milliseconds, as the reference `dayOf` reads modification
    /// times; None for an unknown time.
    pub fn day_of(&self, millis: i64) -> Option<String> {
        if millis == 0 {
            return None;
        }
        self.calendar.as_ref()?.date_at(millis).ok()
    }
}

type Provide = Rc<dyn Fn(&App) -> Option<LiveNotebook>>;

#[derive(Default)]
struct Notebooks(Option<Provide>);

impl Global for Notebooks {}

/// Makes `provide` the notebook the tools answer for.
pub fn register(provide: Provide, cx: &mut App) {
    cx.set_global(Notebooks(Some(provide)));
}

/// The notebook the tools answer for, when one is open and its index has started.
pub fn current(cx: &App) -> Option<LiveNotebook> {
    cx.try_global::<Notebooks>()?.0.as_ref()?(cx)
}
