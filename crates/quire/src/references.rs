//! Wiki (`[[Page]]`) and Roam (`((id))`) references in page editors, resolved through the
//! notebook that hosts the editor. Following a reference reuses Zed's navigation actions: Go to
//! Definition (F12, Vim `g d`) opens what it names, Go to Definition Split (Alt-F12, Vim
//! `Ctrl-W d`) opens it beside, and Find All References (Alt-Shift-F12, Vim `g A`) opens the
//! named page's linked references. Write views also follow a rendered link on click.

use crate::notebook_session::OpenPage;
use editor::Editor;
use gpui::{App, Context, Entity, KeyBinding, Subscription, Window};
use language::Buffer;
use multi_buffer::ToOffset;
use quire_core::markdown::{Reference, ReferenceKind, ReferenceSyntax};
use quire_store::index::IndexClient;
use std::rc::Rc;

mod completion;

/// What a reference names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A page by name, path, or daily date.
    Page(String),
    /// A block by ID, optionally scoped to a page with `Page#^id`.
    Block(String),
}

impl Target {
    pub fn of(reference: &Reference) -> Option<Self> {
        if reference.media.is_some() {
            return None;
        }
        Some(match reference.kind {
            ReferenceKind::Page => Self::Page(reference.target.clone()),
            ReferenceKind::Embed
                if reference.syntax == ReferenceSyntax::Wiki
                    && !reference.target.contains("#^") =>
            {
                Self::Page(reference.target.clone())
            }
            ReferenceKind::Block | ReferenceKind::Embed => Self::Block(reference.target.clone()),
        })
    }
}

/// A request to open what a reference names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Follow {
    pub target: Target,
    /// Open in the side pane.
    pub beside: bool,
    /// Show the linked references of the named page instead of the page itself.
    pub backlinks: bool,
}

/// The notebook a page editor's references resolve in.
pub trait Notebook {
    /// Opens what a reference names.
    fn follow(&self, follow: Follow, window: &mut Window, cx: &mut App);
    /// The open page with this ID, loading it if needed, so every view of a page shares one
    /// buffer and one save stream.
    fn open_page(&self, page: &str, window: &mut Window, cx: &mut App) -> Option<Entity<OpenPage>>;
    /// The ID of the page whose buffer this is, when the notebook has it open.
    fn page_id(&self, buffer: &Entity<Buffer>, cx: &App) -> Option<String>;
}

/// Shares one notebook among a window's page editors and tells them when indexed answers may
/// have changed.
pub struct ReferenceHost {
    notebook: Rc<dyn Notebook>,
    index: Option<IndexClient>,
}

impl ReferenceHost {
    pub fn new(notebook: Rc<dyn Notebook>) -> Self {
        Self {
            notebook,
            index: None,
        }
    }

    pub fn notebook(&self) -> &Rc<dyn Notebook> {
        &self.notebook
    }

    /// The query index, once it runs.
    pub fn index(&self) -> Option<&IndexClient> {
        self.index.as_ref()
    }

    /// The index started, restarted, or answers differently than before.
    pub fn changed(&mut self, index: Option<IndexClient>, cx: &mut Context<Self>) {
        self.index = index;
        cx.notify();
    }
}

/// Marks an editor whose references resolve through a host.
pub(crate) struct Linked {
    pub host: Entity<ReferenceHost>,
    _subscription: Subscription,
}

impl editor::Addon for Linked {
    fn to_any(&self) -> &dyn std::any::Any {
        self
    }
    fn to_any_mut(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// The host an editor resolves references through.
pub(crate) fn host(editor: &Editor) -> Option<Entity<ReferenceHost>> {
    editor.addon::<Linked>().map(|linked| linked.host.clone())
}

/// Lets an editor follow and complete references in the host's notebook.
pub fn install(
    editor: &Entity<Editor>,
    host: &Entity<ReferenceHost>,
    window: &mut Window,
    cx: &mut App,
) {
    editor.update(cx, |editor, cx| {
        if editor.addon::<Linked>().is_some() {
            return;
        }
        let subscription = cx.observe_in(host, window, |editor, _, window, cx| {
            crate::editor_projection::references_changed(editor, window, cx);
            cx.notify();
        });
        editor.register_addon(Linked {
            host: host.clone(),
            _subscription: subscription,
        });
        editor.set_completion_provider(Some(Rc::new(completion::References::new(host.clone()))));
        editor.disable_word_completions();
    });
}

pub fn init(cx: &mut App) {
    cx.observe_new(|editor: &mut Editor, _, cx| {
        let handle = cx.entity().downgrade();
        let follow = move |beside: bool, backlinks: bool| {
            let handle = handle.clone();
            move |window: &mut Window, cx: &mut App| {
                let handled = handle.upgrade().is_some_and(|editor| {
                    editor.update(cx, |editor, cx| {
                        follow_at_caret(editor, beside, backlinks, window, cx)
                    })
                });
                if !handled {
                    cx.propagate();
                }
            }
        };
        let open = follow(false, false);
        editor
            .register_action(move |_: &editor::actions::GoToDefinition, window, cx| {
                open(window, cx)
            })
            .detach();
        let beside = follow(true, false);
        editor
            .register_action(
                move |_: &editor::actions::GoToDefinitionSplit, window, cx| beside(window, cx),
            )
            .detach();
        let backlinks = follow(false, true);
        editor
            .register_action(move |_: &editor::actions::FindAllReferences, window, cx| {
                backlinks(window, cx)
            })
            .detach();
    })
    .detach();
    const CONTEXT: &str = "Editor && !QuireControl";
    cx.bind_keys([
        KeyBinding::new(
            "f12",
            editor::actions::GoToDefinition::default(),
            Some(CONTEXT),
        ),
        KeyBinding::new(
            "alt-f12",
            editor::actions::GoToDefinitionSplit,
            Some(CONTEXT),
        ),
        KeyBinding::new(
            "alt-shift-f12",
            editor::actions::FindAllReferences::default(),
            Some(CONTEXT),
        ),
    ]);
    completion::bind_keys(cx);
}

const NOT_READY: &str = "This page is still being read. Try following the link again in a moment.";

/// Follows the reference under the newest caret. Returns false when there is none, so Zed's
/// own handling of the key runs.
fn follow_at_caret(
    editor: &mut Editor,
    beside: bool,
    backlinks: bool,
    window: &mut Window,
    cx: &mut Context<Editor>,
) -> bool {
    let Some(host) = host(editor) else {
        return false;
    };
    let multi = editor.buffer().read(cx).snapshot(cx);
    let head = editor.selections.newest_anchor().head().to_offset(&multi);
    let Some((buffer, offset)) = editor.buffer().read(cx).point_to_buffer_offset(head, cx) else {
        return false;
    };
    let snapshot = buffer.read(cx).snapshot();
    if snapshot
        .language()
        .is_none_or(|language| language.name().as_ref() != "Markdown")
    {
        return false;
    }
    let Some(references) = crate::page_analysis::references(&snapshot) else {
        crate::outline::set_notice(Some(NOT_READY), cx);
        return true;
    };
    let Some(target) = reference_at(&references, offset.0).and_then(Target::of) else {
        return false;
    };
    let notebook = host.read(cx).notebook().clone();
    notebook.follow(
        Follow {
            target,
            beside,
            backlinks,
        },
        window,
        cx,
    );
    true
}

/// The reference whose text holds `offset`, including a caret right after it.
pub fn reference_at(references: &[Reference], offset: usize) -> Option<&Reference> {
    references
        .iter()
        .find(|reference| reference.range.start <= offset && offset <= reference.range.end)
}

/// Runs `then` with the page's shared buffer once the page has loaded.
pub(crate) fn when_loaded(
    page: Entity<OpenPage>,
    then: impl FnOnce(Entity<Buffer>, &mut App) + 'static,
    cx: &mut App,
) {
    if let Some(loaded) = page.read(cx).loaded() {
        let buffer = loaded.analysis.read(cx).source.clone();
        then(buffer, cx);
        return;
    }
    let then = std::cell::Cell::new(Some(then));
    cx.observe(&page, move |page, cx| {
        let Some(loaded) = page.read(cx).loaded() else {
            return;
        };
        let buffer = loaded.analysis.read(cx).source.clone();
        if let Some(then) = then.take() {
            then(buffer, cx);
        }
    })
    .detach();
}

#[cfg(test)]
mod tests;
