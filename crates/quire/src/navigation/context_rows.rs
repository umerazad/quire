//! Blocks shown with their surroundings, as the tag page and linked references show them:
//! grouped under a heading per page, each with its parents above and nested blocks below,
//! laid out as single-line rows for a virtualized list. Page groups collapse, and a block with
//! more nested blocks than the index lists loads more of them in place.

use super::Jump;
use crate::rich_text::{self, OnLink, RichCache};
use quire_store::index::{Outline, SHOWN_CHILDREN};
use std::collections::{HashMap, HashSet};
use ui::{
    Color, Icon, IconName, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, Toggleable,
    prelude::*,
};

/// Nested blocks each "load more" adds.
pub const MORE_CHILDREN: usize = 50;

/// Where a block's nested blocks are read: its page revision and the block's position.
pub type BlockAt = (String, u32);

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextRow {
    Page {
        page: String,
        title: String,
        matches: usize,
        collapsed: bool,
    },
    Parents {
        page: String,
        jump: Jump,
        text: String,
    },
    Block {
        page: String,
        jump: Jump,
        text: String,
    },
    Child {
        page: String,
        jump: Jump,
        depth: u32,
        text: String,
    },
    More {
        page: String,
        jump: Jump,
        at: BlockAt,
        count: usize,
    },
}

/// One block to lay out: the page identity and title it belongs to, its text, and where it
/// sits.
pub struct ContextItem<'a> {
    pub page: String,
    pub title: &'a str,
    pub text: &'a str,
    /// Where opening the block puts the caret.
    pub jump: Jump,
    pub at: BlockAt,
    pub outline: &'a Outline,
}

/// What the reader changed in a list of context rows: collapsed page groups and blocks whose
/// nested blocks were loaded past the first few.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Expansion {
    collapsed: HashSet<String>,
    shown: HashMap<BlockAt, usize>,
}

impl Expansion {
    pub fn is_collapsed(&self, page: &str) -> bool {
        self.collapsed.contains(page)
    }

    pub fn set_collapsed(&mut self, page: &str, collapsed: bool) {
        if collapsed {
            self.collapsed.insert(page.to_owned());
        } else {
            self.collapsed.remove(page);
        }
    }

    /// How many nested blocks to list under the block at `at`.
    pub fn shown(&self, at: &BlockAt) -> usize {
        self.shown.get(at).copied().unwrap_or(SHOWN_CHILDREN)
    }

    /// Lists `MORE_CHILDREN` more nested blocks under the block at `at`.
    pub fn more(&mut self, at: BlockAt) {
        let shown = self.shown(&at) + MORE_CHILDREN;
        self.shown.insert(at, shown);
    }

    /// The blocks loaded past the first few, for asking the index again.
    pub fn loaded(&self) -> impl Iterator<Item = (&BlockAt, usize)> {
        self.shown.iter().map(|(at, shown)| (at, *shown))
    }
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

    /// Where opening the row puts the caret; a page heading opens the page at its top.
    pub fn jump(&self) -> Option<&Jump> {
        match self {
            Self::Page { .. } => None,
            Self::Parents { jump, .. }
            | Self::Block { jump, .. }
            | Self::Child { jump, .. }
            | Self::More { jump, .. } => Some(jump),
        }
    }

    /// The row as a list item; `noun` names one and several matches in page headings. Block
    /// text shows as rich text whose links open through `on_link`.
    pub fn item(
        &self,
        index: usize,
        highlighted: bool,
        noun: (&str, &str),
        rich: &RichCache,
        on_link: &OnLink,
        cx: &App,
    ) -> ListItem {
        let base = ListItem::new(("context-row", index))
            .spacing(ListItemSpacing::Dense)
            .toggle_state(highlighted);
        let line = |text: &str, muted: bool| {
            let rich = rich.get(text);
            let marker = rich.task.map(|done| if done { "☑" } else { "☐" });
            h_flex()
                .min_w_0()
                .gap_1()
                .child(Label::new(marker.unwrap_or("•")).color(Color::Muted))
                .child(rich_text::line(
                    ("context-text", index),
                    &rich,
                    muted,
                    Some(on_link.clone()),
                    cx,
                ))
        };
        match self {
            Self::Page {
                title,
                matches,
                collapsed,
                ..
            } => base.toggle(Some(!collapsed)).child(
                h_flex()
                    .gap_2()
                    .child(Icon::new(IconName::File).color(Color::Muted))
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
                h_flex()
                    .min_w_0()
                    .text_sm()
                    .child(rich_text::line(
                        ("context-parents", index),
                        &rich.get(text),
                        true,
                        Some(on_link.clone()),
                        cx,
                    ))
                    .child(Label::new("›").size(LabelSize::Small).color(Color::Muted)),
            ),
            Self::Block { text, .. } => base.indent_level(1).child(line(text, false)),
            Self::Child { depth, text, .. } => base
                .indent_level(1 + (*depth as usize).min(5))
                .child(line(text, true)),
            Self::More { count, .. } => base.indent_level(2).child(
                Label::new(match count {
                    1 => "1 more nested block · Enter or click shows it".to_owned(),
                    count => format!(
                        "{count} more nested blocks · Enter or click shows {}",
                        if *count > MORE_CHILDREN {
                            format!("{MORE_CHILDREN} more")
                        } else {
                            "them".to_owned()
                        }
                    ),
                })
                .size(LabelSize::Small)
                .color(Color::Accent),
            ),
        }
    }
}

/// Lays blocks out grouped by page in the order given, each with its parents and nested blocks;
/// a collapsed page keeps only its heading.
pub fn context_rows<'a>(
    items: impl IntoIterator<Item = ContextItem<'a>>,
    expansion: &Expansion,
) -> Vec<ContextRow> {
    let mut rows = Vec::new();
    let mut heading: Option<(usize, String)> = None;
    for item in items {
        let page = item.page;
        if heading.as_ref().is_none_or(|(_, id)| id != &page) {
            rows.push(ContextRow::Page {
                page: page.clone(),
                title: item.title.to_owned(),
                matches: 0,
                collapsed: expansion.is_collapsed(&page),
            });
            heading = Some((rows.len() - 1, page.clone()));
        }
        if let Some((index, _)) = &heading
            && let ContextRow::Page { matches, .. } = &mut rows[*index]
        {
            *matches += 1;
        }
        if expansion.is_collapsed(&page) {
            continue;
        }
        if !item.outline.ancestors.is_empty() {
            rows.push(ContextRow::Parents {
                page: page.clone(),
                jump: item.jump.clone(),
                text: item.outline.ancestors.join(" › "),
            });
        }
        rows.push(ContextRow::Block {
            page: page.clone(),
            jump: item.jump.clone(),
            text: item.text.to_owned(),
        });
        rows.extend(
            item.outline
                .children
                .iter()
                .map(|(depth, text)| ContextRow::Child {
                    page: page.clone(),
                    jump: item.jump.clone(),
                    depth: *depth,
                    text: text.clone(),
                }),
        );
        if item.outline.more_children > 0 {
            rows.push(ContextRow::More {
                page,
                jump: item.jump,
                at: item.at,
                count: item.outline.more_children,
            });
        }
    }
    rows
}

/// What activating a row does: a "more" row loads more nested blocks, and every other row opens
/// its page.
pub enum Activate {
    Open { page: String, jump: Option<Jump> },
    More(BlockAt),
}

impl ContextRow {
    pub fn activate(&self) -> Activate {
        match self {
            Self::More { at, .. } => Activate::More(at.clone()),
            row => Activate::Open {
                page: row.page().to_owned(),
                jump: row.jump().cloned(),
            },
        }
    }
}

/// The highlighted row after collapsing or expanding the group of `rows[highlighted]`: its page
/// heading, so the highlight never points past the rows that remain.
pub fn heading_of(rows: &[ContextRow], highlighted: usize) -> Option<(usize, String)> {
    let page = rows.get(highlighted)?.page();
    rows.iter()
        .position(|row| matches!(row, ContextRow::Page { page: id, .. } if id == page))
        .map(|index| (index, page.to_owned()))
}

/// Replaces the outline of each loaded block with the longer one the index gave for it.
pub fn with_loaded<T>(
    items: &mut [T],
    at: impl Fn(&T) -> BlockAt,
    outline: impl Fn(&mut T) -> &mut Outline,
    loaded: Vec<(BlockAt, Outline)>,
) {
    let loaded: HashMap<_, _> = loaded.into_iter().collect();
    for item in items {
        if let Some(longer) = loaded.get(&at(item)) {
            *outline(item) = longer.clone();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(rows: &[ContextRow]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                ContextRow::Page {
                    title,
                    matches,
                    collapsed,
                    ..
                } => format!("{title} {matches}{}", if *collapsed { " ▸" } else { "" }),
                ContextRow::Parents { text, .. } => format!("^ {text}"),
                ContextRow::Block { text, .. } => format!("- {text}"),
                ContextRow::Child { depth, text, .. } => format!("{depth} {text}"),
                ContextRow::More { count, .. } => format!("+{count}"),
            })
            .collect()
    }

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
            jump: Jump::default(),
            at: (format!("r-{page}"), 0),
        };
        let items = || {
            [
                item("a", "A", "Met [[Ana]]", &nested),
                item("a", "A", "again", &plain),
                item("b", "B", "elsewhere", &plain),
            ]
        };
        let mut expansion = Expansion::default();
        let rows = context_rows(items(), &expansion);
        assert_eq!(
            texts(&rows),
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
        assert!(matches!(
            rows[5].activate(),
            Activate::More(at) if at == ("r-a".to_owned(), 0)
        ));
        assert_eq!(heading_of(&rows, 4), Some((0, "a".to_owned())));

        expansion.set_collapsed("a", true);
        assert_eq!(
            texts(&context_rows(items(), &expansion)),
            ["A 2 ▸", "B 1", "- elsewhere"],
            "a collapsed group keeps its heading and count"
        );
        expansion.set_collapsed("a", false);
        assert_eq!(context_rows(items(), &expansion), rows);
    }

    #[test]
    fn loading_more_grows_the_listed_children_and_replaces_outlines() {
        let mut expansion = Expansion::default();
        let at = ("r".to_owned(), 4);
        assert_eq!(expansion.shown(&at), SHOWN_CHILDREN);
        expansion.more(at.clone());
        expansion.more(at.clone());
        assert_eq!(expansion.shown(&at), SHOWN_CHILDREN + 2 * MORE_CHILDREN);
        assert_eq!(expansion.loaded().count(), 1);

        let mut items = vec![
            (at.clone(), Outline::default()),
            (("s".into(), 1), Outline::default()),
        ];
        let longer = Outline {
            children: vec![(1, "deep".into())],
            ..Outline::default()
        };
        with_loaded(
            &mut items,
            |item| item.0.clone(),
            |item| &mut item.1,
            vec![(at, longer.clone())],
        );
        assert_eq!(items[0].1, longer);
        assert_eq!(items[1].1, Outline::default());
    }
}
