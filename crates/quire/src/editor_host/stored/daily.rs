//! Daily notes in the window: today's page, the notebook's daily layout read from the replicated
//! settings page, and the layout editor in the Daily notes browser.

use super::*;
use crate::navigation::now_millis;
use editor::Editor;
use quire_core::daily::{
    DailyCalendar, DailyLayout, detect_layout, format_daily, read_settings, write_settings,
};
use quire_store::{index::SETTINGS_PAGE, notebook::Edit};
use ui::{Color, Disableable, LabelSize, h_flex, v_flex};

/// The settings page's path, as the reference sync engine writes it.
const SETTINGS_PATH: &str = "notebook-settings";

pub(in crate::editor_host::stored) struct DailyState {
    pub(in crate::editor_host::stored) layout: DailyLayout,
    pub(in crate::editor_host::stored) calendar: Option<DailyCalendar>,
    pub(in crate::editor_host::stored) read_only: Option<SharedString>,
    heads: Vec<String>,
    pub(in crate::editor_host::stored) editing: Option<[Entity<Editor>; 2]>,
    pub(in crate::editor_host::stored) now: Rc<dyn Fn() -> i64>,
    task: Option<Task<()>>,
}

impl DailyState {
    pub(in crate::editor_host::stored) fn new(calendar: Option<DailyCalendar>) -> Self {
        Self {
            layout: DailyLayout::default(),
            calendar,
            read_only: None,
            heads: Vec::new(),
            editing: None,
            now: Rc::new(now_millis),
            task: None,
        }
    }
}

fn text_field(text: &str, window: &mut Window, cx: &mut App) -> Entity<Editor> {
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_text(text, window, cx);
        editor
    })
}

impl StoredWindow {
    pub(in crate::editor_host::stored) fn daily_folder(&self, date: &str) -> Option<String> {
        let calendar = self.nav.daily.calendar.as_ref()?;
        self.nav.daily.layout.folder(date, calendar).ok()
    }

    /// Reads the notebook's daily layout from its settings page, then starts the index with it,
    /// so date titles and links follow the configured pattern from the first query.
    pub(in crate::editor_host::stored) fn load_settings(&mut self, cx: &mut gpui::Context<Self>) {
        let storage = self.session.borrow().storage();
        self.nav.daily.task = Some(cx.spawn(async move |this, cx| {
            let checkout = match storage.checkout(SETTINGS_PAGE.into(), None) {
                Ok(reply) => reply.await.map_err(|error| format!("{error:#}")),
                Err(error) => Err(error.to_string()),
            };
            this.update(cx, |this, cx| {
                match checkout {
                    Ok(checkout) => {
                        let text = checkout
                            .selected
                            .as_ref()
                            .and_then(|record| record.root().field("text").as_str().map(str::to_owned));
                        let settings = read_settings(text.as_deref());
                        this.nav.daily.layout = settings.layout;
                        this.nav.daily.read_only = settings.read_only.map(SharedString::from);
                        this.nav.daily.heads = checkout.heads;
                    }
                    Err(error) => {
                        this.nav.daily.read_only = Some(
                            format!("Cannot read notebook settings: {error}. Daily notes use the default layout; reopen the notebook to retry.").into(),
                        );
                    }
                }
                this.nav.daily.task = None;
                this.start_index(cx);
            })
            .ok();
        }));
    }

    pub(in crate::editor_host::stored) fn start_index(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(calendar) = self.nav.daily.calendar.clone() else {
            return;
        };
        let config = crate::query_index::config(self.nav.daily.layout.clone(), calendar);
        self.index =
            IndexSync::spawn(self.nav.database.clone(), config, cx).map_err(SharedString::from);
        if let Ok(index) = &self.index {
            cx.observe(index, |this, _, cx| {
                this.refresh_catalogue(cx);
                cx.notify();
            })
            .detach();
            cx.subscribe(
                index,
                |this, _, _: &crate::query_index::IndexChanged, cx| this.refresh_queries(cx),
            )
            .detach();
            let pages = self.session.borrow().open_pages();
            index.update(cx, |index, cx| {
                for page in pages {
                    index.track(page, cx);
                }
            });
        }
        self.refresh_catalogue(cx);
        self.refresh_queries(cx);
        cx.notify();
    }

    fn today(&self) -> Result<String, String> {
        let calendar = self.nav.daily.calendar.as_ref().ok_or_else(|| {
            "Daily notes need the system time zone. Set a valid time zone and reopen the notebook."
                .to_owned()
        })?;
        calendar.date_at((self.nav.daily.now)())
    }

    /// The reference `openDaily`: today's existing page, else a new one from the layout.
    pub(in crate::editor_host::stored) fn open_today(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let (Some(client), Ok(date), Some(calendar)) = (
            self.index_client(cx),
            self.today(),
            self.nav.daily.calendar.clone(),
        ) else {
            self.nav.error = Some(
                self.today()
                    .err()
                    .unwrap_or_else(|| "Today's page needs the notebook index, which did not start. Reopen the notebook to retry.".into())
                    .into(),
            );
            cx.notify();
            return;
        };
        let layout = self.nav.daily.layout.clone();
        let lookup = client.with_generation({
            let date = date.clone();
            move |index| index.daily_page(&date)
        });
        self.nav.daily.task = Some(cx.spawn_in(window, async move |this, cx| {
            let found = match lookup {
                Ok(reply) => reply.await.map_err(|error| format!("{error:#}")),
                Err(error) => Err(error.to_string()),
            };
            this.update_in(cx, |this, window, cx| {
                this.nav.daily.task = None;
                match found {
                    Ok((_, Some(page))) => {
                        this.open_page(page.page.to_lossy_string(), false, window, cx)
                    }
                    Ok((_, None)) => {
                        let title = format_daily(&layout.title, &date, &calendar);
                        let folder = layout.folder(&date, &calendar);
                        match title.and_then(|title| Ok((title, folder?))) {
                            Ok((title, folder)) => this.create(title, folder, false, window, cx),
                            Err(error) => this.nav.error = Some(error.into()),
                        }
                    }
                    Err(error) => {
                        this.nav.error = Some(
                            format!("Cannot look up today's page: {error}. Wait for indexing to finish, then retry.").into(),
                        )
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(in crate::editor_host::stored) fn edit_layout(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let layout = self.nav.daily.layout.clone();
        let fields = [
            text_field(&layout.folder, window, cx),
            text_field(&layout.title, window, cx),
        ];
        for field in &fields {
            cx.observe(field, |_, _, cx| cx.notify()).detach();
        }
        window.focus(&fields[0].focus_handle(cx), cx);
        self.nav.daily.editing = Some(fields);
        cx.notify();
    }

    pub(in crate::editor_host::stored) fn typed_layout(&self, cx: &App) -> Option<DailyLayout> {
        let [folder, title] = self.nav.daily.editing.as_ref()?;
        Some(DailyLayout {
            folder: folder.read(cx).text(cx),
            title: title.read(cx).text(cx),
        })
    }

    fn detect(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        let (Some(calendar), Some([folder, title])) = (
            self.nav.daily.calendar.as_ref(),
            self.nav.daily.editing.clone(),
        ) else {
            return;
        };
        let detected = detect_layout(
            self.nav
                .pages
                .iter()
                .map(|listed| listed.page.path.as_str()),
            calendar,
        );
        match detected {
            Some(layout) => {
                folder.update(cx, |editor, cx| editor.set_text(layout.folder, window, cx));
                title.update(cx, |editor, cx| editor.set_text(layout.title, window, cx));
            }
            None => self.nav.error = Some(
                "No date-titled pages to detect a layout from. Type the folder and title instead."
                    .into(),
            ),
        }
        cx.notify();
    }

    /// Saves the typed layout to the replicated settings page and restarts the index with it.
    /// Existing pages are never moved.
    pub(in crate::editor_host::stored) fn save_layout(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(layout) = self.typed_layout(cx) else {
            return;
        };
        if let Some(problem) = &self.nav.daily.read_only {
            self.nav.error = Some(problem.clone());
            cx.notify();
            return;
        }
        if let Some(problem) = layout.problem() {
            self.nav.error = Some(problem.into());
            cx.notify();
            return;
        }
        let storage = self.session.borrow().storage();
        let edit = Edit {
            page: SETTINGS_PAGE.into(),
            base: self.nav.daily.heads.iter().take(1).cloned().collect(),
            path: SETTINGS_PATH.into(),
            text: write_settings(&layout),
            deleted: false,
            created: now_millis() as u64,
            recovery: None,
        };
        self.nav.daily.task = Some(cx.spawn(async move |this, cx| {
            let saved = match storage.save(edit) {
                Ok(reply) => reply.await.map_err(|error| format!("{error:#}")),
                Err(error) => Err(error.to_string()),
            };
            this.update(cx, |this, cx| {
                this.nav.daily.task = None;
                match saved {
                    Ok(ack) => {
                        this.nav.daily.heads = ack.heads;
                        this.nav.daily.layout = layout;
                        this.nav.daily.editing = None;
                        this.nav.error = None;
                        this.start_index(cx);
                    }
                    Err(error) => this.nav.error = Some(
                        format!(
                            "Cannot save the daily layout: {error}. Check free space, then retry."
                        )
                        .into(),
                    ),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub(in crate::editor_host::stored) fn render_daily_settings(
        &mut self,
        cx: &mut gpui::Context<Self>,
    ) -> gpui::Div {
        let layout = self
            .typed_layout(cx)
            .unwrap_or_else(|| self.nav.daily.layout.clone());
        let preview = match (self.today(), self.nav.daily.calendar.as_ref()) {
            (Ok(date), Some(calendar)) => match layout.problem() {
                Some(problem) => problem.to_owned(),
                None => layout
                    .path(&date, calendar)
                    .map(|path| format!("Today's page: {path}"))
                    .unwrap_or_else(|error| error.to_owned()),
            },
            (Err(error), _) => error,
            (Ok(_), None) => String::new(),
        };
        let editing = self.nav.daily.editing.clone();
        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Label::new(preview)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
                    .child(
                        Button::new("open-today", "Open today").on_click(
                            cx.listener(|this, _, window, cx| this.open_today(window, cx)),
                        ),
                    )
                    .when(editing.is_none(), |div| {
                        div.child(Button::new("edit-layout", "Daily layout…").on_click(
                            cx.listener(|this, _, window, cx| this.edit_layout(window, cx)),
                        ))
                    }),
            )
            .when_some(self.nav.daily.read_only.clone(), |div, warning| {
                div.child(
                    Label::new(warning)
                        .size(LabelSize::Small)
                        .color(Color::Warning),
                )
            })
            .when_some(editing, |div, [folder, title]| {
                let field = |label: &'static str, editor: Entity<Editor>| {
                    h_flex()
                        .gap_2()
                        .child(Label::new(label).size(LabelSize::Small))
                        .child(
                            gpui::div()
                                .flex_1()
                                .border_1()
                                .border_color(cx.theme().colors().border)
                                .p_1()
                                .child(editor),
                        )
                };
                div.child(field("Folder", folder))
                    .child(field("Title", title))
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("detect-layout", "Detect from notebook").on_click(
                                    cx.listener(|this, _, window, cx| this.detect(window, cx)),
                                ),
                            )
                            .child(
                                Button::new("save-layout", "Save daily layout")
                                    .disabled(self.nav.daily.read_only.is_some())
                                    .on_click(cx.listener(|this, _, _, cx| this.save_layout(cx))),
                            )
                            .child(Button::new("cancel-layout", "Cancel").on_click(cx.listener(
                                |this, _, _, cx| {
                                    this.nav.daily.editing = None;
                                    cx.notify();
                                },
                            ))),
                    )
            })
    }
}
