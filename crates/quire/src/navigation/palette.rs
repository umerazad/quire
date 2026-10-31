//! The find palette (Command/Ctrl-K) and new-page prompt (Command/Ctrl-N), as a Zed picker over
//! the query index: commands whose label holds the query, titles containing the query, the page
//! `find_page` resolves (names, unique titles, and daily dates), blocks holding the query as
//! typed (unsaved text included), then ranked full-text hits.

use super::Jump;
use gpui::{App, Context, SharedString, Task, Window};
use picker::{Picker, PickerDelegate};
use quire_core::ai::{
    Verb,
    ask::{Needs, PRESETS, Preset},
};
use quire_store::index::{IndexClient, ListedPage, SearchHit};
use settings::ThemeAppearanceMode;
use std::{collections::HashSet, rc::Rc, sync::Arc};
use ui::{
    Icon, IconName, IconSize, Label, LabelCommon, LabelSize, ListItem, ListItemSpacing, Toggleable,
    prelude::*,
};

const LIMIT: usize = 40;
const SNIPPET: usize = 150;
/// Blocks holding the query as typed, per page, as the reference palette lists them.
const TEXT_HITS_PER_PAGE: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteMode {
    Find,
    Create,
}

/// What a command row does when chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaletteCommand {
    Appearance(ThemeAppearanceMode),
    /// Runs the verb on the selection or the block at the caret of the page editor used last.
    Ai(Verb),
    AskNotebook,
    Preset(&'static Preset),
}

/// A command offered above the pages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommandRow {
    pub command: PaletteCommand,
    pub label: String,
    pub detail: String,
    /// Listed only once the query has text, so an empty palette opens on pages and AI.
    pub when_typed: bool,
}

/// What the command rows depend on, read when the palette opens.
#[derive(Clone, Debug, Default)]
pub struct CommandContext {
    pub appearance: Option<ThemeAppearanceMode>,
    pub vim: bool,
    pub ai: bool,
    /// The title of the page in the main pane, when one is shown.
    pub page: Option<String>,
    pub daily: bool,
}

const APPEARANCES: [(ThemeAppearanceMode, &str); 3] = [
    (ThemeAppearanceMode::Light, "Light"),
    (ThemeAppearanceMode::Dark, "Dark"),
    (ThemeAppearanceMode::System, "Follow system"),
];

/// The reference palette commands: the three appearances, and with AI on the verbs (with Vim
/// off, since Vim has `:ai`), Ask the notebook, and the presets the shown page allows.
pub fn commands(context: &CommandContext) -> Vec<CommandRow> {
    let appearances = APPEARANCES.into_iter().map(|(mode, name)| CommandRow {
        command: PaletteCommand::Appearance(mode),
        label: format!("Appearance: {name}"),
        detail: if context.appearance == Some(mode) {
            "In use now".into()
        } else if mode == ThemeAppearanceMode::System {
            "Match your computer's light or dark setting".into()
        } else {
            format!("Use {} colors everywhere", name.to_lowercase())
        },
        when_typed: true,
    });
    let ai = |command, label: String, detail: &str| CommandRow {
        command,
        label: format!("AI: {label}"),
        detail: detail.into(),
        when_typed: false,
    };
    let verbs = Verb::ALL
        .into_iter()
        .filter(|_| context.ai && !context.vim)
        .map(|verb| {
            ai(
                PaletteCommand::Ai(verb),
                verb.label().into(),
                "Selection or current block",
            )
        });
    let ask = context.ai.then(|| {
        ai(
            PaletteCommand::AskNotebook,
            "Ask the notebook".into(),
            "Opens the answer pane beside the page",
        )
    });
    let presets = PRESETS
        .iter()
        .filter(|preset| context.ai && preset.offered(context.page.as_deref(), context.daily))
        .map(|preset| {
            ai(
                PaletteCommand::Preset(preset),
                preset.label_for(context.page.as_deref()),
                if preset.needs == Needs::Daily {
                    "Proposes links, task states, and dates for review"
                } else {
                    "Asks in the answer pane"
                },
            )
        });
    appearances.chain(verbs).chain(ask).chain(presets).collect()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteItem {
    Command(CommandRow),
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
        from: Option<u32>,
    },
    Create {
        title: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaletteChoice {
    Open { page: String, jump: Option<Jump> },
    Create { title: String },
    Command(PaletteCommand),
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
    folder: String,
    commands: Vec<CommandRow>,
    rich: crate::rich_text::RichCache,
}

/// What one index step returned for a query.
pub struct Results {
    pub pages: Vec<ListedPage>,
    pub found: Option<String>,
    /// Blocks holding the query as typed, up to eight per page.
    pub live: Vec<SearchHit>,
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

/// Orders palette rows the reference way: commands whose label holds the query, the resolved
/// page, title matches in catalogue order, then blocks holding the query and full-text hits not
/// already shown, cut to 40, with a create row when nothing has that exact title.
pub fn items(
    query: &str,
    mode: PaletteMode,
    commands: &[CommandRow],
    results: Results,
) -> Vec<PaletteItem> {
    let trimmed = query.trim();
    let mut items = Vec::new();
    if mode == PaletteMode::Find {
        let needle = query.to_lowercase();
        let typed = needle.trim();
        let commands = commands
            .iter()
            .filter(|row| {
                (!row.when_typed || !typed.is_empty()) && row.label.to_lowercase().contains(typed)
            })
            .cloned()
            .map(PaletteItem::Command);
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
        for hit in results.live.into_iter().chain(results.hits) {
            if shown.insert(format!("{}:{}", hit.source.path, hit.snippet)) {
                items.push(PaletteItem::Hit {
                    page: hit.source.page.to_lossy_string(),
                    title: hit.source.title,
                    snippet: snippet(&hit.snippet),
                    block: hit.block_id,
                    from: hit.from,
                });
            }
        }
        items.truncate(LIMIT);
        items.splice(0..0, commands);
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
            folder: String::new(),
            commands: Vec::new(),
            rich: Default::default(),
        }
    }

    /// Names the folder a created page goes into in the prompt.
    pub fn in_folder(mut self, folder: String) -> Self {
        self.folder = folder;
        self
    }

    /// Offers `commands` above the pages in Find mode.
    pub fn with_commands(mut self, commands: Vec<CommandRow>) -> Self {
        self.commands = commands;
        self
    }

    #[cfg(test)]
    pub fn items(&self) -> &[PaletteItem] {
        &self.items
    }

    #[cfg(test)]
    pub fn requested(&self) -> usize {
        self.requested
    }

    /// Whether the rows answer the latest query.
    #[cfg(test)]
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
                self.items = items(query, self.mode, &self.commands, results);
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
        let find = self.mode == PaletteMode::Find;
        let search = find && query.trim().chars().count() >= 2;
        let reply = self.client.with_generation(move |index| {
            let pages = index.pages()?;
            let found = index
                .find_page(query.trim())?
                .map(|page| page.page.to_lossy_string());
            let live = if find {
                index.text_hits(&query, TEXT_HITS_PER_PAGE, LIMIT)?
            } else {
                Vec::new()
            };
            let hits = if search {
                index.search(&query)?
            } else {
                Vec::new()
            };
            Ok(Results {
                pages,
                found,
                live,
                hits,
            })
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
            PaletteItem::Command(row) => PaletteChoice::Command(row.command),
            PaletteItem::Page { page, .. } => PaletteChoice::Open {
                page: page.clone(),
                jump: None,
            },
            PaletteItem::Hit {
                page, block, from, ..
            } => PaletteChoice::Open {
                page: page.clone(),
                jump: match (block, from) {
                    (block, Some(from)) => Some(Jump::at(block.clone(), *from)),
                    (Some(block), None) => Some(Jump::block(block.clone())),
                    (None, None) => None,
                },
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
            PaletteMode::Find if self.commands.is_empty() => {
                "Find a page or text… (Shift-Enter opens beside)".into()
            }
            PaletteMode::Find => "Find a page, text, or command… (Shift-Enter opens beside)".into(),
            PaletteMode::Create if self.folder.is_empty() => "New page title…".into(),
            PaletteMode::Create => format!("New page title in {}…", self.folder).into(),
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
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let item = self.items.get(ix)?;
        let icon = match item {
            PaletteItem::Command(CommandRow {
                command: PaletteCommand::Appearance(_),
                ..
            }) => Some(IconName::Screen),
            PaletteItem::Command(_) => Some(IconName::Sparkle),
            _ => None,
        };
        let (title, detail) = match item {
            PaletteItem::Command(row) => (
                row.label.clone(),
                Some(
                    Label::new(row.detail.clone())
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate()
                        .into_any_element(),
                ),
            ),
            PaletteItem::Page { title, path, .. } => (
                title.clone(),
                Some(
                    Label::new(path.clone())
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .truncate()
                        .into_any_element(),
                ),
            ),
            PaletteItem::Hit { title, snippet, .. } => (
                title.clone(),
                Some(
                    div()
                        .text_sm()
                        .child(crate::rich_text::line(
                            ("palette-hit", ix),
                            &self.rich.get(snippet),
                            true,
                            None,
                            cx,
                        ))
                        .into_any_element(),
                ),
            ),
            PaletteItem::Create { title } => (format!("Create page “{title}”"), None),
        };
        Some(
            ListItem::new(ix)
                .spacing(ListItemSpacing::Sparse)
                .inset(true)
                .toggle_state(selected)
                .when_some(icon, |item, icon| {
                    item.start_slot(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
                })
                .child(
                    v_flex()
                        .min_w_0()
                        .child(Label::new(title).truncate())
                        .children(detail),
                ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(rows: &[CommandRow]) -> Vec<&str> {
        rows.iter().map(|row| row.label.as_str()).collect()
    }

    #[test]
    fn commands_follow_ai_vim_and_the_shown_page() {
        let off = CommandContext {
            appearance: Some(ThemeAppearanceMode::Dark),
            ..Default::default()
        };
        let rows = commands(&off);
        assert_eq!(
            labels(&rows),
            [
                "Appearance: Light",
                "Appearance: Dark",
                "Appearance: Follow system"
            ],
            "with AI off only the appearances are offered"
        );
        assert_eq!(rows[0].detail, "Use light colors everywhere");
        assert_eq!(rows[1].detail, "In use now");
        assert_eq!(
            rows[2].detail,
            "Match your computer's light or dark setting"
        );
        let day = CommandContext {
            ai: true,
            vim: true,
            page: Some("2026-10-30".into()),
            daily: true,
            ..off.clone()
        };
        assert_eq!(
            labels(&commands(&day))[3..],
            [
                "AI: Ask the notebook",
                "AI: Due today",
                "AI: This week",
                "AI: Summarize the last occurrence of 2026-10-30",
                "AI: What changed yesterday",
                "AI: Tidy this day",
                "AI: Tagged with…",
            ],
            "Vim keeps the verbs on :ai; a daily page adds Tidy"
        );
        let browser = CommandContext { ai: true, ..off };
        let rows = commands(&browser);
        assert_eq!(rows.len(), 3 + 6 + 1 + 4);
        assert!(
            !labels(&rows)
                .iter()
                .any(|label| label.contains("Summarize") || label.contains("Tidy")),
            "without a shown page, page presets are left out"
        );
    }

    #[test]
    fn commands_come_first_and_appearances_wait_for_typing() {
        let rows = commands(&CommandContext {
            ai: true,
            ..Default::default()
        });
        let results = || Results {
            pages: Vec::new(),
            found: None,
            live: Vec::new(),
            hits: Vec::new(),
        };
        let shown = |query: &str| -> Vec<String> {
            items(query, PaletteMode::Find, &rows, results())
                .into_iter()
                .map(|item| match item {
                    PaletteItem::Command(row) => row.label,
                    PaletteItem::Create { title } => format!("create {title}"),
                    other => format!("{other:?}"),
                })
                .collect()
        };
        let empty = shown("");
        assert_eq!(empty.len(), 6 + 1 + 4);
        assert!(empty.iter().all(|label| label.starts_with("AI: ")));
        assert_eq!(shown("DARK"), ["Appearance: Dark", "create DARK"]);
        assert!(
            items("dark", PaletteMode::Create, &rows, results())
                .iter()
                .all(|item| !matches!(item, PaletteItem::Command(_))),
            "the new page prompt offers no commands"
        );
    }

    #[test]
    fn snippets_drop_block_markers_and_stay_short() {
        assert_eq!(snippet("see ^b-1 here ^ ok"), "see  here ^ ok");
        assert_eq!(snippet(&"x".repeat(400)).len(), SNIPPET);
    }
}
