//! Each page's collapsed blocks, kept on this computer in `quire-folds.json` beside the notebook
//! database, by notebook and then page ID. Folding is how one person views a page, so it is
//! neither synced nor written into the Markdown.

use crate::markdown_blocks::Block;
use editor::Editor;
use gpui::{AnyWindowHandle, App, AppContext as _, EntityId, WeakEntity};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    rc::Rc,
    sync::{Arc, Mutex},
};

/// A collapsed block: its ID when it has one, else its position among its siblings from the
/// page's top and its first line, so a block that moved or changed is not folded by mistake.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mark {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default)]
    pub path: Vec<usize>,
    #[serde(default)]
    pub text: String,
}

pub struct FoldStore {
    path: PathBuf,
    notebook: String,
    pages: RefCell<BTreeMap<String, Vec<Mark>>>,
    generation: Cell<u64>,
    /// The generation last written, so an older write that waited never replaces a newer one.
    written: Arc<Mutex<u64>>,
    /// The views showing each page, which take another view's fold changes as they happen.
    views: RefCell<Vec<View>>,
}

struct View {
    page: String,
    editor: WeakEntity<Editor>,
    window: AnyWindowHandle,
}

impl FoldStore {
    pub fn beside(database: &Path, notebook: &str) -> Rc<Self> {
        let path = database.with_file_name("quire-folds.json");
        let pages = read(&path)
            .remove(notebook)
            .and_then(|pages| serde_json::from_value(pages).ok())
            .unwrap_or_default();
        Rc::new(Self {
            path,
            notebook: notebook.to_owned(),
            pages: RefCell::new(pages),
            generation: Cell::new(0),
            written: Arc::default(),
            views: RefCell::default(),
        })
    }

    pub fn marks(&self, page: &str) -> Vec<Mark> {
        self.pages.borrow().get(page).cloned().unwrap_or_default()
    }

    /// Keeps `editor`, a view of `page`, in step with the folds other views save for the page.
    pub fn watch(&self, page: &str, editor: WeakEntity<Editor>, window: AnyWindowHandle) {
        let mut views = self.views.borrow_mut();
        views.retain(|view| view.editor.upgrade().is_some() && view.editor != editor);
        views.push(View {
            page: page.to_owned(),
            editor,
            window,
        });
    }

    /// Records the page's collapsed blocks, which the view `from` changed, and writes the file in
    /// the background. The page's other views take the change once the current update ends. A
    /// failed write shows a notice; the folds stay in this window.
    pub fn save(&self, page: &str, marks: Vec<Mark>, from: EntityId, cx: &mut App) {
        {
            let mut pages = self.pages.borrow_mut();
            if pages
                .get(page)
                .map_or(marks.is_empty(), |saved| *saved == marks)
            {
                return;
            }
            if marks.is_empty() {
                pages.remove(page);
            } else {
                pages.insert(page.to_owned(), marks);
            }
        }
        let others: Vec<_> = self
            .views
            .borrow()
            .iter()
            .filter(|view| view.page == page && view.editor.entity_id() != from)
            .map(|view| (view.editor.clone(), view.window))
            .collect();
        if !others.is_empty() {
            cx.defer(move |cx| {
                for (editor, window) in others {
                    // A view whose window or editor has closed has nothing left to update.
                    window
                        .update(cx, |_, window, cx| {
                            editor
                                .update(cx, |editor, cx| super::folds::sync(editor, window, cx))
                                .ok();
                        })
                        .ok();
                }
            });
        }
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        let pages = serde_json::to_value(&*self.pages.borrow()).unwrap_or_default();
        let (path, notebook, written) = (
            self.path.clone(),
            self.notebook.clone(),
            self.written.clone(),
        );
        let write = cx.background_spawn(async move {
            let mut last = written
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *last > generation {
                return Ok(());
            }
            let mut entries = read(&path);
            entries.insert(notebook, pages);
            write(&path, &Value::Object(entries))?;
            *last = generation;
            Ok::<_, String>(())
        });
        cx.spawn(async move |cx| {
            if let Err(error) = write.await {
                cx.update(|cx| super::show_notice(error, cx));
            }
        })
        .detach();
    }
}

fn read(path: &Path) -> Map<String, Value> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .and_then(|value| match value {
            Value::Object(map) => Some(map),
            _ => None,
        })
        .unwrap_or_default()
}

fn write(path: &Path, value: &Value) -> Result<(), String> {
    let text = serde_json::to_vec(value).map_err(|error| error.to_string())?;
    let temporary = path.with_extension(format!("{}.json.tmp", uuid::Uuid::new_v4()));
    std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path))
        .inspect_err(|_| _ = std::fs::remove_file(&temporary))
        .map_err(|error| {
            format!(
                "Cannot save folded blocks to {}: {error}. Check write access to the notebook folder; your notes are unaffected.",
                path.display()
            )
        })
}

/// Each block's position: its index among the blocks sharing its parent, from the top level down.
fn paths(blocks: &[Block]) -> Vec<Vec<usize>> {
    let mut counts: HashMap<Option<&str>, usize> = HashMap::new();
    let mut by_key: HashMap<&str, Vec<usize>> = HashMap::new();
    blocks
        .iter()
        .map(|block| {
            let count = counts.entry(block.parent.as_deref()).or_default();
            let mut path = block
                .parent
                .as_deref()
                .and_then(|parent| by_key.get(parent).cloned())
                .unwrap_or_default();
            path.push(*count);
            *count += 1;
            by_key.insert(&block.key, path.clone());
            path
        })
        .collect()
}

fn first_line<'a>(text: &'a str, block: &Block) -> &'a str {
    let line = super::model::line_end(text, block.content_from).min(block.content_to);
    text[block.content_from..line].trim()
}

fn has_children(block: &Block) -> bool {
    block.kind == "list" && block.subtree_to > block.content_to
}

/// The marks of the list items whose children start at one of `collapsed`.
pub fn marks(text: &str, blocks: &[Block], collapsed: &[usize]) -> Vec<Mark> {
    blocks
        .iter()
        .zip(paths(blocks))
        .filter(|(block, _)| has_children(block) && collapsed.contains(&block.content_to))
        .map(|(block, path)| Mark {
            id: block.id.clone(),
            path,
            text: first_line(text, block).to_owned(),
        })
        .collect()
}

/// The list items the marks name: by ID, else at the same position with the same first line,
/// else the only item with that first line. Items without children are skipped.
pub fn resolve<'a>(text: &str, blocks: &'a [Block], marks: &[Mark]) -> Vec<&'a Block> {
    let paths = paths(blocks);
    let candidates: Vec<(&Block, &Vec<usize>)> = blocks
        .iter()
        .zip(&paths)
        .filter(|(block, _)| has_children(block))
        .collect();
    marks
        .iter()
        .filter_map(|mark| {
            if let Some(id) = &mark.id {
                return candidates
                    .iter()
                    .find(|(block, _)| block.id.as_ref() == Some(id))
                    .map(|(block, _)| *block);
            }
            let same_text = |block: &&Block| first_line(text, block) == mark.text;
            candidates
                .iter()
                .find(|(block, path)| **path == mark.path && same_text(block))
                .map(|(block, _)| *block)
                .or_else(|| {
                    let mut found = candidates.iter().map(|(block, _)| *block).filter(same_text);
                    found.next().filter(|_| found.next().is_none())
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn blocks(text: &str, cx: &mut TestAppContext) -> Vec<Block> {
        cx.update(|cx| crate::editor_host::init(cx).expect("editor initialization"));
        let buffer = cx.update(|cx| crate::markdown::buffer(text.to_owned(), cx));
        cx.executor().run_until_parked();
        buffer.read_with(cx, |buffer, _| {
            crate::markdown_blocks::current_blocks(&buffer.snapshot()).expect("parsed blocks")
        })
    }

    fn folded<'a>(text: &'a str, blocks: &'a [Block], marks: &[Mark]) -> Vec<&'a str> {
        resolve(text, blocks, marks)
            .into_iter()
            .map(|block| first_line(text, block))
            .collect()
    }

    #[gpui::test]
    fn marks_find_their_blocks_by_id_then_position_then_unique_text(cx: &mut TestAppContext) {
        let text = "- One ^one\n  - Kid\n- Two\n  - Kid\n- Three\n  - Kid\n";
        let parsed = blocks(text, cx);
        let starts: Vec<usize> = parsed
            .iter()
            .filter(|block| block.depth == 0)
            .map(|block| block.content_to)
            .collect();
        let saved = marks(text, &parsed, &starts);
        assert_eq!(
            saved,
            [
                Mark {
                    id: Some("one".into()),
                    path: vec![0],
                    text: "One ^one".into()
                },
                Mark {
                    id: None,
                    path: vec![1],
                    text: "Two".into()
                },
                Mark {
                    id: None,
                    path: vec![2],
                    text: "Three".into()
                },
            ]
        );

        let moved = "- New\n  - Kid\n- Two\n  - Kid\n- Three ^one\n  - Kid\n- One\n  - Kid\n";
        let parsed = blocks(moved, cx);
        assert_eq!(
            folded(moved, &parsed, &saved),
            ["Three ^one", "Two"],
            "the ID wins, Two keeps its position and text, and a changed block is not guessed"
        );
        let changed = "- Two\n  - Kid\n- Two\n  - Kid\n- Gone\n";
        let parsed = blocks(changed, cx);
        assert_eq!(
            folded(changed, &parsed, &saved),
            ["Two"],
            "a repeated first line resolves only at the saved position, and a childless block stays open"
        );
        let shifted = "- Two\n  - Kid\n- X\n";
        let parsed = blocks(shifted, cx);
        assert_eq!(
            folded(shifted, &parsed, &saved[1..2]),
            ["Two"],
            "a block that moved is found by its only matching first line"
        );
    }

    #[gpui::test]
    fn saves_keep_other_notebooks_and_survive_reopening(cx: &mut TestAppContext) {
        let folder = tempfile::tempdir().expect("temporary folder");
        let database = folder.path().join("notebook.db");
        std::fs::write(
            folder.path().join("quire-folds.json"),
            r#"{"other":{"p":[{"path":[0],"text":"Kept"}]}}"#,
        )
        .unwrap();
        let store = FoldStore::beside(&database, "mine");
        assert!(store.marks("p").is_empty());
        let mark = Mark {
            id: None,
            path: vec![1],
            text: "Parent".into(),
        };
        cx.update(|cx| store.save("p", vec![mark.clone()], EntityId::from(0), cx));
        cx.executor().run_until_parked();
        assert_eq!(FoldStore::beside(&database, "mine").marks("p"), [mark]);
        assert_eq!(FoldStore::beside(&database, "other").marks("p").len(), 1);
        cx.update(|cx| store.save("p", Vec::new(), EntityId::from(0), cx));
        cx.executor().run_until_parked();
        assert!(FoldStore::beside(&database, "mine").marks("p").is_empty());
    }
}
