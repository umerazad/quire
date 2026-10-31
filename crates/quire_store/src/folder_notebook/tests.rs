use super::*;
use std::fs;

pub(super) struct Fixture {
    pub(super) _root: tempfile::TempDir,
    pub(super) _state: tempfile::TempDir,
    pub(super) notebook: FolderNotebook,
    pub(super) folder: PathBuf,
    pub(super) state: PathBuf,
}

pub(super) fn fixture(files: &[(&str, &str)]) -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let target = root.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
    let notebook = FolderNotebook::open(root.path(), state.path()).unwrap();
    Fixture {
        folder: root.path().to_owned(),
        state: state.path().to_owned(),
        _root: root,
        _state: state,
        notebook,
    }
}

pub(super) fn edit(path: &str, text: &str, base: &[String]) -> Edit {
    Edit {
        page: page_id_for_path(path),
        base: base.to_vec(),
        path: path.into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: Some(DraftStamp {
            session: "editor".into(),
            version: 1,
        }),
    }
}

pub(super) fn entries(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn refused(error: anyhow::Error) -> FolderConflict {
    error
        .downcast::<FolderConflict>()
        .expect("the save must be refused as a conflict")
}

#[test]
fn listing_skips_hidden_build_and_linked_entries_and_reads_exact_text() {
    let fixture = fixture(&[
        ("Note.md", "---\ntitle: Hello\n---\r\n- one\r\n"),
        ("Folder/Nested.MD", "\u{feff}- bom kept"),
        (".quire/Hidden.md", "- hidden"),
        ("node_modules/Lib.md", "- lib"),
        ("out/Build.md", "- build"),
        ("notes.txt", "plain"),
        ("Folder/.quire-abc.tmp", "- stale temporary"),
    ]);
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        fixture.folder.join("Note.md"),
        fixture.folder.join("Link.md"),
    )
    .unwrap();
    let listing = fixture.notebook.listing();
    let paths: Vec<_> = listing
        .files
        .iter()
        .map(|file| file.path.as_str())
        .collect();
    assert_eq!(paths, ["Folder/Nested.MD", "Note.md"]);
    assert_eq!(listing.folders, ["Folder"]);
    let checkout = fixture
        .notebook
        .checkout(&page_id_for_path("Note.md"), None)
        .unwrap();
    let selected = checkout.selected.unwrap();
    assert_eq!(
        selected.root().field("text").as_str(),
        Some("---\ntitle: Hello\n---\r\n- one\r\n")
    );
    assert_eq!(
        checkout.heads,
        [revision("---\ntitle: Hello\n---\r\n- one\r\n")]
    );
    let nested = fixture
        .notebook
        .checkout(&page_id_for_path("Folder/Nested.MD"), None)
        .unwrap();
    assert_eq!(
        nested.selected.unwrap().root().field("text").as_str(),
        Some("\u{feff}- bom kept")
    );
}

#[test]
fn invalid_utf8_and_escaping_paths_are_refused_with_guidance() {
    let fixture = fixture(&[]);
    fs::write(fixture.folder.join("Latin.md"), [0x66, 0xff, 0x6f]).unwrap();
    let error = read(&fixture.folder, "Latin.md").unwrap_err();
    assert!(
        format!("{error:#}").contains("not valid UTF-8"),
        "{error:#}"
    );
    for path in [
        "../Outside.md",
        "/abs.md",
        ".hidden/A.md",
        "a\\b.md",
        "A.txt",
    ] {
        assert!(validate(path).is_err(), "{path} must be refused");
    }
    #[cfg(unix)]
    {
        let outside = tempfile::tempdir().unwrap();
        fs::write(outside.path().join("Escape.md"), "- outside").unwrap();
        std::os::unix::fs::symlink(outside.path(), fixture.folder.join("Linked")).unwrap();
        let error = fixture
            .notebook
            .save(&edit(
                "Linked/Escape.md",
                "- overwrite",
                &[revision("- outside")],
            ))
            .unwrap_err();
        assert!(format!("{error:#}").contains("link"), "{error:#}");
        assert_eq!(
            fs::read_to_string(outside.path().join("Escape.md")).unwrap(),
            "- outside"
        );
    }
}

#[test]
fn opening_never_writes_inside_the_notebook() {
    let fixture = fixture(&[("Note.md", "- one")]);
    assert_eq!(entries(&fixture.folder), ["Note.md"]);
    let inside = fixture.folder.join("state");
    let error = FolderNotebook::open(&fixture.folder, &inside)
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("outside the notebook"),
        "{error:#}"
    );
    assert!(!inside.exists());
    let other = tempfile::tempdir().unwrap();
    let error = FolderNotebook::open(other.path(), &fixture.state)
        .err()
        .unwrap();
    assert!(
        format!("{error:#}").contains("belongs to the notebook"),
        "{error:#}"
    );
}

#[test]
fn saves_replace_files_atomically_keep_permissions_and_history() {
    let fixture = fixture(&[("Note.md", "- one\r\n")]);
    let page = page_id_for_path("Note.md");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        fs::set_permissions(
            fixture.folder.join("Note.md"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
    }
    let ack = fixture
        .notebook
        .save(&edit("Note.md", "- two\r\n", &[revision("- one\r\n")]))
        .unwrap();
    assert_eq!(ack.revision_id(), revision("- two\r\n"));
    assert_eq!(ack.heads, [revision("- two\r\n")]);
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Note.md")).unwrap(),
        "- two\r\n"
    );
    assert_eq!(
        entries(&fixture.folder),
        ["Note.md"],
        "no temporary file remains"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = fs::metadata(fixture.folder.join("Note.md"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o640);
    }
    assert_eq!(
        fixture.notebook.draft(&page).unwrap(),
        None,
        "the save settles recovery"
    );
    let history = fixture.notebook.revisions("Note.md").unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].revision, revision("- one\r\n"));
    assert_eq!(
        fixture
            .notebook
            .revision_text("Note.md", history[0].id)
            .unwrap(),
        "- one\r\n"
    );
    let old = fixture
        .notebook
        .get(&revision("- one\r\n"))
        .unwrap()
        .unwrap();
    assert_eq!(old.root().field("text").as_str(), Some("- one\r\n"));
}

#[test]
fn a_save_from_an_older_version_is_refused_and_keeps_both_versions() {
    let fixture = fixture(&[("Note.md", "- one")]);
    let page = page_id_for_path("Note.md");
    fs::write(fixture.folder.join("Note.md"), "- changed elsewhere").unwrap();
    let error = fixture
        .notebook
        .save(&edit("Note.md", "- mine", &[revision("- one")]))
        .unwrap_err();
    let conflict = refused(error);
    assert_eq!(
        conflict.disk.as_ref().map(|disk| disk.text.as_str()),
        Some("- changed elsewhere")
    );
    assert!(conflict.message.contains("changed on disk"));
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Note.md")).unwrap(),
        "- changed elsewhere"
    );
    assert_eq!(
        fixture.notebook.draft(&page).unwrap().unwrap().text,
        "- mine"
    );
    assert_eq!(entries(&fixture.folder), ["Note.md"]);

    fs::remove_file(fixture.folder.join("Note.md")).unwrap();
    let conflict = refused(
        fixture
            .notebook
            .save(&edit(
                "Note.md",
                "- mine again",
                &[revision("- changed elsewhere")],
            ))
            .unwrap_err(),
    );
    assert_eq!(
        conflict.disk, None,
        "a deleted note is never recreated silently"
    );
    assert!(!fixture.folder.join("Note.md").exists());
}

#[test]
fn new_notes_are_created_exclusively_with_their_folders() {
    let fixture = fixture(&[]);
    fixture
        .notebook
        .save(&edit("Projects/Plan.md", "- start", &[]))
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Projects/Plan.md")).unwrap(),
        "- start"
    );
    let conflict = refused(
        fixture
            .notebook
            .save(&edit("Projects/Plan.md", "- other", &[]))
            .unwrap_err(),
    );
    assert_eq!(conflict.disk.unwrap().text, "- start");
    let raced = fixture.folder.join("Raced.md");
    let error = fixture
        .notebook
        .save_with(&edit("Raced.md", "- mine", &[]), &|| {
            fs::write(&raced, "- theirs")?;
            Ok(())
        })
        .unwrap_err();
    assert_eq!(refused(error).disk.unwrap().text, "- theirs");
    assert_eq!(fs::read_to_string(&raced).unwrap(), "- theirs");
}

#[test]
fn a_change_between_staging_and_renaming_is_refused() {
    let fixture = fixture(&[("Note.md", "- one")]);
    let note = fixture.folder.join("Note.md");
    let error = fixture
        .notebook
        .save_with(&edit("Note.md", "- mine", &[revision("- one")]), &|| {
            fs::write(&note, "- theirs")?;
            Ok(())
        })
        .unwrap_err();
    assert!(refused(error).message.contains("during saving"));
    assert_eq!(fs::read_to_string(&note).unwrap(), "- theirs");
    assert_eq!(entries(&fixture.folder), ["Note.md"]);
}

#[test]
fn a_crash_between_the_temporary_write_and_the_rename_loses_nothing() {
    let fixture = fixture(&[("Note.md", "- one")]);
    let page = page_id_for_path("Note.md");
    let crashed = fixture.notebook.save_with(
        &edit("Note.md", "- typed before the crash", &[revision("- one")]),
        &|| anyhow::bail!("simulated crash after the temporary file was synced"),
    );
    assert!(crashed.is_err());
    // A real crash leaves its synced temporary file behind; the walk never lists it.
    fs::write(
        fixture.folder.join(".quire-leftover.tmp"),
        "- typed before the crash",
    )
    .unwrap();
    drop(fixture.notebook);
    let reopened = FolderNotebook::open(&fixture.folder, &fixture.state).unwrap();
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Note.md")).unwrap(),
        "- one"
    );
    let checkout = reopened.checkout(&page, None).unwrap();
    assert_eq!(checkout.heads, [revision("- one")]);
    let draft = checkout.draft.expect("recovery survives the crash");
    assert_eq!(draft.text, "- typed before the crash");
    assert_eq!(draft.base, [revision("- one")]);
    let paths: Vec<_> = reopened
        .listing()
        .files
        .into_iter()
        .map(|file| file.path)
        .collect();
    assert_eq!(paths, ["Note.md"]);
    reopened
        .save(&Edit {
            recovery: Some(draft.stamp.clone()),
            ..edit("Note.md", &draft.text, &draft.base)
        })
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Note.md")).unwrap(),
        "- typed before the crash"
    );
    assert_eq!(reopened.draft(&page).unwrap(), None);
}

#[test]
fn drafts_refuse_stale_snapshots_and_other_sessions() {
    let fixture = fixture(&[("Note.md", "- one")]);
    let page = page_id_for_path("Note.md");
    let draft = |session: &str, version: u64, text: &str| Draft {
        page: page.clone(),
        path: "Note.md".into(),
        text: text.into(),
        base: vec![revision("- one")],
        stamp: DraftStamp {
            session: session.into(),
            version,
        },
        created: 1,
    };
    fixture
        .notebook
        .put_draft(&draft("a", 2, "- two"), None)
        .unwrap();
    assert!(
        fixture
            .notebook
            .put_draft(&draft("a", 1, "- older"), None)
            .is_err()
    );
    assert!(
        fixture
            .notebook
            .put_draft(&draft("b", 1, "- other"), None)
            .is_err()
    );
    let seen = draft("a", 2, "- two").stamp;
    fixture
        .notebook
        .put_draft(&draft("b", 1, "- other"), Some(&seen))
        .unwrap();
    assert_eq!(
        fixture.notebook.draft(&page).unwrap().unwrap().text,
        "- other"
    );
    fixture.notebook.clear_draft(&page, "- not this").unwrap();
    assert!(fixture.notebook.draft(&page).unwrap().is_some());
    fixture.notebook.clear_draft(&page, "- other").unwrap();
    assert_eq!(fixture.notebook.draft(&page).unwrap(), None);
}

#[test]
fn refresh_reports_external_edits_and_deletes_once() {
    let fixture = fixture(&[("Note.md", "- one"), ("Other.md", "- other")]);
    let page = page_id_for_path("Note.md");
    fixture.notebook.checkout(&page, None).unwrap();
    let paths = vec![
        "Note.md".to_owned(),
        "Other.md".to_owned(),
        "a.txt".to_owned(),
    ];
    let first = fixture.notebook.refresh(&paths).unwrap();
    assert_eq!(first.len(), 2, "each note is reported once: {first:?}");
    assert!(fixture.notebook.refresh(&paths).unwrap().is_empty());
    fs::write(fixture.folder.join("Note.md"), "- edited").unwrap();
    let changes = fixture.notebook.refresh(&paths).unwrap();
    assert_eq!(
        changes,
        [FolderChange::Changed {
            page: page.clone(),
            document: DiskDocument::new("Note.md".into(), "- edited".into()),
        }]
    );
    fixture
        .notebook
        .save(&edit("Note.md", "- saved here", &[revision("- edited")]))
        .unwrap();
    assert!(
        fixture.notebook.refresh(&paths).unwrap().is_empty(),
        "the adapter's own save is not an outside change"
    );
    fs::write(fixture.folder.join("Later.md"), "- created elsewhere").unwrap();
    assert_eq!(
        fixture
            .notebook
            .heads(&page_id_for_path("Later.md"))
            .unwrap(),
        [revision("- created elsewhere")],
        "a note created after opening is found by listing again"
    );
    fs::remove_file(fixture.folder.join("Note.md")).unwrap();
    assert_eq!(
        fixture.notebook.rescan().unwrap(),
        [
            FolderChange::Changed {
                page: page_id_for_path("Later.md"),
                document: DiskDocument::new("Later.md".into(), "- created elsewhere".into()),
            },
            FolderChange::Removed {
                page: page.clone(),
                path: "Note.md".into(),
            }
        ],
        "reading Later.md for its heads does not hide it from the editor"
    );
    assert_eq!(fixture.notebook.heads(&page).unwrap(), Vec::<String>::new());
}

#[test]
fn case_variants_keep_one_page_and_warn() {
    let fixture = fixture(&[]);
    fs::write(fixture.folder.join("Note.md"), "- upper").unwrap();
    if fs::write(fixture.folder.join("note.md"), "- lower").is_err()
        || entries(&fixture.folder).len() != 2
    {
        return;
    }
    let listing = fixture.notebook.listing();
    assert_eq!(listing.files.len(), 2);
    assert!(listing.warnings[0].contains("differ only in letter case"));
    assert_eq!(
        fixture
            .notebook
            .path_of(&page_id_for_path("note.md"))
            .as_deref(),
        Some("Note.md")
    );
}

#[test]
fn checked_multi_page_saves_write_all_or_report_the_changed_page() {
    let fixture = fixture(&[("A.md", "- a"), ("B.md", "- b")]);
    let edits = vec![
        edit("A.md", "- a2", &[revision("- a")]),
        edit("B.md", "- b2", &[revision("- b")]),
    ];
    fs::write(fixture.folder.join("B.md"), "- b elsewhere").unwrap();
    match fixture.notebook.save_many_if_heads(&edits).unwrap() {
        CheckedSaves::HeadsChanged { page, heads } => {
            assert_eq!(page, page_id_for_path("B.md"));
            assert_eq!(heads, [revision("- b elsewhere")]);
        }
        other => panic!("a changed page must stop the whole write, got {other:?}"),
    }
    assert_eq!(
        fs::read_to_string(fixture.folder.join("A.md")).unwrap(),
        "- a"
    );
    fs::write(fixture.folder.join("B.md"), "- b").unwrap();
    assert!(matches!(
        fixture.notebook.save_many_if_heads(&edits).unwrap(),
        CheckedSaves::Saved(acks) if acks.len() == 2
    ));
    assert_eq!(
        fs::read_to_string(fixture.folder.join("B.md")).unwrap(),
        "- b2"
    );
}

#[test]
fn read_only_notes_are_never_replaced() {
    let fixture = fixture(&[("Locked.md", "- locked")]);
    let note = fixture.folder.join("Locked.md");
    let mut permissions = fs::metadata(&note).unwrap().permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&note, permissions).unwrap();
    let error = fixture
        .notebook
        .save(&edit("Locked.md", "- changed", &[revision("- locked")]))
        .unwrap_err();
    assert!(format!("{error:#}").contains("read-only"), "{error:#}");
    assert_eq!(fs::read_to_string(&note).unwrap(), "- locked");
    let entries = entries(&fixture.folder);
    assert_eq!(entries, ["Locked.md"], "nothing was staged");
}

#[test]
fn writes_never_discard_unsaved_work_from_another_session() {
    let fixture = fixture(&[("Note.md", "- one"), ("Other.md", "- other")]);
    let page = page_id_for_path("Note.md");
    let crashed = Draft {
        page: page.clone(),
        path: "Note.md".into(),
        text: "- typed before a crash".into(),
        base: vec![revision("- one")],
        stamp: DraftStamp {
            session: "crashed".into(),
            version: 4,
        },
        created: 1,
    };
    fixture.notebook.put_draft(&crashed, None).unwrap();
    let bulk = |path: &str, text: &str, base: &str| Edit {
        recovery: None,
        ..edit(path, text, &[revision(base)])
    };
    let error = fixture
        .notebook
        .save_many_if_heads(&[
            bulk("Other.md", "- other #tag", "- other"),
            bulk("Note.md", "- one #tag", "- one"),
        ])
        .unwrap_err();
    assert!(refused(error).message.contains("earlier session"));
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Other.md")).unwrap(),
        "- other",
        "nothing is written"
    );
    assert!(
        fixture
            .notebook
            .save(&edit("Note.md", "- editor", &[revision("- one")]))
            .is_err()
    );
    assert_eq!(
        fixture.notebook.draft(&page).unwrap(),
        Some(crashed.clone())
    );
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Note.md")).unwrap(),
        "- one"
    );
    let mine = Draft {
        text: "- resolved".into(),
        stamp: DraftStamp {
            session: "editor".into(),
            version: 1,
        },
        ..crashed.clone()
    };
    fixture
        .notebook
        .put_draft(&mine, Some(&crashed.stamp))
        .unwrap();
    fixture
        .notebook
        .save(&edit("Note.md", "- resolved", &[revision("- one")]))
        .unwrap();
    assert_eq!(
        fs::read_to_string(fixture.folder.join("Note.md")).unwrap(),
        "- resolved"
    );
}

#[test]
fn reads_for_other_purposes_never_hide_a_change_from_refresh() {
    let fixture = fixture(&[("Note.md", "- one")]);
    let page = page_id_for_path("Note.md");
    let paths = vec!["Note.md".to_owned()];
    fixture.notebook.refresh(&paths).unwrap();
    fs::write(fixture.folder.join("Note.md"), "- outside").unwrap();
    assert_eq!(
        fixture.notebook.heads(&page).unwrap(),
        [revision("- outside")]
    );
    fixture.notebook.checkout(&page, None).unwrap();
    assert_eq!(fixture.notebook.refresh(&paths).unwrap().len(), 1);
}
