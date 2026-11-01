//! The README rendering showcase: the reference sample page in a buffer of its own. It belongs to
//! no notebook, so it never reaches a notebook's storage, index, queries, backups, or exports.
//! Edits last until Quire quits, and Reset sample restores the original as one undo step.
//!
//! The sample's references, embeds, and tags resolve through a notebook of its own, as the
//! reference's sample does: an empty folder in the temporary folder whose index sees only the
//! sample's text, so `((readme-shared))` and its embed show the sample's block, a reference jumps
//! within the sample, and a tag opens a tag page of the sample's blocks.

use crate::{
    editor_host::NotebookWindow,
    navigation::{
        Jump,
        tag_page::{TagPageEvent, TagPageView},
    },
    notebook_backend::NotebookBackend,
    notebook_session::OpenPage,
    page_analysis::PageAnalysis,
    query_index::{IndexChanged, IndexSync},
    references::{Follow, Notebook, ReferenceHost, Target},
};
use editor::{SelectionEffects, scroll::Autoscroll};
use gpui::{
    App, Context, Entity, Focusable as _, Global, Subscription, WeakEntity, Window, WindowOptions,
    actions, prelude::*, px,
};
use multi_buffer::MultiBufferOffset;
use std::{cell::RefCell, path::PathBuf, rc::Rc};
use theme::ActiveTheme as _;
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, IconButton, IconName, IconSize, Label,
    LabelCommon as _, LabelSize, Tooltip, h_flex, v_flex,
};

pub const SAMPLE: &str = include_str!("showcase/README.md");

/// Where the sample sits in its own notebook.
const SAMPLE_PATH: &str = "README.md";

actions!(
    quire_showcase,
    [
        /// Opens README.md, the rendering showcase, beside your notebooks.
        OpenShowcase,
        /// Restores the README sample to its original text.
        ResetShowcase,
    ]
);

/// The sample's page for this run of Quire, shared by every showcase window.
struct Sample {
    page: Entity<PageAnalysis>,
    /// None when the sample's own notebook could not start; the sample still shows, unlinked.
    links: Option<Links>,
}
impl Global for Sample {}

/// What the sample's references resolve through.
struct Links {
    host: Entity<ReferenceHost>,
    views: Views,
    _open: Entity<OpenPage>,
    _index: Entity<IndexSync>,
    _changed: Subscription,
    _scratch: Scratch,
}

type Views = Rc<RefCell<Vec<WeakEntity<ShowcaseWindow>>>>;

/// The sample notebook's folders in the temporary folder, removed when the sample goes.
struct Scratch(PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

pub fn init(cx: &mut App) {
    cx.on_action(|_: &OpenShowcase, cx| open(cx));
}

fn global(cx: &mut App) -> &Sample {
    if !cx.has_global::<Sample>() {
        let buffer = crate::markdown::buffer(SAMPLE, cx);
        let page = cx.new(|cx| PageAnalysis::new(buffer, cx));
        let links = link(&page, cx)
            .map_err(|error| crate::outline::show_notice(error, cx))
            .ok();
        cx.set_global(Sample { page, links });
    }
    cx.global::<Sample>()
}

pub(crate) fn sample(cx: &mut App) -> Entity<PageAnalysis> {
    global(cx).page.clone()
}

/// Starts the sample's own notebook: an empty folder whose index sees the sample's text as an
/// open page, so nothing in it ever reaches a real notebook.
fn link(page: &Entity<PageAnalysis>, cx: &mut App) -> Result<Links, String> {
    let scratch = Scratch(
        std::env::temp_dir().join(format!("quire-readme-{}", uuid::Uuid::new_v4().simple())),
    );
    let (root, state) = (scratch.0.join("notebook"), scratch.0.join("state"));
    std::fs::create_dir_all(&root).map_err(|error| {
        format!(
            "README.md links are off: cannot create {}: {error}. Free space in the temporary folder, then restart Quire.",
            root.display()
        )
    })?;
    let unavailable =
        |error: String| format!("README.md links are off: {error} Restart Quire to retry.");
    let folder = quire_store::folder_notebook::FolderWorker::spawn_folder(
        root.clone(),
        state.clone(),
        None,
        quire_store::worker::QueueLimits::default(),
    )
    .map_err(|error| unavailable(format!("{error:#}")))?;
    let calendar = quire_core::daily::DailyCalendar::utc();
    let index = IndexSync::spawn_folder(
        root,
        state,
        crate::query_index::config(Default::default(), calendar),
        cx,
    )
    .map_err(unavailable)?;
    let backend = NotebookBackend::Folder(folder.client);
    let open = cx.new(|cx| {
        OpenPage::in_memory(
            quire_store::sync::format::page_id_for_path(SAMPLE_PATH),
            SAMPLE_PATH.into(),
            page.clone(),
            backend.clone(),
            cx,
        )
    });
    index.update(cx, |index, cx| index.track(open.clone(), cx));
    let views: Views = Rc::default();
    let notebook = SampleNotebook {
        open: open.downgrade(),
        backend,
        views: views.clone(),
    };
    let client = index.read(cx).client().clone();
    let host = cx.new(|cx| {
        let mut host = ReferenceHost::new(Rc::new(notebook));
        host.changed(Some(client), cx);
        host
    });
    let changed = cx.subscribe(&index, {
        let host = host.downgrade();
        let views = views.clone();
        move |index, _: &IndexChanged, cx| {
            let client = index.read(cx).client().clone();
            host.update(cx, |host, cx| host.changed(Some(client), cx))
                .ok();
            for view in live(&views) {
                view.update(cx, |view, cx| view.refresh_tag(cx));
            }
        }
    });
    Ok(Links {
        host,
        views,
        _open: open,
        _index: index,
        _changed: changed,
        _scratch: scratch,
    })
}

fn live(views: &Views) -> Vec<Entity<ShowcaseWindow>> {
    views.borrow_mut().retain(|view| view.upgrade().is_some());
    views
        .borrow()
        .iter()
        .filter_map(WeakEntity::upgrade)
        .collect()
}

/// The sample's notebook: every page link and reference stays inside the sample.
struct SampleNotebook {
    open: WeakEntity<OpenPage>,
    /// The sample notebook's empty folder, which answers writes made beside the sample.
    backend: NotebookBackend,
    views: Views,
}

impl SampleNotebook {
    /// Runs `act` on the showcase view the reader is using in `window`, or else on any open one,
    /// after the editor that asked has finished its update.
    fn on_view(
        &self,
        window: &mut Window,
        cx: &mut App,
        act: impl FnOnce(&mut ShowcaseWindow, &mut Window, &mut Context<ShowcaseWindow>) + 'static,
    ) {
        let views = self.views.clone();
        window.defer(cx, move |window, cx| {
            let views = live(&views);
            let view = views
                .iter()
                .find(|view| view.read(cx).contains_focus(window, cx))
                .or(views.first())
                .cloned();
            if let Some(view) = view {
                view.update(cx, |view, cx| act(view, window, cx));
            }
        });
    }

    fn reveal(&self, jump: Jump, window: &mut Window, cx: &mut App) {
        self.on_view(window, cx, move |view, window, cx| {
            view.reveal(jump, window, cx)
        });
    }
}

impl Notebook for SampleNotebook {
    fn follow(&self, follow: Follow, window: &mut Window, cx: &mut App) {
        let jump = match follow.target {
            Target::Block(id) => Jump::block(id.rsplit("#^").next().unwrap_or(&id)),
            Target::Page(_) => Jump {
                block: None,
                from: Some(0),
            },
        };
        self.reveal(jump, window, cx);
    }

    fn open_page(&self, page: &str, _: &mut Window, cx: &mut App) -> Option<Entity<OpenPage>> {
        self.open
            .upgrade()
            .filter(|open| open.read(cx).id() == page)
    }

    fn page_id(&self, buffer: &Entity<language::Buffer>, cx: &App) -> Option<String> {
        let open = self.open.upgrade()?;
        let open = open.read(cx);
        (open.loaded()?.analysis.read(cx).source == *buffer).then(|| open.id().to_owned())
    }

    fn page_path(&self, buffer: &Entity<language::Buffer>, cx: &App) -> Option<String> {
        self.page_id(buffer, cx).map(|_| SAMPLE_PATH.to_owned())
    }

    fn show_page(&self, _: &str, _: bool, window: &mut Window, cx: &mut App) {
        self.reveal(
            Jump {
                block: None,
                from: Some(0),
            },
            window,
            cx,
        );
    }

    fn show_page_at(&self, _: &str, jump: Jump, _: bool, window: &mut Window, cx: &mut App) {
        self.reveal(jump, window, cx);
    }

    fn writers(&self, _: &App) -> Option<(Vec<Entity<OpenPage>>, NotebookBackend)> {
        Some((vec![self.open.upgrade()?], self.backend.clone()))
    }

    fn prompt(
        &self,
        _: crate::templates::prompt::Prompt,
        _: &mut Window,
        _: &mut App,
    ) -> futures::channel::oneshot::Receiver<Option<String>> {
        futures::channel::oneshot::channel().1
    }

    fn show_tag(&self, tag: &str, window: &mut Window, cx: &mut App) {
        let tag = tag.to_owned();
        self.on_view(window, cx, move |view, window, cx| {
            view.show_tag(tag, window, cx)
        });
    }
}

/// Replaces the sample's text with the original, as one undo step.
pub fn reset(cx: &mut App) {
    let buffer = sample(cx).read(cx).source.clone();
    buffer.update(cx, |buffer, cx| {
        buffer.finalize_last_transaction();
        buffer.edit([(0..buffer.len(), SAMPLE)], None, cx);
        buffer.finalize_last_transaction();
    });
}

pub fn open(cx: &mut App) {
    let open = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<ShowcaseWindow>());
    if let Some(window) = open {
        window
            .update(cx, |_, window, _| window.activate_window())
            .ok();
        return;
    }
    let page = sample(cx);
    let options = WindowOptions {
        window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
            None,
            gpui::size(px(1100.), px(800.)),
            cx,
        ))),
        window_min_size: Some(crate::title_bar::MIN_SIZE),
        ..crate::title_bar::base_options()
    };
    let opened = cx.open_window(options, |window, cx| {
        window.set_window_title("README.md");
        cx.new(|cx| ShowcaseWindow::new(page, false, window, cx))
    });
    if let Err(error) = opened {
        crate::outline::show_notice(
            format!("Cannot open README.md: {error}. Check desktop access, then retry."),
            cx,
        );
    }
}

/// Asks the window holding a showcase pane to close it.
pub struct CloseShowcase;

/// README.md in a window of its own, or, with `pane`, beside a notebook's pages as the reference
/// shows it.
pub struct ShowcaseWindow {
    pub notebook: Entity<NotebookWindow>,
    pane: bool,
    /// The sample tag page shown in place of the sample, as the reference's sample shows one.
    tag: Option<Entity<TagPageView>>,
    _tag_events: Option<Subscription>,
}

impl gpui::EventEmitter<CloseShowcase> for ShowcaseWindow {}

impl ShowcaseWindow {
    pub fn new(
        page: Entity<PageAnalysis>,
        pane: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let notebook = cx.new(|cx| NotebookWindow::for_page(page, None, window, cx));
        notebook.update(cx, |view, cx| view.show_written(window, cx));
        let links = global(cx)
            .links
            .as_ref()
            .map(|links| (links.host.clone(), links.views.clone()));
        if let Some((host, views)) = links {
            views.borrow_mut().push(cx.entity().downgrade());
            let editors = notebook.read(cx).editors().map(Entity::clone);
            for editor in editors {
                crate::references::install(&editor, &host, window, cx);
            }
        }
        Self {
            notebook,
            pane,
            tag: None,
            _tag_events: None,
        }
    }

    fn contains_focus(&self, window: &Window, cx: &App) -> bool {
        self.notebook
            .read(cx)
            .editors()
            .iter()
            .any(|editor| editor.focus_handle(cx).contains_focused(window, cx))
            || self
                .tag
                .as_ref()
                .is_some_and(|tag| tag.focus_handle(cx).contains_focused(window, cx))
    }

    /// Shows the sample again with the caret where `jump` says, centered.
    fn reveal(&mut self, jump: Jump, window: &mut Window, cx: &mut Context<Self>) {
        self.close_tag(cx);
        let analysis = sample(cx);
        let offset = jump
            .block
            .as_ref()
            .and_then(|id| {
                analysis
                    .read(cx)
                    .current(cx)?
                    .blocks
                    .iter()
                    .find(|block| block.id.as_ref() == Some(id))
                    .map(|block| block.content_from)
            })
            .or(jump.from.map(|from| from as usize))
            .unwrap_or(0)
            .min(analysis.read(cx).source.read(cx).len());
        let [source, second] = self.notebook.read(cx).editors().map(Entity::clone);
        let editor = if source.focus_handle(cx).contains_focused(window, cx) {
            source
        } else {
            second
        };
        editor.update(cx, |editor, cx| {
            editor.change_selections(
                SelectionEffects::scroll(Autoscroll::center()),
                window,
                cx,
                |selections| {
                    selections.select_ranges([MultiBufferOffset(offset)..MultiBufferOffset(offset)])
                },
            )
        });
        window.focus(&editor.focus_handle(cx), cx);
    }

    fn show_tag(&mut self, tag: String, window: &mut Window, cx: &mut Context<Self>) {
        let page = cx.new(|cx| TagPageView::new(tag, Default::default(), window, cx));
        self._tag_events = Some(cx.subscribe_in(
            &page,
            window,
            |this, _, event: &TagPageEvent, window, cx| {
                if let TagPageEvent::Open {
                    jump: Some(jump), ..
                } = event
                {
                    this.reveal(jump.clone(), window, cx);
                }
            },
        ));
        window.focus(&page.focus_handle(cx), cx);
        self.tag = Some(page);
        self.refresh_tag(cx);
    }

    fn refresh_tag(&mut self, cx: &mut Context<Self>) {
        let Some(tag) = self.tag.clone() else {
            return;
        };
        let host = global(cx).links.as_ref().map(|links| links.host.clone());
        let Some(client) = host.and_then(|host| host.read(cx).index().cloned()) else {
            return;
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis() as i64);
        let today = quire_core::daily::DailyCalendar::system()
            .and_then(|calendar| calendar.date_at(now))
            .unwrap_or_default();
        tag.update(cx, |tag, cx| tag.refresh(&client, today, cx));
    }

    fn close_tag(&mut self, cx: &mut Context<Self>) {
        if self.tag.take().is_some() {
            self._tag_events = None;
            cx.notify();
        }
    }

    #[cfg(test)]
    pub(crate) fn shown_tag(&self) -> Option<&Entity<TagPageView>> {
        self.tag.as_ref()
    }
}

impl Render for ShowcaseWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font = crate::app_menus::follow_zoom(window, cx);
        let colors = cx.theme().colors();
        let body = match &self.tag {
            Some(tag) => v_flex()
                .size_full()
                .child(
                    h_flex()
                        .px_3()
                        .gap_3()
                        .child(
                            Button::new("readme-back", "← README.md")
                                .on_click(cx.listener(|this, _, _, cx| this.close_tag(cx))),
                        )
                        .child(
                            Label::new("Sample tags only")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                )
                .child(gpui::div().flex_1().min_h_0().child(tag.clone()))
                .into_any_element(),
            None => self.notebook.clone().into_any_element(),
        };
        v_flex()
            .font(ui_font)
            .size_full()
            .bg(colors.background)
            .text_color(colors.text)
            .on_action(|_: &ResetShowcase, _, cx| reset(cx))
            .child(
                h_flex()
                    .px_3()
                    .py_1()
                    .gap_3()
                    .justify_between()
                    .child(Label::new("README.md"))
                    .child(
                        Label::new(
                            "Rendering showcase · Sample edits stay here and reset when the app restarts.",
                        )
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .child(
                                Button::new("reset-sample", "Reset sample")
                                    .on_click(|_, _, cx| reset(cx)),
                            )
                            .when(self.pane, |tools| {
                                tools.child(
                                    IconButton::new("close-showcase", IconName::Close)
                                        .icon_size(IconSize::Small)
                                        .tooltip(Tooltip::text("Close README.md"))
                                        .on_click(
                                            cx.listener(|_, _, _, cx| cx.emit(CloseShowcase)),
                                        ),
                                )
                            }),
                    ),
            )
            .child(gpui::div().flex_1().min_h_0().child(body))
            .child(
                h_flex().px_3().py_1().child(
                    Label::new("Sample only · Your notebooks are separate")
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
            )
    }
}

#[cfg(test)]
mod tests;
