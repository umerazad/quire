//! Markdown folder import on both backends: the rt engine case, all-or-nothing refusals,
//! collisions ignoring case, the source left untouched, and cancellation before any write.

use anyhow::Result;
use quire_store::{
    folder_notebook::FolderNotebook,
    markdown_import::{CANCELED, SourceFolder, SourcePage, copy_files, read_source},
    notebook::{Edit, NotebookStore, SourceFiles},
    sync::format::{attachment_file_name, page_id_for_path},
};
use std::{
    collections::BTreeMap,
    fs,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
};

fn write(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
}

/// Every file below `root` with its bytes.
fn tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    files
}

/// The rt engine.test.ts source folder.
fn rt_source(root: &Path) {
    write(
        root,
        &[
            ("Plan.md", "- see ![](assets/a.png)\n"),
            ("People/Maram.md", "- hi\n"),
            (".obsidian/x.md", "hidden"),
            ("assets/a.png", "png"),
        ],
    );
}

fn read(source: &Path) -> SourceFolder {
    read_source(source, None, &AtomicBool::new(false), |_| {}).unwrap()
}

fn stored_text(store: &NotebookStore, path: &str) -> Option<String> {
    let checkout = store.checkout(&page_id_for_path(path), None).ok()?;
    let head = checkout.selected?;
    head.root().field("text").as_str().map(str::to_owned)
}

#[test]
fn quire_notebooks_import_new_pages_and_leave_pages_already_here_alone() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("old-vault");
    rt_source(&source);
    let before = tree(&source);
    let store = NotebookStore::open(&directory.path().join("local.sqlite"), "nb", "office")?;
    store.save(&Edit {
        page: page_id_for_path("Plan.md"),
        base: Vec::new(),
        path: "Plan.md".into(),
        text: "- already here\n".into(),
        deleted: false,
        created: 1,
        recovery: None,
    })?;
    let folder = read(&source);
    assert_eq!(folder.files, ["assets/a.png"]);
    assert_eq!(
        folder
            .pages
            .iter()
            .map(|page| page.path.as_str())
            .collect::<Vec<_>>(),
        ["People/Maram.md", "Plan.md"]
    );
    let imported = store.import_pages(&folder.pages, 2)?;
    assert_eq!(
        (imported.saved.len(), imported.skipped.as_slice()),
        (1, ["Plan.md".to_owned()].as_slice())
    );
    assert_eq!(
        stored_text(&store, "People/Maram.md").as_deref(),
        Some("- hi\n")
    );
    assert_eq!(
        stored_text(&store, "Plan.md").as_deref(),
        Some("- already here\n")
    );
    assert_eq!(tree(&source), before, "the source folder is only read");
    let again = store.import_pages(&folder.pages, 3)?;
    assert_eq!(
        (again.saved.len(), again.skipped.len()),
        (0, 2),
        "importing again changes nothing"
    );
    Ok(())
}

#[test]
fn an_unusable_path_or_a_case_collision_never_leaves_part_of_an_import() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    write(
        &source,
        &[
            ("A.md", "- a\n"),
            ("B.md", "- b\n"),
            ("Deep/CON.md", "- reserved\n"),
        ],
    );
    let store = NotebookStore::open(&directory.path().join("local.sqlite"), "nb", "office")?;
    let error = store
        .import_pages(&read(&source).pages, 1)
        .unwrap_err()
        .to_string();
    assert!(
        error.starts_with("Cannot import Deep/CON.md:") && error.ends_with("nothing was imported."),
        "{error}"
    );
    assert_eq!(stored_text(&store, "A.md"), None, "nothing was written");

    // Built directly: a case-insensitive file system cannot hold these names side by side.
    let page = |path: &str| SourcePage {
        path: path.into(),
        text: format!("- {path}\n"),
    };
    let pages = ["A.md", "B.md", "a.MD", "Cafe\u{301}.md", "Café.md"].map(page);
    let imported = store.import_pages(&pages, 2)?;
    let mut created: Vec<String> = imported
        .saved
        .iter()
        .map(|ack| {
            ack.revision
                .root()
                .field("path")
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    created.sort();
    assert_eq!(created, ["A.md", "B.md", "Café.md"]);
    assert_eq!(imported.skipped.len(), 2, "{:?}", imported.skipped);
    Ok(())
}

#[test]
fn quire_notebooks_copy_linked_files_as_attachments_and_point_links_at_them() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    write(
        &source,
        &[
            (
                "Notes/Page.md",
                "- ![Chart](../assets/chart.png) ![[../assets/report.pdf#page=2]]\n- [doc](<../assets/my doc.txt>) [web](https://example.com/a.png) ![gone](missing.png)\n",
            ),
            ("Top.md", "- ![](assets/chart.png)\n"),
            ("assets/chart.png", "chart"),
            ("assets/report.pdf", "%PDF-report"),
            ("assets/my doc.txt", "doc"),
            ("assets/unused.bin", "unused"),
        ],
    );
    let before = tree(&source);
    let store = NotebookStore::open(&directory.path().join("local.sqlite"), "nb", "office")?;
    let folder = read(&source);
    let imported = store.import_folder(
        &folder.pages,
        &SourceFiles {
            root: source.clone(),
            files: folder.files.clone(),
        },
        1,
    )?;
    assert_eq!(
        imported.attached,
        ["assets/chart.png", "assets/my doc.txt", "assets/report.pdf"],
        "only linked files are copied"
    );
    let (chart, report, doc) = (
        attachment_file_name(b"chart", "chart.png"),
        attachment_file_name(b"%PDF-report", "report.pdf"),
        attachment_file_name(b"doc", "my doc.txt"),
    );
    assert_eq!(
        stored_text(&store, "Notes/Page.md").unwrap(),
        format!(
            "- ![Chart](attachments/{chart}) ![[attachments/{report}#page=2]]\n- [doc](<attachments/{doc}>) [web](https://example.com/a.png) ![gone](missing.png)\n"
        )
    );
    assert_eq!(
        stored_text(&store, "Top.md").unwrap(),
        format!("- ![](attachments/{chart})\n")
    );
    let cache = directory.path().join("attachments");
    assert_eq!(fs::read(cache.join(&chart))?, b"chart");
    assert_eq!(fs::read(cache.join(&report))?, b"%PDF-report");
    assert_eq!(fs::read(cache.join(&doc))?, b"doc");
    assert_eq!(fs::read_dir(&cache)?.count(), 3);
    assert_eq!(tree(&source), before, "the source folder is only read");
    Ok(())
}

#[test]
fn a_deleted_page_at_the_same_path_gets_a_new_identity() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    write(&source, &[("Plan.md", "- new\n")]);
    let store = NotebookStore::open(&directory.path().join("local.sqlite"), "nb", "office")?;
    let page = page_id_for_path("Plan.md");
    let edit = |text: &str, deleted: bool, base: Vec<String>| Edit {
        page: page.clone(),
        base,
        path: "Plan.md".into(),
        text: text.into(),
        deleted,
        created: 1,
        recovery: None,
    };
    store.save(&edit("- old\n", false, Vec::new()))?;
    store.save(&edit("- old\n", true, store.heads(&page)?))?;
    let imported = store.import_pages(&read(&source).pages, 5)?;
    let created = imported.saved[0]
        .revision
        .root()
        .field("page")
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(created, page);
    assert_eq!(
        store
            .checkout(&created, None)?
            .selected
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some("- new\n")
    );
    Ok(())
}

#[test]
fn folder_notebooks_import_notes_and_files_without_replacing_anything() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let (source, root, state) = (
        directory.path().join("old-vault"),
        directory.path().join("notes"),
        directory.path().join("state"),
    );
    rt_source(&source);
    write(
        &root,
        &[("plan.md", "- already here\n"), ("assets/b.png", "kept")],
    );
    let before = tree(&source);
    let notebook = FolderNotebook::open(&root, &state)?;
    let folder = read(&source);
    let copied = copy_files(&source, &root, &folder.files)?;
    let imported = notebook.import_pages(&folder.pages, 1)?;
    assert_eq!(copied, [root.join("assets/a.png")]);
    assert_eq!(
        (imported.saved.len(), imported.skipped.as_slice()),
        (1, ["Plan.md".to_owned()].as_slice())
    );
    let after = tree(&root);
    assert_eq!(after["People/Maram.md"], b"- hi\n");
    assert_eq!(after["plan.md"], b"- already here\n");
    assert_eq!(after["assets/a.png"], b"png");
    assert_eq!(after["assets/b.png"], b"kept");
    assert_eq!(
        copy_files(&source, &root, &folder.files)?,
        Vec::<std::path::PathBuf>::new(),
        "copied files are never replaced"
    );
    assert_eq!(tree(&source), before);
    Ok(())
}

#[test]
fn reading_a_source_cancels_and_refuses_the_notebook_itself() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let source = directory.path().join("source");
    write(
        &source,
        &[
            ("B/b.md", "b"),
            ("a.md", "a"),
            ("C.md", "c"),
            ("bad.md", ""),
        ],
    );
    fs::write(source.join("bad.md"), [0x2d, 0x20, 0xff, 0x0a])?;
    #[cfg(unix)]
    std::os::unix::fs::symlink(source.join("a.md"), source.join("link.md"))?;
    let mut seen = Vec::new();
    let folder = read_source(&source, None, &AtomicBool::new(false), |count| {
        seen.push(count)
    })?;
    assert_eq!(
        folder
            .pages
            .iter()
            .map(|page| page.path.as_str())
            .collect::<Vec<_>>(),
        ["B/b.md", "C.md", "a.md", "bad.md"],
        "depth first in name order, links skipped"
    );
    assert_eq!(folder.pages[3].text, "- \u{fffd}\n");
    assert_eq!(seen, [1, 2, 3, 4]);
    let cancel = AtomicBool::new(false);
    let error = read_source(&source, None, &cancel, |_| {
        cancel.store(true, Ordering::Relaxed)
    })
    .unwrap_err();
    assert_eq!(error.to_string(), CANCELED);
    let error = read_source(
        &source.join("B"),
        Some(&source),
        &AtomicBool::new(false),
        |_| {},
    )
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Choose a folder outside this notebook to import."
    );
    Ok(())
}
