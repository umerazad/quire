//! The page's pointer menu (NR16.2) in Write and Source: the reference items for the caret
//! and selection, each running the editor action its keys run.

use super::super::*;
use super::tests::{Notebook, draw, open_notebook, read, wait_until};
use editor::{Editor, MultiBufferOffset, SelectionEffects};
use gpui::{Modifiers, MouseButton, TestAppContext, point, px};

const WELCOME: [(&str, &str, &str); 1] = [(
    "w",
    "Welcome.md",
    "A place for thoughts.\n\n- first item\n- second item\n",
)];

fn page_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn text(editor: &Entity<Editor>, cx: &mut TestAppContext) -> String {
    editor.read_with(cx, |editor, cx| editor.text(cx))
}

fn open(write: bool, cx: &mut TestAppContext) -> (Notebook, Entity<Editor>) {
    open_page(&WELCOME, write, cx)
}

fn open_page(
    pages: &[(&str, &str, &str)],
    write: bool,
    cx: &mut TestAppContext,
) -> (Notebook, Entity<Editor>) {
    let notebook = open_notebook(pages, pages[0].0, cx);
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    let editor = page_editor(&notebook, cx);
    if write {
        let mode = if cfg!(target_os = "macos") {
            "cmd-shift-m"
        } else {
            "ctrl-shift-m"
        };
        cx.simulate_keystrokes(notebook.window.into(), mode);
    }
    let presented = editor.clone();
    wait_until(cx, "the presentation", |cx| {
        draw(&notebook, cx);
        presented.read_with(cx, |editor, _| crate::editor_projection::enabled(editor)) == write
    });
    (notebook, editor)
}

/// Selects `len` bytes from the start of `needle` plus `skip`.
fn select(
    notebook: &Notebook,
    editor: &Entity<Editor>,
    needle: &str,
    skip: usize,
    len: usize,
    cx: &mut TestAppContext,
) {
    let start = text(editor, cx).find(needle).unwrap() + skip;
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(start)..MultiBufferOffset(start + len)])
                })
            })
        })
        .unwrap();
}

/// Draws the deployed menu and focuses it. The editor focuses its menu two animation frames
/// after deploying it, and the test platform draws no animation frames.
fn settle_menu(notebook: &Notebook, cx: &mut TestAppContext) {
    draw(notebook, cx);
    let menu = cx.update(|cx| cx.global::<crate::page_menu::LastMenu>().1.clone());
    notebook
        .window
        .update(cx, |_, window, cx| {
            if let Some(menu) = menu.upgrade() {
                window.focus(&menu.focus_handle(cx), cx);
            }
        })
        .unwrap();
    draw(notebook, cx);
}

fn open_menu(notebook: &Notebook, editor: &Entity<Editor>, cx: &mut TestAppContext) -> Vec<String> {
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "shift-f10");
    settle_menu(notebook, cx);
    assert!(editor.read_with(cx, |editor, _| editor.has_mouse_context_menu()));
    cx.update(|cx| cx.global::<crate::page_menu::LastMenu>().0.clone())
}

fn choose(notebook: &Notebook, items: &[String], label: &str, cx: &mut TestAppContext) {
    let at = items
        .iter()
        .filter(|item| *item != "-" && !item.ends_with("(disabled)"))
        .position(|item| item.trim_start_matches("[x] ").trim_start_matches("[ ] ") == label)
        .unwrap_or_else(|| panic!("{label} is not an enabled item of {items:?}"));
    for _ in 0..=at {
        cx.simulate_keystrokes(notebook.window.into(), "down");
    }
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    cx.run_until_parked();
    draw(notebook, cx);
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[gpui::test]
fn write_menu_restyles_lists_makes_tasks_and_toggles_marks_in_one_undo_step(
    cx: &mut TestAppContext,
) {
    // rt tests/e2e/editor.spec.ts "formats and restyles blocks from the page's right-click menu".
    let (notebook, editor) = open(true, cx);
    select(&notebook, &editor, "first item", 2, 0, cx);
    let items = open_menu(&notebook, &editor, cx);
    assert_eq!(
        items,
        strings(&[
            "Cut (disabled)",
            "Copy (disabled)",
            "Paste",
            "Select all",
            "-",
            "[x] Bullet list",
            "[ ] Numbered list",
            "Turn into task",
            "-",
            "Copy block reference",
            "Copy block embed",
            "Move block to page…",
            "-",
            "AI changes…",
            "Ask AI…",
            "Ask the notebook",
        ])
    );
    choose(&notebook, &items, "Numbered list", cx);
    assert_eq!(
        text(&editor, cx),
        "A place for thoughts.\n\n1. first item\n2. second item\n"
    );

    select(&notebook, &editor, "second item", 1, 0, cx);
    let items = open_menu(&notebook, &editor, cx);
    choose(&notebook, &items, "Turn into task", cx);
    // A task made outside today's daily page gets its created date.
    let tasked = text(&editor, cx);
    assert!(
        tasked.starts_with("A place for thoughts.\n\n1. first item\n2. [ ] second item"),
        "{tasked}"
    );

    select(&notebook, &editor, "first", 0, 5, cx);
    let items = open_menu(&notebook, &editor, cx);
    assert_eq!(
        items,
        strings(&[
            "Cut",
            "Copy",
            "Paste",
            "Select all",
            "-",
            "Bold",
            "Italic",
            "Strikethrough",
            "Code",
            "Link to page",
            "-",
            "[ ] Bullet list",
            "[x] Numbered list",
            "Turn into task",
            "-",
            "Copy block reference",
            "Copy block embed",
            "Move block to page…",
            "-",
            "AI changes…",
            "Ask AI…",
            "Ask the notebook",
        ])
    );
    choose(&notebook, &items, "Bold", cx);
    assert_eq!(
        text(&editor, cx),
        tasked.replacen("1. first item", "1. **first** item", 1)
    );
    let selected = editor.update(cx, |editor, cx| {
        let range = editor
            .selections
            .newest::<MultiBufferOffset>(&editor.display_snapshot(cx))
            .range();
        editor.text(cx)[range.start.0..range.end.0].to_owned()
    });
    assert_eq!(selected, "first");
    cx.dispatch_action(notebook.window.into(), editor::actions::Undo);
    assert_eq!(text(&editor, cx), tasked);

    // Escape closes the menu and gives the page its focus back.
    open_menu(&notebook, &editor, cx);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    draw(&notebook, cx);
    assert!(!editor.read_with(cx, |editor, _| editor.has_mouse_context_menu()));
    assert!(
        notebook
            .window
            .update(cx, |_, window, cx| editor
                .focus_handle(cx)
                .is_focused(window))
            .unwrap()
    );
}

#[gpui::test]
fn reference_editor_keys_switch_views_copy_a_block_reference_and_redo(cx: &mut TestAppContext) {
    let (notebook, editor) = open(false, cx);
    cx.update(|cx| crate::preferences::change(crate::preferences::Change::Vim(false), cx));
    cx.run_until_parked();
    let key = super::tests::key;
    let presented = |cx: &mut TestAppContext| {
        draw(&notebook, cx);
        editor.read_with(cx, |editor, _| crate::editor_projection::enabled(editor))
    };
    cx.simulate_keystrokes(notebook.window.into(), &key("e"));
    wait_until(cx, "Write", |cx| presented(cx));
    cx.simulate_keystrokes(notebook.window.into(), &key("e"));
    wait_until(cx, "Source", |cx| !presented(cx));

    select(&notebook, &editor, "first item", 0, 0, cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-c"));
    cx.run_until_parked();
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    let id = copied
        .strip_prefix("((")
        .and_then(|rest| rest.strip_suffix("))"))
        .unwrap_or_else(|| panic!("{copied} is not a block reference"))
        .to_owned();
    let marked = format!("A place for thoughts.\n\n- first item ^{id}\n- second item\n");
    assert_eq!(text(&editor, cx), marked);
    cx.simulate_keystrokes(notebook.window.into(), &key("z"));
    assert_eq!(text(&editor, cx), WELCOME[0].2);
    cx.simulate_keystrokes(notebook.window.into(), &key("y"));
    assert_eq!(text(&editor, cx), marked, "Mod-Y redoes");
}

/// Off macOS, Vim keeps Ctrl-E and Ctrl-Y; Ctrl-Shift-M still switches views.
#[cfg(not(target_os = "macos"))]
#[gpui::test]
fn vim_keeps_ctrl_e_and_ctrl_y_off_macos(cx: &mut TestAppContext) {
    let (notebook, editor) = open(false, cx);
    cx.update(|cx| crate::preferences::change(crate::preferences::Change::Vim(true), cx));
    cx.run_until_parked();
    cx.simulate_keystrokes(notebook.window.into(), "ctrl-e");
    draw(&notebook, cx);
    assert!(!editor.read_with(cx, |editor, _| crate::editor_projection::enabled(editor)));
    cx.simulate_keystrokes(notebook.window.into(), "ctrl-shift-m");
    wait_until(cx, "Write", |cx| {
        draw(&notebook, cx);
        editor.read_with(cx, |editor, _| crate::editor_projection::enabled(editor))
    });
}

#[gpui::test]
fn source_menu_copies_block_references_and_a_right_click_moves_the_caret(cx: &mut TestAppContext) {
    let (notebook, editor) = open(false, cx);
    select(&notebook, &editor, "second item", 1, 0, cx);
    let items = open_menu(&notebook, &editor, cx);
    // List styles are Write commands; tasks and references work in Source too.
    assert_eq!(
        items,
        strings(&[
            "Cut (disabled)",
            "Copy (disabled)",
            "Paste",
            "Select all",
            "-",
            "Turn into task",
            "-",
            "Copy block reference",
            "Copy block embed",
            "Move block to page…",
            "-",
            "AI changes…",
            "Ask AI…",
            "Ask the notebook",
        ])
    );
    choose(&notebook, &items, "Copy block reference", cx);
    let copied = cx
        .read_from_clipboard()
        .and_then(|item| item.text())
        .unwrap();
    let id = copied
        .strip_prefix("((")
        .and_then(|rest| rest.strip_suffix("))"))
        .unwrap_or_else(|| panic!("{copied} is not a block reference"))
        .to_owned();
    assert_eq!(
        text(&editor, cx),
        format!("A place for thoughts.\n\n- first item\n- second item ^{id}\n")
    );
    let items = open_menu(&notebook, &editor, cx);
    choose(&notebook, &items, "Copy block embed", cx);
    assert_eq!(
        cx.read_from_clipboard().and_then(|item| item.text()),
        Some(format!("{{{{embed: (({id}))}}}}"))
    );
    assert!(text(&editor, cx).ends_with(&format!("second item ^{id}\n")));

    // A right-click below the text, outside the selection, moves the caret to the last line.
    select(&notebook, &editor, "A place", 0, 0, cx);
    draw(&notebook, cx);
    let bounds = editor
        .read_with(cx, |editor, _| editor.last_bounds().copied())
        .expect("a drawn editor");
    let below = bounds.bottom_right() - point(px(20.), px(20.));
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    visual.simulate_mouse_down(below, MouseButton::Right, Modifiers::default());
    visual.simulate_mouse_up(below, MouseButton::Right, Modifiers::default());
    settle_menu(&notebook, cx);
    assert!(editor.read_with(cx, |editor, _| editor.has_mouse_context_menu()));
    let caret = editor.update(cx, |editor, cx| {
        editor
            .selections
            .newest::<MultiBufferOffset>(&editor.display_snapshot(cx))
            .head()
            .0
    });
    assert!(
        caret >= text(&editor, cx).find("second item").unwrap(),
        "the caret stayed at {caret}"
    );
}

#[gpui::test]
fn the_menu_formats_and_copies_the_code_block_at_the_caret(cx: &mut TestAppContext) {
    let pages = [(
        "c",
        "Code.md",
        "- Snippet\n  ```js\n  let  a=1\n  ```\n- After\n",
    )];
    for write in [true, false] {
        let (notebook, editor) = open_page(&pages, write, cx);
        select(&notebook, &editor, "a=1", 0, 0, cx);
        let items = open_menu(&notebook, &editor, cx);
        assert_eq!(
            items[4..7],
            strings(&["-", "Format code", "Copy code"]),
            "write {write}: {items:?}"
        );
        choose(&notebook, &items, "Copy code", cx);
        assert_eq!(
            cx.read_from_clipboard().and_then(|item| item.text()),
            Some("let  a=1\n".to_owned())
        );
        select(&notebook, &editor, "After", 0, 0, cx);
        let items = open_menu(&notebook, &editor, cx);
        assert!(!items.iter().any(|item| item.contains("code")), "{items:?}");
        cx.simulate_keystrokes(notebook.window.into(), "escape");
        select(&notebook, &editor, "a=1", 0, 0, cx);
        let items = open_menu(&notebook, &editor, cx);
        choose(&notebook, &items, "Format code", cx);
        wait_until(cx, "the formatted block", |cx| {
            text(&editor, cx) == "- Snippet\n  ```js\n  let a = 1;\n  ```\n- After\n"
        });
    }
}
