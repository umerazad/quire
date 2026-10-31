//! Export as PDF and Export as Word for the shown page (reference `usePageExport` in
//! `src/ui/PageExport.tsx`): the page is flattened, the save dialog names the file, the AI CLI
//! formats it in its own folder while the status line counts seconds and steps with Cancel, and
//! the checked file is saved. A file that appeared at the chosen path during the run is replaced
//! only after the writer agrees.

use super::{
    StoredWindow,
    exchange::{EXPORT_CANCELED, notebook_file},
};
use crate::{
    ai::{
        process::{Host, Transcript, Update},
        settings::{AiSettings, is_on},
    },
    document_export::{ExportPagePdf, ExportPageWord, Job, Produced, Saved, produce, save},
    exchange::{Exchange, flatten_page},
};
use futures::{FutureExt as _, StreamExt as _, channel::mpsc};
use gpui::{AppContext as _, AsyncWindowContext, Context, Entity, Window};
use quire_core::{
    ai::{
        backend::AI_OFF,
        export::{EXPORT_OFF, Format, export_hint},
    },
    exchange::page_export::{export_name, localize_attachments},
};
use quire_media::files::Files;
use settings::Settings as _;
use std::{path::PathBuf, time::Duration};

/// What the status line says while the CLI works, as the reference panel words it.
fn running_label(backend: &str, title: &str, format: Format, seconds: u64, steps: usize) -> String {
    let steps = match steps {
        0 => String::new(),
        1 => " · 1 step".into(),
        steps => format!(" · {steps} steps"),
    };
    format!(
        "{backend} is formatting {title} as {}… {seconds} s{steps}",
        format.name()
    )
}

/// Everything an export reads from the window before it leaves the UI thread.
struct Started {
    page: String,
    open: Vec<Entity<crate::notebook_session::OpenPage>>,
    storage: crate::notebook_backend::NotebookBackend,
    index: quire_store::index::IndexClient,
    files: Option<Files>,
    host: Host,
    job: Job,
    exchange: Entity<Exchange>,
}

impl StoredWindow {
    /// Where AI runs keep their transcripts: beside the notebook's Quire data.
    fn ai_transcripts(&self) -> PathBuf {
        match self.session.borrow().folder() {
            Some((_, state)) => state.join("ai"),
            None => self.nav.database.with_file_name("quire-ai"),
        }
    }

    pub(super) fn export_document(
        &mut self,
        format: Format,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.exchange_busy(cx) {
            return;
        }
        if !is_on(cx) {
            self.nav.error =
                Some(format!("{AI_OFF} PDF and Word export use the AI command line tool.").into());
            cx.notify();
            return;
        }
        let Some(page) = self.shown_page(cx) else {
            return;
        };
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Export needs the notebook index, which is still starting. Export again in a moment."
                    .into(),
            );
            cx.notify();
            return;
        };
        let settings = AiSettings::get_global(cx).clone();
        let session = self.session.borrow();
        let started = Started {
            open: session.open_pages(),
            storage: session.storage(),
            files: session.media_files(),
            index,
            host: cx.global::<Host>().clone(),
            job: Job {
                backend: settings.backend,
                binary: settings.binary(settings.backend).to_owned(),
                model: settings.model.clone(),
                format,
                title: String::new(),
                text: String::new(),
                today: self.today().unwrap_or_default(),
                transcript: Some(Transcript {
                    folder: self.ai_transcripts(),
                    id: format!("export-{}", uuid::Uuid::new_v4().simple()),
                    page: page.clone(),
                }),
            },
            page,
            exchange: self.exchange.clone(),
        };
        drop(session);
        let exchange = self.exchange.clone();
        exchange.update(cx, |exchange, cx| {
            exchange.start("Exporting the page…", None, cx)
        });
        cx.spawn_in(window, async move |_, cx| {
            let result = export(started, cx).await;
            exchange.update(cx, |exchange, cx| match result {
                Ok(None) => {
                    *exchange = Exchange::Idle;
                    cx.notify();
                }
                Ok(Some(done)) => exchange.finish(Ok(done), cx),
                Err(error) => exchange.finish(Err(error), cx),
            });
        })
        .detach();
    }

    pub(super) fn register_document_export(div: gpui::Div, cx: &mut Context<Self>) -> gpui::Div {
        use gpui::InteractiveElement as _;
        div.on_action(cx.listener(|this, _: &ExportPagePdf, window, cx| {
            this.export_document(Format::Pdf, window, cx)
        }))
        .on_action(cx.listener(|this, _: &ExportPageWord, window, cx| {
            this.export_document(Format::Docx, window, cx)
        }))
    }

    /// Export as PDF and Export as Word, named for the chosen backend, off while AI is off.
    pub(super) fn render_document_export_buttons(&self, cx: &mut Context<Self>) -> [ui::Button; 2] {
        use ui::{ButtonCommon as _, Clickable as _, Disableable as _, Tooltip};
        let on = is_on(cx);
        let backend = AiSettings::get_global(cx).backend;
        let hint: gpui::SharedString = if on {
            export_hint(backend).into()
        } else {
            EXPORT_OFF.into()
        };
        Format::ALL.map(|format| {
            ui::Button::new(
                match format {
                    Format::Pdf => "export-page-pdf",
                    Format::Docx => "export-page-word",
                },
                format!("Export as {} with {}…", format.name(), backend.name()),
            )
            .disabled(!on)
            .tooltip(Tooltip::text(hint.clone()))
            .on_click(
                cx.listener(move |this, _, window, cx| this.export_document(format, window, cx)),
            )
        })
    }
}

async fn export(
    started: Started,
    cx: &mut AsyncWindowContext,
) -> Result<Option<(String, Option<PathBuf>)>, String> {
    let Started {
        page,
        open,
        storage,
        index,
        files,
        host,
        mut job,
        exchange,
    } = started;
    let format = job.format;
    let (title, text) = flatten_page(page, open, storage, index, cx).await?;
    let directory = util::paths::home_dir().clone();
    let chosen = cx
        .update(|_, cx| {
            cx.prompt_for_new_path(&directory, Some(&export_name(&title, format.extension())))
        })
        .map_err(|_| EXPORT_CANCELED.to_owned())?
        .await
        .map_err(|_| "The save dialog closed unexpectedly. Export again.".to_owned())?
        .map_err(|error| format!("{error:#}"))?;
    let Some(target) = chosen else {
        return Ok(None);
    };
    // The save dialog asked before letting the writer pick an existing file.
    let replace = std::fs::symlink_metadata(&target).is_ok();
    job.text = cx
        .background_spawn(async move {
            localize_attachments(&text, |relative| {
                files
                    .as_ref()
                    .and_then(|files| notebook_file(files, relative))
            })
        })
        .await;
    job.title = title.clone();
    let backend = job.backend.name();
    let (stop, stopped) = futures::channel::oneshot::channel();
    exchange.update(cx, |exchange, cx| {
        exchange.start_stoppable(running_label(backend, &title, format, 0, 0), stop, cx)
    });
    let (updates, mut received) = mpsc::unbounded();
    let running = cx.background_spawn(async move {
        produce(&host, job, stopped.map(|_| ()), move |update| {
            updates.unbounded_send(update).ok();
        })
        .await
    });
    let began = cx.background_executor().now();
    let mut steps = 0;
    let second = |cx: &mut AsyncWindowContext| {
        cx.background_executor()
            .timer(Duration::from_secs(1))
            .fuse()
    };
    let mut tick = second(cx);
    loop {
        futures::select_biased! {
            update = received.next() => match update {
                Some(Update::Tool { .. }) => steps += 1,
                Some(_) => continue,
                None => break,
            },
            _ = tick => tick = second(cx),
        }
        let label = running_label(
            backend,
            &title,
            format,
            (cx.background_executor().now() - began).as_secs(),
            steps,
        );
        exchange.update(cx, |exchange, cx| exchange.progress(label, true, cx));
    }
    let bytes = match running.await {
        Produced::File(bytes) => bytes,
        Produced::Failed(message) => return Err(message),
        Produced::Canceled => return Err(EXPORT_CANCELED.into()),
    };
    exchange.update(cx, |exchange, cx| {
        exchange.progress(format!("Saving {}…", target.display()), false, cx)
    });
    let written = target.clone();
    let saved = cx
        .background_spawn(
            async move { save(&written, &bytes, replace).map(|saved| (saved, bytes)) },
        )
        .await?;
    if let (Saved::Exists, bytes) = saved {
        let name = target
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let folder = target
            .parent()
            .map(|folder| folder.display().to_string())
            .unwrap_or_default();
        let answer = cx
            .update(|window, cx| {
                window.prompt(
                    gpui::PromptLevel::Warning,
                    &format!("Replace {name} in {folder}?"),
                    Some(
                        "A file with this name appeared there while the page was being formatted.",
                    ),
                    &["Replace", "Cancel"],
                    cx,
                )
            })
            .map_err(|_| EXPORT_CANCELED.to_owned())?;
        if answer.await != Ok(0) {
            return Err(EXPORT_CANCELED.into());
        }
        let written = target.clone();
        cx.background_spawn(async move { save(&written, &bytes, true) })
            .await?;
    }
    Ok(Some((
        format!("Exported {title} as {}.", format.name()),
        Some(target),
    )))
}
