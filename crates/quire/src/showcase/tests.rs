use super::*;
use gpui::{Entity, TestAppContext};
use multi_buffer::MultiBufferOffset;

fn init_app(cx: &mut TestAppContext) {
    // The sample's inline PDF opens on its own thread and reads its pages' text there, as
    // every embedded PDF does, so its replies arrive from outside the test scheduler.
    cx.executor().allow_parking();
    cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
}

fn showcase_window(cx: &mut TestAppContext) -> Option<gpui::WindowHandle<ShowcaseWindow>> {
    cx.update(|cx| {
        cx.windows()
            .into_iter()
            .find_map(|window| window.downcast::<ShowcaseWindow>())
    })
}

fn sample_text(cx: &mut TestAppContext) -> String {
    cx.update(|cx| sample(cx).read(cx).source.read(cx).text())
}

#[test]
fn the_sample_is_the_reference_readme() {
    assert!(SAMPLE.starts_with("# Rendering at a glance\n"));
    for id in ["^readme-ai", "^readme-outline", "^readme-shared"] {
        assert!(SAMPLE.contains(id), "{id}");
    }
    for state in ["#inbox", "#next", "#waiting", "#someday", "#done"] {
        assert!(SAMPLE.contains(state), "{state}");
    }
    assert!(SAMPLE.contains("data:image/svg+xml"));
    assert!(SAMPLE.contains("data:application/pdf"));
}

#[gpui::test]
fn the_sample_keeps_edits_for_the_session_and_resets_as_one_step(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(|cx| cx.dispatch_action(&OpenShowcase));
    cx.run_until_parked();
    let window = showcase_window(cx).unwrap();
    window
        .update(cx, |view, _, cx| {
            let text = view.notebook.read(cx).source_text(cx);
            assert_eq!(text, SAMPLE);
        })
        .unwrap();
    let buffer = cx.update(|cx| sample(cx).read(cx).source.clone());
    cx.update(|cx| {
        assert!(buffer.read(cx).file().is_none());
        buffer.update(cx, |buffer, cx| buffer.edit([(0..0, "Edited. ")], None, cx));
    });
    window
        .update(cx, |_, window, _| window.remove_window())
        .unwrap();
    cx.run_until_parked();
    assert!(showcase_window(cx).is_none());
    cx.update(|cx| cx.dispatch_action(&OpenShowcase));
    cx.run_until_parked();
    let window = showcase_window(cx).unwrap();
    assert!(sample_text(cx).starts_with("Edited. # Rendering"));
    window
        .update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(ResetShowcase), cx)
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(sample_text(cx), SAMPLE);
    cx.update(|cx| buffer.update(cx, |buffer, cx| buffer.undo(cx)));
    assert!(sample_text(cx).starts_with("Edited. # Rendering"));
}

#[gpui::test]
fn the_showcase_opens_write_beside_source(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(open);
    cx.run_until_parked();
    cx.update(open);
    cx.run_until_parked();
    let count = cx.update(|cx| {
        cx.windows()
            .into_iter()
            .filter(|window| window.downcast::<ShowcaseWindow>().is_some())
            .count()
    });
    assert_eq!(count, 1);
    showcase_window(cx)
        .unwrap()
        .update(cx, |view, _, cx| {
            assert!(view.notebook.read(cx).second_is_written(cx))
        })
        .unwrap();
}

fn wait(cx: &mut TestAppContext, what: &str, done: impl FnMut(&mut TestAppContext) -> bool) {
    crate::test_wait::until(
        cx,
        std::time::Duration::from_millis(2),
        crate::test_wait::advance(std::time::Duration::from_millis(200)),
        done,
        |_| what.to_owned(),
    );
}

fn draw(window: gpui::WindowHandle<ShowcaseWindow>, cx: &mut TestAppContext) {
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

fn written(
    window: gpui::WindowHandle<ShowcaseWindow>,
    cx: &mut TestAppContext,
) -> Entity<editor::Editor> {
    window
        .update(cx, |view, _, cx| {
            view.notebook.read(cx).editors()[1].clone()
        })
        .unwrap()
}

fn line_of(text: &str, offset: usize) -> usize {
    text[..offset].matches('\n').count()
}

#[gpui::test]
fn the_sample_resolves_its_own_references_and_embeds(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(open);
    cx.run_until_parked();
    let window = showcase_window(cx).unwrap();
    let editor = written(window, cx);
    let host = cx.update(|cx| crate::references::host(editor.read(cx)).unwrap());
    let index = cx.update(|cx| host.read(cx).index().cloned().unwrap());
    wait(cx, "the sample's block in its own index", |_| {
        futures::executor::block_on(crate::query_index::ask(
            index.block_preview("readme-shared".into()),
        ))
        .ok()
        .flatten()
        .is_some_and(|preview| preview.source.path == SAMPLE_PATH)
    });
    wait(cx, "the embed of ((readme-shared))", |cx| {
        draw(window, cx);
        cx.update(|cx| !crate::embeds::nested_editors(editor.read(cx)).is_empty())
    });

    let reference = SAMPLE.find("((readme-ai))").unwrap() + 3;
    window
        .update(cx, |_, window, cx| {
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections
                        .select_ranges([MultiBufferOffset(reference)..MultiBufferOffset(reference)])
                });
            });
            window.dispatch_action(Box::new(editor::actions::GoToDefinition::default()), cx);
        })
        .unwrap();
    let target = line_of(SAMPLE, SAMPLE.find("^readme-ai").unwrap());
    wait(cx, "the caret at the referenced block", |cx| {
        let head = editor.update(cx, |editor, cx| {
            editor
                .selections
                .newest::<MultiBufferOffset>(&editor.display_snapshot(cx))
                .head()
        });
        line_of(SAMPLE, head.0) == target
    });
}

#[gpui::test]
fn a_sample_tag_opens_a_tag_page_of_the_sample_and_back(cx: &mut TestAppContext) {
    init_app(cx);
    cx.update(open);
    cx.run_until_parked();
    let window = showcase_window(cx).unwrap();
    let editor = written(window, cx);
    let host = cx.update(|cx| crate::references::host(editor.read(cx)).unwrap());
    window
        .update(cx, |_, window, cx| {
            let notebook = host.read(cx).notebook().clone();
            notebook.show_tag("#meeting", window, cx);
        })
        .unwrap();
    cx.run_until_parked();
    let tag = window
        .update(cx, |view, _, _| view.shown_tag().cloned())
        .unwrap()
        .expect("the tag page replaces the sample");
    wait(cx, "the sample's #meeting blocks", |cx| {
        cx.update(|cx| {
            let rows = tag.read(cx).rows(cx);
            !rows.is_empty()
                && rows.iter().all(|row| match row {
                    crate::navigation::context_rows::ContextRow::Page { title, .. } => {
                        title == "README"
                    }
                    _ => true,
                })
        })
    });
    window.update(cx, |view, _, cx| view.close_tag(cx)).unwrap();
    assert!(
        window
            .update(cx, |view, _, _| view.shown_tag().is_none())
            .unwrap()
    );
}
