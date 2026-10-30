use super::super::*;
use super::links_tests::set_vim;
use super::tests::{Notebook, draw, open_notebook, read, wait_until};
use gpui::TestAppContext;
use multi_buffer::ToOffset as _;

const PAGES: [(&str, &str, &str); 2] = [
    ("w", "Alpha.md", "- \n"),
    ("t", "Target.md", "- target ^blk\n- unnamed zebra block\n"),
];

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

fn wait_for_menu(notebook: &Notebook, cx: &mut TestAppContext) {
    let editor = source(notebook, cx);
    wait_until(cx, "the completion menu", |cx| {
        cx.update(|cx| editor.read(cx).has_visible_completions_menu())
    });
    draw(notebook, cx);
}

fn open(cx: &mut TestAppContext) -> Notebook {
    let notebook = open_notebook(&PAGES, "w", cx);
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    draw(&notebook, cx);
    notebook
}

fn stored_text(notebook: &Notebook, page: &str) -> String {
    let store = quire_store::notebook::NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    store
        .checkout(page, None)
        .unwrap()
        .selected
        .unwrap()
        .root()
        .field("text")
        .as_str()
        .unwrap()
        .to_owned()
}

fn written_id(text: &str) -> String {
    let start = text.find("((").unwrap() + 2;
    let end = text[start..].find("))").unwrap() + start;
    text[start..end].to_owned()
}

#[gpui::test]
fn vim_insert_completes_pages_and_blocks_skipping_the_typed_block_and_writes_its_id(
    cx: &mut TestAppContext,
) {
    let notebook = open(cx);
    cx.simulate_keystrokes(notebook.window.into(), "A [ [ T a r");
    assert_eq!(text_and_caret(&notebook, cx), ("- [[Tar]]\n".into(), 7));
    wait_for_menu(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    assert_eq!(text_and_caret(&notebook, cx), ("- [[Target]]\n".into(), 12));

    cx.simulate_keystrokes(notebook.window.into(), "space ( ( z e b");
    assert_eq!(
        text_and_caret(&notebook, cx),
        ("- [[Target]] ((zeb))\n".into(), 18)
    );
    wait_for_menu(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    let (text, caret) = text_and_caret(&notebook, cx);
    let id = written_id(&text);
    assert!(id.starts_with("b-") && id.len() == 34, "{id}");
    assert_eq!(text, format!("- [[Target]] (({id}))\n"));
    assert_eq!(caret, text.len() - 1);

    let expected = format!("- target ^blk\n- unnamed zebra block ^{id}\n");
    wait_until(
        cx,
        "the new ID to reach the target page's store head",
        |_| stored_text(&notebook, "t") == expected,
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
}

#[gpui::test]
fn write_completion_with_vim_off_reuses_an_existing_id_and_offers_no_buffer_words(
    cx: &mut TestAppContext,
) {
    let notebook = open(cx);
    set_vim(false, cx);
    let mode = if cfg!(target_os = "macos") {
        "cmd-shift-m"
    } else {
        "ctrl-shift-m"
    };
    cx.simulate_keystrokes(notebook.window.into(), mode);
    let editor = source(&notebook, cx);
    assert!(cx.update(|cx| crate::editor_projection::enabled(editor.read(cx))));
    cx.run_until_parked();
    cx.simulate_keystrokes(notebook.window.into(), "end ( ( t a r g");
    assert_eq!(text_and_caret(&notebook, cx), ("- ((targ))\n".into(), 8));
    wait_for_menu(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    assert_eq!(text_and_caret(&notebook, cx), ("- ((blk))\n".into(), 9));
    cx.run_until_parked();
    assert_eq!(
        stored_text(&notebook, "t"),
        "- target ^blk\n- unnamed zebra block\n",
        "a block with an ID keeps its page unchanged"
    );

    cx.simulate_keystrokes(notebook.window.into(), "space [ [ n o");
    let (text, _) = text_and_caret(&notebook, cx);
    assert_eq!(text, "- ((blk)) [[no]]\n");
    cx.run_until_parked();
    assert!(
        !cx.update(|cx| editor.read(cx).has_visible_completions_menu()),
        "no page matches, and buffer words are not offered instead"
    );
}
