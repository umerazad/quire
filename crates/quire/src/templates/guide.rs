//! How templates work, shown beside the Templates page outside its text, so the examples never
//! appear in the `jj` list. Each example can be added to the page with one click; examples the
//! page already defines show Added.

use gpui::{Context, Entity, Render, Subscription, Window};
use language::Buffer;
use quire_core::templates::guide::{EXAMPLES, Example, PLACEHOLDERS, RULES, definition};
use std::collections::HashSet;
use theme::ActiveTheme;
use ui::{Button, Color, Label, LabelCommon, LabelSize, prelude::*};

pub struct TemplatesGuide {
    path: String,
    buffer: Entity<Buffer>,
    _observe: Subscription,
}

impl TemplatesGuide {
    pub fn new(path: String, buffer: Entity<Buffer>, cx: &mut Context<Self>) -> Self {
        Self {
            _observe: cx.observe(&buffer, |_, _, cx| cx.notify()),
            path,
            buffer,
        }
    }

    pub fn shows(&self, path: &str, buffer: &Entity<Buffer>) -> bool {
        self.path == path && self.buffer == *buffer
    }

    /// The template names the page defines now, lowercased.
    pub fn defined(&self, cx: &App) -> HashSet<String> {
        let snapshot = self.buffer.read(cx).snapshot();
        super::definitions(&self.path, &snapshot)
            .unwrap_or_default()
            .into_iter()
            .map(|item| item.name.to_lowercase())
            .collect()
    }

    pub fn add(&self, example: &Example, cx: &mut App) {
        super::append(&self.buffer, &definition(example), cx);
    }

    fn example(
        &self,
        index: usize,
        example: &'static Example,
        added: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        v_flex()
            .p_2()
            .gap_1()
            .rounded_sm()
            .border_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .justify_between()
                    .child(Label::new(example.name))
                    .child(
                        div()
                            .debug_selector(move || format!("quire-add-template-{}", example.name))
                            .child(
                                Button::new(
                                    ("add-template", index),
                                    if added { "Added" } else { "Add" },
                                )
                                .disabled(added)
                                .tooltip(ui::Tooltip::text(if added {
                                    format!("{} is in your templates", example.name)
                                } else {
                                    format!("Add {} to your templates", example.name)
                                }))
                                .on_click(cx.listener(move |this, _, _, cx| this.add(example, cx))),
                            ),
                    ),
            )
            .child(
                Label::new(example.usage)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                div()
                    .p_1()
                    .bg(colors.editor_background)
                    .font_buffer(cx)
                    .text_xs()
                    .children(
                        example
                            .body
                            .split('\n')
                            .map(|line| div().child(line.to_owned())),
                    ),
            )
    }
}

impl Render for TemplatesGuide {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let defined = self.defined(cx);
        let examples: Vec<_> = EXAMPLES
            .iter()
            .enumerate()
            .map(|(index, example)| {
                let added = defined.contains(&example.name.to_lowercase());
                self.example(index, example, added, cx).into_any_element()
            })
            .collect();
        v_flex()
            .id("templates-guide")
            .w(px(360.))
            .h_full()
            .flex_none()
            .overflow_y_scroll()
            .p_2()
            .gap_2()
            .border_l_1()
            .border_color(cx.theme().colors().border)
            .child(Label::new("How templates work"))
            .children(
                RULES
                    .iter()
                    .map(|rule| Label::new(*rule).size(LabelSize::Small)),
            )
            .child(Label::new("Placeholders").color(Color::Muted))
            .children(PLACEHOLDERS.iter().map(|item| {
                h_flex()
                    .gap_2()
                    .child(div().font_buffer(cx).text_xs().child(item.token))
                    .child(
                        Label::new(item.meaning)
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    )
            }))
            .child(Label::new("Examples").color(Color::Muted))
            .children(examples)
    }
}
