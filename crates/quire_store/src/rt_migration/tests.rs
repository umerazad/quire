//! Migration from the synthetic `rt` installation in `tests/fixtures/rt-state`, which `rt`'s own
//! writers produced (`tools/rt-state-vectors.ts`). Every test works on a copy and checks, byte
//! for byte, that migration leaves `rt`'s files as they were.

use super::*;
use crate::{
    folder_notebook::{FolderNotebook, revision},
    notebook::{Edit, NotebookStore},
    sync::format::sha256,
};
use std::collections::BTreeMap;

const PLACEHOLDER: &str = "/rt-fixture";

pub(super) struct Fixture {
    _dir: tempfile::TempDir,
    /// The copy of the fixture: `app-data`, the notebooks, and `backups`.
    pub rt: PathBuf,
    pub rt_data: PathBuf,
    pub quire_data: PathBuf,
}

impl Fixture {
    pub fn options(&self) -> Options {
        Options {
            rt_data: self.rt_data.clone(),
            quire_data: self.quire_data.clone(),
            now: 1_790_000_000_000,
        }
    }

    pub fn migrate(&self) -> Report {
        migrate(&self.options()).unwrap()
    }

    pub fn rt_tree(&self) -> BTreeMap<String, String> {
        tree(&self.rt)
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Every file below `root` with the SHA-256 of its bytes.
pub(super) fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut found = BTreeMap::new();
    let mut stack = vec![root.to_owned()];
    while let Some(folder) = stack.pop() {
        for entry in fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                found.insert(relative, sha256(fs::read(&path).unwrap()));
            }
        }
    }
    found
}

/// A copy of the fixture with its placeholder roots pointing at the copy, as if `rt` had made
/// its notebooks there.
pub(super) fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let base = fs::canonicalize(dir.path()).unwrap();
    let rt = base.join("rt");
    copy_dir(
        &Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rt-state"),
        &rt,
    );
    let rt_data = rt.join("app-data");
    let prefix = rt.to_string_lossy().into_owned();
    let profiles = database::open_unconfigured(&rt_data.join("profiles.sqlite")).unwrap();
    profiles
        .exec_bound(
            "UPDATE profiles SET root=replace(root,?1,?2), backupDirectory=replace(backupDirectory,?1,?2)",
        )
        .unwrap()((PLACEHOLDER, prefix.as_str()))
    .unwrap();
    drop(profiles);
    let settings = rt_data.join("settings.json");
    let text = fs::read_to_string(&settings).unwrap();
    fs::write(&settings, text.replace(PLACEHOLDER, &prefix)).unwrap();
    let quire_data = base.join("quire");
    Fixture {
        _dir: dir,
        rt,
        rt_data,
        quire_data,
    }
}

fn check<'a>(notebook: &'a NotebookReport, what: &str) -> &'a Check {
    notebook
        .checks
        .iter()
        .find(|check| check.what.starts_with(what))
        .unwrap_or_else(|| panic!("no check {what} in {notebook:#?}"))
}

fn store(fixture: &Fixture, folder: &str) -> (NotebookStore, PathBuf) {
    let record = Record::load(&fixture.quire_data).unwrap();
    let Some(Migrated::Quire {
        database,
        notebook,
        writer,
        ..
    }) = record.quire(&fixture.rt.join(folder))
    else {
        panic!("{folder} was not migrated: {record:#?}");
    };
    (
        NotebookStore::open(database, notebook, writer).unwrap(),
        database.clone(),
    )
}

#[test]
fn migration_brings_every_notebook_and_profile_into_quire_and_leaves_rt_unchanged() {
    let fixture = fixture();
    let before = fixture.rt_tree();
    let report = fixture.migrate();
    assert_eq!(fixture.rt_tree(), before, "rt's files changed");

    let [folder, quire] = &report.notebooks[..] else {
        panic!("{report:#?}");
    };
    assert_eq!(folder.kind, NotebookKind::Folder);
    assert!(folder.verified(), "{folder:#?}");
    assert_eq!(check(folder, "Notes").rt, check(folder, "Notes").quire);
    assert!(check(folder, "Notes").rt.starts_with("3 · "));
    assert_eq!(check(folder, "Drafts").rt, "1");
    assert_eq!(check(folder, "Saved versions").rt, "1");
    assert!(check(folder, "Attachments").rt.starts_with("1 · "));
    assert_eq!(quire.kind, NotebookKind::Quire);
    assert!(quire.verified(), "{quire:#?}");
    assert_eq!(check(quire, "Pages").rt, check(quire, "Pages").quire);
    assert!(check(quire, "Pages").rt.starts_with("2 · "));
    assert_eq!(
        check(quire, "Drafts").quire,
        "2 imported now, 0 earlier, 0 left in rt"
    );
    let [problem] = &report.problems[..] else {
        panic!("{:#?}", report.problems);
    };
    assert!(
        problem.contains("pending-notebook")
            && problem.contains("1 saved change that never reached the synced folder")
            && problem.contains("Open the notebook in rt"),
        "{problem}"
    );
    assert!(!report.verified());

    let (store, database) = store(&fixture, "quire-notebook");
    let census = store.census().unwrap();
    assert_eq!(census.outbox, 0, "migration publishes nothing");
    assert_eq!(census.attachments, 1);
    let (welcome, heads) = store.page_at("Welcome.md").unwrap().unwrap();
    let draft = store.draft(&welcome).unwrap().unwrap();
    assert_eq!(draft.text, "- Welcome, typed but not saved\n");
    assert_eq!(
        draft.base, heads,
        "a draft typed into a known version keeps it"
    );
    let (today, _) = store.page_at("Tasks/Today.md").unwrap().unwrap();
    assert!(
        store.draft(&today).unwrap().unwrap().base.is_empty(),
        "a draft without a known version is for review"
    );
    let writer = store.device_id().to_owned();
    assert!(
        !fixture
            .rt
            .join("quire-notebook/devices")
            .join(&writer)
            .exists()
    );
    assert_ne!(
        writer,
        fs::read_to_string(fixture.rt_data.join("device-id"))
            .unwrap()
            .trim()
    );
    assert!(database.starts_with(&fixture.quire_data));

    let profiles = Profiles::load(fixture.quire_data.join("profiles.json")).unwrap();
    assert_eq!(profiles.active(), ProfileId::Work);
    assert_eq!(
        profiles.get(ProfileId::Current).notebook,
        Some(Notebook::Folder {
            root: fixture.rt.join("folder-notebook")
        })
    );
    assert_eq!(
        profiles.get(ProfileId::Current).backup_folder,
        Some(fixture.rt.join("backups"))
    );
    assert!(matches!(
        profiles.get(ProfileId::Work).notebook,
        Some(Notebook::Store { .. })
    ));
    assert_eq!(profiles.get(ProfileId::Personal).notebook, None);
    assert_eq!(
        profiles.get(ProfileId::Personal).git_remote,
        None,
        "the remote waits for Personal's notebook"
    );

    let markdown = report.to_markdown();
    for expected in [
        "## Folder notebook",
        "## Quire notebook",
        "| Drafts | 1 |",
        "profiles.sqlite",
        "device-id",
        "not needed",
        "restorable",
    ] {
        assert!(
            markdown.contains(expected),
            "{expected} missing:\n{markdown}"
        );
    }
    assert!(!markdown.contains('\u{2014}'), "no em dashes in the report");
}

#[test]
fn running_the_migration_again_changes_nothing() {
    let fixture = fixture();
    let first = fixture.migrate();
    let record = fs::read(fixture.quire_data.join(RECORD_FILE)).unwrap();
    let profiles = fs::read(fixture.quire_data.join("profiles.json")).unwrap();
    let (store_before, _) = store(&fixture, "quire-notebook");
    let census = store_before.census().unwrap();
    drop(store_before);
    let before = fixture.rt_tree();

    let second = fixture.migrate();
    assert_eq!(fixture.rt_tree(), before);
    assert_eq!(
        fs::read(fixture.quire_data.join(RECORD_FILE)).unwrap(),
        record
    );
    assert_eq!(
        fs::read(fixture.quire_data.join("profiles.json")).unwrap(),
        profiles
    );
    let (store_after, _) = store(&fixture, "quire-notebook");
    assert_eq!(store_after.census().unwrap(), census);
    assert_eq!(
        check(&second.notebooks[1], "Drafts").quire,
        "0 imported now, 2 earlier, 0 left in rt"
    );
    assert_eq!(
        check(&first.notebooks[0], "Notes").quire,
        check(&second.notebooks[0], "Notes").quire
    );
    assert_eq!(
        check(&first.notebooks[1], "Pages").quire,
        check(&second.notebooks[1], "Pages").quire
    );
    assert_eq!(
        second.profiles,
        ["Profiles were imported by an earlier run; Quire's profiles are unchanged."]
    );
}

#[test]
fn an_interrupted_migration_finishes_on_the_next_run_without_repeating_work() {
    let fixture = fixture();
    let quire = fixture.rt.join("quire-notebook");
    let away = fixture.rt.join("quire-notebook-away");
    fs::rename(&quire, &away).unwrap();
    let first = fixture.migrate();
    assert!(
        first
            .problems
            .iter()
            .any(|problem| problem.contains("is missing. Reconnect")),
        "{:#?}",
        first.problems
    );
    let profiles = Profiles::load(fixture.quire_data.join("profiles.json")).unwrap();
    assert_eq!(profiles.get(ProfileId::Work).notebook, None);

    fs::rename(&away, &quire).unwrap();
    // A crash after a draft reached Quire but before the record noted it.
    let second = fixture.migrate();
    let mut record = Record::load(&fixture.quire_data).unwrap();
    record.completed.retain(|key| !key.starts_with("draft:"));
    record.save(&fixture.quire_data).unwrap();
    let (store_before, _) = store(&fixture, "quire-notebook");
    let census = store_before.census().unwrap();
    drop(store_before);
    let third = fixture.migrate();
    let (store_after, _) = store(&fixture, "quire-notebook");
    assert_eq!(store_after.census().unwrap(), census, "nothing doubled");
    assert!(second.notebooks.iter().all(NotebookReport::verified));
    assert!(third.notebooks.iter().all(NotebookReport::verified));
    let profiles = Profiles::load(fixture.quire_data.join("profiles.json")).unwrap();
    assert!(matches!(
        profiles.get(ProfileId::Work).notebook,
        Some(Notebook::Store { .. })
    ));
    assert_eq!(
        profiles.get(ProfileId::Current).notebook,
        Some(Notebook::Folder {
            root: fixture.rt.join("folder-notebook")
        })
    );
}

#[test]
fn damaged_rt_state_is_reported_with_what_to_do_and_left_unchanged() {
    let fixture = fixture();
    fs::write(
        fixture.rt.join("quire-notebook/quire-notebook.json"),
        b"{\"format\":1,",
    )
    .unwrap();
    fs::remove_dir_all(fixture.rt.join("pending-notebook")).unwrap();
    let before = fixture.rt_tree();
    let report = fixture.migrate();
    assert_eq!(fixture.rt_tree(), before);
    let problems = report.problems.join("\n");
    assert!(
        problems
            .contains("is damaged. Restore it from the sync tool's version history or a backup"),
        "{problems}"
    );
    assert!(
        problems.contains("is missing. Reconnect the drive or folder"),
        "{problems}"
    );
    assert_eq!(
        report.notebooks.len(),
        1,
        "the folder notebook still migrates"
    );

    let damaged = super::tests::fixture();
    fs::write(damaged.rt_data.join("profiles.sqlite"), b"not a database").unwrap();
    let _ = fs::remove_file(damaged.rt_data.join("profiles.sqlite-wal"));
    let _ = fs::remove_file(damaged.rt_data.join("profiles.sqlite-shm"));
    let before = damaged.rt_tree();
    let report = damaged.migrate();
    assert_eq!(damaged.rt_tree(), before);
    assert!(report.notebooks.is_empty());
    assert!(
        report.problems[0].contains("rt's profiles could not be read")
            && report.problems[0].contains("repair or restore profiles.sqlite"),
        "{:#?}",
        report.problems
    );

    let missing = super::tests::fixture();
    let error = migrate(&Options {
        rt_data: missing.rt.join("nowhere"),
        ..missing.options()
    })
    .unwrap_err();
    assert!(
        format!("{error:#}")
            .contains("is not rt's data folder. Pass the folder that holds profiles.sqlite")
    );
}

#[test]
fn rt_backups_restore_as_new_notebooks_and_the_backups_stay_unchanged() {
    let fixture = fixture();
    let backups = list_backups(&fixture.rt.join("backups"));
    assert_eq!(
        backups.iter().map(|backup| backup.kind).collect::<Vec<_>>(),
        [RtBackupKind::Quire, RtBackupKind::Folder]
    );
    let parent = fixture.rt.parent().unwrap().join("restored");
    fs::create_dir(&parent).unwrap();
    let before = fixture.rt_tree();
    for backup in &backups {
        let restored = restore_backup(
            &backup.path,
            &parent,
            &fixture.quire_data,
            1_790_000_000_000,
        )
        .unwrap();
        assert!(restored.report.verified(), "{:#?}", restored.report);
        assert!(restored.root.starts_with(&parent));
        match backup.kind {
            RtBackupKind::Folder => {
                let notes = tree(&backup.path.join("notebook"));
                for path in ["Plans.md", "Ideas.md", "Projects/Garden.md"] {
                    assert_eq!(tree(&restored.root).get(path), notes.get(path));
                }
                let state =
                    crate::folder_notebook::state_folder_in(&fixture.quire_data, &restored.root);
                let notebook = FolderNotebook::open(&restored.root, &state).unwrap();
                let page = crate::sync::format::page_id_for_path("Ideas.md");
                let draft = notebook.draft(&page).unwrap().unwrap();
                assert_eq!(draft.text, "- مرحبا 🙂 ideas, unsaved\n");
                assert!(draft.base.is_empty(), "restored drafts are for review");
            }
            RtBackupKind::Quire => {
                let Notebook::Store {
                    database,
                    notebook,
                    writer,
                } = &restored.notebook
                else {
                    panic!("{:?}", restored.notebook);
                };
                let old =
                    quire_notebook::notebook_identity(&fixture.rt.join("quire-notebook")).unwrap();
                assert_ne!(notebook, &old, "a restored notebook gets a new identity");
                let store = NotebookStore::open(database, notebook, writer).unwrap();
                let census = store.census().unwrap();
                assert_eq!(
                    census.pages.keys().collect::<Vec<_>>(),
                    ["Tasks/Today.md", "Welcome.md"]
                );
                assert_eq!(census.drafts.len(), 2);
                for (path, _) in &census.drafts {
                    let (page, _) = store.page_at(path).unwrap().unwrap();
                    assert!(store.draft(&page).unwrap().unwrap().base.is_empty());
                }
                assert_eq!(census.attachments, 1);
            }
        }
    }
    assert_eq!(
        fixture.rt_tree(),
        before,
        "restoring only reads the backups"
    );

    let damaged = &backups[1].path;
    let note = damaged.join("notebook/Plans.md");
    fs::write(&note, b"tampered").unwrap();
    let listed = fs::read_dir(&parent).unwrap().count();
    let error = restore_backup(damaged, &parent, &fixture.quire_data, 1).unwrap_err();
    assert!(
        format!("{error:#}").contains(
            "Backup verification failed: notebook/Plans.md changed. Choose another completed backup"
        ),
        "{error:#}"
    );
    assert_eq!(
        fs::read_dir(&parent).unwrap().count(),
        listed,
        "nothing restored"
    );
}

#[test]
fn after_editing_in_quire_rt_sees_every_save_once_publication_is_confirmed() {
    let fixture = fixture();
    fixture.migrate();
    let before = fixture.rt_tree();

    let folder_root = fixture.rt.join("folder-notebook");
    let state = crate::folder_notebook::state_folder_in(&fixture.quire_data, &folder_root);
    {
        let _lock =
            crate::notebook_lock::NotebookLock::acquire(&crate::notebook_lock::folder_lock(&state))
                .unwrap();
        let notebook = FolderNotebook::open(&folder_root, &state).unwrap();
        let old = fs::read_to_string(folder_root.join("Plans.md")).unwrap();
        notebook
            .save(&Edit {
                page: crate::sync::format::page_id_for_path("Plans.md"),
                base: vec![revision(&old)],
                path: "Plans.md".into(),
                text: format!("{old}- Edited in Quire\n"),
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
    }
    {
        let (store, database) = store(&fixture, "quire-notebook");
        let _lock = crate::notebook_lock::NotebookLock::acquire(&crate::notebook_lock::store_lock(
            &database,
        ))
        .unwrap();
        let (page, heads) = store.page_at("Tasks/Today.md").unwrap().unwrap();
        store
            .save(&Edit {
                page,
                base: heads,
                path: "Tasks/Today.md".into(),
                text: "- [x] Call back, done in Quire\n".into(),
                deleted: false,
                created: 2,
                recovery: None,
            })
            .unwrap();
    }

    let check = prepare_rollback(&fixture.quire_data, false, 3).unwrap();
    assert!(!check.ready());
    assert_eq!(check.published, 0);
    let quire = &check.notebooks[1];
    assert!(!quire.ready);
    assert!(
        quire.lines[0].contains("1 saves made in Quire are not in the synced folder yet")
            && quire.lines[0].contains("--publish")
            && quire.lines[0].contains("cannot be undone"),
        "{:#?}",
        quire.lines
    );
    assert!(check.notebooks[0].ready, "{:#?}", check.notebooks[0]);
    let changed: Vec<String> = diff(&before, &fixture.rt_tree());
    assert_eq!(changed, ["folder-notebook/Plans.md"], "only the saved note");

    let done = prepare_rollback(&fixture.quire_data, true, 4).unwrap();
    assert!(done.ready(), "{}", done.to_markdown());
    assert_eq!(done.published, 1);
    let (store, _) = store(&fixture, "quire-notebook");
    let writer = store.device_id().to_owned();
    let changed = diff(&before, &fixture.rt_tree());
    assert_eq!(changed.len(), 2, "{changed:#?}");
    assert!(changed[1].starts_with(&format!("quire-notebook/devices/{writer}/log/000000001-")));
    assert_eq!(
        fs::read_to_string(folder_root.join("Plans.md"))
            .unwrap()
            .lines()
            .last(),
        Some("- Edited in Quire")
    );

    // A second confirmation publishes nothing new.
    let again = prepare_rollback(&fixture.quire_data, true, 5).unwrap();
    assert_eq!(again.published, 0);
    assert_eq!(diff(&before, &fixture.rt_tree()), changed);

    // `QUIRE_WRITE_RT_ROLLBACK_VECTOR=1` refreshes the notebooks rt's own code reads back in
    // tests/reference/rt-state.test.ts.
    if std::env::var_os("QUIRE_WRITE_RT_ROLLBACK_VECTOR").is_some() {
        let vector = rollback_vector();
        let _ = fs::remove_dir_all(&vector);
        for notebook in ["folder-notebook", "quire-notebook"] {
            copy_dir(&fixture.rt.join(notebook), &vector.join(notebook));
        }
    }
}

fn rollback_vector() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rt-state-rollback")
}

/// The committed notebooks after a Quire edit and confirmed publication: `rt`'s own files are
/// the fixture's byte for byte, plus the edited note and one new writer's change file, which a
/// replica that never saw the notebook reads as Quire saved it.
#[test]
fn the_committed_rollback_vector_adds_only_quire_edits_to_rt_notebooks() {
    let original = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/rt-state");
    let vector = rollback_vector();
    let before = tree(&original);
    let after = tree(&vector);
    let added: Vec<&String> = after
        .iter()
        .filter(|(path, hash)| before.get(*path) != Some(*hash))
        .map(|(path, _)| path)
        .collect();
    let [note, log] = added[..] else {
        panic!("{added:#?}");
    };
    assert_eq!(note, "folder-notebook/Plans.md");
    assert!(log.starts_with("quire-notebook/devices/") && log.contains("/log/000000001-"));
    for path in before
        .keys()
        .filter(|path| path.starts_with("folder-notebook/") || path.starts_with("quire-notebook/"))
    {
        assert!(
            after.contains_key(path),
            "{path} is missing from the vector"
        );
    }

    let copy = tempfile::tempdir().unwrap();
    let folder = fs::canonicalize(copy.path())
        .unwrap()
        .join("quire-notebook");
    copy_dir(&vector.join("quire-notebook"), &folder);
    let notebook = quire_notebook::notebook_identity(&folder).unwrap();
    let replica =
        NotebookStore::open(&copy.path().join("replica.sqlite"), &notebook, "reader").unwrap();
    let report = replica
        .rescan(&folder, crate::notebook::RescanLimits::default(), None)
        .unwrap();
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    let (today, heads) = replica.page_at("Tasks/Today.md").unwrap().unwrap();
    let head = replica.get(&heads[0]).unwrap().unwrap();
    assert_eq!(
        head.root().field("text").as_str(),
        Some("- [x] Call back, done in Quire\n")
    );
    assert!(!today.is_empty());
}

/// Paths added or changed since `before`; nothing may disappear.
fn diff(before: &BTreeMap<String, String>, after: &BTreeMap<String, String>) -> Vec<String> {
    for path in before.keys() {
        assert!(after.contains_key(path), "{path} disappeared");
    }
    after
        .iter()
        .filter(|(path, hash)| before.get(*path) != Some(*hash))
        .map(|(path, _)| path.clone())
        .collect()
}

#[test]
fn a_running_rt_is_refused_before_anything_is_copied() {
    let fixture = fixture();
    let before = fixture.rt_tree();
    let lock = fixture.rt_data.join("SingletonLock");
    std::os::unix::fs::symlink("rt-host-1234", &lock).unwrap();
    let error = format!("{:#}", migrate(&fixture.options()).unwrap_err());
    fs::remove_file(&lock).unwrap();
    assert!(
        error.contains("rt appears to be running") && error.contains("Quit rt"),
        "{error}"
    );
    assert!(!fixture.quire_data.exists(), "nothing was written to Quire");
    assert_eq!(fixture.rt_tree(), before);
}

#[test]
fn a_quire_profile_holding_another_notebook_keeps_it_and_its_backup_folder() {
    let fixture = fixture();
    fs::create_dir_all(&fixture.quire_data).unwrap();
    let own = fixture.rt.join("quire-own-notes");
    fs::create_dir_all(&own).unwrap();
    let mut profiles = Profiles::load(fixture.quire_data.join("profiles.json")).unwrap();
    profiles
        .set_notebook(ProfileId::Current, Notebook::Folder { root: own.clone() })
        .unwrap();
    drop(profiles);

    let report = fixture.migrate();

    let profiles = Profiles::load(fixture.quire_data.join("profiles.json")).unwrap();
    let current = profiles.get(ProfileId::Current);
    assert_eq!(current.notebook, Some(Notebook::Folder { root: own }));
    assert_eq!(current.backup_folder, None, "{:#?}", report.profiles);
    assert_eq!(
        profiles.active(),
        ProfileId::Current,
        "Quire's choice stays"
    );
    assert!(
        report
            .profiles
            .iter()
            .any(|line| line.contains("kept Quire's notebook")),
        "{:#?}",
        report.profiles
    );
    assert!(
        report
            .profiles
            .iter()
            .any(|line| line.contains("backup folder") && line.contains("holds another notebook")),
        "{:#?}",
        report.profiles
    );
}
