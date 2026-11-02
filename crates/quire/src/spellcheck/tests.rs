use super::*;
use crate::editor_host::NotebookWindow;
use crate::preferences::{Change, Preferences, SettingsFs, change};
use editor::SelectionEffects;
use fs::Fs as _;
use gpui::{AppContext as _, Focusable as _, TestAppContext, WindowHandle};
use std::collections::HashSet;

/// A dictionary that knows every word except the listed ones, records what it was asked, and
/// never reaches the system.
#[derive(Default)]
struct FakeSpeller {
    wrong: HashMap<&'static str, Vec<&'static str>>,
    asked: Mutex<Vec<String>>,
    learned: Mutex<HashSet<String>>,
}

impl FakeSpeller {
    fn new(wrong: &[(&'static str, &[&'static str])]) -> Arc<Self> {
        Arc::new(Self {
            wrong: wrong
                .iter()
                .map(|(word, guesses)| (*word, guesses.to_vec()))
                .collect(),
            ..Default::default()
        })
    }

    fn asked(&self, word: &str) -> usize {
        lock(&self.asked)
            .iter()
            .filter(|asked| *asked == word)
            .count()
    }
}

impl Speller for FakeSpeller {
    fn misspelled(&self, words: &[&str]) -> Vec<bool> {
        lock(&self.asked).extend(words.iter().map(|word| (*word).to_owned()));
        let learned = lock(&self.learned);
        words
            .iter()
            .map(|word| self.wrong.contains_key(word) && !learned.contains(*word))
            .collect()
    }

    fn guesses(&self, word: &str) -> Vec<String> {
        self.wrong
            .get(word)
            .map(|guesses| guesses.iter().map(|guess| (*guess).to_owned()).collect())
            .unwrap_or_default()
    }

    fn learn(&self, word: &str) {
        lock(&self.learned).insert(word.to_owned());
    }
}

fn open(
    text: &str,
    speller: Arc<FakeSpeller>,
    cx: &mut TestAppContext,
) -> WindowHandle<NotebookWindow> {
    cx.update(|cx| {
        crate::editor_host::init(cx).unwrap();
        set_speller(speller, cx);
    });
    let window = cx.add_window(|window, cx| NotebookWindow::new(text, window, cx));
    settle(window, cx);
    window
}

/// Draws, then lets the analysis and a quiet spell pass, twice: a finished analysis schedules
/// another check.
fn settle(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) {
    for _ in 0..2 {
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.run_until_parked();
        cx.executor().advance_clock(QUIET * 2);
        cx.run_until_parked();
    }
}

fn editors(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> [Entity<Editor>; 2] {
    window
        .update(cx, |view, _, _| {
            [view.write_editor().clone(), view.second_editor().clone()]
        })
        .unwrap()
}

/// The underlined words of an editor, in page order.
fn underlined(editor: &Entity<Editor>, cx: &mut TestAppContext) -> Vec<String> {
    editor.read_with(cx, |editor, cx| {
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        editor
            .text_highlights(key(), cx)
            .map(|(_, ranges)| {
                ranges
                    .iter()
                    .map(|range| snapshot.text_for_range(range.clone()).collect())
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn text(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
    window
        .update(cx, |view, _, cx| view.source_text(cx))
        .unwrap()
}

fn caret_at(window: WindowHandle<NotebookWindow>, needle: &str, cx: &mut TestAppContext) {
    let at = text(window, cx).find(needle).unwrap() + 1;
    window
        .update(cx, |view, window, cx| {
            view.write_editor().update(cx, |editor, cx| {
                editor.change_selections(SelectionEffects::no_scroll(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(at)..MultiBufferOffset(at)])
                })
            })
        })
        .unwrap();
}

/// Opens the page menu at the caret and returns its entries as text.
fn open_menu(window: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> Vec<String> {
    cx.simulate_keystrokes(window.into(), "shift-f10");
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    let menu = cx.update(|cx| cx.global::<crate::page_menu::LastMenu>().1.clone());
    window
        .update(cx, |_, window, cx| {
            if let Some(menu) = menu.upgrade() {
                window.focus(&menu.focus_handle(cx), cx);
            }
        })
        .unwrap();
    cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
        .unwrap();
    cx.update(|cx| cx.global::<crate::page_menu::LastMenu>().0.clone())
}

/// Chooses an enabled menu item by its label with the keyboard.
fn choose(
    window: WindowHandle<NotebookWindow>,
    items: &[String],
    label: &str,
    cx: &mut TestAppContext,
) {
    let at = items
        .iter()
        .filter(|item| *item != "-" && !item.ends_with("(disabled)"))
        .position(|item| item == label)
        .unwrap_or_else(|| panic!("{label} is not an enabled item of {items:?}"));
    for _ in 0..=at {
        cx.simulate_keystrokes(window.into(), "down");
    }
    cx.simulate_keystrokes(window.into(), "enter");
    cx.run_until_parked();
}

const PAGE: &str = "---\ntitle: Frontmattr\n---\n# Notes\n\nTeh fox. `codde` and [labell](https://exampel.test/pathh) and [[Pagge]] and #tagg.\n\n<!-- commentt -->\n\n```\nfencedd\n```\n\n- item wrodd ^blokid\n- see https://barre.test/urll then <b>boldd</b>\n";

fn page_speller() -> Arc<FakeSpeller> {
    FakeSpeller::new(&[
        ("Frontmattr", &[]),
        ("Teh", &["The", "Tea"]),
        ("codde", &[]),
        ("labell", &["label"]),
        ("exampel", &[]),
        ("pathh", &[]),
        ("Pagge", &[]),
        ("tagg", &[]),
        ("commentt", &[]),
        ("fencedd", &[]),
        ("wrodd", &["word"]),
        ("blokid", &[]),
        ("barre", &[]),
        ("urll", &[]),
        ("boldd", &["bold"]),
    ])
}

#[gpui::test]
fn only_prose_is_underlined_with_a_wavy_line_in_source_and_write(cx: &mut TestAppContext) {
    let window = open(PAGE, page_speller(), cx);
    let [source, second] = editors(window, cx);
    let prose = ["Teh", "labell", "wrodd", "boldd"];
    assert_eq!(underlined(&source, cx), prose);
    window
        .update(cx, |view, window, cx| view.show_written(window, cx))
        .unwrap();
    settle(window, cx);
    assert!(second.read_with(cx, |editor, _| crate::editor_projection::enabled(editor)));
    assert_eq!(
        underlined(&second, cx),
        prose,
        "Write underlines the same words"
    );
    let style = source.read_with(cx, |editor, cx| {
        editor.text_highlights(key(), cx).unwrap().0
    });
    assert!(style.underline.is_some_and(|underline| underline.wavy));
    assert_eq!(text(window, cx), PAGE, "checking never edits the page");
}

#[gpui::test]
fn each_word_reaches_the_checker_once(cx: &mut TestAppContext) {
    let speller = page_speller();
    let window = open("Teh and Teh again.\n\nTeh.\n", speller.clone(), cx);
    let [source, _] = editors(window, cx);
    assert_eq!(underlined(&source, cx), ["Teh", "Teh", "Teh"]);
    cx.simulate_keystrokes(window.into(), "G o a n d space m o r e escape");
    settle(window, cx);
    assert_eq!(speller.asked("Teh"), 1);
    assert_eq!(speller.asked("and"), 1);
    assert_eq!(speller.asked("more"), 1);
}

#[gpui::test]
fn the_word_being_typed_is_underlined_once_the_caret_leaves_it(cx: &mut TestAppContext) {
    let window = open("Fine words.\n", page_speller(), cx);
    let [source, _] = editors(window, cx);
    cx.simulate_keystrokes(window.into(), "A space T e h");
    settle(window, cx);
    assert_eq!(text(window, cx), "Fine words. Teh\n");
    assert!(underlined(&source, cx).is_empty(), "still typing Teh");
    cx.simulate_keystrokes(window.into(), "space");
    settle(window, cx);
    assert_eq!(underlined(&source, cx), ["Teh"]);
    cx.simulate_keystrokes(window.into(), "escape 0");
    settle(window, cx);
    assert_eq!(
        underlined(&source, cx),
        ["Teh"],
        "moving the caret is not typing"
    );
}

#[gpui::test]
fn only_rows_near_the_view_are_checked(cx: &mut TestAppContext) {
    let speller = FakeSpeller::new(&[("Teh", &[]), ("Lastt", &[])]);
    let text = format!(
        "Teh first.\n{}Lastt line.\n",
        "Fine words here.\n".repeat(600)
    );
    let window = open(&text, speller.clone(), cx);
    let [source, _] = editors(window, cx);
    assert_eq!(underlined(&source, cx), ["Teh"]);
    assert_eq!(
        speller.asked("Lastt"),
        0,
        "the end of the page is far below the view"
    );
    cx.simulate_keystrokes(window.into(), "G");
    settle(window, cx);
    assert_eq!(
        underlined(&source, cx),
        ["Teh", "Lastt"],
        "underlines found earlier stay"
    );
}

#[gpui::test]
fn a_guess_replaces_the_word_as_one_undo_step(cx: &mut TestAppContext) {
    let window = open(PAGE, page_speller(), cx);
    let [source, _] = editors(window, cx);
    caret_at(window, "Teh", cx);
    let items = open_menu(window, cx);
    assert_eq!(
        items[..6],
        ["The", "Tea", "-", "Learn spelling", "Ignore spelling", "-"]
    );
    assert_eq!(items[6], "Cut (disabled)", "the page menu follows");
    choose(window, &items, "The", cx);
    assert!(text(window, cx).contains("\nThe fox."));
    settle(window, cx);
    assert_eq!(underlined(&source, cx), ["labell", "wrodd", "boldd"]);
    window
        .update(cx, |_, window, cx| {
            source.update(cx, |editor, cx| {
                editor.undo(&editor::actions::Undo, window, cx)
            })
        })
        .unwrap();
    assert_eq!(text(window, cx), PAGE, "one undo restores the word");
}

#[gpui::test]
fn the_menu_offers_spelling_only_on_a_misspelled_prose_word(cx: &mut TestAppContext) {
    let window = open(PAGE, page_speller(), cx);
    caret_at(window, "codde", cx);
    assert_eq!(
        open_menu(window, cx)[0],
        "Cut (disabled)",
        "code is not prose"
    );
    cx.simulate_keystrokes(window.into(), "escape");
    caret_at(window, "fox", cx);
    assert_eq!(
        open_menu(window, cx)[0],
        "Cut (disabled)",
        "fox is spelled right"
    );
    cx.simulate_keystrokes(window.into(), "escape");
    caret_at(window, "boldd", cx);
    assert_eq!(open_menu(window, cx)[..2], ["bold", "-"]);
}

#[gpui::test]
fn learning_and_ignoring_take_the_underline_away_in_every_view(cx: &mut TestAppContext) {
    let speller = page_speller();
    let window = open(PAGE, speller.clone(), cx);
    let [source, second] = editors(window, cx);
    caret_at(window, "wrodd", cx);
    let items = open_menu(window, cx);
    choose(window, &items, "Learn spelling", cx);
    settle(window, cx);
    assert!(lock(&speller.learned).contains("wrodd"));
    assert_eq!(underlined(&source, cx), ["Teh", "labell", "boldd"]);
    assert_eq!(underlined(&second, cx), ["Teh", "labell", "boldd"]);
    caret_at(window, "labell", cx);
    let items = open_menu(window, cx);
    choose(window, &items, "Ignore spelling", cx);
    settle(window, cx);
    assert!(
        !lock(&speller.learned).contains("labell"),
        "Ignore lasts the session only"
    );
    assert_eq!(underlined(&source, cx), ["Teh", "boldd"]);
    assert_eq!(text(window, cx), PAGE);
}

#[gpui::test]
async fn the_preference_turns_spelling_off_and_on_live(cx: &mut TestAppContext) {
    let window = open(PAGE, page_speller(), cx);
    let fs = fs::FakeFs::new(cx.executor());
    fs.insert_tree(paths::config_dir(), serde_json::json!({}))
        .await;
    fs.insert_file(
        paths::settings_file(),
        crate::preferences::INITIAL_SETTINGS.into(),
    )
    .await;
    cx.update(|cx| cx.set_global(SettingsFs(fs.clone())));
    let [source, _] = editors(window, cx);
    assert!(
        cx.update(|cx| Preferences::current(cx).spellcheck),
        "on by default"
    );
    cx.update(|cx| change(Change::Spellcheck(false), cx));
    cx.run_until_parked();
    assert!(!cx.update(|cx| Preferences::current(cx).spellcheck));
    assert!(underlined(&source, cx).is_empty());
    caret_at(window, "Teh", cx);
    assert_eq!(open_menu(window, cx)[0], "Cut (disabled)");
    cx.simulate_keystrokes(window.into(), "escape");
    let saved = fs.load(paths::settings_file()).await.unwrap();
    assert!(saved.contains("\"spellcheck\": false"), "{saved}");
    cx.update(|cx| change(Change::Spellcheck(true), cx));
    settle(window, cx);
    assert_eq!(underlined(&source, cx), ["Teh", "labell", "wrodd", "boldd"]);
}

#[gpui::test]
fn every_write_row_end_is_found_without_overflow(cx: &mut TestAppContext) {
    let page = format!(
        "{PAGE}- [ ] Next action #next 📅 [[November 3rd, 2026]]\n- {{{{[[TODO]]}}}} Roam todo #[[waiting]]\n- [x] Done ✅ 2026-10-31"
    );
    let window = open(&page, page_speller(), cx);
    let [_, second] = editors(window, cx);
    window
        .update(cx, |view, window, cx| view.show_written(window, cx))
        .unwrap();
    settle(window, cx);
    second.update(cx, |editor, cx| {
        assert!(crate::editor_projection::enabled(editor));
        let display = editor.display_snapshot(cx);
        let buffer = display.buffer_snapshot();
        for row in 0..=display.max_point().row().0 {
            let end = row_end(&display, DisplayRow(row));
            assert_eq!(
                end.column,
                buffer.line_len(multi_buffer::MultiBufferRow(end.row)),
                "display row {row} ends at its line's end"
            );
        }
    });
}
