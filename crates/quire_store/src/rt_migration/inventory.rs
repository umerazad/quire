//! Everything `rt` 0.9.57 keeps, found on disk and classified: where it lives, its format, and
//! whether Quire reads it in place, imports it, can restore it, or does not need it. Listing
//! only reads names and file types.

use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Disposition {
    /// Quire reads and edits it where it is; both apps see the same data.
    ReadInPlace,
    /// Quire copies what it holds into its own data folder; `rt`'s copy is unchanged.
    Imported,
    /// A backup Quire can restore as a new notebook.
    Restorable,
    /// Kept by `rt` and not migrated; the note says what to do.
    NotMigrated,
    /// Rebuilt or replaced by Quire, or only `rt`'s own runtime state.
    NotNeeded,
}

impl Disposition {
    pub fn label(self) -> &'static str {
        match self {
            Self::ReadInPlace => "read in place",
            Self::Imported => "imported",
            Self::Restorable => "restorable",
            Self::NotMigrated => "not migrated",
            Self::NotNeeded => "not needed",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Item {
    pub what: String,
    pub path: PathBuf,
    pub format: &'static str,
    pub disposition: Disposition,
    pub note: String,
}

fn item(
    what: &str,
    path: PathBuf,
    format: &'static str,
    disposition: Disposition,
    note: &str,
) -> Item {
    Item {
        what: what.into(),
        path,
        format,
        disposition,
        note: note.into(),
    }
}

/// Chromium's own folders and files in an Electron profile.
const ELECTRON: &[&str] = &[
    "Cache",
    "Code Cache",
    "GPUCache",
    "DawnGraphiteCache",
    "DawnWebGPUCache",
    "Crashpad",
    "Session Storage",
    "Shared Dictionary",
    "Network Persistent State",
    "Preferences",
    "Cookies",
    "Cookies-journal",
    "TransportSecurity",
    "Trust Tokens",
    "Trust Tokens-journal",
    "SharedStorage",
    "blob_storage",
    "WebStorage",
    "Local State",
    "lockfile",
];

fn app_data_item(path: PathBuf, name: &str) -> Item {
    use Disposition::*;
    match name {
        "profiles.sqlite" => item(
            "Profiles: each profile's notebook, backup folder, Git remote, and last results",
            path,
            "SQLite (WAL)",
            Imported,
            "Each Quire profile gets its rt profile's notebook, backup folder, and Personal's Git remote, once. rt's backup and push results stay rt's.",
        ),
        "profiles.sqlite-wal" | "profiles.sqlite-shm" => item(
            "Profiles write-ahead log",
            path,
            "SQLite WAL",
            Imported,
            "Copied with profiles.sqlite and read from the copy.",
        ),
        "settings.json" => item(
            "The last notebook before profiles existed",
            path,
            "JSON {root}",
            Imported,
            "Becomes Current's notebook only when profiles.sqlite never recorded one, as rt does.",
        ),
        "appearance.json" => item(
            "Light, dark, or system appearance",
            path,
            "JSON {appearance}",
            Imported,
            "Imported by Settings at Quire's first launch.",
        ),
        "ai-settings.json" => item(
            "AI backend, binary paths, model, writes, terminal registrations",
            path,
            "JSON",
            NotMigrated,
            "Quire turns AI on only after its own Test passes. Choose the backend in Settings > AI, press Test, and register terminals again with Use from a terminal.",
        ),
        "device-id" => item(
            "This computer's writer identity",
            path,
            "UUID text",
            NotNeeded,
            "Quire writes under its own new identities and never reuses rt's.",
        ),
        "notebooks" => item(
            "Each Quire notebook's local database: published log cache, unpublished saves, drafts",
            path,
            "SQLite (WAL) per notebook",
            Imported,
            "Pages and history come from the synced change log; drafts come from a copy. Unpublished saves stop that notebook's migration until rt publishes them.",
        ),
        "Local Storage" => item(
            "Editor settings, tabs, favorites, recents, AI prompt history, synthesis instruction",
            path,
            "Chromium LevelDB",
            Imported,
            "Editor settings and appearance at first launch; tabs, favorites, recents, prompt history, and the synthesis instruction by the migration.",
        ),
        "IndexedDB" => item(
            "Page analysis cache",
            path,
            "Chromium IndexedDB",
            NotNeeded,
            "Quire analyzes pages itself.",
        ),
        "durability" | "transactions" | "recovery" | "retired" => item(
            "Recovery data from rt versions before notebooks kept .quire",
            path,
            "SQLite and JSON",
            NotMigrated,
            "rt moves this into the notebook's .quire folder when it next opens the notebook. Open each notebook in rt once, then run the migration again.",
        ),
        "Example notebook" => item(
            "rt's sample notebook",
            path,
            "Markdown folder",
            NotNeeded,
            "Quire has its own README sample.",
        ),
        "bridge.json" | "bridge.sock" => item(
            "AI tool server address while rt runs",
            path,
            "runtime",
            NotNeeded,
            "Quire runs its own tool server.",
        ),
        _ if ELECTRON.contains(&name) || name.starts_with("Singleton") => item(
            "Electron's own state",
            path,
            "Chromium",
            NotNeeded,
            "Belongs to rt's runtime.",
        ),
        _ => item(
            "Not recognized",
            path,
            "unknown",
            NotMigrated,
            "Left unchanged.",
        ),
    }
}

fn entries(folder: &Path) -> Vec<(String, PathBuf)> {
    let mut found: Vec<_> = fs::read_dir(folder)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| {
            (
                entry.file_name().to_string_lossy().into_owned(),
                entry.path(),
            )
        })
        .collect();
    found.sort();
    found
}

fn notebook_items(root: &Path) -> Vec<Item> {
    use Disposition::*;
    if root.join("quire-notebook.json").is_file() {
        return vec![
            item(
                "Quire notebook change log: identity, each computer's change files, attachments",
                root.to_owned(),
                "format 1 (zstd JSON batches)",
                Imported,
                "Read into a Quire database; rt's files are only read. Quire adds its own writer folder under devices/ only when you publish, which the rollback step asks you to confirm.",
            ),
            item(
                "Each computer's sync status",
                root.join("devices"),
                "JSON status.json",
                NotNeeded,
                "Rewritten by each app as it publishes.",
            ),
        ];
    }
    let mut items = vec![item(
        "Notes and attachments",
        root.to_owned(),
        "Markdown and other files",
        ReadInPlace,
        "Both apps edit the same files; each checks the file's version before writing.",
    )];
    let metadata = root.join(".quire");
    if metadata.join("notebook.json").is_file() {
        items.push(item(
            "Daily note layout",
            metadata.join("notebook.json"),
            "JSON {version, daily}",
            ReadInPlace,
            "Quire writes it only when you save the layout, in rt's exact format.",
        ));
    }
    for (device, path) in entries(&metadata.join("devices")) {
        if !path.is_dir() {
            continue;
        }
        for (name, path) in entries(&path) {
            let what = format!("rt's records from computer {device}");
            items.push(match name.as_str() {
                "durability.sqlite" | "durability.sqlite-wal" | "durability.sqlite-shm" => item(
                    &format!("{what}: drafts and saved versions"),
                    path,
                    "SQLite (WAL)",
                    Imported,
                    "Drafts and history are copied into Quire's state for this notebook, once each.",
                ),
                "recovery" => item(
                    &format!("{what}: older drafts"),
                    path,
                    "JSON files",
                    Imported,
                    "Imported as drafts for review.",
                ),
                "transactions" => item(
                    &format!("{what}: interrupted multi-note changes"),
                    path,
                    "JSON journals",
                    NotMigrated,
                    "Quire does not finish rt's interrupted changes. Open the notebook in rt once so it finishes them.",
                ),
                "retired" => item(
                    &format!("{what}: originals of moved notes"),
                    path,
                    "Markdown files",
                    NotMigrated,
                    "They stay in the folder; rt's history shows them.",
                ),
                "search.sqlite" | "search.sqlite-wal" | "search.sqlite-shm" => item(
                    &format!("{what}: search index"),
                    path,
                    "SQLite FTS",
                    NotNeeded,
                    "Quire builds its own index.",
                ),
                _ => item(&what, path, "unknown", NotMigrated, "Left unchanged."),
            });
        }
    }
    if metadata.join("history").is_dir() {
        items.push(item(
            "Portable history snapshots",
            metadata.join("history"),
            "SQLite per computer",
            Imported,
            "Saved versions are copied into Quire's history, once each.",
        ));
    }
    if metadata.join("ai").is_dir() {
        items.push(item(
            "AI run transcripts",
            metadata.join("ai"),
            "Markdown",
            NotNeeded,
            "They stay in the folder as rt wrote them.",
        ));
    }
    if root.join(".git").exists() {
        items.push(item(
            "Personal Git repository",
            root.join(".git"),
            "Git",
            ReadInPlace,
            "Quire syncs the same repository.",
        ));
    }
    items
}

/// Everything in `rt_data` and in each notebook in `roots`, classified.
pub fn inventory(rt_data: &Path, roots: &[PathBuf]) -> Vec<Item> {
    let mut items: Vec<Item> = entries(rt_data)
        .into_iter()
        .map(|(name, path)| app_data_item(path, &name))
        .collect();
    for root in roots {
        if root.is_dir() {
            items.extend(notebook_items(root));
        } else {
            items.push(item(
                "A profile's notebook",
                root.clone(),
                "missing",
                Disposition::NotMigrated,
                "The folder is missing. Reconnect it, then run the migration again.",
            ));
        }
    }
    items
}

/// The `rt` backups in a profile's backup folder.
pub(super) fn backup_items(folder: &Path) -> Vec<Item> {
    let quire = folder.join("Quire");
    if !quire.is_dir() {
        return Vec::new();
    }
    vec![item(
        "rt backups",
        quire,
        "rt backup folders with manifest.json",
        Disposition::Restorable,
        "Restore one as a new notebook with --restore-rt-backup; the backup is only read.",
    )]
}
