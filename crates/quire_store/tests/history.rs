use quire_store::{
    folder_notebook::FolderNotebook,
    notebook::{Edit, NotebookStore, SavedVersion, VERSION_UNAVAILABLE},
    sync::format::page_id_for_path,
};
use std::fs;

fn save(store: &NotebookStore, page: &str, text: &str, created: u64) -> String {
    store
        .save(&Edit {
            page: page.into(),
            base: store.heads(page).unwrap(),
            path: format!("{page}.md"),
            text: text.into(),
            deleted: false,
            created,
            recovery: None,
        })
        .unwrap()
        .revision_id()
        .to_owned()
}

#[test]
fn quire_notebooks_list_every_earlier_version_newest_first_without_the_current_one() {
    let root = tempfile::tempdir().unwrap();
    let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "local").unwrap();
    let first = save(&store, "p", "- first\n", 1_000);
    let second = save(&store, "p", "- second\n", 2_000);
    save(&store, "p", "- third\n", 3_000);
    let other = save(&store, "q", "- other\n", 4_000);
    assert_eq!(
        store.history("p").unwrap(),
        [
            SavedVersion {
                id: second.clone(),
                saved_at: 2_000
            },
            SavedVersion {
                id: first.clone(),
                saved_at: 1_000
            },
        ]
    );
    assert_eq!(store.version_text("p", &first).unwrap(), "- first\n");
    assert_eq!(store.version_text("p", &second).unwrap(), "- second\n");
    for (page, id) in [("p", other.as_str()), ("p", "missing")] {
        assert_eq!(
            store.version_text(page, id).unwrap_err().to_string(),
            VERSION_UNAVAILABLE
        );
    }
    assert!(store.history("q").unwrap().is_empty());
}

#[test]
fn folder_notebooks_list_saved_versions_by_page() {
    let folder = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    fs::write(folder.path().join("Note.md"), "- disk\n").unwrap();
    let clock = std::cell::Cell::new(10_000);
    let notebook = FolderNotebook::open_with_clock(
        folder.path(),
        state.path(),
        Box::new(move || {
            clock.set(clock.get() + 1_000);
            clock.get()
        }),
    )
    .unwrap();
    let page = page_id_for_path("Note.md");
    for text in ["- one\n", "- two\n"] {
        notebook
            .save(&Edit {
                page: page.clone(),
                base: notebook.heads(&page).unwrap(),
                path: "Note.md".into(),
                text: text.into(),
                deleted: false,
                created: 1,
                recovery: None,
            })
            .unwrap();
    }
    let listed = notebook.history(&page).unwrap();
    let texts: Vec<String> = listed
        .iter()
        .map(|saved| notebook.version_text(&page, &saved.id).unwrap())
        .collect();
    assert_eq!(texts, ["- one\n", "- disk\n"]);
    assert!(listed[0].saved_at > listed[1].saved_at);
    assert_eq!(
        notebook
            .version_text(&page, "not-a-row")
            .unwrap_err()
            .to_string(),
        VERSION_UNAVAILABLE
    );
    assert!(
        notebook
            .history(&page_id_for_path("Missing.md"))
            .unwrap()
            .is_empty()
    );
}
