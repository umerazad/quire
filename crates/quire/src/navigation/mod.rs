//! Notebook navigation: the find palette, page creation, workspace persistence, and the keys
//! that drive tabs, history, favorites, and the browsers.

pub mod live;
pub mod palette;
pub mod tag_page;
pub mod tags;

use gpui::{App, Context, InteractiveElement, KeyBinding, Window};
use quire_core::{markdown::safe_page_path, workspace::Workspace};
use quire_store::{
    index::IndexClient, notebook::Edit, sync::format::page_id_for_path, worker::StorageClient,
};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

gpui::actions!(
    quire_navigation,
    [
        /// Finds a page by title, name, or date, or text inside pages.
        FindPage,
        /// Creates a page from a typed title.
        NewPage,
        GoBack,
        GoForward,
        /// Opens today's daily page, creating it from the daily layout when missing.
        OpenToday,
        ToggleFavorite,
        /// Shows the active page beside itself, or closes the side pane.
        ToggleSidePane,
        CloseTab,
        NextTab,
        PreviousTab,
        ShowPages,
        ShowDaily,
        /// Lists every tag with its block count.
        ShowTags,
        /// Moves focus to the Files tree.
        FocusFiles,
        /// Adds the highlighted tag to the selection for bulk rename or delete, or takes it out.
        ToggleTagSelected,
    ]
);

/// Arrow, Home, End, Enter, and Shift-Enter for a virtualized list view: `step` moves the
/// highlight and `confirm` opens the highlighted row, beside the page when `true`.
pub fn list_keys<V: 'static, E: InteractiveElement>(
    element: E,
    cx: &mut Context<V>,
    step: fn(&mut V, isize, &mut Context<V>),
    confirm: fn(&mut V, bool, &mut Window, &mut Context<V>),
) -> E {
    element
        .on_action(cx.listener(move |this, _: &menu::SelectNext, _, cx| step(this, 1, cx)))
        .on_action(cx.listener(move |this, _: &menu::SelectPrevious, _, cx| step(this, -1, cx)))
        .on_action(
            cx.listener(move |this, _: &menu::SelectFirst, _, cx| step(this, isize::MIN, cx)),
        )
        .on_action(cx.listener(move |this, _: &menu::SelectLast, _, cx| step(this, isize::MAX, cx)))
        .on_action(
            cx.listener(move |this, _: &menu::Confirm, window, cx| {
                confirm(this, false, window, cx)
            }),
        )
        .on_action(
            cx.listener(move |this, _: &menu::SecondaryConfirm, window, cx| {
                confirm(this, true, window, cx)
            }),
        )
}

pub const NAVIGATION_CONTEXT: &str = "QuireNotebook";
pub const NEW_PAGE_TEXT: &str = "- ";

macro_rules! primary {
    ($key:literal) => {
        if cfg!(target_os = "macos") {
            concat!("cmd-", $key)
        } else {
            concat!("ctrl-", $key)
        }
    };
}

pub fn bind_keys(cx: &mut App) {
    let context = Some(NAVIGATION_CONTEXT);
    let mut bindings = vec![
        KeyBinding::new(primary!("k"), FindPage, context),
        KeyBinding::new(primary!("n"), NewPage, context),
        KeyBinding::new(primary!("j"), OpenToday, context),
        KeyBinding::new(primary!("w"), CloseTab, context),
        KeyBinding::new("ctrl-tab", NextTab, context),
        KeyBinding::new("ctrl-shift-tab", PreviousTab, context),
        KeyBinding::new(primary!("\\"), ToggleSidePane, context),
        KeyBinding::new(primary!("shift-e"), FocusFiles, context),
        KeyBinding::new("enter", menu::Confirm, Some("Picker > Editor")),
        KeyBinding::new(
            "shift-enter",
            menu::SecondaryConfirm,
            Some("Picker > Editor"),
        ),
        KeyBinding::new(
            primary!("enter"),
            menu::SecondaryConfirm,
            Some("Picker > Editor"),
        ),
        KeyBinding::new("escape", menu::Cancel, Some("Picker > Editor")),
        KeyBinding::new("up", menu::SelectPrevious, Some("Picker > Editor")),
        KeyBinding::new("down", menu::SelectNext, Some("Picker > Editor")),
        KeyBinding::new("ctrl-p", menu::SelectPrevious, Some("Picker > Editor")),
        KeyBinding::new("ctrl-n", menu::SelectNext, Some("Picker > Editor")),
        KeyBinding::new("up", menu::SelectPrevious, Some("QuireList")),
        KeyBinding::new("down", menu::SelectNext, Some("QuireList")),
        KeyBinding::new("home", menu::SelectFirst, Some("QuireList")),
        KeyBinding::new("end", menu::SelectLast, Some("QuireList")),
        KeyBinding::new("enter", menu::Confirm, Some("QuireList")),
        KeyBinding::new("shift-enter", menu::SecondaryConfirm, Some("QuireList")),
        KeyBinding::new("right", menu::SelectChild, Some("QuireList")),
        KeyBinding::new("left", menu::SelectParent, Some("QuireList")),
        KeyBinding::new("up", menu::SelectPrevious, Some("QuireList > Editor")),
        KeyBinding::new("down", menu::SelectNext, Some("QuireList > Editor")),
        KeyBinding::new("enter", menu::Confirm, Some("QuireList > Editor")),
        KeyBinding::new(
            "shift-enter",
            menu::SecondaryConfirm,
            Some("QuireList > Editor"),
        ),
        KeyBinding::new(primary!("enter"), ToggleTagSelected, Some("QuireTags")),
        KeyBinding::new(
            primary!("enter"),
            ToggleTagSelected,
            Some("QuireTags > Editor"),
        ),
    ];
    if cfg!(target_os = "macos") {
        bindings.push(KeyBinding::new("cmd-[", GoBack, context));
        bindings.push(KeyBinding::new("cmd-]", GoForward, context));
    } else {
        bindings.push(KeyBinding::new("alt-left", GoBack, context));
        bindings.push(KeyBinding::new("alt-right", GoForward, context));
    }
    cx.bind_keys(bindings);
}

/// Notebook-scoped navigation state, kept beside the notebook database on this computer only.
#[derive(Clone)]
pub struct WorkspaceFile {
    path: PathBuf,
    notebook: String,
}

impl WorkspaceFile {
    pub fn beside(database: &Path, notebook: &str) -> Self {
        Self {
            path: database.with_file_name("quire-workspace.json"),
            notebook: notebook.to_owned(),
        }
    }

    fn entries(&self) -> Map<String, Value> {
        std::fs::read(&self.path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            })
            .unwrap_or_default()
    }

    pub fn load(&self) -> Workspace {
        self.entries()
            .get(&self.notebook)
            .map(|value| Workspace::load(&value.to_string()))
            .unwrap_or_default()
    }

    /// Replaces this notebook's entry atomically, keeping other notebooks' entries.
    pub fn save(&self, workspace: &Workspace) -> Result<(), String> {
        let mut entries = self.entries();
        let value = serde_json::from_str(&workspace.save()).map_err(|error| error.to_string())?;
        entries.insert(self.notebook.clone(), value);
        let text =
            serde_json::to_vec(&Value::Object(entries)).map_err(|error| error.to_string())?;
        let temporary = self.path.with_extension("json.tmp");
        std::fs::write(&temporary, text)
            .and_then(|()| std::fs::rename(&temporary, &self.path))
            .map_err(|error| {
                format!(
                    "Cannot save tabs and favorites to {}: {error}. Check write access to the notebook folder; your notes are unaffected.",
                    self.path.display()
                )
            })
    }
}

pub fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

/// The reference `notebook.create`: a validated title in `folder`, refused when any page already
/// uses that name, saved with the starting text `- `. Returns the new page's identity.
pub async fn create_page(
    storage: StorageClient,
    index: IndexClient,
    title: String,
    folder: String,
) -> Result<String, String> {
    let name = safe_page_path(&title).map_err(|error| error.to_string())?;
    let folder = folder.trim_matches('/');
    let path = if folder.is_empty() {
        name
    } else {
        format!("{folder}/{name}")
    };
    let taken = index
        .path_taken(path.clone())
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| format!("{error:#}"))?;
    if taken {
        return Err(format!(
            "A page named {path} already exists. Open it with Find, or choose another title."
        ));
    }
    let derived = page_id_for_path(&path);
    let heads = storage
        .heads(derived.clone())
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| format!("{error:#}"))?;
    let page = if heads.is_empty() {
        derived
    } else {
        format!("p-{}", uuid::Uuid::new_v4())
    };
    storage
        .save(Edit {
            page: page.clone(),
            base: Vec::new(),
            path,
            text: NEW_PAGE_TEXT.into(),
            deleted: false,
            created: now_millis() as u64,
            recovery: None,
        })
        .map_err(|error| error.to_string())?
        .await
        .map_err(|error| {
            format!("Cannot create the page: {error:#}. Check free space, then retry.")
        })?;
    Ok(page)
}
