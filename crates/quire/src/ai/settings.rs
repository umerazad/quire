//! AI backend settings (reference `AiSettings.tsx` and the settings half of `ai-runner.ts`). The
//! choices users make are Zed settings in `settings.json`, under `quire.ai`; whether the chosen
//! backend and binary passed Test, and the prompt history, are Quire's own state in its data
//! folder, so a hand edit of the settings file cannot turn on an untested backend.

use super::process::{self, Host};
use editor::{Editor, EditorEvent};
use gpui::{App, Context, Entity, Focusable as _, Global, SharedString, Task, Window, prelude::*};
use quire_core::ai::{
    Backend,
    backend::{validate_binary, validate_model},
    tools::Writes,
};
use serde::{Deserialize, Serialize};
use settings::{AiBackendName, AiWritesName, RegisterSetting, Settings, SettingsContent};
use std::path::PathBuf;
use ui::{
    Button, ButtonCommon as _, ButtonStyle, Clickable as _, Color, Disableable as _, Label,
    LabelCommon as _, LabelSize, SwitchField, ToggleButtonGroup, ToggleButtonSimple, ToggleState,
    h_flex, v_flex,
};

/// How many earlier prompts the prompt offers.
pub const HISTORY_LIMIT: usize = 30;
pub const UNTESTED: &str =
    "AI commands stay off until a backend test succeeds. Press Test in Settings > AI first.";

#[derive(RegisterSetting, Clone, Debug, PartialEq)]
pub struct AiSettings {
    pub enabled: bool,
    pub backend: Backend,
    pub claude_path: String,
    pub codex_path: String,
    pub model: String,
    pub writes: Writes,
}

fn backend_of(name: AiBackendName) -> Backend {
    match name {
        AiBackendName::Claude => Backend::Claude,
        AiBackendName::Codex => Backend::Codex,
    }
}

fn name_of(backend: Backend) -> AiBackendName {
    match backend {
        Backend::Claude => AiBackendName::Claude,
        Backend::Codex => AiBackendName::Codex,
    }
}

impl Settings for AiSettings {
    fn from_settings(content: &SettingsContent) -> Self {
        let ai = content
            .quire
            .as_ref()
            .and_then(|quire| quire.ai.clone())
            .unwrap_or_default();
        Self {
            enabled: ai.enabled.unwrap_or(false),
            backend: backend_of(ai.backend.unwrap_or_default()),
            claude_path: ai.claude_path.unwrap_or_default().trim().to_owned(),
            codex_path: ai.codex_path.unwrap_or_default().trim().to_owned(),
            model: ai.model.unwrap_or_default().trim().to_owned(),
            writes: writes_of(ai.writes.unwrap_or_default()),
        }
    }
}

fn writes_of(name: AiWritesName) -> Writes {
    match name {
        AiWritesName::Off => Writes::Off,
        AiWritesName::Review => Writes::Review,
        AiWritesName::Apply => Writes::Apply,
    }
}

/// The AI writes choices with their labels and the reference hints.
const WRITES: [(Writes, &str, &str); 3] = [
    (
        Writes::Off,
        "Off",
        "The AI can read the notebook but has no tools to change it.",
    ),
    (
        Writes::Review,
        "Review",
        "Changes the AI proposes wait in AI changes until you apply them. Nothing changes before that.",
    ),
    (
        Writes::Apply,
        "Apply",
        "Changes apply at once, each as one undo step, and stay listed with Undo. A new page cannot be undone here.",
    ),
];

/// Chooses what AI write tools may do.
pub fn set_writes(writes: Writes, cx: &mut App) {
    if AiSettings::get_global(cx).writes == writes {
        return;
    }
    let name = match writes {
        Writes::Off => AiWritesName::Off,
        Writes::Review => AiWritesName::Review,
        Writes::Apply => AiWritesName::Apply,
    };
    write(move |ai| ai.writes = Some(name), cx);
}

impl AiSettings {
    /// The configured binary for `backend`, empty to search for it.
    pub fn binary(&self, backend: Backend) -> &str {
        match backend {
            Backend::Claude => &self.claude_path,
            Backend::Codex => &self.codex_path,
        }
    }
}

/// The backend and binary of the last successful Test.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tested {
    pub backend: String,
    pub binary: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Saved {
    #[serde(default)]
    tested: Option<Tested>,
    #[serde(default)]
    history: Vec<String>,
    /// The registration command Use from a terminal ran, by CLI binary name.
    #[serde(default)]
    terminal: std::collections::BTreeMap<String, String>,
}

/// The registration commands Use from a terminal ran, by CLI binary name.
pub fn registered(cx: &App) -> std::collections::BTreeMap<String, String> {
    cx.try_global::<AiState>()
        .map(|state| state.saved.terminal.clone())
        .unwrap_or_default()
}

/// Records the registration commands Use from a terminal ran.
pub fn set_registered(
    terminal: std::collections::BTreeMap<String, String>,
    cx: &mut App,
) -> Result<(), String> {
    let state = cx.global_mut::<AiState>();
    state.saved.terminal = terminal;
    state.save()
}

/// Quire's AI state file and what it holds.
pub struct AiState {
    path: PathBuf,
    saved: Saved,
}

impl Global for AiState {}

impl AiState {
    /// Reads the state at `path`; a missing or unreadable file starts empty.
    pub fn load(path: PathBuf) -> Self {
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        Self { path, saved }
    }

    fn save(&self) -> Result<(), String> {
        let text = serde_json::to_vec_pretty(&self.saved).unwrap_or_default();
        let parent = self.path.parent().unwrap_or(std::path::Path::new("."));
        let temporary = self.path.with_extension("json.tmp");
        let write = || -> std::io::Result<()> {
            std::fs::create_dir_all(parent)?;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            std::io::Write::write_all(&mut options.open(&temporary)?, &text)?;
            std::fs::rename(&temporary, &self.path)
        };
        write().map_err(|error| {
            format!(
                "Quire could not save its AI state to {}: {error}. Check write access to that folder, then retry.",
                self.path.display()
            )
        })
    }
}

/// Registers the AI settings and reads Quire's AI state; call after `settings::init`.
pub fn init(cx: &mut App) {
    AiSettings::register(cx);
    if !cx.has_global::<AiState>() {
        #[cfg(not(test))]
        let path = paths::data_dir().join("ai-state.json");
        #[cfg(test)]
        let path =
            std::env::temp_dir().join(format!("quire-ai-state-{}.json", uuid::Uuid::new_v4()));
        cx.set_global(AiState::load(path));
    }
    if !cx.has_global::<Host>() {
        cx.set_global(Host::default());
    }
}

/// Whether the chosen backend and binary passed Test.
pub fn is_tested(cx: &App) -> bool {
    let settings = AiSettings::get_global(cx);
    cx.try_global::<AiState>()
        .and_then(|state| state.saved.tested.as_ref())
        .is_some_and(|tested| {
            tested.backend == settings.backend.binary()
                && tested.binary == settings.binary(settings.backend)
        })
}

/// Whether AI commands run: switched on, for a backend and binary that passed Test.
pub fn is_on(cx: &App) -> bool {
    AiSettings::get_global(cx).enabled && is_tested(cx)
}

/// Earlier prompts, newest first.
pub fn history(cx: &App) -> Vec<String> {
    cx.try_global::<AiState>()
        .map(|state| state.saved.history.clone())
        .unwrap_or_default()
}

/// Puts a typed prompt first in the history. History is a convenience; a failed save only loses it.
pub fn remember(prompt: &str, cx: &mut App) {
    let prompt = prompt.trim();
    if prompt.is_empty() || !cx.has_global::<AiState>() {
        return;
    }
    let state = cx.global_mut::<AiState>();
    state.saved.history.retain(|item| item != prompt);
    state.saved.history.insert(0, prompt.to_owned());
    state.saved.history.truncate(HISTORY_LIMIT);
    state.save().ok();
}

fn record_test(backend: Backend, binary: &str, cx: &mut App) -> Result<(), String> {
    let state = cx.global_mut::<AiState>();
    state.saved.tested = Some(Tested {
        backend: backend.binary().into(),
        binary: binary.into(),
    });
    state.save()
}

/// Writes one change to the `quire.ai` settings.
fn write(apply: impl 'static + Send + FnOnce(&mut settings::QuireAiSettingsContent), cx: &mut App) {
    crate::preferences::update(
        move |content| {
            apply(
                content
                    .quire
                    .get_or_insert_default()
                    .ai
                    .get_or_insert_default(),
            )
        },
        cx,
    );
}

/// Chooses the backend. A different backend has not been tested, so commands turn off.
pub fn set_backend(backend: Backend, cx: &mut App) {
    if AiSettings::get_global(cx).backend == backend {
        return;
    }
    write(
        move |ai| {
            ai.backend = Some(name_of(backend));
            ai.enabled = Some(false);
        },
        cx,
    );
}

/// Sets the binary path for `backend`. A different binary has not been tested, so commands turn
/// off.
pub fn set_binary(backend: Backend, path: &str, cx: &mut App) -> Result<(), String> {
    let path = validate_binary(backend, path)?;
    if AiSettings::get_global(cx).binary(backend) == path {
        return Ok(());
    }
    write(
        move |ai| {
            match backend {
                Backend::Claude => ai.claude_path = Some(path),
                Backend::Codex => ai.codex_path = Some(path),
            }
            ai.enabled = Some(false);
        },
        cx,
    );
    Ok(())
}

pub fn set_model(model: &str, cx: &mut App) -> Result<(), String> {
    let model = validate_model(model)?;
    if AiSettings::get_global(cx).model != model {
        write(move |ai| ai.model = Some(model), cx);
    }
    Ok(())
}

pub fn set_enabled(enabled: bool, cx: &mut App) -> Result<(), String> {
    if enabled && !is_tested(cx) {
        return Err(UNTESTED.into());
    }
    write(move |ai| ai.enabled = Some(enabled), cx);
    Ok(())
}

/// Tests the chosen backend and binary. Success records the test and, the first time this
/// backend and binary pass, turns AI commands on.
pub fn test(cx: &mut App) -> Task<Result<String, String>> {
    let settings = AiSettings::get_global(cx).clone();
    let backend = settings.backend;
    let binary = settings.binary(backend).to_owned();
    let host = cx.global::<Host>().clone();
    let model = settings.model.clone();
    let checked =
        cx.background_spawn(async move { process::test(&host, backend, &binary, &model).await });
    cx.spawn(async move |cx| {
        let version = checked.await?;
        cx.update(|cx| {
            let current = AiSettings::get_global(cx).clone();
            if current.backend != backend || current.binary(backend) != settings.binary(backend) {
                return Err(
                    "The backend or binary changed during the test. Press Test again.".into(),
                );
            }
            let first = !is_tested(cx);
            record_test(backend, settings.binary(backend), cx)?;
            if first {
                write(|ai| ai.enabled = Some(true), cx);
            }
            Ok(format!("{} {version} works.", backend.name()))
        })
    })
}

/// The AI section of Settings.
pub struct AiSection {
    binary: Entity<Editor>,
    model: Entity<Editor>,
    shown: AiSettings,
    status: Option<SharedString>,
    error: Option<SharedString>,
    testing: Option<Task<()>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    Binary,
    Model,
}

impl AiSection {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shown = AiSettings::get_global(cx).clone();
        let field = |text: &str, placeholder: &str, window: &mut Window, cx: &mut Context<Self>| {
            let text = text.to_owned();
            let placeholder = placeholder.to_owned();
            cx.new(|cx| {
                let mut editor = Editor::single_line(window, cx);
                editor.set_placeholder_text(&placeholder, window, cx);
                editor.set_text(text, window, cx);
                editor
            })
        };
        let binary = field(
            shown.binary(shown.backend),
            "Detect automatically",
            window,
            cx,
        );
        let model = field(&shown.model, "CLI default", window, cx);
        for (editor, which) in [(&binary, Field::Binary), (&model, Field::Model)] {
            cx.subscribe_in(
                editor,
                window,
                move |this, _, event: &EditorEvent, _, cx| {
                    if matches!(event, EditorEvent::Blurred) {
                        this.apply(which, cx);
                    }
                },
            )
            .detach();
        }
        cx.observe_global_in::<settings::SettingsStore>(window, |this, window, cx| {
            this.refresh(window, cx)
        })
        .detach();
        #[cfg(unix)]
        cx.observe_global::<crate::notebook_tools::terminal::Registration>(|_, cx| cx.notify())
            .detach();
        cx.observe_global::<AiState>(|_, cx| cx.notify()).detach();
        Self {
            binary,
            model,
            shown,
            status: None,
            error: None,
            testing: None,
        }
    }

    #[cfg(test)]
    pub fn status(&self) -> Option<&SharedString> {
        self.status.as_ref()
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    #[cfg(test)]
    pub fn is_testing(&self) -> bool {
        self.testing.is_some()
    }

    #[cfg(test)]
    pub fn binary_field(&self) -> &Entity<Editor> {
        &self.binary
    }

    fn refresh(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let current = AiSettings::get_global(cx).clone();
        if current == self.shown {
            return;
        }
        let texts = [
            (&self.binary, current.binary(current.backend).to_owned()),
            (&self.model, current.model.clone()),
        ];
        for (editor, text) in texts {
            editor.update(cx, |editor, cx| {
                if editor.text(cx).trim() != text {
                    editor.set_text(text, window, cx);
                }
            });
        }
        if current.backend != self.shown.backend {
            self.status = None;
        }
        self.shown = current;
        cx.notify();
    }

    fn apply(&mut self, field: Field, cx: &mut Context<Self>) {
        let backend = self.shown.backend;
        let result = match field {
            Field::Binary => {
                let typed = self.binary.read(cx).text(cx);
                set_binary(backend, &typed, cx)
            }
            Field::Model => {
                let typed = self.model.read(cx).text(cx);
                set_model(&typed, cx)
            }
        };
        self.error = result.err().map(Into::into);
        cx.notify();
    }

    pub fn start_test(&mut self, cx: &mut Context<Self>) {
        self.apply(Field::Binary, cx);
        self.apply(Field::Model, cx);
        if self.error.is_some() {
            return;
        }
        self.status = None;
        let checked = test(cx);
        self.testing = Some(cx.spawn(async move |this, cx| {
            let result = checked.await;
            this.update(cx, |this, cx| {
                this.testing = None;
                match result {
                    Ok(status) => {
                        this.status = Some(status.into());
                        this.error = None;
                    }
                    Err(error) => this.error = Some(error.into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let focused = [
            (self.binary.clone(), Field::Binary),
            (self.model.clone(), Field::Model),
        ]
        .into_iter()
        .find(|(editor, _)| editor.focus_handle(cx).is_focused(window));
        match focused {
            Some((_, field)) => self.apply(field, cx),
            None => cx.propagate(),
        }
    }

    fn field(label: &'static str, editor: &Entity<Editor>) -> gpui::Div {
        v_flex().gap_1().flex_1().child(Label::new(label)).child(
            gpui::div()
                .px_2()
                .py_1()
                .border_1()
                .rounded_sm()
                .child(editor.clone()),
        )
    }

    /// Use from a terminal, where the app serves the notebook tools.
    fn terminal(cx: &App) -> gpui::AnyElement {
        #[cfg(unix)]
        return crate::notebook_tools::terminal::section(cx).into_any_element();
        #[cfg(not(unix))]
        gpui::Empty.into_any_element()
    }

    fn hint(text: impl Into<SharedString>) -> Label {
        Label::new(text).size(LabelSize::Small).color(Color::Muted)
    }
}

impl Render for AiSection {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shown = self.shown.clone();
        let backend = shown.backend;
        let tested = is_tested(cx);
        let backends = Backend::ALL.map(|choice| {
            ToggleButtonSimple::new(choice.name(), move |_, _, cx| set_backend(choice, cx))
        });
        let selected = Backend::ALL
            .iter()
            .position(|choice| *choice == backend)
            .unwrap_or_default();
        let switch_hint = if tested {
            "Run :ai in Vim, press Command-I (Ctrl-I elsewhere), or choose Ask AI… from the page menu."
                .to_owned()
        } else {
            format!(
                "Off until a test succeeds. Press Test {} below.",
                backend.name()
            )
        };
        let label = if self.testing.is_some() {
            "Testing…".to_owned()
        } else {
            format!("Test {}", backend.name())
        };
        let section = cx.entity().downgrade();
        v_flex()
            .id("ai-settings")
            .key_context("QuireAiSettings")
            .on_action(cx.listener(Self::confirm))
            .gap_2()
            .pt_3()
            .child(Label::new("AI").size(LabelSize::Large))
            .child(
                SwitchField::new(
                    "ai-enabled",
                    Some("AI commands"),
                    Some(switch_hint.into()),
                    ToggleState::from(shown.enabled && tested),
                    move |state, _, cx| {
                        let result = set_enabled(*state == ToggleState::Selected, cx);
                        section
                            .update(cx, |section, cx| {
                                section.error = result.err().map(Into::into);
                                cx.notify();
                            })
                            .ok();
                    },
                )
                .disabled(!tested),
            )
            .child(Label::new("Backend"))
            .child(ToggleButtonGroup::single_row("ai-backend", backends).selected_index(selected))
            .child(
                h_flex()
                    .gap_3()
                    .child(Self::field("Binary path", &self.binary))
                    .child(Self::field("Model", &self.model)),
            )
            .child(Label::new("AI writes"))
            .child(
                ToggleButtonGroup::single_row(
                    "ai-writes",
                    WRITES.map(|(choice, label, _)| {
                        ToggleButtonSimple::new(label, move |_, _, cx| set_writes(choice, cx))
                    }),
                )
                .selected_index(
                    WRITES
                        .iter()
                        .position(|(choice, ..)| *choice == shown.writes)
                        .unwrap_or_default(),
                ),
            )
            .child(Self::hint(
                WRITES
                    .iter()
                    .find(|(choice, ..)| *choice == shown.writes)
                    .map_or("", |(.., hint)| hint),
            ))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("ai-test", label)
                            .style(if tested {
                                ButtonStyle::Subtle
                            } else {
                                ButtonStyle::Filled
                            })
                            .disabled(self.testing.is_some())
                            .on_click(cx.listener(|this, _, _, cx| this.start_test(cx))),
                    )
                    .when_some(self.status.clone(), |row, status| {
                        row.child(Label::new(status).color(Color::Success))
                    }),
            )
            .child(Self::hint(format!(
                "Quire runs the {} command line tool already signed in on this computer. {} Prompts and answers are kept in an ai folder with the notebook's Quire data. Changing the backend or binary turns AI commands off until the next successful Test.",
                backend.name(),
                backend.isolation()
            )))
            .child(Self::terminal(cx))
            .when_some(self.error.clone(), |section, error| {
                section.child(Label::new(error).color(Color::Error))
            })
    }
}

#[cfg(test)]
#[path = "settings_tests.rs"]
pub(crate) mod tests;
