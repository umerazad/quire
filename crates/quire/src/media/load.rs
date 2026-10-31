//! Fetching media bytes off the UI thread, and the window-independent cache of decoded images.
//! Local reads, downloads, and decoding all run on the background executor. Decoded images stay
//! cached up to a byte budget; evicting one also drops its texture from every window's atlas.

use futures::{AsyncReadExt as _, FutureExt as _, future::Shared};
use gpui::{App, AppContext as _, Global, RenderImage, Task};
use http_client::AsyncBody;
use quire_media::{
    Bitmap, MediaError,
    files::{Files, read_limited},
    images::{Limits, decode},
    source::{MediaSource, MediaTarget, decode_data},
};
use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

/// Decoded images kept for reuse across views and pages.
const CACHE_BYTES: usize = 192 * 1024 * 1024;

/// Notifies its observers when notebook files may have appeared: a folder notebook's files
/// changed, or a Quire notebook's import delivered attachments that arrived after their page.
#[derive(Default)]
pub(crate) struct Arrivals;

impl Global for Arrivals {}

/// Media that could not load a notebook file try again.
pub(crate) fn files_arrived(cx: &mut App) {
    cx.set_global(Arrivals);
}

/// Whether a view that failed to load `target` tries again when files arrive: only notebook
/// files can appear later; a remote or `data:` source fails the same way again.
pub(crate) fn waits_for_files(target: &Result<MediaTarget, MediaError>) -> bool {
    matches!(target, Ok(target) if matches!(target.source, MediaSource::Notebook(_)))
}

pub(crate) struct LoadedImage {
    pub image: Arc<RenderImage>,
    /// The picture's own size, before downscaling.
    pub natural: (u32, u32),
}

pub(crate) type ImageTask = Shared<Task<Result<Arc<LoadedImage>, MediaError>>>;

#[derive(Clone, PartialEq, Eq, Hash)]
struct Key {
    source: MediaSource,
    files: Option<Files>,
}

enum Entry {
    Loading(ImageTask),
    Ready(Arc<LoadedImage>, usize),
}

#[derive(Default)]
pub(crate) struct MediaCache {
    entries: HashMap<Key, Entry>,
    ready: VecDeque<Key>,
    bytes: usize,
}

impl Global for MediaCache {}

impl MediaCache {
    fn finish(key: Key, image: Option<Arc<LoadedImage>>, cx: &mut App) {
        let cache = cx.default_global::<MediaCache>();
        let Some(image) = image else {
            cache.entries.remove(&key);
            return;
        };
        let size = image.image.as_bytes(0).map_or(0, <[u8]>::len);
        cache.bytes += size;
        cache.ready.push_back(key.clone());
        cache.entries.insert(key, Entry::Ready(image, size));
        let mut dropped = Vec::new();
        while cache.bytes > CACHE_BYTES && cache.ready.len() > 1 {
            let Some(oldest) = cache.ready.pop_front() else {
                break;
            };
            if let Some(Entry::Ready(image, size)) = cache.entries.remove(&oldest) {
                cache.bytes -= size;
                dropped.push(image.image.clone());
            }
        }
        for image in dropped {
            cx.drop_image(image, None);
        }
    }

    /// Forgets a source, so the next load reads it again.
    pub(crate) fn forget(source: &MediaSource, files: Option<&Files>, cx: &mut App) {
        let key = Key {
            source: source.clone(),
            files: files.cloned(),
        };
        let cache = cx.default_global::<MediaCache>();
        if let Some(Entry::Ready(image, size)) = cache.entries.remove(&key) {
            cache.bytes -= size;
            cache.ready.retain(|ready| *ready != key);
            cx.drop_image(image.image.clone(), None);
        }
    }

    #[cfg(test)]
    pub(crate) fn cached_bytes(cx: &App) -> usize {
        cx.try_global::<MediaCache>().map_or(0, |cache| cache.bytes)
    }
}

/// The decoded image for a source, shared with every other view asking for it.
pub(crate) fn image(source: &MediaSource, files: Option<&Files>, cx: &mut App) -> ImageTask {
    let key = Key {
        source: source.clone(),
        files: files.cloned(),
    };
    let cache = cx.default_global::<MediaCache>();
    match cache.entries.get(&key) {
        Some(Entry::Ready(image, _)) => {
            let image = image.clone();
            cache.ready.retain(|ready| *ready != key);
            cache.ready.push_back(key);
            return Task::ready(Ok(image)).shared();
        }
        Some(Entry::Loading(task)) => return task.clone(),
        None => {}
    }
    let bytes = fetch(source, files, Limits::default().max_bytes, cx);
    let task = cx
        .spawn({
            let key = key.clone();
            async move |cx| {
                let loaded = async {
                    let bytes = bytes.await?;
                    let decoded = cx
                        .background_spawn(async move { decode(&bytes, &Limits::default()) })
                        .await?;
                    Ok(Arc::new(LoadedImage {
                        image: Arc::new(render_image(decoded.bitmap)?),
                        natural: decoded.natural,
                    }))
                }
                .await;
                cx.update(|cx| MediaCache::finish(key, loaded.as_ref().ok().cloned(), cx));
                loaded
            }
        })
        .shared();
    cx.default_global::<MediaCache>()
        .entries
        .insert(key, Entry::Loading(task.clone()));
    task
}

/// A GPUI image from a BGRA bitmap.
pub(crate) fn render_image(bitmap: Bitmap) -> Result<RenderImage, MediaError> {
    let (width, height) = (bitmap.width, bitmap.height);
    image::RgbaImage::from_raw(width, height, bitmap.bgra)
        .map(|buffer| RenderImage::new(vec![image::Frame::new(buffer)]))
        .ok_or_else(|| {
            MediaError::Failed(
                "A decoded picture had the wrong size. Retry, or open it in the system viewer."
                    .into(),
            )
        })
}

/// The bytes of a source, at most `limit` of them, read or downloaded in the background.
pub(crate) fn fetch(
    source: &MediaSource,
    files: Option<&Files>,
    limit: usize,
    cx: &App,
) -> Task<Result<Vec<u8>, MediaError>> {
    match source {
        MediaSource::Notebook(path) => {
            let Some(files) = files.cloned() else {
                return Task::ready(Err(no_notebook()));
            };
            let path = path.clone();
            cx.background_spawn(async move { read_limited(&files.locate(&path)?, limit) })
        }
        MediaSource::Data(uri) => {
            let uri = uri.clone();
            cx.background_spawn(async move { decode_data(&uri, limit) })
        }
        MediaSource::Remote(url) => {
            let request = cx.http_client().get(url, AsyncBody::default(), true);
            let url = url.clone();
            cx.background_spawn(async move {
                let unavailable = |reason: String| {
                    MediaError::Missing(format!(
                        "Could not download {url}: {reason}. Check the address and your connection, then retry."
                    ))
                };
                let response = request
                    .await
                    .map_err(|error| unavailable(format!("{error:#}")))?;
                if !response.status().is_success() {
                    return Err(unavailable(format!("HTTP {}", response.status())));
                }
                let declared = response
                    .headers()
                    .get("content-length")
                    .and_then(|value| value.to_str().ok()?.parse::<usize>().ok());
                if declared.is_some_and(|declared| declared > limit) {
                    return Err(quire_media::MediaError::Refused(format!(
                        "{url} is larger than the inline limit. Open it in the system viewer instead."
                    )));
                }
                let mut bytes = Vec::new();
                response
                    .into_body()
                    .take(limit as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .map_err(|error| unavailable(error.to_string()))?;
                if bytes.len() > limit {
                    return Err(quire_media::MediaError::Refused(format!(
                        "{url} is larger than the inline limit. Open it in the system viewer instead."
                    )));
                }
                Ok(bytes)
            })
        }
    }
}

fn no_notebook() -> MediaError {
    MediaError::Missing(
        "Attachments resolve inside a notebook. Open this page from its notebook, then retry."
            .into(),
    )
}

/// Opens a media target with the system's default application: a notebook file or a download in
/// place, an embedded `data:` file after writing it to a temporary file.
pub(crate) fn open_externally(
    target: &MediaTarget,
    files: Option<&Files>,
    cx: &mut App,
) -> Task<Result<(), MediaError>> {
    let file = match &target.source {
        MediaSource::Remote(url) => {
            cx.open_url(url);
            return Task::ready(Ok(()));
        }
        MediaSource::Notebook(path) => {
            let Some(files) = files.cloned() else {
                return Task::ready(Err(no_notebook()));
            };
            let path = path.clone();
            cx.background_spawn(async move { files.locate(&path) })
        }
        MediaSource::Data(uri) => {
            let uri = uri.clone();
            let kind = target.kind;
            cx.background_spawn(async move { write_temporary(&uri, kind) })
        }
    };
    cx.spawn(async move |cx| {
        let path = file.await?;
        let url = http_client::Url::from_file_path(&path).map_err(|_| {
            MediaError::Failed(format!(
                "Cannot open {}. Open it from the file manager instead.",
                path.display()
            ))
        })?;
        cx.update(|cx| cx.open_url(url.as_str()));
        Ok(())
    })
}

fn write_temporary(
    uri: &str,
    kind: quire_media::source::MediaKind,
) -> Result<std::path::PathBuf, MediaError> {
    use std::hash::{Hash as _, Hasher as _};
    let bytes = decode_data(uri, quire_media::pdf::MAX_UNTRUSTED_BYTES)?;
    let extension = match kind {
        quire_media::source::MediaKind::Pdf => "pdf",
        quire_media::source::MediaKind::Image => {
            let header = uri.get(..32).unwrap_or(uri).to_ascii_lowercase();
            ["png", "jpeg", "gif", "webp", "avif", "svg"]
                .into_iter()
                .find(|kind| header.starts_with(&format!("data:image/{kind}")))
                .unwrap_or("png")
        }
    };
    let mut hasher = std::hash::DefaultHasher::new();
    bytes.hash(&mut hasher);
    let directory = std::env::temp_dir().join("quire-media");
    let path = directory.join(format!("{:016x}.{extension}", hasher.finish()));
    std::fs::create_dir_all(&directory)
        .and_then(|()| std::fs::write(&path, &bytes))
        .map_err(|error| {
            MediaError::Failed(format!(
                "Cannot write a temporary copy to open: {error}. Free disk space, then retry."
            ))
        })?;
    Ok(path)
}
