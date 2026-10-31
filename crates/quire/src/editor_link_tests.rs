use super::*;
use gpui::TestAppContext;
use multi_buffer::MultiBufferOffset;

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

fn click(
    window: gpui::WindowHandle<NotebookWindow>,
    selector: &'static str,
    cx: &mut TestAppContext,
) {
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    let bounds = visual.debug_bounds(selector).expect(selector);
    visual.simulate_click(bounds.center(), Default::default());
    cx.executor().run_until_parked();
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

const PAGE: &str = "- see [the site](https://example.com) and [[Other]] ^a\n- bare https://example.org/path, here\n- <https://example.net/a> `https://code.example`\n";

#[gpui::test]
fn external_links_and_bare_urls_render_apart_from_notebook_links(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let window = projected(PAGE, cx);
    assert_eq!(
        display(window, cx),
        "see the site↗ and Other\nbare https://example.org/path, here\nhttps://example.net/a↗ https://code.example\n"
    );
    window
        .update(cx, |view, _, cx| {
            let ([internal, external], expected) =
                crate::editor_projection::link_highlights(view.second.read(cx), cx);
            let (_, internal) = internal.expect("notebook link highlight");
            assert_eq!(
                internal
                    .iter()
                    .map(|range| &PAGE[range.clone()])
                    .collect::<Vec<_>>(),
                ["Other"]
            );
            let (style, external) = external.expect("external link highlight");
            assert_eq!(style, expected);
            assert!(style.underline.is_some_and(|line| line.color.is_some()));
            assert_eq!(
                external
                    .iter()
                    .map(|range| &PAGE[range.clone()])
                    .collect::<Vec<_>>(),
                [
                    "the site",
                    "https://example.org/path",
                    "https://example.net/a"
                ]
            );
        })
        .unwrap();
    window
        .update(cx, |view, window, cx| {
            let editor = view.second.clone();
            view.change_presentation(&editor, window, cx);
        })
        .unwrap();
    assert_eq!(display(window, cx), PAGE, "Source shows every byte");
    window
        .update(cx, |view, _, cx| {
            assert_eq!(view.source.read(cx).text(cx), PAGE)
        })
        .unwrap();
}

#[gpui::test]
fn bare_urls_and_their_arrows_open_the_address_without_editing(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    for (text, selector, url) in [
        (
            "- read https://example.org/a_(b) now ^id\n",
            "quire-web-link",
            "https://example.org/a_(b)",
        ),
        (
            "- read https://example.org/arrow. ^id\n",
            "quire-link-arrow",
            "https://example.org/arrow",
        ),
        (
            "- [Site](https://example.com/s) ^id\n",
            "quire-link-arrow",
            "https://example.com/s",
        ),
        (
            "- <https://example.net/auto> ^id\n",
            "quire-web-link",
            "https://example.net/auto",
        ),
    ] {
        let window = projected(text, cx);
        click(window, selector, cx);
        assert_eq!(cx.opened_url().as_deref(), Some(url), "{text}");
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), text)
            })
            .unwrap();
    }
}

#[gpui::test]
fn the_caret_reveals_link_source_in_its_block_only(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let window = projected(PAGE, cx);
    let bare = PAGE.find("https://example.org").unwrap() + 3;
    caret_at(window, bare, cx);
    let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
    visual.update(|window, cx| window.draw(cx).clear(cx));
    assert_eq!(
        display(window, cx),
        "see the site↗ and Other\nbare https://example.org/path, here\nhttps://example.net/a↗ https://code.example\n"
    );
    let auto = PAGE.find("<https").unwrap() + 3;
    caret_at(window, auto, cx);
    assert_eq!(
        display(window, cx),
        "see the site↗ and Other\nbare https://example.org/path, here\n<https://example.net/a> `https://code.example`\n"
    );
    caret_at(window, 4, cx);
    assert_eq!(
        display(window, cx),
        "see [the site](https://example.com) and [[Other]]\nbare https://example.org/path, here\nhttps://example.net/a↗ https://code.example\n"
    );
}

#[gpui::test]
fn moving_into_a_bare_url_block_reveals_it_with_and_without_vim(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let text = "- [Site](https://example.com/s) ^a\n- next https://example.org/x ^b\n";
    for (vim, down) in [(true, "j"), (false, "down")] {
        super::search_tests::set_vim_mode(cx, vim);
        let window = projected(text, cx);
        caret_at(window, 4, cx);
        assert_eq!(
            display(window, cx),
            "[Site](https://example.com/s)\nnext https://example.org/x\n"
        );
        cx.simulate_keystrokes(window.into(), down);
        cx.executor().run_until_parked();
        assert_eq!(
            display(window, cx),
            "Site↗\nnext https://example.org/x\n",
            "vim {vim}"
        );
        let mut visual = gpui::VisualTestContext::from_window(window.into(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            visual.debug_bounds("quire-link-arrow").is_some(),
            "the other block keeps its arrow"
        );
        window
            .update(cx, |view, _, cx| {
                assert_eq!(view.source.read(cx).text(cx), text)
            })
            .unwrap();
    }
}
