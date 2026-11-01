//! Zed's file watcher over a whole folder tree on every OS. FSEvents (macOS) and
//! ReadDirectoryChangesW (Windows) watch a folder and everything below it from one registration.
//! inotify (Linux) watches one directory, so there Zed's worktree registers every directory it
//! scans and each one that appears later; this does the same for a notebook or synced folder.
//! Dot directories are skipped, as every caller ignores them.

use ::fs::{Fs as _, PathEvent, PathEventKind, RealFs, Watcher};
use futures::{Stream, StreamExt as _};
use gpui::BackgroundExecutor;
use std::{path::Path, pin::Pin, sync::Arc, time::Duration};

/// Whether the platform watcher covers one directory per registration.
pub const PER_DIRECTORY: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

pub type Events = Pin<Box<dyn Send + Stream<Item = Vec<PathEvent>>>>;

/// Watches `root` and everything below it, batching events `latency` apart.
pub async fn watch(
    root: &Path,
    latency: Duration,
    executor: BackgroundExecutor,
) -> (Events, Arc<dyn Watcher>) {
    let fs = RealFs::new(None, executor.clone());
    let (events, watcher) = fs.watch(root, latency).await;
    if !PER_DIRECTORY {
        return (events, watcher);
    }
    let root = root.to_owned();
    let scanning = watcher.clone();
    executor
        .spawn(async move { add_tree(&*scanning, &root) })
        .detach();
    let tracking = watcher.clone();
    let events = events.inspect(move |batch| track(&*tracking, batch));
    (Box::pin(events), watcher)
}

fn hidden(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with('.'))
}

/// Registers `directory` and every directory below it that is not a dot directory.
pub fn add_tree(watcher: &dyn Watcher, directory: &Path) {
    if watcher.add(directory).is_err() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_type().is_ok_and(|kind| kind.is_dir()) && !hidden(&path) {
            add_tree(watcher, &path);
        }
    }
}

/// Registers directories that appeared in `batch` and drops registrations for removed paths, so a
/// directory deleted and made again is watched again.
pub fn track(watcher: &dyn Watcher, batch: &[PathEvent]) {
    for event in batch {
        match event.kind {
            Some(PathEventKind::Removed) => {
                watcher.remove(&event.path).ok();
            }
            _ if !hidden(&event.path) && event.path.is_dir() => add_tree(watcher, &event.path),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{path::PathBuf, sync::Mutex};

    #[derive(Default)]
    struct Recorded {
        added: Mutex<Vec<PathBuf>>,
        removed: Mutex<Vec<PathBuf>>,
    }

    impl Recorded {
        fn take(list: &Mutex<Vec<PathBuf>>) -> Vec<PathBuf> {
            let mut paths = std::mem::take(&mut *list.lock().unwrap());
            paths.sort();
            paths
        }
    }

    impl Watcher for Recorded {
        fn add(&self, path: &Path) -> anyhow::Result<()> {
            self.added.lock().unwrap().push(path.to_owned());
            Ok(())
        }

        fn remove(&self, path: &Path) -> anyhow::Result<()> {
            self.removed.lock().unwrap().push(path.to_owned());
            Ok(())
        }
    }

    #[test]
    fn every_directory_but_dot_directories_is_registered_and_new_ones_join() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path();
        for directory in ["devices/a", "Projects/2026", ".quire/cache", "devices/.tmp"] {
            std::fs::create_dir_all(root.join(directory)).unwrap();
        }
        std::fs::write(root.join("Projects/note.md"), "x").unwrap();
        let recorded = Recorded::default();
        add_tree(&recorded, root);
        let mut expected = vec![
            root.to_owned(),
            root.join("Projects"),
            root.join("Projects/2026"),
            root.join("devices"),
            root.join("devices/a"),
        ];
        expected.sort();
        assert_eq!(Recorded::take(&recorded.added), expected);

        std::fs::create_dir_all(root.join("devices/b/inner")).unwrap();
        let event = |path: &str, kind| PathEvent {
            path: root.join(path),
            kind: Some(kind),
        };
        track(
            &recorded,
            &[
                event("devices/b", PathEventKind::Created),
                event("Projects/note.md", PathEventKind::Changed),
                event("devices/a", PathEventKind::Removed),
            ],
        );
        assert_eq!(
            Recorded::take(&recorded.added),
            [root.join("devices/b"), root.join("devices/b/inner")]
        );
        assert_eq!(Recorded::take(&recorded.removed), [root.join("devices/a")]);
    }
}
