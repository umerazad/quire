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

mod folder;
mod links;
mod navigation;

pub(super) use folder::FolderOptions;

pub(super) fn bind_keys(cx: &mut App) {
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

fn allow_close(
    session: Rc<RefCell<NotebookSession>>,
    closing: Rc<Cell<Option<CloseIntent>>>,
    intent: CloseIntent,
    cx: &App,
) -> bool {
    if session.borrow().is_secured(cx) {
        return true;
    }
    closing.set(Some(intent));
    cx.spawn(async move |cx| {
        cx.update(|cx| session.borrow().recover(cx));
    })
    .detach();
    false
}

pub(super) struct OpenOptions {
    database: PathBuf,
    notebook: String,
    writer: String,
    page: String,
}

impl OpenOptions {
    pub fn parse(arguments: Vec<String>) -> Result<Option<Self>, String> {
        const FLAGS: [&str; 4] = ["--notebook-db", "--notebook-id", "--writer-id", "--page-id"];
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
                    "Unsupported stored-page argument {flag:?}. Pass --notebook-db, --notebook-id, --writer-id, and --page-id with their values; open editor harnesses separately."
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
        let mut take = |flag| {
            values.remove(flag).ok_or_else(|| format!("Missing stored-page argument {flag}. Pass --notebook-db, --notebook-id, --writer-id, and --page-id with the original database identities."))
        };
        Ok(Some(Self {
            database: take("--notebook-db")?.into(),
            notebook: take("--notebook-id")?,
            writer: take("--writer-id")?,
            page: take("--page-id")?,
        }))
    }

    pub fn open(self, cx: &mut App) -> Result<(), String> {
        let database = self.database.clone();
        let notebook = self.notebook.clone();
        let mut session = NotebookSession::open(self.database, self.notebook, self.writer)?;
        let page = session.open_page(self.page, cx);
        let options = gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                None,
                gpui::size(gpui::px(1100.), gpui::px(760.)),
                cx,
            ))),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            window.set_window_title("Quire Native");
            cx.new(|cx| StoredWindow::new(session, page, database, notebook, window, cx))
        }).map_err(|error| format!("Cannot open Quire's window: {error}. Check desktop and graphics access, then retry."))?;
        cx.activate(true);
        Ok(())
    }
}

struct StoredWindow {
    session: Rc<RefCell<NotebookSession>>,
    closing: Rc<Cell<Option<CloseIntent>>>,
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
        let closing = Rc::new(Cell::new(None));
        let quit_guard = cx.on_request_quit({
            let session = session.clone();
            let closing = closing.clone();
            move |cx| allow_close(session.clone(), closing.clone(), CloseIntent::Quit, cx)
        });
        window.on_window_should_close(cx, {
            let session = session.clone();
            let closing = closing.clone();
            move |_, cx| allow_close(session.clone(), closing.clone(), CloseIntent::Window, cx)
        });
        session.borrow().enable_editing(cx);
        cx.observe_global::<crate::outline::Notice>(|_, cx| cx.notify())
            .detach();
        let calendar = DailyCalendar::system()
            .map_err(|error| gpui::SharedString::from(format!("Cannot read the system time zone for daily pages: {error}. Set a valid time zone and reopen the notebook.")));
        let index = match &calendar {
            Ok(_) => Err("Reading notebook settings…".into()),
            Err(error) => Err(error.clone()),
        };
        let nav = navigation::Navigation::new(&database, &notebook, calendar.ok());
        let id = page.read(cx).id().to_owned();
        let links = links::Links::new(cx.weak_entity(), Rc::downgrade(&session), cx);
        let mut this = Self {
            session,
            closing,
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
        };
        let pages = this.session.borrow().open_pages();
        for observed in pages {
            this.watch(&observed, window, cx);
        }
        this.visit(quire_core::workspace::Destination::page(id), window, cx);
        if let Some(side) = this.nav.workspace.side.clone() {
            this.place_beside(side, false, window, cx);
        }
        this.load_settings(cx);
        this
    }

    fn index_status(&self, cx: &gpui::App) -> Option<gpui::SharedString> {
        match &self.index {
            Ok(index) => {
                let index = index.read(cx);
                index.error().cloned().or_else(|| index.status())
            }
            Err(error) => Some(error.clone()),
        }
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
        let editor = cx.new(|cx| {
            let editor = NotebookWindow::for_page(analysis, None, window, cx);
            if writes_enabled {
                editor
                    .commands
                    .update(cx, |commands, _| commands.enable_writes());
            }
            editor
        });
        self.link_editors(&editor, window, cx);
        if let Some(previous) = previous {
            window.focus(&previous, cx);
        }
        self.cache_editor(page, editor.clone());
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
            if self.nav.palette.is_none() {
                self.focus_active(window, cx);
            }
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
        self.session.borrow().is_secured(cx)
            && (!matches!(self.closing.get(), Some(CloseIntent::SaveWindow))
                || self
                    .page
                    .read(cx)
                    .persistence()
                    .is_some_and(|page| !page.read(cx).is_dirty(cx)))
    }

    fn finish_close(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
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
                        match this.closing.take() {
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
        } else if self.closing.get().is_some()
            && (write_failed || self.session.borrow().has_error(cx))
        {
            self.closing.set(None);
            self.close_error = Some(if write_failed {
                "The page could not be saved locally. Keep this window open, resolve the save error, and retry :wq. Copy Markdown remains available."
            } else {
                "Some pages could not be recorded for recovery. Keep this window open, check the storage error, and retry closing after it is corrected. Copy Markdown remains available."
            }.into());
        }
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
        let conflict = self.render_folder_conflict(cx);
        let page = self.page.read(cx);
        let loading = page.is_loading();
        let header = gpui::div()
            .p_2()
            .flex()
            .items_center()
            .gap_2()
            .child(Label::new(page.status(cx)))
            .when(page.persistence().is_some(), |div| {
                div.child(
                    Button::new("save-page", "Save locally")
                        .on_click(cx.listener(|this, _, _, cx| this.save_page(cx))),
                )
            })
            .when(page.loaded().is_some(), |div| {
                div.child(
                    Button::new(
                        "copy-markdown",
                        if self.copy_task.is_some() {
                            "Copying Markdown…"
                        } else {
                            "Copy Markdown"
                        },
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.copy_markdown(cx))),
                )
            })
            .when(
                page.persistence().is_some() && self.shown_page(cx).is_some(),
                |div| {
                    div.child(Button::new("rename-page", "Rename…").on_click(
                        cx.listener(|this, _, window, cx| this.rename_shown_page(window, cx)),
                    ))
                    .child(
                        Button::new("delete-page", "Delete page…").on_click(
                            cx.listener(|this, _, window, cx| this.delete_shown_page(window, cx)),
                        ),
                    )
                },
            )
            .when(page.can_retry(), |div| {
                div.child(
                    Button::new("retry-page", "Retry page").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.page.update(cx, |page, cx| page.retry(cx));
                        },
                    )),
                )
            });
        let root = gpui::div()
            .size_full()
            .relative()
            .on_action(cx.listener(|this, _: &SavePage, window, cx| this.save_focused(window, cx)))
            .on_action(cx.listener(|this, _: &SaveAndClosePage, window, cx| {
                this.save_and_close(window, cx)
            }));
        Self::register_navigation(root, cx)
            .flex()
            .flex_col()
            .bg(cx.theme().colors().editor_background)
            .child(header)
            .when_some(conflict, |div, conflict| div.child(conflict))
            .when_some(self.close_error.clone(), |div, error| {
                div.child(gpui::div().p_2().child(error))
            })
            .when_some(self.copy_error.clone(), |div, error| {
                div.child(gpui::div().p_2().child(error))
            })
            .when_some(self.index_status(cx), |div, error| {
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
            .child(self.render_body(window, cx))
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
                        .second
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
                assert!(!allow_close(
                    view.session.clone(),
                    view.closing.clone(),
                    CloseIntent::Window,
                    cx
                ));
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
                crate::editor_projection::bind(&second, window, cx).unwrap();
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
