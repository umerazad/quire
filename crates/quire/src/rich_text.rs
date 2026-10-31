//! Block text as result rows show it: the Markdown read by the same inline grammar the page
//! editor highlights with, links and tags found by the same `quire_core` rules the index uses,
//! drawn as one line of GPUI `StyledText` with emphasis, code, links, and tag tones, and with the
//! syntax that marks them hidden. Tag pages, linked references, task views, live task lists, and
//! search rows share it, so a block reads the same everywhere it is listed.

use crate::references::Target;
use gpui::{
    AnyElement, App, Context, ElementId, EventEmitter, FontStyle, FontWeight, Hsla,
    InteractiveElement as _, InteractiveText, IntoElement, ParentElement as _, SharedString,
    StrikethroughStyle, Styled as _, StyledText, TextRun, UnderlineStyle, Window, div, px, rgb,
};
use quire_core::markdown::{Reference, ReferenceKind, ReferenceSyntax, references, tags};
use quire_core::tag_tones::{NEUTRAL, TONES, tone_index};
use settings::Settings as _;
use std::{cell::RefCell, collections::HashMap, ops::Range, rc::Rc};
use theme::ActiveTheme as _;

/// How a stretch of row text is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mark {
    Strong,
    Emphasis,
    Strike,
    Code,
    Link,
    /// A tag in the tone with this index into `TONES`, or the neutral look past its end.
    Tag(usize),
    Faint,
}

/// What clicking a stretch of row text opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RowLink {
    Reference(Target),
    Tag(String),
    /// A web or mail address, opened by the system.
    Url(String),
}

impl RowLink {
    /// A Markdown link destination: an address with a scheme opens in the system, and any other
    /// names a page by its notebook path.
    fn of_destination(destination: String) -> Self {
        if destination.contains("://") || destination.starts_with("mailto:") {
            Self::Url(destination)
        } else {
            let path =
                quire_core::markdown::decode_uri_component(&destination).unwrap_or(destination);
            Self::Reference(Target::Page(path))
        }
    }

    /// Opens the link through `notebook`, as a page editor's link would open.
    pub fn open(
        &self,
        beside: bool,
        notebook: &dyn crate::references::Notebook,
        window: &mut Window,
        cx: &mut App,
    ) {
        match self {
            Self::Reference(target) => notebook.follow(
                crate::references::Follow {
                    target: target.clone(),
                    beside,
                    backlinks: false,
                },
                window,
                cx,
            ),
            Self::Tag(tag) => notebook.show_tag(tag, window, cx),
            Self::Url(url) => cx.open_url(url),
        }
    }
}

/// One block's text ready to draw: what shows, how each stretch looks, and what it opens.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Rich {
    pub text: SharedString,
    pub marks: Vec<(Range<usize>, Mark)>,
    pub links: Vec<(Range<usize>, RowLink)>,
    /// The block's task marker, when it has one: whether it is ticked.
    pub task: Option<bool>,
}

thread_local! {
    static PARSER: RefCell<Option<tree_sitter::Parser>> = const { RefCell::new(None) };
}

fn parse(text: &str) -> Option<tree_sitter::Tree> {
    PARSER.with_borrow_mut(|parser| {
        if parser.is_none() {
            let mut inline = tree_sitter::Parser::new();
            inline
                .set_language(&tree_sitter_quire_markdown::INLINE_LANGUAGE.into())
                .ok()?;
            *parser = Some(inline);
        }
        parser.as_mut()?.parse(text, None)
    })
}

fn nodes(tree: &tree_sitter::Tree) -> Vec<tree_sitter::Node<'_>> {
    let mut found = Vec::new();
    let mut stack = vec![tree.root_node()];
    while let Some(node) = stack.pop() {
        found.push(node);
        let mut cursor = node.walk();
        stack.extend(node.children(&mut cursor));
    }
    found.sort_by_key(|node| (node.start_byte(), std::cmp::Reverse(node.end_byte())));
    found
}

fn overlaps(ranges: &[Range<usize>], range: &Range<usize>) -> bool {
    ranges
        .iter()
        .any(|other| other.start < range.end && range.start < other.end)
}

fn children<'a>(node: &tree_sitter::Node<'a>, kind: &str) -> Vec<tree_sitter::Node<'a>> {
    let mut cursor = node.walk();
    node.children(&mut cursor)
        .filter(|child| child.kind() == kind)
        .collect()
}

/// Source edits and looks found in `body`, all in its byte offsets.
#[derive(Default)]
struct Found {
    hidden: Vec<Range<usize>>,
    marks: Vec<(Range<usize>, Mark)>,
    links: Vec<(Range<usize>, RowLink)>,
}

impl Found {
    /// Shows only `shown` of `whole`.
    fn keep(&mut self, whole: Range<usize>, shown: Range<usize>) {
        self.hidden.push(whole.start..shown.start);
        self.hidden.push(shown.end..whole.end);
    }

    fn syntax(&mut self, node: tree_sitter::Node, body: &str, references: &[Range<usize>]) {
        let range = node.byte_range();
        let linkish = matches!(
            node.kind(),
            "inline_link"
                | "full_reference_link"
                | "collapsed_reference_link"
                | "shortcut_link"
                | "image"
                | "uri_autolink"
                | "email_autolink"
        );
        if linkish && overlaps(references, &range) {
            return;
        }
        match node.kind() {
            "emphasis" | "strong_emphasis" | "strikethrough" => {
                let delimiters = children(&node, "emphasis_delimiter");
                if delimiters
                    .iter()
                    .any(|delimiter| overlaps(references, &delimiter.byte_range()))
                {
                    return;
                }
                self.hidden
                    .extend(delimiters.iter().map(|delimiter| delimiter.byte_range()));
                let mark = match node.kind() {
                    "emphasis" => Mark::Emphasis,
                    "strong_emphasis" => Mark::Strong,
                    _ => Mark::Strike,
                };
                self.marks.push((range, mark));
            }
            "code_span" => {
                let delimiters = children(&node, "code_span_delimiter");
                if let [open, close] = delimiters.as_slice() {
                    self.keep(range, open.end_byte()..close.start_byte());
                    self.marks
                        .push((open.end_byte()..close.start_byte(), Mark::Code));
                }
            }
            "inline_link"
            | "full_reference_link"
            | "collapsed_reference_link"
            | "shortcut_link" => {
                let shown = children(&node, "link_text")
                    .first()
                    .map(|text| text.byte_range())
                    .unwrap_or_else(|| {
                        range.start + 1..range.end.saturating_sub(1).max(range.start + 1)
                    });
                let destination = children(&node, "link_destination")
                    .first()
                    .map(|destination| body[destination.byte_range()].to_owned());
                self.keep(range, shown.clone());
                self.marks.push((shown.clone(), Mark::Link));
                if let Some(url) = destination {
                    self.links.push((shown, RowLink::of_destination(url)));
                }
            }
            "image" => {
                let shown = children(&node, "image_description")
                    .first()
                    .map(|text| text.byte_range())
                    .unwrap_or(range.start..range.start);
                self.keep(range, shown.clone());
                self.marks.push((shown, Mark::Faint));
            }
            "uri_autolink" | "email_autolink" if range.len() > 2 => {
                let shown = range.start + 1..range.end - 1;
                self.keep(range, shown.clone());
                self.marks.push((shown.clone(), Mark::Link));
                if node.kind() == "uri_autolink" {
                    self.links
                        .push((shown.clone(), RowLink::Url(body[shown].to_owned())));
                }
            }
            "backslash_escape" => self.hidden.push(range.start..range.start + 1),
            _ => {}
        }
    }

    /// A page or block link shows its label or target without brackets; Roam forms and embeds
    /// show as written.
    fn reference(&mut self, reference: &Reference, body: &str) {
        let range = reference.range.clone();
        let raw = &body[range.clone()];
        let shown = if reference.syntax == ReferenceSyntax::Wiki
            && reference.kind != ReferenceKind::Embed
            && raw.starts_with("[[")
            && raw.ends_with("]]")
        {
            let inner = range.start + 2..range.end - 2;
            let label = raw[2..raw.len() - 2]
                .find('|')
                .map(|bar| inner.start + bar + 1..inner.end)
                .filter(|label| !label.is_empty());
            let shown = label.unwrap_or(inner);
            self.keep(range, shown.clone());
            shown
        } else {
            range
        };
        self.marks.push((shown.clone(), Mark::Link));
        if let Some(target) = Target::of(reference) {
            self.links.push((shown, RowLink::Reference(target)));
        }
    }
}

/// Reads block text, as the index stores it, into what a row shows.
pub fn analyze(source: &str) -> Rich {
    let marker = quire_core::tasks::read_marker(source);
    let body = &source[marker.as_ref().map_or(0, |marker| marker.prefix_len)..];
    let task = marker.map(|marker| marker.checked);
    let Some(tree) = parse(body) else {
        return Rich {
            text: body.replace(['\n', '\t'], " ").into(),
            task,
            ..Rich::default()
        };
    };
    let nodes = nodes(&tree);
    let code: Vec<_> = nodes
        .iter()
        .filter(|node| node.kind() == "code_span")
        .map(|node| node.byte_range())
        .collect();
    let syntax: Vec<_> = nodes
        .iter()
        .filter(|node| {
            matches!(
                node.kind(),
                "code_span"
                    | "inline_link"
                    | "full_reference_link"
                    | "collapsed_reference_link"
                    | "shortcut_link"
                    | "image"
                    | "backslash_escape"
                    | "html_tag"
                    | "uri_autolink"
                    | "email_autolink"
            )
        })
        .map(|node| node.byte_range())
        .collect();
    let found_references = references(body, &code);
    let reference_ranges: Vec<_> = found_references
        .iter()
        .map(|reference| reference.range.clone())
        .collect();
    let mut found = Found::default();
    for node in &nodes {
        found.syntax(*node, body, &reference_ranges);
    }
    for reference in &found_references {
        found.reference(reference, body);
    }
    for tag in tags(body, &syntax, &found_references) {
        let tone = tone_index(&tag.text).unwrap_or(TONES.len());
        found.marks.push((tag.range.clone(), Mark::Tag(tone)));
        found.links.push((tag.range, RowLink::Tag(tag.text)));
    }
    build(body, found, task)
}

/// Applies the hidden ranges and maps every look and link into the shown text.
fn build(body: &str, found: Found, task: Option<bool>) -> Rich {
    let mut hidden: Vec<_> = found
        .hidden
        .into_iter()
        .filter(|range| range.start < range.end)
        .collect();
    hidden.sort_by_key(|range| range.start);
    let mut offsets = vec![0; body.len() + 1];
    let mut text = String::with_capacity(body.len());
    let mut position = 0;
    let mut hole = hidden.iter().peekable();
    while position < body.len() {
        offsets[position] = text.len();
        while hole.peek().is_some_and(|range| range.end <= position) {
            hole.next();
        }
        let ch = body[position..].chars().next().unwrap_or(' ');
        let width = ch.len_utf8();
        if !hole
            .peek()
            .is_some_and(|range| range.start <= position && position < range.end)
        {
            text.push(if matches!(ch, '\n' | '\r' | '\t') {
                ' '
            } else {
                ch
            });
        }
        offsets[position + 1..position + width].fill(text.len());
        position += width;
    }
    offsets[body.len()] = text.len();
    let map = |range: &Range<usize>| offsets[range.start]..offsets[range.end];
    let shown = |range: &Range<usize>| range.start < range.end;
    Rich {
        marks: found
            .marks
            .iter()
            .map(|(range, mark)| (map(range), *mark))
            .filter(|(range, _)| shown(range))
            .fold(Vec::new(), |mut marks, mark| {
                if !marks.contains(&mark) {
                    marks.push(mark);
                }
                marks
            }),
        links: found
            .links
            .into_iter()
            .map(|(range, link)| (map(&range), link))
            .filter(|(range, _)| shown(range))
            .collect(),
        text: text.into(),
        task,
    }
}

/// Analyzed rows by their text, so a list redrawn on every frame parses each block once.
#[derive(Default)]
pub struct RichCache(RefCell<HashMap<String, Rich>>);

impl RichCache {
    const LIMIT: usize = 4096;

    pub fn get(&self, source: &str) -> Rich {
        let mut cache = self.0.borrow_mut();
        if let Some(rich) = cache.get(source) {
            return rich.clone();
        }
        if cache.len() >= Self::LIMIT {
            cache.clear();
        }
        let rich = analyze(source);
        cache.insert(source.to_owned(), rich.clone());
        rich
    }
}

/// Called with the clicked link and whether Shift was held.
pub type OnLink = Rc<dyn Fn(&RowLink, bool, &mut Window, &mut App)>;

/// Link clicks that `view` emits as `event`, for the window that owns the view to open.
pub fn emitter<V: EventEmitter<E>, E: 'static>(
    cx: &Context<V>,
    event: fn(RowLink, bool) -> E,
) -> OnLink {
    let view = cx.weak_entity();
    Rc::new(move |link, beside, _, cx| {
        view.update(cx, |_, cx| cx.emit(event(link.clone(), beside)))
            .ok();
    })
}

fn tone_colors(tone: usize, cx: &App) -> (Hsla, Hsla) {
    let tone = TONES.get(tone).unwrap_or(&NEUTRAL);
    let (ink, background) = if cx.theme().appearance.is_light() {
        tone.light
    } else {
        tone.dark
    };
    (rgb(ink).into(), rgb(background).into())
}

/// The text runs that draw `rich`, in `color` where no mark says otherwise.
pub fn runs(rich: &Rich, color: Hsla, cx: &App) -> Vec<TextRun> {
    let settings = theme_settings::ThemeSettings::get_global(cx);
    let base = settings.ui_font.clone();
    let code = settings.buffer_font.clone();
    let colors = cx.theme().colors();
    let mut cuts: Vec<usize> = rich
        .marks
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .chain([0, rich.text.len()])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    cuts.windows(2)
        .filter(|cut| cut[0] < cut[1])
        .map(|cut| {
            let mut run = TextRun {
                len: cut[1] - cut[0],
                font: base.clone(),
                color,
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            for (_, mark) in rich
                .marks
                .iter()
                .filter(|(range, _)| range.start <= cut[0] && cut[1] <= range.end)
            {
                match mark {
                    Mark::Strong => run.font.weight = FontWeight::BOLD,
                    Mark::Emphasis => run.font.style = FontStyle::Italic,
                    Mark::Strike => {
                        run.strikethrough = Some(StrikethroughStyle {
                            thickness: px(1.),
                            color: None,
                        })
                    }
                    Mark::Code => {
                        run.font = gpui::Font {
                            weight: run.font.weight,
                            style: run.font.style,
                            ..code.clone()
                        };
                        run.background_color = Some(colors.element_background);
                    }
                    Mark::Link => {
                        run.color = colors.text_accent;
                        run.underline = Some(UnderlineStyle {
                            thickness: px(1.),
                            color: Some(colors.text_accent.opacity(0.4)),
                            wavy: false,
                        });
                    }
                    Mark::Tag(tone) => {
                        let (ink, background) = tone_colors(*tone, cx);
                        run.color = ink;
                        run.background_color = Some(background);
                    }
                    Mark::Faint => run.color = colors.text_muted,
                }
            }
            run
        })
        .collect()
}

/// One line of row text, truncated with an ellipsis; links open through `on_link` and stop the
/// row's own click.
pub fn line(
    id: (&'static str, usize),
    rich: &Rich,
    muted: bool,
    on_link: Option<OnLink>,
    cx: &App,
) -> AnyElement {
    let colors = cx.theme().colors();
    let color = if muted {
        colors.text_muted
    } else {
        colors.text
    };
    let text = StyledText::new(rich.text.clone()).with_runs(runs(rich, color, cx));
    let content = match on_link.filter(|_| !rich.links.is_empty()) {
        Some(on_link) => {
            let (ranges, links): (Vec<_>, Vec<_>) = rich.links.iter().cloned().unzip();
            InteractiveText::new(ElementId::from(id), text)
                .on_click(ranges, move |index, window, cx| {
                    cx.stop_propagation();
                    if let Some(link) = links.get(index) {
                        on_link(link, window.modifiers().shift, window, cx);
                    }
                })
                .into_any_element()
        }
        None => text.into_any_element(),
    };
    div()
        .min_w_0()
        .truncate()
        .debug_selector(|| format!("{}-{}", id.0, id.1))
        .child(content)
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(rich: &Rich, mark: Mark) -> Vec<&str> {
        rich.marks
            .iter()
            .filter(|(_, found)| *found == mark)
            .map(|(range, _)| &rich.text[range.clone()])
            .collect()
    }

    #[test]
    fn rows_hide_markup_and_keep_what_it_marks() {
        let rich = analyze("[x] **Ship** the *beta* ~~soon~~ with `cargo run` \\*now\\*");
        assert_eq!(rich.text, "Ship the beta soon with cargo run *now*");
        assert_eq!(rich.task, Some(true));
        assert_eq!(shown(&rich, Mark::Strong), ["Ship"]);
        assert_eq!(shown(&rich, Mark::Emphasis), ["beta"]);
        assert_eq!(shown(&rich, Mark::Strike), ["soon"]);
        assert_eq!(shown(&rich, Mark::Code), ["cargo run"]);
    }

    #[test]
    fn links_show_their_label_and_open_their_target() {
        let rich = analyze(
            "{{[[TODO]]}} Met [[Ana]] about [[Launch Plan|the launch]], see [docs](https://x.y) and ((b-1)) <https://a.b>",
        );
        assert_eq!(rich.task, Some(false));
        assert_eq!(
            rich.text,
            "Met Ana about the launch, see docs and ((b-1)) https://a.b"
        );
        assert_eq!(
            shown(&rich, Mark::Link),
            ["docs", "https://a.b", "Ana", "the launch", "((b-1))"]
        );
        let links: Vec<_> = rich
            .links
            .iter()
            .map(|(range, link)| (&rich.text[range.clone()], link.clone()))
            .collect();
        assert!(links.contains(&("Ana", RowLink::Reference(Target::Page("Ana".into())))));
        assert!(links.contains(&(
            "the launch",
            RowLink::Reference(Target::Page("Launch Plan".into()))
        )));
        assert!(links.contains(&("((b-1))", RowLink::Reference(Target::Block("b-1".into())))));
        assert!(links.contains(&("docs", RowLink::Url("https://x.y".into()))));
        let relative = analyze("[notes](Work/Some%20Page.md)");
        assert_eq!(
            relative.links,
            [(
                0..5,
                RowLink::Reference(Target::Page("Work/Some Page.md".into()))
            )],
            "a Markdown link without a scheme names a page by path"
        );
    }

    #[test]
    fn tags_take_their_tone_outside_code_and_links() {
        let rich = analyze("#next call #Book/fiction and #plan, not `#code` or [#x](u)\nmore");
        assert_eq!(
            rich.text,
            "#next call #Book/fiction and #plan, not #code or #x more"
        );
        let tagged: Vec<_> = rich
            .marks
            .iter()
            .filter_map(|(range, mark)| match mark {
                Mark::Tag(tone) => Some((&rich.text[range.clone()], *tone)),
                _ => None,
            })
            .collect();
        assert_eq!(
            tagged,
            [
                ("#next", tone_index("next").unwrap()),
                ("#Book/fiction", tone_index("book").unwrap()),
                ("#plan", TONES.len()),
            ]
        );
        assert!(rich.links.contains(&(0..5, RowLink::Tag("#next".into()))));
    }

    #[test]
    fn plain_text_passes_through_and_the_cache_reuses_answers() {
        let cache = RichCache::default();
        let rich = cache.get("just words, ünïcode");
        assert_eq!(rich.text, "just words, ünïcode");
        assert!(rich.marks.is_empty() && rich.links.is_empty() && rich.task.is_none());
        assert_eq!(cache.get("just words, ünïcode"), rich);
    }
}
