//! Roam import and export through the window's commands, porting rt's `roam-files.test.ts` and
//! `roam-worker.test.ts` cases with exact files, reports, and refusals.

use super::super::*;
use super::tests::{Notebook, open_notebook, read, settle, wait_until};
use crate::exchange::{CancelExchange, Exchange, ExportRoamJson, ImportRoamData};
use gpui::{TestAppContext, WindowHandle};
use quire_store::{
    notebook::{Draft, DraftStamp, NotebookStore},
    roam_files::IMPORT_CANCELED,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    rc::Rc,
};

/// rt's `roam-files.test.ts` graph, as `JSON.stringify` writes it.
const ORIGINAL: &str = r#"[{"title":"Graph/Page","uid":"page","create-time":1,"children":[{"uid":"parent","string":"Parent","children":[{"uid":"child","string":"Child","edit-time":2}]},{"uid":"embed","string":"{{[[embed]]: ((parent))}}"}]}]"#;

/// The same graph as `JSON.stringify(pages, null, 2)` writes it, with the final newline.
const ORIGINAL_PRETTY: &str = r#"[
  {
    "title": "Graph/Page",
    "uid": "page",
    "create-time": 1,
    "children": [
      {
        "uid": "parent",
        "string": "Parent",
        "children": [
          {
            "uid": "child",
            "string": "Child",
            "edit-time": 2
          }
        ]
      },
      {
        "uid": "embed",
        "string": "{{[[embed]]: ((parent))}}"
      }
    ]
  }
]
"#;

const PAGE: &str = "---\ntitle: \"Graph/Page\"\n---\n\n- Parent ^parent\n  - Child ^child\n- {{[[embed]]: ((parent))}} ^embed\n";

/// Folder notebooks the import opened, each with the notices it was opened with. They open with
/// a state folder kept here, so tests never touch Quire's application data.
#[derive(Default)]
struct Opened {
    roots: Vec<(PathBuf, Vec<String>)>,
    states: Vec<tempfile::TempDir>,
}

fn record_opened(cx: &mut TestAppContext) -> Rc<RefCell<Opened>> {
    let opened = Rc::new(RefCell::new(Opened::default()));
    let recorded = opened.clone();
    cx.update(|cx| {
        cx.set_global(super::folder::OpenFolder(Rc::new(
            move |root: PathBuf, notices: Vec<String>, cx: &mut App| {
                let state = tempfile::tempdir().unwrap();
                let path = state.path().to_owned();
                let mut opened = recorded.borrow_mut();
                opened.roots.push((root.clone(), notices.clone()));
                opened.states.push(state);
                super::folder::FolderOptions::new(root, notices, Some(path)).open(cx)
            },
        )))
    });
    opened
}

fn dispatch(notebook: &Notebook, action: impl gpui::Action, cx: &mut TestAppContext) {
    cx.dispatch_action(notebook.window.into(), action);
}

fn exchange(window: WindowHandle<StoredWindow>, cx: &mut TestAppContext) -> Entity<Exchange> {
    window
        .update(cx, |view, _, _| view.exchange.clone())
        .unwrap()
}

fn finished_in(window: WindowHandle<StoredWindow>, cx: &mut TestAppContext) -> String {
    let status = exchange(window, cx);
    wait_until(cx, "the import or export to finish", |cx| {
        status.read_with(cx, |status, _| {
            !status.is_running() && status.message().is_some()
        })
    });
    status.read_with(cx, |status, _| status.message().unwrap().to_string())
}

/// Answers the file dialog with `file`, then the parent folder dialog with `parent`.
fn choose_source(file: &Path, parent: &Path, cx: &mut TestAppContext) {
    wait_until(cx, "the file dialog", |cx| cx.did_prompt_for_paths());
    let file = file.to_owned();
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.files && !options.directories);
        Some(vec![file])
    });
    wait_until(cx, "the folder dialog", |cx| cx.did_prompt_for_paths());
    let parent = parent.to_owned();
    cx.simulate_path_prompt_response(move |options| {
        assert!(options.directories && !options.files);
        Some(vec![parent])
    });
}

fn choose_destination(window: WindowHandle<StoredWindow>, file: &Path, cx: &mut TestAppContext) {
    cx.dispatch_action(window.into(), ExportRoamJson);
    wait_until(cx, "the save dialog", |cx| cx.did_prompt_for_new_path());
    let file = file.to_owned();
    cx.simulate_new_path_selection(move |_| Some(file));
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

fn notices_of(view: &StoredWindow) -> Vec<String> {
    view.folder
        .notices
        .iter()
        .map(ToString::to_string)
        .collect()
}

/// Export JSON with each generated `b-` identifier written as `b-new`.
fn mask_generated(json: &str) -> String {
    json.split_inclusive('\n')
        .map(|line| match line.split_once("\"uid\": \"b-") {
            Some((head, tail))
                if tail.len() > 32 && tail[..32].bytes().all(|byte| byte.is_ascii_hexdigit()) =>
            {
                format!("{head}\"uid\": \"b-new{}", &tail[32..])
            }
            _ => line.to_owned(),
        })
        .collect()
}

/// The window the import opened.
fn opened_window(
    original: WindowHandle<StoredWindow>,
    cx: &mut TestAppContext,
) -> WindowHandle<StoredWindow> {
    cx.windows()
        .into_iter()
        .filter_map(|window| window.downcast::<StoredWindow>())
        .find(|window| *window != original)
        .expect("the import opened its notebook in a new window")
}

fn notebook_for(window: WindowHandle<StoredWindow>, cx: &mut TestAppContext) -> Notebook {
    let state = tempfile::tempdir().unwrap();
    let path = state.path().join("unused.sqlite");
    let notebook = Notebook {
        _root: state,
        path,
        window,
    };
    settle(&notebook, cx);
    notebook
}

#[gpui::test]
fn importing_roam_json_creates_a_notebook_whose_export_restores_the_graph(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let opened = record_opened(cx);
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("source.json");
    fs::write(&source, ORIGINAL).unwrap();
    let parent = tempfile::tempdir().unwrap();

    dispatch(&notebook, ImportRoamData, cx);
    choose_source(&source, parent.path(), cx);
    assert_eq!(
        finished_in(notebook.window, cx),
        "Imported 1 pages and 3 blocks. Original export unchanged. Details are available in notebook notices."
    );
    let (root, notices) = opened.borrow().roots[0].clone();
    let name = root.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("Roam import "), "{name}");
    assert_eq!(
        fs::read_dir(parent.path()).unwrap().count(),
        1,
        "only the notebook, no staging folder"
    );
    let report = [
        "Imported 1 pages and 3 blocks. Original export unchanged.",
        "Original Roam page and block metadata are retained in .quire/roam.json. Executable macros are preserved as text; remote attachments are not downloaded.",
        "Stored Graph/Page as Graph-Page.md; retained the original title in front matter.",
        "Exact original export archived in .quire/source.json.",
    ];
    assert_eq!(
        tree(&root),
        BTreeMap::from([
            ("Graph-Page.md".into(), PAGE.into()),
            (
                ".quire/roam.json".into(),
                format!(
                    r#"{{"version":1,"pages":{{"Graph-Page.md":{}}}}}"#,
                    &ORIGINAL[1..ORIGINAL.len() - 1]
                )
            ),
            (".quire/source.json".into(), ORIGINAL.into()),
            (
                ".quire/import-report.txt".into(),
                format!("{}\n", report.join("\n"))
            ),
        ])
    );
    assert_eq!(notices, report[1..]);
    assert_eq!(
        fs::read_to_string(&source).unwrap(),
        ORIGINAL,
        "the export is only read"
    );
    let window = opened_window(notebook.window, cx);
    let imported = notebook_for(window, cx);
    read(&imported, cx, |view, _| {
        assert_eq!(notices_of(view), report[1..]);
        assert_eq!(
            view.nav.history.current(),
            Some(&quire_core::workspace::Destination::Browser {
                view: quire_core::workspace::BrowserView::Pages
            }),
            "a Roam notebook opens on its pages, as rt's does"
        );
    });

    let out = tempfile::tempdir().unwrap();
    let output = out.path().join("output.json");
    choose_destination(window, &output, cx);
    assert_eq!(
        finished_in(window, cx),
        format!(
            "Saved Roam JSON to {}. Exported 1 pages to Roam JSON. Existing block IDs are preserved; 0 new export-only IDs were generated.",
            output.display()
        )
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), ORIGINAL_PRETTY);
    assert_eq!(
        fs::read_to_string(out.path().join("output.json.report.txt")).unwrap(),
        "Exported 1 pages to Roam JSON. Existing block IDs are preserved; 0 new export-only IDs were generated.\n"
    );

    choose_destination(window, &output, cx);
    assert_eq!(
        finished_in(window, cx),
        "The export file already exists. Choose a new filename to keep previous exports."
    );
    assert_eq!(fs::read_to_string(&output).unwrap(), ORIGINAL_PRETTY);
    choose_destination(window, &root.join("export.json"), cx);
    assert_eq!(
        finished_in(window, cx),
        "Choose an export filename outside live notebooks and Quire's app-data folder."
    );
    assert!(!root.join("export.json").exists());
}

#[gpui::test]
fn importing_roam_edn_archives_the_exact_database(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let opened = record_opened(cx);
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("source.edn");
    let text = r#"#datascript/DB {:schema {} :datoms [[1 :node/title "CON" 10] [2 :user/settings {:x ##NaN :y #{1 2 3}} 10]]}"#;
    fs::write(&source, text).unwrap();
    let parent = tempfile::tempdir().unwrap();

    dispatch(&notebook, ImportRoamData, cx);
    choose_source(&source, parent.path(), cx);
    assert_eq!(
        finished_in(notebook.window, cx),
        "Imported 1 pages and 0 blocks. Original export unchanged. Details are available in notebook notices."
    );
    let (root, notices) = opened.borrow().roots[0].clone();
    assert_eq!(
        fs::read_to_string(root.join("Page CON.md")).unwrap(),
        "---\ntitle: \"CON\"\n---\n\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".quire/source.edn")).unwrap(),
        text
    );
    assert_eq!(fs::read_to_string(&source).unwrap(), text);
    assert_eq!(
        notices,
        [
            "Read 2 DataScript datoms. Recovered 0 detached text blocks on a separate recovery page. The original database archive retains all non-outline entities and settings.",
            "Original Roam page and block metadata are retained in .quire/roam.json. Executable macros are preserved as text; remote attachments are not downloaded.",
            "Stored CON as Page CON.md; retained the original title in front matter.",
            "Exact original export archived in .quire/source.edn.",
        ]
    );
}

#[gpui::test]
fn malformed_exports_and_canceled_imports_leave_the_parent_unchanged(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let opened = record_opened(cx);
    let input = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    for (name, text, error) in [
        (
            "bad.json",
            "not json",
            "The Roam export is not valid UTF-8 JSON. Extract its JSON or EDN file from the Roam export ZIP, then select that file.",
        ),
        (
            "bad.edn",
            "not an export",
            "Invalid or unsupported EDN near character 4. Export the graph again as JSON or EDN and retry.",
        ),
        (
            "duplicate.json",
            r#"[{"title":"Page","children":[{"uid":"same","string":"A"},{"uid":"same","string":"B"}]}]"#,
            "Roam export has duplicate ID same. Fix the export before importing.",
        ),
    ] {
        let source = input.path().join(name);
        fs::write(&source, text).unwrap();
        dispatch(&notebook, ImportRoamData, cx);
        choose_source(&source, parent.path(), cx);
        assert_eq!(finished_in(notebook.window, cx), error);
        assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    }

    // Enough referencing blocks to parse in several batches, so Cancel lands mid-import.
    let children: Vec<String> = (0..600)
        .map(|index| {
            format!(
                r#"{{"uid":"b{index}","string":"see ((b{}))"}}"#,
                (index + 1) % 600
            )
        })
        .collect();
    let source = input.path().join("large.json");
    fs::write(
        &source,
        format!(
            r#"[{{"title":"Large","children":[{}]}}]"#,
            children.join(",")
        ),
    )
    .unwrap();
    dispatch(&notebook, ImportRoamData, cx);
    choose_source(&source, parent.path(), cx);
    let status = exchange(notebook.window, cx);
    while !status.read_with(cx, |status, _| {
        status
            .message()
            .is_some_and(|message| message.contains("read references in"))
    }) {
        assert!(cx.executor().tick(), "the import never read references");
    }
    dispatch(&notebook, CancelExchange, cx);
    assert_eq!(finished_in(notebook.window, cx), IMPORT_CANCELED);
    assert_eq!(fs::read_dir(parent.path()).unwrap().count(), 0);
    assert!(opened.borrow().roots.is_empty());
}

#[gpui::test]
fn a_quire_notebook_exports_its_outline_as_roam_json(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            (
                "a",
                "Folder/A.md",
                "- Parent ^parent\n  - Child ^child\n    - [ ] Task ^task\n- ## Heading ^heading\n",
            ),
            (
                "b",
                "B.md",
                "![[Folder/A#^parent]]\n\n[[Folder/A#^child]]\n\n![image](assets/photo.png)\n\n![[Folder/A]]\n",
            ),
        ],
        "a",
        cx,
    );
    settle(&notebook, cx);
    let out = tempfile::tempdir().unwrap();
    let output = out.path().join("graph.json");
    choose_destination(notebook.window, &output, cx);
    assert_eq!(
        finished_in(notebook.window, cx),
        format!(
            "Saved Roam JSON to {}. Exported 2 pages to Roam JSON. Existing block IDs are preserved; 4 new export-only IDs were generated.",
            output.display()
        )
    );
    let written = mask_generated(&fs::read_to_string(&output).unwrap());
    assert_eq!(
        written,
        r#"[
  {
    "title": "B",
    "children": [
      {
        "uid": "b-new",
        "string": "{{[[embed]]: ((parent))}}"
      },
      {
        "uid": "b-new",
        "string": "((child))"
      },
      {
        "uid": "b-new",
        "string": "![image](assets/photo.png)"
      },
      {
        "uid": "b-new",
        "string": "{{[[embed]]: [[A]]}}"
      }
    ]
  },
  {
    "title": "A",
    "children": [
      {
        "uid": "parent",
        "string": "Parent",
        "children": [
          {
            "uid": "child",
            "string": "Child",
            "children": [
              {
                "uid": "task",
                "string": "{{[[TODO]]}} Task"
              }
            ]
          }
        ]
      },
      {
        "uid": "heading",
        "string": "Heading",
        "heading": 2
      }
    ]
  }
]
"#
    );
    let notices = read(&notebook, cx, |view, _| notices_of(view));
    assert_eq!(
        notices,
        [
            "Local attachment or relative link in B.md remains a dependency. Copy its files separately; JSON does not contain attachment bytes."
        ]
    );
}

#[gpui::test]
fn exports_refuse_unsaved_recovery_and_unreadable_metadata(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[("a", "A.md", "- Saved ^one\n"), ("b", "B.md", "- b\n")],
        "a",
        cx,
    );
    settle(&notebook, cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let draft = Draft {
        page: "b".into(),
        path: "B.md".into(),
        text: "- Unsaved\n".into(),
        base: Vec::new(),
        stamp: DraftStamp {
            session: "earlier".into(),
            version: 1,
        },
        created: 1,
    };
    store.put_draft(&draft, None).unwrap();
    let out = tempfile::tempdir().unwrap();
    let output = out.path().join("graph.json");
    choose_destination(notebook.window, &output, cx);
    assert_eq!(
        finished_in(notebook.window, cx),
        "Unsaved recovery data exists. Resolve recovered pages before exporting the graph."
    );
    assert!(!output.exists());

    let opened = record_opened(cx);
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("source.json");
    fs::write(
        &source,
        r#"[{"title":"Page","children":[{"uid":"one","string":"Saved"}]}]"#,
    )
    .unwrap();
    let parent = tempfile::tempdir().unwrap();
    dispatch(&notebook, ImportRoamData, cx);
    choose_source(&source, parent.path(), cx);
    finished_in(notebook.window, cx);
    let (root, _) = opened.borrow().roots[0].clone();
    let imported = notebook_for(opened_window(notebook.window, cx), cx);
    fs::write(root.join(".quire/roam.json"), "invalid").unwrap();
    choose_destination(imported.window, &output, cx);
    assert_eq!(
        finished_in(imported.window, cx),
        "The Roam metadata file could not be read. Restore .quire/roam.json from a backup before exporting."
    );
    assert!(!output.exists());
}

/// rt's `import.test.ts` "remaps unsupported IDs consistently without modifying code examples",
/// through the editor's own Markdown parses in both directions.
#[gpui::test]
fn remapped_ids_follow_references_but_not_code_through_the_native_parser(cx: &mut TestAppContext) {
    let notebook = open_notebook(&[("k", "Keep.md", "- keep\n")], "k", cx);
    settle(&notebook, cx);
    let opened = record_opened(cx);
    let input = tempfile::tempdir().unwrap();
    let source = input.path().join("source.json");
    fs::write(
        &source,
        r#"[{"title":"Page","children":[{"uid":"space id","string":"Original"},{"uid":"ref","string":"((space id)) and `((space id))`\r\nnext ((ref))"}]}]"#,
    )
    .unwrap();
    let parent = tempfile::tempdir().unwrap();
    dispatch(&notebook, ImportRoamData, cx);
    choose_source(&source, parent.path(), cx);
    finished_in(notebook.window, cx);
    let (root, notices) = opened.borrow().roots[0].clone();
    let page = fs::read_to_string(root.join("Page.md")).unwrap();
    let uid = page
        .lines()
        .find_map(|line| line.strip_prefix("- Original ^"))
        .unwrap()
        .to_owned();
    assert!(uid.starts_with("b-") && uid.len() == 34, "{uid}");
    assert_eq!(
        page,
        format!(
            "---\ntitle: \"Page\"\n---\n\n- Original ^{uid}\n- (({uid})) and `((space id))`\n  next ((ref))\n  ^ref\n"
        )
    );
    assert_eq!(
        notices[1..2],
        [format!(
            "Remapped block ID space id to {uid} for Markdown compatibility."
        )]
    );

    let imported = notebook_for(opened_window(notebook.window, cx), cx);
    let out = tempfile::tempdir().unwrap();
    let output = out.path().join("graph.json");
    choose_destination(imported.window, &output, cx);
    finished_in(imported.window, cx);
    assert_eq!(
        fs::read_to_string(&output).unwrap(),
        format!(
            "[\n  {{\n    \"title\": \"Page\",\n    \"children\": [\n      {{\n        \"uid\": \"{uid}\",\n        \"string\": \"Original\"\n      }},\n      {{\n        \"uid\": \"ref\",\n        \"string\": \"(({uid})) and `((space id))`\\r\\nnext ((ref))\"\n      }}\n    ]\n  }}\n]\n"
        )
    );
}
