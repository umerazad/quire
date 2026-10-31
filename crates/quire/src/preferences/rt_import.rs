//! Imports the Electron app's machine preferences: `appearance.json` beside its profile data and
//! the `quire.editor.settings` value in its renderer's local storage. Both are only read.

use super::{Change, Preferences};
use serde_json::Value;
use settings::{CaptureTarget, ThemeAppearanceMode};
use std::path::{Path, PathBuf};

pub const EDITOR_KEY: &str = "quire.editor.settings";
pub const LEGACY_VIM_KEY: &str = "quire.editor.vim";
pub const APPEARANCE_KEY: &str = "quire.appearance";
const LEGACY_PREFIX: &str = "threadnote";
const ORIGIN: &str = "file://";

/// Where the Electron app keeps its profile data, newest name first: `QUIRE_DATA_DIR`, then the
/// platform's application data folder under Quire and, before 0.9.23, Threadnote.
pub fn profile_candidates() -> Vec<PathBuf> {
    let overridden = std::env::var_os("QUIRE_DATA_DIR").map(PathBuf::from);
    let base = if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library/Application Support"))
    } else if cfg!(target_os = "windows") {
        std::env::var_os("APPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
    };
    overridden
        .into_iter()
        .chain(
            base.into_iter()
                .flat_map(|base| [base.join("Quire"), base.join("Threadnote")]),
        )
        .collect()
}

/// The changes that reproduce the Electron profile at `root`, relative to Quire's defaults.
pub fn import(root: &Path) -> Result<Vec<Change>, String> {
    let storage = root.join("Local Storage/leveldb");
    let keys = [
        EDITOR_KEY,
        LEGACY_VIM_KEY,
        APPEARANCE_KEY,
        "threadnote.editor.settings",
        "threadnote.editor.vim",
        "threadnote.appearance",
    ];
    let stored = if storage.is_dir() {
        super::local_storage::read(&storage, ORIGIN, &keys)?
    } else {
        Default::default()
    };
    let value = |key: &str| {
        stored.get(key).cloned().or_else(|| {
            stored
                .get(&key.replacen("quire", LEGACY_PREFIX, 1))
                .cloned()
        })
    };
    let preferences = editor_preferences(value(EDITOR_KEY), value(LEGACY_VIM_KEY));
    let appearance = appearance_file(&root.join("appearance.json"))
        .or_else(|| value(APPEARANCE_KEY).and_then(|value| appearance(&value)));
    let mut changes = preferences.changes_from(&Preferences::default());
    if let Some(appearance) = appearance.filter(|mode| *mode != ThemeAppearanceMode::System) {
        changes.insert(0, Change::Appearance(appearance));
    }
    Ok(changes)
}

/// The reference store's reading of its saved value: stored fields over the defaults, all
/// discarded when any is invalid, and the older Vim-only key used when nothing else is saved.
pub fn editor_preferences(saved: Option<String>, legacy_vim: Option<String>) -> Preferences {
    match saved {
        Some(text) => serde_json::from_str::<Value>(&text)
            .ok()
            .and_then(|value| merged(&value))
            .and_then(|preferences| preferences.validate().ok())
            .unwrap_or_default(),
        None => Preferences {
            vim: legacy_vim.as_deref() != Some("false"),
            ..Default::default()
        },
    }
}

fn merged(value: &Value) -> Option<Preferences> {
    let object = value.as_object()?;
    let mut preferences = Preferences::default();
    for (key, value) in object {
        match key.as_str() {
            "vim" => preferences.vim = value.as_bool()?,
            "formatTags" => preferences.format_tags = value.as_bool()?,
            "fontFamily" => preferences.font_family = value.as_str()?.to_owned(),
            "fontSize" => preferences.font_size = value.as_f64()? as f32,
            "lineHeight" => preferences.line_spacing = value.as_f64()? as f32,
            "captureTarget" => {
                preferences.capture_target = match value.as_str()? {
                    "inbox" => CaptureTarget::Inbox,
                    "today" => CaptureTarget::Today,
                    _ => return None,
                }
            }
            _ => {}
        }
    }
    Some(preferences)
}

fn appearance_file(path: &Path) -> Option<ThemeAppearanceMode> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    appearance(value.get("appearance")?.as_str()?)
}

pub fn appearance(value: &str) -> Option<ThemeAppearanceMode> {
    match value {
        "system" => Some(ThemeAppearanceMode::System),
        "light" => Some(ThemeAppearanceMode::Light),
        "dark" => Some(ThemeAppearanceMode::Dark),
        _ => None,
    }
}
