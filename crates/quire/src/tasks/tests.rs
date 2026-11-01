use super::*;
use gpui::{AppContext as _, Focusable as _, TestAppContext, UpdateGlobal as _, WindowHandle};
use quire_core::tasks::Edit;
use serde_json::{Value, json};

const MOD: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};
/// 2026-10-29 12:00 UTC.
const NOW: i64 = 1_793_275_200_000;
const MADE: &str = " ➕ [[2026-10-29]]";

fn install(cx: &mut TestAppContext, title: Option<&'static str>) {
    cx.update(|cx| {
        cx.set_global(TaskContext {
            now: Rc::new(|| NOW),
            calendar: Some(DailyCalendar::utc()),
            layout: DailyLayout::default(),
            title: Rc::new(move |_, _| title.map(str::to_owned)),
        });
        crate::editor_host::init(cx).expect("editor initialization");
    });
}

fn set_vim(vim: bool, cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings =
            format!(r#"{{"vim_mode":{vim},"session":{{"restore_unsaved_buffers":false}}}}"#);
        let result = settings::SettingsStore::update_global(cx, |store, cx| {
            store.set_user_settings(&settings, cx)
        });
        assert!(!matches!(
            result.parse_status,
            settings::ParseStatus::Failed { .. }
        ));
    });
}

/// A view of `marked`, whose `|` is the caret, in Write or Source, with Vim on or off.
fn view(marked: &str, write: bool, vim: bool, cx: &mut TestAppContext) -> WindowHandle<Editor> {
    set_vim(vim, cx);
    let cursor = marked.find('|').expect("caret marker");
    let text = marked.replacen('|', "", 1);
    let window = cx.add_window(|window, cx| {
        let buffer = crate::markdown::buffer(text, cx);
        let editor = Editor::for_buffer(buffer, None, window, cx);
        window.focus(&editor.focus_handle(cx), cx);
        editor
    });
    let editor = window.root(cx).unwrap();
    if write {
        cx.update_window(window.into(), |_, window, cx| {
            crate::editor_projection::bind(&editor, window, cx).expect("Write projection")
        })
        .unwrap();
    }
    cx.executor().run_until_parked();
    select(window, cursor..cursor, cx);
    window
}

fn select(window: WindowHandle<Editor>, range: Range<usize>, cx: &mut TestAppContext) {
    window
        .update(cx, |editor, window, cx| {
            editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                selections
                    .select_ranges([MultiBufferOffset(range.start)..MultiBufferOffset(range.end)])
            })
        })
        .unwrap();
    cx.executor().run_until_parked();
}

fn state(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> String {
    cx.executor().run_until_parked();
    window
        .update(cx, |editor, _, cx| {
            let text = editor.text(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            let head = editor
                .selections
                .newest_anchor()
                .head()
                .to_offset(&snapshot)
                .0;
            format!("{}|{}", &text[..head], &text[head..])
        })
        .unwrap()
}

fn text(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> String {
    state(window, cx).replacen('|', "", 1)
}

fn keys(window: WindowHandle<Editor>, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), keys);
    cx.executor().run_until_parked();
}

fn undo(window: WindowHandle<Editor>, vim: bool, cx: &mut TestAppContext) {
    keys(
        window,
        &if vim {
            "escape u".to_owned()
        } else {
            format!("{MOD}-z")
        },
        cx,
    );
}

fn toggle() -> String {
    format!("{MOD}-enter")
}

fn cycle() -> String {
    format!("{MOD}-shift-enter")
}

#[gpui::test]
fn shortcut_makes_a_task_at_the_caret_and_keeps_the_id_and_caret(cx: &mut TestAppContext) {
    install(cx, None);
    assert_eq!(DailyCalendar::utc().date_at(NOW).unwrap(), "2026-10-29");
    for write in [true, false] {
        for vim in [false, true] {
            let window = view("- With| id ^blk002\n- Plain\n", write, vim, cx);
            keys(window, &toggle(), cx);
            assert_eq!(
                state(window, cx),
                format!("- [ ] With| id{MADE} ^blk002\n- Plain\n"),
                "write={write} vim={vim}"
            );
            undo(window, vim, cx);
            assert_eq!(text(window, cx), "- With id ^blk002\n- Plain\n");
            if !write && !vim {
                let window = view("- With id ^blk002|\n- Plain\n", write, vim, cx);
                keys(window, &toggle(), cx);
                assert_eq!(
                    state(window, cx),
                    format!("- [ ] With id{MADE} ^blk002|\n- Plain\n")
                );
            }

            let window = view("- Alpha\n- |\n\nText\n", write, vim, cx);
            keys(window, &toggle(), cx);
            assert_eq!(
                state(window, cx),
                format!("- Alpha\n- [ ] |{MADE}\n\nText\n"),
                "write={write} vim={vim}"
            );
        }
    }
}

#[gpui::test]
fn cycling_and_toggling_change_state_tags_and_stamps_in_single_undo_steps(cx: &mut TestAppContext) {
    install(cx, None);
    for vim in [false, true] {
        let window = view("- [ ] Write re|port #next ^a\n- Child\n", true, vim, cx);
        let steps = [
            (
                toggle(),
                "- [x] Write report #next ✅ 2026-10-29 ^a\n- Child\n",
            ),
            (cycle(), "- [ ] Write report #inbox ^a\n- Child\n"),
            (cycle(), "- [ ] Write report #next ^a\n- Child\n"),
            (cycle(), "- [ ] Write report #waiting ^a\n- Child\n"),
            (cycle(), "- [ ] Write report #someday ^a\n- Child\n"),
            (cycle(), "- [ ] Write report #inbox ^a\n- Child\n"),
            (
                toggle(),
                "- [x] Write report #inbox ✅ 2026-10-29 ^a\n- Child\n",
            ),
            (toggle(), "- [ ] Write report #inbox ^a\n- Child\n"),
        ];
        for (key, expected) in &steps {
            keys(window, key, cx);
            assert_eq!(text(window, cx), *expected, "vim={vim} after {key}");
        }
        for (_, expected) in steps.iter().rev().skip(1) {
            undo(window, vim, cx);
            assert_eq!(text(window, cx), *expected, "vim={vim} undo");
        }
        undo(window, vim, cx);
        assert_eq!(text(window, cx), "- [ ] Write report #next ^a\n- Child\n");
    }
}

#[gpui::test]
fn roam_markers_reopen_and_legacy_done_tags_go(cx: &mut TestAppContext) {
    install(cx, None);
    let window = view(
        "- {{[[DONE]]}} Le|gacy #done ^roam\n- [ ] Parent\n  continuation #waiting\n  ^parent\n",
        true,
        false,
        cx,
    );
    keys(window, &toggle(), cx);
    assert_eq!(
        text(window, cx),
        "- {{[[TODO]]}} Legacy ^roam\n- [ ] Parent\n  continuation #waiting\n  ^parent\n"
    );
    select(window, 30..30, cx);
    keys(window, &cycle(), cx);
    assert_eq!(
        text(window, cx),
        "- {{[[TODO]]}} Legacy ^roam\n- [ ] Parent\n  continuation #someday\n  ^parent\n"
    );
}

#[gpui::test]
fn selections_make_or_change_every_selected_task_as_one_undo_step(cx: &mut TestAppContext) {
    install(cx, None);
    for vim in [false, true] {
        let original = "- A b\n  - c d\n\nPara one\npara two\n\n```\n- code\n```\n# Head\n";
        let window = view(&format!("|{original}"), true, vim, cx);
        select(window, 0..original.len(), cx);
        keys(window, &toggle(), cx);
        assert_eq!(
            text(window, cx),
            format!(
                "- [ ] A b{MADE}\n  - [ ] c d{MADE}\n\n- [ ] Para one{MADE}\n- [ ] para two{MADE}\n\n```\n- code\n```\n# Head\n"
            ),
            "vim={vim}"
        );
        undo(window, vim, cx);
        assert_eq!(text(window, cx), original, "vim={vim}");

        let tasks = "- [ ] One\n- [ ] Two\n- [ ] Three\n";
        let window = view(&format!("|{tasks}"), true, vim, cx);
        select(window, 0..tasks.find("- [ ] Three").unwrap(), cx);
        keys(window, &toggle(), cx);
        assert_eq!(
            text(window, cx),
            "- [x] One ✅ 2026-10-29\n- [x] Two ✅ 2026-10-29\n- [ ] Three\n",
            "vim={vim}"
        );
        undo(window, vim, cx);
        assert_eq!(text(window, cx), tasks);
    }
}

#[gpui::test]
fn vim_visual_line_selections_change_each_selected_line(cx: &mut TestAppContext) {
    install(cx, None);
    let window = view("- [ ] |One\n- Two\n- [ ] Three\n", true, true, cx);
    keys(window, "shift-v j", cx);
    keys(window, &toggle(), cx);
    assert_eq!(
        text(window, cx),
        format!("- [ ] One\n- [ ] Two{MADE}\n- [ ] Three\n")
    );
}

#[gpui::test]
fn refusals_leave_the_page_and_show_why(cx: &mut TestAppContext) {
    install(cx, None);
    let original = "- Notes\n  ```md\n  sam|ple\n  ```\n# Heading\n";
    let window = view(original, true, false, cx);
    keys(window, &toggle(), cx);
    assert_eq!(text(window, cx), original.replace('|', ""));
    assert_eq!(
        cx.update(|cx| crate::outline::notice(cx)).as_deref(),
        Some(quire_core::tasks::NOT_IN_CODE)
    );
    select(
        window,
        original.find("Heading").unwrap() - 1..original.find("Heading").unwrap() - 1,
        cx,
    );
    keys(window, &toggle(), cx);
    assert_eq!(text(window, cx), original.replace('|', ""));
    assert_eq!(
        cx.update(|cx| crate::outline::notice(cx)).as_deref(),
        Some(quire_core::tasks::NOT_HERE)
    );
    let window = view(
        "- [ ] Du|plicate ^same\n- [ ] Duplicate ^same\n",
        true,
        false,
        cx,
    );
    keys(window, &toggle(), cx);
    assert_eq!(
        text(window, cx),
        "- [ ] Duplicate ^same\n- [ ] Duplicate ^same\n"
    );
    assert_eq!(
        cx.update(|cx| crate::outline::notice(cx)).as_deref(),
        Some(quire_core::tasks::DUPLICATE_ID)
    );
}

#[gpui::test]
fn tasks_made_on_todays_daily_page_need_no_created_stamp(cx: &mut TestAppContext) {
    install(cx, Some("2026-10-29"));
    let window = view("- Written today|\n", true, false, cx);
    keys(window, &toggle(), cx);
    assert_eq!(state(window, cx), "- [ ] Written today|\n");
}

#[gpui::test]
fn other_editors_keep_their_own_enter_bindings(cx: &mut TestAppContext) {
    install(cx, None);
    set_vim(false, cx);
    let window = cx.add_window(|window, cx| {
        let buffer = cx.new(|cx| Buffer::local("- plain", cx));
        let editor = Editor::for_buffer(buffer, None, window, cx);
        window.focus(&editor.focus_handle(cx), cx);
        editor
    });
    cx.executor().run_until_parked();
    keys(window, &toggle(), cx);
    assert!(!text(window, cx).contains("[ ]"));
}

/// Converts UTF-16 offsets in the vectors to bytes and back.
struct Offsets {
    bytes: Vec<usize>,
    units: Vec<usize>,
}

impl Offsets {
    fn new(text: &str) -> Self {
        let mut bytes = Vec::new();
        let mut units = vec![0; text.len() + 1];
        let mut unit = 0;
        for (byte, character) in text.char_indices() {
            units[byte] = unit;
            bytes.extend(std::iter::repeat_n(byte, character.len_utf16()));
            unit += character.len_utf16();
        }
        bytes.push(text.len());
        units[text.len()] = unit;
        Self { bytes, units }
    }

    fn byte(&self, value: &Value) -> usize {
        self.bytes[value.as_u64().unwrap() as usize]
    }

    fn edits(&self, edits: &[Edit]) -> Value {
        json!(
            edits
                .iter()
                .map(|edit| json!({"from": self.units[edit.range.start], "to": self.units[edit.range.end], "insert": edit.replacement}))
                .collect::<Vec<_>>()
        )
    }
}

fn outcome<T>(result: Result<T, &str>, ok: impl FnOnce(T) -> Value) -> Value {
    match result {
        Ok(value) => json!({"ok": ok(value)}),
        Err(error) => json!({"error": error}),
    }
}

fn applied(text: &str, edits: &[Edit]) -> String {
    let mut result = text.to_owned();
    for edit in edits.iter().rev() {
        result.replace_range(edit.range.clone(), &edit.replacement);
    }
    result
}

/// The native analyzer feeds the same tasks and commands as the reference analysis.
#[gpui::test]
fn native_analysis_matches_the_frozen_task_vectors(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let vectors: Value =
        serde_json::from_str(include_str!("../../../../tests/fixtures/task-vectors.json")).unwrap();
    let today = vectors["today"].as_str().unwrap();
    let mut failures = Vec::new();
    for case in vectors["cases"].as_array().unwrap() {
        let text = case["text"].as_str().unwrap();
        let offsets = Offsets::new(text);
        let layout = &case["layout"];
        let days = DailyNames::new(
            DailyLayout {
                folder: layout["folder"].as_str().unwrap().into(),
                title: layout["title"].as_str().unwrap().into(),
            },
            DailyCalendar::utc(),
        );
        let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.executor().run_until_parked();
        let snapshot = cx.update(|cx| buffer.read(cx).snapshot());
        let analysis = Analysis::read(&snapshot).expect("parsed page");
        let native_blocks: Vec<Value> = analysis
            .blocks
            .iter()
            .map(|block| {
                json!([
                    block.key,
                    block.kind,
                    offsets.units[block.from],
                    offsets.units[block.content_from],
                    offsets.units[block.content_to]
                ])
            })
            .collect();
        let reference_blocks: Vec<Value> = case["inputs"]["blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|block| {
                json!([
                    block["key"],
                    block["kind"],
                    block["from"],
                    block["contentFrom"],
                    block["contentTo"]
                ])
            })
            .collect();
        if native_blocks != reference_blocks {
            failures.push(format!(
                "{} blocks text={text:?}\nexpected={reference_blocks:?}\nactual={native_blocks:?}",
                case["name"]
            ));
            continue;
        }
        let duplicate_keys = {
            let mut keys: Vec<_> = analysis.blocks.iter().map(|block| &block.key).collect();
            keys.sort_unstable();
            keys.windows(2).any(|pair| pair[0] == pair[1])
        };
        analysis.with_page(case["title"].as_str().unwrap(), |page| {
            let tasks = page.tasks(&days);
            let mut check = |what: String, expected: &Value, actual: Value| {
                if *expected != actual {
                    failures.push(format!(
                        "{} {what} text={text:?}\nexpected={expected}\nactual={actual}",
                        case["name"]
                    ));
                }
            };
            let actual: Vec<Value> = tasks
                .iter()
                .map(|task| {
                    json!({
                        "key": format!("page.md:{}:{}", task.block_key, offsets.units[task.block_from]),
                        "path": "page.md",
                        "blockKey": task.block_key,
                        "id": task.id,
                        "from": offsets.units[task.content.start],
                        "to": offsets.units[task.content.end],
                        "raw": task.raw,
                        "text": task.text,
                        "marker": task.marker,
                        "checked": task.checked,
                        "state": task.state.as_str(),
                        "tags": task.tags,
                        "links": if duplicate_keys { Value::Null } else { json!(task.links) },
                        "created": task.created,
                        "due": task.due,
                        "completed": task.completed,
                    })
                })
                .collect();
            let mut expected = case["tasks"].clone();
            if duplicate_keys {
                for task in expected.as_array_mut().unwrap() {
                    task["links"] = Value::Null;
                }
            }
            check("tasks".into(), &expected, json!(actual));
            for vector in case["taskAt"].as_array().unwrap() {
                let position = offsets.byte(&vector["position"]);
                let shortcut = if vector["action"] == "toggle" {
                    Shortcut::Toggle
                } else {
                    Shortcut::Cycle
                };
                let result = quire_core::tasks::task_at(
                    page,
                    &tasks,
                    &days,
                    today,
                    position,
                    (position, position),
                    shortcut,
                );
                let actual = outcome(result, |edit| {
                    let mut value = json!({"changes": offsets.edits(&edit.edits)});
                    if let Some((anchor, head)) = edit.selection {
                        let after = Offsets::new(&applied(text, &edit.edits));
                        value["selection"] =
                            json!({"anchor": after.units[anchor], "head": after.units[head]});
                    }
                    value
                });
                check(format!("taskAt {vector}"), &vector["result"], actual);
            }
            for vector in case["tasksIn"].as_array().unwrap() {
                let range = offsets.byte(&vector["from"])..offsets.byte(&vector["to"]);
                let shortcut = if vector["action"] == "toggle" {
                    Shortcut::Toggle
                } else {
                    Shortcut::Cycle
                };
                let result =
                    quire_core::tasks::tasks_in(page, &tasks, &days, today, range, shortcut);
                check(
                    format!("tasksIn {vector}"),
                    &vector["result"],
                    outcome(result, |edits| offsets.edits(&edits)),
                );
            }
        });
    }
    assert!(
        failures.is_empty(),
        "{} native task mismatches:\n{}",
        failures.len(),
        failures
            .iter()
            .take(8)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n\n")
    );
}

#[gpui::test]
fn task_commands_act_at_every_caret_in_one_undo_step(cx: &mut TestAppContext) {
    install(cx, None);
    let page = "- [ ] One ^a\n- Two\n- [ ] Three ^c\n";
    let window = view(&format!("|{page}"), true, false, cx);
    window
        .update(cx, |editor, window, cx| {
            editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                selections.select_ranges(
                    [4, 9, 15, 30].map(|at| MultiBufferOffset(at)..MultiBufferOffset(at)),
                )
            })
        })
        .unwrap();
    keys(window, &toggle(), cx);
    assert_eq!(
        text(window, cx),
        format!("- [x] One ✅ 2026-10-29 ^a\n- [ ] Two{MADE}\n- [x] Three ✅ 2026-10-29 ^c\n"),
        "two carets on one task change it once"
    );
    undo(window, false, cx);
    assert_eq!(text(window, cx), page);
}
