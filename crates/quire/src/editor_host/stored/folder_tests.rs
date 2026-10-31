use super::super::*;
use super::file_tests::{dialog_error, rename_to, summary, visit};
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
    open_folder_with(files, &[], first, cx)
}

fn write_files(root: &std::path::Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
}

/// Opens a folder whose Quire state folder already holds `state_files`, as a crash leaves it.
fn open_folder_with(
    files: &[(&str, &str)],
    state_files: &[(&str, &str)],
    first: &str,
    cx: &mut TestAppContext,
) -> FolderNotebook {
    cx.update(|cx| init(cx).unwrap());
    let folder = tempfile::tempdir().unwrap();
    write_files(folder.path(), files);
    let state = tempfile::tempdir().unwrap();
    write_files(state.path(), state_files);
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

/// The Quire-notebook rename fixture (`file_tests::RENAMED`) as files.
const RENAMED: [(&str, &str); 4] = [
    (
        "Old name.md",
        "---\ntags: [x]\ntitle: \"Old name\"\n---\n- Body ^stable [[Old name]]\n",
    ),
    (
        "Links.md",
        "- [[Old name]] and [[Old name#Part|alias]] and ![[Old name#^stable]]\r\n- `[[Old name]]` [x](Old%20name.md) [[Keep]]\r\n",
    ),
    ("Notes/Plan.md", "- see [[Old name]]\n"),
    ("Keep.md", "- keep\n"),
];

#[gpui::test]
fn renaming_a_folder_page_moves_its_file_rewrites_links_and_keeps_its_tabs(
    cx: &mut TestAppContext,
) {
    let opened = open_folder(&RENAMED, "Notes/Plan.md", cx);
    let notebook = &opened.notebook;
    let (old, new) = (
        page_id_for_path("Old name.md"),
        page_id_for_path("New name.md"),
    );
    wait_until(cx, "Plan's editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i y escape");
    visit(notebook, Destination::page(&old), cx);
    notebook
        .window
        .update(cx, |view, _, cx| view.toggle_favorite(cx))
        .unwrap();
    wait_until(cx, "Old name's editor", |cx| {
        read(notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == old
        })
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::RenamePage);
    rename_to(notebook, "New name", cx);
    wait_until(cx, "the rename", |cx| {
        read(notebook, cx, |view, _| view.nav.dialog.is_none())
    });
    // The same text the Quire-notebook rename writes, through `quire_core::relocate`.
    assert_eq!(
        fs::read_to_string(opened.file("New name.md")).unwrap(),
        "---\ntags: [x]\ntitle: \"New name\"\n---\n- Body ^stable [[New name]]\n"
    );
    assert_eq!(
        fs::read_to_string(opened.file("Links.md")).unwrap(),
        "- [[New name]] and [[New name#Part|alias]] and ![[New name#^stable]]\r\n- `[[Old name]]` [x](New%20name.md) [[Keep]]\r\n"
    );
    assert_eq!(
        fs::read_to_string(opened.file("Notes/Plan.md")).unwrap(),
        "y- see [[New name]]\n"
    );
    assert!(!opened.file("Old name.md").exists());
    let state = notebook.path.parent().unwrap().to_owned();
    let archived: Vec<String> = fs::read_dir(state.join("retired"))
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect();
    assert_eq!(archived, [RENAMED[0].1], "the old file is archived");
    assert_eq!(fs::read_dir(state.join("transactions")).unwrap().count(), 0);

    wait_until(cx, "the new page to open", |cx| active(notebook, cx) == new);
    read(notebook, cx, |view, _| {
        let renamed = Destination::page(&new);
        assert!(view.nav.workspace.favorites.contains(&renamed));
        assert!(view.nav.workspace.tabs.contains(&renamed));
        assert!(!view.nav.workspace.tabs.contains(&Destination::page(&old)));
    });
    wait_until(cx, "the page list with the new name", |cx| {
        read(notebook, cx, |view, _| {
            view.nav.renamed.is_none()
                && view.nav.title(&new) == Some("New name")
                && view.nav.title(&old).is_none()
        })
    });
    wait_until(cx, "the renamed page's editor", |cx| {
        read(notebook, cx, |view, cx| {
            view.editor.is_some() && view.page.read(cx).id() == new
        })
    });
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "i z escape");
    cx.simulate_keystrokes(notebook.window.into(), &super::tests::key("s"));
    wait_until(cx, "the save after renaming", |_| {
        fs::read_to_string(opened.file("New name.md"))
            .unwrap()
            .starts_with('z')
    });
    cx.executor().advance_clock(Duration::from_secs(3));
    cx.run_until_parked();
    assert!(
        !opened.file("Old name.md").exists(),
        "no later save revives the old file"
    );
}

#[gpui::test]
fn renaming_a_folder_page_refuses_taken_names_and_files_created_meanwhile(cx: &mut TestAppContext) {
    let opened = open_folder(&RENAMED, "Old name.md", cx);
    let notebook = &opened.notebook;
    wait_until(cx, "Old name's editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::RenamePage);
    rename_to(notebook, "keep", cx);
    wait_until(cx, "the refusal", |cx| dialog_error(notebook, cx).is_some());
    assert_eq!(
        dialog_error(notebook, cx).unwrap(),
        quire_core::relocate::TAKEN
    );
    // A file the index has not seen yet is still never replaced.
    fs::write(opened.file("fresh.md"), "- someone else's\n").unwrap();
    rename_to(notebook, "Fresh", cx);
    wait_until(cx, "the second refusal", |cx| {
        dialog_error(notebook, cx).is_some_and(|error| error.contains("already exists"))
    });
    assert_eq!(
        fs::read_to_string(opened.file("fresh.md")).unwrap(),
        "- someone else's\n"
    );
    for (path, text) in RENAMED {
        assert_eq!(fs::read_to_string(opened.file(path)).unwrap(), text);
    }
}

#[gpui::test]
fn a_rename_interrupted_by_a_crash_finishes_on_open_and_says_so(cx: &mut TestAppContext) {
    let old = "- old [[Old]]\n";
    let journal = serde_json::json!({
        "edits": [
            {
                "path": "New.md",
                "text": "- old [[New]]\n",
                "expectedRevision": null,
                "historyFrom": "Old.md",
            },
            {
                "path": "Old.md",
                "text": "",
                "expectedRevision": quire_store::folder_notebook::revision(old),
                "remove": true,
                "retirement": "0123456789abcdef0123456789abcdef",
            },
        ],
        "createdAt": 1,
    })
    .to_string();
    let opened = open_folder_with(
        &[("Old.md", old), ("Keep.md", "- keep\n")],
        &[("transactions/interrupted.json", &journal)],
        "Keep.md",
        cx,
    );
    let notebook = &opened.notebook;
    notebook
        .window
        .update(cx, |view, _, cx| view.load_folder_notices(cx))
        .unwrap();
    wait_until(cx, "the notice", |cx| {
        read(notebook, cx, |view, _| !view.folder.notices.is_empty())
    });
    let notices = read(notebook, cx, |view, _| view.folder.notices.clone());
    assert_eq!(notices.len(), 1);
    assert!(notices[0].starts_with("Finished an interrupted change"));
    draw(notebook, cx);
    assert_eq!(
        fs::read_to_string(opened.file("New.md")).unwrap(),
        "- old [[New]]\n"
    );
    assert!(!opened.file("Old.md").exists());
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
