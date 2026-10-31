//! Notebook-scoped navigation state: tabs, history, recents, favorites, and expanded folders.
//! Ports the reference `workspace.ts` rules and the `visit` bookkeeping from its App, keyed by
//! stable page identity rather than path so renames keep every list intact.

use regex::Regex;
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

pub const MAX_RECENT: usize = 12;
pub const SHOWN_RECENT: usize = 6;
pub const MAX_TABS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserView {
    Pages,
    Daily,
    Tags,
    Tasks,
}

/// The list the Tasks browser shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskView {
    Due,
    Upcoming,
    #[default]
    Inbox,
    Next,
    Waiting,
    Someday,
    Projects,
    Done,
}

impl TaskView {
    pub const ALL: [Self; 8] = [
        Self::Due,
        Self::Upcoming,
        Self::Inbox,
        Self::Next,
        Self::Waiting,
        Self::Someday,
        Self::Projects,
        Self::Done,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Due => "Due",
            Self::Upcoming => "Upcoming",
            Self::Inbox => "Inbox",
            Self::Next => "Next",
            Self::Waiting => "Waiting",
            Self::Someday => "Someday",
            Self::Projects => "Projects",
            Self::Done => "Done",
        }
    }
}

/// Which projects the Projects list shows: with an open action, without one, or every one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectStatus {
    #[default]
    Active,
    Inactive,
    All,
}

impl ProjectStatus {
    pub const ALL: [Self; 3] = [Self::Active, Self::Inactive, Self::All];

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Inactive => "Inactive",
            Self::All => "All",
        }
    }
}

/// The reference `TaskStatus`: which of a tag's or page's tasks show. All keeps the blocks or
/// references; the others list matching tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    #[default]
    All,
    Open,
    Due,
    Done,
}

impl TaskStatus {
    pub const ALL: [Self; 4] = [Self::All, Self::Open, Self::Due, Self::Done];

    pub fn label(self) -> &'static str {
        match self {
            Self::All => "All",
            Self::Open => "Open",
            Self::Due => "Due",
            Self::Done => "Done",
        }
    }
}

/// The most rows a remembered list size may ask for, as the reference allows.
pub const MAX_LIMIT: usize = 10_000;

/// The Tags browser order: by name, or by how many blocks use the tag.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TagSort {
    #[default]
    Name,
    Most,
    Fewest,
}

impl TagSort {
    pub fn next(self) -> Self {
        match self {
            Self::Name => Self::Most,
            Self::Most => Self::Fewest,
            Self::Fewest => Self::Name,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Sort: name",
            Self::Most => "Sort: most blocks first",
            Self::Fewest => "Sort: fewest blocks first",
        }
    }
}

/// The Pages browser's remembered filter text and namespace.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PageChoices {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

/// The window chrome a reader set: a hidden sidebar, a collapsed Files heading, and the month the
/// Daily notes browser shows (`YYYY-MM`, or empty for every month).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChromeChoices {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub sidebar_hidden: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub files_collapsed: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub daily_month: String,
}

/// The Tags browser's remembered filter text and order.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagListChoices {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default)]
    pub sort: TagSort,
}

/// A tag page's remembered filter text, task status, and first row shown.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagChoices {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    #[serde(default)]
    pub status: TaskStatus,
    #[serde(default)]
    pub scroll: usize,
}

/// The Tasks browser's remembered choices: the list, the project filter (a page identity), the
/// Projects status filter, the filter text, how many rows show, and an unsent capture.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskChoices {
    #[serde(default)]
    pub view: TaskView,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default)]
    pub project_status: ProjectStatus,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub query: String,
    /// Rows shown; zero means the first page.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub limit: usize,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub draft: String,
}

fn is_zero(value: &usize) -> bool {
    *value == 0
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Destination {
    Page { page: String },
    Tag { tag: String },
    Browser { view: BrowserView },
}

impl Destination {
    pub fn page(id: impl Into<String>) -> Self {
        Self::Page { page: id.into() }
    }

    pub fn page_id(&self) -> Option<&str> {
        match self {
            Self::Page { page } => Some(page),
            Self::Tag { .. } | Self::Browser { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub expanded: Vec<String>,
    /// Folders made here that may hold no page yet; they stay on this computer, as the
    /// reference keeps them until a page is saved in them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub folders: Vec<String>,
    #[serde(default)]
    pub favorites: Vec<Destination>,
    #[serde(default)]
    pub recent: Vec<Destination>,
    #[serde(default)]
    pub tabs: Vec<Destination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<Destination>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    /// Pages whose linked references list the oldest daily pages first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub oldest_first: Vec<String>,
    #[serde(default)]
    pub tasks: TaskChoices,
    #[serde(default)]
    pub pages: PageChoices,
    #[serde(default)]
    pub tag_list: TagListChoices,
    /// Tag pages by tag key.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tag_pages: BTreeMap<String, TagChoices>,
    /// Reading positions by [`position_key`]: the first line shown of a page in either pane.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub positions: BTreeMap<String, u32>,
    #[serde(default)]
    pub chrome: ChromeChoices,
}

/// Where a page's reading position is kept: the main pane and the side pane each keep one.
pub fn position_key(side: bool, page: &str) -> String {
    format!("{}:{page}", if side { "side" } else { "page" })
}

fn position_page(key: &str) -> Option<&str> {
    key.strip_prefix("page:")
        .or_else(|| key.strip_prefix("side:"))
}

/// A saved value, or its default when missing or unreadable.
fn field<T: DeserializeOwned + Default>(value: &Value, key: &str) -> T {
    value
        .get(key)
        .and_then(|item| serde_json::from_value(item.clone()).ok())
        .unwrap_or_default()
}

/// A saved map, keeping only the entries that read back.
fn entries<T: DeserializeOwned>(value: &Value, key: &str) -> BTreeMap<String, T> {
    value
        .get(key)
        .and_then(Value::as_object)
        .map(|items| {
            items
                .iter()
                .filter_map(|(key, item)| {
                    Some((key.clone(), serde_json::from_value(item.clone()).ok()?))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn destinations(value: Option<&Value>) -> Vec<Destination> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

impl Workspace {
    /// Reads saved state, dropping invalid entries; anything unreadable yields an empty workspace.
    pub fn load(text: &str) -> Self {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return Self::default();
        };
        if !value.get("tabs").is_some_and(Value::is_array) {
            return Self::default();
        }
        let mut recent = destinations(value.get("recent"));
        recent.truncate(MAX_RECENT);
        let mut tabs = destinations(value.get("tabs"));
        tabs.truncate(MAX_TABS);
        Self {
            expanded: value
                .get("expanded")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            folders: field(&value, "folders"),
            favorites: destinations(value.get("favorites")),
            recent,
            tabs,
            active: value
                .get("active")
                .and_then(|active| serde_json::from_value(active.clone()).ok()),
            side: value.get("side").and_then(Value::as_str).map(str::to_owned),
            oldest_first: value
                .get("oldest_first")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|item| item.as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default(),
            tasks: {
                let mut tasks: TaskChoices = field(&value, "tasks");
                tasks.limit = tasks.limit.min(MAX_LIMIT);
                tasks
            },
            pages: field(&value, "pages"),
            tag_list: field(&value, "tag_list"),
            tag_pages: entries(&value, "tag_pages"),
            positions: entries(&value, "positions"),
            chrome: field(&value, "chrome"),
        }
    }

    pub fn save(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{\"tabs\":[]}".into())
    }

    /// Records a navigation: the destination becomes active, leads the recents, and gains a tab.
    pub fn visit(&mut self, destination: Destination) {
        self.recent.retain(|item| item != &destination);
        self.recent.insert(0, destination.clone());
        self.recent.truncate(MAX_RECENT);
        self.show(destination);
    }

    /// Activates a destination, adding its tab, without touching recents (history moves).
    pub fn show(&mut self, destination: Destination) {
        if !self.tabs.contains(&destination) {
            self.tabs.push(destination.clone());
            let excess = self.tabs.len().saturating_sub(MAX_TABS);
            self.tabs.drain(..excess);
        }
        self.active = Some(destination);
    }

    /// Closes tabs and returns the destination that should become active, if the active one
    /// closed: the preferred tab when still open, otherwise the last remaining tab, otherwise
    /// the Pages browser.
    pub fn close(
        &mut self,
        closing: impl Fn(&Destination) -> bool,
        preferred: Option<&Destination>,
    ) -> Option<Destination> {
        self.tabs.retain(|tab| !closing(tab));
        let active = self.active.as_ref()?;
        if !closing(active) {
            return None;
        }
        let next = preferred
            .filter(|tab| self.tabs.contains(tab))
            .cloned()
            .or_else(|| self.tabs.last().cloned())
            .unwrap_or(Destination::Browser {
                view: BrowserView::Pages,
            });
        self.show(next.clone());
        Some(next)
    }

    pub fn is_favorite(&self, destination: &Destination) -> bool {
        self.favorites.contains(destination)
    }

    pub fn toggle_favorite(&mut self, destination: Destination) {
        if self.is_favorite(&destination) {
            self.favorites.retain(|item| item != &destination);
        } else {
            self.favorites.push(destination);
        }
    }

    pub fn remove_recent(&mut self, destination: &Destination) {
        self.recent.retain(|item| item != destination);
    }

    /// Drops destinations for pages that no longer exist, as the reference does on load.
    pub fn retain_pages(&mut self, exists: impl Fn(&str) -> bool) {
        let keep = |item: &Destination| item.page_id().is_none_or(&exists);
        self.favorites.retain(keep);
        self.recent.retain(keep);
        self.tabs.retain(keep);
        if self.active.as_ref().is_some_and(|active| !keep(active)) {
            self.active = None;
        }
        if self.side.as_deref().is_some_and(|side| !exists(side)) {
            self.side = None;
        }
        self.oldest_first.retain(|page| exists(page));
        self.positions
            .retain(|key, _| position_page(key).is_none_or(&exists));
        if self
            .tasks
            .project
            .as_deref()
            .is_some_and(|page| !exists(page))
        {
            self.tasks.project = None;
        }
    }

    /// Points every destination for page `from` at page `to`, for a page whose identity changed
    /// with its path, as a renamed note in a folder notebook does.
    pub fn rename_page(&mut self, from: &str, to: &str) {
        let renamed = Destination::page(to);
        let follow = |items: &mut Vec<Destination>| {
            let mut seen = false;
            items.retain_mut(|item| {
                if item.page_id() == Some(from) || item == &renamed {
                    let keep = !seen;
                    seen = true;
                    *item = renamed.clone();
                    keep
                } else {
                    true
                }
            });
        };
        follow(&mut self.favorites);
        follow(&mut self.recent);
        follow(&mut self.tabs);
        if self.active.as_ref().and_then(Destination::page_id) == Some(from) {
            self.active = Some(renamed.clone());
        }
        for page in self.side.iter_mut().chain(&mut self.oldest_first) {
            if page == from {
                *page = to.to_owned();
            }
        }
        if self.tasks.project.as_deref() == Some(from) {
            self.tasks.project = Some(to.to_owned());
        }
        for side in [false, true] {
            if let Some(position) = self.positions.remove(&position_key(side, from)) {
                self.positions.insert(position_key(side, to), position);
            }
        }
    }

    pub fn is_oldest_first(&self, page: &str) -> bool {
        self.oldest_first.iter().any(|item| item == page)
    }

    /// Chooses whether a page's linked references list the oldest daily pages first.
    pub fn set_oldest_first(&mut self, page: &str, oldest: bool) {
        self.oldest_first.retain(|item| item != page);
        if oldest {
            self.oldest_first.push(page.to_owned());
        }
    }

    pub fn toggle_expanded(&mut self, folder: &str) {
        if let Some(index) = self.expanded.iter().position(|item| item == folder) {
            self.expanded.remove(index);
        } else {
            self.expanded.push(folder.to_owned());
        }
    }

    /// Forgets a deleted folder and every folder inside it.
    pub fn forget_folder(&mut self, folder: &str) {
        let kept = |item: &String| {
            item != folder
                && !item
                    .strip_prefix(folder)
                    .is_some_and(|rest| rest.starts_with('/'))
        };
        self.expanded.retain(kept);
        self.folders.retain(kept);
    }

    /// Adds a new folder and opens its parents so it shows.
    pub fn add_folder(&mut self, folder: &str) {
        if !self.folders.iter().any(|item| item == folder) {
            self.folders.push(folder.to_owned());
            self.folders.sort();
        }
        let parts: Vec<&str> = folder.split('/').collect();
        for end in 1..parts.len() {
            let parent = parts[..end].join("/");
            if !self.expanded.contains(&parent) {
                self.expanded.push(parent);
            }
        }
    }
}

/// One window's back/forward list. Not persisted: the reference resets it per notebook load.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct History {
    entries: Vec<Destination>,
    position: usize,
}

impl History {
    pub fn current(&self) -> Option<&Destination> {
        self.entries.get(self.position)
    }

    /// Pushes a destination unless it is the current one, cutting off forward entries.
    pub fn push(&mut self, destination: Destination) {
        if self.current() == Some(&destination) {
            return;
        }
        if !self.entries.is_empty() {
            self.entries.truncate(self.position + 1);
        }
        self.entries.push(destination);
        self.position = self.entries.len() - 1;
    }

    pub fn can_go_back(&self) -> bool {
        self.position > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.position + 1 < self.entries.len()
    }

    pub fn back(&mut self) -> Option<Destination> {
        self.can_go_back().then(|| {
            self.position -= 1;
            self.entries[self.position].clone()
        })
    }

    pub fn forward(&mut self) -> Option<Destination> {
        self.can_go_forward().then(|| {
            self.position += 1;
            self.entries[self.position].clone()
        })
    }

    /// Points entries for page `from` at page `to`, keeping the position.
    pub fn rename_page(&mut self, from: &str, to: &str) {
        for entry in &mut self.entries {
            if entry.page_id() == Some(from) {
                *entry = Destination::page(to);
            }
        }
    }

    /// Removes destinations that no longer exist, keeping the position on the same entry or
    /// the nearest earlier one.
    pub fn retain(&mut self, keep: impl Fn(&Destination) -> bool) {
        let before = self.entries[..self.position.min(self.entries.len())]
            .iter()
            .filter(|item| !keep(item))
            .count();
        self.entries.retain(&keep);
        self.position = self
            .position
            .saturating_sub(before)
            .min(self.entries.len().saturating_sub(1));
    }
}

static NAMESPACE_PART: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[\p{L}\p{N}][\p{L}\p{N} _.&'()-]*$").expect("the namespace pattern is valid")
});

/// The reference `namespaceOf`: what precedes a title's last slash, when every part before it
/// is a name rather than ordinary text with a slash.
pub fn namespace_of(title: &str) -> &str {
    let Some((namespace, name)) = title.rsplit_once('/') else {
        return "";
    };
    let named = !name.is_empty()
        && name.trim_start_matches(js_whitespace) == name
        && namespace.split('/').all(|part| {
            NAMESPACE_PART.is_match(part)
                && part.trim_end_matches(js_whitespace) == part
                && !part.chars().all(|c| c.is_ascii_digit())
        });
    if named { namespace } else { "" }
}

fn js_whitespace(c: char) -> bool {
    c.is_whitespace() || c == '\u{feff}'
}

/// Every namespace with how many titles sit in it or below it, ordered by name.
pub fn namespaces<'a>(titles: impl IntoIterator<Item = &'a str>) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    for title in titles {
        let namespace = namespace_of(title);
        if namespace.is_empty() {
            continue;
        }
        let parts: Vec<_> = namespace.split('/').collect();
        for end in 1..=parts.len() {
            *counts.entry(parts[..end].join("/")).or_default() += 1;
        }
    }
    counts.into_iter().collect()
}

/// Whether a title belongs to `namespace` or one nested below it.
pub fn in_namespace(title: &str, namespace: &str) -> bool {
    let own = namespace_of(title);
    own == namespace
        || own
            .strip_prefix(namespace)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum TreeKind {
    Folder,
    File,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeRow {
    pub path: String,
    pub name: String,
    pub kind: TreeKind,
    pub depth: usize,
}

/// The reference `folderRows`: the physical tree of page paths and inventory entries, folders
/// first and then by path, descending only into expanded folders. Empty folders appear when
/// they are listed as entries.
pub fn folder_rows(
    paths: &[String],
    folders: &[String],
    expanded: &BTreeSet<String>,
    compare: impl Fn(&str, &str) -> std::cmp::Ordering,
) -> Vec<TreeRow> {
    let mut all_folders: BTreeSet<String> = folders.iter().cloned().collect();
    let files: BTreeSet<String> = paths.iter().cloned().collect();
    for item in files.iter().chain(folders.iter()) {
        let parts: Vec<_> = item.split('/').collect();
        for end in 1..parts.len() {
            all_folders.insert(parts[..end].join("/"));
        }
    }
    let mut children = std::collections::HashMap::<String, Vec<(TreeKind, String)>>::new();
    for (kind, path) in all_folders
        .into_iter()
        .map(|path| (TreeKind::Folder, path))
        .chain(files.into_iter().map(|path| (TreeKind::File, path)))
    {
        let parent = path.rsplit_once('/').map_or("", |(parent, _)| parent);
        children
            .entry(parent.to_owned())
            .or_default()
            .push((kind, path));
    }
    for group in children.values_mut() {
        group.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| compare(&a.1, &b.1)));
    }
    let mut rows = Vec::new();
    let mut stack = vec![(String::new(), 0usize, 0usize)];
    while let Some((parent, depth, index)) = stack.pop() {
        let Some((kind, path)) = children.get(&parent).and_then(|group| group.get(index)) else {
            continue;
        };
        stack.push((parent, depth, index + 1));
        rows.push(TreeRow {
            name: path.rsplit('/').next().unwrap_or_default().to_owned(),
            path: path.clone(),
            kind: *kind,
            depth,
        });
        if *kind == TreeKind::Folder && expanded.contains(path) {
            stack.push((path.clone(), depth + 1, 0));
        }
    }
    rows
}
