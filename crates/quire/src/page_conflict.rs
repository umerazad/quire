//! A Quire notebook page that two computers changed from the same version and whose versions could
//! not be merged. Every version is kept until one is chosen here; the choice is saved as following
//! all of them, so every computer that imports it sees one version again.

use crate::notebook_session::OpenPage;
use gpui::{Context, EventEmitter, SharedString, Task, WeakEntity, Window, prelude::*};
use quire_core::daily::DailyCalendar;
use quire_store::{live_sync::read_statuses, notebook::ConflictVersion, worker::StorageClient};
use std::{
    collections::HashMap,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};
use theme::ActiveTheme as _;
use ui::{Button, Clickable as _, Disableable as _, Label, LabelCommon as _};

/// Lines of each version the comparison shows.
const COMPARED_LINES: usize = 400;

/// One competing version, named for the person choosing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VersionChoice {
    /// The computer that saved it: "this computer", another computer's name, or "another
    /// computer" when its status file is missing.
    pub computer: String,
    pub when: String,
    pub text: String,
    pub deleted: bool,
}

/// Names each version's computer and time. `writer` is this app's writer; `folder` is the
/// synced folder whose status files name the other writers' computers.
pub fn describe(
    versions: Vec<ConflictVersion>,
    writer: &str,
    folder: Option<&Path>,
) -> (Vec<String>, Vec<VersionChoice>) {
    let labels: HashMap<String, String> = folder
        .map(read_statuses)
        .unwrap_or_default()
        .into_iter()
        .map(|status| (status.device, status.label))
        .collect();
    let calendar = DailyCalendar::system().unwrap_or_else(|_| DailyCalendar::utc());
    versions
        .into_iter()
        .map(|version| {
            let computer = if version.device == writer {
                "this computer".to_owned()
            } else {
                labels
                    .get(&version.device)
                    .cloned()
                    .unwrap_or_else(|| "another computer".to_owned())
            };
            let when = calendar
                .clock_at(i64::try_from(version.created).unwrap_or(i64::MAX))
                .map_or_else(
                    |_| format!("saved at {}", version.created),
                    |(date, hour, minute)| format!("{date} {hour:02}:{minute:02}"),
                );
            (
                version.revision,
                VersionChoice {
                    computer,
                    when,
                    text: version.text,
                    deleted: version.deleted,
                },
            )
        })
        .unzip()
}

/// The text "Keep both" saves: every version in order, each ending with a line break.
pub fn both(versions: &[VersionChoice]) -> String {
    versions
        .iter()
        .map(|version| {
            if version.text.ends_with('\n') {
                version.text.clone()
            } else {
                format!("{}\n", version.text)
            }
        })
        .collect()
}

/// A version was chosen, so the page now has one version and opens again.
pub struct ConflictResolved;

impl EventEmitter<ConflictResolved> for OpenPage {}

pub struct PageConflict {
    page: String,
    heads: Vec<String>,
    versions: Vec<VersionChoice>,
    client: StorageClient,
    open: WeakEntity<OpenPage>,
    comparing: bool,
    error: Option<SharedString>,
    task: Option<Task<()>>,
}

impl PageConflict {
    pub fn new(
        page: String,
        heads: Vec<String>,
        versions: Vec<VersionChoice>,
        client: StorageClient,
        open: WeakEntity<OpenPage>,
    ) -> Self {
        Self {
            page,
            heads,
            versions,
            client,
            open,
            comparing: false,
            error: None,
            task: None,
        }
    }

    #[cfg(test)]
    pub fn versions(&self) -> &[VersionChoice] {
        &self.versions
    }

    pub fn is_busy(&self) -> bool {
        self.task.is_some()
    }

    /// Keeps one version, or every version one after the other with `None`, then reopens the page.
    pub fn keep(&mut self, choice: Option<usize>, cx: &mut Context<Self>) {
        if self.task.is_some() {
            return;
        }
        let text = match choice {
            Some(index) => match self.versions.get(index) {
                Some(version) => version.text.clone(),
                None => return,
            },
            None => both(&self.versions),
        };
        let created = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |time| {
                u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
            });
        let request =
            self.client
                .resolve_conflict(self.page.clone(), self.heads.clone(), text, created);
        self.error = None;
        self.task = Some(cx.spawn(async move |this, cx| {
            let result = crate::query_index::ask(request).await;
            this.update(cx, |this, cx| {
                this.task = None;
                match result {
                    Ok(_) => {
                        if let Some(open) = this.open.upgrade() {
                            open.update(cx, |_, cx| cx.emit(ConflictResolved));
                        }
                    }
                    Err(error) => {
                        this.error =
                            Some(format!("Could not keep the chosen version: {error}").into());
                    }
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    pub fn toggle_compare(&mut self, cx: &mut Context<Self>) {
        self.comparing = !self.comparing;
        cx.notify();
    }
}

impl Render for PageConflict {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let busy = self.is_busy();
        let keep = self.versions.iter().enumerate().map(|(index, version)| {
            Button::new(
                ("keep-version", index),
                format!(
                    "Keep the version from {} ({}){}",
                    version.computer,
                    version.when,
                    if version.deleted { ", deleted" } else { "" }
                ),
            )
            .disabled(busy)
            .on_click(cx.listener(move |this, _, _, cx| this.keep(Some(index), cx)))
        });
        let comparison = self.comparing.then(|| {
            gpui::div()
                .id("competing-versions")
                .flex()
                .flex_col()
                .gap_2()
                .max_h_96()
                .overflow_y_scroll()
                .children(self.versions.iter().map(|version| {
                    gpui::div()
                        .flex()
                        .flex_col()
                        .p_2()
                        .border_1()
                        .border_color(cx.theme().colors().border)
                        .child(
                            Label::new(format!("From {}, {}", version.computer, version.when))
                                .color(ui::Color::Muted),
                        )
                        .children(
                            version
                                .text
                                .lines()
                                .take(COMPARED_LINES)
                                .map(|line| gpui::div().child(line.to_owned())),
                        )
                }))
        });
        gpui::div()
            .p_2()
            .flex()
            .flex_col()
            .gap_1()
            .bg(cx.theme().colors().surface_background)
            .child(Label::new("This page was changed on two computers."))
            .child(
                Label::new(
                    "Both started from the same version, so neither replaced the other. The newer one is shown. Choose what to keep; the others stay in page history.",
                )
                .color(ui::Color::Muted),
            )
            .when_some(self.error.clone(), |div, error| div.child(Label::new(error)))
            .child(
                gpui::div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .children(keep)
                    .child(
                        Button::new("keep-both-versions", "Keep both, one after the other")
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, _, cx| this.keep(None, cx))),
                    )
                    .child(
                        Button::new(
                            "compare-competing-versions",
                            if self.comparing {
                                "Hide versions"
                            } else {
                                "Compare versions"
                            },
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_compare(cx))),
                    ),
            )
            .when_some(comparison, |div, comparison| div.child(comparison))
    }
}
