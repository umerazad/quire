use super::*;
use gpui::{TestAppContext, WindowHandle};
use multi_buffer::{MultiBufferOffset, ToOffset as _};

const MOD: &str = if cfg!(target_os = "macos") {
    "cmd"
} else {
    "ctrl"
};

/// A page window whose second pane shows `marked` in Write, focused, with `|` as its caret.
fn written(marked: &str, vim: bool, cx: &mut TestAppContext) -> WindowHandle<NotebookWindow> {
    cx.update(|cx| init(cx).expect("editor initialization"));
    search_tests::set_vim_mode(cx, vim);
    let caret = marked.find('|').expect("caret marker");
    let text = marked.replacen('|', "", 1);
    let window = cx.add_window(|window, cx| NotebookWindow::new(&text, window, cx));
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            view.show_written(window, cx);
            let editor = view.second.clone();
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(editor::SelectionEffects::no_scroll(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(caret)..MultiBufferOffset(caret)])
                })
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    window
}

fn keys(window: WindowHandle<NotebookWindow>, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), keys);
    cx.executor().run_until_parked();
}

/// The text the pane's shown editor holds, with `|` at its caret, or `[` `]` around a selection.
fn shown(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |view, _, cx| {
            let editor = view.shown(1);
            let editor = editor.read(cx);
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            let text = snapshot.text();
            let selection = editor.selections.newest_anchor();
            let (start, end) = (
                selection.start.to_offset(&snapshot).0,
                selection.end.to_offset(&snapshot).0,
            );
            if start == end {
                format!("{}|{}", &text[..start], &text[start..])
            } else {
                format!("{}[{}]{}", &text[..start], &text[start..end], &text[end..])
            }
        })
        .unwrap()
}

fn page_text(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |view, _, cx| view.source.read(cx).text(cx))
        .unwrap()
}

fn zoomed(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> bool {
    window
        .update(cx, |view, _, _| view.zooms[1].is_some())
        .unwrap()
}

fn notice(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| crate::outline::notice(cx).map(|notice| notice.to_string()))
}

#[gpui::test]
fn zoom_shows_one_item_keeps_edits_inside_and_zooms_back_out(cx: &mut TestAppContext) {
    let window = written("- Alpha\n- Par|ent ^p\n  - Kid\n- Omega\n", false, cx);
    keys(window, &format!("{MOD}-."), cx);
    assert!(zoomed(window, cx));
    assert_eq!(shown(window, cx), "- Par|ent ^p\n  - Kid");

    keys(window, &format!("{MOD}-a"), cx);
    assert_eq!(shown(window, cx), "- [Parent ^p\n  - Kid]");

    // Enter at the zoomed item's end makes a child, since a sibling would leave the view.
    keys(window, "left end enter", cx);
    assert_eq!(shown(window, cx), "- Parent ^p\n  - |\n  - Kid");
    keys(window, "N e w", cx);
    // A child cannot be outdented out of the zoomed item.
    keys(window, "shift-tab", cx);
    assert_eq!(shown(window, cx), "- Parent ^p\n  - New|\n  - Kid");
    assert_eq!(
        page_text(window, cx),
        "- Alpha\n- Parent ^p\n  - New\n  - Kid\n- Omega\n"
    );

    // Moving the zoomed item past its sibling would edit the page outside the view.
    keys(window, "up home alt-up", cx);
    assert_eq!(notice(cx).as_deref(), Some(crate::outline::OUTSIDE_VIEW));
    assert_eq!(
        page_text(window, cx),
        "- Alpha\n- Parent ^p\n  - New\n  - Kid\n- Omega\n"
    );
    // Its own line is in view: it nests under the item above and stays zoomed, and it cannot
    // be outdented back out from inside the zoom.
    keys(window, "tab", cx);
    assert_eq!(
        page_text(window, cx),
        "- Alpha\n  - Parent ^p\n    - New\n    - Kid\n- Omega\n"
    );
    keys(window, "shift-tab", cx);
    assert_eq!(shown(window, cx), "- |Parent ^p\n    - New\n    - Kid");

    keys(window, &format!("{MOD}-shift-."), cx);
    assert!(!zoomed(window, cx));
    assert_eq!(
        shown(window, cx),
        "- Alpha\n  - |Parent ^p\n    - New\n    - Kid\n- Omega\n"
    );
    window
        .update(cx, |view, window, cx| {
            assert!(view.second.focus_handle(cx).is_focused(window))
        })
        .unwrap();
}

#[gpui::test]
fn zooming_an_item_without_an_id_gives_it_one_as_an_undo_step(cx: &mut TestAppContext) {
    let window = written("- Alpha\n- Pa|rent\n  - Kid\n", false, cx);
    keys(window, &format!("{MOD}-."), cx);
    let text = page_text(window, cx);
    let id = text
        .split('^')
        .nth(1)
        .and_then(|rest| rest.split('\n').next())
        .expect("the zoomed item gets an ID")
        .to_owned();
    assert_eq!(text, format!("- Alpha\n- Parent ^{id}\n  - Kid\n"));
    assert_eq!(shown(window, cx), format!("- Pa|rent ^{id}\n  - Kid"));

    // The zoom shares the page's history: typing and undo stay in step.
    keys(window, "x", cx);
    assert_eq!(shown(window, cx), format!("- Pax|rent ^{id}\n  - Kid"));
    keys(window, &format!("{MOD}-z"), cx);
    assert_eq!(
        page_text(window, cx),
        format!("- Alpha\n- Parent ^{id}\n  - Kid\n")
    );
}

#[gpui::test]
fn zoom_refuses_outside_list_items_and_source_view(cx: &mut TestAppContext) {
    let window = written("Para|graph\n\n- Item\n", false, cx);
    keys(window, &format!("{MOD}-."), cx);
    assert!(!zoomed(window, cx));
    assert_eq!(
        notice(cx).as_deref(),
        Some(crate::outline::zoom::NOT_IN_BLOCK)
    );
    assert_eq!(page_text(window, cx), "Paragraph\n\n- Item\n");
}

#[gpui::test]
fn vim_open_line_and_outdent_respect_the_zoomed_item(cx: &mut TestAppContext) {
    let window = written("- Top\n  - Par|ent ^p\n    - Kid\n", true, cx);
    keys(window, &format!("{MOD}-."), cx);
    assert_eq!(shown(window, cx), "- Par|ent ^p\n    - Kid");
    keys(window, "o", cx);
    assert_eq!(shown(window, cx), "- Parent ^p\n    - |\n    - Kid");
    keys(window, "C h i l d escape j < <", cx);
    assert_eq!(
        page_text(window, cx),
        "- Top\n  - Parent ^p\n    - Child\n    - Kid\n"
    );
    // Above the zoomed item would be out of view.
    keys(window, "g g shift-o", cx);
    assert_eq!(notice(cx).as_deref(), Some(crate::outline::OUTSIDE_VIEW));
    assert_eq!(
        page_text(window, cx),
        "- Top\n  - Parent ^p\n    - Child\n    - Kid\n"
    );
}

#[gpui::test]
fn outline_commands_work_in_an_embedded_block_view(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    search_tests::set_vim_mode(cx, false);
    let note = "before\n\n- Parent ^parent\n  - child ^child\n- outside ^outside\n";
    let buffer = cx.update(|cx| crate::markdown::buffer(note, cx));
    let page = cx.new(|cx| crate::page_analysis::PageAnalysis::new(buffer.clone(), cx));
    let window = cx.add_window(|window, cx| NotebookWindow::for_block(page, "parent", window, cx));
    cx.executor().run_until_parked();
    window
        .update(cx, |view, window, cx| {
            crate::editor_projection::bind(&view.second, window, cx).unwrap();
            window.focus(&view.second.focus_handle(cx), cx);
            let end = "- Parent ^parent\n  - child".len();
            view.second.update(cx, |editor, cx| {
                editor.change_selections(editor::SelectionEffects::no_scroll(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(end)..MultiBufferOffset(end)])
                })
            });
        })
        .unwrap();
    cx.executor().run_until_parked();
    keys(window, "enter y shift-tab", cx);
    assert_eq!(
        shown(window, cx),
        "- Parent ^parent\n  - child ^child\n  - y|"
    );
    keys(window, &format!("{MOD}-a"), cx);
    assert_eq!(
        shown(window, cx),
        "- [Parent ^parent\n  - child ^child\n  - y]"
    );
    assert_eq!(
        cx.update(|cx| buffer.read(cx).text()),
        "before\n\n- Parent ^parent\n  - child ^child\n  - y\n- outside ^outside\n"
    );
}
