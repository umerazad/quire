use super::*;
use gpui::{AppContext as _, Focusable as _, TestAppContext, UpdateGlobal as _, WindowHandle};
use multi_buffer::MultiBufferOffset;
use std::time::Duration;

const MOD: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

/// The formatter answers from its own thread, so the tests let real time pass while they wait.
fn init(vim: bool, cx: &mut TestAppContext) {
    cx.executor().allow_parking();
    cx.update(|cx| {
        crate::editor_host::init(cx).expect("editor initialization");
        let settings = if vim {
            r#"{"vim_mode":true,"session":{"restore_unsaved_buffers":false}}"#
        } else {
            r#"{"vim_mode":false,"session":{"restore_unsaved_buffers":false}}"#
        };
        let result = settings::SettingsStore::update_global(cx, |store, cx| {
            store.set_user_settings(settings, cx)
        });
        assert!(!matches!(
            result.parse_status,
            settings::ParseStatus::Failed { .. }
        ));
    });
}

/// A view of `marked`, whose `|` is the caret, in Write or Source.
fn view(marked: &str, write: bool, cx: &mut TestAppContext) -> WindowHandle<Editor> {
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
    place(window, cursor, cx);
    window
}

fn place(window: WindowHandle<Editor>, cursor: usize, cx: &mut TestAppContext) {
    window
        .update(cx, |editor, window, cx| {
            editor.change_selections(
                editor::SelectionEffects::no_scroll(),
                window,
                cx,
                |selections| {
                    selections.select_ranges([MultiBufferOffset(cursor)..MultiBufferOffset(cursor)])
                },
            )
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

fn keys(window: WindowHandle<Editor>, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), keys);
    cx.executor().run_until_parked();
}

fn notice(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| crate::outline::notice(cx))
        .map(|notice| notice.to_string())
}

/// Waits for the formatter thread's answer to reach the view.
fn settle(window: WindowHandle<Editor>, cx: &mut TestAppContext) {
    crate::test_wait::until(
        cx,
        Duration::from_millis(5),
        crate::test_wait::park,
        |cx| {
            !window
                .update(cx, |editor, _, _| formatting(editor))
                .unwrap()
        },
        |_| "Format code to finish".into(),
    );
}

fn format(window: WindowHandle<Editor>, cx: &mut TestAppContext) {
    keys(window, &format!("{MOD}-shift-i"), cx);
    settle(window, cx);
}

#[gpui::test]
fn fences_report_their_body_tag_and_container_prefix(cx: &mut TestAppContext) {
    init(false, cx);
    let fence = |text: &str, cx: &mut TestAppContext| {
        let at = text.find('|').unwrap();
        let text = text.replacen('|', "", 1);
        let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.executor().run_until_parked();
        buffer.read_with(cx, |buffer, _| fence_at(&buffer.snapshot(), at))
    };
    let text = "- Item\n  ```js\n  const x={a:1}|\n\n  y()\n  ```\n";
    let found = fence(text, cx).unwrap();
    assert_eq!(found.tag, "js");
    assert_eq!(found.prefix, "  ");
    assert_eq!(found.code, "const x={a:1}\n\ny()\n");
    let plain = text.replacen('|', "", 1);
    assert_eq!(
        &plain[found.range.clone()],
        "```js\n  const x={a:1}\n\n  y()\n  ```"
    );
    assert_eq!(&plain[found.body.clone()], "  const x={a:1}\n\n  y()\n");

    let quoted = fence("> ```css\n> a{color:red}|\n>\n> ```\n", cx).unwrap();
    assert_eq!((quoted.tag.as_str(), quoted.prefix.as_str()), ("css", "> "));
    assert_eq!(quoted.code, "a{color:red}\n\n");

    assert_eq!(fence("```\nplain|\n```\n", cx).unwrap().tag, "");
    assert_eq!(fence("```js\nopen|\n", cx), Err(UNCLOSED));
    assert_eq!(fence("Para|graph\n\n```js\nx\n```\n", cx), Err(NOT_IN_CODE));
}

#[test]
fn formatted_lines_take_the_closing_fence_prefix_as_rt_does() {
    assert_eq!(
        formatted_body("  ", "const x = { a: 1 };\n"),
        "  const x = { a: 1 };\n"
    );
    assert_eq!(
        formatted_body("", "a {\n  b: c;\n}\n\n"),
        "a {\n  b: c;\n}\n"
    );
    assert_eq!(formatted_body("> ", "x;\n\ny;\n"), "> x;\n> \n> y;\n");
}

#[gpui::test]
fn format_code_rewrites_the_fence_at_the_caret_as_one_undo_step(cx: &mut TestAppContext) {
    for (vim, undo) in [(false, format!("{MOD}-z")), (true, "u".to_string())] {
        init(vim, cx);
        let before = "Intro\n\n```js\nconst x={a:1}|\n```\n\nAfter\n";
        let window = view(before, true, cx);
        format(window, cx);
        assert_eq!(
            state(window, cx),
            "Intro\n\n```js\nconst x = { a: 1 };|\n```\n\nAfter\n",
            "vim {vim}"
        );
        assert_eq!(notice(cx), None);
        keys(window, &undo, cx);
        assert_eq!(
            state(window, cx).replace('|', ""),
            before.replace('|', ""),
            "one undo step restores the block, vim {vim}"
        );
    }
}

#[gpui::test]
fn format_code_keeps_list_indentation_and_block_ids_in_write_and_source(cx: &mut TestAppContext) {
    init(false, cx);
    for write in [true, false] {
        let window = view(
            "- Parent ^p1\n  ```ts\n  let  a:number=1|\n  type T={b:string}\n  ```\n- Next ^n2\n",
            write,
            cx,
        );
        format(window, cx);
        assert_eq!(
            state(window, cx),
            "- Parent ^p1\n  ```ts\n  let a: number = 1;|\n  type T = { b: string };\n  ```\n- Next ^n2\n",
            "write {write}"
        );
    }
}

#[gpui::test]
fn format_code_formats_each_language_inside_a_real_page(cx: &mut TestAppContext) {
    init(false, cx);
    let page = "# Snippets\n\n- JSON\n  ```json\n  {\"a\":1,\"b\":[true,null]}\n  ```\n- Style\n  ```css\n  a{color:red}\n  ```\n\n```yaml\nk:   v\n```\n\n```xml\n<a><b>x</b></a>\n```\n\n```markdown\n*  item\n```\n";
    let window = view(&format!("|{page}"), true, cx);
    for find in ["{\"a\"", "a{color", "k:", "<a><b>", "*  item"] {
        let text = state(window, cx).replace('|', "");
        place(window, text.find(find).unwrap(), cx);
        format(window, cx);
        assert_eq!(notice(cx), None, "after {find}");
    }
    assert_eq!(
        state(window, cx).replace('|', ""),
        "# Snippets\n\n- JSON\n  ```json\n  { \"a\": 1, \"b\": [true, null] }\n  ```\n- Style\n  ```css\n  a {\n    color: red;\n  }\n  ```\n\n```yaml\nk: v\n```\n\n```xml\n<a><b>x</b></a>\n```\n\n```markdown\n- item\n```\n"
    );
}

#[gpui::test]
fn format_code_explains_why_it_did_nothing(cx: &mut TestAppContext) {
    init(false, cx);
    let cases = [
        (
            "```js\nconst = 1|\n```\n",
            "Code formatting failed: SyntaxError: Unexpected token (1:7). Correct its syntax, then run Format code again.",
        ),
        (
            "```py\nprint( 1 )|\n```\n",
            "Quire has no bundled formatter for `py` code. Format it by hand; Format code supports JavaScript, TypeScript, JSON, CSS, HTML, XML, YAML, and Markdown.",
        ),
        (
            "```\nx|\n```\n",
            "This code block has no language tag, so Quire cannot format it. Add a tag such as `js` after the opening fence, then run Format code again.",
        ),
        ("```js\nlet  a|\n", UNCLOSED),
        ("Plain| text\n", NOT_IN_CODE),
    ];
    for (marked, message) in cases {
        let window = view(marked, true, cx);
        format(window, cx);
        assert_eq!(state(window, cx), marked, "the page is unchanged");
        assert_eq!(notice(cx).as_deref(), Some(message), "{marked:?}");
    }
}

#[gpui::test]
fn a_result_for_a_block_edited_meanwhile_is_discarded(cx: &mut TestAppContext) {
    init(false, cx);
    let window = view("```js\nlet  a=1|\n```\n", true, cx);
    window
        .update(cx, |editor, window, cx| {
            format_code(editor, window, cx);
            editor.handle_input("2", window, cx);
        })
        .unwrap();
    settle(window, cx);
    assert_eq!(state(window, cx), "```js\nlet  a=12|\n```\n");
    assert_eq!(notice(cx).as_deref(), Some(CHANGED));
}

#[gpui::test]
fn an_edit_outside_the_block_does_not_discard_its_result(cx: &mut TestAppContext) {
    init(false, cx);
    let window = view("Title\n\n```js\nlet  a=1|\n```\n", true, cx);
    window
        .update(cx, |editor, window, cx| {
            format_code(editor, window, cx);
            editor.change_selections(
                editor::SelectionEffects::no_scroll(),
                window,
                cx,
                |selections| selections.select_ranges([MultiBufferOffset(5)..MultiBufferOffset(5)]),
            );
            editor.handle_input("!", window, cx);
        })
        .unwrap();
    settle(window, cx);
    assert_eq!(state(window, cx), "Title!|\n\n```js\nlet a = 1;\n```\n");
    assert_eq!(notice(cx), None);
}

#[gpui::test]
fn a_newer_request_cancels_the_pending_one(cx: &mut TestAppContext) {
    init(false, cx);
    let large: String = (0..1500)
        .map(|i| format!("function f{i}(a,b){{return a+b*{i}}}\n"))
        .collect();
    let page = format!("```js\n{large}```\n\n```css\na{{color:red}}\n```\n");
    let window = view(&format!("|{page}"), true, cx);
    let css = page.find("a{color").unwrap();
    window
        .update(cx, |editor, window, cx| {
            let at = |offset| MultiBufferOffset(offset)..MultiBufferOffset(offset);
            let effects = editor::SelectionEffects::no_scroll;
            editor.change_selections(effects(), window, cx, |s| s.select_ranges([at(6)]));
            format_code(editor, window, cx);
            assert!(formatting(editor));
            editor.change_selections(effects(), window, cx, |s| s.select_ranges([at(css)]));
            format_code(editor, window, cx);
        })
        .unwrap();
    settle(window, cx);
    assert_eq!(
        state(window, cx).replace('|', ""),
        format!("```js\n{large}```\n\n```css\na {{\n  color: red;\n}}\n```\n"),
        "the cancelled block keeps its text"
    );
}

#[gpui::test]
fn a_block_that_takes_too_long_reports_the_limit(cx: &mut TestAppContext) {
    init(false, cx);
    cx.update(|cx| {
        use_formatter(
            quire_format::Formatter::new(quire_format::Limits {
                timeout: Duration::from_millis(1),
                ..quire_format::Limits::default()
            }),
            cx,
        )
    });
    let large: String = (0..1500).map(|i| format!("let v{i}={{a:{i}}}\n")).collect();
    let marked = format!("```js\n|{large}```\n");
    let window = view(&marked, true, cx);
    format(window, cx);
    assert_eq!(state(window, cx), marked);
    assert_eq!(
        notice(cx).as_deref(),
        Some(
            "Code formatting stopped after 1 ms. Split the block into smaller ones, then run Format code again."
        )
    );
}

fn toolbars(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> Vec<String> {
    cx.executor().run_until_parked();
    window
        .update(cx, |editor, _, cx| {
            toolbar::toolbars(editor, cx)
                .iter()
                .map(toolbar::Toolbar::text)
                .collect()
        })
        .unwrap()
}

fn draw(window: WindowHandle<Editor>, cx: &mut TestAppContext) -> gpui::VisualTestContext {
    cx.executor().run_until_parked();
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual
}

fn click(window: WindowHandle<Editor>, selector: &'static str, cx: &mut TestAppContext) {
    let mut visual = draw(window, cx);
    let bounds = visual
        .debug_bounds(selector)
        .unwrap_or_else(|| panic!("{selector} is not drawn"));
    visual.simulate_click(bounds.center(), Default::default());
    cx.executor().run_until_parked();
}

fn clipboard(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| cx.read_from_clipboard().and_then(|item| item.text()))
}

fn dispatch_copy(window: WindowHandle<Editor>, cx: &mut TestAppContext) {
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(CopyCode), cx)
        })
        .unwrap();
    cx.executor().run_until_parked();
}

#[gpui::test]
fn toolbars_name_the_language_and_offer_what_applies(cx: &mut TestAppContext) {
    init(false, cx);
    let page = "|Intro\n\n```js\nlet a=1\n```\n\n- Item\n  ```\n  plain\n  ```\n\n> ```py\n> print(1)\n> ```\n\n```TS title\nlet b\n```\n\n```js\nopen\n";
    let window = view(page, true, cx);
    assert_eq!(
        toolbars(window, cx),
        [
            "js · Format code · Copy code",
            "Plain text · Copy code",
            "py · Copy code",
            "TS · Format code · Copy code",
        ],
        "an unclosed block has no toolbar"
    );
    let mut visual = draw(window, cx);
    for ordinal in 0..4 {
        assert!(
            visual
                .debug_bounds(Box::leak(
                    format!("quire-code-toolbar-{ordinal}").into_boxed_str()
                ))
                .is_some(),
            "toolbar {ordinal} is drawn"
        );
    }
    let text = page.replace('|', "");
    place(window, text.find("plain").unwrap(), cx);
    let mut visual = draw(window, cx);
    assert!(
        visual.debug_bounds("quire-code-copy-1").is_none(),
        "the caret in a block shows its fence as written"
    );
    assert!(visual.debug_bounds("quire-code-copy-0").is_some());
    window
        .update(cx, |editor, _, _| editor.set_read_only(true))
        .unwrap();
    window
        .update(cx, |editor, window, cx| {
            crate::editor_projection::refold(editor, window, cx)
        })
        .unwrap();
    assert_eq!(toolbars(window, cx)[0], "js · Copy code");
}

#[gpui::test]
fn copy_code_copies_the_body_without_fences_or_container_prefixes(cx: &mut TestAppContext) {
    for vim in [false, true] {
        init(vim, cx);
        let page = "|Intro\n\n- Item\n  ```js\n  const x={a:1}\n\n  y()\n  ```\n\n> ```css\n> a{color:red}\n>\n> ```\n";
        let window = view(page, true, cx);
        click(window, "quire-code-copy-0", cx);
        assert_eq!(
            clipboard(cx).as_deref(),
            Some("const x={a:1}\n\ny()\n"),
            "vim {vim}"
        );
        assert_eq!(
            toolbars(window, cx),
            ["js · Format code · Copied", "css · Format code · Copy code"]
        );
        click(window, "quire-code-copy-1", cx);
        assert_eq!(clipboard(cx).as_deref(), Some("a{color:red}\n\n"));
        assert_eq!(state(window, cx), page, "copying leaves the page and caret");

        let text = page.replace('|', "");
        let at = text.find("y()").unwrap();
        place(window, at, cx);
        dispatch_copy(window, cx);
        assert_eq!(clipboard(cx).as_deref(), Some("const x={a:1}\n\ny()\n"));
        assert_eq!(toolbars(window, cx)[0], "js · Format code · Copied");
        window
            .update(cx, |editor, window, cx| {
                editor.buffer().update(cx, |buffer, cx| {
                    buffer.edit(
                        [(MultiBufferOffset(at)..MultiBufferOffset(at), "z")],
                        None,
                        cx,
                    )
                });
                crate::editor_projection::refold(editor, window, cx);
            })
            .unwrap();
        place(window, 0, cx);
        assert_eq!(
            toolbars(window, cx),
            [
                "js · Format code · Copy code",
                "css · Format code · Copy code"
            ],
            "a changed block is no longer marked copied"
        );
        dispatch_copy(window, cx);
        assert_eq!(notice(cx).as_deref(), Some(COPY_NOT_IN_CODE));
    }
}

#[gpui::test]
fn the_format_button_shows_progress_and_survives_a_refold(cx: &mut TestAppContext) {
    init(false, cx);
    let page = "|Intro\n\n```js\nfunction f0(a,b){return a+b*0}\n```\n\nAfter\n".to_owned();
    let window = view(&page, true, cx);
    let thread = cx.update(formatter);
    let held = thread.hold();
    let mut visual = draw(window, cx);
    let button = visual
        .debug_bounds("quire-code-format-0")
        .expect("a drawn Format code button");
    visual.simulate_mouse_down(button.center(), gpui::MouseButton::Left, Default::default());
    window
        .update(cx, |editor, window, cx| {
            crate::editor_projection::refold(editor, window, cx)
        })
        .unwrap();
    let mut visual = draw(window, cx);
    visual.simulate_mouse_up(button.center(), gpui::MouseButton::Left, Default::default());
    assert!(
        window
            .update(cx, |editor, _, _| formatting(editor))
            .unwrap()
    );
    assert_eq!(toolbars(window, cx), ["js · Formatting… · Copy code"]);
    assert_eq!(notice(cx).as_deref(), Some(FORMATTING));
    assert_eq!(state(window, cx), page, "the click leaves the caret");
    let text = page.replace('|', "");
    place(window, text.find("After").unwrap(), cx);
    assert_eq!(
        notice(cx).as_deref(),
        Some(FORMATTING),
        "a caret move keeps the progress notice"
    );
    drop(held);
    settle(window, cx);
    assert_eq!(notice(cx), None);
    assert_eq!(toolbars(window, cx), ["js · Format code · Copy code"]);
    assert!(
        state(window, cx).contains("function f0(a, b) {\n  return a + b * 0;\n}\n"),
        "the block is formatted"
    );
}

#[gpui::test]
fn format_and_copy_work_in_an_excerpt_that_shows_the_whole_block(cx: &mut TestAppContext) {
    init(false, cx);
    let page = "# Page\n\n- Parent\n  ```js\n  let  a=1\n  ```\n- Next\n";
    let source = cx.update(|cx| crate::markdown::buffer(page, cx));
    cx.executor().run_until_parked();
    let excerpt = |range: Range<usize>, cx: &mut TestAppContext| {
        let source = source.clone();
        let window = cx.add_window(|window, cx| {
            let buffer = crate::editor_excerpt::buffer(source, range, cx).unwrap();
            let editor = Editor::for_multibuffer(buffer, None, window, cx);
            window.focus(&editor.focus_handle(cx), cx);
            editor
        });
        cx.executor().run_until_parked();
        window
    };
    let start = page.find("- Parent").unwrap();
    let whole = excerpt(start..page.find("- Next").unwrap(), cx);
    let text = whole.update(cx, |editor, _, cx| editor.text(cx)).unwrap();
    place(whole, text.find("a=1").unwrap(), cx);
    dispatch_copy(whole, cx);
    assert_eq!(clipboard(cx).as_deref(), Some("let  a=1\n"));
    format(whole, cx);
    assert_eq!(notice(cx), None);
    let formatted = "# Page\n\n- Parent\n  ```js\n  let a = 1;\n  ```\n- Next\n";
    assert_eq!(source.read_with(cx, |source, _| source.text()), formatted);

    let partial = excerpt(start..formatted.find("1;").unwrap(), cx);
    let text = partial.update(cx, |editor, _, cx| editor.text(cx)).unwrap();
    place(partial, text.find("a =").unwrap(), cx);
    format(partial, cx);
    assert_eq!(notice(cx).as_deref(), Some(PARTLY_SHOWN));
    dispatch_copy(partial, cx);
    assert_eq!(notice(cx).as_deref(), Some(COPY_PARTLY_SHOWN));
    assert_eq!(source.read_with(cx, |source, _| source.text()), formatted);
}

#[gpui::test]
fn toolbar_buttons_work_from_the_keyboard(cx: &mut TestAppContext) {
    for (vim, key) in [(false, "enter"), (true, "space")] {
        init(vim, cx);
        let page = "|Intro\n\n```js\nlet  a=1\n```\n";
        let window = view(page, true, cx);
        draw(window, cx);
        // Tab order in the toolbar: the language, Format code, then Copy code.
        let press = |tabs: usize, cx: &mut TestAppContext| {
            place(window, 0, cx);
            draw(window, cx);
            window
                .update(cx, |_, window, cx| {
                    window.blur(cx);
                    for _ in 0..tabs {
                        window.focus_next(cx);
                    }
                })
                .unwrap();
            draw(window, cx);
            activate(window, key, cx);
        };
        press(1, cx);
        assert_eq!(
            state(window, cx),
            "Intro\n\n|```js\nlet  a=1\n```\n",
            "the language puts the caret in the block, vim {vim}"
        );
        press(3, cx);
        assert_eq!(
            clipboard(cx).as_deref(),
            Some("let  a=1\n"),
            "Copy code, vim {vim}"
        );
        press(2, cx);
        settle(window, cx);
        assert_eq!(
            state(window, cx).replace('|', ""),
            "Intro\n\n```js\nlet a = 1;\n```\n",
            "Format code, vim {vim}"
        );
    }
}

/// Presses and releases `key` on the focused element, as a keyboard activates a control.
fn activate(window: WindowHandle<Editor>, key: &str, cx: &mut TestAppContext) {
    cx.update_window(window.into(), |_, window, cx| {
        let keystroke = gpui::Keystroke::parse_with_os(key, cx.platform_info().os).unwrap();
        window.dispatch_event(
            gpui::PlatformInput::KeyDown(gpui::KeyDownEvent {
                keystroke: keystroke.clone(),
                is_held: false,
                prefer_character_input: false,
            }),
            cx,
        );
        window.dispatch_event(
            gpui::PlatformInput::KeyUp(gpui::KeyUpEvent { keystroke }),
            cx,
        );
    })
    .unwrap();
    cx.executor().run_until_parked();
}
