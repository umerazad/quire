//! The command-line steps for moving from the Electron app (`rt`) to Quire and back, and the
//! parts of `rt`'s renderer storage that belong to the app rather than the store: tabs,
//! favorites, and recents per notebook, the AI prompt history, and the synthesis instruction.
//!
//! `quire --migrate-from-rt [<rt data folder>]` migrates and prints the verification report,
//! also saved as `rt-migration-report.md` in Quire's data folder. `quire --rollback-to-rt`
//! reports what `rt` will see; `--publish` also writes Quire's saves to Quire notebooks' synced
//! folders, which cannot be undone. `quire --list-rt-backups <folder>` and
//! `quire --restore-rt-backup <backup> --into <folder>` restore an `rt` backup as a new notebook.
//! None of them opens a window, and none writes to `rt`'s data.

use crate::preferences::{local_storage, rt_import};
use quire_core::workspace::{Destination, Workspace};
use quire_store::{
    notebook::NotebookStore,
    notebook_lock::{NotebookLock, folder_lock, store_lock},
    rt_migration::{self, Migrated, Record},
    sync::format::page_id_for_path,
};
use serde_json::Value;
use std::path::{Path, PathBuf};

const ORIGIN: &str = "file://";
const WORKSPACE_PREFIX: &str = "quire-workspace:";
const AI_HISTORY: &str = "quire.ai-history";
const SYNTHESIS: &str = "quire.synthesis-instruction";
pub const REPORT_FILE: &str = "rt-migration-report.md";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// `rt`'s data folder: the first candidate that holds its profiles or last notebook.
pub fn default_rt_data() -> Option<PathBuf> {
    rt_import::profile_candidates().into_iter().find(|folder| {
        folder.join("profiles.sqlite").is_file() || folder.join("settings.json").is_file()
    })
}

/// `rt`'s workspace for a notebook, with page paths replaced by Quire page identities. Pages
/// `page_id` cannot place are dropped; browser views Quire lacks are dropped too.
pub fn convert_workspace(rt: &Value, page_id: impl Fn(&str) -> Option<String>) -> Workspace {
    let destination = |value: &Value| -> Option<Destination> {
        match value.get("kind")?.as_str()? {
            "page" => Some(Destination::page(page_id(value.get("path")?.as_str()?)?)),
            "tag" => Some(Destination::Tag {
                tag: value.get("tag")?.as_str()?.to_owned(),
            }),
            "browser" => serde_json::from_value(serde_json::json!({
                "kind": "browser",
                "view": value.get("view")?,
            }))
            .ok(),
            _ => None,
        }
    };
    let list = |key: &str| -> Vec<Destination> {
        let mut found: Vec<Destination> = Vec::new();
        for item in rt.get(key).and_then(Value::as_array).into_iter().flatten() {
            if let Some(item) = destination(item)
                && !found.contains(&item)
            {
                found.push(item);
            }
        }
        found
    };
    let strings = |key: &str| -> Vec<String> {
        rt.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect()
    };
    let converted = Workspace {
        expanded: strings("expanded"),
        favorites: list("favorites"),
        recent: list("recent"),
        tabs: list("tabs"),
        active: rt.get("active").and_then(destination),
        oldest_first: strings("oldestFirst")
            .iter()
            .filter_map(|path| page_id(path))
            .collect(),
        ..Workspace::default()
    };
    Workspace::load(&converted.save())
}

/// Writes `workspace` as `notebook`'s entry in the workspace file beside `database`, unless that
/// entry already has tabs. Returns whether it was written.
fn import_workspace(
    database: &Path,
    notebook: &str,
    workspace: &Workspace,
) -> anyhow::Result<bool> {
    let file = crate::navigation::WorkspaceFile::beside(database, notebook);
    if !file.load().tabs.is_empty() {
        return Ok(false);
    }
    file.save(workspace)
        .map(|()| true)
        .map_err(|error| anyhow::anyhow!("{error}"))
}

/// Merges `rt`'s prompt history and synthesis instruction into Quire's AI state file: Quire's
/// own history first, then `rt`'s prompts it lacks, up to the history limit; `rt`'s instruction
/// only when Quire has none.
fn merge_ai_state(file: &Path, history: &[String], synthesis: Option<&str>) -> Result<(), String> {
    let mut state: Value = std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .filter(Value::is_object)
        .unwrap_or_else(|| serde_json::json!({}));
    let mut merged: Vec<String> = state
        .get("history")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect();
    for prompt in history {
        if !merged.contains(prompt) {
            merged.push(prompt.clone());
        }
    }
    merged.truncate(crate::ai::settings::HISTORY_LIMIT);
    state["history"] = merged.into();
    if let Some(synthesis) = synthesis
        && state.get("synthesis").is_none_or(Value::is_null)
    {
        state["synthesis"] = synthesis.into();
    }
    let text = serde_json::to_vec_pretty(&state).map_err(|error| error.to_string())?;
    let temporary = file.with_extension("json.tmp");
    std::fs::create_dir_all(file.parent().unwrap_or(Path::new(".")))
        .and_then(|()| std::fs::write(&temporary, text))
        .and_then(|()| std::fs::rename(&temporary, file))
        .map_err(|error| {
            format!(
                "Cannot save Quire's AI state to {}: {error}. Check write access to Quire's data folder, then run the migration again.",
                file.display()
            )
        })
}

/// Imports what `rt` kept in its renderer storage for the notebooks in `record`, each item once.
/// Returns a line per item for the report.
pub fn import_local_storage(rt_data: &Path, quire_data: &Path, record: &mut Record) -> Vec<String> {
    let storage = rt_data.join("Local Storage/leveldb");
    if !storage.is_dir() {
        return vec!["rt kept no renderer storage, so there are no tabs, favorites, or prompt history to import.".into()];
    }
    let roots: Vec<(String, Vec<String>, Migrated)> = record
        .notebooks
        .iter()
        .map(|migrated| {
            let root = match migrated {
                Migrated::Folder { root, .. } => root,
                Migrated::Quire { folder, .. } => folder,
            };
            let names = rt_migration::rt_names_for(rt_data, quire_data, root)
                .iter()
                .map(|name| format!("{WORKSPACE_PREFIX}{}", name.display()))
                .collect();
            (root.display().to_string(), names, migrated.clone())
        })
        .collect();
    let mut keys: Vec<&str> = roots
        .iter()
        .flat_map(|(_, names, _)| names.iter().map(String::as_str))
        .collect();
    keys.extend([AI_HISTORY, SYNTHESIS]);
    let stored = match local_storage::read(&storage, ORIGIN, &keys) {
        Ok(stored) => stored,
        Err(error) => {
            return vec![format!(
                "rt's renderer storage could not be read: {error} Tabs, favorites, and prompt history were not imported; run the migration again after quitting rt."
            )];
        }
    };
    let mut lines = Vec::new();
    for (root, names, migrated) in &roots {
        let step = format!("workspace:{WORKSPACE_PREFIX}{root}");
        let Some(text) = names.iter().find_map(|name| stored.get(name)) else {
            continue;
        };
        if record.completed.contains(&step) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            lines.push(format!(
                "rt's tabs for {root} are damaged and were not imported."
            ));
            continue;
        };
        let outcome = match migrated {
            Migrated::Folder { root, state } => (|| {
                let _lock = NotebookLock::acquire(&folder_lock(state))?;
                let workspace = convert_workspace(&value, |path| Some(page_id_for_path(path)));
                import_workspace(
                    &state.join("folder.sqlite"),
                    &format!("folder:{}", root.display()),
                    &workspace,
                )
            })(),
            Migrated::Quire {
                database,
                notebook,
                writer,
                ..
            } => (|| {
                let _lock = NotebookLock::acquire(&store_lock(database))?;
                let store = NotebookStore::open(database, notebook, writer)?;
                let workspace = convert_workspace(&value, |path| {
                    store.page_at(path).ok().flatten().map(|(page, _)| page)
                });
                import_workspace(database, notebook, &workspace)
            })(),
        };
        match outcome {
            Ok(written) => {
                record.completed.insert(step);
                lines.push(if written {
                    format!("Tabs, favorites, and recents imported for {root}.")
                } else {
                    format!("Kept Quire's own tabs for {root}.")
                });
            }
            Err(error) => lines.push(format!(
                "Tabs for {root} were not imported: {error:#} They are tried again on the next run."
            )),
        }
    }
    if !record.completed.contains("ai-state") {
        let history: Vec<String> = stored
            .get(AI_HISTORY)
            .and_then(|text| serde_json::from_str::<Vec<Value>>(text).ok())
            .unwrap_or_default()
            .into_iter()
            .filter_map(|item| item.as_str().map(str::to_owned))
            .collect();
        let synthesis = stored.get(SYNTHESIS).map(String::as_str);
        if !history.is_empty() || synthesis.is_some() {
            match merge_ai_state(&quire_data.join("ai-state.json"), &history, synthesis) {
                Ok(()) => {
                    record.completed.insert("ai-state".into());
                    lines.push(format!(
                        "AI prompt history ({} prompts){} imported.",
                        history.len(),
                        if synthesis.is_some() {
                            " and synthesis instruction"
                        } else {
                            ""
                        }
                    ));
                }
                Err(error) => lines.push(error),
            }
        }
    }
    lines
}

/// What `rt`'s AI settings held, for the user to choose again after Quire's own Test.
fn ai_settings_lines(rt_data: &Path) -> Vec<String> {
    let Some(value) = std::fs::read(rt_data.join("ai-settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
    else {
        return Vec::new();
    };
    let field = |key: &str| value.get(key).map(Value::to_string).unwrap_or_default();
    vec![format!(
        "rt's AI settings: backend {}, model {}, writes {}, binaries {}. Choose them in Settings > AI and press Test; Quire turns AI on only after its own Test passes.",
        field("backend"),
        field("model"),
        field("writes"),
        field("paths")
    )]
}

/// Runs the whole migration: the store's part, then renderer storage, and saves the report.
pub fn migrate(
    rt_data: &Path,
    quire_data: &Path,
) -> anyhow::Result<(rt_migration::Report, String)> {
    let report = rt_migration::migrate(&rt_migration::Options {
        rt_data: rt_data.to_owned(),
        quire_data: quire_data.to_owned(),
        now: now(),
    })?;
    let mut record = Record::load(quire_data)?;
    let mut lines = import_local_storage(rt_data, quire_data, &mut record);
    record.save(quire_data)?;
    lines.extend(ai_settings_lines(rt_data));
    lines.push(if quire_data.join("settings.json").exists() {
        "Editor settings and appearance: Quire's own settings file exists, so rt's were not imported again.".into()
    } else {
        "Editor settings and appearance are imported from rt when Quire first starts.".into()
    });
    let mut markdown = report.to_markdown();
    markdown.push_str("\n## App state\n\n");
    for line in &lines {
        markdown.push_str(&format!("- {line}\n"));
    }
    std::fs::write(quire_data.join(REPORT_FILE), &markdown)?;
    Ok((report, markdown))
}

fn value_after<'a>(arguments: &'a [String], flag: &str) -> Option<&'a str> {
    arguments
        .iter()
        .position(|argument| argument == flag)
        .and_then(|index| arguments.get(index + 1))
        .map(String::as_str)
        .filter(|value| !value.starts_with("--"))
}

/// Runs a migration command named in `arguments`, returning the exit code, or None when the
/// arguments name none.
pub fn run_from_arguments(arguments: &[String], quire_data: &Path) -> Option<i32> {
    let has = |flag: &str| arguments.iter().any(|argument| argument == flag);
    let fail = |message: String| {
        eprintln!("{message}");
        2
    };
    if has("--migrate-from-rt") {
        let Some(rt_data) = value_after(arguments, "--migrate-from-rt")
            .map(PathBuf::from)
            .or_else(default_rt_data)
        else {
            return Some(fail("rt's data folder was not found. Pass it after --migrate-from-rt, usually ~/Library/Application Support/Quire on macOS.".into()));
        };
        return Some(match migrate(&rt_data, quire_data) {
            Ok((report, markdown)) => {
                println!("{markdown}");
                println!("Saved to {}.", quire_data.join(REPORT_FILE).display());
                i32::from(!report.verified())
            }
            Err(error) => fail(format!("The migration from rt stopped: {error:#}")),
        });
    }
    if has("--rollback-to-rt") {
        return Some(
            match rt_migration::prepare_rollback(quire_data, has("--publish"), now()) {
                Ok(report) => {
                    println!("{}", report.to_markdown());
                    i32::from(!report.ready())
                }
                Err(error) => fail(format!("Cannot check the return to rt: {error:#}")),
            },
        );
    }
    if has("--list-rt-backups") {
        let Some(folder) = value_after(arguments, "--list-rt-backups") else {
            return Some(fail(
                "Pass rt's backup folder after --list-rt-backups.".into(),
            ));
        };
        for backup in rt_migration::list_backups(Path::new(folder)) {
            println!(
                "{}\t{}\t{:?}\t{}",
                backup.created_at,
                backup.profile,
                backup.kind,
                backup.path.display()
            );
        }
        return Some(0);
    }
    if has("--restore-rt-backup") {
        let (Some(backup), Some(parent)) = (
            value_after(arguments, "--restore-rt-backup"),
            value_after(arguments, "--into"),
        ) else {
            return Some(fail("Pass the backup after --restore-rt-backup and the folder for the restored notebook after --into.".into()));
        };
        return Some(
            match rt_migration::restore_backup(
                Path::new(backup),
                Path::new(parent),
                quire_data,
                now(),
            ) {
                Ok(restored) => {
                    println!(
                        "Restored as {}. Open it with --notebook-folder or from its profile.\n",
                        restored.root.display()
                    );
                    let report = rt_migration::Report {
                        source: PathBuf::from(backup),
                        notebooks: vec![restored.report],
                        ..Default::default()
                    };
                    println!("{}", report.to_markdown());
                    0
                }
                Err(error) => fail(format!("The backup was not restored: {error:#}")),
            },
        );
    }
    None
}

#[cfg(test)]
mod tests;
