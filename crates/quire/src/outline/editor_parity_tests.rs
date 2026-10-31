//! rt's outline scenarios from `tests/editor.test.tsx` (`5ea5c22310ff`), as keystrokes in a
//! native Write view. Each case names the rt test it ports; `|` marks the caret. Typing goes
//! through key events, as a writer types, rather than rt's direct document dispatches.

use super::tests::{displayed, keys, state, write_view};
use gpui::{AppContext as _, TestAppContext};

struct Case {
    rt: &'static str,
    page: &'static str,
    vim: bool,
    steps: &'static [(&'static str, &'static str)],
}

const MOD: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

const CASES: &[Case] = &[
    Case {
        rt: "splits around children and IDs the way Roam does: a parent's end",
        page: "- Parent| ^parent\n  - Child ^child\n",
        vim: false,
        steps: &[("enter", "- Parent ^parent\n  - |\n  - Child ^child\n")],
    },
    Case {
        rt: "splits around children and IDs the way Roam does: mid-text",
        page: "- Pare|ntTail\n  - Kid\n",
        vim: false,
        steps: &[("enter", "- Pare\n  - |ntTail\n  - Kid\n")],
    },
    Case {
        rt: "splits around children and IDs the way Roam does: the text start",
        page: "- |Parent ^p\n  - Kid\n- Next\n",
        vim: false,
        steps: &[("enter", "- \n- |Parent ^p\n  - Kid\n- Next\n")],
    },
    Case {
        rt: "creates a visible nested outline using Enter and Tab (Vim=false)",
        page: "- First thought|",
        vim: false,
        steps: &[
            ("enter", "- First thought\n- |"),
            ("tab C h i l d", "- First thought\n  - Child|"),
            (
                "enter tab G r a n d c h i l d",
                "- First thought\n  - Child\n    - Grandchild|",
            ),
            ("shift-tab", "- First thought\n  - Child\n  - Grandchild|"),
        ],
    },
    Case {
        rt: "creates a visible nested outline using Enter and Tab (Vim=true)",
        page: "- First thought|",
        vim: true,
        steps: &[
            ("A enter", "- First thought\n- |"),
            ("tab C h i l d", "- First thought\n  - Child|"),
            (
                "enter tab G r a n d c h i l d",
                "- First thought\n  - Child\n    - Grandchild|",
            ),
            ("shift-tab", "- First thought\n  - Child\n  - Grandchild|"),
            (
                "escape enter tab",
                "- First thought\n  - Child\n  - |Grandchild",
            ),
        ],
    },
    Case {
        rt: "leaves a list with repeated Enter: new bullet, outdent, then plain line",
        page: "- a\n  - b|\n",
        vim: false,
        steps: &[
            ("enter", "- a\n  - b\n  - |\n"),
            ("enter", "- a\n  - b\n- |\n"),
            ("enter", "- a\n  - b\n|\n"),
            ("a f t e r", "- a\n  - b\nafter|\n"),
        ],
    },
    Case {
        rt: "keeps the caret out of collapsed children when stepping across lines",
        page: "- Parent| ^p\n  - Kid\n- After\n",
        vim: false,
        steps: &[
            ("MOD-alt-[ right", "- Parent ^p\n  - Kid\n- |After\n"),
            ("left", "- Parent| ^p\n  - Kid\n- After\n"),
        ],
    },
    Case {
        rt: "moves a block whose ID also exists on another page without dropping it",
        page: "- A\n- |B ^dup\n",
        vim: false,
        steps: &[("alt-up", "- |B ^dup\n- A\n")],
    },
    Case {
        rt: "opens a block with Vim o on an empty block, and repeats it with .",
        page: "- one\n- |\n- three\n",
        vim: true,
        steps: &[
            ("o n e w escape", "- one\n- \n- ne|w\n- three\n"),
            (
                "g g o d o t escape .",
                "- one\n- dot\n- do|t\n- \n- new\n- three\n",
            ),
        ],
    },
    Case {
        rt: "keeps a hidden ID through Vim D and C",
        page: "- al|pha ^abc\n- two\n",
        vim: true,
        steps: &[("shift-d", "- a|l ^abc\n- two\n")],
    },
    Case {
        rt: "keeps a hidden ID through Vim D and C: C",
        page: "- a|l ^abc\n- two\n",
        vim: true,
        steps: &[("shift-c Z escape", "- a|Z ^abc\n- two\n")],
    },
    Case {
        rt: "keeps Vim's line commands inside the outline",
        page: "- |one\n- two ^t2\n  - child\n- three\n",
        vim: true,
        steps: &[
            (
                "o n e w escape",
                "- one\n- ne|w\n- two ^t2\n  - child\n- three\n",
            ),
            (
                "j c c X escape",
                "- one\n- new\n- |X ^t2\n  - child\n- three\n",
            ),
            ("> >", "- one\n- new\n  - |X ^t2\n    - child\n- three\n"),
            (
                "g g shift-j",
                "- one| new\n  - X ^t2\n    - child\n- three\n",
            ),
        ],
    },
];

#[gpui::test]
fn outline_scenarios_match_rt_editor_tests(cx: &mut TestAppContext) {
    for case in CASES {
        let window = write_view(case.page, case.vim, cx);
        for (input, expected) in case.steps {
            keys(window, &input.replace("MOD", MOD), cx);
            assert_eq!(state(window, cx), *expected, "{}: after {input:?}", case.rt);
        }
    }
}

/// rt: "never turns the line above a typed - or --- into a heading". Quire's grammar has no
/// setext headings, so only `# Real` is a heading and `---` after a paragraph is a rule.
#[gpui::test]
fn a_typed_dash_never_turns_the_line_above_into_a_heading(cx: &mut TestAppContext) {
    let text = "Abc\n-\n\nDef\n---\n\n# Real\n";
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
    let buffer = cx.update(|cx| crate::markdown::buffer(text, cx));
    cx.executor().run_until_parked();
    buffer.read_with(cx, |buffer, _| {
        let snapshot = buffer.snapshot();
        let blocks = crate::markdown_blocks::current_blocks(&snapshot).expect("parsed blocks");
        let headings: Vec<_> = blocks
            .iter()
            .filter(|block| block.kind == "heading")
            .map(|block| &text[block.content_from..block.content_to])
            .collect();
        assert_eq!(headings, ["# Real"]);
        let tree = snapshot
            .syntax_layers()
            .next()
            .expect("a block syntax layer")
            .node()
            .to_sexp();
        assert_eq!(tree.matches("thematic_break").count(), 1, "{tree}");
        assert!(!tree.contains("setext"), "{tree}");
    });
}

/// rt: "removes a new block's ID with the block on undo". Copying a reference gives the new
/// block an ID while it is still being typed; undoing the typing takes the ID with it, so no
/// orphan ID is left.
#[gpui::test]
fn undo_removes_a_new_blocks_id_with_the_block(cx: &mut TestAppContext) {
    let window = write_view("- A|\n", false, cx);
    keys(window, "enter x", cx);
    cx.update_window(window.into(), |_, window, cx| {
        window.dispatch_action(Box::new(crate::page_menu::CopyBlockReference), cx)
    })
    .unwrap();
    cx.executor().run_until_parked();
    let with_id = state(window, cx);
    assert!(with_id.starts_with("- A\n- x| ^"), "{with_id}");
    keys(window, &format!("{MOD}-z {MOD}-z"), cx);
    assert_eq!(state(window, cx), "- A|\n");
}

/// rt: "toggles nested blocks through clear disclosure bullets without writing block IDs".
#[gpui::test]
fn bullet_arrows_toggle_nested_blocks_without_writing_ids(cx: &mut TestAppContext) {
    let page = "- Parent|\n  - Child\n    - Grandchild\n- Sibling\n";
    let window = write_view(page, false, cx);
    super::tests::click(window, "quire-bullet-fold", cx);
    assert!(!displayed(window, cx).contains("Grandchild"));
    super::tests::click(window, "quire-bullet-fold", cx);
    assert!(displayed(window, cx).contains("Grandchild"));
    assert_eq!(state(window, cx), page, "folding writes no block ID");
}

/// Each display row's text and the indent of the row that continues it, after the page is
/// drawn in a narrow window with soft wrap on.
fn wrapped_rows(page: &str, cx: &mut TestAppContext) -> Vec<(String, Option<u32>)> {
    let window = write_view(&format!("|{page}"), false, cx);
    window
        .update(cx, |editor, _, cx| {
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx)
        })
        .unwrap();
    cx.simulate_window_resize(window.into(), gpui::size(gpui::px(480.), gpui::px(900.)));
    // The first frame measures the bullets; the wrap that follows indents past them.
    for _ in 0..2 {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.executor().run_until_parked();
    }
    window
        .update(cx, |editor, window, cx| {
            let snapshot = editor.snapshot(window, cx).display_snapshot;
            snapshot
                .text()
                .lines()
                .enumerate()
                .map(|(row, line)| {
                    let row = editor::display_map::DisplayRow(row as u32);
                    (line.to_owned(), snapshot.soft_wrap_indent(row))
                })
                .collect()
        })
        .unwrap()
}

/// The columns a line starting with `text`, after any checkbox, wraps its continuation rows to.
fn hang(rows: &[(String, Option<u32>)], text: &str) -> u32 {
    rows.iter()
        .find(|(line, _)| line.trim_start_matches([' ', '☐']).starts_with(text))
        .and_then(|(_, indent)| *indent)
        .unwrap_or_else(|| panic!("{text:?} should wrap: {rows:#?}"))
}

const LONG: &str = "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda";

/// rt `block-wrapping.test.ts`: "measures list, ordered and checkbox prefixes without counting
/// body text". Wrapped rows of a list item hang under its text, past the bullet, a nested
/// item's indentation and a task's checkbox; a paragraph's rows do not indent.
#[gpui::test]
fn wrapped_list_lines_hang_under_their_text(cx: &mut TestAppContext) {
    let rows = wrapped_rows(
        &format!(
            "- Parent {LONG}\n  - [ ] Child {LONG}\n\n12. Ordered {LONG}\n\nParagraph {LONG}\n"
        ),
        cx,
    );
    let (parent, child, ordered) = (
        hang(&rows, "Parent"),
        hang(&rows, "Child"),
        hang(&rows, "Ordered"),
    );
    assert!(parent >= 2, "a bullet's width hangs its rows: {rows:#?}");
    assert!(
        child >= parent + 4,
        "nesting and the checkbox add to it: {rows:#?}"
    );
    assert!(ordered >= 2, "a number hangs its rows too: {rows:#?}");
    assert_eq!(hang(&rows, "Paragraph"), 0, "{rows:#?}");
}

/// rt `block-wrapping.test.ts`: "uses tab stops for source indentation and ignores list syntax
/// in code".
#[gpui::test]
fn wrapped_rows_count_tab_stops_and_leave_code_alone(cx: &mut TestAppContext) {
    let rows = wrapped_rows(
        &format!("- Parent {LONG}\n\t- Child {LONG}\n\n```md\n- Not an outline {LONG}\n```\n"),
        cx,
    );
    assert!(
        hang(&rows, "Child") >= hang(&rows, "Parent") + 4,
        "{rows:#?}"
    );
    assert_eq!(hang(&rows, "- Not an outline"), 0, "{rows:#?}");
}
