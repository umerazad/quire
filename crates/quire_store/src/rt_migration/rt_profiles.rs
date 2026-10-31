//! `rt`'s profiles: `profiles.sqlite` (rt `src/desktop/profiles.ts`), read from a copy, and the
//! `settings.json` that named the last notebook before profiles existed.

use super::copy_database;
use crate::profiles::ProfileId;
use anyhow::{Context as _, Result};
use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// One `rt` profile: its notebook root, backup folder, and Git remote.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RtProfile {
    pub id: ProfileId,
    pub root: Option<PathBuf>,
    pub backup_folder: Option<PathBuf>,
    pub git_remote: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RtProfiles {
    pub active: ProfileId,
    pub profiles: Vec<RtProfile>,
    /// What could not be read, with what to do; reading continues without it.
    pub problems: Vec<String>,
}

impl RtProfiles {
    /// Every notebook root the profiles name.
    pub fn roots(&self) -> Vec<PathBuf> {
        self.profiles
            .iter()
            .filter_map(|profile| profile.root.clone())
            .collect()
    }
}

fn profile_id(id: &str) -> Option<ProfileId> {
    match id {
        "current" => Some(ProfileId::Current),
        "work" => Some(ProfileId::Work),
        "personal" => Some(ProfileId::Personal),
        _ => None,
    }
}

/// The last notebook in `settings.json`, which `rt` copies into Current once (`migrate`).
fn legacy_root(rt_data: &Path, problems: &mut Vec<String>) -> Option<PathBuf> {
    let file = rt_data.join("settings.json");
    let text = match fs::read(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            problems.push(format!(
                "rt's {} could not be read ({error}); its last notebook is not considered. Check that the file is readable.",
                file.display()
            ));
            return None;
        }
    };
    match serde_json::from_slice::<serde_json::Value>(&text) {
        Ok(value) => value
            .get("root")
            .and_then(|root| root.as_str())
            .map(PathBuf::from),
        Err(error) => {
            problems.push(format!(
                "rt's {} is damaged ({error}); its last notebook is not considered. rt reports the same and asks to open the notebook again.",
                file.display()
            ));
            None
        }
    }
}

/// Reads `rt`'s profiles from a copy of `profiles.sqlite` in `scratch`, or Current alone from
/// `settings.json` when `rt` never made profiles. A damaged database is an error.
pub fn read_profiles(rt_data: &Path, scratch: &Path) -> Result<RtProfiles> {
    let mut problems = Vec::new();
    let legacy = legacy_root(rt_data, &mut problems);
    let database = rt_data.join("profiles.sqlite");
    if !database.exists() {
        return Ok(RtProfiles {
            active: ProfileId::Current,
            profiles: ProfileId::ALL
                .into_iter()
                .map(|id| RtProfile {
                    id,
                    root: legacy.clone().filter(|_| id == ProfileId::Current),
                    ..RtProfile::default()
                })
                .collect(),
            problems,
        });
    }
    let copy = copy_database(&database, scratch)?;
    let connection = &copy.connection;
    let rows = connection
        .select::<(String, Option<String>, Option<String>, Option<String>)>(
            "SELECT id,root,backupDirectory,gitRemote FROM profiles ORDER BY rowid",
        )
        .and_then(|mut rows| rows())
        .context("rt's profiles.sqlite has no readable profiles table. Restore it from a backup of rt's data folder")?;
    let preference = |key: &str| -> Result<Option<String>> {
        connection
            .select_row_bound::<_, String>("SELECT value FROM preferences WHERE key=?")
            .and_then(|mut select| select(key))
            .context("rt's profiles.sqlite has no readable preferences table. Restore it from a backup of rt's data folder")
    };
    let active = preference("active")?
        .as_deref()
        .and_then(profile_id)
        .unwrap_or_default();
    let migrated = preference("migrated")?.is_some();
    let mut profiles: Vec<RtProfile> = rows
        .into_iter()
        .filter_map(|(id, root, backup, remote)| {
            Some(RtProfile {
                id: profile_id(&id)?,
                root: root.map(PathBuf::from),
                backup_folder: backup.map(PathBuf::from),
                git_remote: remote,
            })
        })
        .collect();
    if !migrated
        && let Some(current) = profiles
            .iter_mut()
            .find(|profile| profile.id == ProfileId::Current && profile.root.is_none())
    {
        current.root = legacy;
    }
    Ok(RtProfiles {
        active,
        profiles,
        problems,
    })
}
