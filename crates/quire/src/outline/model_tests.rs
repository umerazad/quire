use super::*;
use gpui::TestAppContext;

fn blocks(text: &str, cx: &mut TestAppContext) -> Vec<Block> {
    cx.update(|cx| {
        if cx.try_global::<settings::SettingsStore>().is_none() {
            crate::editor_host::init(cx).expect("editor initialization");
        }
        let buffer = crate::markdown::buffer(text, cx);
        crate::markdown_blocks::current_blocks(&buffer.read(cx).snapshot())
            .expect("Markdown blocks")
    })
}

fn applied(text: &str, outcome: Option<Outcome>) -> Option<(String, usize)> {
    match outcome? {
        Outcome::Apply(plan) => Some((apply_edits(text, &plan.edits), plan.cursor)),
        Outcome::Refuse(message) => Some((message.to_owned(), usize::MAX)),
    }
}

fn no_folds(_: usize) -> bool {
    false
}

#[gpui::test]
fn split_continues_lists_and_leaves_other_blocks(cx: &mut TestAppContext) {
    let split_at = |text: &str, head: usize, zoom: Option<&str>, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        applied(text, split(text, &blocks, head, zoom, no_folds))
    };
    assert_eq!(split_at("First thought", 13, None, cx), None);
    assert_eq!(split_at("First second ^stable", 5, None, cx), None);
    assert_eq!(split_at("First\ncontinued", 15, None, cx), None);
    for id in ["", " ^parent"] {
        let text = format!("- Parent{id}\n  - Child ^child\n");
        assert_eq!(
            split_at(&text, 8, None, cx),
            Some((
                format!("- Parent{id}\n  - \n  - Child ^child\n"),
                format!("- Parent{id}\n  - ").len()
            ))
        );
    }
    assert_eq!(
        split_at("- Parent\n  - ", 13, None, cx),
        Some(("- Parent\n- ".into(), 11))
    );
    assert_eq!(
        split_at("- Parent\n- ", 11, None, cx),
        Some(("- Parent\n".into(), 9))
    );
    for (marker, next) in [
        ("[x]", "[ ]"),
        ("{{DONE}}", "{{TODO}}"),
        ("{{[[DONE]]}}", "{{[[TODO]]}}"),
    ] {
        let text = format!("- {marker} Action ^task");
        let expected = format!("{text}\n- {next} ");
        let cursor = expected.len();
        assert_eq!(
            split_at(&text, text.len(), None, cx),
            Some((expected, cursor))
        );
    }
    assert_eq!(
        split_at("- [ ] Action ^task", 2, None, cx),
        Some(("- [ ] \n- [ ] Action ^task".into(), 9))
    );
    assert_eq!(
        split_at("- Alpha beta ^a\n- Next", 7, None, cx),
        Some(("- Alpha ^a\n- beta\n- Next".into(), 13))
    );
    assert_eq!(
        split_at("9. Nine\n", 7, None, cx),
        Some(("9. Nine\n10. \n".into(), 12))
    );
    let nested = "- Parent\n  - Kid\n- After\n";
    assert_eq!(
        split_at("- Top\n- Root ^z\n- After\n", 12, Some("z"), cx),
        Some(("- Top\n- Root ^z\n  - \n- After\n".into(), 20))
    );
    let blocks = blocks(nested, cx);
    assert_eq!(
        applied(nested, split(nested, &blocks, 8, None, |at| at == 8)),
        Some(("- Parent\n  - Kid\n- \n- After\n".into(), 19))
    );
}

#[gpui::test]
fn split_ignores_code_and_headings_by_block_kind(cx: &mut TestAppContext) {
    for text in [
        "# Heading",
        "```js\nconst x = 1;\n```",
        "<div>html</div>",
        "---\ntitle: Test\n---\n",
    ] {
        let blocks = blocks(text, cx);
        assert_eq!(
            split(text, &blocks, 5.min(text.len()), None, no_folds),
            None,
            "{text:?}"
        );
    }
}

#[gpui::test]
fn indent_moves_subtrees_and_keeps_ids(cx: &mut TestAppContext) {
    let run = |text: &str, head: usize, outdent: bool, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        applied(text, indent(text, &blocks, head, outdent, None))
    };
    let text = "- Parent ^p\n- Sibling ^s\n  - Leaf ^l\n- After ^a\n";
    let (result, head) = run(text, text.find("Sibling").unwrap(), false, cx).unwrap();
    assert_eq!(
        result,
        "- Parent ^p\n  - Sibling ^s\n    - Leaf ^l\n- After ^a\n"
    );
    assert!(result[head..].starts_with("Sibling"));
    for ending in ["", "\n"] {
        let text = format!("- Parent ^p\n  - First ^f\n    - Leaf ^l\n  - Second ^s{ending}");
        let (result, head) = run(&text, text.find("First").unwrap(), true, cx).unwrap();
        assert_eq!(
            result,
            format!(
                "- Parent ^p\n  - Second ^s\n- First ^f\n  - Leaf ^l{}",
                "\n"
            )
        );
        assert!(result[head..].starts_with("First"));
    }
    let text = "10. Parent ^p\n11. Child ^c";
    assert_eq!(
        run(text, text.find("Child").unwrap(), false, cx).map(|(text, _)| text),
        Some("10. Parent ^p\n    11. Child ^c".into())
    );
    for outdent in [false, true] {
        assert_eq!(
            run("- First ^id", 4, outdent, cx),
            Some(("- First ^id".into(), 4))
        );
    }
}

#[gpui::test]
fn bare_bullets_become_bullets(_: &mut TestAppContext) {
    let run = |text: &str| {
        bare_bullet(text, text.len()).map(|plan| (apply_edits(text, &plan.edits), plan.cursor))
    };
    assert_eq!(run("Abc\n-"), Some(("Abc\n- ".into(), 6)));
    assert_eq!(run("Abc\n  *"), Some(("Abc\n  * ".into(), 8)));
    assert_eq!(run("Abc\n-x"), None);
    assert_eq!(run("Abc\n- "), None);
}

#[gpui::test]
fn backspace_and_delete_merge_blocks(cx: &mut TestAppContext) {
    let back = |text: &str, at: usize, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        applied(text, merge_backward(text, &blocks, at))
    };
    let forward = |text: &str, at: usize, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        applied(text, merge_forward(text, &blocks, at))
    };
    let text = "- Alpha\n- Beta\n  - Kid\n";
    assert_eq!(
        back(text, text.find("Beta").unwrap(), cx),
        Some(("- AlphaBeta\n  - Kid\n".into(), 7))
    );
    assert_eq!(
        back("- Alpha ^keep\n- Beta\n", 16, cx),
        Some(("- AlphaBeta ^keep\n".into(), 7))
    );
    assert_eq!(
        back("- Alpha\n- \n- Beta\n", 10, cx),
        Some(("- Alpha\n- Beta\n".into(), 7))
    );
    let task = "- Alpha\n- [ ] Task\n";
    assert_eq!(
        back(task, task.find("Task").unwrap(), cx),
        Some(("- Alpha\n- Task\n".into(), task.find("[ ]").unwrap()))
    );
    let referenced = "- Alpha\n- Beta ^ref\n";
    assert_eq!(
        back(referenced, referenced.find("Beta").unwrap(), cx),
        Some((REFERENCED.into(), usize::MAX))
    );
    assert_eq!(back("- Alpha\n", 2, cx), Some(("- Alpha\n".into(), 2)));
    assert_eq!(back("- Alpha\n", 4, cx), None);
    for merged in [back("- \n- Beta\n", 2, cx), forward("- \n- Beta\n", 2, cx)] {
        assert_eq!(merged, Some(("- Beta\n".into(), 2)));
    }
    assert_eq!(
        back("- [ ] \n  - Kid\n    - Grand\n- Next\n", 2, cx),
        Some(("- Kid\n  - Grand\n- Next\n".into(), 2))
    );
    assert_eq!(
        forward("- \n- Beta ^ref\n", 2, cx),
        Some(("- Beta ^ref\n".into(), 2))
    );
    assert_eq!(back("- \n", 2, cx), Some(("- \n".into(), 2)));
    let titled = "# Notes\n\n```\ncode\n```\n- \n- Next\n";
    let empty = titled.find("- \n").unwrap() + 2;
    assert_eq!(
        back(titled, empty, cx),
        Some(("# Notes\n\n```\ncode\n```\n- Next\n".into(), empty))
    );
    assert_eq!(
        forward(text, 7, cx),
        Some(("- AlphaBeta\n  - Kid\n".into(), 7))
    );
    assert_eq!(
        forward("- Alpha\n  - Kid\n- Next\n", 7, cx),
        Some(("- AlphaKid\n- Next\n".into(), 7))
    );
    assert_eq!(
        forward("- Alpha ^keep\n- [ ] Beta\n", 7, cx),
        Some(("- AlphaBeta ^keep\n".into(), 7))
    );
    assert_eq!(forward(text, 3, cx), None);
    let deep = "- Parent\n  - Kid\n    - Grand\n      - Great\n- Next\n";
    assert_eq!(
        back(deep, deep.find("Kid").unwrap(), cx),
        Some(("- ParentKid\n  - Grand\n    - Great\n- Next\n".into(), 8))
    );
    assert_eq!(
        forward("- Parent\n  - Kid\n    - Grand\n- Next\n", 8, cx),
        Some(("- ParentKid\n  - Grand\n- Next\n".into(), 8))
    );
    let empty_id = "- ^a\n- text\n";
    assert_eq!(
        back(empty_id, empty_id.find("text").unwrap(), cx),
        Some(("- text ^a\n".into(), 2))
    );
    assert_eq!(forward(empty_id, 2, cx), Some(("- text ^a\n".into(), 2)));
    let blocks = blocks(empty_id, cx);
    assert_eq!(
        applied(empty_id, join(empty_id, &blocks, 0)),
        Some(("- text ^a\n".into(), 2))
    );
}

#[gpui::test]
fn blocks_move_with_children_among_siblings(cx: &mut TestAppContext) {
    let run = |text: &str, head: usize, up: bool, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        applied(text, move_block(text, &blocks, head, up))
    };
    let text = "- A\n- B\n  - B1\n- C\n";
    let b = text.find("B").unwrap();
    assert_eq!(
        run(text, b, true, cx),
        Some(("- B\n  - B1\n- A\n- C\n".into(), 2))
    );
    assert_eq!(
        run(text, b, false, cx),
        Some(("- A\n- C\n- B\n  - B1\n".into(), 10))
    );
    assert_eq!(run(text, 2, true, cx), Some((text.into(), 2)));
    let kid = text.find("B1").unwrap();
    assert_eq!(run(text, kid, true, cx), Some((text.into(), kid)));
    assert_eq!(run("- A\n- B", 6, true, cx), Some(("- B\n- A".into(), 2)));
    assert_eq!(run("Paragraph\n", 3, true, cx), None);
}

#[gpui::test]
fn multi_line_paste_makes_blocks(cx: &mut TestAppContext) {
    let run = |text: &str, head: usize, pasted: &str, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        paste(text, &blocks, head, pasted).map(|plan| (apply_edits(text, &plan.edits), plan.cursor))
    };
    let expected = "- Alphaone\n- two\n  - two-kid\n- threeOmega\n";
    assert_eq!(
        run(
            "- AlphaOmega\n",
            7,
            "- one\n- two\n  - two-kid\n- three",
            cx
        ),
        Some((expected.into(), expected.find("Omega").unwrap()))
    );
    let child = "- P\n  - Kid ^k\n    - Grand\n- Next\n";
    let expected = "- P\n  - Kidone ^k\n    - Grand\n    - one-kid\n  - two\n- Next\n";
    assert_eq!(
        run(
            child,
            child.find(" ^k").unwrap(),
            "- one\n  - one-kid\n- two",
            cx
        ),
        Some((expected.into(), expected.find("\n- Next").unwrap()))
    );
    assert_eq!(
        run("- AlphaOmega\n", 7, "line one\nline two", cx).map(|(text, _)| text),
        Some("- Alphaline one\n- line twoOmega\n".into())
    );
    assert_eq!(run("- AlphaOmega\n", 7, "```\ncode\n```", cx), None);
    assert_eq!(run("- AlphaOmega\n", 7, "single", cx), None);
    assert_eq!(run("Paragraph\n", 4, "a\nb", cx), None);
    assert_eq!(
        run("- ^z\n", 2, "- A ^n1\n- B ^n2", cx),
        Some(("- A ^z\n- B ^n2\n".into(), 10))
    );
    assert_eq!(
        run("- foo ^y\n", 5, "- A ^n1\n- B ^n2", cx).map(|(text, _)| text),
        Some("- fooA ^y\n- B ^n2\n".into())
    );
    assert_eq!(
        run("- foo\n  - kid\n", 4, "- A ^n1\n- B ^n2", cx),
        Some(("- foA ^n1\n  - kid\n- Bo ^n2\n".into(), 21))
    );
    assert_eq!(
        run("1. one\n2. two\n", 6, "a\n  - b\nc", cx).map(|(text, _)| text),
        Some("1. onea\n   - b\n1. c\n2. two\n".into())
    );
}

#[gpui::test]
fn vim_open_line_opens_sibling_blocks(cx: &mut TestAppContext) {
    let run = |text: &str, head: usize, below: bool, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        open_line(text, &blocks, head, below, None, no_folds)
            .map(|plan| (apply_edits(text, &plan.edits), plan.cursor))
    };
    assert_eq!(
        run("- One ^a\n- Two\n", 3, true, cx),
        Some(("- One ^a\n- \n- Two\n".into(), 11))
    );
    assert_eq!(
        run("- One ^a\n  - Kid\n", 3, true, cx),
        Some(("- One ^a\n  - \n  - Kid\n".into(), 13))
    );
    assert_eq!(
        run("- \n  - Kid\n", 0, true, cx),
        Some(("- \n  - Kid\n- \n".into(), 13))
    );
    assert_eq!(
        run("- One\n  - [ ] Two\n", 12, false, cx),
        Some(("- One\n  - \n  - [ ] Two\n".into(), 10))
    );
    assert_eq!(run("Paragraph\n", 3, true, cx), None);
}

#[test]
fn positions_map_like_codemirror() {
    let edits = normalize(vec![
        (5..5, "ab".into()),
        (5..8, String::new()),
        (10..10, "z".into()),
    ]);
    assert_eq!(edits, vec![(5..8, "ab".into()), (10..10, "z".into())]);
    assert_eq!(map_position(&edits, 4, true), 4);
    assert_eq!(map_position(&edits, 5, false), 5);
    assert_eq!(map_position(&edits, 5, true), 5);
    assert_eq!(map_position(&edits, 6, true), 7);
    assert_eq!(map_position(&edits, 6, false), 5);
    assert_eq!(map_position(&edits, 8, false), 7);
    assert_eq!(map_position(&edits, 10, false), 9);
    assert_eq!(map_position(&edits, 10, true), 10);
    assert_eq!(map_position(&edits, 12, true), 12);
}

#[gpui::test]
fn numbered_runs_renumber_only_where_edited(cx: &mut TestAppContext) {
    use crate::outline::lists::{Style, numbered_list_changes, renumber_changes};
    let renumber = |text: &str, from: usize, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        apply_edits(
            text,
            &renumber_changes(text, &blocks, |a, b| a <= from && b >= from),
        )
    };
    assert_eq!(
        renumber("1. One\n2. Mid\n2. Two\n3. Three\n", 8, cx),
        "1. One\n2. Mid\n3. Two\n4. Three\n"
    );
    assert_eq!(
        renumber("1. One\n   1. Two\n   3. More\n3. Three\n", 21, cx),
        "1. One\n   1. Two\n   2. More\n2. Three\n"
    );
    assert_eq!(
        renumber("1. A\n1. B\n\nPara\n\n- x\n", 22, cx),
        "1. A\n1. B\n\nPara\n\n- x\n"
    );
    let nine = "1. a\n2. b\n3. c\n4. d\n5. e\n6. f\n7. g\n8. h\n9. i\n9. j\n   - kid\n";
    assert!(renumber(nine, nine.find("9. j").unwrap(), cx).contains("10. j\n    - kid\n"));

    let number = |text: &str, from: usize, to: usize, style, cx: &mut TestAppContext| {
        let blocks = blocks(text, cx);
        numbered_list_changes(text, &blocks, from, to, style).map(|edits| apply_edits(text, &edits))
    };
    let text = "- A\n- B\n  - B1\n  - B2\n- C\n";
    assert_eq!(
        number(
            text,
            text.find("B1").unwrap(),
            text.find("B2").unwrap() + 1,
            None,
            cx
        ),
        Some("- A\n- B\n  1. B1\n  2. B2\n- C\n".into())
    );
    assert_eq!(
        number("1. A\n2. B\n", 3, 3, Some(Style::Numbered), cx),
        Some("1. A\n2. B\n".into())
    );
    assert_eq!(
        number("- A\n- B\n", 3, 3, Some(Style::Bullet), cx),
        Some("- A\n- B\n".into())
    );
    assert_eq!(number("Just a paragraph\n", 3, 3, None, cx), None);
}

/// A code span in an injected inline layer counts as code to its closing backtick, even around
/// `[[x]]`, where walking up with `Node::parent` lost the layer offset and skipped the span.
#[gpui::test]
fn carets_in_an_injected_code_span_are_literal(cx: &mut TestAppContext) {
    let input = "- `code [[x]]` y\n- z\n";
    blocks(input, cx);
    let buffer = cx.update(|cx| crate::markdown::buffer(input, cx));
    cx.run_until_parked();
    let literal: Vec<bool> = cx.update(|cx| {
        (2..=16)
            .map(|at| super::super::literal(&buffer, at, cx))
            .collect()
    });
    let mut expected = vec![false];
    expected.extend([true; 12]);
    expected.extend([false, false]);
    assert_eq!(literal, expected);
}

/// rt's Enter, Tab, and Shift-Tab at every caret position of its outline corpus, from
/// `tests/fixtures/outline-parity.json`, copied unchanged from rt. Offsets there are UTF-16.
/// The command layer's checks run first, as in the editor: a caret in code keeps its text, and
/// a bare `-` becomes a bullet before Tab moves it.
#[gpui::test]
fn outline_commands_match_rt_parity_vectors(cx: &mut TestAppContext) {
    let vectors: Vec<serde_json::Value> = serde_json::from_str(include_str!(
        "../../../../tests/fixtures/outline-parity.json"
    ))
    .expect("outline parity vectors");
    let byte = |text: &str, utf16: u64| {
        let mut units = 0;
        text.char_indices()
            .find(|(_, c)| {
                let found = units >= utf16;
                units += c.len_utf16() as u64;
                found
            })
            .map_or(text.len(), |(at, _)| at)
    };
    let mut parsed: Vec<(String, Vec<Block>, gpui::Entity<language::Buffer>)> = Vec::new();
    let mut mismatches = Vec::new();
    for vector in &vectors {
        let input = vector["input"].as_str().unwrap();
        if parsed.last().is_none_or(|(text, ..)| text != input) {
            let parsed_blocks = blocks(input, cx);
            let buffer = cx.update(|cx| crate::markdown::buffer(input, cx));
            cx.run_until_parked();
            parsed.push((input.to_owned(), parsed_blocks, buffer));
        }
        let (_, blocks, buffer) = parsed.last().unwrap();
        let head = byte(input, vector["pos"].as_u64().unwrap());
        let command = vector["command"].as_str().unwrap();
        let outcome = if cx.update(|cx| super::super::literal(buffer, head, cx)) {
            None
        } else if command == "split" {
            split(input, blocks, head, None, no_folds)
        } else {
            let outdent = command == "outdent";
            match bare_bullet(input, head) {
                Some(bullet) => {
                    let bulleted = apply_edits(input, &bullet.edits);
                    let then = indent(
                        &bulleted,
                        &self::blocks(&bulleted, cx),
                        bullet.cursor,
                        outdent,
                        None,
                    );
                    Some(Outcome::Apply(match then {
                        Some(Outcome::Apply(next)) => Plan {
                            edits: vec![(0..input.len(), apply_edits(&bulleted, &next.edits))],
                            cursor: next.cursor,
                        },
                        _ => bullet,
                    }))
                }
                None => indent(input, blocks, head, outdent, None),
            }
        };
        let output = vector["output"].as_str().unwrap();
        let expected = vector["handled"].as_bool().unwrap().then(|| {
            (
                output.to_owned(),
                byte(output, vector["head"].as_u64().unwrap()),
            )
        });
        let actual = match outcome {
            Some(Outcome::Apply(plan)) => Some((apply_edits(input, &plan.edits), plan.cursor)),
            Some(Outcome::Refuse(_)) => Some((input.to_owned(), head)),
            None => None,
        };
        if actual != expected {
            mismatches.push(format!(
                "{} at {head} in {input:?}: expected {expected:?}, got {actual:?}",
                vector["command"]
            ));
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {} vectors differ:\n{}",
        mismatches.len(),
        vectors.len(),
        mismatches.join("\n")
    );
}

#[test]
fn trimming_drops_the_text_an_edit_keeps() {
    let text = "- Top\n  - Kid1\n  - Kid2 ^k\n- After\n";
    let at = text.find("  - Kid1").unwrap()..text.find("- After").unwrap();
    let edits = vec![(at.clone(), "  - Kid2 ^k\n  - Kid1\n".to_owned())];
    let trimmed = trim_unchanged(text, edits.clone());
    assert_eq!(
        trimmed,
        [(at.start + 7..at.end - 1, "2 ^k\n  - Kid1".to_owned())]
    );
    assert_eq!(apply_edits(text, &trimmed), apply_edits(text, &edits));
    assert_eq!(
        trim_unchanged("é", vec![(0..2, "é".to_owned())]),
        [(2..2, String::new())]
    );
}
