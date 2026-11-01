use super::tests::{Fixture, edit, fixture};
use super::*;
use std::fs;

const A: &str = "- a ![i](img.png)\n";
const B: &str = "- b [[Projects/A]]\n";
const TOP: &str = "- [[Projects/A]] ![p](Projects/img.png)\n";
const PROJECT: [(&str, &str); 5] = [
    ("Projects/A.md", A),
    ("Projects/B.md", B),
    ("Projects/img.png", "png"),
    ("Projects/Scans/receipt.pdf", "pdf"),
    ("Top.md", TOP),
];

fn project() -> Fixture {
    let opened = fixture(&PROJECT);
    fs::create_dir(opened.folder.join("Projects/Empty")).unwrap();
    opened
}

/// The writes a move of `Projects` to `Archive/Work` plans: each note inside under its old
/// identity at its new path, then each page whose links follow.
fn edits() -> Vec<Edit> {
    let at = |from: &str, to: &str, text: &str, base: &str| Edit {
        page: page_id_for_path(from),
        recovery: None,
        ..edit(to, text, &[revision(base)])
    };
    vec![
        at("Projects/A.md", "Archive/Work/A.md", A, A),
        at(
            "Projects/B.md",
            "Archive/Work/B.md",
            "- b [[Archive/Work/A]]\n",
            B,
        ),
        at(
            "Top.md",
            "Top.md",
            "- [[Archive/Work/A]] ![p](Archive/Work/img.png)\n",
            TOP,
        ),
    ]
}

fn journals(opened: &Fixture) -> usize {
    fs::read_dir(opened.state.join("transactions"))
        .map(|entries| {
            entries
                .flatten()
                .filter(|entry| entry.path().is_file())
                .count()
        })
        .unwrap_or(0)
}

fn assert_moved(opened: &Fixture, notebook: &FolderNotebook) {
    let read = |path: &str| fs::read_to_string(opened.folder.join(path)).unwrap();
    assert_eq!(read("Archive/Work/A.md"), A);
    assert_eq!(read("Archive/Work/B.md"), "- b [[Archive/Work/A]]\n");
    assert_eq!(
        read("Top.md"),
        "- [[Archive/Work/A]] ![p](Archive/Work/img.png)\n"
    );
    assert_eq!(read("Archive/Work/img.png"), "png");
    assert_eq!(read("Archive/Work/Scans/receipt.pdf"), "pdf");
    assert!(opened.folder.join("Archive/Work/Empty").is_dir());
    assert!(!opened.folder.join("Projects").exists());
    assert_eq!(journals(opened), 0);
    assert!(
        notebook
            .revisions("Archive/Work/B.md")
            .unwrap()
            .iter()
            .any(|saved| notebook
                .revision_text("Archive/Work/B.md", saved.id)
                .unwrap()
                == B),
        "the moved note keeps the text it replaced in history"
    );
    assert_eq!(
        notebook
            .heads(&page_id_for_path("Archive/Work/B.md"))
            .unwrap(),
        [revision("- b [[Archive/Work/A]]\n")]
    );
    assert_eq!(notebook.path_of(&page_id_for_path("Projects/B.md")), None);
}

fn assert_untouched(opened: &Fixture) {
    for (path, text) in PROJECT {
        assert_eq!(
            fs::read_to_string(opened.folder.join(path)).unwrap(),
            text,
            "{path} is unchanged"
        );
    }
    assert!(opened.folder.join("Projects/Empty").is_dir());
    assert!(!opened.folder.join("Archive/Work").exists());
    assert_eq!(journals(opened), 0, "no journal is left behind");
}

#[test]
fn a_folder_with_notes_moves_whole_with_its_attachments_and_empty_folders() {
    let opened = project();
    let saved = opened
        .notebook
        .move_folder_if_heads(&edits(), "Projects", "Archive/Work")
        .unwrap();
    let CheckedSaves::Saved(acks) = saved else {
        panic!("the move must be saved: {saved:?}");
    };
    assert_eq!(acks.len(), 3);
    assert_eq!(
        acks[1].revision.root().field("path").as_str(),
        Some("Archive/Work/B.md")
    );
    assert_moved(&opened, &opened.notebook);
    assert!(
        !opened.state.join("retired").exists(),
        "notes move with their folder rather than being archived"
    );
}

#[test]
fn a_folder_move_refuses_any_clash_with_nothing_moved() {
    let opened = project();
    let notebook = &opened.notebook;
    fs::create_dir_all(opened.folder.join("Taken")).unwrap();
    fs::write(opened.folder.join("Taken/x.png"), "x").unwrap();
    for to in ["Taken", "taken"] {
        let error = notebook
            .move_folder_if_heads(&edits(), "Projects", to)
            .unwrap_err();
        assert!(
            error.to_string().contains("already exists"),
            "{to}: {error}"
        );
    }
    let error = notebook
        .move_folder_if_heads(&edits(), "Projects", "Projects/Inner")
        .unwrap_err();
    assert!(error.to_string().contains("inside itself"), "{error}");

    let mut stale = edits();
    stale[2].base = vec![revision("- an older Top")];
    assert!(matches!(
        notebook
            .move_folder_if_heads(&stale, "Projects", "Archive/Work")
            .unwrap(),
        CheckedSaves::HeadsChanged { .. }
    ));
    let mut astray = edits();
    astray[0].path = "Elsewhere/A.md".into();
    let error = notebook
        .move_folder_if_heads(&astray, "Projects", "Archive/Work")
        .unwrap_err();
    assert!(error.to_string().contains("must move to"), "{error}");
    assert_untouched(&opened);

    fs::write(opened.folder.join("Projects/Scans/New.md"), "- new").unwrap();
    assert!(matches!(
        notebook
            .move_folder_if_heads(&edits(), "Projects", "Archive/Work")
            .unwrap(),
        CheckedSaves::Appeared { path } if path == "Projects/Scans/New.md"
    ));
    fs::remove_file(opened.folder.join("Projects/Scans/New.md")).unwrap();
    assert_untouched(&opened);
}

#[cfg(unix)]
#[test]
fn a_folder_that_cannot_move_leaves_everything_and_no_journal() {
    use std::os::unix::fs::PermissionsExt as _;
    let opened = project();
    let locked = opened.folder.join("Locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o555)).unwrap();
    let error = opened
        .notebook
        .move_folder_if_heads(&edits_to("Locked/Work"), "Projects", "Locked/Work")
        .unwrap_err();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(error.to_string().contains("Nothing was moved"), "{error:#}");
    assert_untouched(&opened);
}

fn edits_to(folder: &str) -> Vec<Edit> {
    edits()
        .into_iter()
        .map(|edit| Edit {
            path: edit.path.replace("Archive/Work", folder),
            text: edit.text.replace("Archive/Work", folder),
            ..edit
        })
        .collect()
}

#[test]
fn a_folder_move_interrupted_before_the_folder_moves_finishes_on_the_next_open() {
    let opened = project();
    opened.notebook.interrupt_folder.set(true);
    let error = opened
        .notebook
        .move_folder_if_heads(&edits(), "Projects", "Archive/Work")
        .unwrap_err();
    assert!(error.to_string().contains("Injected interruption"));
    assert_eq!(journals(&opened), 1);
    assert!(opened.folder.join("Projects/img.png").exists());
    let stuck = opened.notebook.stuck_changes();
    assert!(
        stuck[0].summary().contains("Still to update: Projects"),
        "{}",
        stuck[0].summary()
    );

    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert!(
        reopened.notices()[0].starts_with("Finished an interrupted change"),
        "{:?}",
        reopened.notices()
    );
    assert_moved(&opened, &reopened);
}

#[test]
fn a_folder_move_interrupted_after_the_folder_moved_finishes_on_the_next_open_from_every_step() {
    // Steps after the folder moves: 0 to 2 write A, B, and Top in place, 3 is after them all.
    for step in 0..4 {
        let opened = project();
        opened.notebook.interrupt_at.set(Some(step));
        let error = opened
            .notebook
            .move_folder_if_heads(&edits(), "Projects", "Archive/Work")
            .unwrap_err();
        assert!(
            error.to_string().contains("Injected interruption"),
            "{error}"
        );
        assert_eq!(journals(&opened), 1, "step {step}");
        assert!(opened.folder.join("Archive/Work/img.png").exists());

        let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
        assert!(
            reopened.notices()[0].starts_with("Finished an interrupted change"),
            "step {step}: {:?}",
            reopened.notices()
        );
        assert_moved(&opened, &reopened);
    }
}

#[test]
fn a_folder_without_notes_moves_whole_unless_a_note_appeared_or_the_target_exists() {
    let opened = fixture(&[
        ("Keep.md", "- keep"),
        ("Scans/2024/receipt.pdf", "pdf"),
        ("Taken/Note.md", "- taken"),
    ]);
    let notebook = &opened.notebook;
    let refused = |from: &str, to: &str| {
        notebook
            .move_noteless_folder(from, to)
            .unwrap_err()
            .to_string()
    };
    assert!(refused("Scans", "Taken").contains("already exists"));
    assert!(refused("Scans", "Scans/Inner").contains("inside itself"));
    assert!(refused("Scans", "../Out").contains("Invalid folder path"));
    assert!(refused("Taken", "Elsewhere").contains("A note appeared in Taken"));
    assert!(opened.folder.join("Taken/Note.md").exists());

    notebook
        .move_noteless_folder("Scans", "Archive/Scans")
        .unwrap();
    assert!(!opened.folder.join("Scans").exists());
    assert_eq!(
        fs::read_to_string(opened.folder.join("Archive/Scans/2024/receipt.pdf")).unwrap(),
        "pdf"
    );
    assert!(refused("Scans", "Again").contains("no longer in the notebook"));
}

#[test]
fn a_new_folder_is_created_on_disk_with_its_parents_unless_the_path_is_taken() {
    let opened = fixture(&[("Taken/Note.md", "- taken"), ("Scans/receipt.pdf", "pdf")]);
    let notebook = &opened.notebook;
    let refused = |folder: &str| notebook.create_folder(folder).unwrap_err().to_string();
    assert!(refused("taken").contains("already exists"));
    assert!(refused("Scans/receipt.pdf").contains("already exists"));
    assert!(refused("../Out").contains("Invalid folder path"));
    assert!(refused(".hidden").contains("Invalid folder path"));

    notebook.create_folder("Archive/2024/Empty").unwrap();
    assert!(opened.folder.join("Archive/2024/Empty").is_dir());
    assert!(
        !opened
            .folder
            .join("Archive/2024/Empty/Placeholder.md")
            .exists()
    );
    assert!(
        notebook
            .listing()
            .folders
            .contains(&"Archive/2024/Empty".to_owned())
    );
    assert!(refused("Archive/2024/Empty").contains("already exists"));
}

#[cfg(unix)]
#[test]
fn a_new_folder_refuses_a_parent_that_links_out_of_the_notebook() {
    let opened = fixture(&[("Keep.md", "- keep")]);
    let outside = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(outside.path(), opened.folder.join("Linked")).unwrap();
    let error = opened.notebook.create_folder("Linked/Inner").unwrap_err();
    assert!(error.to_string().contains("link"), "{error:#}");
    assert!(!outside.path().join("Inner").exists());
}
