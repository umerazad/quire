//! One inline image: loading, the decoded picture fitted to the page column, or a placeholder
//! that says what failed and offers Retry and Open in system viewer.

use super::load::{self, LoadedImage};
use gpui::{Context, Pixels, Render, SharedString, Task, Window, prelude::*, px};
use quire_media::{MediaError, files::Files, source::MediaTarget};
use std::sync::Arc;
use ui::{Button, Clickable as _, LabelSize, prelude::*};

/// Tallest an inline image is shown, as the reference caps it near three quarters of a window.
pub(crate) const MAX_HEIGHT: f32 = 480.;

enum State {
    Loading,
    Ready(Arc<LoadedImage>),
    Failed(MediaError),
}

pub(crate) struct ImageView {
    target: Result<MediaTarget, MediaError>,
    files: Option<Files>,
    state: State,
    message: Option<SharedString>,
    /// The column width the picture fits into.
    width: Pixels,
    _task: Option<Task<()>>,
    _arrivals: gpui::Subscription,
}

/// What a view shows, for tests.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Shows {
    Loading,
    Picture { natural: (u32, u32) },
    Placeholder(String),
}

impl ImageView {
    pub(crate) fn new(
        target: Result<MediaTarget, MediaError>,
        files: Option<Files>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut view = Self {
            target,
            files,
            state: State::Loading,
            message: None,
            width: px(640.),
            _task: None,
            _arrivals: cx.observe_global::<load::Arrivals>(|view, cx| {
                if matches!(view.state, State::Failed(_)) && load::waits_for_files(&view.target) {
                    view.retry(cx);
                }
            }),
        };
        view.load(cx);
        view
    }

    fn load(&mut self, cx: &mut Context<Self>) {
        let target = match &self.target {
            Ok(target) => target,
            Err(error) => {
                self.state = State::Failed(error.clone());
                return;
            }
        };
        self.state = State::Loading;
        let task = load::image(&target.source, self.files.as_ref(), cx);
        self._task = Some(cx.spawn(async move |view, cx| {
            let result = task.await;
            view.update(cx, |view, cx| {
                view.state = match result {
                    Ok(image) => State::Ready(image),
                    Err(error) => State::Failed(error),
                };
                cx.notify();
            })
            .ok();
        }));
    }

    pub(crate) fn retry(&mut self, cx: &mut Context<Self>) {
        if let Ok(target) = &self.target {
            load::MediaCache::forget(&target.source, self.files.as_ref(), cx);
        }
        self.message = None;
        self.load(cx);
        cx.notify();
    }

    fn open(&mut self, cx: &mut Context<Self>) {
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

    /// The picture's shown size: its own size, shrunk to fit the column and
    /// [`MAX_HEIGHT`].
    fn shown_size(&self) -> Option<(Pixels, Pixels)> {
        let State::Ready(image) = &self.state else {
            return None;
        };
        let (w, h) = image.natural;
        let (w, h) = (w.max(1) as f32, h.max(1) as f32);
        let scale = (f32::from(self.width) / w).min(MAX_HEIGHT / h).min(1.);
        Some((px(w * scale), px(h * scale)))
    }

    pub(crate) fn set_width(&mut self, width: Pixels) {
        self.width = width;
    }

    /// The block height this view needs.
    pub(crate) fn height(&self) -> Pixels {
        match self.shown_size() {
            Some((_, height)) => height + px(8.),
            None => px(if matches!(self.state, State::Failed(_)) {
                72.
            } else {
                28.
            }),
        }
    }

    #[cfg(test)]
    pub(crate) fn shows(&self) -> Shows {
        match &self.state {
            State::Loading => Shows::Loading,
            State::Ready(image) => Shows::Picture {
                natural: image.natural,
            },
            State::Failed(error) => Shows::Placeholder(error.message().to_owned()),
        }
    }

    #[cfg(test)]
    pub(crate) fn image(&self) -> Option<Arc<gpui::RenderImage>> {
        match &self.state {
            State::Ready(image) => Some(image.image.clone()),
            _ => None,
        }
    }
}

impl Render for ImageView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let name: SharedString = self
            .target
            .as_ref()
            .map_or_else(|_| "Image".to_owned(), |target| target.name.clone())
            .into();
        let body = match &self.state {
            State::Loading => div()
                .text_color(cx.theme().colors().text_muted)
                .child(format!("Loading {name}…"))
                .into_any_element(),
            State::Ready(image) => {
                let (w, h) = self.shown_size().unwrap_or((px(1.), px(1.)));
                gpui::img(image.image.clone())
                    .debug_selector(|| "quire-image".into())
                    .w(w)
                    .h(h)
                    .max_w_full()
                    .object_fit(gpui::ObjectFit::Contain)
                    .rounded_sm()
                    .into_any_element()
            }
            State::Failed(error) => {
                let openable = self.target.is_ok();
                v_flex()
                    .debug_selector(|| "quire-media-placeholder".into())
                    .gap_1()
                    .p_2()
                    .rounded_sm()
                    .border_1()
                    .border_color(cx.theme().colors().border)
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
                            .when(openable, |row| {
                                row.child(
                                    Button::new("open", "Open in system viewer")
                                        .label_size(LabelSize::Small)
                                        .on_click(cx.listener(|view, _, _, cx| view.open(cx))),
                                )
                            }),
                    )
                    .into_any_element()
            }
        };
        v_flex()
            .py_1()
            .child(body)
            .when_some(self.message.clone(), |column, message| {
                column.child(div().text_color(cx.theme().status().error).child(message))
            })
    }
}
