//! Adding attachments to a page: the Attach file command, pasting an image, and dropping files.
//! In a Quire notebook each file is streamed into the store's durable attachment cache under its
//! content name, as the reference replica does; in a folder notebook it is copied into the
//! notebook's `attachments` folder as `<8 hex>-<name>`, as the reference vault does, and linked by
//! a path from the page's folder. The page gets one link per file in a single edit, so one undo
//! removes them. The links follow the reference: `![[...]]` for PDFs, `![](<...>)` for
//! images, and `[name](<...>)` for anything else.

use crate::notebook_backend::NotebookBackend;
use editor::Editor;
use futures::{FutureExt as _, future::LocalBoxFuture};
use gpui::{
    App, AppContext as _, ClipboardEntry, Context, Entity, ExternalPaths, KeyBinding,
    PathPromptOptions, PromptLevel, Task, WeakEntity, Window, actions,
};
use quire_core::exchange::file_links::root_attachment_target;
use quire_media::{
    files::Files,
    source::{MediaKind, media_kind},
};
use quire_store::folder_attachment::{self, AttachData};
use std::path::{Path, PathBuf};

actions!(
    quire,
    [
        /// Chooses files and attaches them to the page at the caret.
        AttachFile
    ]
);

pub(crate) fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        editor
            .register_action({
                let handle = handle.clone();
                move |_: &AttachFile, window, cx| {
                    if !choose(handle.clone(), window, cx) {
                        cx.propagate();
                    }
                }
            })
            .detach();
        editor
            .register_action(move |_: &editor::actions::Paste, window, cx| {
                if !paste_image(handle.clone(), window, cx) {
                    cx.propagate();
                }
            })
            .detach();
    })
    .detach();
    cx.bind_keys([KeyBinding::new(
        if cfg!(target_os = "macos") {
            "cmd-alt-a"
        } else {
            "ctrl-alt-a"
        },
        AttachFile,
        Some("Editor && !QuireControl"),
    )]);
}

/// The Markdown link for an attachment reference, as the reference inserts it.
pub fn link_for(reference: &str) -> String {
    let escaped = reference.replace('>', "%3E");
    match media_kind(reference) {
        Some(MediaKind::Pdf) => format!("![[{reference}]]"),
        Some(MediaKind::Image) => format!("![](<{escaped}>)"),
        None => format!(
            "[{}](<{escaped}>)",
            reference.rsplit('/').next().unwrap_or(reference)
        ),
    }
}

/// What a file is attached as.
enum Source {
    File(PathBuf),
    Bytes { data: Vec<u8>, name: String },
}

/// Where attached files go.
enum Destination {
    /// A Quire notebook's store, which keeps them under content names.
    Store(quire_store::worker::StorageClient),
    /// A folder notebook's `attachments` folder, linked from the page at `page`.
    Folder { root: PathBuf, page: String },
}

fn destination(editor: &Editor, cx: &App) -> Result<Destination, String> {
    let host = crate::references::host(editor).ok_or_else(|| {
        "Attachments need a notebook. Open this page from its notebook, then retry.".to_owned()
    })?;
    let notebook = host.read(cx).notebook().clone();
    let (_, backend) = notebook.writers(cx).ok_or_else(|| {
        "The notebook is closing. Reopen it, then attach the file again.".to_owned()
    })?;
    match backend {
        NotebookBackend::Store(client) => Ok(Destination::Store(client)),
        NotebookBackend::Folder(_) => {
            let page = editor
                .buffer()
                .read(cx)
                .all_buffers_iter()
                .next()
                .and_then(|buffer| notebook.page_path(&buffer, cx));
            match (notebook.media_files(), page) {
                (Some(Files::Folder { root }), Some(page)) => Ok(Destination::Folder { root, page }),
                _ => Err(
                    "This page is not open from its notebook folder. Reopen the page, then attach the file again."
                        .into(),
                ),
            }
        }
    }
}

/// Copies one file into a folder notebook and returns the link target from `page`.
fn attach_to_folder(root: &Path, page: &str, file: Source) -> Result<String, String> {
    let prefix = uuid::Uuid::new_v4().simple().to_string()[..8].to_owned();
    let stored = match &file {
        Source::File(path) => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            folder_attachment::attach_to_folder(root, AttachData::File(path), &name, &prefix)
        }
        Source::Bytes { data, name } => {
            folder_attachment::attach_to_folder(root, AttachData::Bytes(data), name, &prefix)
        }
    }
    .map_err(|error| format!("{error:#}"))?;
    let name = stored.trim_start_matches("attachments/");
    Ok(root_attachment_target(page, name))
}

fn choose(editor: WeakEntity<Editor>, window: &mut Window, cx: &mut App) -> bool {
    let Some(entity) = editor.upgrade() else {
        return false;
    };
    if crate::references::host(entity.read(cx)).is_none() {
        return false;
    }
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some("Attach".into()),
    });
    window
        .spawn(cx, async move |cx| {
            let Ok(Ok(Some(paths))) = chosen.await else {
                return;
            };
            cx.update(|window, cx| {
                attach(
                    &entity,
                    paths.into_iter().map(Source::File).collect(),
                    window,
                    cx,
                )
                .detach();
            })
            .ok();
        })
        .detach();
    true
}

/// Attaches an image on the clipboard instead of pasting text; declines otherwise.
fn paste_image(editor: WeakEntity<Editor>, window: &mut Window, cx: &mut App) -> bool {
    let Some(entity) = editor.upgrade() else {
        return false;
    };
    if crate::references::host(entity.read(cx)).is_none() {
        return false;
    }
    let Some(image) = cx.read_from_clipboard().and_then(|item| {
        item.into_entries().find_map(|entry| match entry {
            ClipboardEntry::Image(image) => Some(image),
            _ => None,
        })
    }) else {
        return false;
    };
    let extension = match image.format {
        gpui::ImageFormat::Png => "png",
        gpui::ImageFormat::Jpeg => "jpg",
        gpui::ImageFormat::Webp => "webp",
        gpui::ImageFormat::Gif => "gif",
        gpui::ImageFormat::Svg => "svg",
        gpui::ImageFormat::Bmp => "bmp",
        gpui::ImageFormat::Tiff => "tiff",
        gpui::ImageFormat::Ico => "ico",
        gpui::ImageFormat::Pnm => "pnm",
    };
    attach(
        &entity,
        vec![Source::Bytes {
            data: image.bytes,
            name: format!("pasted.{extension}"),
        }],
        window,
        cx,
    )
    .detach();
    true
}

/// The files last dragged over a page editor, kept across frames until they drop.
#[derive(Default)]
struct Dragged(std::cell::RefCell<Option<ExternalPaths>>);

impl editor::Addon for Dragged {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// While files are dragged over the window, a layer over a page editor that accepts them. Drag
/// moves record the files, and the drop is taken by position in the capture phase, because the
/// editor's own mouse handling sits above an ordinary drop target.
pub(crate) fn drop_target(editor: &Entity<Editor>, cx: &App) -> Option<gpui::AnyElement> {
    use gpui::{DispatchPhase, MouseUpEvent, prelude::*};
    use theme::ActiveTheme as _;
    if !cx.has_active_drag() || crate::references::host(editor.read(cx)).is_none() {
        return None;
    }
    let highlight = cx.theme().colors().drop_target_background;
    let moved = editor.downgrade();
    let dropped = editor.downgrade();
    Some(
        gpui::div()
            .absolute()
            .inset_0()
            .on_drag_move::<ExternalPaths>(move |event, _, cx| {
                let inside = event.bounds.contains(&event.event.position);
                let paths = inside.then(|| event.drag(cx).clone());
                moved
                    .update(cx, |editor, _| {
                        if editor.addon::<Dragged>().is_none() {
                            editor.register_addon(Dragged::default());
                        }
                        if let Some(dragged) = editor.addon::<Dragged>() {
                            *dragged.0.borrow_mut() = paths;
                        }
                    })
                    .ok();
            })
            .child(
                gpui::canvas(
                    |bounds, _, _| bounds,
                    move |_, bounds, window, _| {
                        if bounds.contains(&window.mouse_position()) {
                            window.paint_quad(gpui::fill(bounds, highlight));
                        }
                        let dropped = dropped.clone();
                        window.on_mouse_event(move |event: &MouseUpEvent, phase, window, cx| {
                            if phase != DispatchPhase::Capture || !bounds.contains(&event.position)
                            {
                                return;
                            }
                            let Some(editor) = dropped.upgrade() else {
                                return;
                            };
                            let Some(paths) = editor
                                .read(cx)
                                .addon::<Dragged>()
                                .and_then(|dragged| dragged.0.borrow_mut().take())
                            else {
                                return;
                            };
                            cx.stop_active_drag(window);
                            cx.stop_propagation();
                            drop_paths(&editor, &paths, window, cx);
                        });
                    },
                )
                .size_full(),
            )
            .into_any_element(),
    )
}

/// Attaches dropped files to `editor`'s page at its caret.
fn drop_paths(editor: &Entity<Editor>, paths: &ExternalPaths, window: &mut Window, cx: &mut App) {
    let files = paths
        .paths()
        .iter()
        .filter(|path| path.is_file())
        .cloned()
        .map(Source::File)
        .collect::<Vec<_>>();
    if !files.is_empty() {
        attach(editor, files, window, cx).detach();
    }
}

/// Writes each file to the store, then inserts their links at the caret in one edit.
fn attach(
    editor: &Entity<Editor>,
    files: Vec<Source>,
    window: &mut Window,
    cx: &mut App,
) -> Task<()> {
    let destination = match destination(editor.read(cx), cx) {
        Ok(destination) => destination,
        Err(message) => {
            report(message, window, cx);
            return Task::ready(());
        }
    };
    let requests: Vec<LocalBoxFuture<'static, Result<String, String>>> = files
        .into_iter()
        .map(|file| match &destination {
            Destination::Store(client) => {
                let request = match file {
                    Source::File(path) => {
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_default();
                        client.attach_file(path, name)
                    }
                    Source::Bytes { data, name } => client.attach(data, name),
                };
                crate::query_index::ask(request).boxed_local()
            }
            Destination::Folder { root, page } => {
                let (root, page) = (root.clone(), page.clone());
                cx.background_spawn(async move { attach_to_folder(&root, &page, file) })
                    .boxed_local()
            }
        })
        .collect();
    let editor = editor.downgrade();
    window.spawn(cx, async move |cx| {
        let mut links = Vec::new();
        let mut failure = None;
        for request in requests {
            match request.await {
                Ok(reference) => links.push(link_for(&reference)),
                Err(error) => {
                    failure = Some(format!("Could not attach the file: {error}"));
                }
            }
        }
        cx.update(|window, cx| {
            if !links.is_empty() {
                editor
                    .update(cx, |editor, cx| {
                        insert(editor, &links.join(" "), window, cx)
                    })
                    .ok();
            }
            if let Some(message) = failure {
                report(message, window, cx);
            }
        })
        .ok();
    })
}

fn insert(editor: &mut Editor, text: &str, window: &mut Window, cx: &mut Context<Editor>) {
    if editor.read_only(cx) {
        return;
    }
    editor.transact(window, cx, |editor, window, cx| {
        editor.insert(text, window, cx);
    });
}

fn report(message: String, window: &mut Window, cx: &mut App) {
    drop(window.prompt(PromptLevel::Warning, &message, None, &["OK"], cx));
}

#[cfg(test)]
mod tests {
    #[test]
    fn links_follow_the_reference_forms() {
        assert_eq!(
            super::link_for("attachments/abc.pdf"),
            "![[attachments/abc.pdf]]"
        );
        assert_eq!(
            super::link_for("attachments/abc.png"),
            "![](<attachments/abc.png>)"
        );
        assert_eq!(
            super::link_for("attachments/abc.zip"),
            "[abc.zip](<attachments/abc.zip>)"
        );
        assert_eq!(super::link_for("a>b.png"), "![](<a%3Eb.png>)");
    }
}
