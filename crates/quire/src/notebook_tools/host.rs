//! Answers one checked tool call against the live notebook (reference `notebook-host.ts` and
//! `runTool`). Errors are returned as messages, not raised, so the model sees what to fix.

use super::{bridge::Call, live, read};
use gpui::{App, Task};
use quire_core::ai::tools::{NO_NOTEBOOK, check_input, is_write};
use serde_json::Value;

pub fn answer(call: Call, cx: &mut App) -> Task<Result<Value, String>> {
    let Some(notebook) = live::current(cx) else {
        return Task::ready(Err(NO_NOTEBOOK.into()));
    };
    if let Err(problem) = check_input(&call.name, &call.input) {
        return Task::ready(Err(problem));
    }
    if is_write(&call.name) {
        return super::write::answer(call, notebook, cx);
    }
    cx.spawn(async move |cx| {
        read::run(&call.name, &call.input, &notebook, cx)
            .await
            .map_err(|failure| message(&call.name, failure))
    })
}

/// The message the model reads for a failed call.
pub fn message(name: &str, failure: read::Failure) -> String {
    match failure {
        read::Failure::Tool(message) => message,
        read::Failure::Internal(error) => {
            format!("{name} failed: {error}. Retry, or ask the writer to check the notebook.")
        }
    }
}
