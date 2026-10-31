//! What the running Quire does when another launch hands over: open the notebook it named, or
//! bring the window that already shows it forward, or quit for a newer build.

use super::*;
use crate::single_instance::{Launch, Reply};
use quire_store::profiles::Notebook;

/// A newer build to start once this Quire has quit.
#[derive(Default)]
pub(in crate::editor_host) struct Handover(pub Option<Launch>);

impl gpui::Global for Handover {}

/// The notebook a launch's arguments name, if any.
fn named(args: &[String]) -> Result<Option<Notebook>, String> {
    if let Some(options) = OpenOptions::parse(args.to_vec())? {
        return Ok(Some(Notebook::Store {
            database: options.database,
            notebook: options.notebook,
            writer: options.writer,
            folder: options.folder,
        }));
    }
    Ok(FolderOptions::parse(args)?.and_then(|options| {
        std::fs::canonicalize(options.root())
            .ok()
            .map(|root| Notebook::Folder { root })
    }))
}

pub(in crate::editor_host) fn handle_launch(launch: Launch, reply: Reply, cx: &mut App) {
    if reply == Reply::Handover {
        cx.set_global(Handover(Some(launch)));
        cx.quit();
        return;
    }
    let args = launch.args.get(1..).unwrap_or_default();
    let opened = match named(args) {
        Err(error) => Err(error),
        Ok(Some(notebook)) => match super::profiles::showing(&notebook, cx) {
            Some(window) => window
                .update(cx, |_, window, _| window.activate_window())
                .map_err(|error| error.to_string()),
            None => match (
                OpenOptions::parse(args.to_vec()),
                FolderOptions::parse(args),
            ) {
                (Ok(Some(options)), _) => options.open(cx),
                (_, Ok(Some(options))) if options.has_page() => options.open(cx),
                _ => super::open_location(&notebook, cx),
            },
        },
        Ok(None) => {
            if let Some(window) = cx
                .active_window()
                .or_else(|| cx.windows().into_iter().next())
            {
                window
                    .update(cx, |_, window, _| window.activate_window())
                    .ok();
            }
            Ok(())
        }
    };
    cx.activate(true);
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Quire could not open what the other launch asked for: {error}"),
            cx,
        );
    }
}
