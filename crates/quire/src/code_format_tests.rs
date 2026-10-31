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
    for _ in 0..2000 {
        cx.executor().run_until_parked();
        if !window
            .update(cx, |editor, _, _| formatting(editor))
            .unwrap()
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("Format code did not finish within 10 seconds");
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
