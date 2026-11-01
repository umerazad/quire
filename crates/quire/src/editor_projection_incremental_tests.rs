use super::*;
use gpui::TestAppContext;
use std::time::{Duration, Instant};

/// A task page as people keep them: open tasks with tags, dates, and page links, children with
/// web links and bold text, and done tasks, each block with a hidden ID.
pub(super) fn task_page(lines: usize) -> String {
    (0..lines)
        .map(|line| {
            let n = line / 3;
            match line % 3 {
                0 => format!("- [ ] Task {n} #next 📅 [[November 3rd, 2026]] ^t{n:04}\n"),
                1 => format!(
                    "  - note {n} with [site](https://example.com/{n}) and **bold** ^n{n:04}\n"
                ),
                _ => format!("- [x] Done {n} #home ✅ 2026-10-01 ^d{n:04}\n"),
            }
        })
        .collect()
}

fn page_window(text: &str, cx: &mut TestAppContext) -> gpui::WindowHandle<NotebookWindow> {
    cx.update(|cx| cx.remove_global::<PagesOpenInSource>());
    let window = cx.add_window(|window, cx| {
        let buffer = crate::markdown::buffer(text, cx);
        let analysis = cx.new(|cx| crate::page_analysis::PageAnalysis::new(buffer, cx));
        let view = NotebookWindow::for_notebook_page(analysis, false, window, cx);
        window.focus(&view.source.focus_handle(cx), cx);
        view
    });
    cx.simulate_window_resize(window.into(), gpui::size(gpui::px(900.), gpui::px(1000.)));
    cx.executor().run_until_parked();
    draw(window, cx);
    window
        .update(cx, |view, window, cx| {
            assert!(crate::editor_projection::enabled(view.source.read(cx)));
            assert!(view.source.focus_handle(cx).is_focused(window));
        })
        .unwrap();
    window
}

fn draw(window: gpui::WindowHandle<NotebookWindow>, cx: &mut TestAppContext) {
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
}

/// The foreground time of one keystroke, its effects, and the frame that shows it.
fn timed_key(
    key: &str,
    window: gpui::WindowHandle<NotebookWindow>,
    cx: &mut TestAppContext,
) -> (Duration, Duration) {
    let started = Instant::now();
    cx.simulate_keystrokes(window.into(), key);
    cx.executor().run_until_parked();
    let handled = started.elapsed();
    let drawn = Instant::now();
    draw(window, cx);
    (handled, drawn.elapsed())
}

fn report(what: &str, samples: &mut [(Duration, Duration)]) {
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.;
    let mut work: Vec<f64> = samples.iter().map(|(work, _)| ms(*work)).collect();
    let mut frames: Vec<f64> = samples.iter().map(|(_, frame)| ms(*frame)).collect();
    let summary = |values: &mut Vec<f64>| {
        values.sort_by(f64::total_cmp);
        let at = |q: f64| values[((values.len() - 1) as f64 * q).round() as usize];
        format!(
            "p50 {:.2} ms, p90 {:.2} ms, max {:.2} ms",
            at(0.5),
            at(0.9),
            at(1.)
        )
    };
    eprintln!(
        "{what} ({} samples): work {}; frame {}",
        samples.len(),
        summary(&mut work),
        summary(&mut frames)
    );
}

/// Times caret moves and typing on a 1,500-line task page. Run it in release for numbers:
/// `cargo test --release -p quire --features test-support,gpui_platform/runtime_shaders --locked
/// -- --ignored --nocapture large_task_page_timings`.
#[gpui::test]
#[ignore = "a timing report, not a check; run it with --ignored --nocapture"]
fn large_task_page_timings(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = page_window(&task_page(1500), cx);
    let mut down: Vec<_> = (0..120).map(|_| timed_key("j", window, cx)).collect();
    report("Vim j down a 1,500-line task page", &mut down);
    let mut up: Vec<_> = (0..120).map(|_| timed_key("k", window, cx)).collect();
    report("Vim k up the page", &mut up);
    let mut arrows: Vec<_> = (0..60)
        .flat_map(|_| [timed_key("down", window, cx), timed_key("l", window, cx)])
        .collect();
    report("Arrow down and Vim l", &mut arrows);
    let mut jumps: Vec<_> = (0..10)
        .flat_map(|_| {
            [
                timed_key("shift-g", window, cx),
                timed_key("g g", window, cx),
            ]
        })
        .collect();
    report("Vim G and gg", &mut jumps);
    cx.simulate_keystrokes(window.into(), "5 0 j shift-a");
    cx.executor().run_until_parked();
    let mut typing: Vec<_> = ["q", "u", "i", "r", "e"]
        .into_iter()
        .cycle()
        .take(60)
        .map(|key| timed_key(key, window, cx))
        .collect();
    report("Typing at a task's end", &mut typing);
    timed_key("escape", window, cx);
    let mut lines: Vec<_> = (0..10)
        .flat_map(|_| [timed_key("o", window, cx), timed_key("escape", window, cx)])
        .collect();
    report("Vim o and escape, adding a block", &mut lines);
}

/// A page with each kind of syntax Write renders: tasks with tags, dates, and page links, web
/// links and a bare URL, marks, numbered items, a collapsible parent, a rule, a table, code, and
/// text around multibyte characters.
const MIXED: &str = "# Plan 🙂 ^h1
- [ ] Task one #next 📅 [[November 3rd, 2026]] ^t1
  - child with [site](https://example.com) and **bold** ^c1
    - grandchild ^g1
- [x] Done 🙂 task #home ✅ 2026-10-01 ^d1
- bare https://example.com/bare ^b1
1. numbered *item* ^n1
2. second `code` [[Page]] ^n2

---

| a | b |
| - | - |
| 1 | 2 |

```rust
fn main() {}
```
- last [[Page|alias]] and ~~gone~~ ^z1
";

fn check(window: gpui::WindowHandle<NotebookWindow>, step: &str, cx: &mut TestAppContext) {
    window
        .update(cx, |view, window, cx| {
            view.source.update(cx, |editor, cx| {
                crate::editor_projection::check_against_whole_refold(editor, window, cx)
            })
        })
        .unwrap()
        .unwrap_or_else(|error| panic!("After {step}: {error}"));
}

fn keys(window: gpui::WindowHandle<NotebookWindow>, keys: &str, cx: &mut TestAppContext) {
    cx.simulate_keystrokes(window.into(), keys);
    cx.executor().run_until_parked();
    draw(window, cx);
}

#[gpui::test]
fn caret_moves_refold_as_a_whole_refold_would(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = page_window(MIXED, cx);
    check(window, "opening the page", cx);
    keys(window, "j z c", cx);
    check(window, "collapsing the first task", cx);
    let lines = MIXED.lines().count();
    for (step, key) in std::iter::repeat_n("j", lines)
        .chain(std::iter::repeat_n("k", lines))
        .chain([
            "w", "w", "e", "b", "$", "0", "shift-g", "g g", "down", "end",
        ])
        .enumerate()
    {
        keys(window, key, cx);
        check(window, &format!("move {step}, {key}"), cx);
    }
    keys(window, "j z o", cx);
    check(window, "expanding the first task", cx);
    // Chains of moves with no whole refold between them.
    for round in 0..4 {
        for key in ["j", "j", "w", "j", "$", "k", "j", "j"] {
            keys(window, key, cx);
        }
        check(window, &format!("round {round} of moves"), cx);
    }
    window.update(cx, |_, window, cx| window.blur(cx)).unwrap();
    cx.executor().run_until_parked();
    check(window, "losing focus", cx);
    window
        .update(cx, |view, window, cx| {
            window.focus(&view.source.focus_handle(cx), cx)
        })
        .unwrap();
    cx.executor().run_until_parked();
    check(window, "regaining focus", cx);
}

#[gpui::test]
fn edits_refold_as_a_whole_refold_would_before_and_after_the_parse(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).expect("editor initialization"));
    let window = page_window(MIXED, cx);
    let source = window
        .update(cx, |view, _, cx| view.analysis.read(cx).source.clone())
        .unwrap();
    // Edits from outside the view, checked before the parse catches up, then after.
    let edits: [(&str, &str, usize, &str); 6] = [
        ("typing in a link label", "site", 2, "x"),
        ("breaking a page link", "[[Page]]", 0, "["),
        ("typing before a hidden ID", " ^b1", 0, " more"),
        (
            "adding a line of syntax",
            "- bare",
            0,
            "- new **mark** [x](https://x.org) ^new\n",
        ),
        ("typing after a multibyte character", "🙂 ^h1", 4, "é"),
        ("deleting a whole task", "- [x] Done", 0, ""),
    ];
    for (what, needle, from, text) in edits {
        source.update(cx, |source, cx| {
            let page = source.text();
            let at = page.find(needle).unwrap() + from;
            let end = if text.is_empty() {
                page.find("- bare").unwrap()
            } else {
                at
            };
            source.edit([(at..end, text)], None, cx);
        });
        check(window, &format!("{what}, before the parse"), cx);
        cx.executor().run_until_parked();
        check(window, &format!("{what}, after the parse"), cx);
    }
    // Typing, deleting lines, and undo through the view.
    for (what, key) in [
        ("opening a line", "g g j o"),
        ("typing a task", "[ space ] space n e w space # t a g"),
        ("leaving Insert", "escape"),
        ("deleting the line", "d d"),
        ("undoing", "u"),
        ("typing at a link", "j j f [ i x escape"),
        ("joining lines", "shift-j"),
        ("pasting a line", "y y p"),
    ] {
        keys(window, key, cx);
        check(window, what, cx);
    }
}
