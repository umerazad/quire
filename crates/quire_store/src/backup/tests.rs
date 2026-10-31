use super::*;
use crate::{
    folder_notebook::FolderNotebook,
    notebook::{DraftStamp, Edit, NotebookStore},
    sync::format::page_id_for_path,
};
use sqlez::connection::Connection;

mod full_disk;
mod queries;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

fn edit(page: &str, path: &str, text: &str, base: Vec<String>) -> Edit {
    Edit {
        page: page.into(),
        base,
        path: path.into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

fn save(store: &NotebookStore, page: &str, path: &str, text: &str) -> String {
    store
        .save(&edit(page, path, text, store.heads(page).unwrap()))
        .unwrap()
        .revision_id()
        .to_owned()
}

fn draft(page: &str, path: &str, text: &str, base: Vec<String>) -> Draft {
    Draft {
        page: page.into(),
        path: path.into(),
        text: text.into(),
        base,
        stamp: DraftStamp {
            session: "editor".into(),
            version: 1,
        },
        created: 2,
    }
}

/// Every file below `root` with its bytes, by `/`-separated relative path.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    walk(root, &|_| false)
        .unwrap()
        .into_iter()
        .map(|path| {
            let bytes = fs::read(below(root, &path)).unwrap();
            (path, bytes)
        })
        .collect()
}

fn copy_tree(from: &Path, to: &Path) {
    for (path, bytes) in tree(from) {
        let target = below(to, &path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, bytes).unwrap();
    }
}

fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    names
}

fn error_text(result: Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.expect_err("the operation must fail"))
}

struct StoreFixture {
    _root: tempfile::TempDir,
    database: PathBuf,
    destination: tempfile::TempDir,
    attachment: String,
    unreferenced: String,
}

impl StoreFixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let database = root.path().join("notebook").join("local.sqlite");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        let store = NotebookStore::open(&database, "nb", "office").unwrap();
        let attachment = store.attach(b"png bytes", "photo.png").unwrap();
        let unreferenced = store.attach(b"kept without a link", "notes.pdf").unwrap();
        save(&store, "p-a", "A.md", "- first");
        let first = save(
            &store,
            "p-a",
            "A.md",
            &format!("- second ![]({attachment})"),
        );
        save(&store, "p-b", "B.md", "- other");
        store
            .put_draft(&draft("p-a", "A.md", "- typed later", vec![first]), None)
            .unwrap();
        store
            .put_draft(&draft("p-new", "New.md", "- never saved", vec![]), None)
            .unwrap();
        Self {
            _root: root,
            database,
            destination: tempfile::tempdir().unwrap(),
            attachment,
            unreferenced,
        }
    }

    fn source(&self) -> Source {
        Source::Store {
            database: self.database.clone(),
        }
    }

    fn open(&self) -> NotebookStore {
        NotebookStore::open(&self.database, "nb", "office").unwrap()
    }
}

#[test]
fn backups_of_an_open_store_stay_consistent_while_another_thread_writes() {
    let fixture = StoreFixture::new();
    let live = fixture.open();
    let before = live.outbox().unwrap().len();
    let stop = Arc::new(AtomicBool::new(false));
    let saved = Arc::new(AtomicUsize::new(0));
    let writer = thread::spawn({
        let (database, stop, saved) = (fixture.database.clone(), stop.clone(), saved.clone());
        move || -> Result<()> {
            let store = NotebookStore::open(&database, "nb", "office")?;
            let mut round = 0;
            while !stop.load(Ordering::Relaxed) || round < 20 {
                let page = format!("p-w{}", round % 7);
                let base = store.heads(&page)?;
                store.save(&edit(
                    &page,
                    &format!("W{}.md", round % 7),
                    &format!("- round {round}"),
                    base,
                ))?;
                saved.fetch_add(1, Ordering::Relaxed);
                round += 1;
            }
            Ok(())
        }
    });
    while saved.load(Ordering::Relaxed) < 5 {
        thread::yield_now();
    }
    let backups: Vec<_> = (0..3)
        .map(|index| create(&fixture.source(), fixture.destination.path(), 1_000 + index).unwrap())
        .collect();
    let during = saved.load(Ordering::Relaxed);
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap().unwrap();
    let after = live.outbox().unwrap().len();
    assert!(during >= 5, "the writer must save while backups run");
    for backup in &backups {
        let manifest = verify(&backup.path).unwrap();
        assert!(
            !tree(&backup.path)
                .keys()
                .any(|path| path.ends_with("-wal") || path.ends_with("-shm"))
        );
        let copy = tempfile::tempdir().unwrap();
        fs::copy(
            backup.path.join("local.sqlite"),
            copy.path().join("local.sqlite"),
        )
        .unwrap();
        let snapshot =
            NotebookStore::open(&copy.path().join("local.sqlite"), "nb", "office").unwrap();
        let revisions = snapshot.outbox().unwrap().len();
        assert!(
            (before..=after).contains(&revisions),
            "{before} <= {revisions} <= {after}"
        );
        assert_eq!(
            manifest.databases["local.sqlite"]["outbox"] as usize,
            revisions
        );
        for round in 0..7 {
            let page = format!("p-w{round}");
            for head in snapshot.heads(&page).unwrap() {
                assert!(
                    snapshot.get(&head).unwrap().is_some(),
                    "{page} head {head} is in the snapshot"
                );
            }
        }
        assert_eq!(
            snapshot.draft("p-new").unwrap().unwrap().text,
            "- never saved"
        );
    }
    assert_eq!(
        list(&fixture.source(), fixture.destination.path())
            .unwrap()
            .len(),
        3
    );
}

#[test]
fn a_backup_holds_every_cataloged_attachment_and_verifying_changes_nothing() {
    let fixture = StoreFixture::new();
    let _live = fixture.open();
    let backup = create(&fixture.source(), fixture.destination.path(), 5).unwrap();
    let files = tree(&backup.path);
    for reference in [&fixture.attachment, &fixture.unreferenced] {
        assert_eq!(
            files[reference.as_str()],
            fs::read(fixture.database.parent().unwrap().join(reference)).unwrap()
        );
    }
    assert!(!files.contains_key("query-index.sqlite"));
    let manifest = verify(&backup.path).unwrap();
    assert_eq!(manifest.kind, Kind::Store);
    assert_eq!(manifest.notebook.as_deref(), Some("nb"));
    assert_eq!(manifest.databases["local.sqlite"]["drafts"], 2);
    assert_eq!(manifest.databases["local.sqlite"]["attachments"], 2);
    assert_eq!(
        tree(&backup.path),
        files,
        "verification must not change a backup"
    );
    assert!(
        backup.id.starts_with("1970-01-01T00-00-00-005Z-"),
        "{}",
        backup.id
    );
}

#[test]
fn verification_reports_flipped_bytes_missing_or_extra_files_and_damaged_records() {
    let fixture = StoreFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let damaged = |damage: &dyn Fn(&Path)| {
        let copy = tempfile::tempdir().unwrap();
        copy_tree(&backup.path, copy.path());
        damage(copy.path());
        error_text(verify(copy.path()))
    };
    let flip = |path: PathBuf, offset: usize| {
        let mut bytes = fs::read(&path).unwrap();
        bytes[offset] ^= 0x40;
        fs::write(path, bytes).unwrap();
    };
    let attachment = fixture.attachment.clone();
    let text = damaged(&|copy| flip(copy.join(&attachment), 0));
    assert!(
        text.contains(&format!("{attachment} in")) && text.contains("changed since"),
        "{text}"
    );
    let text = damaged(&|copy| fs::remove_file(copy.join(&attachment)).unwrap());
    assert!(text.contains(&format!("{attachment} is missing")), "{text}");
    let text = damaged(&|copy| flip(copy.join("local.sqlite"), 3));
    assert!(
        text.contains("local.sqlite") && text.contains("changed since"),
        "{text}"
    );
    let text = damaged(&|copy| fs::write(copy.join("attachments/stray.txt"), "x").unwrap());
    assert!(
        text.contains("unexpected file attachments/stray.txt"),
        "{text}"
    );
    let text = damaged(&|copy| fs::remove_file(copy.join("manifest.json")).unwrap());
    assert!(text.contains("has no manifest"), "{text}");

    let rewrite_manifest = |copy: &Path, change: &dyn Fn(&mut Manifest)| {
        let path = copy.join("manifest.json");
        let mut manifest: Manifest = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut manifest);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    };
    let text = damaged(&|copy| {
        rewrite_manifest(copy, &|manifest| {
            *manifest
                .databases
                .get_mut("local.sqlite")
                .unwrap()
                .get_mut("revisions")
                .unwrap() += 1
        })
    });
    assert!(text.contains("rows in local.sqlite differ"), "{text}");
    let text = damaged(&|copy| {
        let database = copy.join("local.sqlite");
        let connection = Connection::try_open_file(database.to_str().unwrap()).unwrap();
        connection
            .exec("UPDATE revisions SET body=x'28b52ffd00' WHERE page='\"p-b\"'")
            .unwrap()()
        .unwrap();
        drop(connection);
        let (hash, size) = hash_file(&database).unwrap();
        rewrite_manifest(copy, &|manifest| {
            let entry = manifest
                .files
                .iter_mut()
                .find(|entry| entry.path == "local.sqlite")
                .unwrap();
            entry.hash = hash.clone();
            entry.size = size;
        })
    });
    assert!(
        text.contains("revision") && text.contains("is damaged"),
        "{text}"
    );
}

#[test]
fn failed_backups_keep_every_completed_backup_and_never_list_partial_copies() {
    let fixture = StoreFixture::new();
    let first = create(&fixture.source(), fixture.destination.path(), 1_000).unwrap();
    let second = create(&fixture.source(), fixture.destination.path(), 2_000).unwrap();
    let parent = fixture
        .source()
        .backups_in(&fs::canonicalize(fixture.destination.path()).unwrap())
        .unwrap();
    fs::create_dir(parent.join(".partial-crashed")).unwrap();
    fs::create_dir(parent.join("no-manifest")).unwrap();
    let cached = fixture.database.parent().unwrap().join(&fixture.attachment);
    fs::write(&cached, b"damaged in the cache").unwrap();
    let text = error_text(create(&fixture.source(), fixture.destination.path(), 3_000));
    assert!(text.contains("differs from its catalog entry"), "{text}");
    let listed = list(&fixture.source(), fixture.destination.path()).unwrap();
    assert_eq!(
        listed.iter().map(|backup| &backup.id).collect::<Vec<_>>(),
        [&second.id, &first.id]
    );
    for backup in &listed {
        verify(&backup.path).unwrap();
    }
    assert_eq!(
        names(&parent),
        [
            ".partial-crashed".to_owned(),
            first.id,
            second.id,
            "no-manifest".into()
        ]
    );
}

#[test]
fn backup_folders_inside_the_notebook_are_refused() {
    let fixture = StoreFixture::new();
    let inside = fixture.database.parent().unwrap().join("backups");
    fs::create_dir(&inside).unwrap();
    let text = error_text(create(&fixture.source(), &inside, 1));
    assert!(text.contains("overlaps the notebook's own data"), "{text}");
    assert!(names(&inside).is_empty());
}

fn no_state(_: &Path) -> PathBuf {
    unreachable!("a Quire notebook restore has no state folder")
}

#[test]
fn a_restored_store_is_a_new_notebook_with_the_same_pages_history_drafts_and_attachments() {
    let fixture = StoreFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let original = fixture.open();
    let restored = restore(
        &RestoreRequest {
            backup: &backup.path,
            parent: parent.path(),
            current: Some((&fixture.source(), fixture.destination.path())),
            state_for: &no_state,
            created: 2,
        },
        &mut |_| Ok(()),
    )
    .unwrap();
    let safety = restored
        .safety
        .clone()
        .expect("the current notebook is backed up first");
    verify(&safety.path).unwrap();
    let Location::Store {
        database,
        notebook,
        writer,
    } = restored.location
    else {
        panic!("a Quire notebook restores as a Quire notebook");
    };
    assert_ne!(notebook, "nb");
    assert_eq!(writer, "office");
    assert!(database.starts_with(fs::canonicalize(parent.path()).unwrap()));
    assert!(
        database
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("Restored ")
    );
    assert!(
        NotebookStore::open(&database, "nb", "office").is_err(),
        "the old identity must not open the copy"
    );
    let copy = NotebookStore::open(&database, &notebook, &writer).unwrap();
    for page in ["p-a", "p-b"] {
        assert_eq!(copy.heads(page).unwrap(), original.heads(page).unwrap());
        assert_eq!(copy.history(page).unwrap(), original.history(page).unwrap());
        let head = &copy.heads(page).unwrap()[0];
        assert_eq!(
            copy.get(head).unwrap().unwrap().canonical().unwrap(),
            original.get(head).unwrap().unwrap().canonical().unwrap()
        );
    }
    let mut revisions = copy.outbox().unwrap();
    revisions.sort();
    assert_eq!(revisions.len(), 3);
    for (page, text) in [("p-a", "- typed later"), ("p-new", "- never saved")] {
        let draft = copy.draft(page).unwrap().unwrap();
        assert_eq!(draft.text, text);
        assert!(draft.base.is_empty(), "restored drafts wait for review");
    }
    for reference in [&fixture.attachment, &fixture.unreferenced] {
        assert_eq!(
            fs::read(database.parent().unwrap().join(reference)).unwrap(),
            fs::read(fixture.database.parent().unwrap().join(reference)).unwrap()
        );
    }
    assert_eq!(
        save(&original, "p-b", "B.md", "- still writable"),
        original.heads("p-b").unwrap()[0]
    );
    save(&copy, "p-b", "B.md", "- restored copy writes");
    assert_eq!(
        original
            .get(&original.heads("p-b").unwrap()[0])
            .unwrap()
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("- still writable")
    );
}

#[test]
fn a_restore_interrupted_at_any_step_leaves_the_original_and_creates_nothing() {
    let fixture = StoreFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let original_files = tree(fixture.database.parent().unwrap());
    for (index, stop) in [
        RestoreStep::SafetyBackup,
        RestoreStep::Copied,
        RestoreStep::Adopted,
        RestoreStep::Verified,
        RestoreStep::Swapped,
    ]
    .into_iter()
    .enumerate()
    {
        let parent = tempfile::tempdir().unwrap();
        let text = error_text(restore(
            &RestoreRequest {
                backup: &backup.path,
                parent: parent.path(),
                current: Some((&fixture.source(), fixture.destination.path())),
                state_for: &no_state,
                created: 10 + index as u64,
            },
            &mut |step| {
                anyhow::ensure!(step != stop, "interrupted at {step:?}");
                Ok(())
            },
        ));
        assert!(
            text.contains("current notebook is unchanged"),
            "{stop:?}: {text}"
        );
        assert!(
            names(parent.path()).is_empty(),
            "{stop:?} left {:?}",
            names(parent.path())
        );
        assert_eq!(
            tree(fixture.database.parent().unwrap()),
            original_files,
            "{stop:?}"
        );
        let store = fixture.open();
        assert_eq!(store.draft("p-a").unwrap().unwrap().text, "- typed later");
        drop(store);
    }
    let listed = list(&fixture.source(), fixture.destination.path()).unwrap();
    assert_eq!(
        listed.len(),
        6,
        "the first backup and one safety backup per attempt"
    );
    assert!(listed.iter().all(|backup| verify(&backup.path).is_ok()));
    let store = fixture.open();
    save(
        &store,
        "p-b",
        "B.md",
        "- written after every interrupted restore",
    );
}

#[test]
fn a_damaged_backup_is_refused_before_the_safety_backup_or_any_copy() {
    let fixture = StoreFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    fs::remove_file(backup.path.join(&fixture.unreferenced)).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let text = error_text(restore(
        &RestoreRequest {
            backup: &backup.path,
            parent: parent.path(),
            current: Some((&fixture.source(), fixture.destination.path())),
            state_for: &no_state,
            created: 2,
        },
        &mut |_| Ok(()),
    ));
    assert!(text.contains("is missing"), "{text}");
    assert!(names(parent.path()).is_empty());
    assert_eq!(
        list(&fixture.source(), fixture.destination.path())
            .unwrap()
            .len(),
        1
    );
    let inside = fixture.database.parent().unwrap().to_path_buf();
    fs::write(
        backup.path.join(&fixture.unreferenced),
        b"kept without a link",
    )
    .unwrap();
    let text = error_text(restore(
        &RestoreRequest {
            backup: &backup.path,
            parent: &inside,
            current: Some((&fixture.source(), fixture.destination.path())),
            state_for: &no_state,
            created: 3,
        },
        &mut |_| Ok(()),
    ));
    assert!(text.contains("overlaps the notebook's own data"), "{text}");
}

struct FolderFixture {
    root: tempfile::TempDir,
    state: tempfile::TempDir,
    destination: tempfile::TempDir,
    states: tempfile::TempDir,
}

impl FolderFixture {
    fn new() -> Self {
        let fixture = Self {
            root: tempfile::tempdir().unwrap(),
            state: tempfile::tempdir().unwrap(),
            destination: tempfile::tempdir().unwrap(),
            states: tempfile::tempdir().unwrap(),
        };
        for (path, bytes) in [
            ("A.md", &b"- first"[..]),
            ("People/B.md", b"- other"),
            ("assets/photo.png", b"\x89PNG"),
            (".git/HEAD", b"ref: refs/heads/main"),
            (".quire/notebook.json", b"{\"version\":1}"),
        ] {
            let target = fixture.root.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, bytes).unwrap();
        }
        let notebook = fixture.open();
        let base = notebook.heads(&page_id_for_path("A.md")).unwrap();
        notebook
            .save(&Edit {
                page: page_id_for_path("A.md"),
                base: base.clone(),
                path: "A.md".into(),
                text: "- second".into(),
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
        let base = notebook.heads(&page_id_for_path("A.md")).unwrap();
        notebook
            .put_draft(
                &draft(&page_id_for_path("A.md"), "A.md", "- typed", base),
                None,
            )
            .unwrap();
        fixture
    }

    fn open(&self) -> FolderNotebook {
        FolderNotebook::open(self.root.path(), self.state.path()).unwrap()
    }

    fn source(&self) -> Source {
        Source::Folder {
            root: self.root.path().to_owned(),
            state: self.state.path().to_owned(),
        }
    }

    fn state_for(&self) -> impl Fn(&Path) -> PathBuf + '_ {
        |root: &Path| {
            self.states
                .path()
                .join("folder-notebooks")
                .join(&sha256(root.to_string_lossy().as_bytes())[..16])
        }
    }
}

#[test]
fn folder_backups_copy_notes_state_and_live_reference_databases_consistently() {
    let fixture = FolderFixture::new();
    let device = fixture.root.path().join(".quire/devices/laptop");
    fs::create_dir_all(&device).unwrap();
    let reference =
        Connection::try_open_file(device.join("durability.sqlite").to_str().unwrap()).unwrap();
    reference.exec("PRAGMA journal_mode=WAL").unwrap()().unwrap();
    reference.exec("PRAGMA wal_autocheckpoint=0").unwrap()().unwrap();
    reference.exec("CREATE TABLE pending (path TEXT)").unwrap()().unwrap();
    reference
        .exec("INSERT INTO pending VALUES ('A.md')")
        .unwrap()()
    .unwrap();
    assert!(
        device.join("durability.sqlite-wal").exists(),
        "the reference database is live in WAL mode"
    );
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let manifest = verify(&backup.path).unwrap();
    let files = tree(&backup.path);
    for path in [
        "notebook/A.md",
        "notebook/People/B.md",
        "notebook/assets/photo.png",
        "notebook/.quire/notebook.json",
        "state/folder.sqlite",
    ] {
        assert!(files.contains_key(path), "{path} is backed up");
    }
    assert_eq!(files["notebook/A.md"], b"- second");
    assert!(!files.keys().any(|path| path.contains(".git/")
        || path.ends_with("-wal")
        || path.contains("query-index")));
    assert_eq!(
        manifest.databases["notebook/.quire/devices/laptop/durability.sqlite"]["pending"],
        1
    );
    assert_eq!(manifest.databases["state/folder.sqlite"]["drafts"], 1);
    assert_eq!(manifest.notebook, None);
    drop(reference);
}

#[test]
fn folder_backups_fail_rather_than_mix_versions_when_a_note_changes_meanwhile() {
    let fixture = FolderFixture::new();
    let stop = Arc::new(AtomicBool::new(false));
    let note = fixture.root.path().join("Busy.md");
    let writer = thread::spawn({
        let (note, stop) = (note.clone(), stop.clone());
        move || {
            let staged = note.with_extension("md.tmp");
            let mut round = 0u64;
            while !stop.load(Ordering::Relaxed) {
                fs::write(&staged, format!("- {}", round.to_string().repeat(4096))).unwrap();
                fs::rename(&staged, &note).unwrap();
                round += 1;
            }
        }
    });
    let results: Vec<_> = (0..6)
        .map(|index| create(&fixture.source(), fixture.destination.path(), index))
        .collect();
    stop.store(true, Ordering::Relaxed);
    writer.join().unwrap();
    for result in results {
        match result {
            Ok(backup) => {
                verify(&backup.path).unwrap();
                let text =
                    String::from_utf8(tree(&backup.path)["notebook/Busy.md"].clone()).unwrap();
                let digits = text.trim_start_matches("- ");
                assert!(
                    digits.len().is_multiple_of(4096) && !digits.is_empty(),
                    "a whole version"
                );
            }
            Err(error) => assert!(
                format!("{error:#}").contains("changed while it was being backed up"),
                "{error:#}"
            ),
        }
    }
    let parent = fixture
        .source()
        .backups_in(&fs::canonicalize(fixture.destination.path()).unwrap())
        .unwrap();
    assert!(
        !names(&parent).iter().any(|name| name.starts_with('.')),
        "no partial copy is left"
    );
}

#[test]
fn a_restored_folder_notebook_keeps_notes_history_and_drafts_for_review_in_a_new_folder() {
    let fixture = FolderFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let state_for = fixture.state_for();
    let restored = restore(
        &RestoreRequest {
            backup: &backup.path,
            parent: parent.path(),
            current: Some((&fixture.source(), fixture.destination.path())),
            state_for: &state_for,
            created: 2,
        },
        &mut |_| Ok(()),
    )
    .unwrap();
    let Location::Folder { root, state } = restored.location else {
        panic!("a folder notebook restores as a folder notebook");
    };
    assert_eq!(state, state_for(&root));
    let original = fixture.open();
    let notebook = FolderNotebook::open(&root, &state).unwrap();
    let page = page_id_for_path("A.md");
    assert_eq!(fs::read(root.join("A.md")).unwrap(), b"- second");
    assert_eq!(fs::read(root.join("assets/photo.png")).unwrap(), b"\x89PNG");
    assert!(!root.join(".git").exists());
    assert_eq!(
        notebook.history(&page).unwrap().len(),
        original.history(&page).unwrap().len()
    );
    assert_eq!(notebook.history(&page).unwrap().len(), 1);
    let restored_draft = notebook.draft(&page).unwrap().unwrap();
    assert_eq!(restored_draft.text, "- typed");
    assert!(
        restored_draft.base.is_empty(),
        "restored drafts wait for review"
    );
    assert!(
        !original.draft(&page).unwrap().unwrap().base.is_empty(),
        "the original draft is unchanged"
    );
}

#[test]
fn a_folder_restore_interrupted_at_any_step_leaves_no_folder_or_state_behind() {
    let fixture = FolderFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let before = tree(fixture.root.path());
    let state_for = fixture.state_for();
    for stop in [
        RestoreStep::Copied,
        RestoreStep::Adopted,
        RestoreStep::Verified,
        RestoreStep::Swapped,
    ] {
        let parent = tempfile::tempdir().unwrap();
        let text = error_text(restore(
            &RestoreRequest {
                backup: &backup.path,
                parent: parent.path(),
                current: None,
                state_for: &state_for,
                created: 2,
            },
            &mut |step| {
                anyhow::ensure!(step != stop, "interrupted at {step:?}");
                Ok(())
            },
        ));
        assert!(
            text.contains("current notebook is unchanged"),
            "{stop:?}: {text}"
        );
        assert!(names(parent.path()).is_empty(), "{stop:?}");
        assert!(
            names(&fixture.states.path().join("folder-notebooks")).is_empty(),
            "{stop:?} left state {:?}",
            names(&fixture.states.path().join("folder-notebooks"))
        );
    }
    assert_eq!(tree(fixture.root.path()), before);
    let notebook = fixture.open();
    assert_eq!(
        notebook
            .draft(&page_id_for_path("A.md"))
            .unwrap()
            .unwrap()
            .text,
        "- typed"
    );
}

#[test]
fn stamps_are_utc_dates_that_sort_by_time() {
    assert_eq!(stamp(0), "1970-01-01T00-00-00-000Z");
    assert_eq!(stamp(1_793_455_385_123), "2026-10-31T14-03-05-123Z");
    assert_eq!(stamp(951_782_400_000), "2000-02-29T00-00-00-000Z");
}

/// Whether a scheduled backup at `created` made one, and the number of completed backups after.
fn scheduled(source: &Source, destination: &Path, created: u64) -> (bool, usize) {
    let made = create_if_changed(source, destination, created).unwrap();
    if let Some(made) = &made {
        verify(&made.path).unwrap();
    }
    (made.is_some(), list(source, destination).unwrap().len())
}

#[test]
fn scheduled_store_backups_skip_an_unchanged_notebook_and_follow_saves_and_attachments() {
    let fixture = StoreFixture::new();
    let live = fixture.open();
    let (source, destination) = (fixture.source(), fixture.destination.path());
    assert_eq!(scheduled(&source, destination, 1), (true, 1));
    assert_eq!(scheduled(&source, destination, 2), (false, 1), "unchanged");
    save(&live, "p-b", "B.md", "- other, edited");
    assert_eq!(
        scheduled(&source, destination, 3),
        (true, 2),
        "after a save"
    );
    assert_eq!(scheduled(&source, destination, 4), (false, 2));
    live.attach(b"new bytes", "added.png").unwrap();
    assert_eq!(
        scheduled(&source, destination, 5),
        (true, 3),
        "after an attachment"
    );
    live.put_draft(&draft("p-b", "B.md", "- typing", vec![]), None)
        .unwrap();
    assert_eq!(
        scheduled(&source, destination, 6),
        (true, 4),
        "after a draft"
    );
    assert_eq!(scheduled(&source, destination, 7), (false, 4));
    create(&source, destination, 8).unwrap();
    assert_eq!(
        list(&source, destination).unwrap().len(),
        5,
        "back up now always backs up"
    );
    let newest = list(&source, destination).unwrap().remove(0);
    fs::remove_file(newest.path.join(&fixture.attachment)).unwrap();
    assert_eq!(
        scheduled(&source, destination, 9),
        (true, 6),
        "a newest backup missing a file is not trusted"
    );
    assert_eq!(scheduled(&source, destination, 10), (false, 6));
}

#[test]
fn scheduled_folder_backups_follow_external_edits_new_files_and_state_changes() {
    let fixture = FolderFixture::new();
    let notebook = fixture.open();
    let (source, destination) = (fixture.source(), fixture.destination.path());
    assert_eq!(scheduled(&source, destination, 1), (true, 1));
    assert_eq!(scheduled(&source, destination, 2), (false, 1), "unchanged");
    fs::write(fixture.root.path().join("A.md"), "- secoND").unwrap();
    assert_eq!(
        scheduled(&source, destination, 3),
        (true, 2),
        "after another application rewrites a note in place at the same size"
    );
    let newest = list(&source, destination).unwrap().remove(0);
    assert_eq!(
        fs::read(newest.path.join("notebook/A.md")).unwrap(),
        b"- secoND"
    );
    fs::write(fixture.root.path().join("People/C.md"), "- new").unwrap();
    assert_eq!(
        scheduled(&source, destination, 4),
        (true, 3),
        "after a new note"
    );
    fs::write(
        fixture.root.path().join(".git").join("HEAD"),
        "ref: refs/heads/other",
    )
    .unwrap();
    assert_eq!(
        scheduled(&source, destination, 5),
        (false, 3),
        "repository data is not backed up"
    );
    let base = notebook.heads(&page_id_for_path("A.md")).unwrap();
    notebook
        .put_draft(
            &Draft {
                stamp: DraftStamp {
                    session: "editor".into(),
                    version: 2,
                },
                ..draft(&page_id_for_path("A.md"), "A.md", "- typed again", base)
            },
            None,
        )
        .unwrap();
    assert_eq!(
        scheduled(&source, destination, 6),
        (true, 4),
        "after a recovery draft"
    );
    assert_eq!(scheduled(&source, destination, 7), (false, 4));
    create(&source, destination, 8).unwrap();
    assert_eq!(
        list(&source, destination).unwrap().len(),
        5,
        "back up now always backs up"
    );
}
