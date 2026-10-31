//! Where a notebook folder is synchronized, the files a provider keeps only in the cloud, and the
//! conflict copies providers create. Folder publication proves only that this computer wrote the
//! file; a provider may still be uploading it, and nothing here can see another computer.

use anyhow::{Context as _, Result};
use std::{
    fs::{self, Metadata},
    path::{Component, Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Provider {
    OneDrive,
    Dropbox,
    ICloud,
    GoogleDrive,
}

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Self::OneDrive => "OneDrive",
            Self::Dropbox => "Dropbox",
            Self::ICloud => "iCloud Drive",
            Self::GoogleDrive => "Google Drive",
        }
    }

    /// What makes every file of the notebook stay on this computer, in the provider's words.
    pub fn keep_offline(self) -> &'static str {
        match self {
            Self::OneDrive => {
                "In Finder or File Explorer, choose Always keep on this device for the notebook folder"
            }
            Self::Dropbox => "In Dropbox, make the notebook folder Available offline",
            Self::ICloud => "In Finder, choose Keep Downloaded for the notebook folder",
            Self::GoogleDrive => "In Google Drive, make the notebook folder Available offline",
        }
    }
}

/// The provider whose synchronized folder holds `root`, judged from the folder's real path, so a
/// link such as `~/OneDrive` resolves to the provider's own location.
pub fn detect(root: &Path) -> Option<Provider> {
    let real = fs::canonicalize(root).unwrap_or_else(|_| root.to_owned());
    let names: Vec<String> = real
        .components()
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().to_lowercase()),
            _ => None,
        })
        .collect();
    let after = |parent: &str, name: &str| {
        names
            .windows(2)
            .any(|pair| pair[0] == parent && pair[1] == name)
    };
    names
        .iter()
        .find_map(|name| {
            let name = name.as_str();
            if name.starts_with("onedrive") {
                Some(Provider::OneDrive)
            } else if name == "dropbox"
                || name.starts_with("dropbox (")
                || name.starts_with("dropbox-")
            {
                Some(Provider::Dropbox)
            } else if name == "icloud drive" || name == "com~apple~clouddocs" {
                Some(Provider::ICloud)
            } else if name.starts_with("googledrive") || name == "google drive" {
                Some(Provider::GoogleDrive)
            } else {
                None
            }
        })
        .or_else(|| after("library", "mobile documents").then_some(Provider::ICloud))
}

/// Files a provider shows but keeps only in the cloud, and copies a provider made when two
/// computers changed one file, as paths relative to the notebook folder in walk order.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Scan {
    pub placeholders: Vec<PathBuf>,
    pub conflict_copies: Vec<PathBuf>,
}

/// Tells whether a file's metadata marks it as a placeholder whose contents are not on this
/// computer. Reading only metadata never downloads the file.
pub type PlaceholderTest = dyn Fn(&Metadata) -> bool;

/// macOS File Provider placeholders (OneDrive, Dropbox, Google Drive, iCloud on macOS 14 and
/// later) carry `SF_DATALESS`; Windows Cloud Files placeholders carry the offline or recall
/// attributes. Other platforms have no placeholders.
pub fn platform_placeholder(metadata: &Metadata) -> bool {
    #[cfg(target_os = "macos")]
    {
        use std::os::macos::fs::MetadataExt as _;
        const SF_DATALESS: u32 = 0x4000_0000;
        metadata.st_flags() & SF_DATALESS != 0
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt as _;
        const OFFLINE: u32 = 0x1000;
        const RECALL_ON_OPEN: u32 = 0x4_0000;
        const RECALL_ON_DATA_ACCESS: u32 = 0x40_0000;
        metadata.file_attributes() & (OFFLINE | RECALL_ON_OPEN | RECALL_ON_DATA_ACCESS) != 0
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = metadata;
        false
    }
}

pub fn scan(root: &Path) -> Result<Scan> {
    scan_with(root, &platform_placeholder)
}

/// Walks the notebook folder without following links or entering `.git`.
pub fn scan_with(root: &Path, is_placeholder: &PlaceholderTest) -> Result<Scan> {
    let mut found = Scan::default();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        let entries = fs::read_dir(root.join(&relative)).with_context(|| {
            format!(
                "Cannot list {} in the notebook folder. Check that the folder is available and readable, then retry.",
                root.join(&relative).display()
            )
        })?;
        let mut entries = entries
            .map(|entry| entry.map(|entry| (entry.file_name(), entry.path())))
            .collect::<std::io::Result<Vec<_>>>()
            .context("Cannot list the notebook folder. Check that it is available, then retry.")?;
        entries.sort();
        for (name, path) in entries.into_iter().rev() {
            let name = name.to_string_lossy().into_owned();
            let child = relative.join(&name);
            let metadata = fs::symlink_metadata(&path).with_context(|| {
                format!(
                    "Cannot inspect {} in the notebook folder. Check its permissions, then retry.",
                    child.display()
                )
            })?;
            if metadata.is_dir() {
                if name != ".git" {
                    pending.push(child);
                }
            } else if metadata.is_file() {
                if let Some(original) = icloud_stub(&name) {
                    found.placeholders.push(relative.join(original));
                } else if is_placeholder(&metadata) {
                    found.placeholders.push(child.clone());
                }
                if is_conflict_copy(&child) {
                    found.conflict_copies.push(child);
                }
            }
        }
    }
    found.placeholders.sort();
    found.conflict_copies.sort();
    Ok(found)
}

/// iCloud Drive before macOS 14 replaces a file it evicted with `.<name>.icloud`.
fn icloud_stub(name: &str) -> Option<&str> {
    name.strip_prefix('.')?
        .strip_suffix(".icloud")
        .filter(|original| !original.is_empty())
}

/// Copies whose names only a provider makes: Dropbox's conflicted, case-conflict, and
/// selective-sync copies, and any copy of a format 1 change file or notebook identity, whose
/// exact names Quire chooses. OneDrive, iCloud, and Google Drive name copies `Note-COMPUTER.md`,
/// `Note 2.md`, or `Note (1).md`, which people also choose, so those are not reported for notes.
fn is_conflict_copy(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return false;
    };
    let lower = name.to_lowercase();
    if [
        "conflicted copy",
        "(case conflict",
        "(selective sync conflict",
    ]
    .iter()
    .any(|mark| lower.contains(mark))
    {
        return true;
    }
    let in_log = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|parent| parent == "log");
    if in_log && (lower.ends_with(".qlog.zst") || lower.ends_with(".qsnap.zst")) {
        return !strict_change_name(&name);
    }
    path.parent() == Some(Path::new(""))
        && lower.starts_with("quire-notebook")
        && lower.ends_with(".json")
        && name != "quire-notebook.json"
}

fn strict_change_name(name: &str) -> bool {
    let Some((sequence, rest)) = name.split_once('-') else {
        return false;
    };
    let hash = rest
        .strip_suffix(".qlog.zst")
        .or_else(|| rest.strip_suffix(".qsnap.zst"));
    sequence.len() == 9
        && sequence.bytes().all(|byte| byte.is_ascii_digit())
        && hash.is_some_and(|hash| {
            hash.len() == 64
                && hash
                    .bytes()
                    .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(root: &Path, path: &str, text: &str) {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }

    #[test]
    fn providers_are_detected_from_their_folder_names() {
        let cases = [
            (
                "/Users/a/Library/CloudStorage/OneDrive-Personal/Notes",
                Some(Provider::OneDrive),
            ),
            (
                "/Users/a/OneDrive - Contoso/Notes",
                Some(Provider::OneDrive),
            ),
            ("C:/Users/a/OneDrive/Notes", Some(Provider::OneDrive)),
            ("/Users/a/Dropbox/Notes", Some(Provider::Dropbox)),
            ("/Users/a/Dropbox (Personal)/Notes", Some(Provider::Dropbox)),
            (
                "/Users/a/Library/CloudStorage/Dropbox/Notes",
                Some(Provider::Dropbox),
            ),
            (
                "/Users/a/Library/Mobile Documents/com~apple~CloudDocs/Notes",
                Some(Provider::ICloud),
            ),
            (
                "/Users/a/Library/CloudStorage/GoogleDrive-a@example.com/My Drive",
                Some(Provider::GoogleDrive),
            ),
            ("/Users/a/Documents/Notes", None),
            ("/Users/a/Documents/dropboxes/Notes", None),
        ];
        for (path, expected) in cases {
            assert_eq!(detect(Path::new(path)), expected, "{path}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_link_into_a_provider_folder_is_detected_by_its_real_path() {
        let root = tempfile::tempdir().unwrap();
        let notes = root
            .path()
            .join("Library/CloudStorage/OneDrive-Personal/Notes");
        fs::create_dir_all(&notes).unwrap();
        let link = root.path().join("Notes");
        std::os::unix::fs::symlink(&notes, &link).unwrap();
        assert_eq!(detect(&link), Some(Provider::OneDrive));
    }

    #[test]
    fn placeholders_and_provider_conflict_copies_are_found_without_reading_contents() {
        let root = tempfile::tempdir().unwrap();
        let hash = "a".repeat(64);
        for (path, text) in [
            ("Daily/2026-10-31.md", "- here"),
            ("Daily/.2026-10-30.md.icloud", "stub"),
            (
                "Projects/Plan.md",
                "a file whose contents stayed in the cloud",
            ),
            ("Projects/Plan (conflicted copy 2026-10-31).md", "copy"),
            ("Projects/Plan (Case Conflict).md", "copy"),
            ("Projects/Plan 2.md", "a note a person named"),
            (
                &format!("devices/home/log/000000001-{hash}.qlog.zst"),
                "log",
            ),
            (
                &format!("devices/home/log/000000001-{hash} (1).qlog.zst"),
                "copy",
            ),
            ("quire-notebook.json", "{}"),
            ("quire-notebook (1).json", "{}"),
            (".git/objects/.x.icloud", "ignored"),
        ] {
            write(root.path(), path, text);
        }
        // A stand-in for SF_DATALESS, which only the provider's system extension can set.
        let evicted = fs::metadata(root.path().join("Projects/Plan.md"))
            .unwrap()
            .len();
        let found = scan_with(root.path(), &move |metadata| metadata.len() == evicted).unwrap();
        assert_eq!(
            found.placeholders,
            [
                PathBuf::from("Daily/2026-10-30.md"),
                PathBuf::from("Projects/Plan.md")
            ]
        );
        assert_eq!(
            found.conflict_copies,
            [
                PathBuf::from("Projects/Plan (Case Conflict).md"),
                PathBuf::from("Projects/Plan (conflicted copy 2026-10-31).md"),
                PathBuf::from(format!("devices/home/log/000000001-{hash} (1).qlog.zst")),
                PathBuf::from("quire-notebook (1).json"),
            ]
        );
    }

    #[test]
    fn an_ordinary_folder_has_no_placeholders_on_this_platform() {
        let root = tempfile::tempdir().unwrap();
        write(root.path(), "A.md", "- a");
        assert_eq!(scan(root.path()).unwrap(), Scan::default());
    }

    #[test]
    fn an_unavailable_folder_says_what_failed_and_what_to_do() {
        let root = tempfile::tempdir().unwrap();
        let error = scan(&root.path().join("missing")).unwrap_err();
        assert!(format!("{error:#}").contains("Check that the folder is available"));
    }
}
