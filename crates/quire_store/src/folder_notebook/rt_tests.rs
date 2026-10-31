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

const OLD: &str = "---\ntitle: \"Old\"\n---\n- body\n";
const LINKS: &str = "- see [[Old]]\n";
const RENAMED: &str = "---\ntitle: \"New\"\n---\n- body\n";
const RELINKED: &str = "- see [[New]]\n";

/// The writes a rename of `Old.md` to `New.md` plans: the moved page under its old identity at
/// its new path, then each page whose links follow it.
fn rename_edits() -> Vec<Edit> {
    let plain = |path: &str, text: &str, base: &str| Edit {
        recovery: None,
        ..edit(path, text, &[revision(base)])
    };
    vec![
        Edit {
            page: page_id_for_path("Old.md"),
            ..plain("New.md", RENAMED, OLD)
        },
        plain("Links.md", RELINKED, LINKS),
    ]
}

fn retired(state: &Path) -> Vec<String> {
    let folder = state.join("retired");
    if !folder.exists() {
        return Vec::new();
    }
    entries(&folder)
        .into_iter()
        .map(|name| fs::read_to_string(folder.join(name)).unwrap())
        .collect()
}

fn journals(state: &Path) -> Vec<String> {
    let folder = state.join("transactions");
    if folder.exists() {
        entries(&folder)
    } else {
        Vec::new()
    }
}

fn assert_renamed(folder: &Path, state: &Path, notebook: &FolderNotebook) {
    assert_eq!(fs::read_to_string(folder.join("New.md")).unwrap(), RENAMED);
    assert_eq!(
        fs::read_to_string(folder.join("Links.md")).unwrap(),
        RELINKED
    );
    assert!(!folder.join("Old.md").exists());
    assert_eq!(
        retired(state),
        [OLD],
        "the old file is archived, never lost"
    );
    assert!(journals(state).is_empty());
    let history: Vec<String> = notebook
        .revisions("New.md")
        .unwrap()
        .iter()
        .map(|revision| notebook.revision_text("New.md", revision.id).unwrap())
        .collect();
    assert!(
        history.contains(&OLD.to_owned()),
        "the new path keeps the old history"
    );
    assert_eq!(
        notebook.heads(&page_id_for_path("Old.md")).unwrap(),
        Vec::<String>::new()
    );
}

#[test]
fn a_rename_moves_the_file_and_rewrites_links_through_the_journal() {
    let opened = fixture(&[
        ("Old.md", OLD),
        ("Links.md", LINKS),
        ("Keep.md", "- keep\n"),
    ]);
    let saved = opened.notebook.save_many_if_heads(&rename_edits()).unwrap();
    let CheckedSaves::Saved(acks) = saved else {
        panic!("the rename must be saved");
    };
    assert_eq!(acks.len(), 2);
    assert_eq!(acks[0].heads, [revision(RENAMED)]);
    assert_renamed(&opened.folder, &opened.state, &opened.notebook);
    assert_eq!(
        fs::read_to_string(opened.folder.join("Keep.md")).unwrap(),
        "- keep\n"
    );
}

#[test]
fn a_rename_never_replaces_a_file_whose_name_differs_only_in_case() {
    for taken in ["new.md", "NEW.MD"] {
        let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS), (taken, "- mine\n")]);
        let error = opened
            .notebook
            .save_many_if_heads(&rename_edits())
            .unwrap_err();
        assert!(format!("{error:#}").contains("already exists"), "{error:#}");
        assert_eq!(
            fs::read_to_string(opened.folder.join(taken)).unwrap(),
            "- mine\n"
        );
        assert_eq!(
            fs::read_to_string(opened.folder.join("Old.md")).unwrap(),
            OLD
        );
        assert_eq!(
            fs::read_to_string(opened.folder.join("Links.md")).unwrap(),
            LINKS
        );
        assert!(journals(&opened.state).is_empty());
    }
}

#[test]
fn an_interrupted_folder_move_finishes_on_the_next_open_from_every_step() {
    const A: &str = "- a [[Projects/B]]\n";
    const B: &str = "- b\n";
    const TOP: &str = "- [[Projects/A]] [[Projects/B]]\n";
    let moved = |from: &str, to: &str, text: &str, base: &str| Edit {
        page: page_id_for_path(from),
        recovery: None,
        ..edit(to, text, &[revision(base)])
    };
    let edits = || {
        vec![
            moved("Projects/A.md", "Work/A.md", "- a [[Work/B]]\n", A),
            moved("Projects/B.md", "Work/B.md", B, B),
            moved("Top.md", "Top.md", "- [[Work/A]] [[Work/B]]\n", TOP),
        ]
    };
    // Steps: 0 create Work/B.md, 1 create Work/A.md, 2 rewrite Top.md, 3 and 4 archive the old
    // notes. Every interruption leaves the journal, and the next open finishes the move.
    for step in 0..5 {
        let opened = fixture(&[("Projects/A.md", A), ("Projects/B.md", B), ("Top.md", TOP)]);
        opened.notebook.interrupt_at.set(Some(step));
        let error = opened.notebook.save_many_if_heads(&edits()).unwrap_err();
        assert!(error.to_string().contains("Injected interruption"));
        assert_eq!(journals(&opened.state).len(), 1, "step {step}");

        let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
        assert_eq!(reopened.notices().len(), 1, "step {step}");
        assert!(reopened.notices()[0].starts_with("Finished an interrupted change"));
        for (path, text) in [
            ("Work/A.md", "- a [[Work/B]]\n"),
            ("Work/B.md", B),
            ("Top.md", "- [[Work/A]] [[Work/B]]\n"),
        ] {
            assert_eq!(
                fs::read_to_string(opened.folder.join(path)).unwrap(),
                text,
                "step {step}: {path}"
            );
        }
        assert!(!opened.folder.join("Projects/A.md").exists(), "step {step}");
        assert!(!opened.folder.join("Projects/B.md").exists(), "step {step}");
        let mut archived = retired(&opened.state);
        archived.sort();
        assert_eq!(archived, [A, B], "step {step}: both originals are archived");
        assert!(journals(&opened.state).is_empty());
        assert!(
            reopened
                .revisions("Work/A.md")
                .unwrap()
                .iter()
                .any(|saved| reopened.revision_text("Work/A.md", saved.id).unwrap() == A),
            "step {step}: the moved note keeps its history"
        );
    }
}

#[test]
fn an_interrupted_rename_finishes_on_the_next_open_from_every_step() {
    // Steps: 0 create New.md, 1 rewrite Links.md, 2 archive Old.md.
    for step in 0..3 {
        let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS)]);
        opened.notebook.interrupt_at.set(Some(step));
        let error = opened
            .notebook
            .save_many_if_heads(&rename_edits())
            .unwrap_err();
        assert!(error.to_string().contains("Injected interruption"));
        assert_eq!(journals(&opened.state).len(), 1, "step {step}");
        assert!(opened.folder.join("Old.md").exists(), "step {step}");

        let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
        assert_eq!(reopened.notices().len(), 1);
        assert!(
            reopened.notices()[0].starts_with("Finished an interrupted change"),
            "step {step}: {:?}",
            reopened.notices()
        );
        assert_renamed(&opened.folder, &opened.state, &reopened);
        let again = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
        assert!(
            again.notices().is_empty(),
            "a finished journal is not replayed"
        );
    }
}

#[test]
fn a_rename_interrupted_at_each_batch_boundary_finishes_once_with_history_kept_once() {
    // Staged: every new text is durable beside its note, nothing renamed, no history yet.
    // Step 0: history recorded, nothing renamed. Step 3 (the step count): every note renamed
    // and archived, folders not yet synced and drafts not settled.
    for (staged, step) in [(true, None), (false, Some(0)), (false, Some(3))] {
        let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS)]);
        opened.notebook.interrupt_staged.set(staged);
        opened.notebook.interrupt_at.set(step);
        let error = opened
            .notebook
            .save_many_if_heads(&rename_edits())
            .unwrap_err();
        assert!(error.to_string().contains("Injected interruption"));
        assert_eq!(journals(&opened.state).len(), 1, "{step:?}");
        let expected: &[&str] = if step == Some(3) {
            &["Links.md", "New.md"]
        } else {
            &["Links.md", "Old.md"]
        };
        assert_eq!(
            entries(&opened.folder),
            expected,
            "{step:?}: no staged file is left beside the notes"
        );

        let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
        assert_eq!(reopened.notices().len(), 1, "{step:?}");
        assert!(reopened.notices()[0].starts_with("Finished an interrupted change"));
        assert_renamed(&opened.folder, &opened.state, &reopened);
        let links = reopened.revisions("Links.md").unwrap();
        assert_eq!(links.len(), 1, "{step:?}: the replaced links are kept once");
        assert_eq!(
            reopened.revision_text("Links.md", links[0].id).unwrap(),
            LINKS
        );
        let again = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
        assert!(again.notices().is_empty(), "{step:?}: not replayed");
    }
}

/// Full syncs flush the drive's whole cache on macOS and cost milliseconds each, so a change
/// across notes makes a fixed number of them however many notes it writes.
#[cfg(target_vendor = "apple")]
#[test]
fn a_change_across_notes_makes_the_same_number_of_full_syncs_for_any_note_count() {
    use crate::markdown_import::SourcePage;
    let syncs = |count: usize| {
        let opened = fixture(&[]);
        let pages: Vec<SourcePage> = (0..count)
            .map(|index| SourcePage {
                path: format!("Folder {}/Note {index}.md", index % 2),
                text: format!("- note {index}\n"),
            })
            .collect();
        let before = files::FULL_SYNCS.with(std::cell::Cell::get);
        opened.notebook.import_pages(&pages, 1).unwrap();
        let imported = files::FULL_SYNCS.with(std::cell::Cell::get) - before;
        let edits: Vec<Edit> = pages
            .iter()
            .map(|page| Edit {
                recovery: None,
                ..edit(
                    &page.path,
                    &format!("{}- edited\n", page.text),
                    &[revision(&page.text)],
                )
            })
            .collect();
        let before = files::FULL_SYNCS.with(std::cell::Cell::get);
        let saved = opened.notebook.save_many_if_heads(&edits).unwrap();
        assert!(matches!(saved, CheckedSaves::Saved(acks) if acks.len() == count));
        let edited = files::FULL_SYNCS.with(std::cell::Cell::get) - before;
        (imported, edited)
    };
    let few = syncs(4);
    assert_eq!(syncs(60), few, "full syncs do not grow with the note count");
    assert!(
        few.0 <= 6 && few.1 <= 4,
        "journal file and folder, two new folders, and one barrier per phase: {few:?}"
    );
}

#[test]
fn recovery_applies_the_steps_before_one_that_changed_and_stops_there() {
    let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS)]);
    opened.notebook.interrupt_at.set(Some(0));
    opened
        .notebook
        .save_many_if_heads(&rename_edits())
        .unwrap_err();
    fs::write(opened.folder.join("Links.md"), "- edited elsewhere\n").unwrap();

    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert!(reopened.notices()[0].contains("needs review"));
    assert_eq!(
        fs::read_to_string(opened.folder.join("New.md")).unwrap(),
        RENAMED,
        "the step before the changed note is applied"
    );
    assert_eq!(
        fs::read_to_string(opened.folder.join("Links.md")).unwrap(),
        "- edited elsewhere\n"
    );
    assert_eq!(
        fs::read_to_string(opened.folder.join("Old.md")).unwrap(),
        OLD
    );
    assert_eq!(journals(&opened.state).len(), 1);
}

#[test]
fn an_interrupted_rename_never_overwrites_a_note_changed_before_recovery() {
    let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS)]);
    opened.notebook.interrupt_at.set(Some(1));
    opened
        .notebook
        .save_many_if_heads(&rename_edits())
        .unwrap_err();
    fs::write(opened.folder.join("Links.md"), "- edited elsewhere\n").unwrap();

    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    let notices = reopened.notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("needs review"), "{notices:?}");
    assert!(notices[0].contains("Links.md"), "{notices:?}");
    assert_eq!(
        fs::read_to_string(opened.folder.join("Links.md")).unwrap(),
        "- edited elsewhere\n"
    );
    assert_eq!(
        fs::read_to_string(opened.folder.join("Old.md")).unwrap(),
        OLD
    );
    assert_eq!(
        fs::read_to_string(opened.folder.join("New.md")).unwrap(),
        RENAMED
    );
    assert_eq!(
        journals(&opened.state).len(),
        1,
        "the journal stays for review"
    );
}

/// A rename interrupted before rewriting Links.md, whose note then changed elsewhere, as the
/// next open finds it.
fn stuck_rename() -> (super::tests::Fixture, FolderNotebook) {
    let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS)]);
    opened.notebook.interrupt_at.set(Some(1));
    opened
        .notebook
        .save_many_if_heads(&rename_edits())
        .unwrap_err();
    fs::write(opened.folder.join("Links.md"), "- edited elsewhere\n").unwrap();
    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    (opened, reopened)
}

#[test]
fn a_stuck_change_names_where_each_note_stands_and_finishes_once_the_note_is_put_back() {
    let (opened, reopened) = stuck_rename();
    let stuck = reopened.stuck_changes();
    assert_eq!(stuck.len(), 1);
    assert_eq!(
        stuck[0].steps,
        [
            ("New.md".to_owned(), StepState::Done),
            ("Links.md".to_owned(), StepState::ChangedElsewhere),
            ("Old.md".to_owned(), StepState::Waiting),
        ]
    );
    assert_eq!(
        stuck[0].summary(),
        "Changed since the change was planned: Links.md. Still to update: Old.md. Already updated: New.md."
    );
    let refused = reopened.retry_change(&stuck[0].name).unwrap_err();
    assert!(refused.to_string().contains("Links.md"), "{refused:#}");
    assert_eq!(
        fs::read_to_string(opened.folder.join("Links.md")).unwrap(),
        "- edited elsewhere\n",
        "a retry never overwrites the newer note"
    );
    for name in ["../escape.json", "", ".hidden.json", "missing.json"] {
        assert!(reopened.retry_change(name).is_err(), "{name:?}");
    }

    fs::write(opened.folder.join("Links.md"), LINKS).unwrap();
    reopened.retry_change(&stuck[0].name).unwrap();
    assert_renamed(&opened.folder, &opened.state, &reopened);
    assert!(reopened.stuck_changes().is_empty());
}

#[test]
fn a_stuck_change_set_aside_keeps_its_journal_and_every_note_as_it_is() {
    let (opened, reopened) = stuck_rename();
    let name = reopened.stuck_changes()[0].name.clone();
    let kept = fs::read(opened.state.join("transactions").join(&name)).unwrap();
    let moved = reopened.set_aside_change(&name).unwrap();
    assert_eq!(
        moved,
        opened.state.join("transactions/set-aside").join(&name)
    );
    assert_eq!(
        fs::read(&moved).unwrap(),
        kept,
        "every planned version is kept"
    );
    assert!(reopened.stuck_changes().is_empty());
    for (path, text) in [
        ("Links.md", "- edited elsewhere\n"),
        ("Old.md", OLD),
        ("New.md", RENAMED),
    ] {
        assert_eq!(
            fs::read_to_string(opened.folder.join(path)).unwrap(),
            text,
            "{path}"
        );
    }
    let again = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert!(
        again.notices().is_empty(),
        "a set-aside change is not resumed"
    );
    assert!(again.stuck_changes().is_empty());
}

const DEVICE: &str = "6f1c2b3a-0d4e-4f5a-8b9c-0123456789ab";

/// A reference device database as `VaultFiles` creates it (vault.ts and history-store.ts), kept
/// open in WAL mode like a running reference app, so recent rows sit in its write-ahead log.
struct ReferenceDevice {
    connection: Connection,
}

impl ReferenceDevice {
    fn create(root: &Path) -> Self {
        Self::named(root, DEVICE)
    }

    fn named(root: &Path, device: &str) -> Self {
        let folder = root.join(".quire/devices").join(device);
        fs::create_dir_all(&folder).unwrap();
        let connection = crate::database::open_writable(&folder.join("durability.sqlite")).unwrap();
        for sql in [
            "CREATE TABLE IF NOT EXISTS pending (path TEXT PRIMARY KEY, text TEXT NOT NULL, expected_revision TEXT, saved_at INTEGER NOT NULL)",
            "CREATE TABLE IF NOT EXISTS imported_records (fingerprint TEXT PRIMARY KEY)",
            "CREATE TABLE IF NOT EXISTS foreign_drafts (fingerprint TEXT PRIMARY KEY, device TEXT NOT NULL, path TEXT NOT NULL, text TEXT NOT NULL, expected_revision TEXT, saved_at INTEGER NOT NULL, recovered_path TEXT NOT NULL, recovered_text TEXT NOT NULL)",
            "CREATE TABLE IF NOT EXISTS contents (hash TEXT PRIMARY KEY, codec TEXT NOT NULL, data BLOB NOT NULL)",
            "CREATE TABLE IF NOT EXISTS history (id INTEGER PRIMARY KEY, path TEXT NOT NULL, content TEXT NOT NULL, revision TEXT NOT NULL, saved_at INTEGER NOT NULL)",
        ] {
            connection.exec(sql).unwrap()().unwrap();
        }
        Self { connection }
    }

    fn pending(&self, path: &str, text: &str, expected: Option<&str>, saved_at: i64) {
        self.connection
            .exec_bound("INSERT OR REPLACE INTO pending(path,text,expected_revision,saved_at) VALUES (?,?,?,?)")
            .unwrap()((path, text, expected.map(revision), saved_at))
        .unwrap();
    }

    fn history(&self, path: &str, text: &str, codec: &str, saved_at: i64) {
        let data = match codec {
            "zstd" => zstd::stream::encode_all(text.as_bytes(), 0).unwrap(),
            _ => text.as_bytes().to_vec(),
        };
        let hash = crate::sync::format::sha256(text);
        self.connection
            .exec_bound("INSERT OR IGNORE INTO contents(hash,codec,data) VALUES (?,?,?)")
            .unwrap()((hash.as_str(), codec, data))
        .unwrap();
        self.connection
            .exec_bound("INSERT INTO history(path,content,revision,saved_at) VALUES (?,?,?,?)")
            .unwrap()((path, hash.as_str(), revision(text).as_str(), saved_at))
        .unwrap();
    }
}

/// Every file below `folder` with its bytes, to show nothing there changed.
fn snapshot(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let mut pending = vec![folder.to_owned()];
    while let Some(next) = pending.pop() {
        for entry in fs::read_dir(&next).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push((path.clone(), fs::read(&path).unwrap()));
            }
        }
    }
    found.sort();
    found
}

fn draft_text(notebook: &FolderNotebook, path: &str) -> Option<(String, Vec<String>)> {
    notebook
        .draft(&page_id_for_path(path))
        .unwrap()
        .map(|draft| (draft.text, draft.base))
}

#[test]
fn reference_drafts_and_history_become_recoverable_without_touching_the_notebook() {
    let opened = fixture(&[
        ("A.md", "- a\n"),
        ("B.md", "- b changed elsewhere\n"),
        ("D.md", "- d saved\n"),
        ("E.md", "---\ntitle: \"Eve\"\n---\n- e ^keep\n"),
        ("G.md", "- g now\n"),
    ]);
    let quire_draft = Draft {
        page: page_id_for_path("E.md"),
        path: "E.md".into(),
        text: "- typed in Quire\n".into(),
        base: vec![revision("---\ntitle: \"Eve\"\n---\n- e ^keep\n")],
        stamp: DraftStamp {
            session: "editor".into(),
            version: 1,
        },
        created: 1,
    };
    opened.notebook.put_draft(&quire_draft, None).unwrap();

    let device = ReferenceDevice::create(&opened.folder);
    device.pending("A.md", "- a typed in rt\n", Some("- a\n"), 10);
    device.pending("B.md", "- b typed in rt\n", Some("- b\n"), 11);
    device.pending("C.md", "- c never saved\n", None, 12);
    device.pending("D.md", "- d saved\n", Some("- d\n"), 13);
    device.pending(
        "E.md",
        "---\ntitle: \"Eve\"\n---\n- e typed in rt ^keep\n",
        Some("---\ntitle: \"Eve\"\n---\n- e ^keep\n"),
        14,
    );
    device.history("G.md", "- g first\n", "raw", 1_000);
    device.history(
        "G.md",
        &"- g second, long enough to compress\n".repeat(20),
        "zstd",
        90_000_000,
    );
    let recovery = opened
        .folder
        .join(".quire/devices")
        .join(DEVICE)
        .join("recovery");
    fs::create_dir_all(&recovery).unwrap();
    fs::write(
        recovery.join("legacy.json"),
        serde_json::json!({
            "path": "F.md",
            "text": "- f from an older reference app\n",
            "expectedRevision": null,
            "savedAt": 9,
        })
        .to_string(),
    )
    .unwrap();
    let before = snapshot(&opened.folder);

    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert_eq!(
        snapshot(&opened.folder),
        before,
        "the notebook folder is unchanged"
    );

    let recovered: Vec<String> = reopened
        .recovered_drafts()
        .into_iter()
        .map(|draft| draft.path)
        .collect();
    let copy = recovered
        .iter()
        .find(|path| path.starts_with("Recovered drafts/Draft 6f1c2b3a "))
        .cloned()
        .expect("a page with other unsaved work gets a separate recovered note");
    assert_eq!(recovered, ["A.md", "B.md", "C.md", &copy, "F.md"]);
    let notices = reopened.notices();
    assert!(
        notices[0]
            .starts_with("Recovered 5 unsaved drafts from the reference app: A.md, B.md, C.md"),
        "{notices:?}"
    );

    // The current file is the draft's base, so it settles when the page opens.
    assert_eq!(
        draft_text(&reopened, "A.md"),
        Some(("- a typed in rt\n".into(), vec![revision("- a\n")]))
    );
    // The file moved on since the draft's base, so the page opens in conflict.
    let b = reopened.checkout(&page_id_for_path("B.md"), None).unwrap();
    assert_eq!(b.draft.unwrap().base, [revision("- b\n")]);
    assert_eq!(b.heads, [revision("- b changed elsewhere\n")]);
    assert_eq!(
        draft_text(&reopened, "C.md"),
        Some(("- c never saved\n".into(), vec![]))
    );
    assert_eq!(draft_text(&reopened, "D.md"), None, "already saved");
    assert_eq!(
        draft_text(&reopened, "E.md").unwrap().0,
        "- typed in Quire\n",
        "Quire's own unsaved work is never replaced"
    );
    let (copied, base) = draft_text(&reopened, &copy).unwrap();
    assert!(base.is_empty());
    assert!(
        copied.starts_with("---\nrecovered-source-title-"),
        "{copied}"
    );
    assert!(copied.contains("title: \"Recovered draft: Eve\"\n---\n\n- e typed in rt ^b-"));
    assert!(!copied.contains("^keep"), "block IDs are fresh");
    assert_eq!(
        draft_text(&reopened, "F.md").unwrap().0,
        "- f from an older reference app\n"
    );
    let history: Vec<(String, i64)> = reopened
        .revisions("G.md")
        .unwrap()
        .into_iter()
        .map(|kept| {
            (
                reopened.revision_text("G.md", kept.id).unwrap(),
                kept.saved_at,
            )
        })
        .collect();
    assert_eq!(
        history,
        [
            (
                "- g second, long enough to compress\n".repeat(20),
                90_000_000
            ),
            ("- g first\n".into(), 1_000),
        ]
    );

    let again = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert!(
        again.recovered_drafts().is_empty(),
        "each record is imported once"
    );
    assert!(again.notices().is_empty());
    assert_eq!(snapshot(&opened.folder), before);

    // A newer draft from the same computer replaces the unedited import instead of piling up.
    device.pending("A.md", "- a typed more in rt\n", Some("- a\n"), 20);
    let later = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert_eq!(
        later
            .recovered_drafts()
            .into_iter()
            .map(|draft| draft.path)
            .collect::<Vec<_>>(),
        ["A.md"]
    );
    assert_eq!(
        draft_text(&later, "A.md").unwrap().0,
        "- a typed more in rt\n"
    );
}

#[test]
fn unreadable_reference_data_is_reported_and_left_alone() {
    let opened = fixture(&[("A.md", "- a\n")]);
    let folder = opened.folder.join(".quire/devices").join(DEVICE);
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("durability.sqlite"), "not a database").unwrap();
    let before = snapshot(&opened.folder);
    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    let notices = reopened.notices();
    assert_eq!(notices.len(), 1);
    assert!(notices[0].contains("could not be read"), "{notices:?}");
    assert!(notices[0].contains("reopen the notebook to retry"));
    assert_eq!(snapshot(&opened.folder), before);
    assert!(reopened.recovered_drafts().is_empty());
}

/// A rename of `Old.md` to `New.md` exactly as the reference `commitFiles` journals it in
/// `.quire/devices/<device>/transactions`: the moved note, the linking note, then the old file's
/// removal under a random UUID archive name, with the versions it was planned from.
fn reference_rename_journal(root: &Path) -> PathBuf {
    let folder = root
        .join(".quire/devices")
        .join(DEVICE)
        .join("transactions");
    fs::create_dir_all(&folder).unwrap();
    let journal = folder.join("2b7e1516-28ae-4d2a-a6f7-15880900cf4f.json");
    let disk = |path: &str, text: &str| serde_json::json!({ "path": path, "text": text, "revision": revision(text) });
    fs::write(
        &journal,
        serde_json::json!({
            "edits": [
                { "path": "New.md", "text": RENAMED, "expectedRevision": null, "historyFrom": "Old.md" },
                { "path": "Links.md", "text": RELINKED, "expectedRevision": revision(LINKS) },
                {
                    "path": "Old.md",
                    "text": "",
                    "expectedRevision": revision(OLD),
                    "remove": true,
                    "retirement": "9b2f4c1e-7d3a-4e8b-a1c5-0f6e2d4b8a90",
                },
            ],
            "before": [null, disk("Links.md", LINKS), disk("Old.md", OLD)],
            "createdAt": 1_700_000_000_000_i64,
        })
        .to_string(),
    )
    .unwrap();
    journal
}

#[test]
fn a_change_the_reference_app_left_unfinished_finishes_here_once() {
    let opened = fixture(&[("Old.md", OLD), ("Links.md", LINKS), ("New.md", RENAMED)]);
    let journal = reference_rename_journal(&opened.folder);
    let kept = fs::read(&journal).unwrap();
    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    let notices = reopened.notices();
    assert!(
        notices[0].starts_with(
            "Found 1 change across notes the reference app left unfinished in .quire/devices."
        ),
        "{notices:?}"
    );
    assert!(notices[1].starts_with("Finished an interrupted change"));
    assert_renamed(&opened.folder, &opened.state, &reopened);
    assert_eq!(
        fs::read(&journal).unwrap(),
        kept,
        "the reference app's journal stays as it is"
    );
    let again = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert!(again.notices().is_empty(), "{:?}", again.notices());
}

#[test]
fn a_change_the_reference_app_left_stuck_is_listed_and_set_aside_once() {
    let opened = fixture(&[
        ("Old.md", OLD),
        ("Links.md", "- edited elsewhere\n"),
        ("New.md", RENAMED),
    ]);
    reference_rename_journal(&opened.folder);
    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    let stuck = reopened.stuck_changes();
    assert_eq!(stuck.len(), 1);
    assert_eq!(
        stuck[0].steps,
        [
            ("New.md".to_owned(), StepState::Done),
            ("Links.md".to_owned(), StepState::ChangedElsewhere),
            ("Old.md".to_owned(), StepState::Waiting),
        ]
    );
    reopened.set_aside_change(&stuck[0].name).unwrap();
    let again = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    assert!(again.stuck_changes().is_empty(), "never taken in twice");
    assert!(again.notices().is_empty(), "{:?}", again.notices());
    assert_eq!(
        fs::read_to_string(opened.folder.join("Links.md")).unwrap(),
        "- edited elsewhere\n"
    );
}

impl ReferenceDevice {
    /// Records that this computer's reference app recovered `draft` from computer `origin` as the
    /// note at `recovered_path`, as `importCopiedData` does in vault.ts.
    fn recovered_from(
        &self,
        origin: &str,
        (path, text, saved_at): (&str, &str, i64),
        recovered_path: &str,
        recovered_text: &str,
    ) {
        self.connection
            .exec_bound("INSERT INTO foreign_drafts VALUES (?,?,?,?,?,?,?,?)")
            .unwrap()((
            format!("draft:{path}"),
            origin,
            path,
            text,
            None::<String>,
            saved_at,
            recovered_path,
            recovered_text,
        ))
        .unwrap();
    }
}

#[test]
fn a_draft_the_reference_app_recovered_from_another_computer_is_imported_once() {
    // The third computer sorts first, so its own records are read before the provenance.
    const THIRD: &str = "0c0c0c0c-1111-4222-8333-444455556666";
    let opened = fixture(&[
        ("A.md", "- a\n"),
        ("Recovered drafts/Draft 0c0c0c0c saved.md", "- s kept\n"),
    ]);
    let third = ReferenceDevice::named(&opened.folder, THIRD);
    third.pending("N.md", "- n typed on the third computer\n", None, 30);
    third.pending("S.md", "- s typed on the third computer\n", None, 31);
    let here = ReferenceDevice::create(&opened.folder);
    // Still waiting in this computer's reference app, unchanged since it was recovered.
    let waiting = "---\ntitle: \"Recovered draft: N\"\n---\n\n- n typed on the third computer\n";
    here.recovered_from(
        THIRD,
        ("N.md", "- n typed on the third computer\n", 30),
        "Recovered drafts/Draft 0c0c0c0c waiting.md",
        waiting,
    );
    here.pending(
        "Recovered drafts/Draft 0c0c0c0c waiting.md",
        waiting,
        None,
        40,
    );
    // Already saved there as its own note.
    here.recovered_from(
        THIRD,
        ("S.md", "- s typed on the third computer\n", 31),
        "Recovered drafts/Draft 0c0c0c0c saved.md",
        "- s kept\n",
    );
    here.pending("A.md", "- a typed here\n", Some("- a\n"), 41);

    let reopened = FolderNotebook::open(&opened.folder, &opened.state).unwrap();
    let recovered: Vec<String> = reopened
        .recovered_drafts()
        .into_iter()
        .map(|draft| draft.path)
        .collect();
    assert_eq!(
        recovered,
        ["N.md", "A.md"],
        "each draft comes back once, under its own page"
    );
    assert_eq!(
        draft_text(&reopened, "N.md").unwrap().0,
        "- n typed on the third computer\n"
    );
    assert_eq!(
        draft_text(&reopened, "S.md"),
        None,
        "already saved as a note"
    );
    assert_eq!(
        draft_text(&reopened, "Recovered drafts/Draft 0c0c0c0c waiting.md"),
        None
    );
    let tally = reopened.reference_tally().unwrap();
    assert_eq!(
        (tally.drafts, tally.drafts_accounted),
        (3, 3),
        "the waiting copy counts as the third computer's draft"
    );
    assert!(
        FolderNotebook::open(&opened.folder, &opened.state)
            .unwrap()
            .recovered_drafts()
            .is_empty()
    );
}
