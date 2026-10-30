//! The find palette (Command/Ctrl-K) and new-page prompt (Command/Ctrl-N), as a Zed picker over
//! the query index: titles containing the query, the page `find_page` resolves (names, unique
//! titles, and daily dates), then ranked full-text hits.

use gpui::{App, Context, SharedString, Task, Window};
use picker::{Picker, PickerDelegate};
use quire_store::index::{IndexClient, ListedPage, SearchHit};
use std::{collections::HashSet, rc::Rc, sync::Arc};
use ui::{Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, Toggleable, prelude::*};

const LIMIT: usize = 40;
const SNIPPET: usize = 150;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Find,
    Create,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteItem {
    Page {
        page: String,
        title: String,
        path: String,
    },
    Hit {
        page: String,
        title: String,
        snippet: String,
        block: Option<String>,
    },
    Create {
        title: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteChoice {
    Open { page: String, block: Option<String> },
    Create { title: String },
}

type Choose = Rc<dyn Fn(PaletteChoice, bool, &mut Window, &mut App)>;

pub struct PagePalette {
    client: IndexClient,
    mode: PaletteMode,
    items: Vec<PaletteItem>,
    selected: usize,
    requested: usize,
    applied: usize,
    generation: u64,
    error: Option<SharedString>,
    choose: Choose,
}

/// What one index step returned for a query.
pub struct Results {
    pub pages: Vec<ListedPage>,
    pub found: Option<String>,
    pub hits: Vec<SearchHit>,
}

fn snippet(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let marker = c == '^'
            && chars
                .peek()
                .is_some_and(|next| next.is_alphanumeric() || *next == '_' || *next == '-');
        if marker {
            while chars
                .peek()
                .is_some_and(|next| next.is_alphanumeric() || *next == '_' || *next == '-')
            {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out.chars().take(SNIPPET).collect()
}

/// Orders palette rows the reference way: the resolved page, title matches in catalogue order,
/// then full-text hits not already shown, cut to 40, with a create row when nothing has that
/// exact title.
pub fn items(query: &str, mode: PaletteMode, results: Results) -> Vec<PaletteItem> {
    let trimmed = query.trim();
    let mut items = Vec::new();
    if mode == PaletteMode::Find {
        let needle = query.to_lowercase();
        let found = results.found.as_ref().and_then(|id| {
            results
                .pages
                .iter()
                .find(|listed| listed.page.page.as_str() == Some(id))
        });
        let titled = results
            .pages
            .iter()
            .filter(|listed| listed.page.title.to_lowercase().contains(&needle));
        let mut seen = HashSet::new();
        for listed in found.into_iter().chain(titled) {
            let id = listed.page.page.to_lossy_string();
            if seen.insert(id.clone()) {
                items.push(PaletteItem::Page {
                    page: id,
                    title: listed.page.title.clone(),
                    path: listed.page.path.clone(),
                });
            }
        }
        let mut shown: HashSet<String> = items
            .iter()
            .filter_map(|item| match item {
                PaletteItem::Page { path, .. } => Some(format!("{path}:{path}")),
                _ => None,
            })
            .collect();
        for hit in results.hits {
            if shown.insert(format!("{}:{}", hit.source.path, hit.snippet)) {
                items.push(PaletteItem::Hit {
                    page: hit.source.page.to_lossy_string(),
                    title: hit.source.title,
                    snippet: snippet(&hit.snippet),
                    block: hit.block_id,
                });
            }
        }
        items.truncate(LIMIT);
    }
    let exists = results
        .pages
        .iter()
        .any(|listed| listed.page.title.to_lowercase() == trimmed.to_lowercase());
    if !trimmed.is_empty() && !exists {
        items.push(PaletteItem::Create {
            title: trimmed.to_owned(),
        });
    }
    items
}

impl PagePalette {
    pub fn new(client: IndexClient, mode: PaletteMode, choose: Choose) -> Self {
        Self {
            client,
            mode,
            items: Vec::new(),
            selected: 0,
            requested: 0,
            applied: 0,
            generation: 0,
            error: None,
            choose,
        }
    }

    pub fn items(&self) -> &[PaletteItem] {
        &self.items
    }

    #[cfg(test)]
    pub fn requested(&self) -> usize {
        self.requested
    }

    /// Whether the rows answer the latest query.
    pub fn settled(&self) -> bool {
        self.applied == self.requested
    }

    /// Applies results unless a newer query was issued or they reflect an older index state
    /// than rows already shown. Returns whether they were applied.
    pub fn apply(
        &mut self,
        request: usize,
        generation: u64,
        query: &str,
        results: Result<Results, String>,
    ) -> bool {
        if request != self.requested || (results.is_ok() && generation < self.generation) {
            return false;
        }
        self.applied = request;
        match results {
            Ok(results) => {
                self.generation = generation;
                self.items = items(query, self.mode, results);
                self.error = None;
            }
            Err(error) => {
                self.items.clear();
                self.error = Some(
                    format!("Cannot search the notebook: {error}. Wait for indexing to finish, then type again.").into(),
                );
            }
        }
        self.selected = 0;
        true
    }

    /// Starts an index request for `query` and returns its sequence number.
    fn request(
        &mut self,
        query: String,
    ) -> (
        usize,
        impl std::future::Future<Output = (u64, Result<Results, String>)> + use<>,
    ) {
        self.requested += 1;
        let search = self.mode == PaletteMode::Find && query.trim().chars().count() >= 2;
        let reply = self.client.with_generation(move |index| {
            let pages = index.pages()?;
            let found = index
                .find_page(query.trim())?
                .map(|page| page.page.to_lossy_string());
            let hits = if search {
                index.search(&query)?
            } else {
                Vec::new()
            };
            Ok(Results { pages, found, hits })
        });
        (self.requested, async move {
            match reply {
                Ok(reply) => match reply.await {
                    Ok((generation, results)) => (generation, Ok(results)),
                    Err(error) => (0, Err(format!("{error:#}"))),
                },
                Err(error) => (0, Err(error.to_string())),
            }
        })
    }

    fn choice(&self) -> Option<PaletteChoice> {
        Some(match self.items.get(self.selected)? {
            PaletteItem::Page { page, .. } => PaletteChoice::Open {
                page: page.clone(),
                block: None,
            },
            PaletteItem::Hit { page, block, .. } => PaletteChoice::Open {
                page: page.clone(),
                block: block.clone(),
            },
            PaletteItem::Create { title } => PaletteChoice::Create {
                title: title.clone(),
            },
        })
    }
}

impl PickerDelegate for PagePalette {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "quire-page-palette"
    }

    fn match_count(&self) -> usize {
        self.items.len()
    }

    fn selected_index(&self) -> usize {
        self.selected
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, _: &mut Context<Picker<Self>>) {
        self.selected = ix;
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        match self.mode {
            PaletteMode::Find => "Find a page or text… (Shift-Enter opens beside)".into(),
            PaletteMode::Create => "New page title…".into(),
        }
    }

    fn no_matches_text(&self, _: &mut Window, _: &mut App) -> Option<SharedString> {
        self.error.clone().or_else(|| {
            Some(match self.mode {
                PaletteMode::Find => "No pages match".into(),
                PaletteMode::Create => "Type a title, or choose a page that already has it".into(),
            })
        })
    }

    fn update_matches(
        &mut self,
        query: String,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let (request, work) = self.request(query.clone());
        cx.spawn(async move |picker, cx| {
            let (generation, results) = work.await;
            picker
                .update(cx, |picker, cx| {
                    if picker.delegate.apply(request, generation, &query, results) {
                        cx.notify();
                    }
                })
                .ok();
        })
    }

    fn confirm(&mut self, secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(choice) = self.choice() else {
            return;
        };
        (self.choose)(choice, secondary, window, cx);
        cx.emit(gpui::DismissEvent);
    }

    fn dismissed(&mut self, _: &mut Window, _: &mut Context<Picker<Self>>) {}

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        _: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let (title, detail): (String, Option<String>) = match self.items.get(ix)? {
            PaletteItem::Page { title, path, .. } => (title.clone(), Some(path.clone())),
            PaletteItem::Hit { title, snippet, .. } => (title.clone(), Some(snippet.clone())),
            PaletteItem::Create { title } => (format!("Create page “{title}”"), None),
        };
        Some(
            ListItem::new(ix)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .child(v_flex().child(Label::new(title).truncate()).when_some(
                    detail,
                    |this, detail| {
                        this.child(
                            Label::new(detail)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                    },
                )),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippets_drop_block_markers_and_stay_short() {
        assert_eq!(snippet("see ^b-1 here ^ ok"), "see  here ^ ok");
        assert_eq!(snippet(&"x".repeat(400)).len(), SNIPPET);
    }
}
