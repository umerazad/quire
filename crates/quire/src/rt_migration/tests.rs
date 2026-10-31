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
    assert!(markdown.contains("rt's AI settings (backend Codex, model gpt-5, writes apply, Codex at /opt/bin/codex) are imported when Quire next starts"));
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

#[test]
fn rt_ai_choices_keep_only_values_quire_accepts() {
    let parse = |text: &str| RtAiChoices::parse(&serde_json::from_str(text).unwrap());
    assert_eq!(
        parse(
            r#"{"backend":"claude","paths":{"claude":"claude","codex":" /opt/codex "},"model":"-x","writes":"maybe"}"#
        ),
        RtAiChoices {
            backend: Some(quire_core::ai::Backend::Claude),
            codex_path: Some("/opt/codex".into()),
            ..Default::default()
        },
        "a relative binary, a flag-like model, and an unknown writes value are dropped"
    );
    assert!(parse(r#"{"backend":"gemini","enabled":true}"#).is_empty());
}

mod window {
    use super::*;
    use crate::rt_migration::view::{MigrationWindow, PUBLISH_QUESTION};
    use gpui::{AppContext as _, TestAppContext, WindowHandle};
    use quire_core::ai::{Backend, tools::Writes};
    use settings::Settings as _;
    use std::time::{Duration, Instant};

    async fn init(cx: &mut TestAppContext) {
        cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
        let fs = ::fs::FakeFs::new(cx.executor());
        fs.insert_tree(paths::config_dir(), serde_json::json!({}))
            .await;
        fs.insert_file(
            paths::settings_file(),
            crate::preferences::INITIAL_SETTINGS.into(),
        )
        .await;
        cx.update(|cx| cx.set_global(crate::preferences::SettingsFs(fs)));
    }

    fn settle(window: WindowHandle<MigrationWindow>, cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let deadline = Instant::now() + crate::test_patience::PATIENCE;
        loop {
            cx.executor().advance_clock(Duration::from_millis(100));
            cx.run_until_parked();
            if !window.update(cx, |view, _, _| view.is_running()).unwrap() {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for the step");
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn report(window: WindowHandle<MigrationWindow>, cx: &mut TestAppContext) -> String {
        window
            .update(cx, |view, _, _| {
                assert_eq!(view.error(), None);
                view.report().unwrap_or_default().to_owned()
            })
            .unwrap()
    }

    fn draw(window: WindowHandle<MigrationWindow>, cx: &mut TestAppContext) {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
    }

    #[gpui::test]
    async fn the_window_migrates_imports_ai_settings_checks_the_return_and_restores_backups(
        cx: &mut TestAppContext,
    ) {
        let fixture = fixture();
        init(cx).await;
        let before = tree(&fixture.rt);
        let quire_data = fixture.quire_data.clone();
        let rt_data = fixture.rt_data.clone();
        let window =
            cx.add_window(|window, cx| MigrationWindow::new(quire_data, Some(rt_data), window, cx));
        draw(window, cx);

        window.update(cx, |view, _, cx| view.migrate(cx)).unwrap();
        settle(window, cx);
        let migrated = report(window, cx);
        assert!(migrated.contains("Tabs, favorites, and recents imported for"));
        assert!(
            migrated.contains(
                "Imported rt's AI settings (backend Codex, model gpt-5, writes apply, Codex at /opt/bin/codex). AI stays off until you press Test in Settings > AI."
            ),
            "{migrated}"
        );
        draw(window, cx);
        cx.update(|cx| {
            let ai = crate::ai::settings::AiSettings::get_global(cx);
            assert_eq!(ai.backend, Backend::Codex);
            assert_eq!(ai.codex_path, "/opt/bin/codex");
            assert_eq!(ai.model, "gpt-5");
            assert_eq!(ai.writes, Writes::Apply);
            assert!(!ai.enabled, "AI waits for Quire's own Test");
            assert!(!crate::ai::settings::is_on(cx));
            let work = crate::profiles::get(quire_store::profiles::ProfileId::Work, cx);
            assert!(
                matches!(
                    work.notebook,
                    Some(quire_store::profiles::Notebook::Store { .. })
                ),
                "the profiles migration wrote are the app's now: {work:?}"
            );
            assert_eq!(
                import_ai_settings(&fixture.quire_data, cx),
                None,
                "AI settings are imported once"
            );
        });

        window
            .update(cx, |view, _, cx| view.check_return(cx))
            .unwrap();
        settle(window, cx);
        let check = report(window, cx);
        assert!(!check.is_empty());

        window
            .update(cx, |view, window, cx| view.publish_for_rt(window, cx))
            .unwrap();
        cx.run_until_parked();
        let (question, _) = cx.pending_prompt().expect("publishing asks first");
        assert_eq!(question, PUBLISH_QUESTION);
        cx.simulate_prompt_answer("Cancel");
        cx.run_until_parked();
        assert!(!window.update(cx, |view, _, _| view.is_running()).unwrap());
        assert_eq!(report(window, cx), check, "Cancel runs nothing");

        window
            .update(cx, |view, _, cx| {
                view.list_backups(fixture.rt.join("backups"), cx)
            })
            .unwrap();
        settle(window, cx);
        let backups = window
            .update(cx, |view, _, _| view.backups().to_vec())
            .unwrap();
        assert_eq!(backups.len(), 2, "{backups:?}");
        draw(window, cx);
        let parent = tempfile::tempdir().unwrap();
        let parent_path = fs::canonicalize(parent.path()).unwrap();
        window
            .update(cx, |view, _, cx| {
                view.restore(backups[0].path.clone(), parent_path.clone(), cx)
            })
            .unwrap();
        settle(window, cx);
        assert!(report(window, cx).starts_with("Restored as "));
        let restored = window
            .update(cx, |view, _, _| view.restored().to_vec())
            .unwrap();
        assert_eq!(restored.len(), 1);
        assert!(restored[0].root.starts_with(&parent_path));
        draw(window, cx);
        assert_eq!(tree(&fixture.rt), before, "rt's files never change");
    }

    #[gpui::test]
    async fn migrating_keeps_ai_settings_chosen_in_quire(cx: &mut TestAppContext) {
        let fixture = fixture();
        init(cx).await;
        cx.update(|cx| crate::ai::settings::set_model("sonnet", cx))
            .unwrap();
        cx.run_until_parked();
        let quire_data = fixture.quire_data.clone();
        let rt_data = fixture.rt_data.clone();
        let window =
            cx.add_window(|window, cx| MigrationWindow::new(quire_data, Some(rt_data), window, cx));
        window.update(cx, |view, _, cx| view.migrate(cx)).unwrap();
        settle(window, cx);
        let migrated = report(window, cx);
        assert!(
            migrated.contains("Kept Quire's own AI settings; rt's were not imported."),
            "{migrated}"
        );
        cx.update(|cx| {
            let ai = crate::ai::settings::AiSettings::get_global(cx);
            assert_eq!(ai.model, "sonnet");
            assert_eq!(ai.backend, Backend::Claude);
        });
    }
}
