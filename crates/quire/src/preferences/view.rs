//! The Settings window: the reference app's sections, controls, and messages, writing each change
//! to `settings.json` through the settings store.

use super::{Change, Preferences, restore_defaults, settings_error, update};
use crate::tab_order::{self, TabOrder};
use editor::{Editor, EditorEvent};
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Window, WindowOptions, prelude::*, px, relative,
};
use settings::{CaptureTarget, SettingsStore, ThemeAppearanceMode};
use theme::ActiveTheme as _;
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, Label, LabelCommon as _, LabelSize,
    SwitchField, ToggleButtonGroup, ToggleButtonSimple, ToggleState, h_flex, v_flex,
};

pub const CONTEXT: &str = "QuirePreferences";
pub const FIELD_CONTEXT: &str = "QuirePreferences > Editor";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Field {
    FontFamily,
    FontSize,
    LineSpacing,
}

pub struct PreferencesWindow {
    focus: FocusHandle,
    shown: Preferences,
    font_family: Entity<Editor>,
    font_size: Entity<Editor>,
    line_spacing: Entity<Editor>,
    pub ai: Entity<crate::ai::settings::AiSection>,
    pub data: Entity<super::data::DataSection>,
}

/// Opens Settings, or brings the open Settings window forward.
pub fn open(cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<PreferencesWindow>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(px(560.), px(760.)),
            cx,
        ))),
        ..Default::default()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_window_title("Settings");
        cx.new(|cx| PreferencesWindow::new(window, cx))
    });
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Cannot open Settings: {error}. Check desktop access, then retry."),
            cx,
        );
    }
}

fn text_field(text: &str, window: &mut Window, cx: &mut App) -> Entity<Editor> {
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_text(text, window, cx);
        editor
    })
}

fn number(value: f32) -> String {
    let text = format!("{value:.2}");
    text.trim_end_matches('0').trim_end_matches('.').to_owned()
}

impl PreferencesWindow {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shown = Preferences::current(cx);
        let font_family = text_field(&shown.font_family, window, cx);
        let font_size = text_field(&number(shown.font_size), window, cx);
        let line_spacing = text_field(&number(shown.line_spacing), window, cx);
        for (editor, field) in [
            (&font_family, Field::FontFamily),
            (&font_size, Field::FontSize),
            (&line_spacing, Field::LineSpacing),
        ] {
            cx.subscribe_in(
                editor,
                window,
                move |this, _, event: &EditorEvent, window, cx| {
                    if matches!(event, EditorEvent::Blurred) {
                        this.apply(field, window, cx);
                    }
                },
            )
            .detach();
        }
        cx.observe_global_in::<SettingsStore>(window, |this, window, cx| this.refresh(window, cx))
            .detach();
        cx.observe_global::<super::SettingsError>(|_, cx| cx.notify())
            .detach();
        super::follow_system_appearance(window, cx);
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let ai = cx.new(|cx| crate::ai::settings::AiSection::new(window, cx));
        let data = cx.new(|cx| super::data::DataSection::new(window, cx));
        Self {
            focus,
            shown,
            font_family,
            font_size,
            line_spacing,
            ai,
            data,
        }
    }

    pub fn editor(&self, field: Field) -> &Entity<Editor> {
        match field {
            Field::FontFamily => &self.font_family,
            Field::FontSize => &self.font_size,
            Field::LineSpacing => &self.line_spacing,
        }
    }

    fn shown_text(&self, field: Field) -> String {
        match field {
            Field::FontFamily => self.shown.font_family.clone(),
            Field::FontSize => number(self.shown.font_size),
            Field::LineSpacing => number(self.shown.line_spacing),
        }
    }

    /// Shows values saved elsewhere, such as a hand edit of the settings file.
    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = Preferences::current(cx);
        if current == self.shown {
            return;
        }
        self.shown = current;
        for field in [Field::FontFamily, Field::FontSize, Field::LineSpacing] {
            let text = self.shown_text(field);
            self.editor(field).clone().update(cx, |editor, cx| {
                if editor.text(cx) != text {
                    editor.set_text(text, window, cx);
                }
            });
        }
        cx.notify();
    }

    /// Saves a typed value. Numbers that cannot be used return to the saved value, beside the
    /// reason, as in the reference Settings.
    pub fn apply(&mut self, field: Field, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.editor(field).read(cx).text(cx);
        if typed.trim() == self.shown_text(field) {
            return;
        }
        let parse = |text: &str| text.trim().parse::<f32>().unwrap_or(f32::NAN);
        let change = match field {
            Field::FontFamily => Change::FontFamily(typed),
            Field::FontSize => Change::FontSize(parse(&typed)),
            Field::LineSpacing => Change::LineSpacing(parse(&typed)),
        };
        if change.validate().is_err() && field != Field::FontFamily {
            let text = self.shown_text(field);
            self.editor(field)
                .clone()
                .update(cx, |editor, cx| editor.set_text(text, window, cx));
        }
        super::change(change, cx);
        cx.notify();
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        for field in [Field::FontFamily, Field::FontSize, Field::LineSpacing] {
            if self.editor(field).focus_handle(cx).is_focused(window) {
                self.apply(field, window, cx);
            }
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, window: &mut Window, _: &mut Context<Self>) {
        window.remove_window();
    }

    fn section(title: &'static str) -> gpui::Div {
        v_flex()
            .gap_2()
            .pt_3()
            .child(Label::new(title).size(LabelSize::Large))
    }

    fn hint(text: impl Into<gpui::SharedString>) -> Label {
        Label::new(text).size(LabelSize::Small).color(Color::Muted)
    }

    fn field(
        &self,
        label: &'static str,
        field: Field,
        order: &mut TabOrder,
        window: &Window,
        cx: &App,
    ) -> gpui::Div {
        v_flex()
            .gap_1()
            .flex_1()
            .child(Label::new(label))
            .child(tab_order::field(self.editor(field), order, window, cx))
    }

    fn appearance(&self, order: &mut TabOrder) -> impl IntoElement {
        let choices = [
            ("System", ThemeAppearanceMode::System),
            ("Light", ThemeAppearanceMode::Light),
            ("Dark", ThemeAppearanceMode::Dark),
        ];
        let selected = choices
            .iter()
            .position(|(_, mode)| *mode == self.shown.appearance)
            .unwrap_or_default();
        let buttons = choices.map(|(label, mode)| {
            ToggleButtonSimple::new(label, move |_, _, cx| {
                super::change(Change::Appearance(mode), cx)
            })
        });
        Self::section("Appearance")
            .child(ToggleButtonGroup::single_row("appearance", buttons)
                    .selected_index(selected)
                    .tab_index(order.counter()))
            .child(Self::hint(
                "System follows your computer's light or dark setting as it changes. The Light / Dark button in the sidebar switches between light and dark.",
            ))
    }

    fn capture(&self, order: &mut TabOrder) -> impl IntoElement {
        let selected = match self.shown.capture_target {
            CaptureTarget::Inbox => 0,
            CaptureTarget::Today => 1,
        };
        let buttons = [
            ("Inbox page", CaptureTarget::Inbox),
            ("Today's daily page", CaptureTarget::Today),
        ]
        .map(|(label, target)| {
            ToggleButtonSimple::new(label, move |_, _, cx| {
                super::change(Change::CaptureTarget(target), cx)
            })
        });
        Self::section("Tasks")
            .child(Label::new("Quick capture goes to"))
            .child(ToggleButtonGroup::single_row("capture-target", buttons)
                    .selected_index(selected)
                    .tab_index(order.counter()))
            .child(Self::hint(
                "Applies to the capture field on Tasks. A task captured to today's page needs no ➕ date.",
            ))
    }

    fn typography(&self, order: &mut TabOrder, window: &Window, cx: &App) -> impl IntoElement {
        let preview = self.shown.clone();
        Self::section("Typography")
            .child(self.field("Font family", Field::FontFamily, order, window, cx))
            .child(Self::hint(
                "Use an installed font. Press Enter or leave the field to apply. Unavailable fonts use a monospace fallback.",
            ))
            .child(
                h_flex()
                    .gap_3()
                    .child(self.field("Font size (px)", Field::FontSize, order, window, cx))
                    .child(self.field("Line spacing", Field::LineSpacing, order, window, cx)),
            )
            .child(
                v_flex()
                    .id("typography-preview")
                    .p_3()
                    .border_1()
                    .rounded_md()
                    .font_family(preview.font_family)
                    .text_size(px(preview.font_size))
                    .line_height(relative(preview.line_spacing))
                    .child("A place for thoughts.")
                    .child("Keep the ideas that matter. #thinking"),
            )
    }
}

impl Focusable for PreferencesWindow {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PreferencesWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown.clone();
        let mut order = TabOrder::default();
        let root = v_flex()
            .id("settings")
            .key_context(CONTEXT)
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel));
        tab_order::handle(root, cx)
            .size_full()
            .overflow_y_scroll()
            .p_4()
            .gap_1()
            .bg(cx.theme().colors().background)
            .text_color(cx.theme().colors().text)
            .child(Self::hint("MAKE YOURSELF AT HOME"))
            .child(Label::new("Settings").size(LabelSize::Large))
            .child(Self::hint(
                "Changes apply to every editor and are saved on this computer.",
            ))
            .child(self.appearance(&mut order))
            .child(
                Self::section("Editing")
                    .child(SwitchField::new(
                        "vim",
                        Some("Vim bindings"),
                        Some("Normal, Insert, and Visual modes. Press Escape to return to Normal.".into()),
                        ToggleState::from(shown.vim),
                        |state, _, cx| super::change(Change::Vim(*state == ToggleState::Selected), cx),
                    ).tab_index(order.next()))
                    .child(SwitchField::new(
                        "format-tags",
                        Some("Format tags"),
                        Some("Show #tags as soft labels in Write and rendered references.".into()),
                        ToggleState::from(shown.format_tags),
                        |state, _, cx| {
                            super::change(Change::FormatTags(*state == ToggleState::Selected), cx)
                        },
                    ).tab_index(order.next())),
            )
            .child(self.capture(&mut order))
            .child(self.typography(&mut order, window, cx))
            .child(
                Self::section("Notebook").child(Self::hint(
                    "Daily notes: set the folder and title pattern in the Daily notes browser. The layout is saved in the notebook, so every device uses it.",
                )),
            )
            .child(order.group().child(self.data.clone()))
            .child(order.group().child(self.ai.clone()))
            .when_some(settings_error(cx), |this, error| {
                this.child(Label::new(error).color(Color::Error))
            })
            .child(
                h_flex()
                    .pt_3()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("restore-defaults", "Restore defaults")
                            .tab_index(order.next())
                            .on_click(|_, _, cx| update(restore_defaults, cx)),
                    )
                    .child(
                        Button::new("done", "Done")
                            .tab_index(order.next())
                            .on_click(|_, window, _| window.remove_window()),
                    ),
            )
    }
}
