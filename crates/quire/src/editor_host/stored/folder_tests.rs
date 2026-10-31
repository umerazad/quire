use super::super::*;
use super::tests::{Notebook, active, draw, read, settle, wait_until};
use crate::page_persistence::PagePersistence;
use gpui::TestAppContext;
use quire_store::sync::format::page_id_for_path;
use std::{fs, time::Duration};

struct FolderNotebook {
    notebook: Notebook,
    folder: tempfile::TempDir,
}

impl FolderNotebook {
    fn file(&self, path: &str) -> std::path::PathBuf {
        self.folder.path().join(path)
    }
}

fn open_folder(files: &[(&str, &str)], first: &str, cx: &mut TestAppContext) -> FolderNotebook {
    cx.update(|cx| init(cx).unwrap());
    let folder = tempfile::tempdir().unwrap();
    for (path, text) in files {
        let target = folder.path().join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
    let state = tempfile::tempdir().unwrap();
    let mut session =
        NotebookSession::open_folder(folder.path().into(), state.path().into()).unwrap();
    let page = cx.update(|cx| session.open_page(page_id_for_path(first), cx));
    let database = state.path().join("folder.sqlite");
    let window = cx.add_window(|window, cx| {
        StoredWindow::new(
            session,
            page.clone(),
            database.clone(),
            "folder".into(),
            window,
            cx,
        )
    });
    let notebook = Notebook {
        _root: state,
        path: database,
        window,
    };
    settle(&notebook, cx);
    FolderNotebook { notebook, folder }
}

fn persistence(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<PagePersistence> {
    read(notebook, cx, |view, cx| {
        view.page.read(cx).persistence().cloned().unwrap()
    })
}

fn text(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let persistence = persistence(notebook, cx);
    cx.update(|cx| persistence.read(cx).buffer().read(cx).text())
}

fn changed(notebook: &Notebook, paths: &[&str], cx: &mut TestAppContext) {
    let paths = paths.iter().map(|path| path.to_string()).collect();
    notebook
        .window
        .update(cx, |view, _, cx| view.folder_changed(Some(paths), cx))
        .unwrap();
}

fn search(notebook: &Notebook, query: &str, cx: &mut TestAppContext) -> Vec<String> {
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    futures::executor::block_on(client.search(query.into()).unwrap())
        .unwrap()
        .into_iter()
        .map(|hit| hit.source.path)
        .collect()
}

#[gpui::test]
fn folder_notebooks_index_their_files_and_follow_outside_edits(cx: &mut TestAppContext) {
    let opened = open_folder(
        &[
            ("Alpha.md", "- links to [[Beta]]\n"),
            ("Beta.md", "- bee\n"),
            (".hidden/Gamma.md", "- hidden\n"),
        ],
        "Alpha.md",
        cx,
    );
    let notebook = &opened.notebook;
    let titles = read(notebook, cx, |view, _| {
        view.nav
            .pages
            .iter()
            .map(|listed| listed.page.title.clone())
            .collect::<Vec<_>>()
    });
    assert_eq!(titles, ["Alpha", "Beta"]);
    assert_eq!(search(notebook, "bee", cx), ["Beta.md"]);

    fs::write(opened.file("Alpha.md"), "- reloaded [[Beta]]\n").unwrap();
    changed(notebook, &["Alpha.md"], cx);
    wait_until(cx, "the open page to reload", |cx| {
        text(notebook, cx) == "- reloaded [[Beta]]\n"
    });
    fs::write(opened.file("Beta.md"), "- wasp\n").unwrap();
    changed(notebook, &["Beta.md"], cx);
    wait_until(cx, "the closed page to be reindexed", |cx| {
        search(notebook, "wasp", cx) == ["Beta.md"]
    });
    assert!(search(notebook, "bee", cx).is_empty());

    let page = persistence(notebook, cx);
    page.update(cx, |page, cx| {
        page.buffer()
            .update(cx, |buffer, cx| buffer.edit([(0..0, "- mine\n")], None, cx))
    });
    fs::write(opened.file("Alpha.md"), "- theirs\n").unwrap();
    changed(notebook, &["Alpha.md"], cx);
    wait_until(cx, "the conflict", |cx| {
        read(notebook, cx, |view, cx| view.conflicted(cx).is_some())
    });
    draw(notebook, cx);
    assert_eq!(text(notebook, cx), "- mine\n- reloaded [[Beta]]\n");
    cx.executor().advance_clock(Duration::from_secs(5));
    cx.run_until_parked();
    assert_eq!(
        fs::read_to_string(opened.file("Alpha.md")).unwrap(),
        "- theirs\n",
        "autosave never overwrites the changed file"
    );

    notebook
        .window
        .update(cx, |view, window, cx| view.save_conflict_copy(window, cx))
        .unwrap();
    wait_until(cx, "the copy to open", |cx| {
        active(notebook, cx) != page_id_for_path("Alpha.md")
    });
    let copy = fs::read_dir(opened.folder.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .find(|name| name.starts_with("Alpha (recovered"))
        .expect("the copy is a new note in the folder");
    assert_eq!(
        fs::read_to_string(opened.file(&copy)).unwrap(),
        "- mine\n- reloaded [[Beta]]\n"
    );
    wait_until(cx, "the disk version", |cx| {
        page.read_with(cx, |page, cx| {
            page.conflict().is_none() && page.buffer().read(cx).text() == "- theirs\n"
        })
    });
    assert_eq!(
        fs::read_to_string(opened.file("Alpha.md")).unwrap(),
        "- theirs\n"
    );
}

#[gpui::test]
fn the_folder_watcher_reloads_an_open_page_edited_elsewhere(cx: &mut TestAppContext) {
    let opened = open_folder(&[("Note.md", "- one\n")], "Note.md", cx);
    let notebook = &opened.notebook;
    notebook
        .window
        .update(cx, |view, _, cx| view.watch_folder(cx))
        .unwrap();
    let mut attempts = 0;
    wait_until(cx, "the watcher to reload the page", |cx| {
        // The watcher starts asynchronously; keep editing until it reports a change.
        if attempts % 25 == 0 {
            fs::write(opened.file("Note.md"), format!("- edited {attempts}\n")).unwrap();
        }
        attempts += 1;
        text(notebook, cx).starts_with("- edited")
    });
}

#[gpui::test]
fn folder_notebooks_refuse_delete_and_rename_before_touching_files(cx: &mut TestAppContext) {
    let folder = open_folder(&[("Note.md", "- keep\n")], "Note.md", cx);
    let (open, index, storage) = read(&folder.notebook, cx, |view, cx| {
        let session = view.session.borrow();
        let index = view.index.as_ref().unwrap().read(cx).client().clone();
        (session.open_pages(), index, session.storage())
    });
    let page = page_id_for_path("Note.md");
    let deletion = cx.update(|cx| {
        crate::file_actions::plan_deletion(
            crate::file_actions::Target::Page(page.clone()),
            open.clone(),
            index.clone(),
            storage.clone(),
            cx,
        )
    });
    let rename = cx.update(|cx| {
        crate::file_actions::plan_rename(page, "Renamed".into(), open, index, storage, cx)
    });
    assert_eq!(
        futures::executor::block_on(deletion).err().as_deref(),
        Some(crate::page_writes::FOLDER_DELETION)
    );
    assert_eq!(
        futures::executor::block_on(rename).err().as_deref(),
        Some(crate::file_actions::FOLDER_RENAME)
    );
    assert_eq!(
        fs::read_to_string(folder.file("Note.md")).unwrap(),
        "- keep\n"
    );
    assert!(!folder.file("Renamed.md").exists());
}
