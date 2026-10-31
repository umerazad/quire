//! `jj` template insertion driven by real keys: listing and narrowing the Templates page's
//! templates, answering and dismissing questions, picking a past name from pages that are not
//! open, exact text and caret, one undo step with Vim on and off, fresh IDs with the trigger
//! block's ID kept, and the refusals that stop before editing. Ported from the rt editor test
//! "inserts a template with jj as one undo step" and tests/templates.test.ts.

use super::super::*;
use super::links_tests::set_vim;
use super::tests::{Notebook, draw, key, open_notebook, read, wait_until};
use gpui::TestAppContext;
use multi_buffer::ToOffset as _;
use quire_core::templates::{CANCELLED, CHANGED};

/// 2026-10-30T12:00:00Z, a Friday.
const NOW: i64 = 1_793_361_600_000;

const TEMPLATES: &str = "- Meeting\n  - [[{ask: With whom?}]] {time} on {weekday} #meeting\n    - Agenda\n      - {cursor}\n    - Again [[{ask: With whom?}]] for {page}\n- My Day\n  - Plan for {today} and {date: tomorrow}\n    - {{tasks: here open}}\n- Linked\n  - First ^one\n  - See ((one))\n  - Plain ^two\n- Standup\n  - [[{pick: #standup}]] #standup\n    - {cursor}\n";

fn pages(alpha: &'static str) -> Vec<(&'static str, &'static str, &'static str)> {
    vec![
        ("w", "Alpha.md", alpha),
        ("t", "Templates.md", TEMPLATES),
        (
            "o",
            "Other.md",
            "- Not a template\n  - because this is not the Templates page\n",
        ),
        (
            "d1",
            "2026-10-20.md",
            "- [[Ops review]] #standup\n- [[Team Sync]] #standup\n",
        ),
        (
            "d2",
            "2026-10-28.md",
            "- [[Team Sync]] #standup\n- [[team sync]] 10:00 #standup\n",
        ),
    ]
}

fn source(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn text_and_caret(notebook: &Notebook, cx: &mut TestAppContext) -> (String, usize) {
    let editor = source(notebook, cx);
    cx.update(|cx| {
        let editor = editor.read(cx);
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        let head = editor
            .selections
            .newest_anchor()
            .head()
            .to_offset(&snapshot);
        (editor.text(cx), head.0)
    })
}

fn menu(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    let editor = source(notebook, cx);
    cx.update(|cx| match &*editor.read(cx).context_menu().borrow() {
        Some(editor::code_context_menus::CodeContextMenu::Completions(menu)) => menu
            .entries
            .borrow()
            .iter()
            .filter_map(|entry| entry.as_match().map(|found| found.string.clone()))
            .collect(),
        _ => Vec::new(),
    })
}

fn wait_for_menu(notebook: &Notebook, expected: &[&str], cx: &mut TestAppContext) {
    let editor = source(notebook, cx);
    wait_until(cx, "the template menu", |cx| {
        cx.update(|cx| editor.read(cx).has_visible_completions_menu())
            && menu(notebook, cx) == expected
    });
    draw(notebook, cx);
}

fn prompt_rows(notebook: &Notebook, cx: &mut TestAppContext) -> Option<Vec<String>> {
    read(notebook, cx, |view, cx| {
        let picker = view.nav.template_prompt.as_ref()?;
        let delegate = &picker.read(cx).delegate;
        Some(
            delegate
                .rows()
                .iter()
                .map(|row| delegate.row_text(row))
                .collect(),
        )
    })
}

fn wait_for_prompt(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    wait_until(cx, "a template question", |cx| {
        prompt_rows(notebook, cx).is_some()
    });
    draw(notebook, cx);
    prompt_rows(notebook, cx).unwrap()
}

fn notice(cx: &mut TestAppContext) -> Option<String> {
    cx.update(|cx| crate::outline::notice(cx).map(|notice| notice.to_string()))
}

/// Opens Alpha on the test clock in UTC, focused, with Vim on or off.
fn open(alpha: &'static str, vim: bool, cx: &mut TestAppContext) -> Notebook {
    let notebook = open_notebook(&pages(alpha), "w", cx);
    set_vim(vim, cx);
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.nav.daily.now = Rc::new(|| NOW);
            view.nav.daily.calendar = Some(quire_core::daily::DailyCalendar::utc());
            view.install_task_context(cx);
            view.focus_active(window, cx);
        })
        .unwrap();
    let client = read(&notebook, cx, |view, cx| view.index_client(cx).unwrap());
    wait_until(cx, "the index to read every page", |_| {
        futures::executor::block_on(client.unanalyzed().unwrap()).unwrap() == 0
    });
    draw(&notebook, cx);
    notebook
}

fn wait_for_text(notebook: &Notebook, expected: &str, cx: &mut TestAppContext) {
    let mut last = (String::new(), None);
    let deadline = std::time::Instant::now() + crate::test_patience::PATIENCE;
    while last.0 != expected {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for the inserted template; the page reads {:?} with notice {:?}",
            last.0,
            last.1
        );
        wait_until(cx, "a step", |_| true);
        last = (text_and_caret(notebook, cx).0, notice(cx));
    }
}

#[gpui::test]
fn vim_jj_lists_templates_narrows_asks_once_and_undoes_the_template_as_one_step(
    cx: &mut TestAppContext,
) {
    let notebook = open("- \n", true, cx);
    cx.simulate_keystrokes(notebook.window.into(), "A j j");
    wait_for_menu(&notebook, &["Linked", "Meeting", "My Day", "Standup"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "m e");
    wait_for_menu(&notebook, &["Meeting"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    assert_eq!(wait_for_prompt(&notebook, cx), ["Insert an empty answer"]);
    assert_eq!(text_and_caret(&notebook, cx).0, "- jjme\n");
    cx.simulate_input(notebook.window.into(), "Ana");
    cx.run_until_parked();
    assert_eq!(prompt_rows(&notebook, cx).unwrap(), ["Insert “Ana”"]);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let expected =
        "- [[Ana]] 12:00 on Friday #meeting\n  - Agenda\n    - \n  - Again [[Ana]] for Alpha\n";
    wait_for_text(&notebook, expected, cx);
    assert_eq!(
        text_and_caret(&notebook, cx),
        (expected.into(), expected.find("    - \n").unwrap() + 6)
    );
    assert!(
        prompt_rows(&notebook, cx).is_none(),
        "the question was asked once"
    );
    cx.simulate_keystrokes(notebook.window.into(), "X escape");
    assert_eq!(
        text_and_caret(&notebook, cx).0,
        expected.replace("    - \n", "    - X\n")
    );
    for (undone, why) in [
        (expected, "the typed X"),
        ("- jjme\n", "the template, one step"),
    ] {
        cx.simulate_keystrokes(notebook.window.into(), "u");
        assert_eq!(text_and_caret(&notebook, cx).0, undone, "u undoes {why}");
    }
}

#[gpui::test]
fn write_jj_with_vim_off_inserts_without_questions_and_undoes_only_the_template(
    cx: &mut TestAppContext,
) {
    let notebook = open("- \n", false, cx);
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-m"));
    let editor = source(&notebook, cx);
    assert!(cx.update(|cx| crate::editor_projection::enabled(editor.read(cx))));
    cx.run_until_parked();
    cx.simulate_keystrokes(notebook.window.into(), "end j j m y");
    wait_for_menu(&notebook, &["My Day"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let expected = "- Plan for [[2026-10-30]] and [[2026-10-31]]\n  - {{tasks: here open}}\n";
    wait_for_text(&notebook, expected, cx);
    assert_eq!(text_and_caret(&notebook, cx).1, expected.len() - 1);
    cx.simulate_keystrokes(notebook.window.into(), &key("z"));
    assert_eq!(text_and_caret(&notebook, cx).0, "- jjmy\n");
    cx.simulate_keystrokes(notebook.window.into(), &key("shift-z"));
    assert_eq!(text_and_caret(&notebook, cx).0, expected);
}

#[gpui::test]
fn inserted_blocks_get_fresh_ids_with_links_following(cx: &mut TestAppContext) {
    let notebook = open("- \n- after\n", true, cx);
    cx.simulate_keystrokes(notebook.window.into(), "A j j l i n k");
    wait_for_menu(&notebook, &["Linked"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the linked template", |cx| {
        text_and_caret(&notebook, cx).0.starts_with("- First ^b-")
    });
    let (text, _) = text_and_caret(&notebook, cx);
    let id = &text["- First ^".len().."- First ^".len() + 34];
    assert_eq!(
        text,
        format!("- First ^{id}\n- See (({id}))\n- Plain\n- after\n")
    );
    assert_ne!(id, "b-one");
}

#[gpui::test]
fn the_trigger_block_keeps_its_id_on_the_first_inserted_block(cx: &mut TestAppContext) {
    let notebook = open("- jj ^keep\n- see ((keep))\n", true, cx);
    cx.simulate_keystrokes(notebook.window.into(), "f j l a L");
    wait_for_menu(&notebook, &["Linked"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let expected = "- First ^keep\n- See ((keep))\n- Plain\n- see ((keep))\n";
    wait_for_text(&notebook, expected, cx);
    assert_eq!(
        text_and_caret(&notebook, cx).1,
        "- First ^keep\n- See ((keep))\n- Plain".len()
    );
}

#[gpui::test]
fn picks_list_past_names_from_unopened_pages_and_narrow_as_you_type(cx: &mut TestAppContext) {
    let notebook = open("- \n", true, cx);
    let opened: Vec<String> = read(&notebook, cx, |view, cx| {
        view.session
            .borrow()
            .open_pages()
            .iter()
            .map(|page| page.read(cx).id().to_owned())
            .collect()
    });
    assert!(!opened.contains(&"d1".to_owned()) && !opened.contains(&"d2".to_owned()));
    cx.simulate_keystrokes(notebook.window.into(), "A j j s t a n");
    wait_for_menu(&notebook, &["Standup"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    assert_eq!(
        wait_for_prompt(&notebook, cx),
        [
            "Team Sync · 3 times · 2026-10-28",
            "Ops review · 2026-10-20"
        ]
    );
    cx.simulate_input(notebook.window.into(), "ops");
    cx.run_until_parked();
    assert_eq!(
        prompt_rows(&notebook, cx).unwrap(),
        ["Ops review · 2026-10-20", "Create “ops”"]
    );
    cx.simulate_keystrokes(notebook.window.into(), "down enter");
    let expected = "- [[ops]] #standup\n  - \n";
    wait_for_text(&notebook, expected, cx);
    assert_eq!(text_and_caret(&notebook, cx).1, expected.len() - 1);
}

#[gpui::test]
fn a_dismissed_question_or_a_changed_template_leaves_the_block_unchanged(cx: &mut TestAppContext) {
    let notebook = open("- \n", true, cx);
    cx.simulate_keystrokes(notebook.window.into(), "A j j m e e");
    wait_for_menu(&notebook, &["Meeting"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_for_prompt(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    wait_until(cx, "the cancellation notice", |cx| {
        notice(cx).as_deref() == Some(CANCELLED)
    });
    assert!(prompt_rows(&notebook, cx).is_none());
    assert_eq!(text_and_caret(&notebook, cx).0, "- jjmee\n");

    cx.simulate_keystrokes(notebook.window.into(), "backspace e");
    wait_for_menu(&notebook, &["Meeting"], cx);
    let templates = read(&notebook, cx, |view, cx| {
        view.session
            .borrow()
            .open_pages()
            .into_iter()
            .find(|page| page.read(cx).id() == "t")
            .and_then(|page| Some(page.read(cx).loaded()?.analysis.read(cx).source.clone()))
            .unwrap()
    });
    cx.update(|cx| {
        templates.update(cx, |buffer, cx| {
            let at = buffer.text().find("- My Day").unwrap();
            buffer.edit([(at..at, "    - Changed\n")], None, cx)
        })
    });
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the changed-template notice", |cx| {
        notice(cx).as_deref() == Some(CHANGED)
    });
    assert_eq!(text_and_caret(&notebook, cx).0, "- jjmee\n");
}

#[gpui::test]
fn jj_offers_nothing_in_code_and_refuses_a_block_with_children(cx: &mut TestAppContext) {
    let notebook = open("```\n\n```\n- parent\n  - child\n", true, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j A j j");
    cx.run_until_parked();
    let editor = source(&notebook, cx);
    assert!(!cx.update(|cx| editor.read(cx).has_visible_completions_menu()));
    assert_eq!(
        text_and_caret(&notebook, cx).0,
        "```\njj\n```\n- parent\n  - child\n"
    );

    cx.simulate_keystrokes(notebook.window.into(), "escape j j 0 l l i j j");
    wait_for_menu(&notebook, &["Linked", "Meeting", "My Day", "Standup"], cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the children notice", |cx| {
        notice(cx).as_deref()
            == Some("This block has children. Insert a template in an empty sibling block.")
    });
    assert_eq!(
        text_and_caret(&notebook, cx).0,
        "```\njj\n```\n- jjparent\n  - child\n"
    );
}
