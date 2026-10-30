//! Notebook-scoped navigation state: tabs, history, recents, favorites, and expanded folders.
//! Ports the reference `workspace.ts` rules and the `visit` bookkeeping from its App, keyed by
//! stable page identity rather than path so renames keep every list intact.

use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeSet, sync::LazyLock};

pub const MAX_RECENT: usize = 12;
pub const SHOWN_RECENT: usize = 6;
pub const MAX_TABS: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserView {
    Pages,
    Daily,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Destination {
    Page { page: String },
    Browser { view: BrowserView },
}

impl Destination {
    pub fn page(id: impl Into<String>) -> Self {
        Self::Page { page: id.into() }
    }

    pub fn page_id(&self) -> Option<&str> {
        match self {
            Self::Page { page } => Some(page),
            Self::Browser { .. } => None,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    #[serde(default)]
    pub expanded: Vec<String>,
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
            favorites: destinations(value.get("favorites")),
            recent,
            tabs,
            active: value
                .get("active")
                .and_then(|active| serde_json::from_value(active.clone()).ok()),
            side: value.get("side").and_then(Value::as_str).map(str::to_owned),
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
    }

    pub fn toggle_expanded(&mut self, folder: &str) {
        if let Some(index) = self.expanded.iter().position(|item| item == folder) {
            self.expanded.remove(index);
        } else {
            self.expanded.push(folder.to_owned());
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
