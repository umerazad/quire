//! Where a notebook keeps attachment bytes, and reading them without leaving the notebook.
//!
//! A Quire notebook keeps attachments in its store's durable local cache, named by content hash
//! (`attachments/<sha256>.<ext>`), so pages reference that path. A folder notebook keeps them as
//! ordinary files beside its notes, reached by relative paths. Either way a path that climbs out,
//! passes through a hidden or symbolic-link entry, or resolves outside the root is refused.

use crate::MediaError;
use std::{
    fs::{self, File},
    io::Read as _,
    path::{Component, Path, PathBuf},
};

/// The folder that notebook-relative media paths resolve in.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum Files {
    /// A Quire notebook: its store's attachment cache, which holds `attachments/<name>`.
    Store { attachments: PathBuf },
    /// A folder notebook's root.
    Folder { root: PathBuf },
}

impl Files {
    /// The attachment cache of the store whose database is at `database`.
    pub fn for_store_database(database: &Path) -> Self {
        Self::Store {
            attachments: database
                .parent()
                .unwrap_or(Path::new("."))
                .join("attachments"),
        }
    }

    /// The ordinary file a notebook-relative path names.
    pub fn locate(&self, relative: &str) -> Result<PathBuf, MediaError> {
        let parts: Vec<&str> = relative.split('/').collect();
        if parts.iter().any(|part| {
            part.is_empty() || *part == ".." || part.starts_with('.') || part.contains('\\')
        }) {
            return Err(outside(relative));
        }
        let (root, path) = match self {
            Self::Store { attachments } => match parts.as_slice() {
                ["attachments", name] => (attachments.clone(), attachments.join(name)),
                _ => {
                    return Err(MediaError::Missing(format!(
                        "{relative} is not a Quire attachment. Attach the file with Attach file, which stores it under attachments/."
                    )));
                }
            },
            Self::Folder { root } => (root.clone(), root.join(relative)),
        };
        if Path::new(relative)
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(outside(relative));
        }
        let metadata = fs::symlink_metadata(&path).map_err(|_| missing(relative))?;
        if !metadata.is_file() {
            return Err(MediaError::Refused(format!(
                "{relative} is not an ordinary file. Replace the link or folder with the file itself, then retry."
            )));
        }
        if fs::symlink_metadata(&root).map_or(true, |metadata| !metadata.is_dir()) {
            return Err(outside(relative));
        }
        let root = fs::canonicalize(&root).map_err(|_| missing(relative))?;
        let resolved = fs::canonicalize(&path).map_err(|_| missing(relative))?;
        if !resolved.starts_with(&root) || resolved == root {
            return Err(outside(relative));
        }
        Ok(resolved)
    }
}

/// Reads at most `limit` bytes of an ordinary file, refusing larger files.
pub fn read_limited(path: &Path, limit: usize) -> Result<Vec<u8>, MediaError> {
    let name = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let file = File::open(path).map_err(|_| missing(&name))?;
    let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
    if size > limit as u64 {
        return Err(crate::source::too_large(limit));
    }
    let mut bytes = Vec::with_capacity(size as usize);
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            MediaError::Failed(format!(
                "Cannot read {name}: {error}. Check the file's permissions, then retry."
            ))
        })?;
    if bytes.len() > limit {
        return Err(crate::source::too_large(limit));
    }
    Ok(bytes)
}

fn missing(relative: &str) -> MediaError {
    MediaError::Missing(format!(
        "{relative} is not available on this computer. Make the file available in the notebook (or let sync deliver it), then retry."
    ))
}

fn outside(relative: &str) -> MediaError {
    MediaError::Refused(format!(
        "{relative} is outside the notebook. Move the file into the notebook and link to it there."
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_paths_stay_inside_the_notebook() {
        let root = tempfile::tempdir().unwrap();
        let outer = tempfile::tempdir().unwrap();
        fs::create_dir(root.path().join("assets")).unwrap();
        fs::write(root.path().join("assets/a.png"), b"x").unwrap();
        fs::write(root.path().join(".hidden.png"), b"x").unwrap();
        fs::write(outer.path().join("secret.png"), b"x").unwrap();
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(
                outer.path().join("secret.png"),
                root.path().join("link.png"),
            )
            .unwrap();
            std::os::unix::fs::symlink(outer.path(), root.path().join("linked")).unwrap();
        }
        let files = Files::Folder {
            root: root.path().to_owned(),
        };
        let found = files.locate("assets/a.png").unwrap();
        assert_eq!(fs::read(found).unwrap(), b"x");
        assert!(matches!(
            files.locate("assets/none.png"),
            Err(MediaError::Missing(_))
        ));
        for refused in ["../a.png", ".hidden.png", "assets//a.png", "assets"] {
            assert!(
                matches!(files.locate(refused), Err(MediaError::Refused(_))),
                "{refused}"
            );
        }
        #[cfg(unix)]
        {
            assert!(matches!(
                files.locate("link.png"),
                Err(MediaError::Refused(_))
            ));
            assert!(matches!(
                files.locate("linked/secret.png"),
                Err(MediaError::Refused(_))
            ));
        }
    }

    #[test]
    fn store_paths_name_only_the_attachment_cache() {
        let root = tempfile::tempdir().unwrap();
        let files = Files::for_store_database(&root.path().join("local.sqlite"));
        fs::create_dir(root.path().join("attachments")).unwrap();
        fs::write(root.path().join("attachments/abc.png"), b"png").unwrap();
        fs::write(root.path().join("page.png"), b"png").unwrap();
        assert!(files.locate("attachments/abc.png").is_ok());
        assert!(matches!(
            files.locate("page.png"),
            Err(MediaError::Missing(_))
        ));
        assert!(matches!(
            files.locate("attachments/missing.png"),
            Err(MediaError::Missing(message)) if message.contains("retry")
        ));
    }

    #[test]
    fn reads_refuse_files_over_the_limit() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("big.bin");
        fs::write(&path, vec![0; 2048]).unwrap();
        assert_eq!(read_limited(&path, 4096).unwrap().len(), 2048);
        assert!(matches!(
            read_limited(&path, 1024),
            Err(MediaError::Refused(_))
        ));
    }
}
