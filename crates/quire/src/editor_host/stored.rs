use super::*;
use crate::editor_command::{SaveAndClosePage, SavePage};
use crate::notebook_session::{NotebookSession, OpenPage};
use crate::query_index::IndexSync;
use quire_core::daily::DailyCalendar;
use std::{
    cell::{Cell, RefCell},
    collections::HashMap,
    path::PathBuf,
    rc::Rc,
};
use theme::ActiveTheme;

mod backups;
mod bench;
mod block_move_views;
#[cfg(feature = "test-support")]
mod capture;
mod document_export;
mod example;
mod exchange;
mod folder;
mod help_pane;
mod instance;
mod links;
mod live;
mod navigation;
mod page_views;
mod profiles;
mod sync;

#[cfg(feature = "test-support")]
pub(crate) use capture::run as capture_shell;
pub(super) use example::launch as launch_example_notebook;
pub(super) use folder::FolderOptions;
pub(super) use instance::{Handover, handle_launch};
pub(crate) use profiles::{dispatch_to_notebook, notebook_windows, open_in_profile};
pub(super) use profiles::{launch_location, open_chosen_folder, open_location};

pub(super) fn bind_keys(cx: &mut App) {
    profiles::init(cx);
    cx.bind_keys([KeyBinding::new(
        if cfg!(target_os = "macos") {
            "cmd-s"
        } else {
            "ctrl-s"
        },
        SavePage,
        Some(EDITOR_CONTEXT),
    )]);
}

#[derive(Clone, Copy)]
enum CloseIntent {
    Window,
    SaveWindow,
    Quit,
}

/// What a close of this window waits for: its pages, its backup and Git sync, and the
/// services other parts of Quire register.
#[derive(Clone)]
struct CloseState {
    session: Rc<RefCell<NotebookSession>>,
    /// The window's backup and Git sync, and anything else a close must wait for.
    services: Rc<RefCell<Vec<Rc<crate::lifecycle::Service>>>>,
    closing: Rc<Cell<Option<CloseIntent>>>,
    /// When the current close request started waiting, on the executor's clock.
    since: Rc<Cell<Option<std::time::Instant>>>,
    window: gpui::AnyWindowHandle,
}

impl CloseState {
    fn activity(&self, cx: &App) -> crate::lifecycle::Activity {
        use crate::lifecycle::Activity;
        let session = self.session.borrow();
        let pages = if session.is_secured(cx) {
            Activity::Idle
        } else if session.has_error(cx) {
            Activity::Failed(match session.unrecorded_conflict(cx) {
                Some(path) => crate::lifecycle::unrecorded_conflict(&path).into(),
                None => "Some pages could not be recorded for recovery because storage reported an error.".into(),
            })
        } else {
            Activity::Busy("Pages are still being recorded for recovery.".into())
        };
        let services: Vec<_> = self
            .services
            .borrow()
            .iter()
            .map(|service| service(cx))
            .collect();
        Activity::all(std::iter::once(pages).chain(services))
    }

    fn busy_while(
        running: impl Fn(&App) -> bool + 'static,
        label: &'static str,
    ) -> Rc<crate::lifecycle::Service> {
        Rc::new(move |cx| {
            if running(cx) {
                crate::lifecycle::Activity::Busy(label.into())
            } else {
                crate::lifecycle::Activity::Idle
            }
        })
    }

    /// The close guard: lets an idle window close; otherwise records the request, starts
    /// securing the pages, and leaves the window to finish the close once the work is done.
    fn allow(&self, intent: CloseIntent, cx: &App) -> bool {
        if crate::lifecycle::quit_forced(cx)
            || self.activity(cx) == crate::lifecycle::Activity::Idle
        {
            return true;
        }
        self.closing.set(Some(intent));
        if self.since.get().is_none() {
            self.since.set(Some(cx.background_executor().now()));
        }
        let (session, window) = (self.session.clone(), self.window);
        cx.spawn(async move |cx| {
            cx.update(|cx| {
                session.borrow().recover(cx);
                window
                    .update(cx, |view, window, cx| {
                        if let Ok(view) = view.downcast::<StoredWindow>() {
                            view.update(cx, |this, cx| this.finish_close(window, cx));
                        }
                    })
                    .ok();
            });
        })
        .detach();
        false
    }
}

#[derive(Clone)]
pub(super) struct OpenOptions {
    database: PathBuf,
    notebook: String,
    writer: String,
    page: String,
    /// The synced folder saves publish to and other computers' changes come from.
    folder: Option<PathBuf>,
    /// Starts on today's daily page once the notebook is ready, as a launch does.
    today: bool,
}

impl OpenOptions {
    pub fn parse(arguments: Vec<String>) -> Result<Option<Self>, String> {
        const FLAGS: [&str; 5] = [
            "--notebook-db",
            "--notebook-id",
            "--writer-id",
            "--page-id",
            "--synced-folder",
        ];
        if !arguments
            .iter()
            .any(|argument| FLAGS.contains(&argument.split('=').next().unwrap_or_default()))
        {
            return Ok(None);
        }
        let mut values = HashMap::new();
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            if !FLAGS.contains(&flag.as_str()) {
                return Err(format!(
                    "Unsupported stored-page argument {flag:?}. Pass --notebook-db, --notebook-id, --writer-id, and --page-id with their values, and --synced-folder for a Quire notebook's synced folder; open editor harnesses separately."
                ));
            }
            let value = arguments
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| {
                    format!("Missing value for {flag}. Pass a nonempty value after the flag.")
                })?;
            if values.insert(flag.clone(), value).is_some() {
                return Err(format!(
                    "Repeated stored-page argument {flag}. Pass each option once."
                ));
            }
        }
        let folder = values
            .remove("--synced-folder")
            .map(|folder| std::path::absolute(&folder).unwrap_or_else(|_| folder.into()));
        let mut take = |flag| {
            values.remove(flag).ok_or_else(|| format!("Missing stored-page argument {flag}. Pass --notebook-db, --notebook-id, --writer-id, and --page-id with the original database identities."))
        };
        Ok(Some(Self {
            database: take("--notebook-db")?.into(),
            notebook: take("--notebook-id")?,
            writer: take("--writer-id")?,
            page: take("--page-id")?,
            folder,
            today: false,
        }))
    }

    pub fn open(self, cx: &mut App) -> Result<(), String> {
        let database = self.database.clone();
        let notebook = self.notebook.clone();
        let today = self.today;
        crate::profiles::check_open(
            &quire_store::profiles::Notebook::Store {
                database: database.clone(),
                notebook: notebook.clone(),
                writer: self.writer.clone(),
                folder: self.folder.clone(),
            },
            cx,
        )?;
        let mut session =
            NotebookSession::open_synced(self.database, self.notebook, self.writer, self.folder)?;
        let page = session.open_page(self.page, cx);
        let options = crate::title_bar::options(crate::title_bar::NOTEBOOK_SIZE, cx);
        cx.open_window(options, move |window, cx| {
            window.set_window_title("Quire Native");
            cx.new(|cx| {
                let mut this = StoredWindow::new(session, page, database, notebook, window, cx);
                this.nav.daily.open_on_launch = today;
                this
            })
        }).map_err(|error| format!("Cannot open Quire's window: {error}. Check desktop and graphics access, then retry."))?;
        cx.activate(true);
        Ok(())
    }
}

struct StoredWindow {
    session: Rc<RefCell<NotebookSession>>,
    /// Where this notebook lives, which names its profile.
    location: Option<quire_store::profiles::Notebook>,
    /// Formats times for the profile line.
    clock: Rc<dyn Fn(i64) -> String>,
    closing: Rc<Cell<Option<CloseIntent>>>,
    close: CloseState,
    /// A close question is on screen.
    asking: bool,
    /// Looks again while a close waits on busy work, so it can ask after [`crate::lifecycle::PATIENCE`].
    close_poll: Option<gpui::Task<()>>,
    close_error: Option<gpui::SharedString>,
    _quit_guard: gpui::Subscription,
    page: Entity<OpenPage>,
    editor: Option<Entity<NotebookWindow>>,
    copy_task: Option<gpui::Task<()>>,
    copy_error: Option<gpui::SharedString>,
    index: Result<Entity<IndexSync>, gpui::SharedString>,
    nav: navigation::Navigation,
    links: links::Links,
    folder: folder::FolderView,
    exchange: Entity<crate::exchange::Exchange>,
    backups: Entity<crate::backups::Backups>,
    sync: Entity<crate::sync_status::SyncStatus>,
    /// Help beside the pages, while it is open.
    help: Option<Entity<crate::help::view::HelpWindow>>,
    /// The page details panel, page figures, and README.md beside the pages.
    chrome: page_views::PageChrome,
    /// Publishing and importing for a Quire notebook with a synced folder.
    live: Option<Entity<crate::live_sync::LiveSync>>,
    live_view: Option<Entity<crate::live_sync::LiveSyncView>>,
}

impl StoredWindow {
    fn new(
        session: NotebookSession,
        page: Entity<OpenPage>,
        database: std::path::PathBuf,
        notebook: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        let session = Rc::new(RefCell::new(session));
        let location = session.borrow().location();
        let profile_error = location
            .clone()
            .and_then(|location| crate::profiles::adopt(location, cx).err());
        session.borrow().enable_editing(cx);
        cx.observe_global::<crate::outline::Notice>(|_, cx| cx.notify())
            .detach();
        let calendar = DailyCalendar::system()
            .map_err(|error| gpui::SharedString::from(format!("Cannot read the system time zone for daily pages: {error}. Set a valid time zone and reopen the notebook.")));
        let index = match &calendar {
            Ok(_) => Err("Reading notebook settings…".into()),
            Err(error) => Err(error.clone()),
        };
        let backups = backups::new_backups(
            &session,
            &database,
            location.clone(),
            calendar.clone().ok(),
            cx,
        );
        let sync = sync::new_sync_status(&session, location.clone(), calendar.clone().ok(), cx);
        let (live, live_view) =
            live::new_live_sync(&session, location.as_ref(), calendar.clone().ok(), cx).unzip();
        let close = CloseState {
            session: session.clone(),
            services: Rc::new(RefCell::new(
                vec![
                    CloseState::busy_while(
                        {
                            let backups = backups.clone();
                            move |cx| backups.read(cx).is_running()
                        },
                        "A backup is running.",
                    ),
                    CloseState::busy_while(
                        {
                            let sync = sync.clone();
                            move |cx| sync.read(cx).is_running()
                        },
                        "Git sync is running.",
                    ),
                ]
                .into_iter()
                .chain(live.clone().map(|live| -> Rc<crate::lifecycle::Service> {
                    Rc::new(move |cx| live.read(cx).activity())
                }))
                .collect(),
            )),
            closing: Rc::new(Cell::new(None)),
            since: Rc::new(Cell::new(None)),
            window: window.window_handle(),
        };
        let quit_guard = cx.on_request_quit({
            let close = close.clone();
            move |cx| close.allow(CloseIntent::Quit, cx)
        });
        window.on_window_should_close(cx, {
            let close = close.clone();
            move |_, cx| close.allow(CloseIntent::Window, cx)
        });
        let mut nav =
            navigation::Navigation::new(&database, &notebook, calendar.ok(), cx.focus_handle());
        nav.error = profile_error.map(Into::into);
        let id = page.read(cx).id().to_owned();
        let links = links::Links::new(
            cx.weak_entity(),
            Rc::downgrade(&session),
            crate::outline::fold_store::FoldStore::beside(&database, &notebook),
            cx,
        );
        let mut this = Self {
            session,
            location,
            clock: backups::clock_label(
                DailyCalendar::system().unwrap_or_else(|_| DailyCalendar::utc()),
            ),
            closing: close.closing.clone(),
            close,
            asking: false,
            close_poll: None,
            close_error: None,
            _quit_guard: quit_guard,
            page,
            editor: None,
            copy_task: None,
            copy_error: None,
            index,
            nav,
            links,
            folder: folder::FolderView::default(),
            exchange: cx.new(|_| crate::exchange::Exchange::default()),
            backups,
            sync,
            help: None,
            chrome: page_views::PageChrome::default(),
            live,
            live_view,
        };
        let pages = this.session.borrow().open_pages();
        for observed in pages {
            this.watch(&observed, window, cx);
        }
        this.visit(quire_core::workspace::Destination::page(id), window, cx);
        if let Some(side) = this.nav.workspace.side.clone() {
            this.place_beside(side, false, window, cx);
        }
        this.watch_backups(window, cx);
        this.watch_sync(window, cx);
        this.watch_live_sync(window, cx);
        this.load_settings(window, cx);
        this.offer_notebook_tools(cx);
        bench::start(window, cx);
        this
    }

    /// Why the index cannot answer, shown as a line above the page until it is fixed.
    fn index_problem(&self, cx: &gpui::App) -> Option<gpui::SharedString> {
        match &self.index {
            Ok(index) => index.read(cx).error().cloned(),
            Err(error) => Some(error.clone()),
        }
    }

    /// Indexing progress. Every save is re-analyzed, so this comes and goes while the user
    /// works; it floats over the window's corner rather than taking a line, which would move the
    /// page under a pointer that is pressing a link.
    fn index_progress(&self, cx: &gpui::App) -> Option<gpui::SharedString> {
        let index = self.index.as_ref().ok()?.read(cx);
        index.error().is_none().then(|| index.status()).flatten()
    }

    /// The editor for a loaded page, built once per page and kept while the window lives.
    fn editor_for(
        &mut self,
        page: &Entity<OpenPage>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Option<Entity<NotebookWindow>> {
        if let Some(editor) = self.cached_editor(page) {
            return Some(editor);
        }
        let loaded = page.read(cx).loaded()?;
        let analysis = loaded.analysis.clone();
        eprintln!(
            "Quire page buffer ready: {} checkout revision bodies, {} source bytes, {} heads, recovery={}, deleted={}, {:.1} ms to buffer ready.",
            loaded.loaded_revision_bodies,
            loaded.source.original_len(),
            loaded.heads.len(),
            loaded.recovered,
            loaded.deleted,
            loaded.load_time.as_secs_f64() * 1000.
        );
        let writes_enabled = loaded.persistence.is_some();
        let previous = window.focused(cx);
        let editor =
            cx.new(|cx| NotebookWindow::for_notebook_page(analysis, writes_enabled, window, cx));
        self.link_editors(&editor, window, cx);
        self.track_editor_focus(&editor, window, cx);
        if let Some(previous) = previous {
            window.focus(&previous, cx);
        }
        self.attach_page_info(page, cx);
        Self::invite_writing(&editor, window, cx);
        self.cache_editor(page, editor.clone());
        self.keep_reading_position(&editor, page, false, window, cx);
        Some(editor)
    }

    fn attach_editor(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        self.attach_side(window, cx);
        if self.editor.is_some() {
            return;
        }
        let page = self.page.clone();
        self.editor = self.editor_for(&page, window, cx);
        if self.editor.is_some() {
            self.update_title(window, cx);
            if window
                .focused(cx)
                .is_none_or(|focused| focused == self.nav.loading_focus)
            {
                self.focus_active(window, cx);
            }
            self.apply_jump(window, cx);
        }
    }

    fn save_page(&mut self, cx: &mut gpui::Context<Self>) {
        let persistence = self.page.read(cx).persistence().cloned();
        if let Some(page) = persistence {
            page.update(cx, |page, cx| page.save(cx));
        }
        self.close_error = None;
    }

    fn save_and_close(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.page.read(cx).persistence().is_none() {
            return;
        }
        self.closing.set(Some(CloseIntent::SaveWindow));
        self.save_page(cx);
        self.session.borrow().recover(cx);
        self.finish_close(window, cx);
    }

    fn can_finish_close(&self, cx: &App) -> bool {
        self.close.activity(cx) == crate::lifecycle::Activity::Idle
            && (!matches!(self.closing.get(), Some(CloseIntent::SaveWindow))
                || self
                    .page
                    .read(cx)
                    .persistence()
                    .is_some_and(|page| !page.read(cx).is_dirty(cx)))
    }

    fn end_close_request(&mut self) {
        self.closing.set(None);
        self.close.since.set(None);
        self.close_poll = None;
    }

    fn finish_close(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        use crate::lifecycle::{Activity, PATIENCE};
        if self.closing.get().is_some() && !self.asking {
            self.prepare_live_close(cx);
        }
        let write_failed = matches!(self.closing.get(), Some(CloseIntent::SaveWindow))
            && self
                .page
                .read(cx)
                .persistence()
                .is_some_and(|page| page.read(cx).has_error());
        if self.can_finish_close(cx) {
            self.close_error = None;
            if self.closing.get().is_some() {
                cx.defer_in(window, |this, window, cx| {
                    if this.can_finish_close(cx) {
                        let intent = this.closing.get();
                        this.end_close_request();
                        match intent {
                            Some(CloseIntent::Quit) => cx.quit(),
                            Some(CloseIntent::Window | CloseIntent::SaveWindow) => {
                                window.remove_window()
                            }
                            None => {}
                        }
                    } else {
                        this.session.borrow().recover(cx);
                    }
                });
            }
            return;
        }
        if self.closing.get().is_none() || self.asking {
            return;
        }
        if write_failed {
            self.end_close_request();
            self.close_error = Some("The page could not be saved locally. Keep this window open, resolve the save error, and retry :wq. Copy Markdown remains available.".into());
            return;
        }
        match self.close.activity(cx) {
            Activity::Failed(reason) => self.ask_about_close(true, reason, window, cx),
            activity => {
                self.poll_close(window, cx);
                let now = cx.background_executor().now();
                let waited = self
                    .close
                    .since
                    .get()
                    .is_some_and(|since| now.saturating_duration_since(since) >= PATIENCE);
                if waited {
                    let reason = match activity {
                        Activity::Busy(reason) => reason,
                        _ => "The page is still being saved locally.".into(),
                    };
                    self.ask_about_close(false, reason, window, cx);
                }
            }
        }
    }

    /// Looks again every [`crate::lifecycle::POLL`] while a close waits on busy work.
    fn poll_close(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.close_poll.is_some() {
            return;
        }
        if self.close.since.get().is_none() {
            self.close.since.set(Some(cx.background_executor().now()));
        }
        self.close_poll = Some(cx.spawn_in(window, async move |this, cx| {
            loop {
                cx.background_executor().timer(crate::lifecycle::POLL).await;
                let waiting = this
                    .update_in(cx, |this, window, cx| {
                        this.finish_close(window, cx);
                        this.closing.get().is_some()
                    })
                    .unwrap_or(false);
                if !waiting {
                    break;
                }
            }
        }));
    }

    /// Asks whether to keep waiting (or editing, after a failure) or to close anyway, naming
    /// what is unfinished.
    fn ask_about_close(
        &mut self,
        failed: bool,
        reason: gpui::SharedString,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        use crate::lifecycle::{
            CLOSE_ANYWAY, CLOSE_ANYWAY_DETAIL, FAILED, KEEP_EDITING, KEEP_WAITING, STILL_WORKING,
        };
        self.asking = true;
        let (message, stay) = if failed {
            (FAILED, KEEP_EDITING)
        } else {
            (STILL_WORKING, KEEP_WAITING)
        };
        let detail = format!("{reason} {CLOSE_ANYWAY_DETAIL}");
        let answer = window.prompt(
            gpui::PromptLevel::Warning,
            message,
            Some(&detail),
            &[stay, CLOSE_ANYWAY],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let close = answer.await == Ok(1);
            this.update_in(cx, |this, window, cx| {
                this.asking = false;
                let intent = this.closing.get();
                if close {
                    this.end_close_request();
                    match intent {
                        Some(CloseIntent::Quit) => crate::lifecycle::force_quit(cx),
                        Some(_) => window.remove_window(),
                        None => {}
                    }
                } else if failed {
                    this.end_close_request();
                    this.close_error = Some(format!("{reason} Keep this window open, check the storage error, and retry closing after it is corrected. Copy Markdown remains available.").into());
                } else {
                    this.close.since.set(Some(cx.background_executor().now()));
                    this.finish_close(window, cx);
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn copy_markdown(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(page) = self.page.read(cx).loaded() else {
            return;
        };
        let source = page.current_source(cx);
        let snapshot = page.analysis.read(cx).source.read(cx).snapshot();
        let work = cx
            .background_spawn(async move { source.render(&snapshot).map(|text| text.to_string()) });
        self.copy_error = None;
        self.copy_task = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(text) => cx.write_to_clipboard(gpui::ClipboardItem::new_string(text)),
                    Err(error) => this.copy_error = Some(error.into()),
                }
                this.copy_task = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for StoredWindow {
    fn render(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let ui_font = crate::app_menus::follow_zoom(window, cx);
        let conflict = self
            .render_folder_conflict(cx)
            .or_else(|| self.render_version_conflict(cx));
        let loading = self.page.read(cx).is_loading();
        let header = self.render_workspace_header(window, cx);
        let profile = self.render_profile(cx);
        let root = gpui::div()
            .track_focus(&self.nav.loading_focus)
            .font(ui_font)
            .size_full()
            .relative()
            .on_action(
                cx.listener(|this, _: &crate::app_menus::CloseWindow, window, cx| {
                    this.close_window(window, cx)
                }),
            )
            .on_action(cx.listener(|this, _: &SavePage, window, cx| this.save_focused(window, cx)))
            .on_action(cx.listener(|this, _: &SaveAndClosePage, window, cx| {
                this.save_and_close(window, cx)
            }));
        Self::register_page_views(
            Self::register_help(
                Self::register_sync(
                    Self::register_backups(
                        Self::register_document_export(
                            Self::register_exchange(
                                Self::register_block_move(Self::register_navigation(root, cx), cx),
                                cx,
                            ),
                            cx,
                        ),
                        cx,
                    ),
                    cx,
                ),
                cx,
            ),
            cx,
        )
        .flex()
        .bg(cx.theme().colors().editor_background)
        .when(!self.nav.workspace.chrome.sidebar_hidden, |div| {
            div.child(
                gpui::div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .h_full()
                    .bg(cx.theme().colors().panel_background)
                    .children(crate::title_bar::lights_strip(window, cx))
                    .child(
                        gpui::div()
                            .flex_1()
                            .min_h_0()
                            .child(self.render_sidebar(cx)),
                    ),
            )
        })
        .child(
            gpui::div()
                .flex()
                .flex_col()
                .flex_1()
                .min_w_0()
                .min_h_0()
                .child(header)
                .child(self.render_tabs(cx))
                .child(profile)
                .child(self.render_exchange_buttons(cx))
                .child(self.exchange.clone())
                .child(self.backups.clone())
                .child(self.sync.clone())
                .when_some(self.live_view.clone(), |div, live| div.child(live))
                .when_some(conflict, |div, conflict| div.child(conflict))
                .when_some(self.close_error.clone(), |div, error| {
                    div.child(gpui::div().p_2().child(error))
                })
                .when_some(self.copy_error.clone(), |div, error| {
                    div.child(gpui::div().p_2().child(error))
                })
                .when_some(self.index_problem(cx), |div, error| {
                    div.child(gpui::div().p_2().child(error))
                })
                .when_some(self.nav.error.clone(), |div, error| {
                    div.child(gpui::div().p_2().child(error))
                })
                .when_some(crate::outline::notice(cx), |div, notice| {
                    div.child(gpui::div().p_2().child(notice))
                })
                .when(loading, |div| {
                    div.child(gpui::div().p_2().child("Opening notebook storage…"))
                })
                .child(self.render_body(window, cx)),
        )
        .when_some(self.index_progress(cx), |div, progress| {
            div.child(
                gpui::div()
                    .debug_selector(|| "quire-index-progress".into())
                    .absolute()
                    .bottom_2()
                    .right_4()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .bg(cx.theme().colors().elevated_surface_background)
                    .child(
                        ui::Label::new(progress)
                            .size(ui::LabelSize::Small)
                            .color(ui::Color::Muted),
                    ),
            )
        })
        .when_some(self.nav.palette.clone(), |div, palette| {
            div.child(
                gpui::div()
                    .absolute()
                    .top_16()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .occlude()
                    .child(palette),
            )
        })
        .when_some(self.nav.template_prompt.clone(), |div, prompt| {
            div.child(
                gpui::div()
                    .absolute()
                    .top_16()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .occlude()
                    .child(prompt),
            )
        })
        .when_some(self.nav.dialog.clone(), |div, dialog| {
            div.child(
                gpui::div()
                    .absolute()
                    .top_16()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .occlude()
                    .child(dialog),
            )
        })
        .when_some(self.nav.page_history.clone(), |div, history| {
            div.child(
                gpui::div()
                    .absolute()
                    .top_16()
                    .left_0()
                    .right_0()
                    .flex()
                    .justify_center()
                    .occlude()
                    .child(history),
            )
        })
        .children(self.render_menu())
        .children(bench::marker(cx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StoredFixture {
        _root: tempfile::TempDir,
        path: PathBuf,
        window: gpui::WindowHandle<StoredWindow>,
        page: Entity<OpenPage>,
    }

    fn open_stored(text: &str, cx: &mut gpui::TestAppContext) -> StoredFixture {
        use quire_store::notebook::{Edit, NotebookStore};
        cx.update(|cx| init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.sqlite");
        let store = NotebookStore::open(&path, "nb", "local").unwrap();
        store
            .save(&Edit {
                page: "p".into(),
                base: vec![],
                path: "Note.md".into(),
                text: text.into(),
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
        drop(store);
        let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
        let page = cx.update(|cx| session.open_page("p".into(), cx));
        let window = cx.add_window(|window, cx| {
            StoredWindow::new(session, page.clone(), path.clone(), "nb".into(), window, cx)
        });
        crate::notebook_session::tests::wait(&page, cx);
        cx.executor().run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        StoredFixture {
            _root: root,
            path,
            window,
            page,
        }
    }

    fn persistence(
        fixture: &StoredFixture,
        cx: &mut gpui::TestAppContext,
    ) -> Entity<crate::page_persistence::PagePersistence> {
        cx.update(|cx| fixture.page.read(cx).persistence().unwrap().clone())
    }

    #[gpui::test]
    fn vim_write_saves_the_shared_page_and_write_quit_waits_for_a_local_commit(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = open_stored("hello\r\n", cx);
        let model = persistence(&fixture, cx);
        cx.simulate_keystrokes(fixture.window.into(), "i x escape : w r i t e enter");
        crate::page_persistence::tests::wait(&model, cx);
        fixture
            .window
            .update(cx, |view, window, cx| {
                assert!(
                    !view
                        .editor
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .commands
                        .read(cx)
                        .is_open()
                );
                assert!(!model.read(cx).is_dirty(cx));
                window.focus(
                    &view
                        .editor
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .source
                        .focus_handle(cx),
                    cx,
                );
            })
            .unwrap();
        cx.simulate_keystrokes(fixture.window.into(), "0 i y escape : w q enter");
        crate::page_persistence::tests::wait(&model, cx);
        while cx.executor().tick() {}
        assert!(fixture.window.update(cx, |_, _, _| {}).is_err());
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        assert!(store.draft("p").unwrap().is_none());
        assert_eq!(
            store
                .checkout("p", None)
                .unwrap()
                .selected
                .unwrap()
                .root()
                .field("text")
                .as_str(),
            Some("yxhello\r\n")
        );
    }

    #[gpui::test]
    fn vim_write_quit_keeps_later_typing_open_until_it_is_saved(cx: &mut gpui::TestAppContext) {
        let fixture = open_stored("hello\r\n", cx);
        let model = persistence(&fixture, cx);
        let lock =
            sqlez::connection::Connection::try_open_file(fixture.path.to_str().unwrap()).unwrap();
        lock.exec("BEGIN IMMEDIATE").unwrap()().unwrap();
        cx.simulate_keystrokes(fixture.window.into(), "i x escape : w q enter");
        while cx.executor().tick() {}
        fixture
            .window
            .update(cx, |view, _, _| {
                assert!(matches!(view.closing.get(), Some(CloseIntent::SaveWindow)));
            })
            .unwrap();
        cx.simulate_keystrokes(fixture.window.into(), "i y escape");
        lock.exec("ROLLBACK").unwrap()().unwrap();
        crate::page_persistence::tests::wait(&model, cx);
        fixture
            .window
            .update(cx, |_, _, cx| {
                assert!(model.read(cx).is_dirty(cx));
                assert!(model.read(cx).is_secured(cx));
            })
            .unwrap();
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        assert_eq!(store.draft("p").unwrap().unwrap().text, "yxhello\r\n");
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(450));
        crate::page_persistence::tests::wait(&model, cx);
        while cx.executor().tick() {}
        assert!(fixture.window.update(cx, |_, _, _| {}).is_err());
        assert!(store.draft("p").unwrap().is_none());
        assert_eq!(
            store
                .checkout("p", None)
                .unwrap()
                .selected
                .unwrap()
                .root()
                .field("text")
                .as_str(),
            Some("yxhello\r\n")
        );
    }

    #[gpui::test]
    fn vim_force_write_quit_merges_changed_heads_instead_of_overwriting(
        cx: &mut gpui::TestAppContext,
    ) {
        let fixture = open_stored("hello\nworld\n", cx);
        let model = persistence(&fixture, cx);
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        let peer = store
            .save(&quire_store::notebook::Edit {
                page: "p".into(),
                path: "Note.md".into(),
                text: "hello\npeer\n".into(),
                base: store.heads("p").unwrap(),
                deleted: false,
                created: 2,
                recovery: None,
            })
            .unwrap();
        cx.simulate_keystrokes(fixture.window.into(), "i x escape : w q ! enter");
        crate::page_persistence::tests::wait(&model, cx);
        while cx.executor().tick() {}
        assert!(fixture.window.update(cx, |_, _, _| {}).is_err());
        let heads = store.heads("p").unwrap();
        assert_eq!(heads.len(), 1);
        assert_ne!(heads, [peer.revision_id()]);
        let head = store.get(&heads[0]).unwrap().unwrap();
        assert_eq!(head.root().field("text").as_str(), Some("xhello\npeer\n"));
        assert!(store.draft("p").unwrap().is_none());
    }

    #[gpui::test]
    fn vim_filename_and_range_writes_remain_errors_without_saving(cx: &mut gpui::TestAppContext) {
        let fixture = open_stored("hello", cx);
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        let heads = store.heads("p").unwrap();
        for keys in [": % w enter", ": w space o t h e r enter"] {
            cx.simulate_keystrokes(fixture.window.into(), keys);
            cx.executor().run_until_parked();
            fixture
                .window
                .update(cx, |view, _, cx| {
                    let prompt = view.editor.as_ref().unwrap().read(cx).commands.read(cx);
                    assert!(prompt.is_open());
                    assert!(prompt.error.as_ref().unwrap().contains("unavailable"));
                })
                .unwrap();
            assert_eq!(store.heads("p").unwrap(), heads);
            assert!(store.draft("p").unwrap().is_none());
            cx.simulate_keystrokes(fixture.window.into(), "escape");
        }
    }

    #[gpui::test]
    fn stored_editor_keyboard_save_commits_the_shared_source(cx: &mut gpui::TestAppContext) {
        let fixture = open_stored("hello\r\n", cx);
        let model = persistence(&fixture, cx);
        fixture
            .window
            .update(cx, |view, _, cx| {
                assert!(
                    !view
                        .editor
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .source
                        .read(cx)
                        .read_only(cx)
                )
            })
            .unwrap();
        cx.simulate_keystrokes(fixture.window.into(), "i x escape");
        cx.simulate_keystrokes(
            fixture.window.into(),
            if cfg!(target_os = "macos") {
                "cmd-s"
            } else {
                "ctrl-s"
            },
        );
        crate::page_persistence::tests::wait(&model, cx);
        cx.update(|cx| assert_eq!(model.read(cx).status(cx), "Saved locally"));
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        assert_eq!(
            store
                .checkout("p", None)
                .unwrap()
                .selected
                .unwrap()
                .root()
                .field("text")
                .as_str(),
            Some("xhello\r\n")
        );
        assert!(store.draft("p").unwrap().is_none());
    }

    #[gpui::test]
    fn close_records_current_recovery_before_removing_the_window(cx: &mut gpui::TestAppContext) {
        let fixture = open_stored("hello\r\n", cx);
        let model = persistence(&fixture, cx);
        cx.simulate_keystrokes(fixture.window.into(), "i x escape");
        fixture
            .window
            .update(cx, |view, _, cx| {
                assert!(!view.close.allow(CloseIntent::Window, cx));
            })
            .unwrap();
        crate::page_persistence::tests::wait(&model, cx);
        while cx.executor().tick() {}
        assert!(fixture.window.update(cx, |_, _, _| {}).is_err());
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        assert_eq!(store.draft("p").unwrap().unwrap().text, "xhello\r\n");
        assert_eq!(
            store
                .checkout("p", None)
                .unwrap()
                .selected
                .unwrap()
                .root()
                .field("text")
                .as_str(),
            Some("hello\r\n")
        );
    }

    #[gpui::test]
    fn native_quit_resumes_after_recording_the_current_recovery(cx: &mut gpui::TestAppContext) {
        let fixture = open_stored("hello\r\n", cx);
        let model = persistence(&fixture, cx);
        cx.simulate_keystrokes(fixture.window.into(), "i x escape");
        assert!(!cx.simulate_native_quit_request());
        assert_eq!(cx.platform_quit_count(), 0);
        crate::page_persistence::tests::wait(&model, cx);
        while cx.executor().tick() {}
        assert_eq!(cx.platform_quit_count(), 1);
        assert!(cx.simulate_native_quit_request());
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        assert_eq!(store.draft("p").unwrap().unwrap().text, "xhello\r\n");
    }

    #[gpui::test]
    fn quit_failure_keeps_the_window_and_current_text_accessible(cx: &mut gpui::TestAppContext) {
        let fixture = open_stored("hello", cx);
        let model = persistence(&fixture, cx);
        crate::page_persistence::tests::stop_storage(&model, cx);
        cx.simulate_keystrokes(fixture.window.into(), "i x escape");
        assert!(!cx.simulate_native_quit_request());
        crate::page_persistence::tests::wait(&model, cx);
        cx.executor().run_until_parked();
        let (message, detail) = cx.pending_prompt().unwrap();
        assert_eq!(message, crate::lifecycle::FAILED);
        assert!(
            detail.contains("could not be recorded for recovery"),
            "{detail}"
        );
        cx.simulate_prompt_answer(crate::lifecycle::KEEP_EDITING);
        cx.executor().run_until_parked();
        assert_eq!(cx.platform_quit_count(), 0);
        fixture
            .window
            .update(cx, |view, _, cx| {
                assert!(view.close_error.is_some());
                assert!(view.closing.get().is_none());
                assert_eq!(
                    view.editor
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .source
                        .read(cx)
                        .text(cx),
                    "xhello"
                );
                view.copy_markdown(cx);
            })
            .unwrap();
        cx.executor().run_until_parked();
        cx.update(|cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), "xhello"));
    }

    #[gpui::test]
    fn semantic_excerpt_edits_and_source_undo_use_the_same_persistence_stream(
        cx: &mut gpui::TestAppContext,
    ) {
        use multi_buffer::MultiBufferOffset;
        let raw = "- hello ^b-one\r\n  - child\r\n- outside\r";
        let fixture = open_stored(raw, cx);
        let model = persistence(&fixture, cx);
        let analysis = cx.update(|cx| fixture.page.read(cx).loaded().unwrap().analysis.clone());
        let excerpt = cx
            .new(|cx| crate::semantic_excerpt::SemanticExcerpt::new(analysis, "b-one".into(), cx));
        cx.executor().run_until_parked();
        let buffer = cx.update(|cx| excerpt.read(cx).buffer.clone());
        buffer.update(cx, |buffer, cx| {
            buffer.edit(
                [(MultiBufferOffset(2)..MultiBufferOffset(2), "x")],
                None,
                cx,
            );
        });
        cx.update(|cx| assert!(model.read(cx).is_dirty(cx)));
        cx.simulate_keystrokes(fixture.window.into(), "u");
        fixture
            .window
            .update(cx, |view, _, cx| {
                assert_eq!(
                    view.editor
                        .as_ref()
                        .unwrap()
                        .read(cx)
                        .source
                        .read(cx)
                        .text(cx),
                    "- hello ^b-one\n  - child\n- outside\n"
                )
            })
            .unwrap();
        cx.simulate_keystrokes(
            fixture.window.into(),
            if cfg!(target_os = "macos") {
                "cmd-s"
            } else {
                "ctrl-s"
            },
        );
        crate::page_persistence::tests::wait(&model, cx);
        let store =
            quire_store::notebook::NotebookStore::open(&fixture.path, "nb", "local").unwrap();
        assert_eq!(
            store
                .checkout("p", None)
                .unwrap()
                .selected
                .unwrap()
                .root()
                .field("text")
                .as_str(),
            Some(raw)
        );
        cx.update(|cx| assert!(!model.read(cx).is_dirty(cx)));
    }

    fn arguments() -> Vec<String> {
        [
            "--notebook-db",
            "/tmp/notebook with spaces.sqlite",
            "--notebook-id",
            "nb",
            "--writer-id",
            "local",
            "--page-id",
            "p",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn explicit_open_arguments_keep_original_identities() {
        assert!(
            OpenOptions::parse(vec!["--write-harness".into()])
                .unwrap()
                .is_none()
        );
        let parsed = OpenOptions::parse(arguments()).unwrap().unwrap();
        assert_eq!(
            parsed.database,
            PathBuf::from("/tmp/notebook with spaces.sqlite")
        );
        assert_eq!(parsed.notebook, "nb");
        assert_eq!(parsed.writer, "local");
        assert_eq!(parsed.page, "p");
    }

    #[test]
    fn incomplete_duplicate_or_mixed_open_arguments_fail_before_startup() {
        for args in [
            vec!["--notebook-id".into()],
            vec!["--notebook-db".into(), "--notebook-id".into()],
            arguments()[..6].to_vec(),
            [arguments(), vec!["--page-id".into(), "other".into()]].concat(),
            [arguments(), vec!["--excerpt-harness".into()]].concat(),
            [arguments(), vec!["--input-harness".into()]].concat(),
            vec!["--notebook-db=/tmp/local.sqlite".into()],
        ] {
            assert!(OpenOptions::parse(args).is_err());
        }
    }

    #[gpui::test]
    fn stored_recovery_draws_in_two_read_only_views_with_conflicts_and_tombstones(
        cx: &mut gpui::TestAppContext,
    ) {
        use quire_store::notebook::{Draft, DraftStamp, Edit, NotebookStore};
        cx.update(|cx| init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.sqlite");
        let store = NotebookStore::open(&path, "nb", "local").unwrap();
        let mut edit = Edit {
            page: "p".into(),
            base: vec![],
            path: "Note.md".into(),
            text: "committed".into(),
            deleted: false,
            created: 1,
            recovery: None,
        };
        let base = store.save(&edit).unwrap();
        edit.base = vec![base.revision_id().into()];
        edit.deleted = true;
        edit.text.clear();
        store.save(&edit).unwrap();
        // A save would merge a second head at once, so the competing version arrives by import.
        let mut other = serde_json::json!({"page":"p","parents":[],"generation":0,"path":"Note.md","text":"other branch","deleted":false,"created":1,"device":"peer"});
        other["id"] = quire_store::sync::format::revision_id(&other)
            .unwrap()
            .into();
        let batch = quire_store::sync::format::encode(
            &serde_json::json!({"notebook":"nb","device":"peer","seq":1,"previous":null,"created":1,"revisions":[other],"attachments":[]}),
            quire_store::sync::format::Kind::Batch,
        )
        .unwrap();
        store
            .import_batch("peer", &batch.name, &batch.data)
            .unwrap();
        let text = "- [ ] recovered اردو #work ^b-one\r\n- end\r";
        let normalized = "- [ ] recovered اردو #work ^b-one\n- end\n";
        let draft = Draft {
            page: "p".into(),
            path: "Recovered.md".into(),
            text: text.into(),
            base: vec![base.revision_id().into()],
            stamp: DraftStamp {
                session: "previous".into(),
                version: 7,
            },
            created: 2,
        };
        store.put_draft(&draft, None).unwrap();
        let heads = store.heads("p").unwrap();
        let outbox = store.outbox().unwrap();
        drop(store);
        let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
        let page = cx.update(|cx| session.open_page("p".into(), cx));
        let window = cx.add_window(|window, cx| {
            StoredWindow::new(session, page.clone(), path.clone(), "nb".into(), window, cx)
        });
        crate::notebook_session::tests::wait(&page, cx);
        cx.executor().run_until_parked();
        window
            .update(cx, |view, window, cx| {
                let page = view.page.read(cx);
                let loaded = page.loaded().unwrap();
                assert!(loaded.deleted);
                assert!(loaded.recovered);
                assert_eq!(loaded.heads.len(), 2);
                assert!(page.status(cx).contains("2 conflicting versions"));
                assert!(page.status(cx).contains("selected version is deleted"));
                assert!(page.status(cx).contains("recovered unsaved changes"));
                let editor = view.editor.as_ref().unwrap();
                let (source, second) = {
                    let editor = editor.read(cx);
                    (editor.source.clone(), editor.second.clone())
                };
                assert!(source.read(cx).read_only(cx));
                assert!(second.read(cx).read_only(cx));
                assert_eq!(source.read(cx).text(cx), normalized);
                assert_eq!(second.read(cx).text(cx), normalized);
                crate::editor_projection::bind(&source, window, cx).unwrap();
            })
            .unwrap();
        cx.executor().run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.simulate_keystrokes(window.into(), "i x escape d d");
        cx.simulate_keystrokes(window.into(), ": w q enter");
        cx.executor().run_until_parked();
        window
            .update(cx, |view, _, cx| {
                let editor = view.editor.as_ref().unwrap().read(cx);
                assert!(editor.commands.read(cx).is_open());
                assert!(
                    editor
                        .commands
                        .read(cx)
                        .error
                        .as_ref()
                        .unwrap()
                        .contains("unavailable")
                );
                assert_eq!(editor.source.read(cx).text(cx), normalized);
                assert_eq!(editor.second.read(cx).text(cx), normalized);
            })
            .unwrap();
        window
            .update(cx, |view, _, cx| view.copy_markdown(cx))
            .unwrap();
        cx.executor().run_until_parked();
        cx.update(|cx| assert_eq!(cx.read_from_clipboard().unwrap().text().unwrap(), text));
        let store = NotebookStore::open(&path, "nb", "local").unwrap();
        assert_eq!(store.heads("p").unwrap(), heads);
        assert_eq!(store.outbox().unwrap(), outbox);
        assert_eq!(store.draft("p").unwrap(), Some(draft));
    }
}
