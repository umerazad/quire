use super::*;
use gpui::{AppContext as _, Focusable, TestAppContext, UpdateGlobal as _, WindowHandle};

const MOD: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

/// A Write view of `marked`, whose `|` is the caret, with Vim on or off.
pub(super) fn write_view(marked: &str, vim: bool, cx: &mut TestAppContext) -> WindowHandle<Editor> {
    cx.update(|cx| {
        crate::editor_host::init(cx).expect("editor initialization");
        let result = settings::SettingsStore::update_global(cx, |store, cx| {
            store.set_user_settings(
                &format!(r#"{{"vim_mode":{vim},"session":{{"restore_unsaved_buffers":false}}}}"#),
                cx,
            )
        });
        assert!(!matches!(
            result.parse_status,
            settings::ParseStatus::Failed { .. }
        ));
    });
    let cursor = marked.find('|').expect("caret marker");
    let text = marked.replacen('|', "", 1);
    let window = cx.add_window(|window, cx| {
        let buffer = crate::markdown::buffer(text, cx);
        let editor = Editor::for_buffer(buffer, None, window, cx);
        window.focus(&editor.focus_handle(cx), cx);
        editor
    });
    let editor = window.root(cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        crate::editor_projection::bind(&editor, window, cx).expect("Write projection")
    })
    .unwrap();
    cx.executor().run_until_parked();
    place(window, cursor, cx);
    window
}

fn place(window: WindowHandle<Editor>, cursor: usize, cx: &mut TestAppContext) {
    window
        .update(cx, |editor, window, cx| {
            editor.change_selections(SelectionEffects::no_scroll(), window, cx, |selections| {
                selections.select_ranges([MultiBufferOffset(cursor)..MultiBufferOffset(cursor)])
            })
        })
        .unwrap();
    cx.executor().run_until_parked();
}

pub(super) fn state(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> String {
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

pub(super) fn keys(window: WindowHandle<Editor>, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), keys);
    cx.executor().run_until_parked();
}

fn expect(window: WindowHandle<Editor>, steps: &[(&str, &str)], cx: &mut TestAppContext) {
    for (input, expected) in steps {
        keys(window, input, cx);
        assert_eq!(state(window, cx), *expected, "after {input:?}");
    }
}

pub(super) fn displayed(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |editor, window, cx| {
            editor.snapshot(window, cx).display_snapshot.text()
        })
        .unwrap()
}

fn undo(vim: bool) -> String {
    if vim { "u".into() } else { format!("{MOD}-z") }
}

#[gpui::test]
fn enter_continues_splits_and_ends_list_items(cx: &mut TestAppContext) {
    for vim in [false, true] {
        let insert = if vim { "i " } else { "" };
        let window = write_view("- [x] Act|ion ^t\n- Next\n", vim, cx);
        expect(
            window,
            &[
                (
                    &format!("{insert}enter"),
                    "- [x] Act ^t\n- [ ] |ion\n- Next\n",
                ),
                ("x", "- [x] Act ^t\n- [ ] x|ion\n- Next\n"),
            ],
            cx,
        );
        if vim {
            // Vim undoes a whole Insert session, as Vim does.
            keys(window, "escape", cx);
        } else {
            keys(window, &undo(vim), cx);
            assert_eq!(
                state(window, cx).replace('|', ""),
                "- [x] Act ^t\n- [ ] ion\n- Next\n"
            );
        }
        keys(window, &undo(vim), cx);
        assert_eq!(
            state(window, cx).replace('|', ""),
            "- [x] Action ^t\n- Next\n"
        );

        let window = write_view("- Parent\n  - Kid|\n", vim, cx);
        expect(
            window,
            &[
                (&format!("{insert}enter"), "- Parent\n  - Kid\n  - |\n"),
                ("enter", "- Parent\n  - Kid\n- |\n"),
                ("enter", "- Parent\n  - Kid\n|\n"),
                ("enter", "- Parent\n  - Kid\n\n|\n"),
            ],
            cx,
        );

        // Vim's Normal-mode caret rests on the last visible character, so `A` appends.
        let append = if vim { "shift-a " } else { "" };
        let window = write_view("- Parent ^p|\n  - Kid\n", vim, cx);
        expect(
            window,
            &[(&format!("{append}enter"), "- Parent ^p\n  - |\n  - Kid\n")],
            cx,
        );

        let window = write_view("Plain| paragraph ^p\n", vim, cx);
        expect(
            window,
            &[(&format!("{insert}enter"), "Plain\n|paragraph ^p\n")],
            cx,
        );
        let window = write_view("Paragraph| ^p\n", vim, cx);
        expect(
            window,
            &[(&format!("{append}enter"), "Paragraph ^p\n|\n")],
            cx,
        );
    }
}

#[gpui::test]
fn enter_keeps_code_inside_list_items_literal(cx: &mut TestAppContext) {
    let window = write_view(
        "- Parent ^p\n  ```js\n  con|sole.log(1);\n  ```\n",
        false,
        cx,
    );
    keys(window, "enter", cx);
    assert!(
        state(window, cx).starts_with("- Parent ^p\n  ```js\n  con\n"),
        "{}",
        state(window, cx)
    );
}

#[gpui::test]
fn numbered_lists_continue_and_renumber_in_one_undo_step(cx: &mut TestAppContext) {
    let window = write_view("1. One|\n2. Two\n   - Kid\n", false, cx);
    expect(
        window,
        &[
            ("enter", "1. One\n2. |\n3. Two\n   - Kid\n"),
            ("t", "1. One\n2. t|\n3. Two\n   - Kid\n"),
        ],
        cx,
    );
    keys(window, &undo(false), cx);
    keys(window, &undo(false), cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        "1. One\n2. Two\n   - Kid\n"
    );
}

#[gpui::test]
fn tab_and_vim_shift_move_subtrees(cx: &mut TestAppContext) {
    let window = write_view(
        "- Parent ^p\n- Sib|ling ^s\n  - Leaf ^l\n- After ^a\n",
        false,
        cx,
    );
    expect(
        window,
        &[
            (
                "tab",
                "- Parent ^p\n  - Sib|ling ^s\n    - Leaf ^l\n- After ^a\n",
            ),
            (
                "shift-tab",
                "- Parent ^p\n- Sib|ling ^s\n  - Leaf ^l\n- After ^a\n",
            ),
        ],
        cx,
    );
    let window = write_view("- Abc\n-|", false, cx);
    expect(window, &[("tab", "- Abc\n  - |")], cx);

    let window = write_view("- One\n- T|wo\n  - Kid\n- Three\n", true, cx);
    expect(
        window,
        &[
            ("> >", "- One\n  - |Two\n    - Kid\n- Three\n"),
            ("j j 0", "- One\n  - Two\n    - Kid\n- |Three\n"),
            (".", "- One\n  - Two\n    - Kid\n  - |Three\n"),
            ("< <", "- One\n  - Two\n    - Kid\n- |Three\n"),
        ],
        cx,
    );
    keys(window, "u", cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        "- One\n  - Two\n    - Kid\n  - Three\n"
    );
}

#[gpui::test]
fn blocks_move_with_children(cx: &mut TestAppContext) {
    for vim in [false, true] {
        let window = write_view("- A\n- B|\n  - B1\n- C\n", vim, cx);
        expect(
            window,
            &[
                ("alt-up", "- B|\n  - B1\n- A\n- C\n"),
                ("alt-up", "- B|\n  - B1\n- A\n- C\n"),
                ("alt-shift-down", "- A\n- B|\n  - B1\n- C\n"),
                (&format!("{MOD}-shift-down"), "- A\n- C\n- B|\n  - B1\n"),
            ],
            cx,
        );
        keys(window, &undo(vim), cx);
        assert_eq!(
            state(window, cx).replace('|', ""),
            "- A\n- B\n  - B1\n- C\n"
        );
    }
}

#[gpui::test]
fn backspace_and_delete_merge_or_remove_empty_bullets(cx: &mut TestAppContext) {
    for vim in [false, true] {
        let insert = if vim { "i " } else { "" };
        let window = write_view("- |\n  - Kid\n    - Grand\n- Next\n", vim, cx);
        expect(
            window,
            &[(&format!("{insert}backspace"), "- |Kid\n  - Grand\n- Next\n")],
            cx,
        );
        let window = write_view("- Alpha\n- |\n- Beta\n", vim, cx);
        expect(
            window,
            &[(&format!("{insert}delete"), "- Alpha\n- |Beta\n")],
            cx,
        );
        let window = write_view("- |\n", vim, cx);
        expect(window, &[(&format!("{insert}backspace"), "- |\n")], cx);
        let window = write_view("- Alpha\n- |Beta\n  - Kid\n", vim, cx);
        expect(
            window,
            &[(&format!("{insert}backspace"), "- Alpha|Beta\n  - Kid\n")],
            cx,
        );
        let window = write_view("- Alpha|\n- [ ] Beta\n", vim, cx);
        expect(
            window,
            &[(&format!("{insert}delete"), "- Alpha|Beta\n")],
            cx,
        );
        let window = write_view("- Alpha\n- |Beta ^ref\n", vim, cx);
        keys(window, &format!("{insert}backspace"), cx);
        assert_eq!(state(window, cx), "- Alpha\n- |Beta ^ref\n");
        assert_eq!(
            cx.update(|cx| notice(cx)).as_deref(),
            Some(model::REFERENCED)
        );
        keys(window, "x", cx);
        assert_eq!(cx.update(|cx| notice(cx)), None);
    }
}

#[gpui::test]
fn task_line_start_reaches_text_then_checkbox(cx: &mut TestAppContext) {
    let text = "- One ^a\n- [ ] Ta|sk\n";
    let window = write_view(text, false, cx);
    expect(
        window,
        &[
            ("home", "- One ^a\n- [ ] |Task\n"),
            ("home", "- One ^a\n- |[ ] Task\n"),
            ("right", "- One ^a\n- [ ]| Task\n"),
            ("right", "- One ^a\n- [ ] |Task\n"),
            ("left", "- One ^a\n- [ ]| Task\n"),
            ("left", "- One ^a\n- |[ ] Task\n"),
            ("left", "- One| ^a\n- [ ] Task\n"),
        ],
        cx,
    );
    let window = write_view(text, true, cx);
    expect(
        window,
        &[
            ("0", "- One ^a\n- [ ] |Task\n"),
            ("0", "- One ^a\n- |[ ] Task\n"),
            ("l l", "- One ^a\n- [ ] |Task\n"),
            ("l ^", "- One ^a\n- [ ] |Task\n"),
            ("^", "- One ^a\n- |[ ] Task\n"),
        ],
        cx,
    );
    let window = write_view("- Plain te|xt\n", true, cx);
    expect(
        window,
        &[("0", "- |Plain text\n"), ("0", "- |Plain text\n")],
        cx,
    );
}

#[gpui::test]
fn folding_collapses_children_and_enter_adds_a_sibling(cx: &mut TestAppContext) {
    let window = write_view("- Parent|\n  - Kid\n    - Grand\n- After\n", true, cx);
    keys(window, "z c", cx);
    assert_eq!(displayed(window, cx), "Parent⋯\nAfter\n");
    expect(
        window,
        &[("i enter", "- Parent\n  - Kid\n    - Grand\n- |\n- After\n")],
        cx,
    );
    keys(window, "escape k z o", cx);
    assert_eq!(displayed(window, cx), "Parent\n  Kid\n    Grand\n\nAfter\n");

    let window = write_view("- Parent\n  - Kid|\n- After\n", false, cx);
    keys(window, &format!("{MOD}-alt-["), cx);
    assert_eq!(state(window, cx), "- Parent|\n  - Kid\n- After\n");
    assert_eq!(displayed(window, cx), "Parent⋯\nAfter\n");
    keys(window, &format!("{MOD}-alt-]"), cx);
    assert_eq!(displayed(window, cx), "Parent\n  Kid\nAfter\n");
}

/// Moves the pointer to the middle of `selector`'s drawn bounds, or to the window's corner when
/// it is `None`.
pub(super) fn hover(
    window: WindowHandle<Editor>,
    selector: Option<&'static str>,
    cx: &mut TestAppContext,
) {
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let at = selector.map_or(gpui::point(gpui::px(1.), gpui::px(1.)), |selector| {
        visual
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("a rendered {selector}"))
            .center()
    });
    visual.simulate_mouse_move(at, None, Default::default());
    cx.executor().run_until_parked();
}

pub(super) fn click(window: WindowHandle<Editor>, selector: &'static str, cx: &mut TestAppContext) {
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("a rendered {selector}"));
    visual.simulate_click(bounds.center(), Default::default());
    cx.executor().run_until_parked();
}

fn collapsed_count(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> usize {
    window
        .update(cx, |editor, _, cx| folds::collapsed(editor, cx).len())
        .unwrap()
}

#[gpui::test]
fn bullets_draw_without_changing_text_and_their_arrow_folds(cx: &mut TestAppContext) {
    let window = write_view("- Parent|\n  - Kid\n- After\n", false, cx);
    assert_eq!(displayed(window, cx), "Parent\n  Kid\nAfter\n");
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    let bullet = visual
        .debug_bounds("quire-bullet")
        .expect("a rendered bullet");
    assert!(
        bullet.size.width <= gpui::px(24.),
        "a bullet is as narrow as rt's: {bullet:?}"
    );
    assert!(
        visual.debug_bounds("quire-bullet-fold").is_none(),
        "the arrow waits for the pointer, as in rt"
    );
    hover(window, Some("quire-line-0"), cx);
    click(window, "quire-bullet-fold", cx);
    assert_eq!(displayed(window, cx), "Parent⋯\nAfter\n");
    assert_eq!(collapsed_count(window, cx), 1);
    hover(window, None, cx);
    click(window, "quire-bullet-fold", cx);
    assert_eq!(displayed(window, cx), "Parent\n  Kid\nAfter\n");
    hover(window, None, cx);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert!(
        visual.debug_bounds("quire-bullet-fold").is_none(),
        "an expanded block's arrow hides when the pointer leaves"
    );
    assert_eq!(state(window, cx), "- Parent|\n  - Kid\n- After\n");
}

#[gpui::test]
fn zed_fold_actions_fold_blocks_and_keep_write_syntax_hidden(cx: &mut TestAppContext) {
    let window = write_view("- Parent\n  - Ki|d\n- After\n", false, cx);
    cx.dispatch_action(window.into(), editor::actions::Fold);
    cx.executor().run_until_parked();
    assert_eq!(displayed(window, cx), "Parent⋯\nAfter\n");
    place(window, "- Parent".len(), cx);
    expect(
        window,
        &[("enter", "- Parent\n  - Kid\n- |\n- After\n")],
        cx,
    );
    keys(window, &undo(false), cx);
    place(window, "- Par".len(), cx);
    cx.dispatch_action(window.into(), editor::actions::UnfoldLines);
    cx.executor().run_until_parked();
    assert_eq!(
        displayed(window, cx),
        "Parent\n  Kid\nAfter\n",
        "markers stay hidden after Zed's unfold"
    );
}

#[gpui::test]
fn folds_follow_moved_blocks_and_keep_nested_folds(cx: &mut TestAppContext) {
    let window = write_view(
        "- Parent|\n  - Kid\n    - Grand\n- After\n  - Below\n",
        true,
        cx,
    );
    keys(window, "j z c k z c", cx);
    assert_eq!(displayed(window, cx), "Parent⋯\nAfter\n  Below\n");
    keys(window, "z o", cx);
    assert_eq!(
        displayed(window, cx),
        "Parent\n  Kid⋯\nAfter\n  Below\n",
        "opening a block keeps its children's folds"
    );
    keys(window, "z c alt-down", cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        "- After\n  - Below\n- Parent\n  - Kid\n    - Grand\n"
    );
    assert_eq!(displayed(window, cx), "After\n  Below\nParent⋯\n");
    keys(window, "z o", cx);
    assert_eq!(displayed(window, cx), "After\n  Below\nParent\n  Kid⋯\n");
    keys(window, "z shift-m", cx);
    assert_eq!(displayed(window, cx), "After⋯\nParent⋯\n");
    keys(window, "z shift-r", cx);
    assert_eq!(
        displayed(window, cx),
        "After\n  Below\nParent\n  Kid\n    Grand\n"
    );
    assert_eq!(collapsed_count(window, cx), 0);
}

#[gpui::test]
fn vim_open_and_join_follow_the_outline(cx: &mut TestAppContext) {
    let window = write_view("- O|ne ^a\n  - Kid\n- Two\n", true, cx);
    expect(
        window,
        &[
            ("o x escape", "- One ^a\n  - |x\n  - Kid\n- Two\n"),
            (
                "j j shift-o y escape",
                "- One ^a\n  - x\n  - Kid\n- |y\n- Two\n",
            ),
            ("k shift-j", "- One ^a\n  - x\n  - Kid| y\n- Two\n"),
        ],
        cx,
    );
    let window = write_view("- O|ne\n- Two ^t\n", true, cx);
    keys(window, "shift-j", cx);
    assert_eq!(state(window, cx), "- O|ne\n- Two ^t\n");
    assert_eq!(
        cx.update(|cx| notice(cx)).as_deref(),
        Some(model::REFERENCED)
    );
}

#[gpui::test]
fn pasting_lines_into_a_bullet_makes_blocks(cx: &mut TestAppContext) {
    let window = write_view("- Alpha|Omega\n", false, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        "- one\n- two\n  - two-kid\n- three".into(),
    ));
    expect(
        window,
        &[(
            &format!("{MOD}-v"),
            "- Alphaone\n- two\n  - two-kid\n- three|Omega\n",
        )],
        cx,
    );
    keys(window, &undo(false), cx);
    assert_eq!(state(window, cx).replace('|', ""), "- AlphaOmega\n");
    cx.write_to_clipboard(gpui::ClipboardItem::new_string("single".into()));
    expect(
        window,
        &[(&format!("{MOD}-v"), "- Alphasingle|Omega\n")],
        cx,
    );
}

#[gpui::test]
fn numbered_list_toggles_and_converts_by_style(cx: &mut TestAppContext) {
    let original =
        "Intro\n\n- One\n- T|wo\n  - Child\n    more child text\n- [ ] Three ^t\n\nAfter\n";
    let window = write_view(original, false, cx);
    keys(window, &format!("{MOD}-shift-7"), cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        "Intro\n\n1. One\n2. Two\n   - Child\n     more child text\n3. [ ] Three ^t\n\nAfter\n"
    );
    keys(window, &format!("{MOD}-shift-7"), cx);
    assert_eq!(state(window, cx), original);
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(NumberList.boxed_clone(), cx)
        })
        .unwrap();
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(NumberList.boxed_clone(), cx)
        })
        .unwrap();
    assert!(state(window, cx).contains("2. T|wo"));
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(BulletList.boxed_clone(), cx)
        })
        .unwrap();
    assert_eq!(state(window, cx), original);
    let window = write_view("Just a para|graph\n", false, cx);
    keys(window, &format!("{MOD}-shift-7"), cx);
    assert_eq!(cx.update(|cx| notice(cx)).as_deref(), Some(lists::NO_LIST));
    assert_eq!(state(window, cx), "Just a para|graph\n");
}

#[gpui::test]
fn source_view_keeps_ordinary_editing(cx: &mut TestAppContext) {
    let window = write_view("- On|e ^a\n- \n- Next\n", false, cx);
    let editor = window.root(cx).unwrap();
    cx.update_window(window.into(), |_, window, cx| {
        crate::editor_projection::unbind(&editor, window, cx)
    })
    .unwrap();
    keys(window, "enter", cx);
    let after = state(window, cx);
    assert!(
        after.starts_with("- On\n") && after.ends_with("e ^a\n- \n- Next\n"),
        "{after}"
    );
    place(
        window,
        after.replacen('|', "", 1).find("- \n").unwrap() + 2,
        cx,
    );
    keys(window, "backspace", cx);
    assert!(
        state(window, cx).contains("-|\n- Next\n"),
        "{}",
        state(window, cx)
    );
}

#[gpui::test]
fn merging_never_edits_collapsed_children(cx: &mut TestAppContext) {
    let window = write_view("- Parent|\n  - Kid\n- After\n", false, cx);
    keys(window, &format!("{MOD}-alt-["), cx);
    expect(window, &[("delete", "- Parent|\n  - Kid\n- After\n")], cx);
    place(window, "- Parent\n  - Kid\n- ".len(), cx);
    expect(
        window,
        &[("backspace", "- Parent\n  - Kid\n- |After\n")],
        cx,
    );
    place(window, "- Parent".len(), cx);
    keys(window, &format!("{MOD}-alt-]"), cx);
    place(window, "- Parent\n  - Kid\n- ".len(), cx);
    expect(window, &[("backspace", "- Parent\n  - Kid|After\n")], cx);
}

#[gpui::test]
fn vim_dot_repeats_opening_a_block_with_its_text(cx: &mut TestAppContext) {
    let window = write_view("- O|ne\n", true, cx);
    expect(
        window,
        &[
            ("o n e w escape", "- One\n- ne|w\n"),
            (".", "- One\n- new\n- ne|w\n"),
            (
                "k shift-o u p escape .",
                "- One\n- u|p\n- up\n- new\n- new\n",
            ),
        ],
        cx,
    );
}

#[gpui::test]
fn vim_dot_repeats_opened_text_that_contains_o(cx: &mut TestAppContext) {
    let window = write_view("- |one\n", true, cx);
    expect(
        window,
        &[("o d o t escape .", "- one\n- dot\n- do|t\n")],
        cx,
    );
    let window = write_view("- |one\n", true, cx);
    expect(
        window,
        &[
            ("o d o t escape", "- one\n- do|t\n"),
            (".", "- one\n- dot\n- do|t\n"),
        ],
        cx,
    );
}

#[gpui::test]
fn moved_blocks_keep_the_caret_when_the_parse_lags(cx: &mut TestAppContext) {
    let window = write_view("- A\n- B|\n  - B1\n- C\n", false, cx);
    window
        .update(cx, |editor, _, cx| {
            let source = editor.buffer().read(cx).as_singleton().unwrap();
            source.update(cx, |buffer, _| {
                buffer.set_sync_parse_timeout(Some(std::time::Duration::ZERO))
            });
        })
        .unwrap();
    expect(
        window,
        &[
            ("alt-up", "- B|\n  - B1\n- A\n- C\n"),
            ("alt-shift-down", "- A\n- B|\n  - B1\n- C\n"),
            (&format!("{MOD}-shift-down"), "- A\n- C\n- B|\n  - B1\n"),
        ],
        cx,
    );
}

#[gpui::test]
fn numbered_lists_renumber_after_plain_edits(cx: &mut TestAppContext) {
    let window = write_view("1. One\n2. T|wo\n3. Three\n", true, cx);
    keys(window, "d d", cx);
    assert_eq!(state(window, cx).replace('|', ""), "1. One\n2. Three\n");
    keys(window, "u", cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        "1. One\n2. Two\n3. Three\n"
    );

    let window = write_view("1. One\n2. Two\n|\n", false, cx);
    keys(window, "1 . space c", cx);
    assert_eq!(state(window, cx), "1. One\n2. Two\n3. c|\n");
}

/// The ID after `^` on each line of the view's text, or "" for lines without one.
fn line_ids(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> Vec<String> {
    state(window, cx)
        .replace('|', "")
        .lines()
        .map(|line| line.rsplit_once(" ^").map_or("", |(_, id)| id).to_owned())
        .collect()
}

#[gpui::test]
fn copied_block_ids_become_fresh_ids(cx: &mut TestAppContext) {
    let window = write_view("- O|ne ^a\n- Two ^b\n", true, cx);
    keys(window, "y y p", cx);
    let ids = line_ids(window, cx);
    assert_eq!(ids[0], "a");
    assert!(ids[1].starts_with("b-"), "{ids:?}");
    assert_eq!(ids[2], "b");
    assert!(state(window, cx).starts_with("- One ^a\n- |One ^b-"));
    keys(window, "u", cx);
    assert_eq!(state(window, cx).replace('|', ""), "- One ^a\n- Two ^b\n");

    let window = write_view("- Two ^b\n- |\n", false, cx);
    cx.write_to_clipboard(gpui::ClipboardItem::new_string(
        "X ^b\n- Y ^n\n- Z ^n".into(),
    ));
    keys(window, &format!("{MOD}-v"), cx);
    let ids = line_ids(window, cx);
    assert_eq!(ids[0], "b");
    assert!(ids[1].starts_with("b-") && ids[1] != "b", "{ids:?}");
    assert_eq!(ids[2], "n", "a new ID stays");
    assert!(ids[3].starts_with("b-"), "{ids:?}");
    keys(window, &undo(false), cx);
    assert_eq!(state(window, cx).replace('|', ""), "- Two ^b\n- \n");
}

/// Write hides list markers and IDs, and Vim's linewise yank and put still copy and place
/// whole source lines.
#[gpui::test]
fn vim_linewise_yank_and_put_keep_hidden_syntax(cx: &mut TestAppContext) {
    let window = write_view("- O|ne\n- Two\n", true, cx);
    expect(window, &[("y y p", "- One\n- |One\n- Two\n")], cx);
    let window = write_view("- One\n- T|wo ^t\n", true, cx);
    keys(window, "y y P", cx);
    let ids = line_ids(window, cx);
    assert!(
        state(window, cx).starts_with("- One\n- |Two ^b-"),
        "{ids:?}"
    );
    assert_eq!(ids[2], "t");
}

#[gpui::test]
fn link_pairs_delete_together_and_enter_steps_out(cx: &mut TestAppContext) {
    let window = write_view("- See |\n", false, cx);
    expect(
        window,
        &[
            ("[ [", "- See [[|]]\n"),
            ("backspace", "- See |\n"),
            ("( (", "- See ((|))\n"),
            ("backspace", "- See |\n"),
        ],
        cx,
    );
    let window = write_view("- See [[Pa|]] and ((b))\n", false, cx);
    keys(window, "enter", cx);
    assert_eq!(state(window, cx), "- See [[Pa]]| and ((b))\n");
    let window = write_view("- See [[Pa]] and ((b|))\n", false, cx);
    keys(window, "enter", cx);
    assert_eq!(state(window, cx), "- See [[Pa]] and ((b))|\n");
    let window = write_view("- Closed [[Pa]]|]]\n", false, cx);
    keys(window, "enter", cx);
    assert_eq!(state(window, cx), "- Closed [[Pa]]\n- |]]\n");
}

#[cfg(target_os = "macos")]
#[gpui::test]
fn cmd_backspace_stops_at_the_text_start(cx: &mut TestAppContext) {
    let window = write_view("- [ ] Hello wor|ld ^x\n- Next\n", false, cx);
    expect(
        window,
        &[
            (&*format!("{MOD}-backspace"), "- [ ] |ld ^x\n- Next\n"),
            (&*format!("{MOD}-backspace"), "- [ ] |ld ^x\n- Next\n"),
            (&*format!("{MOD}-z"), "- [ ] Hello wor|ld ^x\n- Next\n"),
        ],
        cx,
    );
}

#[gpui::test]
fn vim_counts_and_visual_shifts_move_sibling_subtrees(cx: &mut TestAppContext) {
    let page = "- One\n- T|wo\n  - Kid\n- Three\n- Four\n";
    let window = write_view(page, true, cx);
    expect(
        window,
        &[
            ("3 > >", "- One\n  - |Two\n    - Kid\n  - Three\n- Four\n"),
            ("u", page),
            (
                "shift-v j j >",
                "- One\n  - |Two\n    - Kid\n  - Three\n- Four\n",
            ),
        ],
        cx,
    );
    assert!(
        window
            .update(cx, |editor, _, cx| editor
                .selections
                .newest::<MultiBufferOffset>(&editor.display_snapshot(cx)))
            .unwrap()
            .is_empty(),
        "visual mode ends"
    );
    expect(
        window,
        &[("shift-v j j <", "- One\n- |Two\n  - Kid\n- Three\n- Four\n")],
        cx,
    );
    keys(window, "j shift-v j j >", cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        "- One\n- Two\n  - Kid\n- Three\n- Four\n"
    );
    assert_eq!(
        cx.update(|cx| notice(cx)).as_deref(),
        Some(model::NOT_SIBLINGS),
        "a kid and its parent's siblings move only together with the parent"
    );

    let page = "- A\n  - B\n    - C\n- D|\n  - Kid\n- E\n";
    let window = write_view(page, true, cx);
    expect(
        window,
        &[
            (
                "shift-v j j 3 >",
                "- A\n  - B\n    - C\n      - |D\n        - Kid\n      - E\n",
            ),
            ("u", "- A\n  - B\n    - C\n- |D\n  - Kid\n- E\n"),
            (
                "shift-v 5 >",
                "- A\n  - B\n    - C\n      - |D\n        - Kid\n- E\n",
            ),
        ],
        cx,
    );
    expect(
        window,
        &[(
            "shift-v 2 <",
            "- A\n  - B\n    - C\n  - |D\n    - Kid\n- E\n",
        )],
        cx,
    );
}

/// A fresh ID the index names after the writer typed on is part of the paste's undo step.
#[gpui::test]
fn a_late_follow_up_edit_joins_its_edit_after_later_typing(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
    let buffer = cx.update(|cx| crate::markdown::buffer("- Here\n", cx));
    let edit = |at: usize, text: &str, cx: &mut TestAppContext| {
        buffer.update(cx, |buffer, cx| {
            buffer.edit([(at..at, text)], None, cx);
            buffer.finalize_last_transaction();
            buffer.peek_undo_stack().map(|entry| entry.transaction_id())
        })
    };
    let pasted = edit(7, "- Copy ^far\n", cx);
    edit(0, "!", cx);
    cx.update(|cx| {
        let at = "!- Here\n- Copy ^".len();
        amend_later(&buffer, &[(at..at + 3, "new".into())], pasted, cx)
    });
    let text = |cx: &mut TestAppContext| buffer.read_with(cx, |buffer, _| buffer.text());
    assert_eq!(text(cx), "!- Here\n- Copy ^new\n");
    buffer.update(cx, |buffer, cx| buffer.undo(cx));
    assert_eq!(
        text(cx),
        "- Here\n- Copy ^new\n",
        "undo reverts the typing first"
    );
    buffer.update(cx, |buffer, cx| buffer.undo(cx));
    assert_eq!(
        text(cx),
        "- Here\n",
        "the next undo reverts the paste with its follow-up"
    );
}
