use super::super::*;
use super::links_tests::set_vim;
use super::tests::{Notebook, active, draw, open_notebook, read, wait_until};
use gpui::TestAppContext;

const SOURCE: &str = "- Original ^shared\n  - Child\n";
const OTHER: &str = "- intro\n- {{embed: ((shared))}}\n- ![[Source]]\n- ((missing))\n";

fn host_editor(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<editor::Editor> {
    read(notebook, cx, |view, cx| {
        view.editor.as_ref().unwrap().read(cx).source.clone()
    })
}

fn nested(editor: &Entity<editor::Editor>, cx: &mut TestAppContext) -> Vec<Entity<editor::Editor>> {
    cx.update(|cx| crate::embeds::nested_editors(editor.read(cx)))
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

fn display(notebook: &Notebook, host: &Entity<editor::Editor>, cx: &mut TestAppContext) -> String {
    notebook
        .window
        .update(cx, |_, window, cx| {
            host.update(cx, |host, cx| {
                host.snapshot(window, cx).display_snapshot.text()
            })
        })
        .unwrap()
}

fn toggle_mode(notebook: &Notebook, cx: &mut TestAppContext) {
    let mode = if cfg!(target_os = "macos") {
        "cmd-shift-m"
    } else {
        "ctrl-shift-m"
    };
    cx.simulate_keystrokes(notebook.window.into(), mode);
}

/// Opens `page` in Write with the caret on its first line, and waits for `count` nested editors.
fn open_in_write(
    pages: &[(&str, &str, &str)],
    page: &str,
    count: usize,
    cx: &mut TestAppContext,
) -> (Notebook, Entity<editor::Editor>) {
    let notebook = open_notebook(pages, page, cx);
    cx.simulate_window_resize(
        notebook.window.into(),
        gpui::size(gpui::px(1600.), gpui::px(1200.)),
    );
    wait_until(cx, "the page editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    toggle_mode(&notebook, cx);
    let editor = host_editor(&notebook, cx);
    wait_until(cx, "the embeds", |cx| {
        draw(&notebook, cx);
        nested(&editor, cx).len() == count
    });
    (notebook, editor)
}

fn edit_embedded_block(vim: bool, cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    if !vim {
        set_vim(false, cx);
    }
    let editors = nested(&host, cx);
    cx.update(|cx| {
        assert_eq!(
            editors[0].read(cx).text(cx),
            "- Original ^shared\n  - Child"
        );
        assert_eq!(editors[1].read(cx).text(cx), SOURCE);
    });
    wait_until(cx, "the embed headers", |cx| {
        draw(&notebook, cx);
        cx.update(|cx| crate::embeds::labels(host.read(cx)))
            == [
                "Open Source",
                "Open Source",
                "((missing)) · unresolved. Check the reference in Source.",
            ]
    });
    assert_eq!(
        display(&notebook, &host, cx),
        "intro\n↳\n\n↳\n\n↳\n",
        "each working embed starts on its reference's folded line; a broken one says why"
    );
    assert!(
        gpui::VisualTestContext::from_window(notebook.window.into(), cx)
            .debug_bounds("quire-embed")
            .is_some()
    );
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editors[0].focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    if vim {
        cx.simulate_keystrokes(notebook.window.into(), "I N e w space escape");
    } else {
        cx.simulate_keystrokes(notebook.window.into(), "home");
        cx.simulate_input(notebook.window.into(), "New ");
    }
    let edited = "- New Original ^shared\n  - Child\n";
    cx.update(|cx| {
        assert_eq!(
            editors[1].read(cx).text(cx),
            edited,
            "both embeds share one buffer"
        );
        assert_eq!(host.read(cx).text(cx), OTHER, "the host page is unchanged");
    });
    wait_until(cx, "the edit to reach the source page's store head", |_| {
        stored_text(&notebook, "s") == edited
    });
    assert_eq!(stored_text(&notebook, "o"), OTHER);

    let undo = if vim {
        "u"
    } else if cfg!(target_os = "macos") {
        "cmd-z"
    } else {
        "ctrl-z"
    };
    cx.simulate_keystrokes(notebook.window.into(), undo);
    cx.update(|cx| assert_eq!(editors[1].read(cx).text(cx), SOURCE));
    wait_until(cx, "the undo to reach the source page's store head", |_| {
        stored_text(&notebook, "s") == SOURCE
    });
}

#[gpui::test]
fn vim_edits_in_a_block_embed_persist_through_the_source_page(cx: &mut TestAppContext) {
    edit_embedded_block(true, cx);
}

#[gpui::test]
fn edits_in_a_block_embed_persist_through_the_source_page_with_vim_off(cx: &mut TestAppContext) {
    edit_embedded_block(false, cx);
}

#[gpui::test]
fn the_caret_reveals_an_embed_reference_its_link_opens_the_source_and_source_view_drops_embeds(
    cx: &mut TestAppContext,
) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    let bounds = |selector: &'static str, cx: &mut TestAppContext| {
        gpui::VisualTestContext::from_window(notebook.window.into(), cx)
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("no {selector} drawn"))
    };
    let folded = bounds("quire-embed", cx);
    let link = bounds("quire-embed-link", cx);
    assert_eq!(
        link.top(),
        folded.top(),
        "the link sits on the embed's first line"
    );
    cx.simulate_keystrokes(notebook.window.into(), "j");
    wait_until(cx, "the revealed reference", |cx| {
        draw(&notebook, cx);
        display(&notebook, &host, cx) == "intro\n{{embed: ((shared))}}\n\n\n↳\n\n↳\n"
    });
    assert_eq!(
        bounds("quire-embed", cx).top() - folded.top(),
        link.size.height,
        "the revealed reference pushes the embed down one line"
    );
    cx.simulate_keystrokes(notebook.window.into(), "k");
    wait_until(cx, "the folded reference", |cx| {
        draw(&notebook, cx);
        display(&notebook, &host, cx) == "intro\n↳\n\n↳\n\n↳\n"
    });
    assert_eq!(bounds("quire-embed", cx), folded);

    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    let link = visual
        .debug_bounds("quire-embed-link")
        .expect("a rendered embed link");
    visual.simulate_click(link.center(), Default::default());
    wait_until(cx, "the embedded block's page", |cx| {
        active(&notebook, cx) == "s"
    });

    let back = if cfg!(target_os = "macos") {
        "cmd-["
    } else {
        "alt-left"
    };
    cx.simulate_keystrokes(notebook.window.into(), back);
    wait_until(cx, "the embedding page", |cx| active(&notebook, cx) == "o");
    let host = host_editor(&notebook, cx);
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    if !nested(&host, cx).is_empty() {
        toggle_mode(&notebook, cx);
    }
    wait_until(cx, "Source view without embeds", |cx| {
        draw(&notebook, cx);
        nested(&host, cx).is_empty() && display(&notebook, &host, cx) == OTHER
    });
}

#[gpui::test]
fn an_embed_link_click_survives_a_refold_between_press_and_release(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    let link = visual
        .debug_bounds("quire-embed-link")
        .expect("a rendered embed link");
    visual.simulate_mouse_down(link.center(), gpui::MouseButton::Left, Default::default());
    notebook
        .window
        .update(cx, |_, window, cx| {
            host.update(cx, |host, cx| {
                crate::editor_projection::refold(host, window, cx)
            })
        })
        .unwrap();
    draw(&notebook, cx);
    let mut visual = gpui::VisualTestContext::from_window(notebook.window.into(), cx);
    visual.simulate_mouse_up(link.center(), gpui::MouseButton::Left, Default::default());
    wait_until(cx, "the embedded block's page", |cx| {
        active(&notebook, cx) == "s"
    });
}

fn headers(
    notebook: &Notebook,
    host: &Entity<editor::Editor>,
    cx: &mut TestAppContext,
) -> Vec<String> {
    draw(notebook, cx);
    cx.update(|cx| crate::embeds::labels(host.read(cx)))
}

fn wait_for_headers(
    notebook: &Notebook,
    host: &Entity<editor::Editor>,
    expected: &[&str],
    cx: &mut TestAppContext,
) {
    let mut last = Vec::new();
    wait_until(cx, "the embed headers", |cx| {
        last = headers(notebook, host, cx);
        last == expected
    });
}

const UNRESOLVED: &str = " · unresolved. Check the reference in Source.";

/// The source page's shared buffer, as the embeds edit it.
fn source_buffer(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<language::Buffer> {
    read(notebook, cx, |view, cx| {
        let pages = view.session.borrow().open_pages();
        let page = pages.iter().find(|page| page.read(cx).id() == "s").unwrap();
        page.read(cx)
            .loaded()
            .unwrap()
            .analysis
            .read(cx)
            .source
            .clone()
    })
}

#[gpui::test]
fn embeds_follow_a_renamed_source_page(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.ask_rename("s".into(), window, cx)
        })
        .unwrap();
    super::file_tests::rename_to(&notebook, "Renamed", cx);
    wait_for_headers(
        &notebook,
        &host,
        &[
            "Open Renamed",
            "Open Renamed",
            &format!("((missing)){UNRESOLVED}"),
        ],
        cx,
    );
    cx.update(|cx| {
        assert_eq!(
            host.read(cx).text(cx),
            "- intro\n- {{embed: ((shared))}}\n- ![[Renamed]]\n- ((missing))\n",
            "the rename rewrote the page embed's link in the open host page"
        );
        let editors = crate::embeds::nested_editors(host.read(cx));
        assert_eq!(editors.len(), 2);
        assert_eq!(editors[1].read(cx).text(cx), SOURCE);
    });
}

#[gpui::test]
fn deleting_the_embedded_block_then_its_page_leaves_unresolved_headers(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    let source = source_buffer(&notebook, cx);
    source.update(cx, |buffer, cx| {
        let len = buffer.len();
        buffer.edit([(0..len, "- Rewritten\n")], None, cx)
    });
    wait_for_headers(
        &notebook,
        &host,
        &[
            &format!("{{{{embed: ((shared))}}}}{UNRESOLVED}"),
            "Open Source",
            &format!("((missing)){UNRESOLVED}"),
        ],
        cx,
    );
    let editors = nested(&host, cx);
    assert_eq!(editors.len(), 1, "only the page embed keeps an editor");
    cx.update(|cx| assert_eq!(editors[0].read(cx).text(cx), "- Rewritten\n"));

    notebook
        .window
        .update(cx, |view, window, cx| {
            view.ask_delete(crate::file_actions::Target::Page("s".into()), window, cx)
        })
        .unwrap();
    super::file_tests::summary(&notebook, cx);
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_for_headers(
        &notebook,
        &host,
        &[
            &format!("{{{{embed: ((shared))}}}}{UNRESOLVED}"),
            &format!("![[Source]]{UNRESOLVED}"),
            &format!("((missing)){UNRESOLVED}"),
        ],
        cx,
    );
    assert!(nested(&host, cx).is_empty());
    assert_eq!(stored_text(&notebook, "o"), OTHER);
}

#[gpui::test]
fn two_panes_show_one_embed_and_share_its_edits(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page("o".into(), true, window, cx)
        })
        .unwrap();
    let side = |cx: &mut TestAppContext| {
        read(&notebook, cx, |view, cx| {
            view.nav
                .side
                .as_ref()
                .and_then(|side| side.editor.clone())
                .map(|editor| editor.read(cx).source.clone())
        })
    };
    wait_until(cx, "the side pane's editor", |cx| side(cx).is_some());
    let beside = side(cx).unwrap();
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&beside.focus_handle(cx), cx)
        })
        .unwrap();
    toggle_mode(&notebook, cx);
    wait_until(cx, "the side pane's embeds", |cx| {
        draw(&notebook, cx);
        nested(&beside, cx).len() == 2
    });
    let main = nested(&host, cx);
    let other = nested(&beside, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&other[0].focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "I S i d e space escape");
    let edited = "- Side Original ^shared\n  - Child\n";
    cx.update(|cx| {
        for editor in [&main[1], &other[1]] {
            assert_eq!(editor.read(cx).text(cx), edited);
        }
        assert_eq!(main[0].read(cx).text(cx), edited.trim_end());
    });
    wait_until(cx, "the edit in the source page's store head", |_| {
        stored_text(&notebook, "s") == edited
    });
    assert_eq!(stored_text(&notebook, "o"), OTHER);
}

#[gpui::test]
fn an_embed_edit_merges_with_a_concurrent_save_of_its_source(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    let store = quire_store::notebook::NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    super::tests::save(&store, "s", "Source.md", &format!("{SOURCE}- peer\n"));
    let editors = nested(&host, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editors[0].focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "I N e w space escape");
    let merged = "- New Original ^shared\n  - Child\n- peer\n";
    wait_until(cx, "the merged save", |_| {
        stored_text(&notebook, "s") == merged
    });
    assert_eq!(store.heads("s").unwrap().len(), 1);
}

#[gpui::test]
fn outline_commands_an_embed_declines_never_edit_the_host_page(cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    let editors = nested(&host, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editors[0].focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "o escape shift-o escape");
    cx.update(|cx| {
        assert_eq!(host.read(cx).text(cx), OTHER, "the host page is unchanged");
        assert_ne!(
            editors[1].read(cx).text(cx),
            SOURCE,
            "the source page took the lines"
        );
    });
}

#[gpui::test]
fn an_embed_of_its_own_ancestor_stops_at_a_recursion_notice(cx: &mut TestAppContext) {
    let pages = [(
        "c",
        "Cycle.md",
        "- Parent ^cycle\n  - {{embed: ((cycle))}}\n",
    )];
    let (notebook, host) = open_in_write(&pages, "c", 1, cx);
    let inner = nested(&host, cx).remove(0);
    wait_until(cx, "the recursion notice", |cx| {
        draw(&notebook, cx);
        cx.update(|cx| crate::embeds::shows_recursion(inner.read(cx)))
    });
    assert!(nested(&inner, cx).is_empty());
    cx.update(|cx| {
        assert_eq!(
            inner.read(cx).text(cx),
            "- Parent ^cycle\n  - {{embed: ((cycle))}}"
        )
    });
}

/// Which editor has focus, the host or a nested editor by its order, and the caret's row there.
fn caret_place(
    notebook: &Notebook,
    host: &Entity<editor::Editor>,
    cx: &mut TestAppContext,
) -> (String, u32) {
    let editors = nested(host, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            let (name, editor) = std::iter::once(("host".to_owned(), host.clone()))
                .chain(
                    editors
                        .into_iter()
                        .enumerate()
                        .map(|(at, editor)| (at.to_string(), editor)),
                )
                .find(|(_, editor)| editor.focus_handle(cx).is_focused(window))
                .expect("an editor has focus");
            let row = editor.update(cx, |editor, cx| {
                let display = editor.display_snapshot(cx);
                editor.selections.newest::<text::Point>(&display).head().row
            });
            (name, row)
        })
        .unwrap()
}

fn walk_across_embeds(vim: bool, cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    set_vim(vim, cx);
    draw(&notebook, cx);
    let (down, up) = if vim { ("j", "k") } else { ("down", "up") };
    let mut walk = |key: &str, expected: (&str, u32)| {
        cx.simulate_keystrokes(notebook.window.into(), key);
        draw(&notebook, cx);
        let place = caret_place(&notebook, &host, cx);
        assert_eq!((place.0.as_str(), place.1), expected, "after {key}");
    };
    for expected in [
        ("host", 1),
        ("0", 0),
        ("0", 1),
        ("host", 2),
        ("1", 0),
        ("1", 1),
        ("host", 3),
    ] {
        walk(down, expected);
    }
    for expected in [
        ("1", 1),
        ("1", 0),
        ("host", 2),
        ("0", 1),
        ("0", 0),
        ("host", 1),
        ("host", 0),
    ] {
        walk(up, expected);
    }
    assert_eq!(stored_text(&notebook, "o"), OTHER, "walking edits nothing");
}

#[gpui::test]
fn vim_j_and_k_walk_into_and_out_of_embeds(cx: &mut TestAppContext) {
    walk_across_embeds(true, cx);
}

#[gpui::test]
fn arrow_keys_walk_into_and_out_of_embeds_with_vim_off(cx: &mut TestAppContext) {
    walk_across_embeds(false, cx);
}

/// An empty child item interrupts its parent's paragraph, as in rt's Lezer parser, so the
/// parent keeps its block ID and an embed of it resolves.
#[gpui::test]
fn an_embed_of_an_item_above_an_empty_child_resolves(cx: &mut TestAppContext) {
    let source = "- holder ^spot\n  - \n";
    let pages = [
        ("s", "Source.md", source),
        ("o", "Other.md", "- ((spot))\n"),
    ];
    let (notebook, host) = open_in_write(&pages, "o", 1, cx);
    wait_for_headers(&notebook, &host, &["Open Source"], cx);
    let editors = nested(&host, cx);
    cx.update(|cx| assert_eq!(editors[0].read(cx).text(cx), "- holder ^spot\n  - "));
}

/// Knows every word but one, and never reaches the system's dictionary.
struct OneWrongWord(&'static str);

impl crate::spellcheck::Speller for OneWrongWord {
    fn misspelled(&self, words: &[&str]) -> Vec<bool> {
        words.iter().map(|word| *word == self.0).collect()
    }
    fn guesses(&self, _: &str) -> Vec<String> {
        Vec::new()
    }
    fn learn(&self, _: &str) {}
}

/// An embedded block is checked like the page around it, through the embedded page's analysis.
#[gpui::test]
fn embedded_blocks_underline_misspelled_words(cx: &mut TestAppContext) {
    let pages = [
        ("s", "Source.md", "- Orignal text ^shared\n  - Child\n"),
        ("o", "Other.md", "- intro\n- {{embed: ((shared))}}\n"),
    ];
    let (notebook, editor) = open_in_write(&pages, "o", 1, cx);
    cx.update(|cx| {
        crate::spellcheck::set_speller(std::sync::Arc::new(OneWrongWord("Orignal")), cx)
    });
    let embedded = nested(&editor, cx).remove(0);
    wait_until(cx, "the embed's underline", |cx| {
        draw(&notebook, cx);
        cx.executor().advance_clock(crate::spellcheck::QUIET * 2);
        cx.run_until_parked();
        cx.update(|cx| crate::spellcheck::underlined(embedded.read(cx), cx)) == ["Orignal"]
    });
}

/// Types `keys` one at a time, letting each reparse and refold land before the next, as a person
/// typing does.
fn type_slowly(notebook: &Notebook, keys: &[&str], cx: &mut TestAppContext) {
    for key in keys {
        cx.simulate_keystrokes(notebook.window.into(), key);
        draw(notebook, cx);
        cx.run_until_parked();
        draw(notebook, cx);
    }
}

fn type_words_at_the_end_of_an_embedded_block(vim: bool, cx: &mut TestAppContext) {
    let pages = [("s", "Source.md", SOURCE), ("o", "Other.md", OTHER)];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    set_vim(vim, cx);
    let editors = nested(&host, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            window.focus(&editors[0].focus_handle(cx), cx)
        })
        .unwrap();
    draw(&notebook, cx);
    type_slowly(&notebook, if vim { &["A"] } else { &["end"] }, cx);
    type_slowly(
        &notebook,
        &[
            "space",
            "a",
            "space",
            "b",
            "backspace",
            "c",
            "enter",
            "d",
            "shift-tab",
        ],
        cx,
    );
    if vim {
        type_slowly(&notebook, &["escape"], cx);
    }
    let edited = "- Original a c ^shared\n  - d\n  - Child\n";
    cx.update(|cx| assert_eq!(editors[1].read(cx).text(cx), edited));
    wait_until(
        cx,
        "the typed words in the source page's store head",
        |_| stored_text(&notebook, "s") == edited,
    );
    assert_eq!(stored_text(&notebook, "o"), OTHER);
}

#[gpui::test]
fn spaces_typed_at_the_end_of_an_embedded_block_stay(cx: &mut TestAppContext) {
    type_words_at_the_end_of_an_embedded_block(false, cx);
}

#[gpui::test]
fn vim_spaces_typed_at_the_end_of_an_embedded_block_stay(cx: &mut TestAppContext) {
    type_words_at_the_end_of_an_embedded_block(true, cx);
}

/// A one-line embed adds no row: its line takes the reference's place, and revealing the
/// reference puts it on a row of its own below.
#[gpui::test]
fn a_one_line_embed_takes_its_references_line(cx: &mut TestAppContext) {
    let pages = [
        ("s", "Source.md", "- One line ^one\n"),
        ("o", "Other.md", "- intro\n- ((one))\n- after\n"),
    ];
    let (notebook, host) = open_in_write(&pages, "o", 1, cx);
    wait_for_headers(&notebook, &host, &["Open Source"], cx);
    assert_eq!(display(&notebook, &host, cx), "intro\n↳\nafter\n");
    assert!(
        gpui::VisualTestContext::from_window(notebook.window.into(), cx)
            .debug_bounds("quire-embed-link")
            .is_some()
    );
    cx.simulate_keystrokes(notebook.window.into(), "j");
    wait_until(cx, "the revealed reference", |cx| {
        draw(&notebook, cx);
        display(&notebook, &host, cx) == "intro\n((one))\n\nafter\n"
    });
    cx.simulate_keystrokes(notebook.window.into(), "j");
    draw(&notebook, cx);
    assert_eq!(caret_place(&notebook, &host, cx), ("0".to_owned(), 0));
    cx.simulate_keystrokes(notebook.window.into(), "A space x escape");
    wait_until(cx, "the edit in the source page's store head", |_| {
        stored_text(&notebook, "s") == "- One line x ^one\n"
    });
    wait_until(cx, "the folded reference", |cx| {
        draw(&notebook, cx);
        display(&notebook, &host, cx) == "intro\n↳\nafter\n"
    });
    assert_eq!(stored_text(&notebook, "o"), "- intro\n- ((one))\n- after\n");
}

/// A caret move refolds only the syntax whose block it enters or leaves; past embeds and block
/// previews the view must still be what refolding it whole shows. Moves that enter an embed move
/// focus into its nested editor, which sends the page editor no event, so it keeps the source it
/// showed until focus returns; those steps are not compared.
#[gpui::test]
fn caret_moves_past_embeds_and_previews_refold_as_a_whole_refold_would(cx: &mut TestAppContext) {
    let other = format!("{OTHER}- see ((shared)) and [[Source]] ^p1\n- end\n");
    let pages = [
        ("s", "Source.md", SOURCE),
        ("o", "Other.md", other.as_str()),
    ];
    let (notebook, host) = open_in_write(&pages, "o", 2, cx);
    wait_until(cx, "the resolved preview", |cx| {
        draw(&notebook, cx);
        cx.update(|cx| crate::editor_projection::preview_labels(host.read(cx)))
            .iter()
            .any(|label| label.contains("Original"))
    });
    let check = |step: &str, cx: &mut TestAppContext| {
        notebook
            .window
            .update(cx, |_, window, cx| {
                host.update(cx, |editor, cx| {
                    if !editor.focus_handle(cx).is_focused(window) {
                        return Ok(false);
                    }
                    crate::editor_projection::check_against_whole_refold(editor, window, cx)
                        .map(|()| true)
                })
            })
            .unwrap()
            .unwrap_or_else(|error| panic!("After {step}: {error}"))
    };
    assert!(check("opening the page", cx));
    let lines = other.lines().count();
    let mut checked = 0;
    for (step, key) in std::iter::repeat_n("k", lines)
        .chain(std::iter::repeat_n("j", lines))
        .chain(std::iter::repeat_n("k", lines))
        .enumerate()
    {
        cx.simulate_keystrokes(notebook.window.into(), key);
        cx.run_until_parked();
        draw(&notebook, cx);
        checked += usize::from(check(&format!("move {step}, {key}"), cx));
    }
    assert!(
        checked > lines,
        "only {checked} moves left the caret in the page editor"
    );
}
