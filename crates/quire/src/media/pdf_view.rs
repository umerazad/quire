//! One inline PDF: a scrolling list of rendered pages with zoom, `#page=N`, find, text
//! selection, and Open in system viewer. Only pages the list lays out are rendered, a few
//! rendered pages are kept, and evicted pages leave the texture atlas, so a long document costs
//! the same memory as a short one.

use super::load;
use editor::{Editor, EditorEvent};
use gpui::{
    App, Bounds, ClipboardItem, Context, Entity, FocusHandle, KeyBinding, ListAlignment,
    ListOffset, ListState, MouseButton, MouseDownEvent, MouseMoveEvent, Pixels, Point, Render,
    RenderImage, SharedString, Subscription, Task, Window, actions, canvas, list, prelude::*, px,
};
use quire_media::{
    MediaError,
    files::Files,
    pdf::{Document, PageText},
    source::{MediaSource, MediaTarget},
};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    ops::Range,
    rc::Rc,
    sync::Arc,
};
use ui::{Button, Clickable as _, LabelSize, prelude::*};

actions!(quire_pdf, [ZoomIn, ZoomOut, NextMatch, PreviousMatch]);

const CONTEXT: &str = "QuirePdf";
/// Rendered pages kept per document; more are rendered again when scrolled back to.
pub(crate) const KEPT_PAGES: usize = 6;
/// Height of the page viewport inside the note.
const VIEWPORT: f32 = 520.;
const TOOLBAR: f32 = 36.;
const PAGE_GAP: f32 = 8.;
const ZOOM_STEPS: [f32; 9] = [0.5, 0.75, 1., 1.25, 1.5, 2., 2.5, 3., 4.];

pub(crate) fn bind_keys(cx: &mut App) {
    let (copy, zoom_in, zoom_out) = if cfg!(target_os = "macos") {
        ("cmd-c", "cmd-=", "cmd--")
    } else {
        ("ctrl-c", "ctrl-=", "ctrl--")
    };
    cx.bind_keys([
        KeyBinding::new(copy, editor::actions::Copy, Some(CONTEXT)),
        KeyBinding::new(zoom_in, ZoomIn, Some(CONTEXT)),
        KeyBinding::new(zoom_out, ZoomOut, Some(CONTEXT)),
        KeyBinding::new("enter", NextMatch, Some("QuirePdfFind > Editor")),
        KeyBinding::new("shift-enter", PreviousMatch, Some("QuirePdfFind > Editor")),
    ]);
}

enum State {
    Loading,
    Open(Document),
    Failed(MediaError),
}

struct Rendered {
    width: u32,
    image: Arc<RenderImage>,
}

/// Selected characters, from where the drag started to where it is now, each as a page and a
/// character index; it can run across pages.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Selection {
    anchor: (usize, usize),
    head: (usize, usize),
}

impl Selection {
    /// The selected characters of `page`, out of its `length`.
    fn range(&self, page: usize, length: usize) -> Option<Range<usize>> {
        let (start, end) = (self.anchor.min(self.head), self.anchor.max(self.head));
        if page < start.0 || page > end.0 {
            return None;
        }
        let from = if page == start.0 { start.1 } else { 0 };
        let to = if page == end.0 { end.1 + 1 } else { length };
        Some(from..to.min(length).max(from))
    }

    fn pages(&self) -> std::ops::RangeInclusive<usize> {
        self.anchor.0.min(self.head.0)..=self.anchor.0.max(self.head.0)
    }
}

pub(crate) struct PdfView {
    target: Result<MediaTarget, MediaError>,
    files: Option<Files>,
    state: State,
    focus: FocusHandle,
    list: ListState,
    zoom: f32,
    /// The width pages fit at zoom 1, the note's column.
    width: Pixels,
    rendered: HashMap<usize, Rendered>,
    recent: VecDeque<usize>,
    rendering: HashMap<usize, (u32, Task<()>)>,
    texts: HashMap<usize, Arc<PageText>>,
    reading: Option<Task<()>>,
    find: Entity<Editor>,
    query: String,
    matches: Vec<(usize, Range<usize>)>,
    current: Option<usize>,
    selection: Option<Selection>,
    page_bounds: Rc<RefCell<HashMap<usize, Bounds<Pixels>>>>,
    message: Option<SharedString>,
    _open: Option<Task<()>>,
    _find: Subscription,
    _arrivals: Subscription,
}

/// What a view shows, for tests.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PdfShows {
    Loading,
    Open {
        engine: quire_media::pdf::Engine,
        pages: usize,
    },
    Placeholder(String),
}

impl PdfView {
    pub(crate) fn new(
        target: Result<MediaTarget, MediaError>,
        files: Option<Files>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let find = cx.new(|cx| {
            let mut find = Editor::single_line(window, cx);
            find.set_placeholder_text("Find in PDF", window, cx);
            find
        });
        let subscription = cx.subscribe(&find, |view, find, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) {
                let query = find.read(cx).text(cx);
                view.search(query, cx);
            }
        });
        let mut view = Self {
            target,
            files,
            state: State::Loading,
            focus: cx.focus_handle(),
            list: ListState::new(0, ListAlignment::Top, px(400.)),
            zoom: 1.,
            width: px(640.),
            rendered: HashMap::new(),
            recent: VecDeque::new(),
            rendering: HashMap::new(),
            texts: HashMap::new(),
            reading: None,
            find,
            query: String::new(),
            matches: Vec::new(),
            current: None,
            selection: None,
            page_bounds: Rc::default(),
            message: None,
            _open: None,
            _find: subscription,
            _arrivals: cx.observe_global::<load::Arrivals>(|view, cx| {
                if matches!(view.state, State::Failed(_)) && load::waits_for_files(&view.target) {
                    view.retry(cx);
                }
            }),
        };
        cx.on_release(|view, cx| {
            for (_, rendered) in view.rendered.drain() {
                cx.drop_image(rendered.image, None);
            }
        })
        .detach();
        view.open(cx);
        view
    }

    fn open(&mut self, cx: &mut Context<Self>) {
        let target = match &self.target {
            Ok(target) => target.clone(),
            Err(error) => {
                self.state = State::Failed(error.clone());
                return;
            }
        };
        self.state = State::Loading;
        let opening: Task<Result<Document, MediaError>> = match &target.source {
            MediaSource::Notebook(path) => match self.files.clone() {
                Some(files) => {
                    let path = path.clone();
                    cx.background_spawn(
                        async move { Document::open_stored(&files.locate(&path)?) },
                    )
                }
                None => Task::ready(Err(MediaError::Missing(
                    "Attachments resolve inside a notebook. Open this page from its notebook, then retry."
                        .into(),
                ))),
            },
            source => {
                let bytes = load::fetch(
                    source,
                    self.files.as_ref(),
                    quire_media::pdf::MAX_UNTRUSTED_BYTES,
                    cx,
                );
                cx.background_spawn(async move { Document::open_untrusted(bytes.await?) })
            }
        };
        self._open = Some(cx.spawn(async move |view, cx| {
            let opened = opening.await;
            view.update(cx, |view, cx| {
                match opened {
                    Ok(document) => {
                        let pages = document.info().pages.len();
                        view.list.reset(pages);
                        if let Some(page) = target.page {
                            view.list.scroll_to(ListOffset {
                                item_ix: (page as usize).clamp(1, pages.max(1)) - 1,
                                offset_in_item: px(0.),
                            });
                        }
                        view.state = State::Open(document);
                        view.read_texts(cx);
                    }
                    Err(error) => view.state = State::Failed(error),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn document(&self) -> Option<&Document> {
        match &self.state {
            State::Open(document) => Some(document),
            _ => None,
        }
    }

    pub(crate) fn width(&self) -> Pixels {
        self.width
    }

    pub(crate) fn set_width(&mut self, width: Pixels) {
        if width != self.width {
            self.width = width;
            self.relayout();
        }
    }

    /// The block height this view needs.
    pub(crate) fn height(&self) -> Pixels {
        match &self.state {
            State::Open(_) => px(TOOLBAR + VIEWPORT + 12.),
            State::Failed(_) => px(72.),
            State::Loading => px(28.),
        }
    }

    fn page_width(&self) -> Pixels {
        self.width * self.zoom
    }

    fn page_height(&self, page: usize) -> Pixels {
        let Some(size) = self
            .document()
            .and_then(|document| document.info().pages.get(page).copied())
            .filter(|size| size.width > 0.)
        else {
            return px(1.);
        };
        self.page_width() * (size.height / size.width)
    }

    fn relayout(&mut self) {
        if let Some(pages) = self.document().map(|document| document.info().pages.len()) {
            let top = self.list.logical_scroll_top();
            self.list.reset(pages);
            self.list.scroll_to(ListOffset {
                item_ix: top.item_ix,
                offset_in_item: px(0.),
            });
        }
    }

    pub(crate) fn set_zoom(&mut self, zoom: f32, cx: &mut Context<Self>) {
        let zoom = zoom.clamp(ZOOM_STEPS[0], ZOOM_STEPS[ZOOM_STEPS.len() - 1]);
        if zoom != self.zoom {
            self.zoom = zoom;
            self.relayout();
            cx.notify();
        }
    }

    fn step_zoom(&mut self, up: bool, cx: &mut Context<Self>) {
        let next = if up {
            ZOOM_STEPS
                .iter()
                .copied()
                .find(|step| *step > self.zoom + 0.01)
        } else {
            ZOOM_STEPS
                .iter()
                .rev()
                .copied()
                .find(|step| *step < self.zoom - 0.01)
        };
        if let Some(zoom) = next {
            self.set_zoom(zoom, cx);
        }
    }

    /// Renders `page` at the current zoom when it is not already, keeping [`KEPT_PAGES`].
    fn request(&mut self, page: usize, window: &mut Window, cx: &mut Context<Self>) {
        let width = (f32::from(self.page_width()) * window.scale_factor()).round() as u32;
        if self
            .rendered
            .get(&page)
            .is_some_and(|done| done.width == width)
            || self
                .rendering
                .get(&page)
                .is_some_and(|(asked, _)| *asked == width)
        {
            return;
        }
        let Some(document) = self.document() else {
            return;
        };
        let render = document.render(page, width);
        let task = cx.spawn(async move |view, cx| {
            let rendered = render.await.and_then(load::render_image).map(Arc::new);
            view.update(cx, |view, cx| {
                view.rendering.remove(&page);
                match rendered {
                    Ok(image) => view.keep(page, Rendered { width, image }, cx),
                    Err(error) => view.message = Some(error.message().to_owned().into()),
                }
                cx.notify();
            })
            .ok();
        });
        self.rendering.insert(page, (width, task));
    }

    fn keep(&mut self, page: usize, rendered: Rendered, cx: &mut Context<Self>) {
        if let Some(old) = self.rendered.insert(page, rendered) {
            cx.drop_image(old.image, None);
        }
        self.recent.retain(|recent| *recent != page);
        self.recent.push_back(page);
        while self.recent.len() > KEPT_PAGES {
            if let Some(oldest) = self.recent.pop_front()
                && let Some(old) = self.rendered.remove(&oldest)
            {
                cx.drop_image(old.image, None);
            }
        }
    }

    /// Reads every page's text in the background, for find.
    fn read_texts(&mut self, cx: &mut Context<Self>) {
        let Some(document) = self.document().cloned() else {
            return;
        };
        let pages = document.info().pages.len();
        self.reading = Some(cx.spawn(async move |view, cx| {
            for page in 0..pages {
                let Ok(Some(text)) = document.text(page).await else {
                    continue;
                };
                let stop = view
                    .update(cx, |view, cx| {
                        view.texts.insert(page, text);
                        view.refresh_matches(cx);
                    })
                    .is_err();
                if stop {
                    return;
                }
            }
        }));
    }

    fn search(&mut self, query: String, cx: &mut Context<Self>) {
        self.query = query;
        self.current = None;
        self.refresh_matches(cx);
        if !self.matches.is_empty() {
            self.go_to_match(0, cx);
        }
    }

    fn refresh_matches(&mut self, cx: &mut Context<Self>) {
        let mut pages: Vec<_> = self.texts.keys().copied().collect();
        pages.sort_unstable();
        self.matches = pages
            .into_iter()
            .flat_map(|page| {
                self.texts[&page]
                    .find(&self.query)
                    .into_iter()
                    .map(move |range| (page, range))
            })
            .collect();
        if self
            .current
            .is_some_and(|current| current >= self.matches.len())
        {
            self.current = None;
        }
        cx.notify();
    }

    fn go_to_match(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some((page, range)) = self.matches.get(index).cloned() else {
            return;
        };
        self.current = Some(index);
        let top = self.texts[&page]
            .boxes(range)
            .first()
            .map_or(0., |bounds| bounds.top);
        let scale = self.page_scale(page);
        self.list.scroll_to(ListOffset {
            item_ix: page,
            offset_in_item: (px(top) * scale - px(40.)).max(px(0.)),
        });
        cx.notify();
    }

    pub(crate) fn next_match(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.matches.len();
        if count == 0 {
            return;
        }
        let next = match (self.current, forward) {
            (None, true) => 0,
            (None, false) => count - 1,
            (Some(current), true) => (current + 1) % count,
            (Some(current), false) => (current + count - 1) % count,
        };
        self.go_to_match(next, cx);
    }

    /// Pixels per PDF point for a page at the current zoom.
    fn page_scale(&self, page: usize) -> f32 {
        self.document()
            .and_then(|document| document.info().pages.get(page).copied())
            .filter(|size| size.width > 0.)
            .map_or(1., |size| f32::from(self.page_width()) / size.width)
    }

    fn char_at(&self, page: usize, position: Point<Pixels>) -> Option<usize> {
        let bounds = *self.page_bounds.borrow().get(&page)?;
        let scale = self.page_scale(page);
        let x = f32::from(position.x - bounds.origin.x) / scale;
        let y = f32::from(position.y - bounds.origin.y) / scale;
        self.texts.get(&page)?.char_at(x, y)
    }

    /// Selects the characters from `anchor` through `head`, inclusive, each a page and a
    /// character index.
    pub(crate) fn select(
        &mut self,
        anchor: (usize, usize),
        head: (usize, usize),
        cx: &mut Context<Self>,
    ) {
        self.selection = Some(Selection { anchor, head });
        cx.notify();
    }

    /// The selected text, with a line break between pages. Pages whose text is not read yet
    /// are left out.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let selection = self.selection?;
        let parts: Vec<String> = selection
            .pages()
            .filter_map(|page| {
                let text = self.texts.get(&page)?;
                Some(text.text(selection.range(page, text.chars.len())?))
            })
            .collect();
        (!parts.is_empty()).then(|| parts.join("\n"))
    }

    fn copy(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selected_text() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    pub(crate) fn open_externally(&mut self, cx: &mut Context<Self>) {
        let Ok(target) = &self.target else {
            return;
        };
        let opened = load::open_externally(target, self.files.as_ref(), cx);
        cx.spawn(async move |view, cx| {
            if let Err(error) = opened.await {
                view.update(cx, |view, cx| {
                    view.message = Some(error.message().to_owned().into());
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        self.message = None;
        self.open(cx);
        cx.notify();
    }

    fn render_page(
        &mut self,
        page: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.request(page, window, cx);
        let width = self.page_width();
        let height = self.page_height(page);
        let scale = self.page_scale(page);
        let highlight = |bounds: quire_media::pdf::CharBox, color: gpui::Hsla| {
            div()
                .absolute()
                .left(px(bounds.left) * scale)
                .top(px(bounds.top) * scale)
                .w(px(bounds.right - bounds.left) * scale)
                .h(px(bounds.bottom - bounds.top) * scale)
                .bg(color)
        };
        let mut marks = Vec::new();
        if let Some(text) = self.texts.get(&page) {
            let colors = cx.theme().colors();
            for (index, (_, range)) in self
                .matches
                .iter()
                .enumerate()
                .filter(|(_, (on, _))| *on == page)
            {
                let color = if Some(index) == self.current {
                    gpui::hsla(0.08, 0.95, 0.55, 0.45)
                } else {
                    gpui::hsla(0.15, 0.95, 0.6, 0.35)
                };
                marks.extend(
                    text.boxes(range.clone())
                        .into_iter()
                        .map(|bounds| highlight(bounds, color)),
                );
            }
            if let Some(range) = self
                .selection
                .and_then(|selection| selection.range(page, text.chars.len()))
            {
                marks.extend(
                    text.boxes(range)
                        .into_iter()
                        .map(|bounds| highlight(bounds, colors.element_selection_background)),
                );
            }
        }
        let bounds_store = self.page_bounds.clone();
        div()
            .pb(px(PAGE_GAP))
            .child(
                div()
                    .id(("quire-pdf-page", page))
                    .debug_selector(move || format!("quire-pdf-page-{page}"))
                    .relative()
                    .w(width)
                    .h(height)
                    .bg(gpui::white())
                    .border_1()
                    .border_color(cx.theme().colors().border)
                    .cursor_text()
                    .when_some(self.rendered.get(&page), |page_div, rendered| {
                        page_div.child(gpui::img(rendered.image.clone()).size_full())
                    })
                    .children(marks)
                    .child(
                        canvas(
                            move |bounds, _, _| {
                                bounds_store.borrow_mut().insert(page, bounds);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .size_full(),
                    )
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            window.focus(&view.focus, cx);
                            match view.char_at(page, event.position) {
                                Some(at) => view.select((page, at), (page, at), cx),
                                None => {
                                    view.selection = None;
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .on_mouse_move(cx.listener(move |view, event: &MouseMoveEvent, _, cx| {
                        if event.pressed_button != Some(MouseButton::Left) {
                            return;
                        }
                        // A drag that started on another page extends onto this one.
                        let Some(selection) = view.selection else {
                            return;
                        };
                        if let Some(head) = view.char_at(page, event.position) {
                            view.select(selection.anchor, (page, head), cx);
                        }
                    })),
            )
            .into_any_element()
    }

    #[cfg(test)]
    pub(crate) fn shows(&self) -> PdfShows {
        match &self.state {
            State::Loading => PdfShows::Loading,
            State::Open(document) => PdfShows::Open {
                engine: document.info().engine,
                pages: document.info().pages.len(),
            },
            State::Failed(error) => PdfShows::Placeholder(error.message().to_owned()),
        }
    }

    #[cfg(test)]
    pub(crate) fn rendered_pages(&self) -> Vec<(usize, u32)> {
        let mut pages: Vec<_> = self
            .rendered
            .iter()
            .map(|(page, rendered)| (*page, rendered.width))
            .collect();
        pages.sort_unstable();
        pages
    }

    #[cfg(test)]
    pub(crate) fn rendered_images(&self) -> Vec<Arc<RenderImage>> {
        self.rendered
            .values()
            .map(|rendered| rendered.image.clone())
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn top_page(&self) -> usize {
        self.list.logical_scroll_top().item_ix
    }

    #[cfg(test)]
    pub(crate) fn matches(&self) -> (usize, Option<usize>) {
        (self.matches.len(), self.current)
    }

    #[cfg(test)]
    pub(crate) fn find_editor(&self) -> Entity<Editor> {
        self.find.clone()
    }

    #[cfg(test)]
    pub(crate) fn texts_read(&self) -> usize {
        self.texts.len()
    }

    #[cfg(test)]
    pub(crate) fn scroll_to_page(&mut self, page: usize, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset {
            item_ix: page,
            offset_in_item: px(0.),
        });
        cx.notify();
    }

    #[cfg(test)]
    pub(crate) fn zoom(&self) -> f32 {
        self.zoom
    }

    /// Where a character of a laid-out page is in the window, to aim the mouse at it.
    #[cfg(test)]
    pub(crate) fn char_center(&self, page: usize, index: usize) -> Option<Point<Pixels>> {
        let bounds = *self.page_bounds.borrow().get(&page)?;
        let char_box = (*self.texts.get(&page)?.boxes.get(index)?)?;
        let scale = self.page_scale(page);
        Some(gpui::point(
            bounds.origin.x + px((char_box.left + char_box.right) / 2.) * scale,
            bounds.origin.y + px((char_box.top + char_box.bottom) / 2.) * scale,
        ))
    }
}

impl gpui::Focusable for PdfView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PdfView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name: SharedString = self
            .target
            .as_ref()
            .map_or_else(|_| "PDF".to_owned(), |target| target.name.clone())
            .into();
        let colors = cx.theme().colors();
        let open_button = || {
            Button::new("open", "Open in system viewer")
                .label_size(LabelSize::Small)
                .on_click(cx.listener(|view, _, _, cx| view.open_externally(cx)))
        };
        let body = match &self.state {
            State::Loading => div()
                .text_color(colors.text_muted)
                .child(format!("Loading {name}…"))
                .into_any_element(),
            State::Failed(error) => v_flex()
                .debug_selector(|| "quire-media-placeholder".into())
                .gap_1()
                .p_2()
                .rounded_sm()
                .border_1()
                .border_color(colors.border)
                .child(
                    div()
                        .text_color(cx.theme().status().error)
                        .child(format!("{name}: {}", error.message())),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new("retry", "Retry")
                                .label_size(LabelSize::Small)
                                .on_click(cx.listener(|view, _, _, cx| view.retry(cx))),
                        )
                        .when(self.target.is_ok(), |row| row.child(open_button())),
                )
                .into_any_element(),
            State::Open(document) => {
                let info = document.info();
                let pages = info.pages.len();
                let top = self
                    .list
                    .logical_scroll_top()
                    .item_ix
                    .min(pages.saturating_sub(1));
                let found = match (self.query.is_empty(), self.matches.len(), self.current) {
                    (true, _, _) => String::new(),
                    (false, 0, _) => "No matches".into(),
                    (false, count, Some(current)) => format!("{} of {count}", current + 1),
                    (false, count, None) => format!("{count} matches"),
                };
                let toolbar = h_flex()
                    .h(px(TOOLBAR))
                    .gap_2()
                    .px_2()
                    .border_b_1()
                    .border_color(colors.border)
                    .child(div().flex_1().truncate().child(name.clone()))
                    .child(
                        div()
                            .text_color(colors.text_muted)
                            .child(format!("Page {} of {pages}", top + 1)),
                    )
                    .child(
                        Button::new("zoom-out", "−")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|view, _, _, cx| view.step_zoom(false, cx))),
                    )
                    .child(format!("{:.0}%", self.zoom * 100.))
                    .child(
                        Button::new("zoom-in", "+")
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|view, _, _, cx| view.step_zoom(true, cx))),
                    )
                    .child(
                        div()
                            .key_context("QuirePdfFind")
                            .w(px(160.))
                            .px_1()
                            .border_1()
                            .border_color(colors.border)
                            .rounded_sm()
                            .on_action(
                                cx.listener(|view, _: &NextMatch, _, cx| view.next_match(true, cx)),
                            )
                            .on_action(cx.listener(|view, _: &PreviousMatch, _, cx| {
                                view.next_match(false, cx)
                            }))
                            .child(self.find.clone()),
                    )
                    .child(div().text_color(colors.text_muted).child(found))
                    .child(open_button());
                let pages_list = list(self.list.clone(), cx.processor(Self::render_page))
                    .w(self.page_width())
                    .h(px(VIEWPORT));
                v_flex()
                    .debug_selector(|| "quire-pdf".into())
                    .border_1()
                    .border_color(colors.border)
                    .rounded_sm()
                    .child(toolbar)
                    .child(
                        div()
                            .id("quire-pdf-pages")
                            .h(px(VIEWPORT))
                            .overflow_x_scroll()
                            .bg(colors.editor_background)
                            // The pages took the scroll; the note around them stays put. At the
                            // first or last page the scroll carries on to the note.
                            .on_scroll(cx.listener(
                                move |view, event: &gpui::ScrollEvent, _, cx| {
                                    let up = match event.delta {
                                        gpui::ScrollDelta::Pixels(delta) => delta.y > px(0.),
                                        gpui::ScrollDelta::Lines(delta) => delta.y > 0.,
                                    };
                                    let top = view.list.logical_scroll_top();
                                    let at_end = if up {
                                        top.item_ix == 0 && top.offset_in_item <= px(0.)
                                    } else {
                                        top.item_ix + 1 >= pages
                                    };
                                    if !at_end {
                                        cx.stop_propagation();
                                    }
                                },
                            ))
                            .child(pages_list),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .track_focus(&self.focus)
            .key_context(CONTEXT)
            .on_action(cx.listener(|view, _: &editor::actions::Copy, _, cx| view.copy(cx)))
            .on_action(cx.listener(|view, _: &ZoomIn, _, cx| view.step_zoom(true, cx)))
            .on_action(cx.listener(|view, _: &ZoomOut, _, cx| view.step_zoom(false, cx)))
            .py_1()
            .child(body)
            .when_some(self.message.clone(), |column, message| {
                column.child(div().text_color(cx.theme().status().error).child(message))
            })
    }
}
