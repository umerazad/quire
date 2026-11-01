//! Write's Markdown styling beyond list syntax, as `rt` draws it (`Editor.tsx` decorations):
//! emphasis, code, strikethrough, heading, and quote marks hide outside the block holding the caret
//! while the text they mark keeps its style; a thematic break draws as a rule unless the caret is on
//! its line; a GFM table draws as a table unless the caret is in it. Everything is read from the
//! page's Markdown and inline syntax trees, and the Markdown text never changes.

use crate::rich_text::Rich;
use editor::{
    Addon, Editor, HighlightKey, NavigationOverlayKey,
    display_map::{BlockPlacement, BlockProperties, BlockStyle, CustomBlockId, RenderBlock},
};
use gpui::{App, Context, HighlightStyle, WeakEntity, Window, prelude::*};
use language::BufferSnapshot;
use multi_buffer::{Anchor, ToOffset};
use std::{ops::Range, sync::Arc};

/// How Write styles a span of text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Style {
    Strong,
    Emphasis,
    Strike,
    Code,
    Heading,
    Quote,
}

impl Style {
    pub const ALL: [Style; 6] = [
        Style::Strong,
        Style::Emphasis,
        Style::Strike,
        Style::Code,
        Style::Heading,
        Style::Quote,
    ];
}

/// Source text Write replaces outside `block`, the source range that shows it again while it
/// holds the caret.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mark {
    pub range: Range<usize>,
    pub block: Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Left,
    Center,
    Right,
}

/// A GFM table: its source rows, the header row's cells and the body rows' cells, and each
/// column's alignment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Table {
    pub mark: Mark,
    pub rows: Vec<Vec<Rich>>,
    pub align: Vec<Align>,
}

#[derive(Default, Debug)]
pub struct Marks {
    pub hidden: Vec<Mark>,
    pub styled: Vec<(Style, Range<usize>)>,
    pub rules: Vec<Mark>,
    pub tables: Vec<Table>,
    /// Arabic-script runs on Urdu lines, set in Nastaliq.
    pub urdu: Vec<Range<usize>>,
}

/// Inline nodes whose text Write leaves alone: code keeps its delimiters' content, and HTML and
/// math are not Markdown emphasis.
const OPAQUE: [&str; 3] = ["html_tag", "latex_block", "email_autolink"];

fn text(snapshot: &BufferSnapshot, range: Range<usize>) -> String {
    snapshot.text_for_range(range).collect()
}

fn without_newline(snapshot: &BufferSnapshot, range: Range<usize>) -> Range<usize> {
    let tail = text(snapshot, range.clone());
    let trimmed = tail.trim_end_matches(['\n', '\r']);
    range.start..range.start + trimmed.len()
}

/// The marks and styles of the current parse; `None` while either syntax tree is stale.
pub fn marks(snapshot: &BufferSnapshot, blocks: &[crate::markdown_blocks::Block]) -> Option<Marks> {
    let block_of = |range: &Range<usize>| {
        blocks[..blocks.partition_point(|block| block.from <= range.start)]
            .last()
            .filter(|block| range.end <= block.content_to)
            .map(|block| block.from..block.content_to)
            .unwrap_or_else(|| range.clone())
    };
    let primary = snapshot
        .syntax_layers()
        .find(|layer| layer.language.name().as_ref() == "Markdown")?;
    let root = primary.node();
    if root.has_changes() {
        return None;
    }
    let mut marks = Marks::default();
    let mut stack = vec![(root, false)];
    while let Some((node, in_quote)) = stack.pop() {
        let range = node.byte_range();
        match node.kind() {
            "fenced_code_block" | "indented_code_block" | "minus_metadata" | "plus_metadata" => {
                if in_quote {
                    continuation_quote_marks(snapshot, node, &block_of, &mut marks.hidden);
                }
                continue;
            }
            "thematic_break" => {
                let range = without_newline(snapshot, range);
                let line = snapshot.text.point_to_offset(language::Point::new(
                    snapshot.text.offset_to_point(range.start).row,
                    0,
                ));
                marks.rules.push(Mark {
                    block: line..range.end,
                    range: line..range.end,
                });
                if in_quote {
                    continuation_quote_marks(snapshot, node, &block_of, &mut marks.hidden);
                }
                continue;
            }
            "pipe_table" => {
                let range = without_newline(snapshot, range);
                let line = snapshot.text.point_to_offset(language::Point::new(
                    snapshot.text.offset_to_point(range.start).row,
                    0,
                ));
                let (rows, align) = table_cells(snapshot, node);
                marks.tables.push(Table {
                    mark: Mark {
                        block: block_of(&range).start.min(line)..range.end,
                        range: line..range.end,
                    },
                    rows,
                    align,
                });
                continue;
            }
            "atx_heading" => {
                marks
                    .styled
                    .push((Style::Heading, without_newline(snapshot, range.clone())));
                let mut cursor = node.walk();
                if let Some(marker) = node.children(&mut cursor).find(|child| {
                    child.kind().starts_with("atx_h") && child.kind().ends_with("_marker")
                }) {
                    let mut end = marker.end_byte();
                    if snapshot.text.chars_at(end).next() == Some(' ') {
                        end += 1;
                    }
                    let mark = marker.start_byte()..end;
                    marks.hidden.push(Mark {
                        block: block_of(&mark),
                        range: mark,
                    });
                }
            }
            "block_quote" => {
                marks
                    .styled
                    .push((Style::Quote, without_newline(snapshot, range.clone())));
            }
            "block_quote_marker" | "block_continuation" if in_quote => {
                quote_marks(snapshot, node, &block_of, &mut marks.hidden);
                continue;
            }
            _ => {}
        }
        let quoted = in_quote || node.kind() == "block_quote";
        let mut cursor = node.walk();
        let children: Vec<_> = node.children(&mut cursor).collect();
        stack.extend(children.into_iter().rev().map(|child| (child, quoted)));
    }
    for layer in snapshot
        .syntax_layers()
        .filter(|layer| layer.language.name().as_ref() == "Markdown-Inline")
    {
        let root = layer.node();
        if root.has_changes() {
            return None;
        }
        let mut stack = vec![root];
        while let Some(node) = stack.pop() {
            let range = node.byte_range();
            if OPAQUE.contains(&node.kind()) {
                continue;
            }
            let (style, delimiter) = match node.kind() {
                "emphasis" => (Style::Emphasis, "emphasis_delimiter"),
                "strong_emphasis" => (Style::Strong, "emphasis_delimiter"),
                "strikethrough" => (Style::Strike, "emphasis_delimiter"),
                "code_span" => (Style::Code, "code_span_delimiter"),
                "backslash_escape" => {
                    let mark = range.start..range.start + 1;
                    marks.hidden.push(Mark {
                        block: block_of(&range),
                        range: mark,
                    });
                    continue;
                }
                _ => {
                    let mut cursor = node.walk();
                    let children: Vec<_> = node.children(&mut cursor).collect();
                    stack.extend(children.into_iter().rev());
                    continue;
                }
            };
            marks.styled.push((style, range.clone()));
            let block = block_of(&range);
            let mut cursor = node.walk();
            for child in node.children(&mut cursor) {
                if child.kind() == delimiter {
                    marks.hidden.push(Mark {
                        range: child.byte_range(),
                        block: block.clone(),
                    });
                } else if style != Style::Code {
                    stack.push(child);
                }
            }
        }
    }
    marks.hidden.sort_by_key(|mark| mark.range.start);
    marks.hidden.dedup_by(|next, kept| {
        if kept.block == next.block && kept.range.end == next.range.start {
            kept.range.end = next.range.end;
            true
        } else {
            false
        }
    });
    marks
        .styled
        .sort_by_key(|(_, range)| (range.start, std::cmp::Reverse(range.end)));
    let mut outer = std::collections::HashMap::new();
    marks.styled.retain(|(style, range)| {
        let nested = outer
            .get(style)
            .is_some_and(|around: &Range<usize>| range.end <= around.end);
        if !nested {
            outer.insert(*style, range.clone());
        }
        !nested
    });
    Some(marks)
}

/// The quote marks in the continuation prefixes inside `node`, whose own text, such as code, may
/// hold a `>` of its own.
fn continuation_quote_marks(
    snapshot: &BufferSnapshot,
    node: language::Node<'_>,
    block_of: &impl Fn(&Range<usize>) -> Range<usize>,
    hidden: &mut Vec<Mark>,
) {
    let mut stack = vec![node];
    while let Some(node) = stack.pop() {
        if node.kind() == "block_continuation" {
            quote_marks(snapshot, node, block_of, hidden);
            continue;
        }
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
}

/// Every `>` that marks a quote level in `node`'s text, which is a quote's marker or a
/// continuation line's prefix.
fn quote_marks(
    snapshot: &BufferSnapshot,
    node: language::Node<'_>,
    block_of: &impl Fn(&Range<usize>) -> Range<usize>,
    hidden: &mut Vec<Mark>,
) {
    let range = node.byte_range();
    let source = text(snapshot, range.clone());
    let mut line_start = true;
    for (at, c) in source.char_indices() {
        match c {
            '\n' => line_start = true,
            '>' if line_start => {
                let mark = range.start + at..range.start + at + 1;
                hidden.push(Mark {
                    block: block_of(&mark),
                    range: mark,
                });
            }
            ' ' | '\t' => {}
            _ => line_start = false,
        }
    }
}

fn table_cells(
    snapshot: &BufferSnapshot,
    table: language::Node<'_>,
) -> (Vec<Vec<Rich>>, Vec<Align>) {
    let mut rows = Vec::new();
    let mut align = Vec::new();
    let mut cursor = table.walk();
    for row in table.named_children(&mut cursor) {
        let mut cells = row.walk();
        match row.kind() {
            "pipe_table_header" | "pipe_table_row" => rows.push(
                row.named_children(&mut cells)
                    .filter(|cell| cell.kind() == "pipe_table_cell")
                    .map(|cell| crate::rich_text::inline(text(snapshot, cell.byte_range()).trim()))
                    .collect(),
            ),
            "pipe_table_delimiter_row" => {
                align = row
                    .named_children(&mut cells)
                    .filter(|cell| cell.kind() == "pipe_table_delimiter_cell")
                    .map(|cell| {
                        let mut parts = cell.walk();
                        let kinds: Vec<_> = cell
                            .named_children(&mut parts)
                            .map(|part| part.kind())
                            .collect();
                        match (
                            kinds.contains(&"pipe_table_align_left"),
                            kinds.contains(&"pipe_table_align_right"),
                        ) {
                            (true, true) => Align::Center,
                            (false, true) => Align::Right,
                            _ => Align::Left,
                        }
                    })
                    .collect();
            }
            _ => {}
        }
    }
    (rows, align)
}

/// Letters Urdu uses that Arabic does not, as `rt`'s `script-lang.ts` lists them: ٹ ڈ ڑ ک گ ں ھ
/// ہ ۂ ۃ ی ے ۓ and the Urdu full stop ۔. A line holding one is an Urdu line.
fn urdu_letter(c: char) -> bool {
    matches!(
        c,
        '\u{0679}'
            | '\u{0688}'
            | '\u{0691}'
            | '\u{06A9}'
            | '\u{06AF}'
            | '\u{06BA}'
            | '\u{06BE}'
            | '\u{06C1}'..='\u{06C3}'
            | '\u{06CC}'
            | '\u{06D2}'..='\u{06D4}'
    )
}

/// Characters `rt`'s "Quire Urdu" face covers (its `unicode-range`): Arabic script and the
/// joiners and direction marks used with it. Other characters on an Urdu line keep the editor font.
fn nastaliq_covers(c: char) -> bool {
    matches!(
        c,
        '\u{0600}'..='\u{06FF}'
            | '\u{0750}'..='\u{077F}'
            | '\u{0870}'..='\u{08FF}'
            | '\u{200C}'..='\u{200F}'
            | '\u{FB50}'..='\u{FDFF}'
            | '\u{FE70}'..='\u{FEFF}'
    )
}

/// The byte ranges of `text` to set in Nastaliq: each run of covered characters on a line that
/// holds an Urdu letter.
pub fn urdu_runs(text: &str) -> Vec<Range<usize>> {
    let mut runs = Vec::new();
    let mut line_start = 0;
    for line in text.split_inclusive('\n') {
        if line.chars().any(urdu_letter) {
            let mut run: Option<Range<usize>> = None;
            for (at, c) in line.char_indices() {
                let at = line_start + at;
                match (&mut run, nastaliq_covers(c)) {
                    (Some(run), true) => run.end = at + c.len_utf8(),
                    (None, true) => run = Some(at..at + c.len_utf8()),
                    (Some(_), false) => runs.extend(run.take()),
                    (None, false) => {}
                }
            }
            runs.extend(run);
        }
        line_start += line.len();
    }
    runs
}

/// A mark, rule, or table in a Write view, in view anchors, with the source text it stood for
/// when parsed: it is replaced only while that text is unchanged.
#[derive(Clone)]
pub(crate) struct Anchored {
    pub range: Range<Anchor>,
    pub block: Range<Anchor>,
    pub source: Arc<str>,
}

#[derive(Clone)]
pub(crate) struct AnchoredTable {
    pub mark: Anchored,
    pub rows: Arc<[Vec<Rich>]>,
    pub align: Arc<[Align]>,
}

/// One Write view's marks, styles, rules, and tables from its latest parse.
#[derive(Clone, Default)]
pub(crate) struct Presentation {
    pub hidden: Arc<[Anchored]>,
    pub styled: Arc<[(Style, Range<Anchor>)]>,
    pub rules: Arc<[Anchored]>,
    pub tables: Arc<[AnchoredTable]>,
    pub urdu: Arc<[Range<Anchor>]>,
}

impl Presentation {
    /// Anchors the marks that lie inside the view, as `anchor` maps a source byte into it.
    pub(crate) fn anchor(
        snapshot: &BufferSnapshot,
        marks: Marks,
        contained: impl Fn(&Range<usize>) -> bool,
        anchor: impl Fn(usize) -> Option<Anchor>,
    ) -> Self {
        let anchored = |mark: &Mark| {
            if !contained(&mark.range) || !contained(&mark.block) {
                return None;
            }
            Some(Anchored {
                range: anchor(mark.range.start)?..anchor(mark.range.end)?,
                block: anchor(mark.block.start)?..anchor(mark.block.end)?,
                source: text(snapshot, mark.range.clone()).into(),
            })
        };
        Self {
            hidden: marks.hidden.iter().filter_map(anchored).collect(),
            styled: marks
                .styled
                .into_iter()
                .filter(|(_, range)| contained(range))
                .filter_map(|(style, range)| {
                    Some((style, anchor(range.start)?..anchor(range.end)?))
                })
                .collect(),
            rules: marks.rules.iter().filter_map(anchored).collect(),
            tables: marks
                .tables
                .into_iter()
                .filter_map(|table| {
                    Some(AnchoredTable {
                        mark: anchored(&table.mark)?,
                        rows: table.rows.into(),
                        align: table.align.into(),
                    })
                })
                .collect(),
            urdu: marks
                .urdu
                .into_iter()
                .filter(|range| contained(range))
                .filter_map(|range| Some(anchor(range.start)?..anchor(range.end)?))
                .collect(),
        }
    }
}

/// `rt`'s Markdown colours (`styles.css`, `roam-theme.css`, `dark-theme.css`) in the light or dark
/// theme.
#[derive(Clone, Copy)]
struct Palette {
    strong: u32,
    strike: u32,
    code_ink: u32,
    code_back: u32,
    heading: u32,
    quote: u32,
    syntax: u32,
    rule: u32,
    table_ink: u32,
    table_line: u32,
    table_head: u32,
}

impl Palette {
    fn of(cx: &App) -> Self {
        use theme::ActiveTheme;
        if cx.theme().appearance.is_light() {
            Self {
                strong: 0x1d2d21,
                strike: 0x939b87,
                code_ink: 0x343a40,
                code_back: 0xf1f3f5,
                heading: 0x46573e,
                quote: 0x8b927c,
                syntax: 0xb0b6a2,
                rule: 0xdee2e6,
                table_ink: 0x7c8c6b,
                table_line: 0xdde5d2,
                table_head: 0xf0f4e7,
            }
        } else {
            Self {
                strong: 0xcfe3d3,
                strike: 0x8b9380,
                code_ink: 0xcbd3db,
                code_back: 0x1a1b1b,
                heading: 0xb2c6a9,
                quote: 0x919882,
                syntax: 0x747966,
                rule: 0x33373c,
                table_ink: 0x90a17f,
                table_line: 0x292b26,
                table_head: 0x1b1c19,
            }
        }
    }

    /// GPUI highlights cannot change a run's size, and Zed gives every display row one height, so
    /// headings are bold in `rt`'s heading colour rather than larger.
    fn style(self, style: Style) -> HighlightStyle {
        let color = |ink: u32| Some(gpui::rgb(ink).into());
        match style {
            Style::Strong => HighlightStyle {
                color: color(self.strong),
                font_weight: Some(gpui::FontWeight::BOLD),
                ..Default::default()
            },
            Style::Emphasis => HighlightStyle {
                font_style: Some(gpui::FontStyle::Italic),
                ..Default::default()
            },
            Style::Strike => HighlightStyle {
                color: color(self.strike),
                strikethrough: Some(gpui::StrikethroughStyle {
                    thickness: gpui::px(1.),
                    color: color(self.strike),
                }),
                ..Default::default()
            },
            Style::Code => HighlightStyle {
                color: color(self.code_ink),
                background_color: color(self.code_back),
                ..Default::default()
            },
            Style::Heading => HighlightStyle {
                color: color(self.heading),
                font_weight: Some(gpui::FontWeight::BOLD),
                ..Default::default()
            },
            Style::Quote => HighlightStyle {
                color: color(self.quote),
                font_style: Some(gpui::FontStyle::Italic),
                ..Default::default()
            },
        }
    }
}

fn style_key(style: Style) -> HighlightKey {
    struct Key<const N: usize>;
    let key = match style {
        Style::Strong => NavigationOverlayKey::unique::<Key<0>>(),
        Style::Emphasis => NavigationOverlayKey::unique::<Key<1>>(),
        Style::Strike => NavigationOverlayKey::unique::<Key<2>>(),
        Style::Code => NavigationOverlayKey::unique::<Key<3>>(),
        Style::Heading => NavigationOverlayKey::unique::<Key<4>>(),
        Style::Quote => NavigationOverlayKey::unique::<Key<5>>(),
    };
    HighlightKey::NavigationOverlay(key)
}

fn syntax_key() -> HighlightKey {
    struct Syntax;
    HighlightKey::NavigationOverlay(NavigationOverlayKey::unique::<Syntax>())
}

fn urdu_key() -> HighlightKey {
    struct Urdu;
    HighlightKey::NavigationOverlay(NavigationOverlayKey::unique::<Urdu>())
}

/// The Nastaliq run style. GPUI rows share one height, so unlike `rt` the line does not grow to
/// 2.5 times the font size.
pub(crate) fn urdu_style() -> HighlightStyle {
    HighlightStyle {
        font_family: Some(crate::script_fonts::URDU),
        ..Default::default()
    }
}

/// Styles the marked text, and the marks themselves where they show, in muted ink as `rt`'s
/// `md-syntax`. An empty presentation clears every style.
pub(crate) fn highlight(
    editor: &mut Editor,
    presentation: &Presentation,
    cx: &mut Context<Editor>,
) {
    let palette = Palette::of(cx);
    for style in Style::ALL {
        let key = style_key(style);
        editor.clear_highlights(key, cx);
        let ranges: Vec<_> = presentation
            .styled
            .iter()
            .filter(|(kind, _)| *kind == style)
            .map(|(_, range)| range.clone())
            .collect();
        if !ranges.is_empty() {
            editor.highlight_text(key, ranges, palette.style(style), cx);
        }
    }
    editor.clear_highlights(syntax_key(), cx);
    if !presentation.hidden.is_empty() {
        let marks = presentation
            .hidden
            .iter()
            .map(|mark| mark.range.clone())
            .collect();
        let ink = HighlightStyle::color(gpui::rgb(palette.syntax).into());
        editor.highlight_text(syntax_key(), marks, ink, cx);
    }
    editor.clear_highlights(urdu_key(), cx);
    if !presentation.urdu.is_empty() {
        editor.highlight_text(urdu_key(), presentation.urdu.to_vec(), urdu_style(), cx);
    }
}

/// The styles a view applies, by style, in source offsets; for tests.
#[cfg(test)]
pub(crate) fn applied(editor: &Editor, cx: &App) -> Vec<(Style, Vec<Range<usize>>)> {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    Style::ALL
        .into_iter()
        .filter_map(|style| {
            let (_, ranges) = editor.text_highlights(style_key(style), cx)?;
            Some((
                style,
                ranges
                    .iter()
                    .map(|range| {
                        range.start.to_offset(&snapshot).0..range.end.to_offset(&snapshot).0
                    })
                    .collect(),
            ))
        })
        .collect()
}

/// The ranges a view sets in Nastaliq, in source offsets, with their style; for tests.
#[cfg(test)]
pub(crate) fn urdu_applied(
    editor: &Editor,
    cx: &App,
) -> Option<(HighlightStyle, Vec<Range<usize>>)> {
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let (style, ranges) = editor.text_highlights(urdu_key(), cx)?;
    Some((
        style,
        ranges
            .iter()
            .map(|range| range.start.to_offset(&snapshot).0..range.end.to_offset(&snapshot).0)
            .collect(),
    ))
}

/// A rule or table drawn in place of its source rows.
pub(crate) enum Replacement {
    Rule(Anchored),
    Table(AnchoredTable),
}

impl Replacement {
    fn mark(&self) -> &Anchored {
        match self {
            Replacement::Rule(mark) => mark,
            Replacement::Table(table) => &table.mark,
        }
    }
}

/// The rules and tables a view draws now, as Zed blocks replacing their rows.
#[derive(Default)]
struct Replaced {
    shown: Vec<(Range<Anchor>, Arc<str>, CustomBlockId)>,
}

impl Addon for Replaced {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// Draws exactly `wanted`, keeping blocks whose rows and source are unchanged. A caret that
/// moves onto replaced rows lands at their start, inside the source, which shows it again.
pub(crate) fn replace(editor: &mut Editor, wanted: Vec<Replacement>, cx: &mut Context<Editor>) {
    if wanted.is_empty() && editor.addon::<Replaced>().is_none() {
        return;
    }
    let snapshot = editor.buffer().read(cx).snapshot(cx);
    let offsets =
        |range: &Range<Anchor>| range.start.to_offset(&snapshot)..range.end.to_offset(&snapshot);
    let mut old = editor
        .addon_mut::<Replaced>()
        .map(|replaced| std::mem::take(&mut replaced.shown))
        .unwrap_or_default();
    let mut shown = Vec::new();
    for replacement in wanted {
        let mark = replacement.mark();
        let at = offsets(&mark.range);
        if let Some(index) = old
            .iter()
            .position(|(range, source, _)| offsets(range) == at && *source == mark.source)
        {
            shown.push(old.remove(index));
            continue;
        }
        let rows =
            snapshot.offset_to_point(at.end).row - snapshot.offset_to_point(at.start).row + 1;
        let host = cx.entity().downgrade();
        let start = mark.range.start;
        let render = match &replacement {
            Replacement::Rule(_) => render_rule(host, start),
            Replacement::Table(table) => render_table(host, start, table.clone()),
        };
        let block = editor.insert_blocks(
            [BlockProperties {
                placement: BlockPlacement::Replace(mark.range.start..=mark.range.end),
                height: Some(rows),
                style: BlockStyle::Flex,
                render,
                priority: 0,
            }],
            None,
            cx,
        )[0];
        shown.push((mark.range.clone(), mark.source.clone(), block));
    }
    if !old.is_empty() {
        let removed = old.into_iter().map(|(_, _, block)| block).collect();
        editor.remove_blocks(removed, None, cx);
    }
    if editor.addon::<Replaced>().is_none() {
        editor.register_addon(Replaced::default());
    }
    editor
        .addon_mut::<Replaced>()
        .expect("registered above")
        .shown = shown;
}

/// Puts the caret at `at`, in a rule or table's source, which shows it for editing.
fn edit_at(host: &WeakEntity<Editor>, at: Anchor, window: &mut Window, cx: &mut App) {
    host.update(cx, |editor, cx| {
        window.focus(&gpui::Focusable::focus_handle(editor, cx), cx);
        editor.change_selections(Default::default(), window, cx, |selections| {
            selections.select_anchor_ranges([at..at])
        });
    })
    .ok();
}

/// `rt`'s rule: a 2 px line in the theme's line colour, across the text column.
fn render_rule(host: WeakEntity<Editor>, at: Anchor) -> RenderBlock {
    Arc::new(move |block| {
        let palette = Palette::of(block.app);
        let host = host.clone();
        gpui::div()
            .id(block.block_id)
            .debug_selector(|| "quire-rule".into())
            .role(gpui::Role::Splitter)
            .w_full()
            .h(block.line_height * block.height as f32)
            .pl(block.anchor_x)
            .pr(block.em_width)
            .flex()
            .items_center()
            .cursor_text()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                edit_at(&host, at, window, cx);
            })
            .child(
                gpui::div()
                    .w_full()
                    .h(gpui::px(2.))
                    .bg(gpui::rgb(palette.rule)),
            )
            .into_any_element()
    })
}

/// A GFM table as `rt`'s Markdown widget draws it: a bordered grid with a shaded header, each row
/// one line high and each column aligned as its delimiter says. The spare row the delimiter line
/// leaves pads the grid.
fn render_table(host: WeakEntity<Editor>, at: Anchor, table: AnchoredTable) -> RenderBlock {
    Arc::new(move |block| {
        let palette = Palette::of(block.app);
        let line = gpui::rgb(palette.table_line);
        let columns = table.rows.iter().map(Vec::len).max().unwrap_or(0);
        let last = table.rows.len().saturating_sub(1);
        let host = host.clone();
        let ink: gpui::Hsla = gpui::rgb(palette.table_ink).into();
        let font = <theme_settings::ThemeSettings as settings::Settings>::get_global(block.app)
            .buffer_font
            .clone();
        let head = gpui::Font {
            weight: gpui::FontWeight::MEDIUM,
            ..font.clone()
        };
        let rows: Vec<_> = table
            .rows
            .iter()
            .enumerate()
            .map(|(index, cells)| {
                let base = if index == 0 { &head } else { &font };
                gpui::div()
                    .flex()
                    .w_full()
                    .h(block.line_height)
                    .border_t_1()
                    .border_color(line)
                    .when(index == 0, |row| row.bg(gpui::rgb(palette.table_head)))
                    .when(index == last, |row| row.border_b_1())
                    .children((0..columns).map(|column| {
                        let cell = gpui::div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .px(block.em_width)
                            .flex()
                            .items_center()
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .border_l_1()
                            .border_color(line)
                            .when(column + 1 == columns, |cell| cell.border_r_1())
                            .children(cells.get(column).map(|rich| {
                                cell_text(
                                    &host,
                                    index * columns + column,
                                    rich,
                                    ink,
                                    base.clone(),
                                    block.app,
                                )
                            }));
                        match table.align.get(column) {
                            Some(Align::Right) => cell.justify_end(),
                            Some(Align::Center) => cell.justify_center(),
                            _ => cell,
                        }
                    }))
            })
            .collect();
        gpui::div()
            .id(block.block_id)
            .debug_selector(|| "quire-table".into())
            .role(gpui::Role::Table)
            .w_full()
            .h(block.line_height * block.height as f32)
            .pl(block.anchor_x)
            .pr(block.em_width)
            .flex()
            .flex_col()
            .justify_center()
            .text_color(ink)
            .cursor_text()
            .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                cx.stop_propagation();
                edit_at(&host, at, window, cx);
            })
            .children(rows)
            .into_any_element()
    })
}

/// A table cell's inline Markdown, as rows draw it: emphasis, code, links, and tags, with their
/// syntax hidden. A press on a link opens it through the page's notebook instead of showing the
/// table's source.
fn cell_text(
    host: &WeakEntity<Editor>,
    ordinal: usize,
    rich: &Rich,
    ink: gpui::Hsla,
    font: gpui::Font,
    cx: &App,
) -> gpui::AnyElement {
    let text = gpui::StyledText::new(rich.text.clone())
        .with_runs(crate::rich_text::runs_in(rich, ink, font, cx));
    if rich.links.is_empty() {
        return text.into_any_element();
    }
    let layout = text.layout().clone();
    let (ranges, links): (Vec<_>, Vec<_>) = rich.links.iter().cloned().unzip();
    let pressed = ranges.clone();
    let host = host.clone();
    gpui::div()
        .debug_selector(move || format!("quire-table-link-{ordinal}"))
        .on_mouse_down(gpui::MouseButton::Left, move |event, _, cx| {
            if layout
                .index_for_position(event.position)
                .is_ok_and(|at| pressed.iter().any(|range| range.contains(&at)))
            {
                cx.stop_propagation();
            }
        })
        .child(
            gpui::InteractiveText::new(("quire-table-cell", ordinal), text).on_click(
                ranges,
                move |index, window, cx| {
                    cx.stop_propagation();
                    let notebook = host.upgrade().and_then(|editor| {
                        crate::references::host(editor.read(cx))
                            .map(|host| host.read(cx).notebook().clone())
                    });
                    if let (Some(link), Some(notebook)) = (links.get(index), notebook) {
                        link.open(window.modifiers().shift, notebook.as_ref(), window, cx);
                    }
                },
            ),
        )
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn parsed(text: &str, cx: &mut TestAppContext) -> (Marks, String) {
        cx.update(|cx| crate::editor_host::init(cx).unwrap());
        let source = cx.update(|cx| crate::markdown::buffer(text, cx));
        cx.executor().run_until_parked();
        source.read_with(cx, |source, _| {
            let snapshot = source.snapshot();
            let blocks = crate::markdown_blocks::blocks(&snapshot).unwrap();
            (marks(&snapshot, &blocks).unwrap(), snapshot.text())
        })
    }

    fn hidden(marks: &Marks, text: &str) -> Vec<String> {
        marks
            .hidden
            .iter()
            .map(|mark| text[mark.range.clone()].to_owned())
            .collect()
    }

    #[gpui::test]
    fn inline_marks_hide_and_style_their_text(cx: &mut TestAppContext) {
        let note = "- **bold** *it* _un_ `code` ~~gone~~ \\* ^id\n";
        let (marks, text) = parsed(note, cx);
        assert_eq!(
            hidden(&marks, &text),
            ["**", "**", "*", "*", "_", "_", "`", "`", "~~", "~~", "\\"]
        );
        let styled: Vec<_> = marks
            .styled
            .iter()
            .map(|(style, range)| (*style, &text[range.clone()]))
            .collect();
        assert_eq!(
            styled,
            [
                (Style::Strong, "**bold**"),
                (Style::Emphasis, "*it*"),
                (Style::Emphasis, "_un_"),
                (Style::Code, "`code`"),
                (Style::Strike, "~~gone~~"),
            ]
        );
        assert!(
            marks
                .hidden
                .iter()
                .all(|mark| mark.block == (0..note.len() - 1))
        );
    }

    #[gpui::test]
    fn headings_quotes_rules_and_tables_are_found_outside_code(cx: &mut TestAppContext) {
        let note = "## Title\n\n> one\n> two\n\n---\n\n- ---\n\n| a | b |\n| :-- | --: |\n| 1 | 2 |\n\n```\n**not** > no\n---\n```\n";
        let (marks, text) = parsed(note, cx);
        assert_eq!(hidden(&marks, &text), ["## ", ">", ">"]);
        let styled: Vec<_> = marks
            .styled
            .iter()
            .map(|(style, range)| (*style, &text[range.clone()]))
            .collect();
        assert_eq!(
            styled,
            [(Style::Heading, "## Title"), (Style::Quote, "> one\n> two")]
        );
        let rules: Vec<_> = marks
            .rules
            .iter()
            .map(|rule| &text[rule.range.clone()])
            .collect();
        assert_eq!(rules, ["---", "- ---"]);
        let [table] = marks.tables.as_slice() else {
            panic!("one table: {:?}", marks.tables);
        };
        assert_eq!(
            &text[table.mark.range.clone()],
            "| a | b |\n| :-- | --: |\n| 1 | 2 |"
        );
        assert_eq!(cell_texts(table), [["a", "b"], ["1", "2"]]);
        assert_eq!(table.align, [Align::Left, Align::Right]);
    }

    fn cell_texts(table: &Table) -> Vec<Vec<&str>> {
        table
            .rows
            .iter()
            .map(|row| row.iter().map(|cell| cell.text.as_ref()).collect())
            .collect()
    }

    /// rt draws a table through its Markdown renderer, so cells show bold, italic, code, links,
    /// and tags with their syntax hidden.
    #[gpui::test]
    fn table_cells_read_their_inline_markdown(cx: &mut TestAppContext) {
        use crate::rich_text::{Mark, RowLink};
        let note = "| **Name** | Notes |\n| --- | --- |\n| *it* `code` | see [[Target]] #tag |\n";
        let (marks, _) = parsed(note, cx);
        let [table] = marks.tables.as_slice() else {
            panic!("one table: {:?}", marks.tables);
        };
        assert_eq!(
            cell_texts(table),
            [["Name", "Notes"], ["it code", "see Target #tag"]]
        );
        let shown = |cell: &Rich| -> Vec<(String, Mark)> {
            cell.marks
                .iter()
                .map(|(range, mark)| (cell.text[range.clone()].to_owned(), *mark))
                .collect()
        };
        assert_eq!(
            shown(&table.rows[0][0]),
            [("Name".to_owned(), Mark::Strong)]
        );
        assert_eq!(
            shown(&table.rows[1][0]),
            [
                ("it".to_owned(), Mark::Emphasis),
                ("code".to_owned(), Mark::Code)
            ]
        );
        let links: Vec<_> = table.rows[1][1]
            .links
            .iter()
            .map(|(range, link)| (&table.rows[1][1].text[range.clone()], link.clone()))
            .collect();
        assert_eq!(
            links,
            [
                (
                    "Target",
                    RowLink::Reference(crate::references::Target::Page("Target".into()))
                ),
                ("#tag", RowLink::Tag("#tag".into())),
            ]
        );
    }

    /// Front matter is metadata, so its `---` fences never draw as rules, as in rt, which starts
    /// rules after `frontmatterLength`.
    #[gpui::test]
    fn front_matter_fences_are_never_rules(cx: &mut TestAppContext) {
        let note = "---\ntitle: Plan\ntags: [a]\n---\n\n- item\n\n---\n";
        let (marks, text) = parsed(note, cx);
        let rules: Vec<_> = marks.rules.iter().map(|rule| rule.range.start).collect();
        assert_eq!(
            rules,
            [text.rfind("---").unwrap()],
            "only the rule after the list"
        );
    }

    #[gpui::test]
    fn setext_underlines_stay_paragraph_text_and_rules(cx: &mut TestAppContext) {
        let note = "Title\n===\n\nOther\n---\n";
        let (marks, text) = parsed(note, cx);
        assert!(
            marks
                .styled
                .iter()
                .all(|(style, _)| *style != Style::Heading),
            "no setext heading: {:?}",
            marks.styled
        );
        let rules: Vec<_> = marks
            .rules
            .iter()
            .map(|rule| &text[rule.range.clone()])
            .collect();
        assert_eq!(
            rules,
            ["---"],
            "a dash line after a paragraph is a rule, as in rt"
        );
    }

    #[gpui::test]
    fn quoted_code_hides_only_the_quote_prefixes(cx: &mut TestAppContext) {
        let note = "> ```\n> >x\n> ```\n";
        let (marks, text) = parsed(note, cx);
        let at: Vec<_> = marks.hidden.iter().map(|mark| mark.range.start).collect();
        assert_eq!(hidden(&marks, &text), [">", ">", ">"]);
        assert_eq!(at, [0, 6, 11], "the code's own `>` stays");
    }

    #[test]
    fn urdu_lines_set_their_arabic_script_runs_in_nastaliq() {
        let text = "- English only\n- عربي فقط\n- یہ اردو ہے (Urdu)\u{200C}۔\n";
        let runs: Vec<_> = urdu_runs(text)
            .into_iter()
            .map(|range| &text[range])
            .collect();
        assert_eq!(runs, ["یہ", "اردو", "ہے", "\u{200C}۔"]);
    }

    #[test]
    fn a_highlight_font_family_replaces_the_run_family_and_keeps_fallbacks() {
        let base = gpui::TextStyle {
            font_family: "Editor".into(),
            font_fallbacks: Some(gpui::FontFallbacks::from_fonts(vec!["Naskh".into()])),
            ..Default::default()
        };
        let font = base.clone().highlight(urdu_style()).font();
        assert_eq!(font.family.as_ref(), crate::script_fonts::URDU);
        assert_eq!(font.fallbacks, base.font().fallbacks);
        let unchanged = base.clone().highlight(HighlightStyle::default()).font();
        assert_eq!(unchanged.family.as_ref(), "Editor");
    }
}
