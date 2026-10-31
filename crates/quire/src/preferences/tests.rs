use super::*;
use gpui::{Focusable as _, TestAppContext};
use local_storage::{decode_value, snappy, storage_key};
use std::collections::HashMap;
use theme::ActiveTheme as _;

fn init_app(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
}

fn varint(mut value: usize, out: &mut Vec<u8>) {
    while value >= 0x80 {
        out.push(value as u8 | 0x80);
        value >>= 7;
    }
    out.push(value as u8);
}

fn latin1(text: &str) -> Vec<u8> {
    std::iter::once(1).chain(text.bytes()).collect()
}

/// A Chromium write batch in one full log record.
fn log_file(sequence: u64, records: &[(&[u8], Option<&[u8]>)]) -> Vec<u8> {
    let mut batch = sequence.to_le_bytes().to_vec();
    batch.extend((records.len() as u32).to_le_bytes());
    for (key, value) in records {
        batch.push(value.is_some() as u8);
        varint(key.len(), &mut batch);
        batch.extend_from_slice(key);
        if let Some(value) = value {
            varint(value.len(), &mut batch);
            batch.extend_from_slice(value);
        }
    }
    let mut file = vec![0; 4];
    file.extend((batch.len() as u16).to_le_bytes());
    file.push(1);
    file.extend(batch);
    file
}

fn literal_snappy(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    varint(bytes.len(), &mut out);
    for chunk in bytes.chunks(60) {
        out.push(((chunk.len() - 1) << 2) as u8);
        out.extend_from_slice(chunk);
    }
    out
}

type TableRecord<'a> = (&'a [u8], u64, Option<&'a [u8]>);

/// A one-block table whose data block is Snappy-compressed.
fn table_file(records: &[TableRecord]) -> Vec<u8> {
    let block = |entries: Vec<(Vec<u8>, Vec<u8>)>| {
        let mut out = Vec::new();
        for (key, value) in entries {
            varint(0, &mut out);
            varint(key.len(), &mut out);
            varint(value.len(), &mut out);
            out.extend(key);
            out.extend(value);
        }
        out.extend(0u32.to_le_bytes());
        out.extend(1u32.to_le_bytes());
        out
    };
    let data = block(
        records
            .iter()
            .map(|(key, sequence, value)| {
                let mut internal = key.to_vec();
                internal.extend((sequence << 8 | value.is_some() as u64).to_le_bytes());
                (internal, value.unwrap_or_default().to_vec())
            })
            .collect(),
    );
    let compressed = literal_snappy(&data);
    let mut file = compressed.clone();
    file.extend([1, 0, 0, 0, 0]);
    let mut handle = Vec::new();
    varint(0, &mut handle);
    varint(compressed.len(), &mut handle);
    let index = block(vec![(b"\xff".to_vec(), handle)]);
    let index_offset = file.len();
    file.extend(&index);
    file.extend([0, 0, 0, 0, 0]);
    let mut footer = Vec::new();
    varint(0, &mut footer);
    varint(0, &mut footer);
    varint(index_offset, &mut footer);
    varint(index.len(), &mut footer);
    footer.resize(40, 0);
    footer.extend(0xdb47_7524_8b80_fb57u64.to_le_bytes());
    file.extend(footer);
    file
}

fn profile_with_storage(files: &[(&str, Vec<u8>)]) -> tempfile::TempDir {
    let root = tempfile::tempdir().unwrap();
    let storage = root.path().join("Local Storage/leveldb");
    std::fs::create_dir_all(&storage).unwrap();
    for (name, bytes) in files {
        std::fs::write(storage.join(name), bytes).unwrap();
    }
    root
}

#[test]
fn defaults_match_the_reference_editor_settings() {
    let defaults = Preferences::default();
    assert_eq!(defaults.appearance, ThemeAppearanceMode::System);
    assert!(defaults.vim);
    assert!(defaults.format_tags);
    assert_eq!(defaults.capture_target, CaptureTarget::Inbox);
    assert_eq!(defaults.font_family, "Pragmata Pro Mono Liga");
    assert_eq!(defaults.font_size, 16.);
    assert_eq!(defaults.line_spacing, 1.85);
    assert_eq!(defaults.clone().validate(), Ok(defaults));
}

#[gpui::test]
fn the_settings_store_starts_from_quire_defaults(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(|cx| {
        assert_eq!(Preferences::current(cx), Preferences::default());
        let theme = ThemeSettings::get_global(cx);
        assert_eq!(
            theme
                .buffer_font
                .fallbacks
                .as_ref()
                .map(|f| f.fallback_list().to_vec()),
            Some(FONT_FALLBACKS.map(String::from).to_vec())
        );
    });
}

#[test]
fn validation_rejects_what_the_reference_rejects() {
    let invalid = [
        (
            Preferences {
                font_size: f32::NAN,
                ..Default::default()
            },
            "Enter a size from 10 to 40 pixels.",
        ),
        (
            Preferences {
                font_size: 9.,
                ..Default::default()
            },
            "Enter a size from 10 to 40 pixels.",
        ),
        (
            Preferences {
                font_family: " ".into(),
                ..Default::default()
            },
            "Enter a font family of 1 to 150 characters.",
        ),
        (
            Preferences {
                font_family: "x".repeat(151),
                ..Default::default()
            },
            "Enter a font family of 1 to 150 characters.",
        ),
        (
            Preferences {
                line_spacing: 4.,
                ..Default::default()
            },
            "Enter a value from 1.2 to 2.5.",
        ),
    ];
    for (preferences, message) in invalid {
        let error = preferences.validate().unwrap_err();
        assert!(error.ends_with(message), "{error}");
    }
    let edges = Preferences {
        font_family: "  Iosevka  ".into(),
        font_size: 40.,
        line_spacing: 1.2,
        ..Default::default()
    };
    assert_eq!(edges.validate().unwrap().font_family, "Iosevka");
}

#[test]
fn reference_storage_values_import_like_the_reference_store() {
    let saved = r#"{"vim":false,"fontFamily":" Iosevka ","fontSize":20,"lineHeight":1.5,"formatTags":false,"captureTarget":"today"}"#;
    assert_eq!(
        rt_import::editor_preferences(Some(saved.into()), None),
        Preferences {
            vim: false,
            font_family: "Iosevka".into(),
            font_size: 20.,
            line_spacing: 1.5,
            format_tags: false,
            capture_target: CaptureTarget::Today,
            ..Default::default()
        }
    );
    assert_eq!(
        rt_import::editor_preferences(Some(r#"{"fontSize":18}"#.into()), None),
        Preferences {
            font_size: 18.,
            ..Default::default()
        }
    );
    for corrupt in ["not json", r#"{"fontSize":900}"#, r#"{"vim":"yes"}"#] {
        assert_eq!(
            rt_import::editor_preferences(Some(corrupt.into()), Some("false".into())),
            Preferences::default(),
            "{corrupt}"
        );
    }
    assert!(!rt_import::editor_preferences(None, Some("false".into())).vim);
    assert!(rt_import::editor_preferences(None, Some("true".into())).vim);
    assert_eq!(rt_import::appearance("sepia"), None);
}

#[test]
fn snappy_copies_overlap_their_own_output() {
    assert_eq!(
        snappy(&[12, 0x08, b'a', b'b', b'c', 0x15, 3]).unwrap(),
        b"abcabcabcabc"
    );
    assert!(snappy(&[4, 0x05, 9]).is_err());
}

#[test]
fn chromium_values_decode_latin1_and_utf16() {
    assert_eq!(decode_value(&latin1("dark")).as_deref(), Some("dark"));
    let utf16: Vec<u8> = std::iter::once(0)
        .chain("دار".encode_utf16().flat_map(u16::to_le_bytes))
        .collect();
    assert_eq!(decode_value(&utf16).as_deref(), Some("دار"));
    assert_eq!(decode_value(&[2, 1]), None);
}

#[test]
fn local_storage_takes_the_newest_record_across_logs_and_tables() {
    let editor = storage_key("file://", rt_import::EDITOR_KEY, false);
    let appearance = storage_key("file://", rt_import::APPEARANCE_KEY, false);
    let other_origin = storage_key("http://localhost:5173", rt_import::EDITOR_KEY, false);
    let old = latin1(r#"{"fontSize":12}"#);
    let new = latin1(r#"{"fontSize":22}"#);
    let profile = profile_with_storage(&[
        (
            "000005.ldb",
            table_file(&[
                (&appearance, 5, Some(&latin1("light"))),
                (&editor, 6, Some(&new)),
            ]),
        ),
        (
            "000009.log",
            log_file(
                2,
                &[
                    (&editor, Some(&old)),
                    (&other_origin, Some(&latin1("{}"))),
                    (&appearance, None),
                ],
            ),
        ),
        ("LOG", b"ignored".to_vec()),
    ]);
    let read = local_storage::read(
        &profile.path().join("Local Storage/leveldb"),
        "file://",
        &[rt_import::EDITOR_KEY, rt_import::APPEARANCE_KEY],
    )
    .unwrap();
    assert_eq!(
        read,
        HashMap::from([
            (
                rt_import::EDITOR_KEY.to_owned(),
                r#"{"fontSize":22}"#.to_owned()
            ),
            (rt_import::APPEARANCE_KEY.to_owned(), "light".to_owned()),
        ])
    );
    let deleted = profile_with_storage(&[
        (
            "000005.ldb",
            table_file(&[(&appearance, 3, Some(&latin1("light")))]),
        ),
        ("000009.log", log_file(7, &[(&appearance, None)])),
    ]);
    assert!(
        local_storage::read(
            &deleted.path().join("Local Storage/leveldb"),
            "file://",
            &[rt_import::APPEARANCE_KEY]
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn a_reference_profile_becomes_changes_from_quire_defaults() {
    let editor = storage_key("file://", rt_import::EDITOR_KEY, false);
    let profile = profile_with_storage(&[(
        "000003.log",
        log_file(
            1,
            &[(
                &editor,
                Some(&latin1(
                    r#"{"vim":false,"fontSize":16,"lineHeight":2,"captureTarget":"today"}"#,
                )),
            )],
        ),
    )]);
    std::fs::write(
        profile.path().join("appearance.json"),
        r#"{"appearance":"dark"}"#,
    )
    .unwrap();
    assert_eq!(
        rt_import::import(profile.path()).unwrap(),
        vec![
            Change::Appearance(ThemeAppearanceMode::Dark),
            Change::Vim(false),
            Change::CaptureTarget(CaptureTarget::Today),
            Change::LineSpacing(2.),
        ]
    );
    let legacy = profile_with_storage(&[(
        "000003.log",
        log_file(
            1,
            &[(
                &storage_key("file://", "threadnote.editor.vim", false),
                Some(&latin1("false")),
            )],
        ),
    )]);
    assert_eq!(
        rt_import::import(legacy.path()).unwrap(),
        vec![Change::Vim(false)]
    );
}

#[gpui::test]
fn prepare_imports_a_reference_profile_once_and_then_reads_the_file(cx: &mut TestAppContext) {
    init_app(cx);
    let editor = storage_key("file://", rt_import::EDITOR_KEY, false);
    let profile = profile_with_storage(&[(
        "000003.log",
        log_file(
            1,
            &[(
                &editor,
                Some(&latin1(r#"{"fontFamily":"Iosevka","formatTags":false}"#)),
            )],
        ),
    )]);
    let config = tempfile::tempdir().unwrap();
    let path = config.path().join("nested/settings.json");
    let profiles = vec![config.path().join("missing"), profile.path().to_owned()];
    let prepared = cx.update(|cx| prepare(&path, &profiles, cx)).unwrap();
    assert_eq!(prepared.imported_from.as_deref(), Some(profile.path()));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), prepared.text);
    assert!(
        prepared.text.contains("// Quire settings"),
        "{}",
        prepared.text
    );
    cx.update(|cx| {
        apply_user_settings(&prepared.text, cx);
        let current = Preferences::current(cx);
        assert_eq!(current.font_family, "Iosevka");
        assert!(!current.format_tags);
        assert!(current.vim);
        assert_eq!(settings_error(cx), None);
    });
    std::fs::write(&path, "{\"vim_mode\": false}").unwrap();
    let again = cx.update(|cx| prepare(&path, &profiles, cx)).unwrap();
    assert_eq!(again.imported_from, None);
    assert_eq!(again.text, "{\"vim_mode\": false}");
}

#[gpui::test]
fn prepare_without_a_profile_writes_quire_initial_settings(cx: &mut TestAppContext) {
    init_app(cx);
    let config = tempfile::tempdir().unwrap();
    let path = config.path().join("settings.json");
    let prepared = cx.update(|cx| prepare(&path, &[], cx)).unwrap();
    assert_eq!(prepared.text, INITIAL_SETTINGS);
    assert_eq!(prepared.imported_from, None);
    cx.update(|cx| {
        apply_user_settings(&prepared.text, cx);
        assert_eq!(Preferences::current(cx), Preferences::default());
    });
}

#[gpui::test]
fn a_damaged_reference_profile_leaves_no_file_so_import_retries(cx: &mut TestAppContext) {
    init_app(cx);
    let profile = profile_with_storage(&[("000005.ldb", vec![0; 64])]);
    let config = tempfile::tempdir().unwrap();
    let path = config.path().join("settings.json");
    let error = cx
        .update(|cx| prepare(&path, &[profile.path().to_owned()], cx))
        .unwrap_err();
    assert!(
        error.starts_with("Cannot import preferences from"),
        "{error}"
    );
    assert!(error.ends_with("set them again in Preferences."), "{error}");
    assert!(!path.exists());
}

#[gpui::test]
fn invalid_settings_text_reports_and_keeps_the_last_values(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(|cx| {
        apply_user_settings("{\"buffer_font_size\": 20}", cx);
        apply_user_settings("{\"buffer_font_size\": ", cx);
        assert_eq!(Preferences::current(cx).font_size, 20.);
        let error = settings_error(cx).unwrap();
        assert!(error.starts_with("Cannot read"), "{error}");
        assert!(error.ends_with("keeps the last settings that loaded."));
        apply_user_settings("{}", cx);
        assert_eq!(settings_error(cx), None);
    });
}

async fn fake_fs(cx: &mut TestAppContext) -> Arc<fs::FakeFs> {
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(paths::config_dir(), serde_json::json!({}))
        .await;
    cx.update(|cx| cx.set_global(SettingsFs(fs.clone())));
    fs
}

#[gpui::test]
async fn changes_are_saved_to_the_settings_file_and_apply_live(cx: &mut TestAppContext) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), INITIAL_SETTINGS.into())
        .await;
    cx.update(|cx| {
        change(Change::FontSize(22.), cx);
        change(Change::Appearance(ThemeAppearanceMode::Light), cx);
        change(Change::FormatTags(false), cx);
        change(Change::CaptureTarget(CaptureTarget::Today), cx);
        change(Change::FontFamily(" Iosevka ".into()), cx);
        change(Change::LineSpacing(2.), cx);
    });
    cx.run_until_parked();
    cx.update(|cx| {
        let current = Preferences::current(cx);
        assert_eq!(current.font_size, 22.);
        assert_eq!(current.appearance, ThemeAppearanceMode::Light);
        assert!(!current.format_tags);
        assert_eq!(current.capture_target, CaptureTarget::Today);
        assert_eq!(current.font_family, "Iosevka");
        assert_eq!(current.line_spacing, 2.);
        assert_eq!(cx.theme().appearance, Appearance::Light);
        assert_eq!(
            ThemeSettings::get_global(cx).buffer_font_size(cx),
            gpui::px(22.)
        );
    });
    let text = fs.load(paths::settings_file()).await.unwrap();
    assert!(text.contains("// Quire settings"), "{text}");
    assert!(text.contains("\"buffer_font_size\": 22"), "{text}");
    assert!(text.contains("\"capture_target\": \"today\""), "{text}");

    cx.update(|cx| change(Change::FontSize(41.), cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(Preferences::current(cx).font_size, 22.);
        assert!(settings_error(cx).unwrap().contains("10 to 40 pixels"));
    });

    cx.update(|cx| update(restore_defaults, cx));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(Preferences::current(cx), Preferences::default());
        assert_eq!(settings_error(cx), None);
    });
}

#[gpui::test]
async fn toggling_appearance_flips_the_shown_theme(cx: &mut TestAppContext) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), INITIAL_SETTINGS.into())
        .await;
    let before = cx.update(|cx| cx.theme().appearance);
    cx.update(|cx| cx.dispatch_action(&ToggleAppearance));
    cx.run_until_parked();
    let after = cx.update(|cx| cx.theme().appearance);
    assert_ne!(before, after);
    cx.update(|cx| cx.dispatch_action(&UseSystemAppearance));
    cx.run_until_parked();
    cx.update(|cx| {
        assert_eq!(
            Preferences::current(cx).appearance,
            ThemeAppearanceMode::System
        );
        assert_eq!(cx.theme().appearance, before);
    });
}

#[gpui::test]
async fn hand_edits_to_the_watched_file_apply_without_restart(cx: &mut TestAppContext) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), b"{}".to_vec()).await;
    cx.update(|cx| watch(fs.clone(), cx));
    cx.run_until_parked();
    fs.insert_file(paths::settings_file(), b"{\"vim_mode\": false}".to_vec())
        .await;
    cx.run_until_parked();
    cx.update(|cx| assert!(!Preferences::current(cx).vim));
}

fn settings_window(cx: &mut TestAppContext) -> Option<gpui::WindowHandle<view::PreferencesWindow>> {
    cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|window| window.downcast::<view::PreferencesWindow>())
    })
}

fn page(cx: &mut TestAppContext) -> gpui::WindowHandle<crate::editor_host::NotebookWindow> {
    cx.add_window(|window, cx| crate::editor_host::NotebookWindow::new("hello", window, cx))
}

fn type_into(
    settings: gpui::WindowHandle<view::PreferencesWindow>,
    field: view::Field,
    text: &'static str,
    cx: &mut TestAppContext,
) {
    settings
        .update(cx, |view, window, cx| {
            let editor = view.editor(field).clone();
            editor.update(cx, |editor, cx| editor.set_text(text, window, cx));
            window.focus(&editor.focus_handle(cx), cx);
        })
        .unwrap();
    cx.simulate_keystrokes(settings.into(), "enter");
    cx.run_until_parked();
}

#[gpui::test]
async fn the_settings_window_opens_once_and_saves_typed_values(cx: &mut TestAppContext) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), INITIAL_SETTINGS.into())
        .await;
    let page = page(cx);
    let shortcut = if cfg!(target_os = "macos") {
        "cmd-,"
    } else {
        "ctrl-,"
    };
    cx.simulate_keystrokes(page.into(), shortcut);
    cx.run_until_parked();
    cx.simulate_keystrokes(page.into(), shortcut);
    cx.run_until_parked();
    let count = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .filter(|window| window.downcast::<view::PreferencesWindow>().is_some())
            .count()
    });
    assert_eq!(count, 1);
    let settings = settings_window(cx).unwrap();
    type_into(settings, view::Field::FontSize, "20", cx);
    cx.update(|cx| assert_eq!(Preferences::current(cx).font_size, 20.));
    type_into(settings, view::Field::LineSpacing, "4", cx);
    settings
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.editor(view::Field::LineSpacing).read(cx).text(cx),
                "1.85"
            );
            assert!(settings_error(cx).unwrap().contains("1.2 to 2.5"));
        })
        .unwrap();
    cx.update(|cx| apply_user_settings("{\"buffer_font_family\": \"Iosevka\"}", cx));
    cx.run_until_parked();
    settings
        .update(cx, |view, _, cx| {
            assert_eq!(
                view.editor(view::Field::FontFamily).read(cx).text(cx),
                "Iosevka"
            );
            assert_eq!(view.editor(view::Field::FontSize).read(cx).text(cx), "16");
        })
        .unwrap();
    cx.simulate_keystrokes(settings.into(), "escape");
    cx.run_until_parked();
    assert!(settings_window(cx).is_none());
}

#[gpui::test]
async fn turning_vim_off_changes_open_editors_at_once(cx: &mut TestAppContext) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), INITIAL_SETTINGS.into())
        .await;
    let page = page(cx);
    cx.simulate_keystrokes(page.into(), "x");
    page.update(cx, |view, _, cx| assert_eq!(view.source_text(cx), "ello"))
        .unwrap();
    cx.update(|cx| change(Change::Vim(false), cx));
    cx.run_until_parked();
    cx.simulate_keystrokes(page.into(), "x");
    page.update(cx, |view, _, cx| assert_eq!(view.source_text(cx), "xello"))
        .unwrap();
}

#[gpui::test]
async fn format_tags_labels_write_by_tone_and_turns_off_live(cx: &mut TestAppContext) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), INITIAL_SETTINGS.into())
        .await;
    let text = "- An #idea and #Book/fiction with #anything\n";
    let page = cx.add_window(|window, cx| {
        let mut view = crate::editor_host::NotebookWindow::new(text, window, cx);
        view.show_written(window, cx);
        view
    });
    cx.run_until_parked();
    let painted = |page: gpui::WindowHandle<crate::editor_host::NotebookWindow>,
                   cx: &mut TestAppContext| {
        page.update(cx, |view, _, cx| {
            (
                crate::tag_labels::painted(view.second_editor().read(cx), cx),
                view.source_text(cx),
            )
        })
        .unwrap()
    };
    let idea = quire_core::tag_tones::tone_index("idea").unwrap();
    let book = quire_core::tag_tones::tone_index("book").unwrap();
    let neutral = quire_core::tag_tones::TONES.len();
    assert_eq!(
        painted(page, cx),
        (vec![(book, 1), (idea, 1), (neutral, 1)], text.to_owned())
    );
    cx.update(|cx| change(Change::FormatTags(false), cx));
    cx.run_until_parked();
    assert_eq!(painted(page, cx), (Vec::new(), text.to_owned()));
}

#[gpui::test]
async fn tab_walks_settings_in_reading_order_and_keys_work_the_focused_control(
    cx: &mut TestAppContext,
) {
    init_app(cx);
    let fs = fake_fs(cx).await;
    fs.insert_file(paths::settings_file(), INITIAL_SETTINGS.into())
        .await;
    cx.update(view::open);
    cx.run_until_parked();
    let settings = settings_window(cx).unwrap();
    let press =
        |keys: &str, cx: &mut TestAppContext| crate::tab_order::press(settings.into(), keys, cx);
    let focused_field = |cx: &mut TestAppContext| {
        settings
            .update(cx, |view, window, cx| {
                [
                    view::Field::FontFamily,
                    view::Field::FontSize,
                    view::Field::LineSpacing,
                ]
                .into_iter()
                .find(|field| view.editor(*field).focus_handle(cx).is_focused(window))
            })
            .unwrap()
    };
    press("tab tab tab", cx);
    press("enter", cx);
    cx.update(|cx| {
        assert_eq!(
            Preferences::current(cx).appearance,
            ThemeAppearanceMode::Dark,
            "the third stop is the Dark appearance button"
        )
    });
    let vim = cx.update(|cx| Preferences::current(cx).vim);
    press("tab space", cx);
    cx.update(|cx| assert_eq!(Preferences::current(cx).vim, !vim));
    press("tab tab tab enter", cx);
    cx.update(|cx| {
        assert_eq!(
            Preferences::current(cx).capture_target,
            CaptureTarget::Today,
            "Format tags, then Inbox page and Today's daily page follow Vim bindings"
        )
    });
    press("tab", cx);
    assert_eq!(focused_field(cx), Some(view::Field::FontFamily));
    press("tab", cx);
    assert_eq!(focused_field(cx), Some(view::Field::FontSize));
    press("tab", cx);
    assert_eq!(focused_field(cx), Some(view::Field::LineSpacing));
    press("shift-tab shift-tab", cx);
    assert_eq!(focused_field(cx), Some(view::Field::FontFamily));
    press("shift-tab", cx);
    assert_eq!(focused_field(cx), None);
    press("tab", cx);
    assert_eq!(focused_field(cx), Some(view::Field::FontFamily));
    press("tab tab tab", cx);
    assert_eq!(focused_field(cx), None, "Data follows Typography");
    let ai_fields = |cx: &mut TestAppContext| {
        settings
            .update(cx, |view, window, cx| {
                let ai = view.ai.read(cx);
                (
                    ai.binary_field().focus_handle(cx).is_focused(window),
                    ai.model_field().focus_handle(cx).is_focused(window),
                )
            })
            .unwrap()
    };
    let mut seen = Vec::new();
    for _ in 0..60 {
        press("tab", cx);
        let (binary, model) = ai_fields(cx);
        if binary {
            seen.push("binary");
        }
        if model {
            seen.push("model");
        }
        if focused_field(cx).is_some() {
            seen.push("typography");
            break;
        }
    }
    assert_eq!(
        seen,
        ["binary", "model", "typography"],
        "Tab reaches the AI fields in order, past Done, and wraps to the top"
    );
}
