//! Linked references under a page: the blocks that link to it with their parents and nested
//! blocks, grouped by page, ordered newest or oldest daily page first, and narrowed by a
//! filter line with one-click suggestion chips.

use super::{list_keys, live::Live};
use editor::{Editor, EditorEvent};
use gpui::{
    App, ClickEvent, Context, Entity, EventEmitter, FocusHandle, Focusable, ScrollStrategy,
    Subscription, UniformListScrollHandle, Window, px, uniform_list,
};
use quire_core::reference_filter::{ReferenceFilter, TermState, term_state, toggle_term};
use quire_store::{
    index::{Facet, FacetKind, IndexClient, ReferenceBlock},
    sync::json::JsString,
};
use ui::{
    Button, ButtonStyle, Color, Icon, IconButton, IconName, Label, LabelCommon, LabelSize,
    ListItem, ListItemSpacing, Toggleable, prelude::*, v_flex,
};

/// References whose contexts suggest chips, as the reference's first page of 40.
const FACET_SOURCES: usize = 40;
const FACETS: usize = 8;

pub enum ReferencesEvent {
    Open { page: String, beside: bool },
    Order { oldest: bool },
}

/// One query's answer: every reference counted, the ones the filter keeps, and chips.
#[derive(Clone, Debug, Default)]
pub struct ReferenceResults {
    pub total: usize,
    pub shown: Vec<ReferenceBlock>,
    pub facets: Vec<Facet>,
    /// Some reference comes from a daily page, so the order toggle applies.
    pub dated: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReferenceRow {
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

impl ReferenceRow {
    pub fn page(&self) -> &str {
        match self {
            Self::Page { page, .. }
            | Self::Parents { page, .. }
            | Self::Block { page, .. }
            | Self::Child { page, .. }
            | Self::More { page, .. } => page,
        }
    }
}

/// Lays references out grouped by page, each with its parents above and nested blocks below.
pub fn rows(references: &[ReferenceBlock]) -> Vec<ReferenceRow> {
    let mut rows = Vec::new();
    let mut heading: Option<(usize, String)> = None;
    for reference in references {
        let page = reference.source.page.to_lossy_string();
        if heading.as_ref().is_none_or(|(_, id)| id != &page) {
            rows.push(ReferenceRow::Page {
                page: page.clone(),
                title: reference.source.title.clone(),
                matches: 0,
            });
            heading = Some((rows.len() - 1, page.clone()));
        }
        if let Some((index, _)) = &heading
            && let ReferenceRow::Page { matches, .. } = &mut rows[*index]
        {
            *matches += 1;
        }
        if !reference.ancestors.is_empty() {
            rows.push(ReferenceRow::Parents {
                page: page.clone(),
                text: reference.ancestors.join(" › "),
            });
        }
        rows.push(ReferenceRow::Block {
            page: page.clone(),
            text: reference.snippet.clone(),
        });
        rows.extend(
            reference
                .children
                .iter()
                .map(|(depth, text)| ReferenceRow::Child {
                    page: page.clone(),
                    depth: *depth,
                    text: text.clone(),
                }),
        );
        if reference.more_children > 0 {
            rows.push(ReferenceRow::More {
                page,
                count: reference.more_children,
            });
        }
    }
    rows
}

pub struct ReferencesView {
    page: String,
    oldest_first: bool,
    pub(crate) results: Live<ReferenceResults>,
    pub(crate) filter: Entity<Editor>,
    client: Option<IndexClient>,
    collapsed: bool,
    highlighted: usize,
    focus: FocusHandle,
    scroll: UniformListScrollHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ReferencesEvent> for ReferencesView {}

impl Focusable for ReferencesView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.results.value.total > 1 {
            self.filter.focus_handle(cx)
        } else {
            self.focus.clone()
        }
    }
}

impl ReferencesView {
    pub fn new(
        page: String,
        oldest_first: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let filter = cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(
                "Filter by #tag, [[page]], project:name or words · -term hides",
                window,
                cx,
            );
            editor
        });
        let subscriptions = vec![cx.subscribe(&filter, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                this.highlighted = 0;
                this.query(cx);
            }
        })];
        Self {
            page,
            oldest_first,
            results: Live::default(),
            filter,
            client: None,
            collapsed: false,
            highlighted: 0,
            focus: cx.focus_handle(),
            scroll: UniformListScrollHandle::new(),
            _subscriptions: subscriptions,
        }
    }

    pub fn text(&self, cx: &App) -> String {
        self.filter.read(cx).text(cx)
    }

    /// The typed filter, or the reason it cannot be read.
    pub fn parsed(&self, cx: &App) -> Result<ReferenceFilter, &'static str> {
        ReferenceFilter::parse(&self.text(cx))
    }

    pub fn set_oldest_first(&mut self, oldest: bool, cx: &mut Context<Self>) {
        if self.oldest_first != oldest {
            self.oldest_first = oldest;
            self.highlighted = 0;
            self.query(cx);
        }
    }

    pub fn refresh(&mut self, client: &IndexClient, cx: &mut Context<Self>) {
        self.client = Some(client.clone());
        self.query(cx);
    }

    /// Asks for the references and the filter's result in one index step, so the rows, the
    /// count, and the chips always describe the same filter and index state.
    fn query(&mut self, cx: &mut Context<Self>) {
        let Some(client) = &self.client else {
            return;
        };
        let filter = self.parsed(cx).unwrap_or_default();
        let page = JsString::from(self.page.as_str());
        let oldest = self.oldest_first;
        let answer = client.with_generation(move |index| {
            let all = index.linked_references(&page, oldest)?;
            let dated = all
                .iter()
                .any(|reference| index.date(&reference.source.title).is_some());
            let shown: Vec<ReferenceBlock> = if filter.is_empty() {
                all.clone()
            } else {
                let names = index.filter_names(&filter)?;
                all.iter()
                    .filter(|reference| filter.matches(&names, &reference.context))
                    .cloned()
                    .collect()
            };
            let facets = if all.len() > 1 {
                let contexts: Vec<_> = shown
                    .iter()
                    .take(FACET_SOURCES)
                    .map(|reference| reference.context.clone())
                    .collect();
                index.reference_facets(&page, &contexts, FACETS)?
            } else {
                Vec::new()
            };
            Ok(ReferenceResults {
                total: all.len(),
                shown,
                facets,
                dated,
            })
        });
        self.results.request(
            answer,
            cx,
            |this| &mut this.results,
            |error| {
                format!(
                    "Cannot list linked references: {error}. Wait for indexing to finish, then retry."
                )
            },
        );
        cx.notify();
    }

    pub fn rows(&self) -> Vec<ReferenceRow> {
        rows(&self.results.value.shown)
    }

    /// Adds a chip's term to the filter, or `-term` with `exclude`, or takes it out.
    pub fn toggle_facet(
        &mut self,
        term: &str,
        exclude: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let text = toggle_term(&self.text(cx), term, exclude);
        self.set_filter(&text, window, cx);
    }

    pub fn set_filter(&mut self, text: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.filter
            .update(cx, |filter, cx| filter.set_text(text, window, cx));
    }

    fn move_highlight(&mut self, delta: isize, cx: &mut Context<Self>) {
        let count = self.rows().len();
        if count == 0 {
            return;
        }
        let next = (self.highlighted as isize)
            .saturating_add(delta)
            .clamp(0, count as isize - 1);
        self.highlighted = next as usize;
        self.scroll
            .scroll_to_item(self.highlighted, ScrollStrategy::Top);
        cx.notify();
    }

    fn open(&mut self, index: usize, beside: bool, cx: &mut Context<Self>) {
        if let Some(row) = self.rows().get(index) {
            self.highlighted = index;
            cx.emit(ReferencesEvent::Open {
                page: row.page().to_owned(),
                beside,
            });
        }
    }

    fn render_facet(
        &self,
        index: usize,
        facet: &Facet,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Button {
        let state = term_state(text, &facet.term);
        let icon = match facet.kind {
            FacetKind::Tag => None,
            FacetKind::Page => Some(IconName::File),
            FacetKind::Project => Some(IconName::Folder),
        };
        let label = match state {
            Some(TermState::Exclude) => format!("−{} {}", facet.label, facet.count),
            _ => format!("{} {}", facet.label, facet.count),
        };
        let tooltip = match state {
            Some(TermState::Include) => format!("Showing only references with {}", facet.label),
            Some(TermState::Exclude) => format!("Hiding references with {}", facet.label),
            None => format!(
                "Show only references with {} · Alt-click hides them",
                facet.label
            ),
        };
        let term = facet.term.clone();
        Button::new(("facet", index), label)
            .style(ButtonStyle::Subtle)
            .when_some(icon, |button, icon| button.start_icon(Icon::new(icon)))
            .toggle_state(state.is_some())
            .tooltip(ui::Tooltip::text(tooltip))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                this.toggle_facet(&term, event.modifiers().alt, window, cx)
            }))
    }

    fn render_rows(&self, rows: Vec<ReferenceRow>, cx: &mut Context<Self>) -> impl IntoElement {
        uniform_list(
            "reference-rows",
            rows.len(),
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let base = ListItem::new(("reference-row", index))
                            .spacing(ListItemSpacing::Dense)
                            .toggle_state(index == this.highlighted);
                        let item = match rows.get(index)? {
                            ReferenceRow::Page { title, matches, .. } => base
                                .start_slot(Icon::new(IconName::File).color(Color::Muted))
                                .child(
                                    h_flex()
                                        .gap_2()
                                        .child(Label::new(title.clone()).truncate())
                                        .child(
                                            Label::new(match matches {
                                                1 => "1 reference".to_owned(),
                                                count => format!("{count} references"),
                                            })
                                            .size(LabelSize::Small)
                                            .color(Color::Muted),
                                        ),
                                ),
                            ReferenceRow::Parents { text, .. } => base.indent_level(1).child(
                                Label::new(format!("{text} ›"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted)
                                    .truncate(),
                            ),
                            ReferenceRow::Block { text, .. } => base
                                .indent_level(1)
                                .child(Label::new(format!("• {text}")).truncate()),
                            ReferenceRow::Child { depth, text, .. } => {
                                base.indent_level(1 + (*depth as usize).min(5)).child(
                                    Label::new(format!("• {text}"))
                                        .color(Color::Muted)
                                        .truncate(),
                                )
                            }
                            ReferenceRow::More { count, .. } => base.indent_level(2).child(
                                Label::new(match count {
                                    1 => {
                                        "1 more nested block · open the page to read it".to_owned()
                                    }
                                    count => format!(
                                        "{count} more nested blocks · open the page to read them"
                                    ),
                                })
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                            ),
                        };
                        Some(
                            item.on_click(cx.listener(move |this, event: &ClickEvent, _, cx| {
                                this.open(index, event.modifiers().shift, cx)
                            })),
                        )
                    })
                    .collect()
            }),
        )
        .track_scroll(&self.scroll)
        .flex_1()
    }
}

impl Render for ReferencesView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let results = &self.results.value;
        let (total, shown, dated) = (results.total, results.shown.len(), results.dated);
        let text = self.text(cx);
        let parsed = self.parsed(cx);
        let filtering = total > 1 || !text.is_empty();
        let facets: Vec<_> = results
            .facets
            .clone()
            .iter()
            .enumerate()
            .map(|(index, facet)| self.render_facet(index, facet, &text, cx))
            .collect();
        let rows = self.rows();
        let header = h_flex()
            .gap_2()
            .child(
                IconButton::new(
                    "references-collapse",
                    if self.collapsed {
                        IconName::ChevronRight
                    } else {
                        IconName::ChevronDown
                    },
                )
                .tooltip(ui::Tooltip::text(if self.collapsed {
                    "Show linked references"
                } else {
                    "Hide linked references"
                }))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.collapsed = !this.collapsed;
                    cx.notify();
                })),
            )
            .child(Label::new("Linked references"))
            .child(
                Label::new(total.to_string())
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .when(dated && !self.collapsed, |header| {
                let oldest = self.oldest_first;
                header.child(
                    Button::new(
                        "reference-order",
                        if oldest {
                            "Oldest first"
                        } else {
                            "Newest first"
                        },
                    )
                    .style(ButtonStyle::Subtle)
                    .tooltip(ui::Tooltip::text("Change the order of daily pages"))
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(ReferencesEvent::Order { oldest: !oldest })
                    })),
                )
            });
        let root = v_flex()
            .id("linked-references")
            .key_context("QuireList QuireReferences")
            .track_focus(&self.focus)
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .p_1()
            .gap_1()
            .on_action(
                cx.listener(|this, _: &menu::Cancel, window, cx| this.set_filter("", window, cx)),
            )
            .when(!self.collapsed && shown > 0, |root| root.h(px(300.)));
        list_keys(
            root,
            cx,
            |this, delta, cx| this.move_highlight(delta, cx),
            |this, beside, _, cx| {
                let index = this.highlighted;
                this.open(index, beside, cx)
            },
        )
        .child(header)
        .when(!self.collapsed, |root| {
            root.when(filtering, |root| {
                root.child(
                    h_flex()
                        .gap_2()
                        .child(Icon::new(IconName::Filter).color(Color::Muted))
                        .child(
                            div()
                                .flex_1()
                                .border_1()
                                .border_color(cx.theme().colors().border)
                                .p_1()
                                .child(self.filter.clone()),
                        )
                        .when(!text.trim().is_empty(), |line| {
                            line.child(
                                Label::new(format!("{shown} of {total}"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                        })
                        .when(!text.is_empty(), |line| {
                            line.child(
                                IconButton::new("reference-filter-clear", IconName::Close)
                                    .tooltip(ui::Tooltip::text("Clear the filter"))
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.set_filter("", window, cx)
                                    })),
                            )
                        }),
                )
            })
            .when_some(parsed.err(), |root, error| {
                root.child(Label::new(error).color(Color::Error))
            })
            .when(!facets.is_empty(), |root| {
                root.child(h_flex().gap_1().flex_wrap().children(facets))
            })
            .when_some(self.results.error.clone(), |root, error| {
                root.child(Label::new(error).color(Color::Error))
            })
            .when(total == 0, |root| {
                root.child(
                    Label::new("Thoughts that point here will appear here.").color(Color::Muted),
                )
            })
            .when(total > 0 && shown == 0, |root| {
                root.child(
                    h_flex()
                        .gap_2()
                        .child(
                            Label::new("No linked references match this filter.")
                                .color(Color::Muted),
                        )
                        .child(
                            Button::new("reference-filter-reset", "Clear it")
                                .style(ButtonStyle::Subtle)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.set_filter("", window, cx)
                                })),
                        ),
                )
            })
            .when(shown > 0, |root| root.child(self.render_rows(rows, cx)))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_core::reference_filter::ReferenceContext;
    use quire_store::index::PageRef;

    fn reference(page: &str, snippet: &str) -> ReferenceBlock {
        ReferenceBlock {
            source: PageRef {
                page: JsString::from(page),
                revision: format!("r-{page}"),
                path: format!("{page}.md"),
                title: page.to_uppercase(),
            },
            from: 0,
            block_id: None,
            snippet: snippet.into(),
            context: ReferenceContext::default(),
            ancestors: Vec::new(),
            children: Vec::new(),
            more_children: 0,
        }
    }

    #[test]
    fn rows_group_references_with_parents_children_and_the_rest_counted() {
        let mut nested = reference("a", "Met [[Ana]]");
        nested.ancestors = vec!["Week".into(), "Monday".into()];
        nested.children = vec![(1, "asked".into()), (2, "about pricing".into())];
        nested.more_children = 3;
        let rows = rows(&[nested, reference("a", "again"), reference("b", "elsewhere")]);
        let text: Vec<String> = rows
            .iter()
            .map(|row| match row {
                ReferenceRow::Page { title, matches, .. } => format!("{title} {matches}"),
                ReferenceRow::Parents { text, .. } => format!("^ {text}"),
                ReferenceRow::Block { text, .. } => format!("- {text}"),
                ReferenceRow::Child { depth, text, .. } => format!("{depth} {text}"),
                ReferenceRow::More { count, .. } => format!("+{count}"),
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
    }
}
