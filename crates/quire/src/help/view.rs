//! The Help and shortcuts window: searchable topics, then the generated shortcut reference.

use super::{Shortcut, content::SECTIONS, shortcuts};
use editor::{Editor, EditorEvent};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, ScrollHandle, Window, WindowOptions, prelude::*,
    px,
};
use theme::ActiveTheme as _;
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, Label, LabelCommon as _, LabelSize, h_flex,
    v_flex,
};

pub const CONTEXT: &str = "QuireHelp";
pub const FIELD_CONTEXT: &str = "QuireHelp > Editor";

pub struct HelpWindow {
    focus: FocusHandle,
    search: Entity<Editor>,
    scroll: ScrollHandle,
    shortcuts: Vec<Shortcut>,
    /// Shown beside a notebook's pages rather than in its own window.
    pane: bool,
}

/// Help in a notebook window asks to be closed.
pub struct CloseHelp;

impl gpui::EventEmitter<CloseHelp> for HelpWindow {}

/// Opens Help, or brings the open Help window forward.
pub fn open(cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<HelpWindow>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(px(760.), px(820.)),
            cx,
        ))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_window_title("Help & shortcuts");
        cx.new(|cx| HelpWindow::new(false, window, cx))
    });
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Cannot open Help: {error}. Check desktop access, then retry."),
            cx,
        );
    }
}

impl HelpWindow {
    /// Help in its own window, or, with `pane`, beside a notebook's pages as in the reference app.
    pub fn new(pane: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text("Find a shortcut or learn a feature…", window, cx);
            editor
        });
        cx.subscribe(&search, |_, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                cx.notify();
            }
        })
        .detach();
        if !pane {
            crate::preferences::follow_system_appearance(window, cx);
        }
        window.focus(&search.focus_handle(cx), cx);
        Self {
            focus: cx.focus_handle(),
            search,
            scroll: ScrollHandle::new(),
            shortcuts: shortcuts(cx),
            pane,
        }
    }

    pub fn search_focus(&self, cx: &App) -> FocusHandle {
        self.search.focus_handle(cx)
    }

    pub fn needle(&self, cx: &App) -> String {
        self.search.read(cx).text(cx).trim().to_lowercase()
    }

    #[cfg(test)]
    pub fn search(&self) -> &Entity<Editor> {
        &self.search
    }

    /// The topics whose text contains the search, in guide order.
    pub fn topics(&self, cx: &App) -> Vec<&'static str> {
        let needle = self.needle(cx);
        SECTIONS
            .iter()
            .filter(|section| {
                [
                    section.title,
                    section.description,
                    section.example.unwrap_or_default(),
                ]
                .into_iter()
                .chain(section.steps.iter().copied())
                .any(|text| text.to_lowercase().contains(&needle))
            })
            .map(|section| section.id)
            .collect()
    }

    /// The shortcut rows that match the search.
    pub fn rows(&self, cx: &App) -> Vec<&Shortcut> {
        let needle = self.needle(cx);
        self.shortcuts
            .iter()
            .filter(|shortcut| shortcut.matches(&needle))
            .collect()
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane {
            cx.emit(CloseHelp);
        } else {
            window.remove_window();
        }
    }

    /// Opens Tasks with its capture field, in this notebook window or the active profile's.
    pub fn capture_task(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let action = Box::new(crate::navigation::CaptureTask);
        if self.pane {
            window.dispatch_action(action, cx);
        } else if let Err(error) = crate::editor_host::dispatch_to_notebook(action, cx) {
            crate::outline::show_notice(error, cx);
        }
    }

    fn show_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search
            .update(cx, |editor, cx| editor.clear(window, cx));
    }
}

impl Focusable for HelpWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn muted(text: impl Into<gpui::SharedString>) -> Label {
    Label::new(text).size(LabelSize::Small).color(Color::Muted)
}

impl Render for HelpWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let topics = self.topics(cx);
        let rows = self.rows(cx);
        let empty = topics.is_empty() && rows.is_empty();
        let colors = cx.theme().colors();
        let nav = h_flex()
            .flex_wrap()
            .gap_1()
            .children(SECTIONS.iter().enumerate().map(|(index, section)| {
                let scroll = self.scroll.clone();
                Button::new(section.id, section.title).on_click(cx.listener(
                    move |this, _, window, cx| {
                        this.show_all(window, cx);
                        scroll.scroll_to_item(index);
                    },
                ))
            }));
        let sections = SECTIONS
            .iter()
            .filter(|section| topics.contains(&section.id))
            .map(|section| {
                v_flex()
                    .gap_1()
                    .pt_3()
                    .child(Label::new(section.title).size(LabelSize::Large))
                    .child(Label::new(section.description))
                    .children(section.steps.iter().enumerate().map(|(index, step)| {
                        h_flex()
                            .items_start()
                            .gap_2()
                            .child(muted(format!("{}.", index + 1)))
                            .child(gpui::div().flex_1().child(*step))
                    }))
                    .when_some(section.example, |this, example| {
                        this.child(
                            gpui::div()
                                .p_2()
                                .rounded_sm()
                                .bg(colors.editor_background)
                                .font_family("Lilex")
                                .whitespace_normal()
                                .children(
                                    example
                                        .lines()
                                        .map(|line| gpui::div().child(line.to_owned())),
                                ),
                        )
                    })
            });
        let table = v_flex()
            .pt_3()
            .gap_1()
            .child(Label::new("Keyboard shortcuts").size(LabelSize::Large))
            .child(muted(
                "Generated from the keys bound in this copy of Quire, as this platform shows them.",
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(gpui::div().w(px(180.)).child(muted("Shortcut")))
                    .child(gpui::div().flex_1().child(muted("Action")))
                    .child(gpui::div().w(px(170.)).child(muted("Where"))),
            )
            .children(rows.iter().map(|row| {
                h_flex()
                    .gap_2()
                    .child(gpui::div().w(px(180.)).child(row.keys.clone()))
                    .child(gpui::div().flex_1().child(row.action.clone()))
                    .child(gpui::div().w(px(170.)).child(muted(row.scope.clone())))
            }));
        v_flex()
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::cancel))
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .child(
                v_flex()
                    .p_4()
                    .gap_2()
                    .child(
                        h_flex()
                            .justify_between()
                            .child(Label::new("Make yourself at home.").size(LabelSize::Large))
                            .when(self.pane, |this| {
                                this.child(
                                    Button::new("close-help", "Close")
                                        .tooltip(ui::Tooltip::text("Close help (Escape)"))
                                        .on_click(cx.listener(|_, _, _, cx| cx.emit(CloseHelp))),
                                )
                            }),
                    )
                    .child(muted(
                        "A practical guide to writing, linking and getting things done in Quire. Available offline.",
                    ))
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                gpui::div()
                                    .flex_1()
                                    .px_2()
                                    .py_1()
                                    .border_1()
                                    .border_color(colors.border)
                                    .rounded_sm()
                                    .child(self.search.clone()),
                            )
                            .child(
                                Button::new("capture-task", "Capture a task")
                                    .style(ui::ButtonStyle::Filled)
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.capture_task(window, cx)
                                    })),
                            ),
                    )
                    .child(nav)
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("open-readme", "README.md rendering sample")
                                    .on_click(|_, _, cx| crate::showcase::open(cx)),
                            )
                            .child(
                                Button::new("open-settings", "Settings")
                                    .on_click(|_, _, cx| crate::preferences::view::open(cx)),
                            ),
                    )
                    .child(muted(
                        "Keys match this platform. Editor shortcuts need focus in a note; notebook shortcuts work outside dialogs.",
                    )),
            )
            .child(
                v_flex()
                    .id("help-topics")
                    .flex_1()
                    .min_h_0()
                    .px_4()
                    .pb_4()
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll)
                    .children(sections)
                    .when(!rows.is_empty(), |this| this.child(table))
                    .when(empty, |this| {
                        this.child(
                            v_flex()
                                .pt_3()
                                .gap_1()
                                .child(Label::new("No matching help."))
                                .child(muted(
                                    "Try \u{201c}task\u{201d}, \u{201c}Vim\u{201d}, \u{201c}embed\u{201d} or a shortcut such as \u{201c}Enter\u{201d}.",
                                ))
                                .child(Button::new("show-all", "Show all help").on_click(
                                    cx.listener(|this, _, window, cx| this.show_all(window, cx)),
                                )),
                        )
                    }),
            )
            .child(
                h_flex()
                    .px_4()
                    .py_1()
                    .justify_between()
                    .child(muted("Help & shortcuts"))
                    .child(muted("F1 opens this guide")),
            )
    }
}
