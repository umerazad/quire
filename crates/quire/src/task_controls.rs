//! Task lines in Write. The checkbox is drawn as `rt` draws it (`controls.css`): a rounded box a
//! little smaller than the text, jade with a check when done, and a jade border under the
//! pointer. A done task's text is muted, as `rt`'s `task-completed-line`. After each open task's
//! first line sit its due date controls: a calendar that opens a menu (today, tomorrow, next
//! week, a typed date, or no due date) and two buttons that make the task due tomorrow or next
//! Monday. They stay faint until the pointer or the caret is on the line. Each choice is one
//! undoable source edit through `crate::tasks`, so Source and every view see it.
//!
//! The lines are found when the page is parsed, never on a caret move; the controls are text
//! regions after the line's last fragment, so they follow wrapping and never shift the text.
//! Write also breaks a `- [ ]` that it shows as text, such as in a quote or code, out of the
//! ligature some fonts draw for it, so it reads as written and not as a tiny box.

use crate::markdown_blocks::Block;
use crate::navigation::pointer_menu::{Entry, Item};
use crate::tasks::{Due, TaskContext};
use editor::{
    Addon, Editor, FoldPlaceholder, HighlightKey, NavigationOverlayKey, TextRegion,
    TextRegionLayout,
};
use gpui::{
    AnyElement, App, Context, Entity, Focusable as _, Hsla, InteractiveElement as _,
    IntoElement as _, ParentElement as _, Pixels, SharedString, StatefulInteractiveElement as _,
    Styled as _, Subscription, WeakEntity, Window, div, linear_color_stop, linear_gradient, px,
    rgb,
};
use multi_buffer::{Anchor, MultiBufferOffset, ToOffset as _};
use settings::Settings as _;
use std::{
    any::{Any, TypeId},
    ops::Range,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use theme::ActiveTheme as _;
use ui::{Color, Icon, IconName, IconSize, Tooltip};

/// `rt`'s checkbox and done-task colours, light (`controls.css`) or dark (`dark-theme.css`).
#[derive(Clone, Copy)]
struct Palette {
    border: u32,
    fill: u32,
    jade_from: u32,
    jade: u32,
    check: u32,
    ring: u32,
    done: u32,
}

impl Palette {
    fn of(cx: &App) -> Self {
        if cx.theme().appearance.is_light() {
            Self {
                border: 0x8296a3,
                fill: 0xffffff,
                jade_from: 0x4b8a76,
                jade: 0x367462,
                check: 0xffffff,
                ring: 0xeaf4ef,
                done: 0x63766d,
            }
        } else {
            Self {
                border: 0x8195a2,
                fill: 0x1c1e21,
                jade_from: 0x70b09a,
                jade: 0x7fbeaa,
                check: 0x1c1e21,
                ring: 0x26302c,
                done: 0x99ada4,
            }
        }
    }
}

fn font_size(cx: &App) -> Pixels {
    theme_settings::ThemeSettings::get_global(cx).buffer_font_size(cx)
}

/// The checkbox's side: `rt`'s 13 px box beside 15 px text, scaled with the text.
fn box_size(cx: &App) -> Pixels {
    (font_size(cx) * 0.87).round().max(px(11.))
}

/// A task's checkbox, ticked when `checked`. Its border turns jade while the pointer is over
/// `group`.
pub(crate) fn checkbox(checked: bool, group: SharedString, cx: &App) -> gpui::Div {
    let palette = Palette::of(cx);
    let size = box_size(cx);
    let mark = div()
        .debug_selector(move || {
            if checked {
                "quire-task-box-checked".into()
            } else {
                "quire-task-box".into()
            }
        })
        .flex_none()
        .size(size)
        .rounded(px(4.))
        .border(px(1.5))
        .flex()
        .items_center()
        .justify_center();
    let (jade, ring) = (palette.jade, palette.ring);
    let mark = mark.group_hover(group, move |style| {
        style.border_color(rgb(jade)).shadow(vec![gpui::BoxShadow {
            color: rgb(ring).into(),
            offset: gpui::point(px(0.), px(0.)),
            blur_radius: px(0.),
            spread_radius: px(3.),
            inset: false,
        }])
    });
    if checked {
        mark.border_color(rgb(palette.jade))
            .bg(linear_gradient(
                150.,
                linear_color_stop(rgb(palette.jade_from), 0.),
                linear_color_stop(rgb(palette.jade), 1.),
            ))
            .child(
                Icon::new(IconName::Check)
                    .size(IconSize::Custom(ui::rems_from_px(f32::from(size) * 0.82)))
                    .color(Color::Custom(rgb(palette.check).into())),
            )
    } else {
        mark.border_color(rgb(palette.border)).bg(rgb(palette.fill))
    }
}

/// A task's first line in the view.
#[derive(Clone)]
struct TaskLine {
    /// The task marker through the end of the first line.
    range: Range<Anchor>,
    /// The text after the marker, which a done task mutes.
    text: Range<Anchor>,
    done: bool,
    /// Whether the line holds a due stamp that "Clear due date" removes.
    dated: bool,
}

const NOT_HOVERED: usize = usize::MAX;

/// Which task line the pointer is over, by ordinal: its text, or its controls. Each is tracked
/// apart, so moving from one to the other never hides the controls.
struct Hover {
    line: AtomicUsize,
    controls: AtomicUsize,
}

impl Default for Hover {
    fn default() -> Self {
        Self {
            line: AtomicUsize::new(NOT_HOVERED),
            controls: AtomicUsize::new(NOT_HOVERED),
        }
    }
}

impl Hover {
    fn shows(&self, ordinal: usize) -> bool {
        [&self.line, &self.controls]
            .iter()
            .any(|over| over.load(Ordering::Relaxed) == ordinal)
    }

    fn set(
        &self,
        controls: bool,
        ordinal: usize,
        hovered: bool,
        editor: &WeakEntity<Editor>,
        cx: &mut App,
    ) {
        let over = if controls { &self.controls } else { &self.line };
        let changed = if hovered {
            over.swap(ordinal, Ordering::Relaxed) != ordinal
        } else {
            over.compare_exchange(ordinal, NOT_HOVERED, Ordering::Relaxed, Ordering::Relaxed)
                .is_ok()
        };
        if changed {
            editor.update(cx, |_, cx| cx.notify()).ok();
        }
    }
}

/// The open "Pick a date" field and the task line it dates.
struct Picking {
    line: Anchor,
    field: Entity<Editor>,
}

struct TaskLines {
    lines: Vec<TaskLine>,
    hover: Arc<Hover>,
    picking: Option<Picking>,
    /// The spacers this module placed in `- [ ]` text Write shows as written.
    guards: crate::editor_projection::KeptFolds,
    _theme: Subscription,
}

impl TaskLines {
    fn line_at(&self, offset: MultiBufferOffset, cx: &App, editor: &Editor) -> Option<&TaskLine> {
        let snapshot = editor.buffer().read(cx).snapshot(cx);
        self.lines.iter().find(|line| {
            line.range.start.to_offset(&snapshot) <= offset
                && offset <= line.range.end.to_offset(&snapshot)
        })
    }
}

struct DoneText;
struct DashGuard;

fn done_key() -> HighlightKey {
    HighlightKey::NavigationOverlay(NavigationOverlayKey::unique::<DoneText>())
}

/// How faint the controls of a line away from the pointer and caret are.
const IDLE_OPACITY: f32 = 0.0;

impl Addon for TaskLines {
    fn text_regions(
        &self,
        editor: &Entity<Editor>,
        visible: Range<MultiBufferOffset>,
        window: &Window,
        cx: &App,
    ) -> Vec<TextRegion> {
        let view = editor.read(cx);
        let snapshot = view.buffer().read(cx).snapshot(cx);
        let caret = view
            .focus_handle(cx)
            .is_focused(window)
            .then(|| view.selections.newest_anchor().head().to_offset(&snapshot));
        let picking = self
            .picking
            .as_ref()
            .map(|picking| (picking.line.to_offset(&snapshot), picking.field.clone()));
        // Task lines come in page order, so only those from the first that reaches the view are
        // read.
        let first = self
            .lines
            .partition_point(|line| line.range.end.to_offset(&snapshot) < visible.start);
        self.lines
            .iter()
            .enumerate()
            .skip(first)
            .take_while(|(_, line)| line.range.start.to_offset(&snapshot) <= visible.end)
            .filter_map(|(ordinal, line)| {
                let range =
                    line.range.start.to_offset(&snapshot)..line.range.end.to_offset(&snapshot);
                let field = picking
                    .as_ref()
                    .filter(|(at, _)| *at == range.start)
                    .map(|(_, field)| field.clone());
                if line.done && field.is_none() {
                    return None;
                }
                let current =
                    caret.is_some_and(|caret| range.contains(&caret) || caret == range.end);
                let row = Row {
                    editor: editor.downgrade(),
                    ordinal,
                    line: line.clone(),
                    current,
                    field,
                    hover: self.hover.clone(),
                };
                Some(TextRegion {
                    range: line.range.clone(),
                    render: Arc::new(move |layout, cx| controls(&row, layout, cx)),
                })
            })
            .collect()
    }

    fn to_any(&self) -> &dyn Any {
        self
    }

    fn to_any_mut(&mut self) -> Option<&mut dyn Any> {
        Some(self)
    }
}

const CALENDAR: &str = "icons/quire/tasks/calendar.svg";
const TOMORROW: &str = "icons/quire/tasks/chevron-right.svg";
const NEXT_WEEK: &str = "icons/quire/tasks/chevrons-right.svg";

/// `Mon Nov 2`, with the year when it is not this year's.
fn day_label(iso: &str, today: &str) -> String {
    let weekday = jiff::civil::Date::strptime("%Y-%m-%d", iso)
        .map(|date| date.strftime("%a").to_string())
        .unwrap_or_default();
    format!("{weekday} {}", quire_core::tasks::short_date(iso, today))
        .trim()
        .to_owned()
}

fn today(cx: &App) -> Option<String> {
    cx.try_global::<TaskContext>()?.today()
}

/// The label of a quick due date choice, with the date it sets.
fn due_label(name: &str, due: &Due, cx: &App) -> String {
    match today(cx).and_then(|today| Some(day_label(&due.date(&today)?, &today))) {
        Some(day) => format!("{name} ({day})"),
        None => name.to_owned(),
    }
}

fn control(
    id: impl Into<gpui::ElementId>,
    icon: &'static str,
    label: String,
    cx: &App,
) -> gpui::Stateful<gpui::Div> {
    let wash = cx.theme().colors().ghost_element_hover;
    let size = (font_size(cx) * 1.3).round();
    div()
        .id(id)
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .size(size)
        .rounded(px(4.))
        .cursor_pointer()
        .hover(move |style| style.bg(wash))
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .tooltip(Tooltip::text(label))
        .on_mouse_down(gpui::MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .child(
            Icon::from_path(icon)
                .size(IconSize::Custom(ui::rems_from_px(f32::from(size) * 0.62)))
                .color(Color::Muted),
        )
}

/// Applies `due` to the task whose line starts at `line`.
fn date_line(
    editor: &WeakEntity<Editor>,
    line: Anchor,
    due: &Due,
    window: &mut Window,
    cx: &mut App,
) {
    editor
        .update(cx, |editor, cx| {
            let snapshot = editor.buffer().read(cx).snapshot(cx);
            let at = line.to_offset(&snapshot).0;
            crate::tasks::set_due(editor, due, Some(at), window, cx);
            window.focus(&editor.focus_handle(cx), cx);
        })
        .ok();
}

/// One task line's controls as a text region draws them.
struct Row {
    editor: WeakEntity<Editor>,
    ordinal: usize,
    line: TaskLine,
    /// Whether the focused editor's caret is on the line.
    current: bool,
    field: Option<Entity<Editor>>,
    hover: Arc<Hover>,
}

fn controls(row: &Row, layout: &TextRegionLayout, cx: &mut App) -> AnyElement {
    let Row {
        editor,
        ordinal,
        line,
        current,
        field,
        hover,
    } = row;
    let (ordinal, current, field) = (*ordinal, *current, field.clone());
    let Some(last) = layout.fragments.last() else {
        return div().into_any_element();
    };
    let local = last.origin - layout.bounds.origin;
    let shown = current || field.is_some() || hover.shows(ordinal);
    let trackers = layout.fragments.iter().enumerate().map(|(index, bounds)| {
        let local = bounds.origin - layout.bounds.origin;
        let hover = hover.clone();
        let editor = editor.clone();
        div()
            .id(("quire-task-line", index))
            .absolute()
            .left(local.x)
            .top(local.y)
            .w(bounds.size.width)
            .h(bounds.size.height)
            .on_hover(move |hovered, _, cx| hover.set(false, ordinal, *hovered, &editor, cx))
    });
    let start = line.range.start;
    let strip = div()
        .id(("quire-task-controls", ordinal))
        .debug_selector(|| "quire-task-controls".into())
        .absolute()
        .left(local.x + last.size.width)
        .pl(font_size(cx) * 0.75)
        .top(local.y)
        .h(last.size.height)
        .flex()
        .items_center()
        .gap(px(1.))
        .opacity(if shown { 1. } else { IDLE_OPACITY })
        .on_hover({
            let hover = hover.clone();
            let editor = editor.clone();
            move |hovered, _, cx| hover.set(true, ordinal, *hovered, &editor, cx)
        });
    let strip = match field {
        Some(field) => strip.child(date_field(editor, field, cx)),
        None => {
            let menu_editor = editor.clone();
            let dated = line.dated;
            let tomorrow = editor.clone();
            let next_week = editor.clone();
            strip
                .child(
                    control(
                        ("quire-task-due-menu", ordinal),
                        CALENDAR,
                        "Set due date".into(),
                        cx,
                    )
                    .debug_selector(|| "quire-task-due-menu".into())
                    .on_click(move |event, window, cx| {
                        cx.stop_propagation();
                        open_menu(&menu_editor, start, dated, event.position(), window, cx);
                    }),
                )
                .child(
                    control(
                        ("quire-task-due-tomorrow", ordinal),
                        TOMORROW,
                        due_label("Due tomorrow", &Due::Tomorrow, cx),
                        cx,
                    )
                    .debug_selector(|| "quire-task-due-tomorrow".into())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        date_line(&tomorrow, start, &Due::Tomorrow, window, cx);
                    }),
                )
                .child(
                    control(
                        ("quire-task-due-next-week", ordinal),
                        NEXT_WEEK,
                        due_label("Due next week", &Due::NextWeek, cx),
                        cx,
                    )
                    .debug_selector(|| "quire-task-due-next-week".into())
                    .on_click(move |_, window, cx| {
                        cx.stop_propagation();
                        date_line(&next_week, start, &Due::NextWeek, window, cx);
                    }),
                )
        }
    };
    div()
        .size_full()
        .children(trackers)
        .child(strip)
        .into_any_element()
}

fn date_field(editor: &WeakEntity<Editor>, field: Entity<Editor>, cx: &App) -> AnyElement {
    let colors = cx.theme().colors();
    let confirm = editor.clone();
    let cancel = editor.clone();
    div()
        .id("quire-task-due-field")
        .key_context(crate::task_lists::DATE_CONTEXT)
        .debug_selector(|| "quire-task-due-field".into())
        .w(px(240.))
        .px_1()
        .rounded(px(4.))
        .border_1()
        .border_color(colors.border_focused)
        .bg(colors.editor_background)
        .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_action(move |_: &menu::Confirm, window, cx| finish(&confirm, true, window, cx))
        .on_action(move |_: &menu::Cancel, window, cx| finish(&cancel, false, window, cx))
        .child(field)
        .into_any_element()
}

/// The due date menu of the task whose line starts at `line`, at the pointer.
fn open_menu(
    editor: &WeakEntity<Editor>,
    line: Anchor,
    dated: bool,
    position: gpui::Point<Pixels>,
    window: &mut Window,
    cx: &mut App,
) {
    let Some(handle) = editor.upgrade() else {
        return;
    };
    let quick = |name: &str, due: Due, cx: &App| {
        let editor = editor.clone();
        Item::run(due_label(name, &due, cx), move |window, cx| {
            date_line(&editor, line, &due, window, cx)
        })
        .done()
    };
    let picker = editor.clone();
    let clear = editor.clone();
    let entries = vec![
        quick("Today", Due::Today, cx),
        quick("Tomorrow", Due::Tomorrow, cx),
        quick("Next week", Due::NextWeek, cx),
        Item::run("Pick a date…", move |window, cx| {
            picker
                .update(cx, |editor, cx| start_picking(editor, line, window, cx))
                .ok();
        })
        .done(),
        Entry::Separator,
        Item::run("Clear due date", move |window, cx| {
            date_line(&clear, line, &Due::Clear, window, cx)
        })
        .disabled(!dated)
        .done(),
    ];
    let focus = handle.focus_handle(cx);
    let menu = crate::navigation::pointer_menu::build(entries, Some(focus), window, cx);
    handle.update(cx, |editor, cx| {
        editor.deploy_mouse_context_menu(position, menu, window, cx)
    });
}

fn start_picking(editor: &mut Editor, line: Anchor, window: &mut Window, cx: &mut Context<Editor>) {
    let field = crate::navigation::tasks::field_editor(None, window, cx);
    window.focus(&field.focus_handle(cx), cx);
    if let Some(lines) = editor.addon_mut::<TaskLines>() {
        lines.picking = Some(Picking { line, field });
    }
    cx.notify();
}

const WRITE_ONLY: &str =
    "Typing a due date works in Write. Switch the page to Write and try again.";

/// Opens the due date field of the task at the newest caret, for Set due date.
pub(crate) fn pick_at_caret(
    editor: &mut Editor,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> bool {
    let Some(lines) = editor.addon::<TaskLines>() else {
        if crate::editor_projection::enabled(editor) {
            return false;
        }
        crate::outline::set_notice(Some(WRITE_ONLY), cx);
        return true;
    };
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let caret = editor
        .selections
        .newest_anchor()
        .head()
        .to_offset(&snapshot);
    let Some(line) = lines
        .line_at(caret, cx, editor)
        .map(|line| line.range.start)
    else {
        crate::outline::set_notice(Some(quire_core::tasks::NO_TASK), cx);
        return true;
    };
    crate::outline::set_notice(None, cx);
    start_picking(editor, line, window, cx);
    true
}

/// Applies the typed due date, or with `confirm` false closes the field. A date that does not
/// read keeps the field open with a notice that says what to type.
fn finish(editor: &WeakEntity<Editor>, confirm: bool, window: &mut Window, cx: &mut App) {
    editor
        .update(cx, |editor, cx| {
            let Some(picking) = editor
                .addon_mut::<TaskLines>()
                .and_then(|lines| lines.picking.take())
            else {
                return;
            };
            if confirm {
                let typed = picking.field.read(cx).text(cx);
                let today = today(cx).unwrap_or_default();
                match quire_core::tasks::date_phrase(&typed, &today) {
                    Ok(date) => {
                        let due = date.map_or(Due::Clear, Due::On);
                        let snapshot = editor.buffer().read(cx).snapshot(cx);
                        let at = picking.line.to_offset(&snapshot).0;
                        crate::tasks::set_due(editor, &due, Some(at), window, cx);
                    }
                    Err(error) => {
                        crate::outline::show_notice(error, cx);
                        if let Some(lines) = editor.addon_mut::<TaskLines>() {
                            lines.picking = Some(picking);
                        }
                        cx.notify();
                        return;
                    }
                }
            }
            window.focus(&editor.focus_handle(cx), cx);
            cx.notify();
        })
        .ok();
}

/// Where a `- ` and a task box follow each other in text Write shows as written: the space,
/// which a spacer replaces so a ligature font draws the characters. List markers are left out;
/// Write draws those as bullets and checkboxes.
fn dash_boxes(text: &str, blocks: &[Block]) -> Vec<usize> {
    text.match_indices("- [")
        .filter(|(at, _)| matches!(text.get(at + 3..at + 5), Some(" ]" | "x]" | "X]")))
        .map(|(at, _)| at + 1)
        .filter(|space| {
            let index = blocks.partition_point(|block| block.from <= *space);
            !index.checked_sub(1).is_some_and(|index| {
                let block = &blocks[index];
                block.kind == "list" && *space < block.content_from
            })
        })
        .collect()
}

fn spacer() -> FoldPlaceholder {
    FoldPlaceholder {
        collapsed_text: Some(" ".into()),
        constrain_width: false,
        merge_adjacent: false,
        type_tag: Some(TypeId::of::<DashGuard>()),
        render: Arc::new(|_, _, cx| {
            let theme = theme_settings::ThemeSettings::get_global(cx);
            let size = theme.buffer_font_size(cx);
            let font = cx.text_system().resolve_font(&theme.buffer_font);
            let width = cx
                .text_system()
                .advance(font, size, ' ')
                .map_or(size * 0.5, |advance| advance.width);
            div().w(width).h_full().into_any_element()
        }),
    }
}

fn has_done_tag(text: &str) -> bool {
    text.match_indices('#').any(|(at, _)| {
        (at == 0 || text[..at].ends_with([' ', '\t']))
            && text[at + 1..]
                .get(..4)
                .is_some_and(|name| name.eq_ignore_ascii_case("done"))
            && !text[at + 5..]
                .starts_with(|c: char| c.is_alphanumeric() || matches!(c, '_' | '-' | '/'))
    })
}

fn restyle(editor: &mut Editor, cx: &mut Context<Editor>) {
    let key = done_key();
    editor.clear_highlights(key, cx);
    let Some(lines) = editor.addon::<TaskLines>() else {
        return;
    };
    let ranges: Vec<_> = lines
        .lines
        .iter()
        .filter(|line| line.done)
        .map(|line| line.text.clone())
        .collect();
    if ranges.is_empty() {
        return;
    }
    let style = gpui::HighlightStyle {
        color: Some(Hsla::from(rgb(Palette::of(cx).done))),
        ..Default::default()
    };
    editor.highlight_text(key, ranges, style, cx);
}

/// Finds the page's task lines after a parse, mutes done ones, and guards `- [ ]` text from
/// ligatures. `shown` says whether the view holds a source range and `anchor` places a source
/// offset in the view.
pub(crate) fn sync(
    editor: &mut Editor,
    text: &str,
    blocks: &[Block],
    shown: impl Fn(&Range<usize>) -> bool,
    anchor: impl Fn(usize) -> Option<Anchor>,
    window: &mut Window,
    cx: &mut Context<Editor>,
) {
    let lines: Vec<TaskLine> = blocks
        .iter()
        .filter(|block| block.kind == "list")
        .filter_map(|block| {
            let content = text.get(block.content_from..block.content_to)?;
            let first = &content[..content.find(['\n', '\r']).unwrap_or(content.len())];
            let marker = quire_core::tasks::read_marker(first)?;
            let end = block.content_from + first.len();
            let range = block.content_from..end;
            if !shown(&range) {
                return None;
            }
            Some(TaskLine {
                range: anchor(range.start)?..anchor(end)?,
                text: anchor(block.content_from + marker.prefix_len)?..anchor(end)?,
                done: marker.checked || has_done_tag(first),
                dated: first.contains('📅'),
            })
        })
        .collect();
    if editor.addon::<TaskLines>().is_none() {
        let theme = cx.observe_global::<theme::GlobalTheme>(restyle);
        editor.register_addon(TaskLines {
            lines: Vec::new(),
            hover: Arc::default(),
            picking: None,
            guards: Default::default(),
            _theme: theme,
        });
    }
    if let Some(addon) = editor.addon_mut::<TaskLines>() {
        addon.lines = lines;
    }
    restyle(editor, cx);
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let key = crate::editor_projection::fold_key("dash guard");
    let guards: Vec<_> = dash_boxes(text, blocks)
        .into_iter()
        .filter(|space| shown(&(*space..space + 1)))
        .filter_map(|space| {
            let range = anchor(space)?..anchor(space + 1)?;
            Some((
                range.start.to_offset(&snapshot)..range.end.to_offset(&snapshot),
                key,
            ))
        })
        .collect();
    let mut kept = editor
        .addon_mut::<TaskLines>()
        .map(|lines| std::mem::take(&mut lines.guards))
        .unwrap_or_default();
    let tag = TypeId::of::<DashGuard>();
    crate::editor_projection::sync_folds(editor, tag, &mut kept, guards, |_| spacer(), window, cx);
    if let Some(lines) = editor.addon_mut::<TaskLines>() {
        lines.guards = kept;
    }
}

/// Removes the task lines' controls, muted text, and ligature guards, when Write ends.
pub(crate) fn clear(editor: &mut Editor, cx: &mut Context<Editor>) {
    editor.unregister_addon::<TaskLines>();
    editor.clear_highlights(done_key(), cx);
    let everything = MultiBufferOffset(0)..editor.buffer().read(cx).snapshot(cx).len();
    editor.remove_folds_with_type(&[everything], TypeId::of::<DashGuard>(), false, cx);
}

/// Checks that the ligature guards are those last wanted, and how many there are; for tests.
#[cfg(test)]
pub(crate) fn check_guards(editor: &Editor, cx: &mut App) -> Result<usize, String> {
    let empty = crate::editor_projection::KeptFolds::default();
    let kept = editor
        .addon::<TaskLines>()
        .map_or(&empty, |lines| &lines.guards);
    crate::editor_projection::check_kept_folds(editor, TypeId::of::<DashGuard>(), kept, cx)?;
    Ok(kept.len())
}

/// The task lines `editor` found, as (done, dated), for tests.
#[cfg(test)]
pub(crate) fn lines(editor: &Editor) -> Vec<(bool, bool)> {
    editor
        .addon::<TaskLines>()
        .map(|lines| {
            lines
                .lines
                .iter()
                .map(|line| (line.done, line.dated))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether `editor` shows a due date field, for tests.
#[cfg(test)]
pub(crate) fn picking(editor: &Editor) -> Option<Entity<Editor>> {
    editor
        .addon::<TaskLines>()
        .and_then(|lines| lines.picking.as_ref())
        .map(|picking| picking.field.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_done_tag_is_a_whole_tag() {
        assert!(has_done_tag("[ ] Ship #done"));
        assert!(has_done_tag("#DONE first"));
        assert!(!has_done_tag("[ ] #doneish"));
        assert!(!has_done_tag("[ ] a#done"));
    }

    #[test]
    fn hover_over_the_line_or_its_controls_shows_them() {
        let hover = Hover::default();
        assert!(!hover.shows(2));
        hover.line.store(2, Ordering::Relaxed);
        assert!(hover.shows(2));
        hover.line.store(NOT_HOVERED, Ordering::Relaxed);
        hover.controls.store(2, Ordering::Relaxed);
        assert!(hover.shows(2) && !hover.shows(3));
    }

    use crate::editor_host::NotebookWindow;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle};

    /// Task commands read the clock as 2026-10-29, a Thursday, in UTC.
    fn page(text: &str, cx: &mut TestAppContext) -> WindowHandle<NotebookWindow> {
        cx.update(|cx| {
            crate::editor_host::init(cx).unwrap();
            cx.set_global(TaskContext {
                now: std::rc::Rc::new(|| 1_793_275_200_000),
                calendar: Some(quire_core::daily::DailyCalendar::utc()),
                layout: Default::default(),
                title: std::rc::Rc::new(|_, _| None),
            });
        });
        let page = cx.add_window(|window, cx| {
            let mut view = NotebookWindow::new(text, window, cx);
            view.show_written(window, cx);
            view
        });
        cx.run_until_parked();
        page
    }

    fn due(iso: &str) -> String {
        let days = quire_core::tasks::DailyNames::new(
            Default::default(),
            quire_core::daily::DailyCalendar::utc(),
        );
        quire_core::tasks::stamp_text(quire_core::tasks::DateField::Due, iso, &days)
    }

    fn shown(page: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
        page.update(cx, |view, window, cx| {
            view.second_editor().update(cx, |editor, cx| {
                editor.snapshot(window, cx).display_snapshot.text()
            })
        })
        .unwrap()
    }

    fn source(page: WindowHandle<NotebookWindow>, cx: &mut TestAppContext) -> String {
        page.update(cx, |view, _, cx| view.source_text(cx)).unwrap()
    }

    fn click(page: WindowHandle<NotebookWindow>, selector: &'static str, cx: &mut TestAppContext) {
        let mut visual = VisualTestContext::from_window(page.into(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        let bounds = visual.debug_bounds(selector).expect(selector);
        visual.simulate_click(bounds.center(), Default::default());
        cx.run_until_parked();
    }

    fn focus_at(page: WindowHandle<NotebookWindow>, range: Range<usize>, cx: &mut TestAppContext) {
        page.update(cx, |view, window, cx| {
            let editor = view.second_editor().clone();
            window.focus(&editor.focus_handle(cx), cx);
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |s| {
                    s.select_ranges([MultiBufferOffset(range.start)..MultiBufferOffset(range.end)])
                })
            });
        })
        .unwrap();
        cx.run_until_parked();
    }

    #[gpui::test]
    fn the_push_buttons_date_one_task_as_one_undoable_edit(cx: &mut TestAppContext) {
        let page = page("- [ ] Call Ada\n- [x] Filed ✅ 2026-10-28\n", cx);
        let lines = page
            .update(cx, |view, _, cx| {
                super::lines(view.second_editor().read(cx))
            })
            .unwrap();
        assert_eq!(lines, vec![(false, false), (true, false)]);
        click(page, "quire-task-due-tomorrow", cx);
        assert_eq!(
            source(page, cx),
            format!(
                "- [ ] Call Ada{}\n- [x] Filed ✅ 2026-10-28\n",
                due("2026-10-30")
            )
        );
        click(page, "quire-task-due-next-week", cx);
        assert_eq!(
            source(page, cx),
            format!(
                "- [ ] Call Ada{}\n- [x] Filed ✅ 2026-10-28\n",
                due("2026-11-02")
            ),
            "next week replaces the due stamp with next Monday"
        );
        page.update(cx, |view, window, cx| {
            view.second_editor().update(cx, |editor, cx| {
                editor.undo(&editor::actions::Undo, window, cx)
            })
        })
        .unwrap();
        assert_eq!(
            source(page, cx),
            format!(
                "- [ ] Call Ada{}\n- [x] Filed ✅ 2026-10-28\n",
                due("2026-10-30")
            )
        );
    }

    #[gpui::test]
    fn the_calendar_opens_the_due_menu(cx: &mut TestAppContext) {
        let page = page("- [ ] Call Ada\n", cx);
        click(page, "quire-task-due-menu", cx);
        page.update(cx, |view, _, cx| {
            assert!(view.second_editor().read(cx).has_mouse_context_menu());
        })
        .unwrap();
        assert_eq!(source(page, cx), "- [ ] Call Ada\n");
    }

    #[gpui::test]
    fn done_tasks_get_no_push_buttons(cx: &mut TestAppContext) {
        let page = page("- [x] Filed\n- {{[[DONE]]}} Sent\n- [ ] Open #done\n", cx);
        let mut visual = VisualTestContext::from_window(page.into(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("quire-task-due-tomorrow").is_none());
        assert!(visual.debug_bounds("quire-task-box-checked").is_some());
    }

    #[gpui::test]
    fn due_actions_date_every_selected_task_and_clear(cx: &mut TestAppContext) {
        let text = "- [ ] One\n- plain\n- [ ] Two\n";
        let page = page(text, cx);
        focus_at(page, 0..text.len() - 1, cx);
        page.update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(crate::tasks::DueNextWeek), cx)
        })
        .unwrap();
        cx.run_until_parked();
        let next = due("2026-11-02");
        assert_eq!(
            source(page, cx),
            format!("- [ ] One{next}\n- plain\n- [ ] Two{next}\n")
        );
        focus_at(page, 3..3, cx);
        page.update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(crate::tasks::ClearDueDate), cx)
        })
        .unwrap();
        cx.run_until_parked();
        assert_eq!(
            source(page, cx),
            format!("- [ ] One\n- plain\n- [ ] Two{next}\n")
        );
    }

    #[gpui::test]
    fn set_due_date_opens_a_field_that_reads_typed_dates(cx: &mut TestAppContext) {
        let page = page("- [ ] Call Ada\n", cx);
        focus_at(page, 6..6, cx);
        page.update(cx, |_, window, cx| {
            window.dispatch_action(Box::new(crate::tasks::SetDueDate), cx)
        })
        .unwrap();
        cx.run_until_parked();
        let field = page
            .update(cx, |view, _, cx| {
                super::picking(view.second_editor().read(cx))
            })
            .unwrap()
            .expect("a due date field");
        let mut visual = VisualTestContext::from_window(page.into(), cx);
        visual.update(|window, cx| window.draw(cx).clear(cx));
        assert!(visual.debug_bounds("quire-task-due-field").is_some());
        visual.update(|window, cx| {
            assert!(field.focus_handle(cx).is_focused(window));
            field.update(cx, |field, cx| field.set_text("someday", window, cx));
        });
        cx.simulate_keystrokes(page.into(), "enter");
        cx.run_until_parked();
        assert_eq!(
            source(page, cx),
            "- [ ] Call Ada\n",
            "a bad date edits nothing"
        );
        assert!(cx.read(crate::outline::notice).is_some());
        let mut visual = VisualTestContext::from_window(page.into(), cx);
        visual.update(|window, cx| {
            assert!(
                field.focus_handle(cx).is_focused(window),
                "the field stays open"
            );
            field.update(cx, |field, cx| field.set_text("+3", window, cx));
        });
        cx.simulate_keystrokes(page.into(), "enter");
        cx.run_until_parked();
        assert_eq!(
            source(page, cx),
            format!("- [ ] Call Ada{}\n", due("2026-11-01"))
        );
        page.update(cx, |view, window, cx| {
            let editor = view.second_editor().read(cx);
            assert!(super::picking(editor).is_none());
            assert!(editor.focus_handle(cx).is_focused(window));
        })
        .unwrap();
    }

    #[gpui::test]
    fn crlf_empty_tasks_and_quoted_task_text_render_cleanly(cx: &mut TestAppContext) {
        let page = page("- [ ] a\r\n- [ ]\r\n\r\n> - [ ] Quoted\r\n", cx);
        let shown = shown(page, cx);
        assert_eq!(
            shown.matches('☐').count(),
            2,
            "a CRLF line's empty task is a task: {shown:?}"
        );
        assert!(
            shown.contains("- [ ] Quoted"),
            "quoted task text reads as written: {shown:?}"
        );
        let guards = page
            .update(cx, |view, _, cx| {
                view.second_editor().update(cx, |editor, cx| {
                    let snapshot = editor.display_snapshot(cx);
                    let len = snapshot.buffer_snapshot().len();
                    snapshot
                        .folds_in_range(MultiBufferOffset(0)..len)
                        .filter(|fold| fold.placeholder.type_tag == Some(TypeId::of::<DashGuard>()))
                        .count()
                })
            })
            .unwrap();
        assert_eq!(guards, 1, "only the quoted dash and box are split apart");
    }
}
