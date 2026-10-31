use super::super::*;
use super::file_tests::{dialog_error, summary, visit};
use super::tests::{Notebook, active, draw, read, settle, wait_until};
use crate::page_persistence::PagePersistence;
use gpui::TestAppContext;
use quire_core::workspace::Destination;
use quire_store::sync::format::page_id_for_path;
use std::{fs, time::Duration};

struct FolderNotebook {
    notebook: Notebook,
    folder: tempfile::TempDir,
    /// Stands in for the system Trash: deleted items move here, numbered in order.
    bin: tempfile::TempDir,
}

impl FolderNotebook {
    fn file(&self, path: &str) -> std::path::PathBuf {
        self.folder.path().join(path)
    }

    fn trashed(&self, item: usize) -> std::path::PathBuf {
        self.bin.path().join(item.to_string())
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
    let bin = tempfile::tempdir().unwrap();
    let into = bin.path().to_owned();
    let trash: quire_store::folder_notebook::Trash = std::sync::Arc::new(move |path| {
        let next = fs::read_dir(&into)?.count();
        Ok(fs::rename(path, into.join(next.to_string()))?)
    });
    let mut session =
        NotebookSession::open_folder(folder.path().into(), state.path().into(), Some(trash))
            .unwrap();
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
    FolderNotebook {
        notebook,
        folder,
        bin,
    }
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
fn deleting_a_folder_page_moves_it_to_the_trash_and_forgets_it(cx: &mut TestAppContext) {
    let opened = open_folder(
        &[
            ("Keep.md", "- keep\n"),
            ("Old.md", "- Original\n"),
            ("Links.md", "- [[Old]]\n"),
        ],
        "Keep.md",
        cx,
    );
    let notebook = &opened.notebook;
    let old = page_id_for_path("Old.md");
    visit(notebook, Destination::page(&old), cx);
    notebook
        .window
        .update(cx, |view, _, cx| view.toggle_favorite(cx))
        .unwrap();
    wait_until(cx, "Old's editor", |cx| {
        read(notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == old
        })
    });
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i x escape");
    cx.dispatch_action(notebook.window.into(), crate::navigation::DeletePage);
    let (heading, detail, _) = summary(notebook, cx);
    assert_eq!(heading, "Delete Old?");
    assert_eq!(
        detail,
        "1 link from other pages will point to a missing page. The files go to the system Trash, where you can restore them."
    );
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the deletion", |cx| {
        read(notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    assert!(!opened.file("Old.md").exists());
    assert_eq!(
        fs::read_to_string(opened.trashed(0)).unwrap(),
        "x- Original\n",
        "unsaved typing is saved before the note goes to the Trash"
    );
    read(notebook, cx, |view, cx| {
        let gone = Destination::page(&old);
        assert!(!view.nav.workspace.tabs.contains(&gone));
        assert!(!view.nav.workspace.favorites.contains(&gone));
        assert!(!view.nav.workspace.recent.contains(&gone));
        assert!(
            view.session
                .borrow()
                .open_pages()
                .iter()
                .all(|page| page.read(cx).id() != old)
        );
    });
    assert_eq!(active(notebook, cx), page_id_for_path("Keep.md"));
    wait_until(cx, "the page list without Old", |cx| {
        read(notebook, cx, |view, _| {
            view.nav.catalogue.is_none() && view.nav.title(&old).is_none()
        })
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert!(!opened.file("Old.md").exists(), "nothing revives the note");
}

#[gpui::test]
fn deleting_a_folder_moves_it_whole_unless_a_page_appeared_or_changed(cx: &mut TestAppContext) {
    let opened = open_folder(
        &[
            ("Keep.md", "- keep\n"),
            ("Projects/A.md", "- a\n"),
            ("Projects/Deep/B.md", "- b\n"),
            ("Projects/photo.png", "png"),
        ],
        "Keep.md",
        cx,
    );
    let notebook = &opened.notebook;
    let ask = |cx: &mut TestAppContext| {
        notebook
            .window
            .update(cx, |view, window, cx| {
                view.ask_delete(
                    crate::file_actions::Target::Folder("Projects".into()),
                    window,
                    cx,
                )
            })
            .unwrap();
        summary(notebook, cx)
    };
    let (heading, detail, listed) = ask(cx);
    assert_eq!(
        heading,
        "Delete Projects with its 2 pages and 1 other file?"
    );
    assert_eq!(
        detail,
        "No other page links here. The files go to the system Trash, where you can restore them."
    );
    assert_eq!(listed, ["Projects/A.md", "Projects/Deep/B.md"]);
    fs::write(opened.file("Projects/Late.md"), "- late\n").unwrap();
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the refusal", |cx| dialog_error(notebook, cx).is_some());
    assert_eq!(
        dialog_error(notebook, cx).unwrap(),
        "Projects/Late.md appeared in Projects after you chose to delete it. Nothing was deleted; try again."
    );
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    fs::remove_file(opened.file("Projects/Late.md")).unwrap();

    ask(cx);
    fs::write(opened.file("Projects/A.md"), "- edited elsewhere\n").unwrap();
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the stale refusal", |cx| {
        dialog_error(notebook, cx).is_some()
    });
    assert_eq!(
        dialog_error(notebook, cx).unwrap(),
        crate::file_actions::STALE_DELETION
    );
    assert!(!opened.trashed(0).exists());
    cx.simulate_keystrokes(notebook.window.into(), "escape");

    ask(cx);
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "enter");
    wait_until(cx, "the folder deletion", |cx| {
        read(notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    assert!(!opened.file("Projects").exists());
    assert_eq!(
        fs::read_to_string(opened.trashed(0).join("photo.png")).unwrap(),
        "png"
    );
    assert_eq!(
        fs::read_to_string(opened.trashed(0).join("A.md")).unwrap(),
        "- edited elsewhere\n"
    );
    assert!(opened.file("Keep.md").exists());
}

#[gpui::test]
fn folder_notebooks_refuse_rename_before_touching_files(cx: &mut TestAppContext) {
    let folder = open_folder(&[("Note.md", "- keep\n")], "Note.md", cx);
    let (open, index, storage) = read(&folder.notebook, cx, |view, cx| {
        let session = view.session.borrow();
        let index = view.index.as_ref().unwrap().read(cx).client().clone();
        (session.open_pages(), index, session.storage())
    });
    let page = page_id_for_path("Note.md");
    let rename = cx.update(|cx| {
        crate::file_actions::plan_rename(page, "Renamed".into(), open, index, storage, cx)
    });
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

/// `.quire/notebook.json` as the reference `writeNotebookSettings` writes it.
const RT_SETTINGS: &str = "{\n  \"version\": 1,\n  \"daily\": {\n    \"folder\": \"Journal\",\n    \"title\": \"{DD}.{MM}.{YYYY}\"\n  }\n}\n";

fn save_layout(notebook: &Notebook, folder: &str, title: &str, cx: &mut TestAppContext) {
    let (folder, title) = (folder.to_owned(), title.to_owned());
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.edit_layout(window, cx);
            let [folder_field, title_field] = view.nav.daily.editing.clone().unwrap();
            folder_field.update(cx, |editor, cx| editor.set_text(folder, window, cx));
            title_field.update(cx, |editor, cx| editor.set_text(title, window, cx));
            view.save_layout(cx);
        })
        .unwrap();
    cx.run_until_parked();
}

#[gpui::test]
fn folder_notebooks_use_and_save_the_reference_daily_layout(cx: &mut TestAppContext) {
    let opened = open_folder(
        &[("Note.md", "- a\n"), (".quire/notebook.json", RT_SETTINGS)],
        "Note.md",
        cx,
    );
    let notebook = &opened.notebook;
    let layout = read(notebook, cx, |view, _| {
        assert!(view.nav.daily.read_only.is_none());
        view.nav.daily.layout.clone()
    });
    assert_eq!(
        layout,
        quire_core::daily::DailyLayout {
            folder: "Journal".into(),
            title: "{DD}.{MM}.{YYYY}".into(),
        }
    );

    save_layout(notebook, "Daily/{YYYY}", "{YYYY}-{MM}-{DD}", cx);
    wait_until(cx, "the saved layout", |cx| {
        read(notebook, cx, |view, _| view.nav.daily.editing.is_none())
    });
    let saved = RT_SETTINGS
        .replace("Journal", "Daily/{YYYY}")
        .replace("{DD}.{MM}.{YYYY}", "{YYYY}-{MM}-{DD}");
    assert_eq!(
        fs::read_to_string(opened.file(".quire/notebook.json")).unwrap(),
        saved
    );

    fs::write(opened.file(".quire/notebook.json"), RT_SETTINGS).unwrap();
    save_layout(notebook, "Elsewhere", "{YYYY}-{MM}-{DD}", cx);
    wait_until(cx, "the refusal", |cx| {
        read(notebook, cx, |view, _| view.nav.error.clone())
            .is_some_and(|error| error.contains("changed in another application"))
    });
    assert_eq!(
        fs::read_to_string(opened.file(".quire/notebook.json")).unwrap(),
        RT_SETTINGS,
        "a layout saved elsewhere is never overwritten"
    );
}

#[gpui::test]
fn newer_reference_settings_stay_read_only_and_unchanged(cx: &mut TestAppContext) {
    let newer = "{\"version\":2,\"daily\":{\"folder\":\"X\"}}";
    let opened = open_folder(
        &[("Note.md", "- a\n"), (".quire/notebook.json", newer)],
        "Note.md",
        cx,
    );
    read(&opened.notebook, cx, |view, _| {
        assert_eq!(
            view.nav.daily.layout,
            quire_core::daily::DailyLayout::default()
        );
        assert!(
            view.nav
                .daily
                .read_only
                .as_ref()
                .unwrap()
                .contains("newer Quire")
        );
    });
    save_layout(&opened.notebook, "Daily", "{YYYY}-{MM}-{DD}", cx);
    assert_eq!(
        fs::read_to_string(opened.file(".quire/notebook.json")).unwrap(),
        newer
    );
}
