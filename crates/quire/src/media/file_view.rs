//! A wiki embed of a file that is neither an image nor a PDF, such as `![[report.zip]]`: a button
//! that opens the file with the system's default application, as the reference's file attachment
//! does. The file is found when the button is pressed, so a file synced in later opens too.

use gpui::{Context, Pixels, SharedString, Task, Window, prelude::*, px};
use quire_media::{
    MediaError,
    files::Files,
    source::{MediaSource, media_source},
};
use ui::{
    Button, ButtonCommon as _, Clickable as _, Color, IconName, Label, LabelCommon as _, LabelSize,
    v_flex,
};

/// Whether a wiki embed names a file to open rather than a page: it ends in an extension other
/// than `.md`, as the reference decides.
pub fn is_file(target: &str) -> bool {
    let name = target.rsplit('/').next().unwrap_or(target);
    name.rsplit_once('.').is_some_and(|(stem, extension)| {
        !stem.is_empty()
            && !extension.is_empty()
            && extension.bytes().all(|byte| byte.is_ascii_alphanumeric())
            && !extension.eq_ignore_ascii_case("md")
    })
}

pub struct FileView {
    target: SharedString,
    source: Result<MediaSource, MediaError>,
    files: Option<Files>,
    error: Option<SharedString>,
    opening: Option<Task<()>>,
}

impl FileView {
    pub fn new(target: &str, page: Option<&str>, files: Option<Files>) -> Self {
        Self {
            target: target.to_owned().into(),
            source: media_source(target, page),
            files,
            error: None,
            opening: None,
        }
    }

    pub fn height(&self) -> Pixels {
        match self.error {
            Some(_) => px(56.),
            None => px(32.),
        }
    }

    #[cfg(test)]
    pub fn error(&self) -> Option<&SharedString> {
        self.error.as_ref()
    }

    /// Opens the file with the system's default application, or says why it cannot.
    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.error = None;
        let located = match (&self.source, &self.files) {
            (Ok(MediaSource::Remote(url)), _) => {
                cx.open_url(url);
                cx.notify();
                return;
            }
            (Ok(MediaSource::Notebook(path)), Some(files)) => {
                let (files, path) = (files.clone(), path.clone());
                cx.background_spawn(async move { files.locate(&path) })
            }
            (Ok(_), None) => Task::ready(Err(MediaError::Missing(
                "Files open from inside a notebook. Open this page from its notebook, then retry."
                    .into(),
            ))),
            (Ok(MediaSource::Data(_)), Some(_)) | (Err(_), _) => {
                Task::ready(Err(self.source.clone().err().unwrap_or_else(|| {
                    MediaError::Refused(
                        "This embedded file cannot be opened. Link to a file inside the notebook."
                            .into(),
                    )
                })))
            }
        };
        self.opening = Some(cx.spawn(async move |this, cx| {
            let opened = located.await.and_then(|path| {
                http_client::Url::from_file_path(&path).map_err(|_| {
                    MediaError::Failed(format!(
                        "Cannot open {}. Open it from the file manager instead.",
                        path.display()
                    ))
                })
            });
            this.update(cx, |this, cx| {
                this.opening = None;
                match opened {
                    Ok(url) => cx.open_url(url.as_str()),
                    Err(error) => this.error = Some(error.message().to_owned().into()),
                }
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }
}

impl Render for FileView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .debug_selector(|| "quire-file-embed".into())
            .items_start()
            .child(
                Button::new("open-file", self.target.clone())
                    .start_icon(ui::Icon::new(IconName::Link).size(ui::IconSize::Small))
                    .end_icon(ui::Icon::new(IconName::ArrowUpRight).size(ui::IconSize::XSmall))
                    .tooltip(ui::Tooltip::text("Open with the default application"))
                    .on_click(cx.listener(|this, _, _, cx| this.open(cx))),
            )
            .children(
                self.error
                    .clone()
                    .map(|error| Label::new(error).size(LabelSize::Small).color(Color::Error)),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::is_file;

    #[test]
    fn file_embeds_end_in_an_extension_other_than_markdown() {
        for file in [
            "report.zip",
            "Scans/2024/receipt.DOCX",
            "attachments/ab12-notes.txt",
        ] {
            assert!(is_file(file), "{file}");
        }
        for page in [
            "Plans",
            "Plans.md",
            "v1.2 notes",
            ".hidden",
            "Projects/Garden",
        ] {
            assert!(!is_file(page), "{page}");
        }
    }
}
