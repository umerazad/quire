//! Blocks shown with their surroundings, as the tag page and linked references show them:
//! grouped under a heading per page, each with its parents above and nested blocks below,
//! laid out as single-line rows for a virtualized list.

use quire_store::index::Outline;
use ui::{
    Color, Icon, IconName, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, Toggleable,
    prelude::*,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextRow {
    Page {
        page: String,
        title: String,
        matches: usize,
    },
    Parents {
        page: String,
        text: String,
    },
    Block {
        page: String,
        text: String,
    },
    Child {
        page: String,
        depth: u32,
        text: String,
    },
    More {
        page: String,
        count: usize,
    },
}

/// One block to lay out: the page identity and title it belongs to, its text, and where it
/// sits.
pub struct ContextItem<'a> {
    pub page: String,
    pub title: &'a str,
    pub text: &'a str,
    pub outline: &'a Outline,
}

impl ContextRow {
    pub fn page(&self) -> &str {
        match self {
            Self::Page { page, .. }
            | Self::Parents { page, .. }
            | Self::Block { page, .. }
            | Self::Child { page, .. }
            | Self::More { page, .. } => page,
        }
    }

    /// The row as a list item; `noun` names one and several matches in page headings.
    pub fn item(&self, index: usize, highlighted: bool, noun: (&str, &str)) -> ListItem {
        let base = ListItem::new(("context-row", index))
            .spacing(ListItemSpacing::Dense)
            .toggle_state(highlighted);
        match self {
            Self::Page { title, matches, .. } => base
                .start_slot(Icon::new(IconName::File).color(Color::Muted))
                .child(
                    h_flex()
                        .gap_2()
                        .child(Label::new(title.clone()).truncate())
                        .child(
                            Label::new(match matches {
                                1 => format!("1 {}", noun.0),
                                count => format!("{count} {}", noun.1),
                            })
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                        ),
                ),
            Self::Parents { text, .. } => base.indent_level(1).child(
                Label::new(format!("{text} ›"))
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            ),
            Self::Block { text, .. } => base
                .indent_level(1)
                .child(Label::new(format!("• {text}")).truncate()),
            Self::Child { depth, text, .. } => {
                base.indent_level(1 + (*depth as usize).min(5)).child(
                    Label::new(format!("• {text}"))
                        .color(Color::Muted)
                        .truncate(),
                )
            }
            Self::More { count, .. } => base.indent_level(2).child(
                Label::new(match count {
                    1 => "1 more nested block · open the page to read it".to_owned(),
                    count => format!("{count} more nested blocks · open the page to read them"),
                })
                .size(LabelSize::Small)
                .color(Color::Muted),
            ),
        }
    }
}

/// Lays blocks out grouped by page in the order given, each with its parents and nested blocks.
pub fn context_rows<'a>(items: impl IntoIterator<Item = ContextItem<'a>>) -> Vec<ContextRow> {
    let mut rows = Vec::new();
    let mut heading: Option<(usize, String)> = None;
    for item in items {
        let page = item.page;
        if heading.as_ref().is_none_or(|(_, id)| id != &page) {
            rows.push(ContextRow::Page {
                page: page.clone(),
                title: item.title.to_owned(),
                matches: 0,
            });
            heading = Some((rows.len() - 1, page.clone()));
        }
        if let Some((index, _)) = &heading
            && let ContextRow::Page { matches, .. } = &mut rows[*index]
        {
            *matches += 1;
        }
        if !item.outline.ancestors.is_empty() {
            rows.push(ContextRow::Parents {
                page: page.clone(),
                text: item.outline.ancestors.join(" › "),
            });
        }
        rows.push(ContextRow::Block {
            page: page.clone(),
            text: item.text.to_owned(),
        });
        rows.extend(
            item.outline
                .children
                .iter()
                .map(|(depth, text)| ContextRow::Child {
                    page: page.clone(),
                    depth: *depth,
                    text: text.clone(),
                }),
        );
        if item.outline.more_children > 0 {
            rows.push(ContextRow::More {
                page,
                count: item.outline.more_children,
            });
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_group_blocks_with_parents_children_and_the_rest_counted() {
        let nested = Outline {
            ancestors: vec!["Week".into(), "Monday".into()],
            children: vec![(1, "asked".into()), (2, "about pricing".into())],
            more_children: 3,
        };
        let plain = Outline::default();
        let item = |page: &str, title: &'static str, text: &'static str, outline| ContextItem {
            page: page.into(),
            title,
            text,
            outline,
        };
        let rows = context_rows([
            item("a", "A", "Met [[Ana]]", &nested),
            item("a", "A", "again", &plain),
            item("b", "B", "elsewhere", &plain),
        ]);
        let text: Vec<String> = rows
            .iter()
            .map(|row| match row {
                ContextRow::Page { title, matches, .. } => format!("{title} {matches}"),
                ContextRow::Parents { text, .. } => format!("^ {text}"),
                ContextRow::Block { text, .. } => format!("- {text}"),
                ContextRow::Child { depth, text, .. } => format!("{depth} {text}"),
                ContextRow::More { count, .. } => format!("+{count}"),
            })
            .collect();
        assert_eq!(
            text,
            [
                "A 2",
                "^ Week › Monday",
                "- Met [[Ana]]",
                "1 asked",
                "2 about pricing",
                "+3",
                "- again",
                "B 1",
                "- elsewhere"
            ]
        );
        assert!(rows.iter().all(|row| !row.page().is_empty()));
    }
}
