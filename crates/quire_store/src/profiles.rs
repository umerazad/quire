//! Current, Work, and Personal: the three notebooks a person keeps apart on one computer, each
//! with its own backup folder and, for Personal only, a Git remote. Ports `rt`'s
//! `src/desktop/profiles.ts`; the records live in `profiles.json` in Quire's application data.
//!
//! Live notebooks never overlap one another or any backup folder, so a backup can never copy a
//! live notebook into another one and Work notes never reach a Personal Git remote. The one
//! exception is moving Current's notebook into Work or Personal, which takes it out of Current.

use crate::{durable_files::sync_directory, git_sync};
use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs,
    io::Write as _,
    path::{Component, Path, PathBuf},
};

#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ProfileId {
    #[default]
    Current,
    Work,
    Personal,
}

impl ProfileId {
    pub const ALL: [Self; 3] = [Self::Current, Self::Work, Self::Personal];

    pub fn label(self) -> &'static str {
        match self {
            Self::Current => "Current",
            Self::Work => "Work",
            Self::Personal => "Personal",
        }
    }

    pub fn git(self) -> git_sync::Profile {
        match self {
            Self::Current => git_sync::Profile::Current,
            Self::Work => git_sync::Profile::Work,
            Self::Personal => git_sync::Profile::Personal,
        }
    }
}

/// Where a profile's notebook lives.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Notebook {
    /// A folder of Markdown notes.
    Folder { root: PathBuf },
    /// A Quire notebook database with its identities.
    Store {
        database: PathBuf,
        notebook: String,
        writer: String,
    },
}

impl Notebook {
    /// The folder that holds the notebook's live data: the notes folder, or the database's folder
    /// with its attachments.
    pub fn root(&self) -> &Path {
        match self {
            Self::Folder { root } => root,
            Self::Store { database, .. } => database.parent().unwrap_or(database),
        }
    }

    /// Whether both name the same notebook, comparing real paths and ignoring letter case.
    pub fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Folder { root: left }, Self::Folder { root: right }) => same_path(left, right),
            (
                Self::Store {
                    database: left,
                    notebook: a,
                    ..
                },
                Self::Store {
                    database: right,
                    notebook: b,
                    ..
                },
            ) => a == b && same_path(left, right),
            _ => false,
        }
    }

    pub fn describe(&self) -> String {
        self.root().display().to_string()
    }
}

/// One profile's notebook, backup folder, and Git remote, with the outcome of the last backup
/// and push. A failure never clears the time of the last success.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub notebook: Option<Notebook>,
    pub backup_folder: Option<PathBuf>,
    pub last_backup_at: Option<u64>,
    pub last_backup_path: Option<PathBuf>,
    pub backup_error: Option<String>,
    pub git_remote: Option<String>,
    pub last_push_at: Option<u64>,
    pub git_error: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Records {
    active: ProfileId,
    profiles: BTreeMap<ProfileId, Profile>,
}

/// The profiles of this computer. Without a file (tests), changes last for the process.
#[derive(Debug)]
pub struct Profiles {
    file: Option<PathBuf>,
    records: Records,
}

/// The path with links resolved as far as it exists, so a folder not created yet compares like
/// its existing parent.
fn real(path: &Path) -> PathBuf {
    let lexical = path
        .components()
        .fold(PathBuf::new(), |mut normal, component| {
            match component {
                Component::CurDir => {}
                Component::ParentDir => {
                    normal.pop();
                }
                other => normal.push(other),
            }
            normal
        });
    lexical
        .ancestors()
        .find_map(|existing| {
            let resolved = fs::canonicalize(existing).ok()?;
            Some(resolved.join(lexical.strip_prefix(existing).ok()?))
        })
        .unwrap_or(lexical)
}

fn folded(path: &Path) -> Vec<String> {
    real(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

fn same_path(left: &Path, right: &Path) -> bool {
    folded(left) == folded(right)
}

/// Whether one folder contains the other or they are the same, ignoring letter case, as `rt`
/// compares them.
pub fn overlaps(left: &Path, right: &Path) -> bool {
    let (left, right) = (folded(left), folded(right));
    left.starts_with(&right) || right.starts_with(&left)
}

impl Profiles {
    /// Profiles kept in memory only.
    pub fn in_memory() -> Self {
        Self {
            file: None,
            records: Records::default(),
        }
    }

    /// Reads the profiles in `file`, or starts with Current active and nothing chosen when it does
    /// not exist yet. An unreadable file is an error, so it is never overwritten.
    pub fn load(file: PathBuf) -> Result<Self> {
        let records = match fs::read(&file) {
            Ok(bytes) => serde_json::from_slice(&bytes).with_context(|| {
                format!(
                    "Cannot read Quire's profiles in {}. Fix the file or move it aside, then reopen Quire.",
                    file.display()
                )
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Records::default(),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "Cannot read Quire's profiles in {}. Check access to Quire's application data, then reopen Quire.",
                        file.display()
                    )
                });
            }
        };
        Ok(Self {
            file: Some(file),
            records,
        })
    }

    pub fn active(&self) -> ProfileId {
        self.records.active
    }

    pub fn get(&self, id: ProfileId) -> Profile {
        self.records.profiles.get(&id).cloned().unwrap_or_default()
    }

    /// The profile whose notebook this is.
    pub fn holding(&self, notebook: &Notebook) -> Option<ProfileId> {
        ProfileId::ALL.into_iter().find(|id| {
            self.get(*id)
                .notebook
                .is_some_and(|held| held.same(notebook))
        })
    }

    /// Applies `change` and saves, leaving everything as it was when saving fails.
    fn change<T>(&mut self, change: impl FnOnce(&mut Records) -> T) -> Result<T> {
        let before = self.records.clone();
        let result = change(&mut self.records);
        if self.records != before
            && let Err(error) = self.save()
        {
            self.records = before;
            return Err(error);
        }
        Ok(result)
    }

    fn save(&self) -> Result<()> {
        let Some(file) = &self.file else {
            return Ok(());
        };
        let failed = || {
            format!(
                "Cannot save Quire's profiles in {}. Check write access and free space in Quire's application data, then retry.",
                file.display()
            )
        };
        let directory = file
            .parent()
            .context("Quire's profiles file has no folder.")?;
        fs::create_dir_all(directory).with_context(failed)?;
        let mut temporary = tempfile::Builder::new()
            .prefix(".profiles-")
            .tempfile_in(directory)
            .with_context(failed)?;
        temporary
            .write_all(&serde_json::to_vec_pretty(&self.records)?)
            .and_then(|()| temporary.as_file().sync_all())
            .with_context(failed)?;
        temporary
            .persist(file)
            .map_err(|error| error.error)
            .with_context(failed)?;
        sync_directory(directory).with_context(failed)
    }

    pub fn activate(&mut self, id: ProfileId) -> Result<()> {
        self.change(|records| records.active = id)
    }

    /// Checks that `notebook` may become `id`'s notebook, returning the profile it would be taken
    /// from: only Current's own notebook moves into Work or Personal.
    pub fn check_notebook(&self, id: ProfileId, notebook: &Notebook) -> Result<Option<ProfileId>> {
        let root = notebook.root();
        for other in ProfileId::ALL.into_iter().filter(|other| *other != id) {
            let Some(held) = self.get(other).notebook else {
                continue;
            };
            if other == ProfileId::Current && held.same(notebook) {
                return Ok(Some(other));
            }
            if overlaps(root, held.root()) {
                bail!(
                    "This folder overlaps the {} notebook. Choose a separate folder for {}.",
                    other.label(),
                    id.label()
                );
            }
        }
        for other in ProfileId::ALL {
            if let Some(folder) = self.get(other).backup_folder
                && overlaps(root, &folder)
            {
                bail!(
                    "The notebook overlaps a backup folder. Choose separate folders for live notes and backups."
                );
            }
        }
        Ok(None)
    }

    /// Makes `notebook` the notebook of `id`. A different notebook starts without backup or push
    /// results and without a Git remote; the backup folder stays.
    pub fn set_notebook(&mut self, id: ProfileId, notebook: Notebook) -> Result<()> {
        let taken = self.check_notebook(id, &notebook)?;
        self.change(|records| {
            if let Some(taken) = taken {
                records.profiles.entry(taken).or_default().notebook = None;
            }
            let profile = records.profiles.entry(id).or_default();
            if profile
                .notebook
                .as_ref()
                .is_some_and(|held| held.same(&notebook))
            {
                profile.notebook = Some(notebook);
                return;
            }
            *profile = Profile {
                notebook: Some(notebook),
                backup_folder: profile.backup_folder.take(),
                ..Profile::default()
            };
        })
    }

    pub fn set_backup_folder(&mut self, id: ProfileId, folder: &Path) -> Result<()> {
        for other in ProfileId::ALL {
            if let Some(notebook) = self.get(other).notebook
                && overlaps(folder, notebook.root())
            {
                bail!(
                    "The backup folder overlaps a live notebook. Choose an independent folder or external drive."
                );
            }
        }
        self.change(|records| {
            let profile = records.profiles.entry(id).or_default();
            profile.backup_folder = Some(folder.to_owned());
            profile.last_backup_at = None;
            profile.last_backup_path = None;
            profile.backup_error = None;
        })
    }

    /// Records a completed backup, or why one failed, keeping the last success after a failure.
    pub fn record_backup(
        &mut self,
        id: ProfileId,
        outcome: std::result::Result<(u64, PathBuf), String>,
    ) -> Result<()> {
        self.change(|records| {
            let profile = records.profiles.entry(id).or_default();
            match outcome {
                Ok((at, path)) => {
                    profile.last_backup_at = Some(at);
                    profile.last_backup_path = Some(path);
                    profile.backup_error = None;
                }
                Err(error) => profile.backup_error = Some(error),
            }
        })
    }

    /// Sets Personal's Git remote; every other profile refuses Git.
    pub fn set_git_remote(&mut self, id: ProfileId, remote: &str) -> Result<Option<String>> {
        let remote = git_sync::check_remote(id.git(), remote)?;
        self.change(|records| {
            let profile = records.profiles.entry(id).or_default();
            profile.git_remote = remote.clone();
            profile.last_push_at = None;
            profile.git_error = None;
        })?;
        Ok(remote)
    }

    /// Records a completed push, or why a sync failed, keeping the last push after a failure.
    pub fn record_git(
        &mut self,
        id: ProfileId,
        outcome: std::result::Result<u64, String>,
    ) -> Result<()> {
        self.change(|records| {
            let profile = records.profiles.entry(id).or_default();
            match outcome {
                Ok(at) => {
                    profile.last_push_at = Some(at);
                    profile.git_error = None;
                }
                Err(error) => profile.git_error = Some(error),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(root: &Path) -> Notebook {
        Notebook::Folder {
            root: root.to_owned(),
        }
    }

    #[test]
    fn current_moves_into_a_named_profile_and_the_choice_survives_reopening() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("data").join("profiles.json");
        let notes = directory.path().join("notes");
        fs::create_dir_all(&notes).unwrap();
        let mut profiles = Profiles::load(file.clone()).unwrap();
        assert_eq!(profiles.active(), ProfileId::Current);
        profiles
            .set_notebook(ProfileId::Current, folder(&notes))
            .unwrap();
        profiles
            .set_notebook(ProfileId::Work, folder(&notes))
            .unwrap();
        profiles.activate(ProfileId::Work).unwrap();
        assert_eq!(profiles.get(ProfileId::Current).notebook, None);
        let profiles = Profiles::load(file).unwrap();
        assert_eq!(profiles.active(), ProfileId::Work);
        assert_eq!(profiles.holding(&folder(&notes)), Some(ProfileId::Work));
        assert_eq!(
            profiles.holding(&folder(&notes.join("."))),
            Some(ProfileId::Work)
        );
    }

    #[test]
    fn overlapping_notebooks_and_backup_folders_are_refused() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        let mut profiles = Profiles::in_memory();
        profiles
            .set_notebook(ProfileId::Work, folder(&root.join("work")))
            .unwrap();
        let refused = profiles
            .set_notebook(
                ProfileId::Personal,
                folder(&root.join("WORK").join("private")),
            )
            .unwrap_err();
        assert_eq!(
            refused.to_string(),
            "This folder overlaps the Work notebook. Choose a separate folder for Personal."
        );
        assert!(
            profiles
                .set_notebook(ProfileId::Personal, folder(root))
                .is_err()
        );
        let refused = profiles
            .set_backup_folder(ProfileId::Work, &root.join("work").join("backups"))
            .unwrap_err();
        assert!(refused.to_string().contains("overlaps a live notebook"));
        profiles
            .set_backup_folder(ProfileId::Work, &root.join("backups"))
            .unwrap();
        let refused = profiles
            .set_notebook(
                ProfileId::Personal,
                folder(&root.join("backups").join("notes")),
            )
            .unwrap_err();
        assert!(refused.to_string().contains("overlaps a backup folder"));
        let store = Notebook::Store {
            database: root.join("work").join("local.sqlite"),
            notebook: "nb".into(),
            writer: "w".into(),
        };
        assert!(profiles.set_notebook(ProfileId::Current, store).is_err());
        profiles
            .set_notebook(ProfileId::Personal, folder(&root.join("personal")))
            .unwrap();
        assert_eq!(
            profiles.get(ProfileId::Personal).notebook,
            Some(folder(&root.join("personal")))
        );
    }

    #[test]
    fn git_stays_personal_and_failures_never_erase_the_last_success() {
        let mut profiles = Profiles::in_memory();
        for id in [ProfileId::Current, ProfileId::Work] {
            assert!(
                profiles
                    .set_git_remote(id, "git@example.com:notes.git")
                    .unwrap_err()
                    .to_string()
                    .contains("only in Personal")
            );
        }
        assert!(
            profiles
                .set_git_remote(ProfileId::Personal, "https://token@example.com/notes.git")
                .unwrap_err()
                .to_string()
                .contains("credentials")
        );
        profiles
            .record_backup(ProfileId::Work, Ok((10, "completed".into())))
            .unwrap();
        profiles
            .record_backup(ProfileId::Work, Err("drive disconnected".into()))
            .unwrap();
        let work = profiles.get(ProfileId::Work);
        assert_eq!(work.last_backup_at, Some(10));
        assert_eq!(work.last_backup_path, Some("completed".into()));
        assert_eq!(work.backup_error.as_deref(), Some("drive disconnected"));
        profiles
            .set_git_remote(ProfileId::Personal, " git@example.com:notes.git ")
            .unwrap();
        profiles.record_git(ProfileId::Personal, Ok(20)).unwrap();
        profiles
            .record_git(ProfileId::Personal, Err("network offline".into()))
            .unwrap();
        let personal = profiles.get(ProfileId::Personal);
        assert_eq!(
            personal.git_remote.as_deref(),
            Some("git@example.com:notes.git")
        );
        assert_eq!(personal.last_push_at, Some(20));
        assert_eq!(personal.git_error.as_deref(), Some("network offline"));
        profiles
            .set_backup_folder(ProfileId::Personal, Path::new("/backups"))
            .unwrap();
        profiles
            .set_notebook(ProfileId::Personal, folder(Path::new("/new-personal")))
            .unwrap();
        let personal = profiles.get(ProfileId::Personal);
        assert_eq!(personal.git_remote, None);
        assert_eq!(personal.last_push_at, None);
        assert_eq!(personal.backup_folder, Some("/backups".into()));
        profiles
            .set_notebook(ProfileId::Personal, folder(Path::new("/new-personal")))
            .unwrap();
        assert_eq!(
            profiles.get(ProfileId::Personal).backup_folder,
            Some("/backups".into())
        );
    }

    #[test]
    fn a_damaged_file_is_reported_and_a_failed_save_changes_nothing() {
        let directory = tempfile::tempdir().unwrap();
        let file = directory.path().join("profiles.json");
        fs::write(&file, "{not json").unwrap();
        let error = Profiles::load(file.clone()).unwrap_err();
        assert!(format!("{error:#}").contains("Fix the file or move it aside"));
        assert_eq!(fs::read_to_string(&file).unwrap(), "{not json");

        let blocked = directory.path().join("blocked.json");
        let mut profiles = Profiles::load(blocked.clone()).unwrap();
        fs::create_dir(&blocked).unwrap();
        fs::write(blocked.join("kept"), "a folder where the file belongs").unwrap();
        let error = profiles.activate(ProfileId::Personal).unwrap_err();
        assert!(format!("{error:#}").contains("Cannot save Quire's profiles"));
        assert_eq!(profiles.active(), ProfileId::Current);
    }
}
