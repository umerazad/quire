//! Machine preferences, kept as Zed settings in Quire's `settings.json`. Quire layers the
//! Electron app's defaults over Zed's, the settings file is watched so edits apply live, and the
//! Electron profile is imported once when no settings file exists yet.

pub mod data;
pub mod local_storage;
pub mod rt_import;
pub mod view;

use fs::Fs;
use gpui::{App, Global, KeyBinding, UpdateGlobal as _, Window, actions};
use settings::{
    BufferLineHeight, CaptureTarget, FontFamilyName, FontSize, QuireSettingsContent,
    RegisterSetting, Settings, SettingsContent, SettingsStore, ThemeAppearanceMode, ThemeName,
    ThemeSelection,
};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};
use theme::{Appearance, SystemAppearance};
use theme_settings::ThemeSettings;

actions!(
    quire_preferences,
    [
        /// Opens Preferences: appearance, editing, tasks, and typography.
        OpenPreferences,
        /// Switches between the light and dark appearance, as an explicit choice.
        ToggleAppearance,
        /// Follows the computer's light or dark setting.
        UseSystemAppearance,
        /// Uses the light appearance.
        UseLightAppearance,
        /// Uses the dark appearance.
        UseDarkAppearance,
    ]
);

pub const DEFAULT_FONT_FAMILY: &str = "Pragmata Pro Mono Liga";
/// After the editor fonts, Arabic-script text falls back to the bundled Naskh.
pub const FONT_FALLBACKS: [&str; 3] = [
    "PragmataPro Mono Liga",
    ".ZedMono",
    crate::script_fonts::ARABIC,
];
pub const DEFAULT_FONT_SIZE: f32 = 16.;
pub const DEFAULT_LINE_SPACING: f32 = 1.85;
pub const FONT_SIZES: std::ops::RangeInclusive<f32> = 10.0..=40.0;
pub const LINE_SPACINGS: std::ops::RangeInclusive<f32> = 1.2..=2.5;
pub const FONT_FAMILY_LIMIT: usize = 150;

pub const INITIAL_SETTINGS: &str = "{\n  // Quire settings\n  //\n  // Preferences writes here, and edits apply as soon as this file is saved.\n  // Quire uses Zed's settings format; keys left out keep Quire's defaults.\n}\n";

/// The Quire-only part of the settings: tag formatting and the quick-capture destination.
#[derive(RegisterSetting, Clone, Copy, Debug, PartialEq)]
pub struct QuirePreferences {
    pub format_tags: bool,
    pub capture_target: CaptureTarget,
}

impl Settings for QuirePreferences {
    fn from_settings(content: &SettingsContent) -> Self {
        let quire = content.quire.clone().unwrap_or_default();
        Self {
            format_tags: quire.format_tags.unwrap_or(true),
            capture_target: quire.capture_target.unwrap_or_default(),
        }
    }
}

/// The Electron app's defaults, layered over Zed's own.
pub fn apply_defaults(content: &mut SettingsContent) {
    content.vim_mode = Some(true);
    // The reference's Vim cursors: an underline in Normal mode and a bar in Insert mode.
    let cursors = content
        .vim
        .get_or_insert_default()
        .cursor_shape
        .get_or_insert_default();
    cursors.normal = Some(settings::CursorShape::Underline);
    cursors.insert = Some(settings::VimInsertModeCursorShape::Bar);
    content
        .session
        .get_or_insert_default()
        .restore_unsaved_buffers = Some(false);
    let theme = content.theme.as_mut();
    theme.buffer_font_family = Some(FontFamilyName(DEFAULT_FONT_FAMILY.into()));
    theme.buffer_font_fallbacks = Some(
        FONT_FALLBACKS
            .iter()
            .map(|family| FontFamilyName((*family).into()))
            .collect(),
    );
    theme.buffer_font_size = Some(FontSize(DEFAULT_FONT_SIZE));
    theme.buffer_line_height = Some(BufferLineHeight::Custom(DEFAULT_LINE_SPACING));
    theme.theme = Some(selection(ThemeAppearanceMode::System, None));
    content.quire = Some(QuireSettingsContent {
        format_tags: Some(true),
        capture_target: Some(CaptureTarget::Inbox),
        ai: Some(settings::QuireAiSettingsContent {
            enabled: Some(false),
            backend: Some(settings::AiBackendName::Claude),
            claude_path: Some(String::new()),
            codex_path: Some(String::new()),
            model: Some(String::new()),
            writes: Some(settings::AiWritesName::Review),
        }),
    });
}

fn selection(mode: ThemeAppearanceMode, current: Option<&ThemeSelection>) -> ThemeSelection {
    let (light, dark) = match current {
        Some(ThemeSelection::Dynamic { light, dark, .. }) => (light.clone(), dark.clone()),
        _ => (
            ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
            ThemeName(settings::DEFAULT_DARK_THEME.into()),
        ),
    };
    ThemeSelection::Dynamic { mode, light, dark }
}

/// Every preference the Preferences window edits, as the Electron app named them.
#[derive(Clone, Debug, PartialEq)]
pub struct Preferences {
    pub appearance: ThemeAppearanceMode,
    pub vim: bool,
    pub format_tags: bool,
    pub capture_target: CaptureTarget,
    pub font_family: String,
    pub font_size: f32,
    pub line_spacing: f32,
}

impl Default for Preferences {
    fn default() -> Self {
        Self {
            appearance: ThemeAppearanceMode::System,
            vim: true,
            format_tags: true,
            capture_target: CaptureTarget::Inbox,
            font_family: DEFAULT_FONT_FAMILY.into(),
            font_size: DEFAULT_FONT_SIZE,
            line_spacing: DEFAULT_LINE_SPACING,
        }
    }
}

impl Preferences {
    pub fn current(cx: &App) -> Self {
        let theme = ThemeSettings::get_global(cx);
        let quire = QuirePreferences::get_global(cx);
        Self {
            appearance: theme.theme.mode().unwrap_or(ThemeAppearanceMode::System),
            vim: vim_mode_setting::VimModeSetting::get_global(cx).0,
            format_tags: quire.format_tags,
            capture_target: quire.capture_target,
            font_family: theme.buffer_font.family.to_string(),
            font_size: f32::from(theme.buffer_font_size_settings()),
            line_spacing: theme.buffer_line_height.value(),
        }
    }

    /// The reference checks, with its messages: every value or none.
    pub fn validate(self) -> Result<Self, String> {
        [
            Change::FontFamily(self.font_family.clone()),
            Change::FontSize(self.font_size),
            Change::LineSpacing(self.line_spacing),
        ]
        .iter()
        .try_for_each(Change::validate)?;
        Ok(Self {
            font_family: self.font_family.trim().to_owned(),
            ..self
        })
    }

    /// The changes that turn `base` into `self`.
    pub fn changes_from(&self, base: &Self) -> Vec<Change> {
        [
            (self.appearance != base.appearance).then_some(Change::Appearance(self.appearance)),
            (self.vim != base.vim).then_some(Change::Vim(self.vim)),
            (self.format_tags != base.format_tags).then_some(Change::FormatTags(self.format_tags)),
            (self.capture_target != base.capture_target)
                .then_some(Change::CaptureTarget(self.capture_target)),
            (self.font_family != base.font_family)
                .then(|| Change::FontFamily(self.font_family.clone())),
            (self.font_size != base.font_size).then_some(Change::FontSize(self.font_size)),
            (self.line_spacing != base.line_spacing)
                .then_some(Change::LineSpacing(self.line_spacing)),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

/// One preference edit, written to the user's settings file.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Appearance(ThemeAppearanceMode),
    Vim(bool),
    FormatTags(bool),
    CaptureTarget(CaptureTarget),
    FontFamily(String),
    FontSize(f32),
    LineSpacing(f32),
}

impl Change {
    pub fn validate(&self) -> Result<(), String> {
        match self {
            Self::FontFamily(family)
                if family.trim().is_empty() || family.encode_utf16().count() > FONT_FAMILY_LIMIT =>
            {
                Err("The font family is empty or too long. Enter a font family of 1 to 150 characters.".into())
            }
            Self::FontSize(size) if !FONT_SIZES.contains(size) => Err(
                "The font size is outside the supported range. Enter a size from 10 to 40 pixels."
                    .into(),
            ),
            Self::LineSpacing(spacing) if !LINE_SPACINGS.contains(spacing) => Err(
                "The line spacing is outside the supported range. Enter a value from 1.2 to 2.5."
                    .into(),
            ),
            _ => Ok(()),
        }
    }

    pub fn apply(&self, content: &mut SettingsContent) {
        let theme = content.theme.as_mut();
        match self {
            Self::Appearance(mode) => theme.theme = Some(selection(*mode, theme.theme.as_ref())),
            Self::Vim(enabled) => content.vim_mode = Some(*enabled),
            Self::FormatTags(enabled) => {
                content.quire.get_or_insert_default().format_tags = Some(*enabled)
            }
            Self::CaptureTarget(target) => {
                content.quire.get_or_insert_default().capture_target = Some(*target)
            }
            Self::FontFamily(family) => {
                theme.buffer_font_family = Some(FontFamilyName(family.trim().into()))
            }
            Self::FontSize(size) => theme.buffer_font_size = Some(FontSize(*size)),
            Self::LineSpacing(spacing) => {
                theme.buffer_line_height = Some(BufferLineHeight::Custom(*spacing))
            }
        }
    }
}

/// Removes every value Preferences edits, so Quire's defaults apply again.
pub fn restore_defaults(content: &mut SettingsContent) {
    let theme = content.theme.as_mut();
    theme.buffer_font_family = None;
    theme.buffer_font_size = None;
    theme.buffer_line_height = None;
    if let Some(ThemeSelection::Dynamic { mode, .. }) = theme.theme.as_mut() {
        *mode = ThemeAppearanceMode::System;
    } else {
        theme.theme = None;
    }
    content.vim_mode = None;
    if let Some(quire) = content.quire.as_mut() {
        quire.format_tags = None;
        quire.capture_target = None;
    }
}

/// Where Preferences writes; replaced in tests by a fake file system.
#[derive(Clone)]
pub struct SettingsFs(pub Arc<dyn Fs>);
impl Global for SettingsFs {}

/// The last failure to read or write settings, shown in Preferences until the next success.
#[derive(Default)]
pub struct SettingsError(pub Option<gpui::SharedString>);
impl Global for SettingsError {}

pub fn settings_error(cx: &App) -> Option<gpui::SharedString> {
    cx.try_global::<SettingsError>()
        .and_then(|error| error.0.clone())
}

fn set_error(error: Option<String>, cx: &mut App) {
    cx.set_global(SettingsError(error.map(Into::into)));
    cx.refresh_windows();
}

/// Registers Quire's settings and defaults; call after `settings::init`.
pub fn init(cx: &mut App) {
    QuirePreferences::register(cx);
    SettingsStore::update_global(cx, |store, cx| {
        store.update_default_settings(cx, apply_defaults)
    });
    cx.on_action(|_: &ToggleAppearance, cx| toggle_appearance(cx));
    cx.on_action(|_: &UseSystemAppearance, cx| {
        change(Change::Appearance(ThemeAppearanceMode::System), cx)
    });
    cx.on_action(|_: &UseLightAppearance, cx| {
        change(Change::Appearance(ThemeAppearanceMode::Light), cx)
    });
    cx.on_action(|_: &UseDarkAppearance, cx| {
        change(Change::Appearance(ThemeAppearanceMode::Dark), cx)
    });
    cx.on_action(|_: &OpenPreferences, cx| view::open(cx));
}

/// Binds the Settings keys after the editor's own, so they win inside the Settings fields.
pub fn bind_keys(cx: &mut App) {
    let open = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    cx.bind_keys([
        KeyBinding::new(open, OpenPreferences, None),
        KeyBinding::new("escape", menu::Cancel, Some(view::CONTEXT)),
        KeyBinding::new("enter", menu::Confirm, Some(view::FIELD_CONTEXT)),
        KeyBinding::new("escape", menu::Cancel, Some(view::FIELD_CONTEXT)),
        KeyBinding::new(
            "escape",
            menu::Cancel,
            Some(crate::rt_migration::view::CONTEXT),
        ),
    ]);
    crate::tab_order::bind_keys(&[view::CONTEXT], cx);
}

/// Switches to the other of light and dark from what is shown, saved as an explicit choice.
pub fn toggle_appearance(cx: &mut App) {
    let dark = match Preferences::current(cx).appearance {
        ThemeAppearanceMode::Dark => true,
        ThemeAppearanceMode::Light => false,
        ThemeAppearanceMode::System => SystemAppearance::global(cx).0 == Appearance::Dark,
    };
    let next = if dark {
        ThemeAppearanceMode::Light
    } else {
        ThemeAppearanceMode::Dark
    };
    change(Change::Appearance(next), cx);
}

/// Validates and saves one preference. The settings store applies it to every window once the
/// file is written, the same path a hand edit of `settings.json` takes.
pub fn change(change: Change, cx: &mut App) {
    if let Err(error) = change.validate() {
        set_error(Some(error), cx);
        return;
    }
    update(move |content| change.apply(content), cx);
}

pub fn update(apply: impl 'static + Send + FnOnce(&mut SettingsContent), cx: &mut App) {
    let Some(SettingsFs(fs)) = cx.try_global::<SettingsFs>().cloned() else {
        set_error(
            Some("Preferences cannot be saved before the settings file is loaded. Restart Quire, then change them again.".into()),
            cx,
        );
        return;
    };
    let done = cx
        .global::<SettingsStore>()
        .update_settings_file_with_completion(fs, move |content, _| apply(content));
    cx.spawn(async move |cx| {
        let error = match done.await {
            Ok(Ok(())) => None,
            Ok(Err(error)) => Some(format!(
                "Preferences changed for this session but could not be saved to {}: {error:#}. Check write access to that folder before restarting.",
                paths::settings_file().display()
            )),
            Err(_) => Some("Preferences could not be saved because Quire is closing. Change them again after restarting.".into()),
        };
        cx.update(|cx| set_error(error, cx));
    })
    .detach();
}

/// The text first read from the settings file, and the Electron profile it was imported from.
#[derive(Debug, PartialEq)]
pub struct Prepared {
    pub text: String,
    pub imported_from: Option<PathBuf>,
}

/// Reads `settings_path`, first creating it from the newest Electron profile in `profiles` when
/// it is missing. A failed import leaves no file, so the next launch tries again.
pub fn prepare(settings_path: &Path, profiles: &[PathBuf], cx: &App) -> Result<Prepared, String> {
    match std::fs::read_to_string(settings_path) {
        Ok(text) => {
            return Ok(Prepared {
                text,
                imported_from: None,
            });
        }
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            return Err(format!(
                "Cannot read {}: {error}. Check that the file is readable; Quire uses its default preferences until then.",
                settings_path.display()
            ));
        }
        Err(_) => {}
    }
    let profile = profiles.iter().find(|profile| profile.is_dir());
    let changes = match profile {
        Some(profile) => rt_import::import(profile).map_err(|error| {
            format!(
                "Cannot import preferences from {}: {error}",
                profile.display()
            )
        })?,
        None => Vec::new(),
    };
    let text = cx
        .global::<SettingsStore>()
        .new_text_for_update(INITIAL_SETTINGS.to_owned(), |content| {
            changes.iter().for_each(|change| change.apply(content))
        })
        .map_err(|error| {
            format!("Cannot prepare the settings file: {error:#}. Reinstall Quire.")
        })?;
    let parent = settings_path.parent().unwrap_or(Path::new("."));
    let temporary = settings_path.with_extension("json.tmp");
    std::fs::create_dir_all(parent)
        .and_then(|()| std::fs::write(&temporary, &text))
        .and_then(|()| std::fs::rename(&temporary, settings_path))
        .map_err(|error| {
            format!(
                "Cannot create {}: {error}. Check write access to {}; Quire uses its default preferences until then.",
                settings_path.display(),
                parent.display()
            )
        })?;
    Ok(Prepared {
        text,
        imported_from: profile.filter(|_| !changes.is_empty()).cloned(),
    })
}

/// Applies the user's settings text, reporting a parse failure without dropping earlier values.
pub fn apply_user_settings(text: &str, cx: &mut App) {
    let result = SettingsStore::update_global(cx, |store, cx| store.set_user_settings(text, cx));
    let error = match result.parse_status {
        settings::ParseStatus::Failed { error } => Some(format!(
            "Cannot read {}: {error}. Fix the file; Quire keeps the last settings that loaded.",
            paths::settings_file().display()
        )),
        _ => None,
    };
    set_error(error, cx);
}

/// Loads the settings file before the first window draws, then follows its changes.
pub fn load(fs: Arc<dyn Fs>, cx: &mut App) {
    cx.set_global(SettingsFs(fs.clone()));
    match prepare(paths::settings_file(), &rt_import::profile_candidates(), cx) {
        Ok(prepared) => {
            apply_user_settings(&prepared.text, cx);
            if let Some(profile) = prepared.imported_from {
                crate::outline::show_notice(
                    format!(
                        "Imported your preferences from {}. Change them in Preferences.",
                        profile.display()
                    ),
                    cx,
                );
            }
        }
        Err(error) => set_error(Some(error), cx),
    }
    watch(fs, cx);
}

/// Applies every later save of the settings file, whoever made it.
pub fn watch(fs: Arc<dyn Fs>, cx: &mut App) {
    let (mut changes, watcher) =
        settings::watch_config_file(cx.background_executor(), fs, paths::settings_file().clone());
    cx.spawn(async move |cx| {
        let _watcher = watcher;
        while let Some(text) = futures::StreamExt::next(&mut changes).await {
            cx.update(|cx| apply_user_settings(&text, cx));
        }
    })
    .detach();
}

/// Keeps the System appearance in step with the window's light or dark setting.
pub fn follow_system_appearance<V: 'static>(window: &mut Window, cx: &mut gpui::Context<V>) {
    cx.observe_window_appearance(window, |_, window, cx| {
        let appearance = SystemAppearance(window.appearance().into());
        if SystemAppearance::global(cx).0 != appearance.0 {
            *SystemAppearance::global_mut(cx) = appearance;
            theme_settings::reload_theme(cx);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests;
