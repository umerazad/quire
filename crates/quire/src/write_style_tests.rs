use super::*;
use crate::write_style::{Style, applied};
use gpui::TestAppContext;
use multi_buffer::{MultiBufferOffset, ToOffset as _};

fn projected(text: &str, cx: &mut TestAppContext) -> gpui::WindowHandle<NotebookWindow> {
    let window = cx.add_window(|window, cx| {
        let view = NotebookWindow::new(text, window, cx);
        crate::editor_projection::bind(&view.second, window, cx).unwrap();
        view
    });
    cx.executor().run_until_parked();
    window
}

fn display(window: gpui::WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |view, window, cx| {
            view.second.update(cx, |editor, cx| {
                editor.snapshot(window, cx).display_snapshot.text()
            })
        })
        .unwrap()
}

fn caret_at(window: gpui::WindowHandle<NotebookWindow>, offset: usize, cx: &mut TestAppContext) {
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.second.focus_handle(cx), cx);
            view.second.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(offset)..MultiBufferOffset(offset)])
                })
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
}

fn caret(window: gpui::WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> usize {
    window
        .update(cx, |view, _, cx| {
            let editor = view.second.read(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            editor
                .selections
                .newest_anchor()
                .head()
                .to_offset(&snapshot)
                .0
        })
        .unwrap()
}

fn drawn(
    window: gpui::WindowHandle<NotebookWindow>,
    selector: &'static str,
    cx: &mut TestAppContext,
) -> Option<gpui::Bounds<gpui::Pixels>> {
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    visual.debug_bounds(selector)
}

const PAGE: &str = "- **bold** and *it* ^a\n- `code` ~~old~~ \\*\n## Head\n> quote\n";

#[gpui::test]
fn marks_hide_outside_the_caret_block_while_their_text_keeps_its_style(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let window = projected(PAGE, cx);
    assert_eq!(
        display(window, cx),
        "bold and it\ncode old *\nHead\n quote\n"
    );
    let style_of = |text: &str| {
        let start = PAGE.find(text).unwrap();
        start..start + text.len()
    };
    window
        .update(cx, |view, _, cx| {
            assert_eq!(
                applied(view.second.read(cx), cx),
                [
                    (Style::Strong, vec![style_of("**bold**")]),
                    (Style::Emphasis, vec![style_of("*it*")]),
                    (Style::Strike, vec![style_of("~~old~~")]),
                    (Style::Code, vec![style_of("`code`")]),
                    (Style::Heading, vec![style_of("## Head")]),
                    (Style::Quote, vec![style_of("> quote")]),
                ]
            );
        })
        .unwrap();
    caret_at(window, PAGE.find("bold").unwrap(), cx);
    assert_eq!(
        display(window, cx),
        "**bold** and *it*\ncode old *\nHead\n quote\n",
        "the caret's block shows its marks, others keep them hidden"
    );
    caret_at(window, PAGE.find("Head").unwrap(), cx);
    assert_eq!(
        display(window, cx),
        "bold and it\ncode old *\n## Head\n quote\n"
    );
    caret_at(window, PAGE.find("quote").unwrap(), cx);
    assert_eq!(
        display(window, cx),
        "bold and it\ncode old *\nHead\n> quote\n"
    );
    window
        .update(cx, |view, window, cx| {
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
        })
        .unwrap();
    assert_eq!(display(window, cx), PAGE, "Source shows every byte");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), PAGE);
            assert!(
                applied(view.second.read(cx), cx).is_empty(),
                "Source drops Write's styles"
            );
        })
        .unwrap();
}

#[gpui::test]
fn the_caret_never_lands_inside_a_hidden_mark(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- one\n- **two** three\n";
    let marks = [
        text.find("**two").unwrap()..text.find("two").unwrap(),
        text.find("** three").unwrap()..text.find(" three").unwrap(),
    ];
    let inside = |at: usize| marks.iter().any(|mark| mark.start < at && at < mark.end);
    for (vim, keys) in [(false, "down"), (true, "j"), (true, "w"), (false, "end")] {
        super::search_tests::set_vim_mode(cx, vim);
        let window = projected(text, cx);
        caret_at(window, 2, cx);
        for key in keys.split(' ') {
            cx.simulate_keystrokes(window.into(), key);
            cx.executor().run_until_parked();
        }
        let at = caret(window, cx);
        assert!(!inside(at), "vim {vim} {keys}: caret {at} inside a mark");
        if keys != "end" {
            assert!(at >= text.find("- **").unwrap(), "vim {vim} {keys}: {at}");
            assert_eq!(
                display(window, cx),
                "one\n**two** three\n",
                "vim {vim} {keys}: the caret's block shows its marks"
            );
        }
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), text)
            })
            .unwrap();
    }
    super::search_tests::set_vim_mode(cx, true);
    let window = projected(text, cx);
    caret_at(window, text.find("two").unwrap(), cx);
    cx.simulate_keystrokes(window.into(), "w");
    cx.executor().run_until_parked();
    assert_eq!(
        caret(window, cx),
        text.find("** three").unwrap(),
        "in the caret's block the marks show, so Vim's w stops at the closing mark as in Source"
    );
}

#[gpui::test]
fn rules_draw_off_their_line_and_show_their_dashes_on_it(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- above\n\n---\n\n- below\n";
    let window = projected(text, cx);
    assert!(drawn(window, "quire-rule", cx).is_some(), "rule drawn");
    assert!(!display(window, cx).contains("---"));
    caret_at(window, text.find("---").unwrap(), cx);
    assert!(drawn(window, "quire-rule", cx).is_none());
    assert!(display(window, cx).contains("\n---\n"));
    caret_at(window, 2, cx);
    assert!(drawn(window, "quire-rule", cx).is_some());
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), text)
        })
        .unwrap();
}

#[gpui::test]
fn tables_draw_off_the_caret_and_show_raw_rows_when_the_caret_moves_in(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "above\n\n| a | b |\n| --- | --: |\n| 1 | 2 |\n\nbelow\n";
    for (vim, down) in [(false, "down down"), (true, "j j")] {
        super::search_tests::set_vim_mode(cx, vim);
        let window = projected(text, cx);
        caret_at(window, 0, cx);
        let table = drawn(window, "quire-table", cx).expect("table drawn");
        assert!(table.size.height > gpui::px(0.));
        assert!(!display(window, cx).contains("| a |"));
        for key in down.split(' ') {
            cx.simulate_keystrokes(window.into(), key);
            cx.executor().run_until_parked();
        }
        let at = caret(window, cx);
        assert!(
            (text.find("| a").unwrap()..text.find("\n\nbelow").unwrap()).contains(&at),
            "vim {vim}: caret {at} moved into the table's source"
        );
        assert!(drawn(window, "quire-table", cx).is_none(), "vim {vim}");
        assert!(display(window, cx).contains("| a | b |\n| --- | --: |\n| 1 | 2 |"));
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), text)
            })
            .unwrap();
    }
}

#[gpui::test]
fn setext_underlines_never_make_headings_in_write(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "Title\n===\n";
    let window = projected(text, cx);
    assert_eq!(display(window, cx), text);
    window
        .update(cx, |view, _, cx| {
            assert!(applied(view.second.read(cx), cx).is_empty());
        })
        .unwrap();
}

#[gpui::test]
fn child_guides_hang_under_each_parent_bullet_through_its_subtree(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- parent\n  - child\n    - grandchild\n- next\n";
    let window = projected(text, cx);
    let parent = drawn(window, "quire-guide-0", cx).expect("parent guide");
    let child = drawn(window, "quire-guide-1", cx).expect("child guide");
    assert!(
        drawn(window, "quire-guide-2", cx).is_none(),
        "a leaf has no guide"
    );
    assert!(
        drawn(window, "quire-guide-3", cx).is_none(),
        "a leaf has no guide"
    );
    let next = drawn(window, "quire-bullet", cx).expect("the last bullet, at the page's left");
    assert_eq!(parent.size.width, gpui::px(2.));
    assert!(
        (parent.center().x - next.center().x).abs() <= gpui::px(1.),
        "the guide hangs under the dot: {parent:?} {next:?}"
    );
    assert!(
        child.origin.x > parent.origin.x,
        "a deeper parent's guide sits right of it"
    );
    let line = next.size.height;
    let first_row = next.origin.y - line * 3.;
    assert!(
        parent.origin.y > first_row + line / 2.,
        "starts below the dot: {parent:?} {next:?}"
    );
    assert!(
        (parent.bottom() - next.origin.y).abs() <= gpui::px(1.),
        "ends with the subtree's last line, before the next sibling: {parent:?} {next:?}"
    );
    assert!((child.bottom() - parent.bottom()).abs() <= gpui::px(1.));
    window
        .update(cx, |view, window, cx| {
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
        })
        .unwrap();
    assert!(
        drawn(window, "quire-guide-0", cx).is_none(),
        "Source keeps Zed's guides"
    );
}

#[gpui::test]
fn urdu_lines_are_set_in_nastaliq_in_write(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- عربي فقط\n- یہ اردو ہے\n";
    let window = projected(text, cx);
    window
        .update(cx, |view, _, cx| {
            let (style, ranges) =
                crate::write_style::urdu_applied(view.second.read(cx), cx).expect("Urdu runs");
            assert_eq!(style.font_family, Some(crate::script_fonts::URDU));
            assert_eq!(
                ranges
                    .iter()
                    .map(|range| &text[range.clone()])
                    .collect::<Vec<_>>(),
                ["یہ", "اردو", "ہے"],
                "only the Urdu line, as rt marks lines with Urdu letters"
            );
        })
        .unwrap();
}

/// rt's `scriptLines` sets Urdu lines in Nastaliq in Source as in Write, and Source follows edits.
#[gpui::test]
fn urdu_lines_are_set_in_nastaliq_in_source(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- عربي فقط\n- یہ اردو ہے\n";
    let window = projected(text, cx);
    window
        .update(cx, |view, window, cx| {
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
        })
        .unwrap();
    cx.executor().run_until_parked();
    let urdu = |cx: &mut TestAppContext| {
        window
            .update(cx, |view, _, cx| {
                let editor = view.second.read(cx);
                assert!(!crate::editor_projection::enabled(editor), "Source");
                let text = editor.text(cx);
                let (style, ranges) =
                    crate::write_style::urdu_applied(editor, cx).unwrap_or_default();
                if !ranges.is_empty() {
                    assert_eq!(style.font_family, Some(crate::script_fonts::URDU));
                }
                ranges
                    .into_iter()
                    .map(|range| text[range].to_owned())
                    .collect::<Vec<_>>()
            })
            .unwrap()
    };
    assert_eq!(urdu(cx), ["یہ", "اردو", "ہے"]);
    window
        .update(cx, |view, _, cx| {
            view.second.update(cx, |editor, cx| {
                let end = editor.buffer().read(cx).len(cx);
                editor.buffer().update(cx, |buffer, cx| {
                    buffer.edit([(end..end, "- ٹھیک\n")], None, cx)
                })
            })
        })
        .unwrap();
    cx.executor().run_until_parked();
    assert_eq!(urdu(cx), ["یہ", "اردو", "ہے", "ٹھیک"]);
}

#[gpui::test]
fn a_page_opened_in_source_sets_urdu_in_nastaliq(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- یہ اردو ہے\n";
    let window = cx.add_window(|window, cx| {
        let buffer = crate::markdown::buffer(text, cx);
        Editor::for_buffer(buffer, None, window, cx)
    });
    cx.executor().run_until_parked();
    window
        .update(cx, |editor, _, cx| {
            let (_, ranges) = crate::write_style::urdu_applied(editor, cx).expect("Urdu runs");
            assert_eq!(
                ranges
                    .into_iter()
                    .map(|range| &text[range])
                    .collect::<Vec<_>>(),
                ["یہ", "اردو", "ہے"]
            );
        })
        .unwrap();
}
