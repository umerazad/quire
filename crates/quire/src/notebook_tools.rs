//! Live notebook tools for AI command line tools (NR29): the bridge the app serves on a private
//! socket while AI commands are on, the stdio MCP server those tools start, and the tool host
//! that answers each call against the open notebook.

pub mod bridge;
pub mod changes;
pub mod host;
pub mod live;
pub mod read;
pub mod review;
pub mod stdio;
pub mod terminal;
pub mod write;

use crate::ai::settings::{AiSettings, AiState, is_on};
use bridge::{CALL_TIMEOUT, NotebookBridge, Options};
use gpui::{App, Global, SharedString};
use settings::{Settings as _, SettingsStore};
use std::{path::PathBuf, rc::Rc};

/// The argument that starts the MCP server instead of the app.
pub const SERVER_FLAG: &str = "--mcp-server";

/// The bridge while AI commands are on, or why it could not start.
#[derive(Default)]
pub struct Served {
    bridge: Option<NotebookBridge>,
    error: Option<SharedString>,
    directory: PathBuf,
}

impl Global for Served {}

fn options(directory: PathBuf) -> Options {
    Options {
        directory,
        enabled: Rc::new(is_on),
        writes: Rc::new(|cx| AiSettings::get_global(cx).writes),
        notebook_open: Rc::new(|cx| live::current(cx).is_some()),
        forward: Rc::new(host::answer),
        timeout: CALL_TIMEOUT,
    }
}

/// Starts or stops the bridge to match whether AI commands are on.
fn sync(cx: &mut App) {
    let on = is_on(cx);
    let served = cx.global::<Served>();
    match (on, served.bridge.is_some()) {
        (true, false) => {
            let started = NotebookBridge::start(options(served.directory.clone()), cx);
            let served = cx.global_mut::<Served>();
            match started {
                Ok(bridge) => {
                    served.bridge = Some(bridge);
                    served.error = None;
                }
                Err(error) => served.error = Some(error.into()),
            }
        }
        (false, true) => {
            let served = cx.global_mut::<Served>();
            served.bridge = None;
            served.error = None;
        }
        _ => {}
    }
}

/// Serves the tools from `directory` (Quire's data folder) while AI commands are on; call after
/// the AI settings are registered.
pub fn init(directory: PathBuf, cx: &mut App) {
    changes::init(cx);
    terminal::init(&directory.join(bridge::FILE_NAME), cx);
    cx.on_action(|_: &crate::ai::ReviewAiChanges, cx| review::open(cx));
    cx.bind_keys([gpui::KeyBinding::new(
        "escape",
        menu::Cancel,
        Some(review::CONTEXT),
    )]);
    cx.set_global(Served {
        directory,
        ..Served::default()
    });
    cx.observe_global::<SettingsStore>(sync).detach();
    cx.observe_global::<AiState>(sync).detach();
    cx.on_app_quit(|cx| {
        cx.global_mut::<Served>().bridge = None;
        async {}
    })
    .detach();
    sync(cx);
}

/// Where the bridge lives: Quire's data folder, or a fresh temporary one in tests.
pub fn directory() -> PathBuf {
    #[cfg(not(test))]
    return paths::data_dir().clone();
    #[cfg(test)]
    std::env::temp_dir().join(format!("quire-bridge-{}", uuid::Uuid::new_v4()))
}

#[cfg(test)]
mod stdio_tests;

#[cfg(test)]
mod read_tests;

#[cfg(test)]
mod terminal_tests;
