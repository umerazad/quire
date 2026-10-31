//! Use from a terminal (reference `registerTerminal` and `terminalArgs`): registers Quire's MCP
//! server with the installed Claude Code and Codex CLIs in the user's own settings, through each
//! CLI's `mcp` command, only when the writer presses Register or Remove.

use crate::ai::{
    process::Host,
    settings::{AiSettings, is_on, registered, set_registered},
};
use gpui::{App, Global, IntoElement, ParentElement as _, Styled as _, prelude::*};
use quire_core::ai::{Backend, mcp::SERVER_NAME};
use settings::Settings as _;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};
use ui::{
    Button, Clickable as _, Color, Disableable as _, Label, LabelCommon as _, LabelSize, h_flex,
    v_flex,
};

/// How a terminal session starts the server: this app's binary in MCP mode, pointed at the bridge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub command: PathBuf,
    pub bridge: PathBuf,
}

/// The CLI arguments that add or remove the server, as the reference builds them, with this
/// binary and its `--mcp-server` flag in place of Electron running the server script.
pub fn terminal_args(backend: Backend, server: &Server, remove: bool) -> Vec<String> {
    let command = server.command.to_string_lossy().into_owned();
    let bridge = server.bridge.to_string_lossy().into_owned();
    let strings = |parts: &[&str]| parts.iter().map(|part| (*part).to_owned()).collect();
    match (backend, remove) {
        (Backend::Claude, true) => strings(&["mcp", "remove", SERVER_NAME, "--scope", "user"]),
        (Backend::Claude, false) => {
            let config = serde_json::json!({
                "type": "stdio",
                "command": command,
                "args": [super::SERVER_FLAG],
                "env": { "QUIRE_BRIDGE": bridge },
            });
            let mut args: Vec<String> = strings(&["mcp", "add-json", SERVER_NAME]);
            args.push(config.to_string());
            args.extend(strings(&["--scope", "user"]));
            args
        }
        (Backend::Codex, true) => strings(&["mcp", "remove", SERVER_NAME]),
        (Backend::Codex, false) => {
            let mut args: Vec<String> = strings(&["mcp", "add", SERVER_NAME, "--env"]);
            args.extend([
                format!("QUIRE_BRIDGE={bridge}"),
                "--".into(),
                command,
                super::SERVER_FLAG.into(),
            ]);
            args
        }
    }
}

/// A command line to show the writer, quoted for a POSIX shell.
pub fn shell_line(binary: &str, args: &[String]) -> String {
    std::iter::once(binary)
        .chain(args.iter().map(String::as_str))
        .map(|part| {
            let plain = !part.is_empty()
                && part
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || "_@%+=:,./-".contains(c));
            if plain {
                part.to_owned()
            } else {
                format!("'{}'", part.replace('\'', "'\\''"))
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// What registering did for one CLI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub backend: Backend,
    pub ok: bool,
    pub command: Option<String>,
    pub message: String,
}

/// Adds the server to (or removes it from) each installed CLI, replacing any older entry first,
/// and returns what happened with the commands that now stand registered.
pub async fn register(
    host: &Host,
    binaries: &[(Backend, String)],
    mut terminal: BTreeMap<String, String>,
    server: &Server,
    remove: bool,
) -> (Vec<Outcome>, BTreeMap<String, String>) {
    let mut outcomes = Vec::new();
    for (backend, configured) in binaries {
        let (backend, key) = (*backend, backend.binary().to_owned());
        if remove && !terminal.contains_key(&key) {
            continue;
        }
        let binary = match host.locate(backend, configured).await {
            Ok(binary) => binary,
            Err(message) => {
                outcomes.push(Outcome {
                    backend,
                    ok: false,
                    command: None,
                    message,
                });
                continue;
            }
        };
        let args = terminal_args(backend, server, remove);
        let name = binary
            .file_name()
            .map_or_else(|| key.clone(), |name| name.to_string_lossy().into_owned());
        let command = shell_line(&name, &args);
        if !remove {
            host.execute(&binary, &terminal_args(backend, server, true))
                .await
                .ok();
        }
        let message = match host.execute(&binary, &args).await {
            Ok(_) if remove => {
                terminal.remove(&key);
                Ok(format!("Removed Quire from {}.", backend.name()))
            }
            Ok(_) => {
                terminal.insert(key, command.clone());
                Ok(format!(
                    "{} sessions in a terminal can now use Quire's tools while the app is open.",
                    backend.name()
                ))
            }
            Err(error) => Err(format!(
                "{} could not {} the server: {error}. Run the command in a terminal to see the full error.",
                backend.name(),
                if remove { "remove" } else { "register" }
            )),
        };
        outcomes.push(Outcome {
            backend,
            ok: message.is_ok(),
            command: Some(command),
            message: message.unwrap_or_else(|error| error),
        });
    }
    (outcomes, terminal)
}

/// The last registration's outcome, and the server terminal sessions start.
pub struct Registration {
    pub server: Server,
    pub running: bool,
    pub outcomes: Vec<Outcome>,
    pub error: Option<String>,
}

impl Global for Registration {}

pub fn init(bridge: &Path, cx: &mut App) {
    let command = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("quire"));
    cx.set_global(Registration {
        server: Server {
            command,
            bridge: bridge.to_owned(),
        },
        running: false,
        outcomes: Vec::new(),
        error: None,
    });
}

/// Registers (or removes) the server with every installed CLI, after the writer pressed the
/// button that asks for it.
pub fn start(remove: bool, cx: &mut App) {
    let Some(registration) = cx.try_global::<Registration>() else {
        return;
    };
    if registration.running || (!remove && !is_on(cx)) {
        return;
    }
    let server = registration.server.clone();
    let settings = AiSettings::get_global(cx);
    let binaries: Vec<(Backend, String)> = Backend::ALL
        .iter()
        .map(|backend| (*backend, settings.binary(*backend).to_owned()))
        .collect();
    let host = cx.global::<Host>().clone();
    let terminal = registered(cx);
    cx.global_mut::<Registration>().running = true;
    let registering =
        cx.background_spawn(
            async move { register(&host, &binaries, terminal, &server, remove).await },
        );
    cx.spawn(async move |cx| {
        let (outcomes, terminal) = registering.await;
        cx.update(|cx| {
            let saved = set_registered(terminal, cx).err();
            let registration = cx.global_mut::<Registration>();
            registration.running = false;
            registration.outcomes = outcomes;
            registration.error = saved;
        })
    })
    .detach();
}

fn hint(text: impl Into<gpui::SharedString>) -> Label {
    Label::new(text).size(LabelSize::Small).color(Color::Muted)
}

/// The Use from a terminal part of Settings > AI.
pub fn section(cx: &App) -> impl IntoElement {
    let registration = cx.try_global::<Registration>();
    let running = registration.is_some_and(|registration| registration.running);
    let terminal = registered(cx);
    let on = is_on(cx);
    let outcomes = registration
        .map(|registration| registration.outcomes.clone())
        .unwrap_or_default();
    let error = registration.and_then(|registration| registration.error.clone());
    v_flex()
        .gap_1()
        .pt_2()
        .child(Label::new("Use from a terminal"))
        .child(hint(
            "Registers Quire's tools with Claude Code and Codex in your user settings, so a claude or codex session in any terminal can read and change the notebook open here. The tools answer only while Quire is open; AI writes applies to them too.",
        ))
        .child(
            h_flex()
                .gap_2()
                .child(
                    Button::new(
                        "ai-terminal-register",
                        if terminal.is_empty() {
                            "Register"
                        } else {
                            "Register again"
                        },
                    )
                    .disabled(running || !on)
                    .on_click(|_, _, cx| start(false, cx)),
                )
                .when(!terminal.is_empty(), |row| {
                    row.child(
                        Button::new("ai-terminal-remove", "Remove")
                            .disabled(running)
                            .on_click(|_, _, cx| start(true, cx)),
                    )
                }),
        )
        .when(!on, |section| {
            section.child(hint("Turn on AI commands to register."))
        })
        .children(outcomes.into_iter().map(|outcome| {
            Label::new(outcome.message)
                .size(LabelSize::Small)
                .color(if outcome.ok { Color::Muted } else { Color::Error })
        }))
        .children(terminal.into_iter().map(|(key, command)| {
            let name = Backend::ALL
                .iter()
                .find(|backend| backend.binary() == key)
                .map_or(key.as_str(), |backend| backend.name());
            v_flex()
                .child(hint(format!("{name} ran:")))
                .child(
                    gpui::div()
                        .font_family("Lilex")
                        .text_sm()
                        .child(command),
                )
        }))
        .when_some(error, |section, error| {
            section.child(Label::new(error).color(Color::Error))
        })
}
