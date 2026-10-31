//! The AI prompt (reference `src/editor/ai-prompt.ts`): a Zed picker in a block under the text a
//! command will use. It says what that text is, offers the verbs that fit it and earlier prompts,
//! and recalls earlier prompts with Up and Down the way Zed's command palette recalls commands.
//! Enter runs the typed instruction or the chosen row; Escape closes it.

use super::{
    proposal::{self, Invocation},
    settings,
};
use editor::{
    Addon, Editor,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId},
};
use gpui::{
    App, Context, DismissEvent, Entity, Focusable as _, SharedString, Task, WeakEntity, Window,
    prelude::*,
};
use picker::{Direction, Picker, PickerDelegate};
use quire_core::{ai::Verb, fuzzy::fuzzy_filter};
use std::{ops::Range, sync::Arc};
use ui::{
    Color, HighlightedLabel, Label, LabelCommon as _, LabelSize, ListItem, ListItemSpacing,
    Toggleable as _, h_flex, v_flex,
};

/// How many rows the prompt shows before it scrolls.
const VISIBLE_ROWS: usize = 7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetKind {
    Selection,
    Block,
    Blank,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// The typed instruction.
    Run(String),
    /// A verb, run with the typed words as extra instruction.
    Verb(Verb),
    /// An earlier prompt, with the byte offsets the query matched.
    Earlier { index: usize, matched: Vec<usize> },
}

pub struct AiPrompt {
    kind: TargetKind,
    label: String,
    rows_selected: Option<Range<u32>>,
    history: Vec<String>,
    rows: Vec<Row>,
    selected: usize,
    query: String,
    /// The history entry Up or Down recalled, and the text typed before recalling.
    recalled: Option<usize>,
    draft: String,
    chosen: Option<Invocation>,
    host: WeakEntity<Editor>,
}

impl AiPrompt {
    fn verbs(&self) -> Vec<Verb> {
        match self.kind {
            TargetKind::Blank => vec![Verb::Continue],
            TargetKind::Selection | TargetKind::Block => Verb::ALL
                .into_iter()
                .filter(|verb| *verb != Verb::Ask)
                .collect(),
        }
    }

    fn filter(&mut self, query: &str) {
        self.query = query.to_owned();
        let typed = query.trim();
        let earlier = fuzzy_filter(&self.history, typed, |item| item.as_str())
            .into_iter()
            .filter(|(index, _)| self.history[*index] != typed)
            .map(|(index, found)| Row::Earlier {
                matched: char_offsets(&self.history[index], &found.positions),
                index,
            });
        self.rows = (!typed.is_empty())
            .then(|| Row::Run(typed.to_owned()))
            .into_iter()
            .chain(self.verbs().into_iter().map(Row::Verb))
            .chain(earlier)
            .collect();
        self.selected = 0;
    }

    /// What each row shows.
    pub fn row_text(&self, row: &Row) -> String {
        match row {
            Row::Run(text) if self.kind == TargetKind::Blank => format!("Write “{text}” here"),
            Row::Run(text) => format!("Run “{text}”"),
            Row::Verb(verb) if self.query.trim().is_empty() => verb.label().to_owned(),
            Row::Verb(verb) => format!("{}: {}", verb.label(), self.query.trim()),
            Row::Earlier { index, .. } => self.history[*index].clone(),
        }
    }

    #[cfg(test)]
    pub fn rows(&self) -> Vec<String> {
        self.rows.iter().map(|row| self.row_text(row)).collect()
    }

    #[cfg(test)]
    pub fn label(&self) -> &str {
        &self.label
    }
}

fn char_offsets(text: &str, positions: &[usize]) -> Vec<usize> {
    text.char_indices()
        .enumerate()
        .filter(|(index, _)| positions.contains(index))
        .map(|(_, (offset, _))| offset)
        .collect()
}

impl PickerDelegate for AiPrompt {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "quire-ai-prompt"
    }

    fn match_count(&self) -> usize {
        self.rows.len()
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = ix;
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        match self.kind {
            TargetKind::Selection => "How should this change? Such as: make it formal",
            TargetKind::Block => "How should this change? Such as: tighten",
            TargetKind::Blank => "What should go here? Such as: list three questions",
        }
        .into()
    }

    fn select_history(
        &mut self,
        direction: Direction,
        query: &str,
        _: &mut Window,
        _: &mut App,
    ) -> Option<String> {
        match direction {
            Direction::Up if self.selected == 0 || self.recalled.is_some() => {
                let next = self.recalled.map_or(0, |index| index + 1);
                let item = self.history.get(next)?.clone();
                if self.recalled.is_none() {
                    self.draft = query.to_owned();
                }
                self.recalled = Some(next);
                Some(item)
            }
            Direction::Down => match self.recalled? {
                0 => {
                    self.recalled = None;
                    Some(std::mem::take(&mut self.draft))
                }
                index => {
                    self.recalled = Some(index - 1);
                    self.history.get(index - 1).cloned()
                }
            },
            Direction::Up => None,
        }
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        if self
            .recalled
            .is_some_and(|index| self.history.get(index) != Some(&query))
        {
            self.recalled = None;
        }
        self.filter(&query);
        let rows = self.rows.len();
        if let Some(host) = self.host.upgrade() {
            cx.defer(move |cx| host.update(cx, |editor, cx| resize(editor, rows, cx)));
        }
        cx.notify();
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let typed = self.query.trim().to_owned();
        let (args, verb, remembered) = match row {
            Row::Run(text) => (text.clone(), None, Some(text.clone())),
            Row::Verb(verb) => (typed, Some(*verb), None),
            Row::Earlier { index, .. } => {
                let text = self.history[*index].clone();
                (text.clone(), None, Some(text))
            }
        };
        if let Some(text) = remembered {
            settings::remember(&text, cx);
        }
        self.chosen = Some(Invocation {
            args,
            rows: self.rows_selected.clone(),
            verb,
        });
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn render_header(
        &self,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<gpui::AnyElement> {
        Some(
            v_flex()
                .px_2()
                .child(
                    Label::new(SharedString::from(self.label.clone()))
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element(),
        )
    }

    fn render_footer(
        &self,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<gpui::AnyElement> {
        Some(
            h_flex()
                .px_2()
                .child(
                    Label::new("Enter runs · ↑ ↓ earlier prompts · Escape closes")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .into_any_element(),
        )
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let row = self.rows.get(ix)?;
        let item = ListItem::new(ix)
            .spacing(ListItemSpacing::Sparse)
            .inset(true)
            .toggle_state(selected);
        Some(match row {
            Row::Earlier { index, matched } => item.child(
                h_flex()
                    .gap_2()
                    .child(HighlightedLabel::new(
                        self.history[*index].clone(),
                        matched.clone(),
                    ))
                    .child(
                        Label::new("earlier")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
            ),
            _ => item.child(Label::new(self.row_text(row))),
        })
    }
}

/// The prompt an editor shows, and its block.
struct Shown {
    picker: Entity<Picker<AiPrompt>>,
    block: CustomBlockId,
    _subscriptions: Vec<gpui::Subscription>,
}

impl Addon for Shown {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The prompt shown in `editor`, for tests.
#[cfg(test)]
pub fn current(editor: &Editor) -> Option<Entity<Picker<AiPrompt>>> {
    editor.addon::<Shown>().map(|shown| shown.picker.clone())
}

fn block_rows(rows: usize) -> u32 {
    3 + rows.clamp(1, VISIBLE_ROWS) as u32
}

fn resize(editor: &mut Editor, rows: usize, cx: &mut Context<Editor>) {
    if let Some(block) = editor.addon::<Shown>().map(|shown| shown.block) {
        editor.resize_blocks([(block, block_rows(rows))].into_iter().collect(), None, cx);
    }
}

fn close(editor: &mut Editor, cx: &mut Context<Editor>) -> Option<Option<Invocation>> {
    let shown = editor.addon::<Shown>()?;
    let block = shown.block;
    let chosen = shown
        .picker
        .update(cx, |picker, _| picker.delegate.chosen.take());
    editor.remove_blocks([block].into_iter().collect(), None, cx);
    editor.unregister_addon::<Shown>();
    cx.notify();
    Some(chosen)
}

/// Opens the prompt under the text the next command will use. `rows` are the lines of a Visual
/// selection, recorded before Visual mode ends.
pub fn open(editor: &Entity<Editor>, rows: Option<Range<u32>>, window: &mut Window, cx: &mut App) {
    if let Err(message) = editor.update(cx, |editor, cx| show(editor, rows, window, cx)) {
        crate::outline::show_notice(message, cx);
    }
}

fn show(
    editor: &mut Editor,
    rows: Option<Range<u32>>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> Result<(), String> {
    if !settings::is_on(cx) {
        return Err(if settings::is_tested(cx) {
            quire_core::ai::backend::AI_OFF.into()
        } else {
            settings::UNTESTED.into()
        });
    }
    if editor.read_only(cx) {
        return Err(proposal::READ_ONLY.into());
    }
    let target = proposal::prompt_target(editor, rows, cx).ok_or(proposal::NO_RANGE)?;
    close(editor, cx);
    let host = cx.entity().downgrade();
    let mut delegate = AiPrompt {
        kind: target.kind,
        label: target.label,
        rows_selected: target.rows,
        history: settings::history(cx),
        rows: Vec::new(),
        selected: 0,
        query: String::new(),
        recalled: None,
        draft: String::new(),
        chosen: None,
        host,
    };
    delegate.filter("");
    let count = delegate.rows.len();
    let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx).embedded());
    let block = editor.insert_blocks(
        [BlockProperties {
            placement: BlockPlacement::Below(target.at),
            height: Some(block_rows(count)),
            style: BlockStyle::Flex,
            render: Arc::new({
                let picker = picker.clone();
                move |block| {
                    gpui::div()
                        .key_context("QuireControl")
                        .ml(block.anchor_x)
                        .w_full()
                        .max_w(gpui::rems(40.))
                        .child(picker.clone())
                        .into_any_element()
                }
            }),
            priority: 0,
        }],
        None,
        cx,
    )[0];
    let dismissed = cx.subscribe_in(
        &picker,
        window,
        |editor, _, _: &DismissEvent, window, cx| {
            let Some(chosen) = close(editor, cx) else {
                return;
            };
            window.focus(&editor.focus_handle(cx), cx);
            if let Some(invocation) = chosen {
                proposal::start_in(editor, invocation, window, cx);
            }
        },
    );
    let blurred = cx.on_focus_out(&picker.focus_handle(cx), window, |editor, _, window, cx| {
        let focused_inside = editor
            .addon::<Shown>()
            .is_some_and(|shown| shown.picker.focus_handle(cx).contains_focused(window, cx));
        if !focused_inside {
            close(editor, cx);
        }
    });
    editor.register_addon(Shown {
        picker: picker.clone(),
        block,
        _subscriptions: vec![dismissed, blurred],
    });
    picker.update(cx, |picker, cx| picker.focus(window, cx));
    cx.notify();
    Ok(())
}

#[cfg(test)]
#[path = "prompt_tests.rs"]
mod tests;
