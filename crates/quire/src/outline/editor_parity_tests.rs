//! rt's outline scenarios from `tests/editor.test.tsx` (`5ea5c22310ff`), as keystrokes in a
//! native Write view. Each case names the rt test it ports; `|` marks the caret. Typing goes
//! through key events, as a writer types, rather than rt's direct document dispatches.

use super::tests::{displayed, keys, state, write_view};
use gpui::TestAppContext;

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
        steps: &[("o n e w escape", "- one\n- \n- ne|w\n- three\n")],
    },
    Case {
        rt: "keeps a hidden ID through Vim D and C",
        page: "- al|pha ^abc\n- two\n",
        vim: true,
        steps: &[("shift-d", "- al| ^abc\n- two\n")],
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
