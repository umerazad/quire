//! The window for moving from the Electron app (`rt`) to Quire and back: migrate with its
//! verification report, check what `rt` will see after Quire, publish Quire's saves for `rt`
//! (a one-way door, confirmed first), and restore `rt` backups as new notebooks that join the
//! active profile. It runs the same steps as the command-line flags.

use super::{REPORT_FILE, default_rt_data, import_ai_settings, migrate, now};
use gpui::{
    App, Context, FocusHandle, Focusable, PathPromptOptions, PromptLevel, SharedString, Task,
    Window, WindowOptions, prelude::*, px,
};
use quire_store::{
    profiles::Notebook,
    rt_migration::{self, RtBackup, RtBackupKind},
};
use std::path::{Path, PathBuf};
use theme::ActiveTheme as _;
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, Disableable as _, Label, LabelCommon as _,
    LabelSize, Tooltip, h_flex, v_flex,
};

gpui::actions!(
    quire_migration,
    [
        /// Opens the move from the Electron app (rt) and the return to it.
        OpenMigration,
    ]
);

pub const CONTEXT: &str = "QuireMigration";
pub const CLOSE_NOTEBOOKS: &str = "Close every notebook window first: migrating and checking the return to rt need each notebook to themselves. Quire keeps this window open.";
pub const PUBLISH_QUESTION: &str = "Publish Quire's saves for rt?";
pub const PUBLISH_DETAIL: &str = "Quire writes its saves into each Quire notebook's synced folder as change files that every computer keeps. This cannot be undone. Folder notebooks need no publishing.";

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenMigration, cx| open(paths::data_dir().clone(), cx));
}

/// Opens the migration window for Quire's data folder `quire_data`, or brings it forward.
pub fn open(quire_data: PathBuf, cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<MigrationWindow>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(px(680.), px(760.)),
            cx,
        ))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_window_title("Move from rt");
        cx.new(|cx| MigrationWindow::new(quire_data, default_rt_data(), window, cx))
    });
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Cannot open the migration window: {error}. Check desktop access, then retry."),
            cx,
        );
    }
}

/// A notebook restored from an `rt` backup, until it is opened.
#[derive(Clone, Debug)]
pub struct RestoredNotebook {
    pub root: PathBuf,
    pub notebook: Notebook,
}

pub struct MigrationWindow {
    focus: FocusHandle,
    quire_data: PathBuf,
    pub rt_data: Option<PathBuf>,
    running: Option<SharedString>,
    error: Option<SharedString>,
    /// The last report, as Markdown.
    report: Option<String>,
    backup_folder: Option<PathBuf>,
    backups: Vec<RtBackup>,
    restored: Vec<RestoredNotebook>,
    work: Option<Task<()>>,
}

fn folder_prompt(prompt: &str) -> PathPromptOptions {
    PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(prompt.to_owned().into()),
    }
}

type Chosen = futures::channel::oneshot::Receiver<anyhow::Result<Option<Vec<PathBuf>>>>;

async fn first_path(chosen: Chosen) -> Option<PathBuf> {
    chosen.await.ok()?.ok()??.into_iter().next()
}

impl MigrationWindow {
    pub fn new(
        quire_data: PathBuf,
        rt_data: Option<PathBuf>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        crate::preferences::follow_system_appearance(window, cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            focus,
            quire_data,
            rt_data,
            running: None,
            error: None,
            report: None,
            backup_folder: None,
            backups: Vec::new(),
            restored: Vec::new(),
            work: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.running.is_some()
    }

    #[cfg(test)]
    pub fn report(&self) -> Option<&str> {
        self.report.as_deref()
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    #[cfg(test)]
    pub fn backups(&self) -> &[RtBackup] {
        &self.backups
    }

    #[cfg(test)]
    pub fn restored(&self) -> &[RestoredNotebook] {
        &self.restored
    }

    /// Refuses while another step runs, or while a notebook window holds a notebook's lock.
    fn ready(&mut self, needs_closed_notebooks: bool, cx: &mut Context<Self>) -> bool {
        self.error = if self.is_running() {
            Some("Another step is still running. Wait for it to finish.".into())
        } else if needs_closed_notebooks && crate::editor_host::notebook_windows(cx) > 0 {
            Some(CLOSE_NOTEBOOKS.into())
        } else {
            None
        };
        cx.notify();
        self.error.is_none()
    }

    /// Runs `work` off the UI thread and shows its outcome.
    fn run<T: Send + 'static>(
        &mut self,
        label: &str,
        work: impl FnOnce() -> anyhow::Result<T> + Send + 'static,
        done: impl FnOnce(&mut Self, T, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        self.running = Some(label.to_owned().into());
        self.error = None;
        cx.notify();
        let work = cx.background_spawn(async move { work() });
        self.work = Some(cx.spawn(async move |this, cx| {
            let result = work.await;
            this.update(cx, |this, cx| {
                this.running = None;
                this.work = None;
                match result {
                    Ok(value) => done(this, value, cx),
                    Err(error) => this.error = Some(format!("{error:#}").into()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    pub fn choose_rt_data(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(folder_prompt("Use as rt's data folder"));
        cx.spawn(async move |this, cx| {
            if let Some(folder) = first_path(chosen).await {
                this.update(cx, |this, cx| {
                    this.rt_data = Some(folder);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    /// Migrates `rt`'s notebooks, profiles, tabs, and AI history into Quire's data folder, then
    /// takes up the profiles and AI settings it wrote.
    pub fn migrate(&mut self, cx: &mut Context<Self>) {
        if !self.ready(true, cx) {
            return;
        }
        let Some(rt_data) = self.rt_data.clone() else {
            self.error = Some("rt's data folder was not found. Choose it, usually Quire under your application data folder.".into());
            return cx.notify();
        };
        let quire_data = self.quire_data.clone();
        self.run(
            "Migrating from rt…",
            move || migrate(&rt_data, &quire_data).map(|(_, markdown)| markdown),
            |this, mut markdown, cx| {
                crate::profiles::load(this.quire_data.join("profiles.json"), cx);
                if let Some(line) = import_ai_settings(&this.quire_data, cx) {
                    markdown.push_str(&format!("- {line}\n"));
                }
                this.report = Some(markdown);
            },
            cx,
        );
    }

    /// Reports what `rt` will see of Quire's work, without changing anything.
    pub fn check_return(&mut self, cx: &mut Context<Self>) {
        self.prepare_return(false, cx);
    }

    /// Asks before publishing, since change files in a synced folder cannot be taken back.
    pub fn publish_for_rt(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.ready(true, cx) {
            return;
        }
        let answer = window.prompt(
            PromptLevel::Warning,
            PUBLISH_QUESTION,
            Some(PUBLISH_DETAIL),
            &["Publish", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await == Ok(0) {
                this.update(cx, |this, cx| this.prepare_return(true, cx))
                    .ok();
            }
        })
        .detach();
    }

    fn prepare_return(&mut self, publish: bool, cx: &mut Context<Self>) {
        if !self.ready(true, cx) {
            return;
        }
        let quire_data = self.quire_data.clone();
        self.run(
            if publish {
                "Publishing Quire's saves for rt…"
            } else {
                "Checking what rt will see…"
            },
            move || rt_migration::prepare_rollback(&quire_data, publish, now()),
            |this, report, _| this.report = Some(report.to_markdown()),
            cx,
        );
    }

    /// Lists the `rt` backups in a chosen backup folder.
    pub fn choose_backups(&mut self, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(folder_prompt("List rt backups in this folder"));
        cx.spawn(async move |this, cx| {
            if let Some(folder) = first_path(chosen).await {
                this.update(cx, |this, cx| this.list_backups(folder, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn list_backups(&mut self, folder: PathBuf, cx: &mut Context<Self>) {
        if !self.ready(false, cx) {
            return;
        }
        let listed = folder.clone();
        self.run(
            "Listing rt backups…",
            move || Ok(rt_migration::list_backups(&listed)),
            move |this, backups, _| {
                if backups.is_empty() {
                    this.error = Some(
                        format!(
                            "No completed rt backups are in {}. Choose rt's backup destination, the folder that holds Quire/<profile>/.",
                            folder.display()
                        )
                        .into(),
                    );
                }
                this.backup_folder = Some(folder);
                this.backups = backups;
            },
            cx,
        );
    }

    /// Asks for a parent folder, then restores `backup` there as a new notebook.
    pub fn choose_restore_parent(&mut self, backup: PathBuf, cx: &mut Context<Self>) {
        let chosen = cx.prompt_for_paths(folder_prompt("Restore the backup into this folder"));
        cx.spawn(async move |this, cx| {
            if let Some(parent) = first_path(chosen).await {
                this.update(cx, |this, cx| this.restore(backup, parent, cx))
                    .ok();
            }
        })
        .detach();
    }

    pub fn restore(&mut self, backup: PathBuf, parent: PathBuf, cx: &mut Context<Self>) {
        if !self.ready(false, cx) {
            return;
        }
        let quire_data = self.quire_data.clone();
        let source = backup.clone();
        self.run(
            "Verifying and restoring the backup…",
            move || rt_migration::restore_backup(&source, &parent, &quire_data, now()),
            move |this, restored, _| {
                let report = rt_migration::Report {
                    source: backup,
                    notebooks: vec![restored.report],
                    ..Default::default()
                };
                this.report = Some(format!(
                    "Restored as {}.\n\n{}",
                    restored.root.display(),
                    report.to_markdown()
                ));
                this.restored.push(RestoredNotebook {
                    root: restored.root,
                    notebook: restored.notebook,
                });
            },
            cx,
        );
    }

    /// Opens a restored notebook as the active profile's notebook, saving open pages first.
    pub fn open_restored(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(restored) = self.restored.get(index).cloned() else {
            return;
        };
        let id = crate::profiles::active(cx);
        cx.defer(move |cx| crate::editor_host::open_in_profile(id, restored.notebook, cx));
    }

    fn reveal_report(&mut self, cx: &mut Context<Self>) {
        cx.reveal_path(&self.quire_data.join(REPORT_FILE));
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, _: &mut Context<Self>) {
        if !self.is_running() {
            window.remove_window();
        }
    }

    fn hint(text: impl Into<SharedString>) -> Label {
        Label::new(text).size(LabelSize::Small).color(Color::Muted)
    }

    fn section(title: &'static str) -> gpui::Div {
        v_flex()
            .gap_2()
            .pt_3()
            .child(Label::new(title).size(LabelSize::Large))
    }

    fn backup_line(backup: &RtBackup) -> String {
        let kind = match backup.kind {
            RtBackupKind::Folder => "folder notebook",
            RtBackupKind::Quire => "Quire notebook",
        };
        let name = backup
            .source_name
            .as_deref()
            .map(|name| format!(" · {name}"))
            .unwrap_or_default();
        format!(
            "{} · {} profile · {kind}{name}",
            crate::preferences::data::when(Some(backup.created_at)),
            backup.profile
        )
    }
}

impl Focusable for MigrationWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn path_label(path: Option<&Path>) -> String {
    path.map_or_else(
        || "Not found. Choose rt's data folder.".into(),
        |path| path.display().to_string(),
    )
}

impl Render for MigrationWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_running();
        let active = crate::profiles::active(cx);
        v_flex()
            .id("rt-migration")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_1()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(Label::new("Move from rt").size(LabelSize::Large))
            .child(Self::hint(
                "Quire reads rt's data and never changes it, so rt still opens what it opened before. Migrating again later finishes anything left over and repeats nothing.",
            ))
            .child(
                Self::section("Migrate")
                    .child(Label::new("rt's data folder"))
                    .child(Label::new(path_label(self.rt_data.as_deref())).size(LabelSize::Small))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("choose-rt-data", "Choose rt's data folder")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.choose_rt_data(cx))),
                            )
                            .child(
                                Button::new("migrate-from-rt", "Migrate from rt")
                                    .disabled(busy || self.rt_data.is_none())
                                    .tooltip(Tooltip::text(
                                        "Bring rt's notebooks, drafts, history, profiles, tabs, and AI settings into Quire",
                                    ))
                                    .on_click(cx.listener(|this, _, _, cx| this.migrate(cx))),
                            ),
                    )
                    .child(Self::hint(
                        "Quit rt first. Folder notebooks stay where they are; Quire notebooks are read from their synced folders. AI settings arrive turned off until you press Test in Settings > AI.",
                    )),
            )
            .child(
                Self::section("Return to rt")
                    .child(Self::hint(
                        "Check first: it reports what rt will see and changes nothing. Folder notebooks need nothing more. Quire notebooks show Quire's saves in rt only after they are published.",
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Button::new("check-return", "Check what rt will see")
                                    .disabled(busy)
                                    .on_click(cx.listener(|this, _, _, cx| this.check_return(cx))),
                            )
                            .child(
                                Button::new("publish-for-rt", "Publish for rt…")
                                    .disabled(busy)
                                    .tooltip(Tooltip::text(
                                        "Write Quire's saves into each Quire notebook's synced folder; this cannot be undone",
                                    ))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.publish_for_rt(window, cx)
                                    })),
                            ),
                    ),
            )
            .child(
                Self::section("rt backups")
                    .child(Self::hint(format!(
                        "Restoring verifies the backup, then creates a new notebook beside your others. Open it to make it the {} profile's notebook.",
                        active.label()
                    )))
                    .child(
                        Button::new("list-rt-backups", "Choose rt's backup folder")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.choose_backups(cx))),
                    )
                    .children(self.backups.iter().enumerate().map(|(index, backup)| {
                        let path = backup.path.clone();
                        h_flex()
                            .gap_2()
                            .child(Label::new(Self::backup_line(backup)))
                            .child(
                                Button::new(("restore-rt-backup", index), "Restore…")
                                    .disabled(busy)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.choose_restore_parent(path.clone(), cx)
                                    })),
                            )
                    }))
                    .children(self.restored.iter().enumerate().map(|(index, restored)| {
                        h_flex()
                            .gap_2()
                            .child(Label::new(restored.root.display().to_string()))
                            .child(
                                Button::new(
                                    ("open-restored", index),
                                    format!("Open as the {} notebook", active.label()),
                                )
                                .disabled(busy)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.open_restored(index, cx)
                                })),
                            )
                    })),
            )
            .when_some(self.running.clone(), |this, running| {
                this.child(Label::new(running))
            })
            .when_some(self.error.clone(), |this, error| {
                this.child(Label::new(error).color(Color::Error))
            })
            .when_some(self.report.clone(), |this, report| {
                this.child(
                    Self::section("Report")
                        .child(
                            Button::new("show-report", "Show report file")
                                .on_click(cx.listener(|this, _, _, cx| this.reveal_report(cx))),
                        )
                        .child(
                            v_flex()
                                .id("migration-report")
                                .p_2()
                                .border_1()
                                .rounded_md()
                                .text_sm()
                                .children(
                                    report
                                        .lines()
                                        .map(|line| Label::new(line.to_owned()).size(LabelSize::Small)),
                                ),
                        ),
                )
            })
    }
}
