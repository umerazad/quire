//! The reference `tests/ai-settings.test.tsx` and the settings cases of `ai-runner.test.ts`:
//! commands stay off until Test passes, a new backend or binary turns them off, and invalid
//! values are refused with the reference messages. Test runs the fake CLI, never an installed one.

use super::*;
use crate::ai::process::tests::{Bench, fake_cli};
use gpui::TestAppContext;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

pub(crate) async fn init_ai(bench: &Bench, cx: &mut TestAppContext) -> Arc<fs::FakeFs> {
    cx.executor().allow_parking();
    cx.update(|cx| {
        crate::editor_host::init(cx).expect("editor initialization");
        cx.set_global(AiState::load(bench.folder.path().join("ai-state.json")));
        cx.set_global(bench.host("claude-transform.jsonl", &[]));
    });
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(paths::config_dir(), serde_json::json!({}))
        .await;
    fs.insert_file(
        paths::settings_file(),
        crate::preferences::INITIAL_SETTINGS.into(),
    )
    .await;
    cx.update(|cx| cx.set_global(crate::preferences::SettingsFs(fs.clone())));
    cx.update(|cx| crate::preferences::watch(fs.clone(), cx));
    cx.run_until_parked();
    fs
}

pub(crate) fn wait_until(
    cx: &mut TestAppContext,
    what: &str,
    mut done: impl FnMut(&mut TestAppContext) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        cx.run_until_parked();
        if done(cx) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Points Claude Code at the fake CLI and passes Test, as a user would in Settings.
pub(crate) fn pass_test(cx: &mut TestAppContext) {
    let fake = fake_cli().to_string_lossy().into_owned();
    cx.update(|cx| set_binary(Backend::Claude, &fake, cx))
        .unwrap();
    cx.run_until_parked();
    let checked = cx.update(test);
    let result = std::rc::Rc::new(std::cell::RefCell::new(None));
    cx.spawn({
        let result = result.clone();
        async move |_| *result.borrow_mut() = Some(checked.await)
    })
    .detach();
    wait_until(cx, "the backend test", |_| result.borrow().is_some());
    assert_eq!(
        result.take(),
        Some(Ok("Claude Code 2.1.312 works.".to_owned()))
    );
    cx.run_until_parked();
    cx.update(|cx| assert!(is_on(cx)));
}

async fn settings_text(fs: &fs::FakeFs) -> String {
    fs::Fs::load(fs, paths::settings_file()).await.unwrap()
}

#[gpui::test]
async fn commands_stay_off_until_test_passes_and_a_new_backend_turns_them_off(
    cx: &mut TestAppContext,
) {
    let bench = Bench::new();
    let fs = init_ai(&bench, cx).await;
    cx.update(|cx| {
        assert!(!is_on(cx) && !is_tested(cx));
        assert_eq!(set_enabled(true, cx), Err(UNTESTED.into()));
    });
    cx.update(crate::preferences::view::open);
    cx.run_until_parked();
    let settings = cx
        .update(|cx| {
            cx.windows()
                .into_iter()
                .find_map(|window| window.downcast::<crate::preferences::view::PreferencesWindow>())
        })
        .unwrap();
    let section = settings.read_with(cx, |view, _| view.ai.clone()).unwrap();
    let fake = fake_cli().to_string_lossy().into_owned();
    settings
        .update(cx, |_, window, cx| {
            let field = section.read(cx).binary_field().clone();
            field.update(cx, |editor, cx| editor.set_text(fake.clone(), window, cx));
            window.focus(&field.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(settings.into(), "enter");
    cx.run_until_parked();
    assert!(
        settings_text(&fs)
            .await
            .contains(&format!("\"claude_path\": \"{fake}\""))
    );
    cx.update(|cx| assert!(!is_tested(cx)));

    section.update(cx, |section, cx| section.start_test(cx));
    wait_until(cx, "Test to finish", |cx| {
        !section.read_with(cx, |section, _| section.is_testing())
    });
    section.read_with(cx, |section, _| {
        assert_eq!(
            section.status().map(|status| status.to_string()),
            Some("Claude Code 2.1.312 works.".into())
        );
        assert_eq!(section.error(), None);
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(is_tested(cx) && is_on(cx)));
    assert!(settings_text(&fs).await.contains("\"enabled\": true"));
    let saved = std::fs::read_to_string(bench.folder.path().join("ai-state.json")).unwrap();
    assert!(saved.contains("\"backend\": \"claude\""), "{saved}");

    cx.update(|cx| set_backend(Backend::Codex, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(!is_tested(cx) && !is_on(cx));
        assert_eq!(set_enabled(true, cx), Err(UNTESTED.into()));
    });
    cx.update(|cx| set_backend(Backend::Claude, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert!(is_tested(cx) && !is_on(cx), "the tested pair, switched off");
        set_enabled(true, cx).unwrap();
    });
    cx.run_until_parked();
    cx.update(|cx| assert!(is_on(cx)));

    let missing = bench
        .folder
        .path()
        .join("nope")
        .to_string_lossy()
        .into_owned();
    cx.update(|cx| set_binary(Backend::Claude, &missing, cx))
        .unwrap();
    cx.run_until_parked();
    cx.update(|cx| assert!(!is_on(cx) && !is_tested(cx)));
    section.update(cx, |section, cx| section.start_test(cx));
    wait_until(cx, "the failing Test", |cx| {
        !section.read_with(cx, |section, _| section.is_testing())
    });
    section.read_with(cx, |section, _| {
        let error = section.error().unwrap().to_string();
        assert!(
            error.starts_with(&format!("Claude Code was not found at {missing}.")),
            "{error}"
        );
    });
    cx.update(|cx| assert!(!is_tested(cx)));
}

#[gpui::test]
async fn invalid_paths_and_models_are_refused_with_the_reference_messages(cx: &mut TestAppContext) {
    let bench = Bench::new();
    let fs = init_ai(&bench, cx).await;
    cx.update(|cx| {
        assert!(
            set_binary(Backend::Codex, "codex", cx)
                .unwrap_err()
                .contains("must be absolute, such as /opt/homebrew/bin/codex")
        );
        assert!(
            set_model("--yolo", cx)
                .unwrap_err()
                .contains("unsupported characters")
        );
        set_model(" sonnet ", cx).unwrap();
    });
    cx.run_until_parked();
    assert!(settings_text(&fs).await.contains("\"model\": \"sonnet\""));
    cx.update(|cx| assert_eq!(AiSettings::get_global(cx).model, "sonnet"));
}

#[gpui::test]
async fn ai_writes_default_to_review_and_save_as_settings(cx: &mut TestAppContext) {
    let bench = Bench::new();
    let fs = init_ai(&bench, cx).await;
    cx.update(|cx| assert_eq!(AiSettings::get_global(cx).writes, Writes::Review));
    cx.update(|cx| set_writes(Writes::Apply, cx));
    cx.run_until_parked();
    assert!(settings_text(&fs).await.contains("\"writes\": \"apply\""));
    cx.update(|cx| assert_eq!(AiSettings::get_global(cx).writes, Writes::Apply));
    cx.update(|cx| set_writes(Writes::Off, cx));
    cx.run_until_parked();
    cx.update(|cx| assert_eq!(AiSettings::get_global(cx).writes, Writes::Off));
}

#[gpui::test]
async fn prompt_history_keeps_thirty_distinct_prompts_newest_first(cx: &mut TestAppContext) {
    let bench = Bench::new();
    init_ai(&bench, cx).await;
    cx.update(|cx| {
        for index in 0..35 {
            remember(&format!("prompt {index}"), cx);
        }
        remember(" prompt 20 ", cx);
        remember("   ", cx);
        let history = history(cx);
        assert_eq!(history.len(), HISTORY_LIMIT);
        assert_eq!(history[..3], ["prompt 20", "prompt 34", "prompt 33"]);
        assert_eq!(
            history.iter().filter(|item| *item == "prompt 20").count(),
            1
        );
    });
    let reloaded = AiState::load(bench.folder.path().join("ai-state.json"));
    assert_eq!(
        reloaded.saved.history.first().map(String::as_str),
        Some("prompt 20")
    );
}
