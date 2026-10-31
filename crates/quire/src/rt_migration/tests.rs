//! The app's part of the migration on a copy of `tests/fixtures/rt-state`, with renderer storage
//! written in Chromium's LevelDB log format: tabs, favorites, recents, and AI prompt history.

use super::*;
use quire_core::workspace::BrowserView;
use std::{collections::BTreeMap, fs};

struct Fixture {
    _dir: tempfile::TempDir,
    rt: PathBuf,
    rt_data: PathBuf,
    quire_data: PathBuf,
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut found = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.insert(
                    path.strip_prefix(root).unwrap().display().to_string(),
                    fs::read(&path).unwrap(),
                );
            }
        }
    }
    found
}

fn varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

/// One Chromium write batch in a LevelDB log, with UTF-16 values as Chromium stores them.
fn leveldb_log(records: &[(&str, String)]) -> Vec<u8> {
    let mut batch = 1u64.to_le_bytes().to_vec();
    batch.extend((records.len() as u32).to_le_bytes());
    for (key, value) in records {
        let key = local_storage::storage_key(ORIGIN, key, false);
        let value: Vec<u8> = std::iter::once(0)
            .chain(value.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        batch.push(1);
        varint(key.len(), &mut batch);
        batch.extend(key);
        varint(value.len(), &mut batch);
        batch.extend(value);
    }
    let mut file = vec![0; 4];
    file.extend((batch.len() as u16).to_le_bytes());
    file.push(1);
    file.extend(batch);
    file
}

/// A copy of the `rt` fixture whose profiles point at the copy and whose renderer storage holds
/// tabs for both migrated notebooks and a prompt history.
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let rt = base.join("rt");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rt-state"),
        &rt,
    );
    let rt_data = rt.join("app-data");
    let prefix = rt.display().to_string();
    let profiles = sqlez::connection::Connection::try_open_file(
        rt_data.join("profiles.sqlite").to_str().unwrap(),
    )
    .unwrap();
    profiles
        .exec_bound::<(&str, &str)>(
            "UPDATE profiles SET root=replace(root,?1,?2), backupDirectory=replace(backupDirectory,?1,?2)",
        )
        .unwrap()(("/rt-fixture", prefix.as_str()))
    .unwrap();
    drop(profiles);
    let settings = rt_data.join("settings.json");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(&settings, text.replace("/rt-fixture", &prefix)).unwrap();
    let folder = serde_json::json!({
        "expanded": ["Projects"],
        "favorites": [{"kind": "page", "path": "Ideas.md"}],
        "recent": [{"kind": "page", "path": "Plans.md"}, {"kind": "page", "path": "Gone.md"}],
        "tabs": [
            {"kind": "page", "path": "Plans.md"},
            {"kind": "tag", "tag": "project"},
            {"kind": "browser", "view": "tasks"},
            {"kind": "browser", "view": "ai"}
        ],
        "active": {"kind": "page", "path": "Plans.md"},
        "positions": {"page:Plans.md": 120}
    });
    let quire = serde_json::json!({
        "expanded": [],
        "favorites": [],
        "recent": [],
        "tabs": [{"kind": "page", "path": "Welcome.md"}, {"kind": "page", "path": "Tasks/Today.md"}],
        "active": {"kind": "page", "path": "Tasks/Today.md"},
        "positions": {}
    });
    let storage = rt_data.join("Local Storage/leveldb");
    fs::create_dir_all(&storage).unwrap();
    fs::write(
        storage.join("000003.log"),
        leveldb_log(&[
            (
                &format!("{WORKSPACE_PREFIX}{prefix}/folder-notebook"),
                folder.to_string(),
            ),
            (
                &format!("{WORKSPACE_PREFIX}{prefix}/quire-notebook"),
                quire.to_string(),
            ),
            (
                AI_HISTORY,
                r#"["Summarize this page","مرحبا 🙂 translate"]"#.into(),
            ),
            (SYNTHESIS, "List the decisions.".into()),
        ]),
    )
    .unwrap();
    Fixture {
        _dir: dir,
        quire_data: base.join("quire"),
        rt,
        rt_data,
    }
}

#[test]
fn migration_imports_tabs_favorites_and_prompt_history_once_and_reads_rt_only() {
    let fixture = fixture();
    fs::create_dir_all(&fixture.quire_data).unwrap();
    fs::write(
        fixture.quire_data.join("ai-state.json"),
        r#"{"history":["Summarize this page","Quire's own"],"terminal":{"claude":"claude mcp add"}}"#,
    )
    .unwrap();
    let before = tree(&fixture.rt);
    let (report, markdown) = migrate(&fixture.rt_data, &fixture.quire_data).unwrap();
    assert_eq!(tree(&fixture.rt), before, "rt's files changed");
    assert_eq!(report.notebooks.len(), 2);
    assert!(markdown.contains("Tabs, favorites, and recents imported for"));
    assert!(markdown.contains("AI prompt history (2 prompts) and synthesis instruction imported."));
    assert!(markdown.contains("rt's AI settings: backend \"codex\""));
    assert_eq!(
        fs::read_to_string(fixture.quire_data.join(REPORT_FILE)).unwrap(),
        markdown
    );

    let folder_root = fixture.rt.join("folder-notebook");
    let state_folder =
        quire_store::folder_notebook::state_folder_in(&fixture.quire_data, &folder_root);
    let folder = crate::navigation::WorkspaceFile::beside(
        &state_folder.join("folder.sqlite"),
        &format!("folder:{}", folder_root.display()),
    )
    .load();
    let plans = Destination::page(page_id_for_path("Plans.md"));
    assert_eq!(
        folder.tabs,
        [
            plans.clone(),
            Destination::Tag {
                tag: "project".into()
            },
            Destination::Browser {
                view: BrowserView::Tasks
            },
        ],
        "the AI browser has no Quire counterpart"
    );
    assert_eq!(folder.active, Some(plans));
    assert_eq!(
        folder.favorites,
        [Destination::page(page_id_for_path("Ideas.md"))]
    );
    assert_eq!(folder.expanded, ["Projects"]);

    let record = Record::load(&fixture.quire_data).unwrap();
    let Some(Migrated::Quire {
        database,
        notebook,
        writer,
        ..
    }) = record.quire(&fixture.rt.join("quire-notebook"))
    else {
        panic!("{record:#?}");
    };
    let quire = crate::navigation::WorkspaceFile::beside(database, notebook).load();
    let store = NotebookStore::open(database, notebook, writer).unwrap();
    let today = store.page_at("Tasks/Today.md").unwrap().unwrap().0;
    assert_eq!(quire.tabs.len(), 2);
    assert_eq!(quire.active, Some(Destination::page(today)));
    drop(store);

    let state_file = fs::read(fixture.quire_data.join("ai-state.json")).unwrap();
    let state: Value = serde_json::from_slice(&state_file).unwrap();
    assert_eq!(
        state["history"],
        serde_json::json!(["Summarize this page", "Quire's own", "مرحبا 🙂 translate"]),
        "Quire's prompts first, rt's added once"
    );
    assert_eq!(state["synthesis"], "List the decisions.");
    assert_eq!(
        state["terminal"]["claude"], "claude mcp add",
        "other AI state kept"
    );

    let workspaces = [
        state_folder.join("quire-workspace.json"),
        database.with_file_name("quire-workspace.json"),
    ];
    let saved = workspaces.each_ref().map(|file| fs::read(file).unwrap());
    let (_, again) = migrate(&fixture.rt_data, &fixture.quire_data).unwrap();
    assert!(
        !again.contains("Tabs, favorites, and recents imported"),
        "{again}"
    );
    assert_eq!(
        fs::read(fixture.quire_data.join("ai-state.json")).unwrap(),
        state_file
    );
    assert_eq!(workspaces.map(|file| fs::read(file).unwrap()), saved);
    assert_eq!(tree(&fixture.rt), before);
}

#[test]
fn a_renamed_page_and_unknown_destinations_are_dropped_from_converted_tabs() {
    let rt = serde_json::json!({
        "tabs": [
            {"kind": "page", "path": "Kept.md"},
            {"kind": "page", "path": "Renamed.md"},
            {"kind": "browser", "view": "readme"},
            {"kind": "other"},
            {"kind": "page", "path": "Kept.md"}
        ],
        "oldestFirst": ["Kept.md", "Renamed.md"]
    });
    let workspace = convert_workspace(&rt, |path| (path == "Kept.md").then(|| "p-kept".into()));
    assert_eq!(workspace.tabs, [Destination::page("p-kept")]);
    assert_eq!(workspace.oldest_first, ["p-kept"]);
    assert_eq!(
        convert_workspace(&Value::Null, |_| None),
        Workspace::default()
    );
}

#[test]
fn command_line_steps_explain_missing_values_and_ignore_other_arguments() {
    let data = tempfile::tempdir().unwrap();
    let arguments = |values: &[&str]| {
        values
            .iter()
            .map(|value| value.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        run_from_arguments(&arguments(&["--notebook-folder", "/notes"]), data.path()),
        None
    );
    assert_eq!(
        run_from_arguments(&arguments(&["--list-rt-backups"]), data.path()),
        Some(2)
    );
    assert_eq!(
        run_from_arguments(&arguments(&["--restore-rt-backup", "/b"]), data.path()),
        Some(2)
    );
    assert_eq!(
        run_from_arguments(
            &arguments(&[
                "--migrate-from-rt",
                data.path().join("nowhere").to_str().unwrap()
            ]),
            data.path()
        ),
        Some(2)
    );
    assert_eq!(
        run_from_arguments(&arguments(&["--rollback-to-rt"]), data.path()),
        Some(0),
        "nothing migrated means nothing to return"
    );
}
