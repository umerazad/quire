//! Answers one checked tool call against the live notebook (reference `notebook-host.ts` and
//! `runTool`). Errors are returned as messages, not raised, so the model sees what to fix.

use super::{bridge::Call, live, read};
use gpui::{App, Global, Task};
use quire_core::ai::tools::{NO_NOTEBOOK, check_input, is_write};
use serde_json::Value;
use std::collections::HashMap;

/// Runs started by Tidy this day, by run ID, with the day's page they may change. Quire enforces
/// Tidy's limits here, whatever the prompt says.
#[derive(Default)]
struct TidyRuns(HashMap<String, String>);

impl Global for TidyRuns {}

/// Limits run `run` to Tidy's changes on page `page`.
pub fn start_tidy(run: &str, page: &str, cx: &mut App) {
    cx.default_global::<TidyRuns>()
        .0
        .insert(run.to_owned(), page.to_owned());
}

pub fn end_tidy(run: &str, cx: &mut App) {
    if let Some(runs) = cx.try_global::<TidyRuns>()
        && runs.0.contains_key(run)
    {
        cx.global_mut::<TidyRuns>().0.remove(run);
    }
}

/// The day's page a Tidy run may change, or None for any other run.
pub fn tidy_day(run: Option<&str>, cx: &App) -> Option<String> {
    cx.try_global::<TidyRuns>()?.0.get(run?).cloned()
}

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
