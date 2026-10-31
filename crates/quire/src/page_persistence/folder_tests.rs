//! Folder notebook pages: saves write the files, outside changes reload clean pages, and unsaved
//! work never overwrites or disappears under a changed file.

use super::tests::wait;
use super::*;
use crate::notebook_session::{NotebookSession, OpenPage, tests::wait as wait_loaded};
use gpui::TestAppContext;
use quire_store::{
    folder_notebook::{FolderNotebook, revision},
    sync::format::page_id_for_path,
};
use std::fs;

pub(crate) struct Folder {
    pub root: tempfile::TempDir,
    pub state: tempfile::TempDir,
}

impl Folder {
    pub fn new(files: &[(&str, &str)]) -> Self {
        let root = tempfile::tempdir().unwrap();
        for (path, text) in files {
            let target = root.path().join(path);
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(target, text).unwrap();
        }
        Self {
            root,
            state: tempfile::tempdir().unwrap(),
        }
    }

    pub fn read(&self, path: &str) -> String {
        fs::read_to_string(self.root.path().join(path)).unwrap()
    }

    pub fn write(&self, path: &str, text: &str) {
        fs::write(self.root.path().join(path), text).unwrap();
    }

    pub fn session(&self) -> NotebookSession {
        NotebookSession::open_folder(self.root.path().into(), self.state.path().into()).unwrap()
    }
}

pub(crate) fn open(
    session: &mut NotebookSession,
    path: &str,
    cx: &mut TestAppContext,
) -> (Entity<OpenPage>, Entity<PagePersistence>) {
    let page = cx.update(|cx| session.open_page(page_id_for_path(path), cx));
    wait_loaded(&page, cx);
    cx.update(|cx| session.enable_editing(cx));
    let persistence = page.read_with(cx, |page, _| {
        page.persistence()
            .cloned()
            .expect("the folder page opens editable")
    });
    wait(&persistence, cx);
    (page, persistence)
}

pub(crate) fn text(persistence: &Entity<PagePersistence>, cx: &mut TestAppContext) -> String {
    persistence.read_with(cx, |persistence, cx| persistence.buffer().read(cx).text())
}

fn append(persistence: &Entity<PagePersistence>, text: &str, cx: &mut TestAppContext) {
    persistence.update(cx, |persistence, cx| {
        persistence.buffer().update(cx, |buffer, cx| {
            let end = buffer.len();
            buffer.edit([(end..end, text)], None, cx);
        })
    });
}

fn save(persistence: &Entity<PagePersistence>, cx: &mut TestAppContext) {
    persistence.update(cx, |persistence, cx| persistence.save(cx));
    wait(persistence, cx);
}

fn draft_text(session: &NotebookSession, path: &str) -> Option<String> {
    let reply = session
        .storage()
        .folder()
        .unwrap()
        .draft(page_id_for_path(path))
        .unwrap();
    futures::executor::block_on(reply)
        .unwrap()
        .map(|draft| draft.text)
}

fn conflict_disk(
    persistence: &Entity<PagePersistence>,
    cx: &mut TestAppContext,
) -> Option<Option<String>> {
    persistence.read_with(cx, |persistence, _| {
        persistence
            .conflict()
            .map(|conflict| conflict.disk.as_ref().map(|disk| disk.text.clone()))
    })
}

#[gpui::test]
fn folder_saves_write_files_and_refuse_to_overwrite_outside_edits(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let folder = Folder::new(&[("Note.md", "- one\r\n")]);
    let mut session = folder.session();
    let (_page, persistence) = open(&mut session, "Note.md", cx);
    append(&persistence, "- two\n", cx);
    save(&persistence, cx);
    assert_eq!(folder.read("Note.md"), "- one\r\n- two\r\n");
    assert!(!persistence.read_with(cx, |persistence, cx| persistence.is_dirty(cx)));
    assert_eq!(draft_text(&session, "Note.md"), None);

    folder.write("Note.md", "- changed elsewhere\n");
    append(&persistence, "- three\n", cx);
    save(&persistence, cx);
    assert_eq!(folder.read("Note.md"), "- changed elsewhere\n");
    assert_eq!(
        conflict_disk(&persistence, cx),
        Some(Some("- changed elsewhere\n".into()))
    );
    assert_eq!(
        draft_text(&session, "Note.md").as_deref(),
        Some("- one\r\n- two\r\n- three\r\n"),
        "the refused text stays in recovery"
    );
    let status = persistence.read_with(cx, |persistence, cx| persistence.status(cx));
    assert!(status.contains("changed on disk"), "{status}");

    append(&persistence, "- four\n", cx);
    cx.executor().advance_clock(SAVE_LATEST);
    wait(&persistence, cx);
    save(&persistence, cx);
    assert_eq!(
        folder.read("Note.md"),
        "- changed elsewhere\n",
        "neither autosave nor an explicit save writes during a conflict"
    );
    assert_eq!(
        draft_text(&session, "Note.md").as_deref(),
        Some("- one\r\n- two\r\n- three\r\n- four\r\n"),
        "typing during a conflict is still recorded"
    );

    persistence.update(cx, |persistence, cx| persistence.use_disk(cx));
    wait(&persistence, cx);
    assert_eq!(text(&persistence, cx), "- changed elsewhere\n");
    assert_eq!(conflict_disk(&persistence, cx), None);
    assert!(!persistence.read_with(cx, |persistence, cx| persistence.is_dirty(cx)));
    assert_eq!(draft_text(&session, "Note.md"), None);
    append(&persistence, "- five\n", cx);
    save(&persistence, cx);
    assert_eq!(folder.read("Note.md"), "- changed elsewhere\n- five\n");
    session.storage().close().unwrap();
}

#[gpui::test]
fn outside_edits_reload_clean_pages_and_conflict_with_unsaved_work(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let folder = Folder::new(&[("Note.md", "- one\n"), ("Other.md", "- other\n")]);
    let mut session = folder.session();
    let (_page, persistence) = open(&mut session, "Note.md", cx);
    let disk = |text: &str| Some(DiskDocument::new("Note.md".into(), text.into()));

    persistence.update(cx, |persistence, cx| {
        persistence.external(disk("- reloaded\n"), cx)
    });
    wait(&persistence, cx);
    assert_eq!(text(&persistence, cx), "- reloaded\n");
    assert!(!persistence.read_with(cx, |persistence, cx| persistence.is_dirty(cx)));
    assert_eq!(
        persistence.read_with(cx, |persistence, _| persistence.base().to_vec()),
        [revision("- reloaded\n")]
    );

    append(&persistence, "- mine\n", cx);
    persistence.update(cx, |persistence, cx| {
        persistence.external(disk("- theirs\n"), cx)
    });
    wait(&persistence, cx);
    assert_eq!(text(&persistence, cx), "- reloaded\n- mine\n");
    assert_eq!(
        conflict_disk(&persistence, cx),
        Some(Some("- theirs\n".into()))
    );
    assert_eq!(
        draft_text(&session, "Note.md").as_deref(),
        Some("- reloaded\n- mine\n")
    );

    persistence.update(cx, |persistence, cx| {
        persistence.external(disk("- reloaded\n- mine\n"), cx)
    });
    wait(&persistence, cx);
    assert_eq!(
        conflict_disk(&persistence, cx),
        None,
        "an outside version equal to the buffer resolves the conflict"
    );
    assert!(!persistence.read_with(cx, |persistence, cx| persistence.is_dirty(cx)));

    let (_other, other) = open(&mut session, "Other.md", cx);
    fs::remove_file(folder.root.path().join("Other.md")).unwrap();
    other.update(cx, |other, cx| other.external(None, cx));
    wait(&other, cx);
    assert_eq!(conflict_disk(&other, cx), Some(None));
    assert_eq!(text(&other, cx), "- other\n", "the page text stays open");
    other.update(cx, |other, cx| other.use_disk(cx));
    wait(&other, cx);
    assert!(other.read_with(cx, |other, cx| {
        other.buffer().read(cx).capability() == language::Capability::ReadOnly
    }));
    assert!(!folder.root.path().join("Other.md").exists());
    session.storage().close().unwrap();
}

#[gpui::test]
fn recovered_folder_drafts_save_when_current_and_open_in_conflict_otherwise(
    cx: &mut TestAppContext,
) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let folder = Folder::new(&[("Current.md", "- current\n"), ("Changed.md", "- old\n")]);
    {
        let notebook = FolderNotebook::open(folder.root.path(), folder.state.path()).unwrap();
        let draft = |path: &str, text: &str, base: &str| Draft {
            page: page_id_for_path(path),
            path: path.into(),
            text: text.into(),
            base: vec![revision(base)],
            stamp: DraftStamp {
                session: "previous".into(),
                version: 3,
            },
            created: 1,
        };
        notebook
            .put_draft(
                &draft("Current.md", "- current\n- typed\n", "- current\n"),
                None,
            )
            .unwrap();
        notebook
            .put_draft(&draft("Changed.md", "- old\n- typed\n", "- old\n"), None)
            .unwrap();
        notebook
            .put_draft(
                &Draft {
                    base: vec![],
                    ..draft("New.md", "- never written\n", "")
                },
                None,
            )
            .unwrap();
    }
    folder.write("Changed.md", "- changed while closed\n");
    let mut session = folder.session();

    let (_current, current) = open(&mut session, "Current.md", cx);
    wait(&current, cx);
    assert_eq!(folder.read("Current.md"), "- current\n- typed\n");
    assert_eq!(conflict_disk(&current, cx), None);

    let (_new, new) = open(&mut session, "New.md", cx);
    wait(&new, cx);
    assert_eq!(folder.read("New.md"), "- never written\n");

    let (_changed, changed) = open(&mut session, "Changed.md", cx);
    assert_eq!(text(&changed, cx), "- old\n- typed\n");
    assert_eq!(
        conflict_disk(&changed, cx),
        Some(Some("- changed while closed\n".into()))
    );
    save(&changed, cx);
    assert_eq!(folder.read("Changed.md"), "- changed while closed\n");
    assert_eq!(
        draft_text(&session, "Changed.md").as_deref(),
        Some("- old\n- typed\n")
    );
    session.storage().close().unwrap();
}

#[gpui::test]
fn conflicts_follow_the_newest_disk_state(cx: &mut TestAppContext) {
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let folder = Folder::new(&[("Note.md", "- one\n"), ("Gone.md", "- gone\n")]);
    let mut session = folder.session();
    let (_page, persistence) = open(&mut session, "Note.md", cx);
    let disk = |text: &str| Some(DiskDocument::new("Note.md".into(), text.into()));

    append(&persistence, "- mine\n", cx);
    folder.write("Note.md", "- newest\n");
    persistence.update(cx, |persistence, cx| {
        persistence.save(cx);
        persistence.external(disk("- older\n"), cx);
    });
    wait(&persistence, cx);
    assert_eq!(
        conflict_disk(&persistence, cx),
        Some(Some("- newest\n".into())),
        "a change queued during the save never replaces the refusal's newer disk text"
    );

    folder.write("Note.md", "- one\n");
    persistence.update(cx, |persistence, cx| {
        persistence.external(disk("- one\n"), cx)
    });
    wait(&persistence, cx);
    assert_eq!(
        conflict_disk(&persistence, cx),
        None,
        "a file restored to the edits' base ends the conflict"
    );
    save(&persistence, cx);
    assert_eq!(folder.read("Note.md"), "- one\n- mine\n");

    let (_gone, gone) = open(&mut session, "Gone.md", cx);
    fs::remove_file(folder.root.path().join("Gone.md")).unwrap();
    gone.update(cx, |gone, cx| gone.external(None, cx));
    wait(&gone, cx);
    gone.update(cx, |gone, cx| gone.use_disk(cx));
    wait(&gone, cx);
    folder.write("Gone.md", "- back\n");
    gone.update(cx, |gone, cx| {
        gone.external(
            Some(DiskDocument::new("Gone.md".into(), "- back\n".into())),
            cx,
        )
    });
    wait(&gone, cx);
    assert_eq!(text(&gone, cx), "- back\n");
    assert!(gone.read_with(cx, |gone, cx| {
        gone.buffer().read(cx).capability() == language::Capability::ReadWrite
    }));
    append(&gone, "- again\n", cx);
    save(&gone, cx);
    assert_eq!(folder.read("Gone.md"), "- back\n- again\n");
    session.storage().close().unwrap();
}
