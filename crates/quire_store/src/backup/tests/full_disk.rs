//! Backups and restores on a drive that fills up: each write fails as a full drive does, and
//! nothing partial is left, listed, or changed in the notebook.

use super::*;

/// Runs `work` on a drive with `free` bytes left for the files backups and restores copy.
fn with_free_space<T>(free: u64, work: impl FnOnce() -> T) -> T {
    files::FREE_SPACE.set(Some(free));
    let result = work();
    files::FREE_SPACE.set(None);
    result
}

/// Room for everything these fixtures write, so a sweep ends with a success.
const PLENTY: u64 = 1 << 30;

#[test]
fn a_full_drive_fails_a_store_backup_with_nothing_partial_listed_and_the_notebook_unchanged() {
    let fixture = StoreFixture::new();
    let first = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let original = tree(fixture.database.parent().unwrap());
    let parent = fixture
        .source()
        .backups_in(&fs::canonicalize(fixture.destination.path()).unwrap())
        .unwrap();
    let mut failures = 0;
    for (index, free) in [0, 1, 4, 9, 100, PLENTY].into_iter().enumerate() {
        let created = 10 + index as u64;
        match with_free_space(free, || {
            create(&fixture.source(), fixture.destination.path(), created)
        }) {
            Ok(backup) => {
                assert_eq!(free, PLENTY, "{free} bytes cannot hold a backup");
                verify(&backup.path).unwrap();
            }
            Err(error) => {
                failures += 1;
                let text = format!("{error:#}");
                assert!(
                    text.contains("The drive is full. Free space on it"),
                    "{text}"
                );
                assert_eq!(names(&parent), std::slice::from_ref(&first.id), "{free}");
            }
        }
        assert_eq!(tree(fixture.database.parent().unwrap()), original, "{free}");
    }
    assert_eq!(failures, 5);
    verify(&first.path).unwrap();
    save(
        &fixture.open(),
        "p-b",
        "B.md",
        "- written after the drive filled",
    );
}

#[test]
fn a_full_drive_during_a_store_restore_creates_nothing_and_keeps_every_backup() {
    let fixture = StoreFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let original = tree(fixture.database.parent().unwrap());
    for (index, free) in [0, 2, 20].into_iter().enumerate() {
        let parent = tempfile::tempdir().unwrap();
        let text = error_text(with_free_space(free, || {
            restore(
                &RestoreRequest {
                    backup: &backup.path,
                    parent: parent.path(),
                    current: None,
                    state_for: &no_state,
                    created: 10 + index as u64,
                },
                &mut |_| Ok(()),
            )
        }));
        assert!(text.contains("The drive is full"), "{text}");
        assert!(text.contains("current notebook is unchanged"), "{text}");
        assert!(names(parent.path()).is_empty());
        assert_eq!(tree(fixture.database.parent().unwrap()), original);
    }
    verify(&backup.path).unwrap();
}

#[test]
fn a_full_drive_fails_a_folder_backup_and_a_restore_without_leaving_anything_behind() {
    let fixture = FolderFixture::new();
    let backup = create(&fixture.source(), fixture.destination.path(), 1).unwrap();
    let notes = tree(fixture.root.path());
    let state = tree(fixture.state.path());
    for free in [0, 3, 12] {
        let text = error_text(with_free_space(free, || {
            create(&fixture.source(), fixture.destination.path(), 5)
        }));
        assert!(text.contains("The drive is full"), "{text}");
        assert_eq!(
            list(&fixture.source(), fixture.destination.path())
                .unwrap()
                .len(),
            1
        );
    }
    let state_for = fixture.state_for();
    let mut restored = None;
    let mut failures = 0;
    for (index, free) in [0, 7, 30, 200, 2_000, PLENTY].into_iter().enumerate() {
        let parent = tempfile::tempdir().unwrap();
        let outcome = with_free_space(free, || {
            restore(
                &RestoreRequest {
                    backup: &backup.path,
                    parent: parent.path(),
                    current: Some((&fixture.source(), fixture.destination.path())),
                    state_for: &state_for,
                    created: 10 + index as u64,
                },
                &mut |_| Ok(()),
            )
        });
        match outcome {
            Ok(done) => {
                assert_eq!(free, PLENTY, "{free} bytes cannot hold a restore");
                restored = Some((parent, done));
            }
            Err(error) => {
                failures += 1;
                let text = format!("{error:#}");
                assert!(text.contains("The drive is full"), "{free}: {text}");
                assert!(
                    text.contains("current notebook is unchanged")
                        || text.contains("nothing was restored"),
                    "{free}: {text}"
                );
                assert!(names(parent.path()).is_empty(), "{free}");
                assert!(
                    names(&fixture.states.path().join("folder-notebooks"))
                        .iter()
                        .all(|name| !name.starts_with(".restoring-")),
                    "{free}"
                );
            }
        }
        assert_eq!(tree(fixture.root.path()), notes, "{free}");
        assert_eq!(tree(fixture.state.path()), state, "{free}");
    }
    assert!(failures >= 4, "only {failures} restores ran out of space");
    let (_parent, done) = restored.expect("a restore with room finishes");
    let Location::Folder { root, .. } = done.location else {
        panic!("a folder notebook restores as a folder notebook");
    };
    assert_eq!(fs::read(root.join("A.md")).unwrap(), b"- second");
    for backup in list(&fixture.source(), fixture.destination.path()).unwrap() {
        verify(&backup.path).unwrap();
    }
}
