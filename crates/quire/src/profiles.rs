//! The Current, Work, and Personal profiles of this computer (`quire_store::profiles`), shared by
//! every window. A notebook opened in Quire becomes the active profile's notebook unless another
//! profile already holds it, as opening a notebook does in the reference app; its backups and Git
//! sync follow that profile.

use gpui::{App, Global, SharedString};
use quire_store::profiles::{Notebook, Profile, ProfileId, Profiles};
use std::path::PathBuf;

gpui::actions!(
    quire_profile,
    [
        /// Saves open pages and opens the Current profile's notebook.
        UseCurrentProfile,
        /// Saves open pages and opens the Work profile's notebook.
        UseWorkProfile,
        /// Saves open pages and opens the Personal profile's notebook.
        UsePersonalProfile,
        /// Chooses a folder of Markdown notes as the active profile's notebook and opens it.
        OpenNotebookFolder,
    ]
);

pub struct ProfileStore(Profiles);

impl Global for ProfileStore {}

impl ProfileStore {
    /// Whether `notebook` may become the notebook of `id`.
    pub fn check(&self, id: ProfileId, notebook: &Notebook) -> Result<(), String> {
        self.0
            .check_notebook(id, notebook)
            .map(drop)
            .map_err(|error| format!("{error:#}"))
    }
}

/// Starts with profiles kept in memory; [`load`] replaces them with this computer's.
pub fn init(cx: &mut App) {
    if !cx.has_global::<ProfileStore>() {
        cx.set_global(ProfileStore(Profiles::in_memory()));
    }
}

/// Reads this computer's profiles from `file`. A damaged file stays untouched: Quire keeps
/// profiles in memory for this run and says so.
pub fn load(file: PathBuf, cx: &mut App) {
    match Profiles::load(file) {
        Ok(profiles) => cx.set_global(ProfileStore(profiles)),
        Err(error) => {
            cx.set_global(ProfileStore(Profiles::in_memory()));
            crate::outline::show_notice(
                format!("{error:#} Until then, profile changes last only until Quire quits."),
                cx,
            );
        }
    }
}

pub fn active(cx: &App) -> ProfileId {
    cx.global::<ProfileStore>().0.active()
}

pub fn get(id: ProfileId, cx: &App) -> Profile {
    cx.global::<ProfileStore>().0.get(id)
}

/// The profile holding `notebook`.
pub fn holding(notebook: &Notebook, cx: &App) -> Option<ProfileId> {
    cx.try_global::<ProfileStore>()
        .and_then(|store| store.0.holding(notebook))
}

/// Changes the profiles and refreshes every window and the menus, or explains why not.
pub fn update<T>(
    cx: &mut App,
    change: impl FnOnce(&mut Profiles) -> anyhow::Result<T>,
) -> Result<T, String> {
    let result =
        change(&mut cx.global_mut::<ProfileStore>().0).map_err(|error| format!("{error:#}"));
    crate::app_menus::refresh(cx);
    cx.refresh_windows();
    result
}

/// Whether `notebook` may open: a notebook a profile holds always may, and any other must fit
/// the active profile without overlapping another notebook or a backup folder.
pub fn check_open(notebook: &Notebook, cx: &App) -> Result<(), String> {
    let profiles = &cx.global::<ProfileStore>().0;
    if profiles.holding(notebook).is_some() {
        return Ok(());
    }
    profiles
        .check_notebook(profiles.active(), notebook)
        .map(drop)
        .map_err(|error| format!("{error:#}"))
}

/// Makes the opened `notebook` belong to a profile: the one holding it, which becomes active,
/// or else the active profile.
pub fn adopt(notebook: Notebook, cx: &mut App) -> Result<ProfileId, String> {
    if let Some(id) = holding(&notebook, cx) {
        update(cx, |profiles| profiles.activate(id))?;
        return Ok(id);
    }
    let id = active(cx);
    update(cx, |profiles| profiles.set_notebook(id, notebook))
        .map(|()| id)
        .map_err(|error| {
            format!("{error} This notebook stays open, but it has no profile, so backups and Git sync are off for it.")
        })
}

/// One line for the window: the profile, its last backup, and its last push, apart from local
/// saves.
pub fn status(id: ProfileId, label: &dyn Fn(i64) -> String, cx: &App) -> SharedString {
    let profile = get(id, cx);
    let backup = match (&profile.backup_error, profile.last_backup_at) {
        (Some(error), _) => format!("Backup failed: {error}"),
        (None, Some(at)) => format!("Last backup {}", label(at as i64)),
        (None, None) if profile.backup_folder.is_some() => "No backup yet".into(),
        (None, None) => "No backup folder".into(),
    };
    let git = match (id, &profile.git_error, profile.last_push_at) {
        (ProfileId::Personal, Some(error), _) => format!(" · Git sync failed: {error}"),
        (ProfileId::Personal, None, Some(at)) => format!(" · Last push {}", label(at as i64)),
        _ => String::new(),
    };
    format!("{} profile · {backup}{git}", id.label()).into()
}
