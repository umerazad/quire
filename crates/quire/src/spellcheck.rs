//! Spelling in page editors, in Write, Source, and embedded blocks, as the reference app's
//! editor gets it from Chromium. The system checker (`Speller`) judges each prose word once per
//! session; the page analysis's gaps keep code, URLs, references, tags, HTML, and front matter
//! out. A check runs off the UI thread after the view or text has been quiet for [`QUIET`], over
//! the visible rows
//! and a screen above and below, and leaves the word being typed alone until the caret leaves
//! it. Misspelled words get a wavy underline in the theme's error colour; a right-click on one
//! offers the checker's guesses, Learn spelling, and Ignore spelling at the top of the page menu.

mod words;

#[cfg(all(target_os = "macos", not(test)))]
mod macos;
#[cfg(test)]
mod tests;
#[cfg(all(target_os = "windows", not(test)))]
mod windows;

use crate::navigation::pointer_menu::{Entry, Item};
use crate::page_analysis::PageAnalysis;
use editor::{
    Addon, DisplayPoint, Editor, EditorEvent, HighlightKey, NavigationOverlayKey,
    display_map::DisplayRow,
};
use gpui::{
    App, AppContext as _, BorrowAppContext as _, Entity, Global, HighlightStyle, Subscription,
    Task, UnderlineStyle, WeakEntity, px,
};
use multi_buffer::{AnchorRangeExt as _, MultiBufferOffset, ToOffset as _};
use settings::Settings as _;
use std::{
    any::Any,
    collections::HashMap,
    ops::Range,
    sync::{Arc, Mutex},
    time::Duration,
};
use text::{Anchor, OffsetRangeExt as _, ToOffset as _};
use theme::ActiveTheme as _;

/// How long the text and view stay still before a check.
pub const QUIET: Duration = Duration::from_millis(250);
/// The most guesses the menu offers.
const GUESSES: usize = 6;
/// Verdicts kept before the cache starts over.
const CACHE_LIMIT: usize = 50_000;
/// Rows checked when the editor has not been laid out yet.
const UNMEASURED_ROWS: f64 = 60.;

/// A spelling dictionary. The app uses the system's; tests use a fake so they never reach it.
pub trait Speller: Send + Sync {
    /// Whether each word is misspelled, in order.
    fn misspelled(&self, words: &[&str]) -> Vec<bool>;
    /// Replacements for a misspelled word, best first.
    fn guesses(&self, word: &str) -> Vec<String>;
    /// Adds the word to the user's dictionary.
    fn learn(&self, word: &str);
}

/// The checker and what it said about each word this session.
struct Spelling {
    speller: Option<Arc<dyn Speller>>,
    misspelled: Arc<Mutex<HashMap<String, bool>>>,
}

impl Global for Spelling {}

fn system_speller() -> Option<Arc<dyn Speller>> {
    #[cfg(all(target_os = "macos", not(test)))]
    return Some(Arc::new(macos::SystemSpeller::new()));
    #[cfg(all(target_os = "windows", not(test)))]
    return windows::SystemSpeller::new().map(|speller| Arc::new(speller) as Arc<dyn Speller>);
    #[cfg(not(all(any(target_os = "macos", target_os = "windows"), not(test))))]
    None
}

pub fn init(cx: &mut App) {
    cx.set_global(Spelling {
        speller: system_speller(),
        misspelled: Arc::default(),
    });
}

#[cfg(test)]
pub(crate) fn set_speller(speller: Arc<dyn Speller>, cx: &mut App) {
    cx.set_global(Spelling {
        speller: Some(speller),
        misspelled: Arc::default(),
    });
}

/// The words an editor underlines, in page order.
#[cfg(test)]
pub(crate) fn underlined(editor: &Editor, cx: &App) -> Vec<String> {
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
}

/// The Settings hint for Check spelling, which says when this system has no checker.
pub fn preference_hint(cx: &App) -> &'static str {
    if speller(cx).is_some() {
        "Underline misspelled words in pages, using this computer's dictionary. Right-click a word for suggestions."
    } else {
        "Spelling uses the system dictionary on macOS and Windows; this system has none Quire can use."
    }
}

fn speller(cx: &App) -> Option<Arc<dyn Speller>> {
    cx.try_global::<Spelling>()
        .and_then(|spelling| spelling.speller.clone())
}

fn enabled(cx: &App) -> Option<Arc<dyn Speller>> {
    crate::preferences::QuirePreferences::get_global(cx)
        .spellcheck
        .then(|| speller(cx))
        .flatten()
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Whether `word` is misspelled, asking the checker only for words it has not judged yet.
fn judge(words: &[&str], speller: &dyn Speller, cache: &Mutex<HashMap<String, bool>>) -> Vec<bool> {
    let unknown: Vec<&str> = {
        let cache = lock(cache);
        let mut unknown: Vec<&str> = words
            .iter()
            .copied()
            .filter(|word| !cache.contains_key(*word))
            .collect();
        unknown.sort_unstable();
        unknown.dedup();
        unknown
    };
    let verdicts = (!unknown.is_empty()).then(|| speller.misspelled(&unknown));
    let mut cache = lock(cache);
    if let Some(verdicts) = verdicts {
        if cache.len() + unknown.len() > CACHE_LIMIT {
            cache.clear();
        }
        cache.extend(unknown.iter().map(|word| (*word).to_owned()).zip(verdicts));
    }
    words
        .iter()
        .map(|word| cache.get(*word).copied().unwrap_or(false))
        .collect()
}

fn key() -> HighlightKey {
    struct Misspelled;
    HighlightKey::NavigationOverlay(NavigationOverlayKey::unique::<Misspelled>())
}

fn style(cx: &App) -> HighlightStyle {
    HighlightStyle {
        underline: Some(UnderlineStyle {
            thickness: px(1.),
            color: Some(cx.theme().status().error),
            wavy: true,
        }),
        ..Default::default()
    }
}

/// An editor's spelling: its page, the misspelled words found so far, and the pending check.
struct SpellCheck {
    analysis: WeakEntity<PageAnalysis>,
    misspelled: Vec<Range<Anchor>>,
    /// The page as of the last check, to tell typing from moving the caret.
    checked: Option<text::BufferSnapshot>,
    /// The caret while a word is being typed there.
    typing: Option<Anchor>,
    task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl Addon for SpellCheck {
    fn to_any(&self) -> &dyn Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

/// Checks spelling in `editor`, a view of `analysis`'s page, from now on. Calling it again does
/// nothing.
pub fn attach(editor: &Entity<Editor>, analysis: &Entity<PageAnalysis>, cx: &mut App) {
    editor.update(cx, |editor, cx| {
        if editor.addon::<SpellCheck>().is_some() {
            return;
        }
        let subscriptions = vec![
            cx.subscribe_self(|editor, event: &EditorEvent, cx| {
                if matches!(
                    event,
                    EditorEvent::ScrollPositionChanged { .. }
                        | EditorEvent::SelectionsChanged { .. }
                        | EditorEvent::BufferEdited
                ) {
                    schedule(editor, cx);
                }
            }),
            cx.observe(analysis, |editor, _, cx| schedule(editor, cx)),
            cx.observe_global::<settings::SettingsStore>(schedule),
            cx.observe_global::<Spelling>(schedule),
            cx.observe_global::<theme::GlobalTheme>(schedule),
        ];
        editor.register_addon(SpellCheck {
            analysis: analysis.downgrade(),
            misspelled: Vec::new(),
            checked: None,
            typing: None,
            task: None,
            _subscriptions: subscriptions,
        });
        schedule(editor, cx);
    });
}

/// Checks again once the editor has been quiet for [`QUIET`], or clears the underlines at once
/// when spelling is off.
fn schedule(editor: &mut Editor, cx: &mut gpui::Context<Editor>) {
    let on = enabled(cx).is_some();
    let Some(state) = editor.addon_mut::<SpellCheck>() else {
        return;
    };
    if !on {
        state.task = None;
        state.checked = None;
        state.typing = None;
        let had = !std::mem::take(&mut state.misspelled).is_empty();
        if had {
            editor.clear_highlights(key(), cx);
        }
        return;
    }
    state.task = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(QUIET).await;
        let Ok(Some(job)) = this.update(cx, prepare) else {
            return;
        };
        let found = cx.background_spawn(job.run()).await;
        this.update(cx, |editor, cx| apply(editor, found, cx)).ok();
    }));
}

/// What a background check reads: the page, the ranges to check, and the checker.
struct Job {
    page: text::BufferSnapshot,
    ranges: Vec<Range<usize>>,
    gaps: Arc<[Range<usize>]>,
    hold: Option<usize>,
    speller: Arc<dyn Speller>,
    cache: Arc<Mutex<HashMap<String, bool>>>,
}

struct Found {
    page: text::BufferSnapshot,
    checked: Vec<Range<usize>>,
    misspelled: Vec<Range<usize>>,
}

impl Job {
    async fn run(self) -> Found {
        let texts: Vec<(usize, String)> = self
            .ranges
            .iter()
            .map(|range| {
                (
                    range.start,
                    self.page.text_for_range(range.clone()).collect(),
                )
            })
            .collect();
        let words: Vec<(Range<usize>, &str)> = texts
            .iter()
            .flat_map(|(base, text)| words::prose(text, *base, &self.gaps, self.hold))
            .collect();
        let spellings: Vec<&str> = words.iter().map(|(_, word)| *word).collect();
        let verdicts = judge(&spellings, self.speller.as_ref(), &self.cache);
        let misspelled = words
            .iter()
            .zip(verdicts)
            .filter(|(_, misspelled)| *misspelled)
            .map(|((range, _), _)| range.clone())
            .collect();
        Found {
            page: self.page,
            checked: self.ranges,
            misspelled,
        }
    }
}

/// The page's lines shown in the editor, with a screen above and below, in page offsets.
fn visible_lines(
    editor: &mut Editor,
    page: &text::BufferSnapshot,
    cx: &mut gpui::Context<Editor>,
) -> Vec<Range<usize>> {
    let rows = editor
        .visible_line_count()
        .unwrap_or(UNMEASURED_ROWS)
        .ceil();
    let top = editor.scroll_position(cx).y.max(0.).floor();
    let display = editor.display_snapshot(cx);
    let last = display.max_point().row().0;
    let row = |row: f64| DisplayRow((row.max(0.) as u32).min(last));
    let start = DisplayPoint::new(row(top - rows), 0).to_point(&display);
    let end = display.clip_point(
        DisplayPoint::new(row(top + 2. * rows), u32::MAX),
        text::Bias::Left,
    );
    let end = end.to_point(&display);
    let multi = display.buffer_snapshot();
    let id = page.remote_id();
    let mut lines: Vec<Range<usize>> = multi
        .range_to_buffer_ranges(start..end)
        .into_iter()
        .filter(|(buffer, _, _)| buffer.remote_id() == id)
        .map(|(_, range, _)| {
            let first = page.offset_to_point(range.start.0).row;
            let last = page.offset_to_point(range.end.0).row;
            page.point_to_offset(text::Point::new(first, 0))
                ..page.point_to_offset(text::Point::new(last, page.line_len(last)))
        })
        .collect();
    lines.sort_unstable_by_key(|range| range.start);
    lines.dedup_by(|next, previous| {
        let joined = next.start <= previous.end;
        if joined {
            previous.end = previous.end.max(next.end);
        }
        joined
    });
    lines
}

/// The caret's page offset when it is a single empty selection in this page.
fn caret(editor: &Editor, page: &text::BufferSnapshot, cx: &App) -> Option<usize> {
    let selection = editor.selections.newest_anchor();
    let multi = editor.buffer().read(cx).snapshot(cx);
    let head = selection.head().to_offset(&multi);
    if head != selection.tail().to_offset(&multi) {
        return None;
    }
    let (buffer, offset) = multi.point_to_buffer_offset(head)?;
    (buffer.remote_id() == page.remote_id()).then_some(offset.0)
}

fn prepare(editor: &mut Editor, cx: &mut gpui::Context<Editor>) -> Option<Job> {
    let speller = enabled(cx)?;
    let analysis = editor.addon::<SpellCheck>()?.analysis.upgrade()?;
    let analysis = analysis.read(cx);
    let gaps = analysis.current_inline(cx)?.prose_gaps.clone();
    let page = analysis.source.read(cx).snapshot().text;
    let caret = caret(editor, &page, cx);
    let state = editor.addon::<SpellCheck>()?;
    let typed = state.checked.as_ref().is_some_and(|checked| {
        caret.is_some_and(|caret| {
            page.edits_since::<usize>(checked.version())
                .any(|edit| edit.new.start <= caret && caret <= edit.new.end)
        })
    });
    let still = state
        .typing
        .is_some_and(|typing| Some(typing.to_offset(&page)) == caret);
    let hold = caret.filter(|_| typed || still);
    let ranges = visible_lines(editor, &page, cx);
    let cache = cx.global::<Spelling>().misspelled.clone();
    let state = editor.addon_mut::<SpellCheck>()?;
    state.typing = hold.map(|caret| page.anchor_before(caret));
    state.checked = Some(page.clone());
    Some(Job {
        page,
        ranges,
        gaps,
        hold,
        speller,
        cache,
    })
}

/// Replaces the underlines in the checked lines and keeps the ones found earlier elsewhere.
fn apply(editor: &mut Editor, found: Found, cx: &mut gpui::Context<Editor>) {
    let style = style(cx);
    let Some(state) = editor.addon_mut::<SpellCheck>() else {
        return;
    };
    if state.checked.is_none() {
        return;
    }
    let page = &found.page;
    let outside = |range: &Range<usize>| {
        found
            .checked
            .iter()
            .all(|checked| range.end < checked.start || checked.end < range.start)
    };
    let mut kept: Vec<Range<usize>> = state
        .misspelled
        .iter()
        .map(|range| range.to_offset(page))
        .filter(|range| range.start < range.end && outside(range))
        .chain(found.misspelled)
        .collect();
    kept.sort_unstable_by_key(|range| range.start);
    state.misspelled = kept
        .into_iter()
        .map(|range| page.anchor_after(range.start)..page.anchor_before(range.end))
        .collect();
    let anchored = state.misspelled.clone();
    let multi = editor.buffer().read(cx).snapshot(cx);
    let ranges = anchored
        .into_iter()
        .filter_map(|range| multi.anchor_range_in_buffer(range))
        .collect();
    editor.highlight_text(key(), ranges, style, cx);
}

/// A misspelled word at a point of the editor, in its page.
struct Misspelling {
    range: Range<Anchor>,
    word: String,
}

fn misspelling_at(editor: &mut Editor, point: DisplayPoint, cx: &mut App) -> Option<Misspelling> {
    let speller = enabled(cx)?;
    let analysis = editor.addon::<SpellCheck>()?.analysis.upgrade()?;
    let analysis = analysis.read(cx);
    let gaps = analysis.current_inline(cx)?.prose_gaps.clone();
    let page = analysis.source.read(cx).snapshot().text;
    let display = editor.display_snapshot(cx);
    let at = point.to_offset(&display, text::Bias::Left);
    let (buffer, at) = display.buffer_snapshot().point_to_buffer_offset(at)?;
    if buffer.remote_id() != page.remote_id() {
        return None;
    }
    let row = page.offset_to_point(at.0).row;
    let start = page.point_to_offset(text::Point::new(row, 0));
    let line: String = page
        .text_for_range(start..page.point_to_offset(text::Point::new(row, page.line_len(row))))
        .collect();
    let (range, word) = words::prose(&line, start, &gaps, None)
        .into_iter()
        .find(|(range, _)| range.start <= at.0 && at.0 <= range.end)?;
    let cache = cx.global::<Spelling>().misspelled.clone();
    judge(&[word], speller.as_ref(), &cache)[0].then(|| Misspelling {
        range: page.anchor_after(range.start)..page.anchor_before(range.end),
        word: word.to_owned(),
    })
}

/// The spelling items for a right-click at `point`: the checker's guesses, then Learn spelling
/// and Ignore spelling. Empty unless the point is on a misspelled word.
pub fn menu_entries(
    editor: &mut Editor,
    point: DisplayPoint,
    cx: &mut gpui::Context<Editor>,
) -> Vec<Entry> {
    let Some(Misspelling { range, word }) = misspelling_at(editor, point, cx) else {
        return Vec::new();
    };
    let Some(speller) = speller(cx) else {
        return Vec::new();
    };
    let editable = !editor.read_only(cx);
    let handle = cx.entity().downgrade();
    let guesses: Vec<Entry> = speller
        .guesses(&word)
        .into_iter()
        .take(GUESSES)
        .map(|guess| {
            let handle = handle.clone();
            let range = range.clone();
            let word = word.clone();
            Item::run(guess.clone(), move |window, cx| {
                handle
                    .update(cx, |editor, cx| {
                        replace(editor, &range, &word, &guess, window, cx)
                    })
                    .ok();
            })
            .disabled(!editable)
            .done()
        })
        .collect();
    let guesses = if guesses.is_empty() {
        vec![
            Item::run("No guesses found", |_, _| {})
                .disabled(true)
                .done(),
        ]
    } else {
        guesses
    };
    let learned = word.clone();
    guesses
        .into_iter()
        .chain([
            Entry::Separator,
            Item::run("Learn spelling", move |_, cx| learn(&learned, cx)).done(),
            Item::run("Ignore spelling", move |_, cx| ignore(&word, cx)).done(),
            Entry::Separator,
        ])
        .collect()
}

/// Replaces the misspelled word with `guess` as one undo step, if the word is still there.
fn replace(
    editor: &mut Editor,
    range: &Range<Anchor>,
    word: &str,
    guess: &str,
    window: &mut gpui::Window,
    cx: &mut gpui::Context<Editor>,
) {
    let multi = editor.buffer().read(cx).snapshot(cx);
    let Some(range) = multi.anchor_range_in_buffer(range.clone()) else {
        return;
    };
    let range = range.to_offset(&multi);
    if !multi.contains_str_at(range.start, word) || range.end.0 - range.start.0 != word.len() {
        return;
    }
    let range: Range<MultiBufferOffset> = range;
    editor.transact(window, cx, |editor, _, cx| {
        editor.edit([(range, guess.to_owned())], cx)
    });
}

/// Adds `word` to the user's dictionary and takes its underlines away everywhere.
pub fn learn(word: &str, cx: &mut App) {
    if let Some(speller) = speller(cx) {
        speller.learn(word);
    }
    ignore(word, cx);
}

/// Accepts `word` for the rest of this session.
pub fn ignore(word: &str, cx: &mut App) {
    if !cx.has_global::<Spelling>() {
        return;
    }
    cx.update_global::<Spelling, _>(|spelling, _| {
        lock(&spelling.misspelled).insert(word.to_owned(), false);
    });
}
