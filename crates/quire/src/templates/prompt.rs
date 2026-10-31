//! The questions a template asks while it expands, as Zed pickers. `{ask: question}` takes the
//! typed text; `{pick: #tag}` lists names used before with the tag, newest first with how often,
//! narrowed by fuzzy match as you type, with a Create row for a name not on the list. Enter
//! answers, Escape dismisses, which cancels the insertion.

use futures::channel::oneshot;
use gpui::{App, Context, DismissEvent, SharedString, Task, Window};
use picker::{Picker, PickerDelegate};
use quire_core::{fuzzy::fuzzy_filter, templates::PastName};
use std::sync::Arc;
use ui::{
    HighlightedLabel, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, Toggleable,
    prelude::*,
};

/// How many past names one pick shows.
const SHOWN: usize = 50;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Prompt {
    Ask { question: String },
    Pick { tag: String, names: Vec<PastName> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Row {
    /// The typed answer to a question.
    Answer(String),
    /// A past name, with the byte offsets the query matched.
    Name { name: usize, matched: Vec<usize> },
    /// A typed name not on the list.
    Create(String),
}

pub struct TemplatePrompt {
    prompt: Prompt,
    rows: Vec<Row>,
    selected: usize,
    answer: Option<oneshot::Sender<Option<String>>>,
}

impl TemplatePrompt {
    pub fn new(prompt: Prompt, answer: oneshot::Sender<Option<String>>) -> Self {
        let mut this = Self {
            prompt,
            rows: Vec::new(),
            selected: 0,
            answer: Some(answer),
        };
        this.filter("");
        this
    }

    #[cfg(test)]
    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// What each row shows, for tests and accessibility labels.
    pub fn row_text(&self, row: &Row) -> String {
        match (row, &self.prompt) {
            (Row::Answer(answer), _) if answer.is_empty() => "Insert an empty answer".into(),
            (Row::Answer(answer), _) => format!("Insert “{answer}”"),
            (Row::Create(name), _) => format!("Create “{name}”"),
            (Row::Name { name, .. }, Prompt::Pick { names, .. }) => {
                let name = &names[*name];
                let detail = [
                    (name.uses > 1).then(|| format!("{} times", name.uses)),
                    (!name.last.is_empty()).then(|| name.last.clone()),
                ]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
                if detail.is_empty() {
                    name.name.clone()
                } else {
                    format!("{} · {detail}", name.name)
                }
            }
            (Row::Name { .. }, Prompt::Ask { .. }) => String::new(),
        }
    }

    fn label(&self) -> &str {
        match &self.prompt {
            Prompt::Ask { question } => question,
            Prompt::Pick { tag, .. } => tag.strip_prefix('#').unwrap_or(tag),
        }
    }

    fn filter(&mut self, query: &str) {
        self.selected = 0;
        self.rows = match &self.prompt {
            Prompt::Ask { .. } => vec![Row::Answer(query.to_owned())],
            Prompt::Pick { names, .. } => {
                let query = query.trim();
                let mut rows: Vec<Row> = fuzzy_filter(names, query, |item| &item.name)
                    .into_iter()
                    .take(SHOWN)
                    .map(|(name, found)| Row::Name {
                        matched: char_offsets(&names[name].name, &found.positions),
                        name,
                    })
                    .collect();
                let exact = names
                    .iter()
                    .any(|item| item.name.to_lowercase() == query.to_lowercase());
                if !query.is_empty() && !exact {
                    rows.push(Row::Create(query.to_owned()));
                }
                rows
            }
        };
    }

    fn finish(&mut self, answer: Option<String>) {
        if let Some(sender) = self.answer.take() {
            sender.send(answer).ok();
        }
    }
}

fn char_offsets(text: &str, positions: &[usize]) -> Vec<usize> {
    text.char_indices()
        .enumerate()
        .filter(|(index, _)| positions.contains(index))
        .map(|(_, (offset, _))| offset)
        .collect()
}

impl PickerDelegate for TemplatePrompt {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "quire-template-prompt"
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
        match &self.prompt {
            Prompt::Ask { question } => question.as_str().into(),
            Prompt::Pick { names, .. } if names.is_empty() => {
                format!("Name this {}", self.label()).into()
            }
            Prompt::Pick { .. } => format!("Find a {}, or type a new name", self.label()).into(),
        }
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        Some(format!("No {} names yet. Type one to create it.", self.label()).into())
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        self.filter(&query);
        cx.notify();
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(row) = self.rows.get(self.selected) else {
            return;
        };
        let answer = match (row, &self.prompt) {
            (Row::Answer(text) | Row::Create(text), _) => text.clone(),
            (Row::Name { name, .. }, Prompt::Pick { names, .. }) => names[*name].name.clone(),
            (Row::Name { .. }, Prompt::Ask { .. }) => return,
        };
        self.finish(Some(answer));
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.finish(None);
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
        Some(match (row, &self.prompt) {
            (Row::Name { name, matched }, Prompt::Pick { names, .. }) => {
                let name = &names[*name];
                let detail = self.row_text(row);
                let detail = detail
                    .strip_prefix(&name.name)
                    .and_then(|rest| rest.strip_prefix(" · "))
                    .map(str::to_owned);
                item.child(
                    h_flex()
                        .gap_2()
                        .child(HighlightedLabel::new(name.name.clone(), matched.clone()))
                        .when_some(detail, |row, detail| {
                            row.child(
                                Label::new(detail)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                        }),
                )
            }
            _ => item.child(Label::new(self.row_text(row))),
        })
    }
}

/// Opens a prompt picker. The answer arrives on the returned receiver; dismissing sends None.
pub fn picker(
    prompt: Prompt,
    window: &mut Window,
    cx: &mut App,
) -> (
    gpui::Entity<Picker<TemplatePrompt>>,
    oneshot::Receiver<Option<String>>,
) {
    let (send, receive) = oneshot::channel();
    let picker = cx.new(|cx| {
        Picker::uniform_list(TemplatePrompt::new(prompt, send), window, cx).reopenable(false, cx)
    });
    (picker, receive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<PastName> {
        [
            ("Team Sync", 4, "2026-10-29"),
            ("Ops review", 1, "2026-10-20"),
            ("Design crit", 2, ""),
        ]
        .into_iter()
        .map(|(name, uses, last)| PastName {
            name: name.into(),
            uses,
            last: last.into(),
        })
        .collect()
    }

    fn texts(prompt: &TemplatePrompt) -> Vec<String> {
        prompt
            .rows()
            .iter()
            .map(|row| prompt.row_text(row))
            .collect()
    }

    #[test]
    fn picks_list_past_names_narrow_by_fuzzy_match_and_offer_create() {
        let (send, _receive) = oneshot::channel();
        let mut prompt = TemplatePrompt::new(
            Prompt::Pick {
                tag: "#meeting".into(),
                names: names(),
            },
            send,
        );
        assert_eq!(
            texts(&prompt),
            [
                "Team Sync · 4 times · 2026-10-29",
                "Ops review · 2026-10-20",
                "Design crit · 2 times"
            ]
        );
        prompt.filter("dcr");
        assert_eq!(texts(&prompt), ["Design crit · 2 times", "Create “dcr”"]);
        assert_eq!(
            prompt.rows()[0],
            Row::Name {
                name: 2,
                matched: vec![0, 7, 8]
            }
        );
        prompt.filter("team sync");
        assert_eq!(texts(&prompt), ["Team Sync · 4 times · 2026-10-29"]);
        prompt.filter("Board prep");
        assert_eq!(texts(&prompt), ["Create “Board prep”"]);
    }

    #[test]
    fn questions_offer_the_typed_answer() {
        let (send, _receive) = oneshot::channel();
        let mut prompt = TemplatePrompt::new(
            Prompt::Ask {
                question: "Who?".into(),
            },
            send,
        );
        assert_eq!(texts(&prompt), ["Insert an empty answer"]);
        prompt.filter("Ana");
        assert_eq!(texts(&prompt), ["Insert “Ana”"]);
    }
}
