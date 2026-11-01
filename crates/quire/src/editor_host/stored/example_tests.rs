use super::super::example::{FIRST_PAGE, FOLDER, PAGES, ensure, launch};
use super::super::*;
use super::tests::{Notebook, active, read, settle};
use gpui::TestAppContext;
use quire_store::{profiles::Notebook as ProfileNotebook, sync::format::page_id_for_path};
use std::fs;

#[test]
fn the_example_notebook_writes_missing_sample_pages_and_keeps_every_other_file() {
    let data = tempfile::tempdir().unwrap();
    let mine = data.path().join("My notes");
    fs::create_dir_all(&mine).unwrap();
    fs::write(mine.join("Field notes.md"), "my own field notes\n").unwrap();

    let root = ensure(data.path()).unwrap();
    assert_eq!(root, data.path().join(FOLDER));
    assert_eq!(
        PAGES.map(|(name, _)| name),
        ["A place to think.md", "Field notes.md", "Reading room.md"]
    );
    for (name, text) in PAGES {
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), text);
    }

    fs::write(root.join(FIRST_PAGE), "edited\n").unwrap();
    fs::remove_file(root.join("Reading room.md")).unwrap();
    ensure(data.path()).unwrap();
    assert_eq!(
        fs::read_to_string(root.join(FIRST_PAGE)).unwrap(),
        "edited\n"
    );
    assert_eq!(
        fs::read_to_string(root.join("Reading room.md")).unwrap(),
        PAGES[2].1
    );
    assert_eq!(
        fs::read_to_string(mine.join("Field notes.md")).unwrap(),
        "my own field notes\n"
    );
    assert_eq!(fs::read_dir(&mine).unwrap().count(), 1);
}

#[test]
fn an_unwritable_example_folder_says_what_failed_and_what_to_do() {
    let data = tempfile::tempdir().unwrap();
    fs::write(data.path().join(FOLDER), "a file, not a folder").unwrap();
    let error = ensure(data.path()).unwrap_err();
    assert!(
        error.starts_with("Cannot make the example notebook folder"),
        "{error}"
    );
    assert!(error.ends_with("then reopen Quire."), "{error}");
}

#[gpui::test]
fn a_first_launch_opens_the_example_notebook_on_its_first_page(cx: &mut TestAppContext) {
    cx.update(|cx| init(cx).unwrap());
    let profile_notebook = |cx: &mut TestAppContext| {
        cx.update(|cx| crate::profiles::get(crate::profiles::active(cx), cx).notebook)
    };
    assert_eq!(profile_notebook(cx), None);
    let data = tempfile::tempdir().unwrap();
    cx.update(|cx| launch(data.path(), cx)).unwrap();
    let window = cx
        .windows()
        .into_iter()
        .find_map(|window| window.downcast::<StoredWindow>())
        .expect("the first launch opened the example notebook");
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("unused.sqlite");
    let notebook = Notebook {
        _root: state,
        path,
        window,
    };
    settle(&notebook, cx);

    assert_eq!(active(&notebook, cx), page_id_for_path(FIRST_PAGE));
    let root = fs::canonicalize(data.path().join(FOLDER)).unwrap();
    assert_eq!(
        profile_notebook(cx),
        Some(ProfileNotebook::Folder { root: root.clone() })
    );
    assert!(
        quire_store::folder_notebook::state_folder_in(data.path(), &root)
            .join("folder.sqlite")
            .exists(),
        "the example notebook keeps its state in the data folder it was given"
    );
    let client = read(&notebook, cx, |view, cx| view.index_client(cx).unwrap());
    let hits = futures::executor::block_on(client.search("loosely held".into()).unwrap()).unwrap();
    assert_eq!(
        hits.into_iter()
            .map(|hit| hit.source.path)
            .collect::<Vec<_>>(),
        ["Field notes.md"]
    );
    let mut entries: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    entries.sort();
    assert_eq!(entries, PAGES.map(|(name, _)| name));
}
