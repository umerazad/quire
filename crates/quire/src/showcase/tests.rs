use super::*;
use gpui::TestAppContext;

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
