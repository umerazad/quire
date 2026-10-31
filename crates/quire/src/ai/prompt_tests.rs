//! The reference `tests/ai-prompt.test.tsx` cases through native keys: the prompt names its
//! target, runs a typed instruction or a verb, remembers typed prompts, recalls them with Up and
//! Down, and closes on Escape without a request.

use super::*;
use crate::ai::proposal::tests::{
    editor, keys, notice, page, proposal, setup, text, typed, wait_ready,
};
use gpui::{TestAppContext, WindowHandle};

const ASK: &str = if cfg!(target_os = "macos") {
    "cmd-i"
} else {
    "ctrl-i"
};

fn prompt(
    window: WindowHandle<crate::editor_host::NotebookWindow>,
    cx: &mut TestAppContext,
) -> Option<Entity<Picker<AiPrompt>>> {
    editor(window, cx).read_with(cx, |editor, _| current(editor))
}

fn read<T>(
    window: WindowHandle<crate::editor_host::NotebookWindow>,
    cx: &mut TestAppContext,
    f: impl FnOnce(&AiPrompt) -> T,
) -> T {
    prompt(window, cx)
        .expect("the AI prompt is open")
        .read_with(cx, |picker, _| f(&picker.delegate))
}

fn query(
    window: WindowHandle<crate::editor_host::NotebookWindow>,
    cx: &mut TestAppContext,
) -> String {
    prompt(window, cx)
        .expect("the AI prompt is open")
        .read_with(cx, |picker, cx| picker.query(cx))
}

#[gpui::test]
async fn runs_a_typed_instruction_on_the_block_and_remembers_it(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- apples are good ^b-1\n- pears\n", cx);
    ai.answer("- apples are fine", &[], cx);
    keys(window, ASK, cx);
    assert_eq!(
        read(window, cx, |prompt| prompt.label().to_owned()),
        "Block · 1 line"
    );
    assert_eq!(
        read(window, cx, AiPrompt::rows),
        [
            "Tighten wording",
            "Rewrite as prose",
            "Rewrite as an outline",
            "Continue writing",
            "Check facts and logic"
        ]
    );
    keys(window, &typed("make it formal"), cx);
    assert_eq!(read(window, cx, AiPrompt::rows)[0], "Run “make it formal”");
    keys(window, "enter", cx);
    assert!(prompt(window, cx).is_none());
    wait_ready(window, cx);
    let sent = &ai.sent()[0];
    assert!(
        sent.contains("make it formal") && sent.contains("<text>\n- apples are good ^b-1\n</text>"),
        "{sent}"
    );
    assert_eq!(cx.update(|cx| settings::history(cx)), ["make it formal"]);
}

#[gpui::test]
async fn a_verb_row_runs_that_verb_without_remembering_it(cx: &mut TestAppContext) {
    let ai = setup(false, cx).await;
    let window = page("- apples are good and they are tasty\n", cx);
    ai.answer("- apples are good", &[], cx);
    keys(window, ASK, cx);
    keys(window, "enter", cx);
    wait_ready(window, cx);
    assert!(ai.sent()[0].starts_with(Verb::Tighten.instruction()));
    assert!(cx.update(|cx| settings::history(cx)).is_empty());
}

#[gpui::test]
async fn up_recalls_earlier_prompts_and_down_returns_to_the_draft(cx: &mut TestAppContext) {
    let _ai = setup(false, cx).await;
    cx.update(|cx| {
        settings::remember("first", cx);
        settings::remember("second", cx);
    });
    let window = page("- apples\n", cx);
    keys(window, ASK, cx);
    assert_eq!(
        read(window, cx, AiPrompt::rows)[5..],
        ["second".to_owned(), "first".to_owned()]
    );
    keys(window, "d r", cx);
    keys(window, "up", cx);
    assert_eq!(query(window, cx), "second");
    keys(window, "up", cx);
    assert_eq!(query(window, cx), "first");
    keys(window, "up", cx);
    assert_eq!(query(window, cx), "first", "Up stops at the oldest prompt");
    keys(window, "down", cx);
    assert_eq!(query(window, cx), "second");
    keys(window, "down", cx);
    assert_eq!(query(window, cx), "dr");
}

#[gpui::test]
async fn escape_closes_the_prompt_and_returns_to_the_page_without_a_request(
    cx: &mut TestAppContext,
) {
    let ai = setup(false, cx).await;
    let window = page("- apples\n", cx);
    keys(window, ASK, cx);
    keys(window, &typed("shorter"), cx);
    keys(window, "escape", cx);
    assert!(prompt(window, cx).is_none());
    assert!(proposal(window, cx).is_none());
    assert!(ai.sent().is_empty());
    assert_eq!(text(window, cx), "- apples\n");
    let host = editor(window, cx);
    let focused = window
        .update(cx, |_, window, cx| host.focus_handle(cx).is_focused(window))
        .unwrap();
    assert!(focused, "Escape returns focus to the page");
}

#[gpui::test]
async fn vim_visual_lines_open_the_prompt_for_those_lines(cx: &mut TestAppContext) {
    let ai = setup(true, cx).await;
    let window = page("- one\n- two\n- three\n", cx);
    ai.answer("- two and three", &[], cx);
    keys(window, "j shift-v j \\ a", cx);
    assert_eq!(
        read(window, cx, |prompt| prompt.label().to_owned()),
        "Selection · 2 lines"
    );
    keys(window, &typed("merge"), cx);
    keys(window, "enter", cx);
    wait_ready(window, cx);
    let sent = &ai.sent()[0];
    assert!(sent.contains("<text>\n- two\n- three\n</text>"), "{sent}");
    keys(window, "enter", cx);
    assert_eq!(text(window, cx), "- one\n- two and three\n");
}

#[gpui::test]
async fn a_blank_line_offers_continue_only_and_commands_refuse_while_off(cx: &mut TestAppContext) {
    let _ai = setup(true, cx).await;
    let window = page("- apples\n\n", cx);
    keys(window, "j \\ a", cx);
    assert_eq!(
        read(window, cx, |prompt| prompt.label().to_owned()),
        "Blank line · writes here, using the text above"
    );
    assert_eq!(read(window, cx, AiPrompt::rows), ["Continue writing"]);
    keys(window, "escape", cx);
    cx.update(|cx| settings::set_enabled(false, cx)).unwrap();
    cx.run_until_parked();
    keys(window, "\\ a", cx);
    assert!(prompt(window, cx).is_none());
    assert_eq!(notice(cx), quire_core::ai::backend::AI_OFF);
}

#[gpui::test]
async fn the_page_menu_offers_ask_ai_and_the_notebook(cx: &mut TestAppContext) {
    let _ai = setup(false, cx).await;
    let window = page("- apples\n", cx);
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    keys(window, "shift-f10", cx);
    let items = cx.update(|cx| cx.global::<crate::page_menu::LastMenu>().0.clone());
    assert_eq!(
        items[items.len().saturating_sub(2)..],
        ["Ask AI…", "Ask the notebook"],
        "a page outside a notebook offers no Tidy: {items:?}"
    );
}
