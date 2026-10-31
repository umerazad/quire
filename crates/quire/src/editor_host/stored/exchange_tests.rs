use super::super::*;
use super::tests::{Notebook, draw, open_notebook, read, settle, wait_until};
use crate::exchange::{
    CancelExchange, Exchange, ExportNotebookMarkdown, ExportPageMarkdown, ImportMarkdownFolder,
};
use gpui::TestAppContext;
use quire_store::{
    markdown_import::CANCELED,
    notebook::NotebookStore,
    sync::format::{attachment_file_name, page_id_for_path},
};
use std::{collections::BTreeMap, fs, path::Path};

fn write_files(root: &Path, files: &[(&str, &str)]) {
    for (path, text) in files {
        let target = root.join(path);
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(target, text).unwrap();
    }
}

/// Every file below `root` with its text.
fn tree(root: &Path) -> BTreeMap<String, String> {
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                files.insert(relative, fs::read_to_string(&path).unwrap());
            }
        }
    }
    files
}

/// The rt engine.test.ts import folder.
fn rt_source(root: &Path) {
    write_files(
        root,
        &[
            ("Plan.md", "- see ![](assets/a.png)\n"),
            ("People/Maram.md", "- hi\n"),
            (".obsidian/x.md", "hidden"),
            ("assets/a.png", "png"),
        ],
    );
}

pub(super) fn dispatch(notebook: &Notebook, action: impl gpui::Action, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), action);
}

pub(super) fn exchange(notebook: &Notebook, cx: &mut TestAppContext) -> Entity<Exchange> {
    read(notebook, cx, |view, _| view.exchange.clone())
}

/// The finished import or export message once nothing runs.
pub(super) fn finished(notebook: &Notebook, cx: &mut TestAppContext) -> String {
    let status = exchange(notebook, cx);
    wait_until(cx, "the import or export to finish", |cx| {
        status.read_with(cx, |status, _| {
            !status.is_running() && status.message().is_some()
        })
    });
    status.read_with(cx, |status, _| status.message().unwrap().to_string())
}

/// Answers the folder dialog with `folder`.
fn choose_folder(folder: &Path, cx: &mut TestAppContext) {
    wait_until(cx, "the folder dialog", |cx| cx.did_prompt_for_paths());
    let folder = folder.to_owned();
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.directories && !options.files);
        Some(vec![folder])
    });
}

/// Runs one step at a time until the import or export shows Cancel, then presses it.
fn cancel_when_running(notebook: &Notebook, cx: &mut TestAppContext) {
    let status = exchange(notebook, cx);
    while !status.read_with(cx, |status, _| {
        matches!(
            status,
            Exchange::Running {
                cancel: Some(_),
                ..
            }
        )
    }) {
        assert!(cx.executor().tick(), "the import or export never started");
    }
    dispatch(notebook, CancelExchange, cx);
}

fn stored_text(database: &Path, path: &str) -> Option<String> {
    let store = NotebookStore::open(database, "nb", "local").unwrap();
    let head = store
        .checkout(&page_id_for_path(path), None)
        .ok()?
        .selected?;
    head.root().field("text").as_str().map(str::to_owned)
}

fn listed(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<String> {
    let mut paths: Vec<String> = read(notebook, cx, |view, _| {
        view.nav
            .pages
            .iter()
            .map(|listed| listed.page.path.clone())
            .collect()
    });
    paths.sort();
    paths
}

#[gpui::test]
fn importing_a_markdown_folder_adds_new_pages_and_leaves_pages_already_here(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(
        &[(&page_id_for_path("Plan.md"), "Plan.md", "- already here\n")],
        &page_id_for_path("Plan.md"),
        cx,
    );
    settle(&notebook, cx);
    let source = tempfile::tempdir().unwrap();
    rt_source(source.path());
    let before = tree(source.path());

    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Imported 1 pages and 0 other files. 1 pages were already here and were left unchanged. 1 other files no imported page links to stayed in the source folder: a Quire notebook keeps only the files its pages link to."
    );
    wait_until(cx, "the page list", |cx| listed(&notebook, cx).len() == 2);
    assert_eq!(listed(&notebook, cx), ["People/Maram.md", "Plan.md"]);
    assert_eq!(
        stored_text(&notebook.path, "People/Maram.md").as_deref(),
        Some("- hi\n")
    );
    assert_eq!(
        stored_text(&notebook.path, "Plan.md").as_deref(),
        Some("- already here\n")
    );
    assert_eq!(
        tree(source.path()),
        before,
        "the source folder is only read"
    );
}

#[gpui::test]
fn importing_into_a_quire_notebook_attaches_linked_files(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let source = tempfile::tempdir().unwrap();
    rt_source(source.path());
    write_files(source.path(), &[("assets/unused.txt", "unused")]);

    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Imported 2 pages and 1 other files. 1 other files no imported page links to stayed in the source folder: a Quire notebook keeps only the files its pages link to."
    );
    let name = attachment_file_name(b"png", "a.png");
    assert_eq!(
        stored_text(&notebook.path, "Plan.md").unwrap(),
        format!("- see ![](attachments/{name})\n")
    );
    let cache = notebook.path.parent().unwrap().join("attachments");
    assert_eq!(fs::read(cache.join(&name)).unwrap(), b"png");
}

#[gpui::test]
fn canceling_an_import_writes_nothing_and_importing_again_brings_everything(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let source = tempfile::tempdir().unwrap();
    write_files(source.path(), &[("A.md", "- a\n"), ("B/C.md", "- c\n")]);

    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    cancel_when_running(&notebook, cx);
    assert_eq!(finished(&notebook, cx), CANCELED);
    assert_eq!(
        stored_text(&notebook.path, "A.md"),
        None,
        "nothing was written"
    );

    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Imported 2 pages and 0 other files."
    );
    assert_eq!(
        stored_text(&notebook.path, "B/C.md").as_deref(),
        Some("- c\n")
    );
}

#[gpui::test]
fn an_unusable_name_refuses_the_whole_import(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let source = tempfile::tempdir().unwrap();
    write_files(
        source.path(),
        &[("A.md", "- a\n"), ("Z/CON.md", "- reserved\n")],
    );
    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    let message = finished(&notebook, cx);
    assert!(message.starts_with("Cannot import Z/CON.md:"), "{message}");
    assert!(message.ends_with("nothing was imported."), "{message}");
    assert_eq!(stored_text(&notebook.path, "A.md"), None);
}

#[gpui::test]
fn exporting_the_shown_page_flattens_it_as_rt_does(cx: &mut TestAppContext) {
    let pages = [
        (
            "plan",
            "Plan.md",
            "# Plan\n\n- Goals for [[Launch|the launch]] ➕ [[2026-10-29]] ^goal\n  - Ship by **December**\n- See ((goal))\n- {{embed: ((src))}}\n- ![[diagram.png]]\n- ![[Notes]]\n- ![Chart](img/chart.png)\n- [[Missing page]] stays as text\n",
        ),
        (
            "source",
            "Source.md",
            "- Embedded root ^src\n  - Embedded child [[Plan]]\n",
        ),
        ("notes", "Notes.md", "- Note body\n"),
    ];
    let notebook = open_notebook(&pages, "plan", cx);
    settle(&notebook, cx);
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.md");

    dispatch(&notebook, ExportPageMarkdown, cx);
    wait_until(cx, "the save dialog", |cx| cx.did_prompt_for_new_path());
    let chosen = target.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    assert_eq!(finished(&notebook, cx), "Exported Plan as Markdown.");
    // A Quire notebook has no attachment files yet, so links keep their notebook paths as rt
    // writes them when no file is found.
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# Plan\n\n- Goals for the launch\n  - Ship by **December**\n- See Goals for the launch\n- Embedded root\n  - Embedded child Plan\n- ![diagram.png](diagram.png)\n- Notes\n  - Note body\n- ![Chart](img/chart.png)\n- Missing page stays as text\n"
    );
    let file = exchange(&notebook, cx).read_with(cx, |status, _| match status {
        Exchange::Finished { file, .. } => file.clone(),
        _ => None,
    });
    assert_eq!(file, Some(target));
    draw(&notebook, cx);
}

#[gpui::test]
fn exporting_a_notebook_confirms_replacing_and_cancels_without_writing(cx: &mut TestAppContext) {
    let pages = [
        ("a", "A.md", "- a ![x](attachments/abc-x.png)\n"),
        ("b", "Deep/B.md", "- b\r\n"),
    ];
    let notebook = open_notebook(&pages, "a", cx);
    settle(&notebook, cx);
    let out = tempfile::tempdir().unwrap();

    dispatch(&notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    cancel_when_running(&notebook, cx);
    assert_eq!(finished(&notebook, cx), super::exchange::EXPORT_CANCELED);
    assert!(
        tree(out.path()).is_empty(),
        "a canceled export leaves nothing"
    );

    write_files(out.path(), &[("A.md", "mine")]);
    dispatch(&notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    wait_until(cx, "the replace question", |cx| cx.has_pending_prompt());
    let folder = out.path().display().to_string();
    assert_eq!(
        cx.pending_prompt(),
        Some((
            format!("Replace 1 file in {folder}?"),
            "1 file already exist: A.md. Replacing them cannot be undone. Choose Cancel to pick an empty folder instead.".into()
        ))
    );
    cx.simulate_prompt_answer("Cancel");
    assert_eq!(finished(&notebook, cx), super::exchange::EXPORT_CANCELED);
    assert_eq!(
        tree(out.path()),
        BTreeMap::from([("A.md".into(), "mine".into())])
    );

    dispatch(&notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    wait_until(cx, "the replace question", |cx| cx.has_pending_prompt());
    cx.simulate_prompt_answer("Replace");
    assert_eq!(
        finished(&notebook, cx),
        format!(
            "Exported 2 pages and 0 other files to {folder}. Not exported: page history and earlier versions; 1 attachment the pages link to (attachments/abc-x.png); sync logs, notebook settings, and this computer's records."
        )
    );
    assert_eq!(
        tree(out.path()),
        BTreeMap::from([
            ("A.md".into(), "- a ![x](attachments/abc-x.png)\n".into()),
            ("Deep/B.md".into(), "- b\r\n".into()),
        ]),
        "stored text with its own line breaks, and no staging folder left"
    );
}

/// A folder notebook window over `files`, with the folder kept for inspection.
fn open_folder(
    files: &[(&str, &str)],
    first: &str,
    cx: &mut TestAppContext,
) -> (Notebook, tempfile::TempDir) {
    cx.update(|cx| init(cx).unwrap());
    open_folder_initialized(files, first, cx)
}

/// `open_folder` for a test that has already initialized the editor.
pub(super) fn open_folder_initialized(
    files: &[(&str, &str)],
    first: &str,
    cx: &mut TestAppContext,
) -> (Notebook, tempfile::TempDir) {
    let folder = tempfile::tempdir().unwrap();
    write_files(folder.path(), files);
    let state = tempfile::tempdir().unwrap();
    let mut session =
        NotebookSession::open_folder(folder.path().into(), state.path().into(), None).unwrap();
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
    (notebook, folder)
}

#[gpui::test]
fn folder_notebooks_import_files_and_export_every_page_and_file(cx: &mut TestAppContext) {
    let (notebook, folder) = open_folder(&[("Keep.md", "- keep\n")], "Keep.md", cx);
    let source = tempfile::tempdir().unwrap();
    rt_source(source.path());

    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Imported 2 pages and 1 other files."
    );
    assert_eq!(
        tree(folder.path()),
        BTreeMap::from([
            ("Keep.md".into(), "- keep\n".into()),
            ("People/Maram.md".into(), "- hi\n".into()),
            ("Plan.md".into(), "- see ![](assets/a.png)\n".into()),
            ("assets/a.png".into(), "png".into()),
        ])
    );
    wait_until(cx, "the page list", |cx| listed(&notebook, cx).len() == 3);

    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(folder.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Choose a folder outside this notebook to import."
    );

    let out = tempfile::tempdir().unwrap();
    dispatch(&notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    let message = finished(&notebook, cx);
    assert!(
        message.starts_with("Exported 3 pages and 1 other file to "),
        "{message}"
    );
    assert_eq!(tree(out.path()), tree(folder.path()));
}

#[gpui::test]
fn a_folder_page_export_points_images_at_files_inside_the_notebook(cx: &mut TestAppContext) {
    let (notebook, folder) = open_folder(
        &[
            (
                "Plan.md",
                "- ![Chart](img/chart.png) ![[gone.png]] ![x](../outside.png)\n",
            ),
            ("img/chart.png", "png"),
        ],
        "Plan.md",
        cx,
    );
    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.md");
    dispatch(&notebook, ExportPageMarkdown, cx);
    wait_until(cx, "the save dialog", |cx| cx.did_prompt_for_new_path());
    let chosen = target.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    assert_eq!(finished(&notebook, cx), "Exported Plan as Markdown.");
    let chart = fs::canonicalize(folder.path().join("img/chart.png")).unwrap();
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        format!(
            "- ![Chart](<{}>) ![gone.png](gone.png) ![x](../outside.png)\n",
            chart.display()
        )
    );
}

#[gpui::test]
fn a_quire_notebook_export_copies_attachments_and_links_them_from_each_page(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let source = tempfile::tempdir().unwrap();
    write_files(
        source.path(),
        &[
            (
                "Deep/B.md",
                "- ![](../assets/a.png) ![x](attachments/gone.png)\n",
            ),
            ("Top.md", "- ![[assets/r.pdf#page=2]]\n"),
            ("assets/a.png", "png"),
            ("assets/r.pdf", "%PDF"),
        ],
    );
    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Imported 2 pages and 2 other files."
    );
    let (image, pdf) = (
        attachment_file_name(b"png", "a.png"),
        attachment_file_name(b"%PDF", "r.pdf"),
    );
    wait_until(cx, "the page list", |cx| listed(&notebook, cx).len() == 3);

    let out = tempfile::tempdir().unwrap();
    dispatch(&notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        format!(
            "Exported 3 pages and 2 other files to {}. Not exported: page history and earlier versions; 1 attachment the pages link to (Deep/attachments/gone.png); sync logs, notebook settings, and this computer's records.",
            out.path().display()
        )
    );
    assert_eq!(
        tree(out.path()),
        BTreeMap::from([
            (
                "Deep/B.md".into(),
                format!("- ![](../attachments/{image}) ![x](attachments/gone.png)\n")
            ),
            ("Keep.md".into(), "- keep\n".into()),
            (
                "Top.md".into(),
                format!("- ![[attachments/{pdf}#page=2]]\n")
            ),
            (format!("attachments/{image}"), "png".into()),
            (format!("attachments/{pdf}"), "%PDF".into()),
        ])
    );
}

#[gpui::test]
fn a_quire_page_export_points_attachment_links_at_the_stored_files(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let source = tempfile::tempdir().unwrap();
    write_files(
        source.path(),
        &[("Plan.md", "- ![Chart](img/c.png)\n"), ("img/c.png", "png")],
    );
    dispatch(&notebook, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(
        finished(&notebook, cx),
        "Imported 1 pages and 1 other files."
    );
    wait_until(cx, "the page list", |cx| listed(&notebook, cx).len() == 2);
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.open_page(page_id_for_path("Plan.md"), false, window, cx)
        })
        .unwrap();
    settle(&notebook, cx);

    let out = tempfile::tempdir().unwrap();
    let target = out.path().join("Plan.md");
    dispatch(&notebook, ExportPageMarkdown, cx);
    wait_until(cx, "the save dialog", |cx| cx.did_prompt_for_new_path());
    let chosen = target.clone();
    cx.simulate_new_path_selection(move |_| Some(chosen));
    assert_eq!(finished(&notebook, cx), "Exported Plan as Markdown.");
    let stored = fs::canonicalize(
        notebook
            .path
            .parent()
            .unwrap()
            .join("attachments")
            .join(attachment_file_name(b"png", "c.png")),
    )
    .unwrap();
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        format!("- ![Chart](<{}>)\n", stored.display())
    );
}

#[gpui::test]
fn a_notebook_export_includes_typing_that_has_not_been_saved(cx: &mut TestAppContext) {
    let id = page_id_for_path("A.md");
    let notebook = open_notebook(&[(&id, "A.md", "- a\n")], &id, cx);
    settle(&notebook, cx);
    let editor = super::media_tests::page_editor(&notebook, cx);
    notebook
        .window
        .update(cx, |_, window, cx| {
            editor.update(cx, |editor, cx| {
                editor.change_selections(Default::default(), window, cx, |selections| {
                    selections.select_ranges([
                        multi_buffer::MultiBufferOffset(3)..multi_buffer::MultiBufferOffset(3)
                    ]);
                });
                editor.insert(" typed", window, cx);
            })
        })
        .unwrap();
    assert_eq!(
        stored_text(&notebook.path, "A.md").as_deref(),
        Some("- a\n"),
        "the typing is not saved yet"
    );

    let out = tempfile::tempdir().unwrap();
    dispatch(&notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    finished(&notebook, cx);
    assert_eq!(
        tree(out.path()),
        BTreeMap::from([("A.md".into(), "- a typed\n".into())])
    );
}

/// Every file below `root` as its SHA-256, by relative path.
fn digest(root: &Path) -> BTreeMap<String, String> {
    tree(root)
        .into_iter()
        .map(|(path, text)| (path, quire_store::sync::format::sha256(text)))
        .collect()
}

/// How many pages and other files `digest` holds.
fn counts(digest: &BTreeMap<String, String>) -> (usize, usize) {
    let pages = digest.keys().filter(|path| path.ends_with(".md")).count();
    (pages, digest.len() - pages)
}

/// Exports the notebook in `notebook` into a new folder.
fn export_all(notebook: &Notebook, cx: &mut TestAppContext) -> tempfile::TempDir {
    let out = tempfile::tempdir().unwrap();
    dispatch(notebook, ExportNotebookMarkdown, cx);
    choose_folder(out.path(), cx);
    let message = finished(notebook, cx);
    assert!(message.starts_with("Exported "), "{message}");
    out
}

const ROUND_TRIP: [(&str, &str); 5] = [
    (
        "Deep/B.md",
        "- ![](../assets/a.png) links [[Top]]\n- second\n",
    ),
    ("Top.md", "- ![[assets/r.pdf#page=2]] #plan\n"),
    ("Ünïcode/Ça.md", "- العربية and اردو\r\n- crlf kept\r\n"),
    ("assets/a.png", "png bytes"),
    ("assets/r.pdf", "%PDF-1.4"),
];

#[gpui::test]
fn a_quire_notebook_round_trips_through_export_and_import_with_equal_counts_and_hashes(
    cx: &mut TestAppContext,
) {
    let first = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    let source = tempfile::tempdir().unwrap();
    write_files(source.path(), &ROUND_TRIP);
    dispatch(&first, ImportMarkdownFolder, cx);
    choose_folder(source.path(), cx);
    assert_eq!(finished(&first, cx), "Imported 3 pages and 2 other files.");
    wait_until(cx, "the page list", |cx| listed(&first, cx).len() == 4);
    let exported = export_all(&first, cx);
    let expected = digest(exported.path());
    assert_eq!(counts(&expected), (4, 2));

    let second = super::tests::open_initialized(&[("k", "Keep.md", "- keep\n")], "k", cx);
    dispatch(&second, ImportMarkdownFolder, cx);
    choose_folder(exported.path(), cx);
    assert_eq!(
        finished(&second, cx),
        "Imported 3 pages and 2 other files. 1 pages were already here and were left unchanged."
    );
    wait_until(cx, "the page list", |cx| listed(&second, cx).len() == 4);
    assert_eq!(listed(&second, cx), listed(&first, cx));
    let again = export_all(&second, cx);
    assert_eq!(digest(again.path()), expected);
    for path in ["Deep/B.md", "Top.md", "Ünïcode/Ça.md"] {
        assert_eq!(
            stored_text(&second.path, path),
            stored_text(&first.path, path),
            "{path}"
        );
    }
}

#[gpui::test]
fn a_folder_notebook_round_trips_through_export_and_import_with_equal_counts_and_hashes(
    cx: &mut TestAppContext,
) {
    let mut files = vec![("Keep.md", "- keep\n")];
    files.extend(ROUND_TRIP);
    let (first, first_folder) = open_folder(&files, "Keep.md", cx);
    let exported = export_all(&first, cx);
    let expected = digest(first_folder.path());
    assert_eq!(counts(&expected), (4, 2));
    assert_eq!(digest(exported.path()), expected);

    let (second, second_folder) =
        open_folder_initialized(&[("Keep.md", "- keep\n")], "Keep.md", cx);
    dispatch(&second, ImportMarkdownFolder, cx);
    choose_folder(exported.path(), cx);
    assert_eq!(
        finished(&second, cx),
        "Imported 3 pages and 2 other files. 1 pages were already here and were left unchanged."
    );
    assert_eq!(digest(second_folder.path()), expected);
    let again = export_all(&second, cx);
    assert_eq!(digest(again.path()), expected);
}
