//! Compatibility with folders the reference app already uses: its `.quire` settings, drafts,
//! history, Trash deletion, and journaled renames.

use super::tests::{edit, entries, fixture};
use super::*;
use std::{
    fs,
    sync::{Arc, Mutex},
};

/// A stand-in for the system Trash: moves each item into `bin` and records its path relative to
/// the notebook.
fn test_trash(root: &Path, bin: &Path) -> (Trash, Arc<Mutex<Vec<String>>>) {
    let moved = Arc::new(Mutex::new(Vec::new()));
    let (root, bin, record) = (
        fs::canonicalize(root).unwrap(),
        bin.to_owned(),
        moved.clone(),
    );
    let trash: Trash = Arc::new(move |path: &Path| {
        let relative = fs::canonicalize(path)?
            .strip_prefix(&root)?
            .to_string_lossy()
            .into_owned();
        let mut moved = record.lock().unwrap();
        fs::rename(path, bin.join(moved.len().to_string()))?;
        moved.push(relative);
        Ok(())
    });
    (trash, moved)
}

fn deletion(path: &str, text: &str) -> Edit {
    Edit {
        deleted: true,
        recovery: None,
        ..edit(path, text, &[revision(text)])
    }
}

/// `.quire/notebook.json` exactly as the reference `writeNotebookSettings` writes it:
/// `JSON.stringify({ version: 1, daily: layout }, null, 2) + "\n"`.
const RT_SETTINGS: &str = "{\n  \"version\": 1,\n  \"daily\": {\n    \"folder\": \"Journal\",\n    \"title\": \"{DD}.{MM}.{YYYY}\"\n  }\n}\n";

#[test]
fn reference_settings_are_read_and_replaced_only_from_the_version_read() {
    let opened = fixture(&[("Note.md", "- a\n"), (".quire/notebook.json", RT_SETTINGS)]);
    let settings = opened.notebook.settings().unwrap().unwrap();
    assert_eq!(settings.text, RT_SETTINGS);

    let newer = RT_SETTINGS.replace("Journal", "Daily");
    let stale = opened
        .notebook
        .save_settings(&newer, Some("not-the-revision"))
        .unwrap_err();
    assert!(format!("{stale:#}").contains("changed in another application"));
    assert_eq!(
        fs::read_to_string(opened.folder.join(".quire/notebook.json")).unwrap(),
        RT_SETTINGS
    );

    let revision = opened
        .notebook
        .save_settings(&newer, Some(&settings.revision))
        .unwrap();
    assert_eq!(
        fs::read_to_string(opened.folder.join(".quire/notebook.json")).unwrap(),
        newer
    );
    assert_eq!(
        opened.notebook.settings().unwrap().unwrap().revision,
        revision
    );
    assert_eq!(
        entries(&opened.folder.join(".quire")),
        ["notebook.json"],
        "the temporary file is renamed into place"
    );
    assert!(
        opened
            .notebook
            .save_settings("not json", Some(&revision))
            .is_err()
    );
}

#[test]
fn settings_create_the_metadata_folder_only_when_saved_and_refuse_links() {
    let opened = fixture(&[("Note.md", "- a\n")]);
    assert_eq!(opened.notebook.settings().unwrap(), None);
    assert!(!opened.folder.join(".quire").exists());
    assert!(
        opened
            .notebook
            .save_settings(RT_SETTINGS, Some("anything"))
            .is_err(),
        "a base for a missing file is stale"
    );
    opened.notebook.save_settings(RT_SETTINGS, None).unwrap();
    assert_eq!(
        fs::read_to_string(opened.folder.join(".quire/notebook.json")).unwrap(),
        RT_SETTINGS
    );

    #[cfg(unix)]
    {
        let linked = fixture(&[("Note.md", "- a\n")]);
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), linked.folder.join(".quire")).unwrap();
        assert!(linked.notebook.settings().is_err());
        assert!(linked.notebook.save_settings(RT_SETTINGS, None).is_err());
        assert!(entries(elsewhere.path()).is_empty());
    }
}

#[test]
fn deletion_without_a_trash_is_refused_before_touching_files() {
    let opened = fixture(&[("Old.md", "- old\n")]);
    let error = opened
        .notebook
        .delete_if_heads(&[deletion("Old.md", "- old\n")], None)
        .unwrap_err();
    assert_eq!(error.to_string(), NO_TRASH);
    assert!(opened.folder.join("Old.md").exists());
}

/// The reference "moves a page to the Trash unless it changed since it was chosen".
#[test]
fn a_page_goes_to_the_trash_unless_it_changed_since_it_was_chosen() {
    let mut opened = fixture(&[("Old.md", "- old\n"), ("Keep.md", "- keep\n")]);
    let bin = tempfile::tempdir().unwrap();
    let (trash, moved) = test_trash(&opened.folder, bin.path());
    opened.notebook.set_trash(trash);

    let stale = Edit {
        base: vec![revision("- stale\n")],
        ..deletion("Old.md", "- old\n")
    };
    assert!(matches!(
        opened.notebook.delete_if_heads(&[stale], None).unwrap(),
        CheckedSaves::HeadsChanged { page, heads }
            if page == page_id_for_path("Old.md") && heads == [revision("- old\n")]
    ));
    assert!(opened.folder.join("Old.md").exists());

    let saved = opened
        .notebook
        .delete_if_heads(&[deletion("Old.md", "- old\n")], None)
        .unwrap();
    assert!(matches!(saved, CheckedSaves::Saved(acks) if acks.len() == 1));
    assert_eq!(*moved.lock().unwrap(), ["Old.md"]);
    assert!(!opened.folder.join("Old.md").exists());
    assert_eq!(fs::read_to_string(bin.path().join("0")).unwrap(), "- old\n");
    assert_eq!(
        opened.notebook.heads(&page_id_for_path("Old.md")).unwrap(),
        Vec::<String>::new()
    );
    assert_eq!(opened.notebook.revisions("Old.md").unwrap().len(), 1);
    assert!(opened.folder.join("Keep.md").exists());
}

/// The reference "moves a folder with its files to the Trash, refusing one with an unlisted page".
#[test]
fn a_folder_goes_to_the_trash_whole_unless_a_page_appeared_in_it() {
    let mut opened = fixture(&[("Projects/Plan.md", "- plan\n"), ("Other.md", "- other\n")]);
    let bin = tempfile::tempdir().unwrap();
    let (trash, moved) = test_trash(&opened.folder, bin.path());
    opened.notebook.set_trash(trash);
    fs::write(opened.folder.join("Projects/photo.png"), "png").unwrap();
    fs::write(opened.folder.join("Projects/Late.md"), "- late\n").unwrap();
    assert_eq!(
        opened.notebook.other_files("Projects"),
        ["Projects/photo.png"]
    );
    let request = [deletion("Projects/Plan.md", "- plan\n")];
    assert!(matches!(
        opened
            .notebook
            .delete_if_heads(&request, Some("Projects"))
            .unwrap(),
        CheckedSaves::Appeared { path } if path == "Projects/Late.md"
    ));
    assert!(moved.lock().unwrap().is_empty());

    fs::remove_file(opened.folder.join("Projects/Late.md")).unwrap();
    let outside = [deletion("Other.md", "- other\n")];
    assert!(
        opened
            .notebook
            .delete_if_heads(&outside, Some("Projects"))
            .is_err(),
        "a page outside the folder is refused"
    );
    opened
        .notebook
        .delete_if_heads(&request, Some("Projects"))
        .unwrap();
    assert_eq!(*moved.lock().unwrap(), ["Projects"]);
    assert!(!opened.folder.join("Projects").exists());
    assert_eq!(entries(&bin.path().join("0")), ["Plan.md", "photo.png"]);
    assert!(opened.folder.join("Other.md").exists());
}

#[test]
fn deletion_refuses_unsaved_work_from_another_session_and_never_revives_a_note() {
    let mut opened = fixture(&[("Note.md", "- disk\n")]);
    let bin = tempfile::tempdir().unwrap();
    let (trash, moved) = test_trash(&opened.folder, bin.path());
    opened.notebook.set_trash(trash);
    let page = page_id_for_path("Note.md");
    let draft = Draft {
        page: page.clone(),
        path: "Note.md".into(),
        text: "- crash draft\n".into(),
        base: vec![revision("- disk\n")],
        stamp: DraftStamp {
            session: "crashed".into(),
            version: 1,
        },
        created: 1,
    };
    opened.notebook.put_draft(&draft, None).unwrap();
    let error = opened
        .notebook
        .delete_if_heads(&[deletion("Note.md", "- disk\n")], None)
        .unwrap_err();
    assert!(error.to_string().contains("unsaved work"));
    assert!(moved.lock().unwrap().is_empty());

    opened.notebook.discard_draft(&page).unwrap();
    opened
        .notebook
        .put_draft(
            &Draft {
                text: "- disk\n".into(),
                ..draft
            },
            None,
        )
        .unwrap();
    opened
        .notebook
        .delete_if_heads(&[deletion("Note.md", "- disk\n")], None)
        .unwrap();
    assert_eq!(opened.notebook.draft(&page).unwrap(), None);
    assert!(
        opened
            .notebook
            .checkout(&page, None)
            .unwrap()
            .draft
            .is_none()
    );
}

/// Regression: an open page's typing that had not reached its file was refused as another
/// session's work, or would have been left behind; it now goes to the Trash with the note.
#[test]
fn typing_not_yet_saved_goes_to_the_trash_with_the_note() {
    let mut opened = fixture(&[("Note.md", "- disk\n")]);
    let bin = tempfile::tempdir().unwrap();
    let (trash, _) = test_trash(&opened.folder, bin.path());
    opened.notebook.set_trash(trash);
    let page = page_id_for_path("Note.md");
    opened
        .notebook
        .put_draft(
            &Draft {
                page: page.clone(),
                path: "Note.md".into(),
                text: "- typed\n".into(),
                base: vec![revision("- disk\n")],
                stamp: DraftStamp {
                    session: "editor".into(),
                    version: 3,
                },
                created: 1,
            },
            None,
        )
        .unwrap();
    let typed = Edit {
        text: "- typed\n".into(),
        ..deletion("Note.md", "- disk\n")
    };
    opened.notebook.delete_if_heads(&[typed], None).unwrap();
    assert_eq!(
        fs::read_to_string(bin.path().join("0")).unwrap(),
        "- typed\n"
    );
    let kept: Vec<String> = opened
        .notebook
        .revisions("Note.md")
        .unwrap()
        .iter()
        .map(|revision| {
            opened
                .notebook
                .revision_text("Note.md", revision.id)
                .unwrap()
        })
        .collect();
    assert_eq!(kept, ["- typed\n", "- disk\n"]);
    assert_eq!(opened.notebook.draft(&page).unwrap(), None);
}
