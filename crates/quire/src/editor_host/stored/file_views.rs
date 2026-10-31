//! Deleting and renaming the shown page or a Files item: the dialog, then closing everything the
//! deleted pages leave behind, or showing the renamed page's new name.

use super::*;
use crate::{
    file_actions::Target,
    navigation::{
        file_dialog::{FileDialog, FileDialogEvent},
        tag_edits::Writers,
    },
};
use quire_core::workspace::{BrowserView, Destination, TreeKind};

impl StoredWindow {
    fn writers(&self) -> Writers {
        let session = self.session.clone();
        Rc::new(move |_| {
            let session = session.borrow();
            (session.open_pages(), session.storage())
        })
    }

    fn show_dialog(
        &mut self,
        dialog: Entity<FileDialog>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        cx.subscribe_in(&dialog, window, |this, _, _: &DismissEvent, window, cx| {
            this.nav.dialog = None;
            this.focus_active(window, cx);
            cx.notify();
        })
        .detach();
        cx.subscribe_in(
            &dialog,
            window,
            |this, _, event: &FileDialogEvent, window, cx| match event {
                FileDialogEvent::Deleted { pages, folder } => {
                    this.pages_deleted(pages, folder.as_deref(), window, cx)
                }
                FileDialogEvent::Renamed(renamed) => {
                    this.nav.renamed = Some(renamed.clone());
                    this.follow_rename(cx);
                    this.update_title(window, cx);
                    cx.notify();
                }
            },
        )
        .detach();
        window.focus(&dialog.focus_handle(cx), cx);
        self.nav.dialog = Some(dialog);
        cx.notify();
    }

    pub(in crate::editor_host::stored) fn ask_delete(
        &mut self,
        target: Target,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Deleting needs the notebook index, which did not start. Reopen the notebook to retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        let writers = self.writers();
        let dialog = cx.new(|cx| FileDialog::delete(target, writers, index, cx));
        self.show_dialog(dialog, window, cx);
    }

    pub(in crate::editor_host::stored) fn ask_rename(
        &mut self,
        page: String,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(
                "Renaming needs the notebook index, which did not start. Reopen the notebook to retry."
                    .into(),
            );
            cx.notify();
            return;
        };
        let open = self.session.borrow().open_pages();
        let path = open
            .iter()
            .find(|open| open.read(cx).id() == page)
            .and_then(|open| open.read(cx).path(cx))
            .or_else(|| {
                self.nav
                    .pages
                    .iter()
                    .find(|listed| listed.page.page.as_str() == Some(page.as_str()))
                    .map(|listed| listed.page.path.clone())
            });
        let Some(path) = path else {
            return;
        };
        let writers = self.writers();
        let dialog = cx.new(|cx| FileDialog::rename(page, path, writers, index, window, cx));
        self.show_dialog(dialog, window, cx);
    }

    pub(in crate::editor_host::stored) fn rename_shown_page(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(page) = self.shown_page(cx) {
            self.ask_rename(page, window, cx);
        }
    }

    /// F2 in Files renames the highlighted page; folders are not renamed.
    pub(in crate::editor_host::stored) fn rename_selected_file(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(Target::Page(page)) = self.selected_file() {
            self.ask_rename(page, window, cx);
        }
    }

    /// Refreshes the page list until it shows a renamed page at its new path with its new title.
    /// An open page reaches the index through its overlays: the new path can arrive before the
    /// merged text that carries the new title, and both can land after the rename is written.
    pub(in crate::editor_host::stored) fn follow_rename(&mut self, cx: &mut gpui::Context<Self>) {
        let Some(renamed) = &self.nav.renamed else {
            return;
        };
        let listed = self.nav.pages.iter().any(|listed| {
            listed.page.page.as_str() == Some(renamed.page.as_str())
                && listed.page.path == renamed.path
                && listed.page.title == renamed.title
        });
        if listed {
            self.nav.renamed = None;
        } else {
            self.refresh_catalogue(cx);
        }
    }

    /// The page in the main pane, when a page rather than a browser or tag page is shown.
    pub(in crate::editor_host::stored) fn shown_page(&self, cx: &App) -> Option<String> {
        (self.nav.view.is_none() && self.nav.tags.shown.is_none())
            .then(|| self.page.read(cx).id().to_owned())
    }

    pub(in crate::editor_host::stored) fn delete_shown_page(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(page) = self.shown_page(cx) {
            self.ask_delete(Target::Page(page), window, cx);
        }
    }

    /// The highlighted Files row as something to act on.
    pub(in crate::editor_host::stored) fn selected_file(&self) -> Option<Target> {
        let row = self
            .file_rows()
            .into_iter()
            .nth(self.nav.browsers.files_selected)?;
        match row.kind {
            TreeKind::Folder => Some(Target::Folder(row.path)),
            TreeKind::File => self.page_at_path(&row.path).map(Target::Page),
        }
    }

    pub(in crate::editor_host::stored) fn delete_selected_file(
        &mut self,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if let Some(target) = self.selected_file() {
            self.ask_delete(target, window, cx);
        }
    }

    /// Closes tabs and forgets favorites, recents, history, open editors, and expanded folders
    /// for deleted pages, as the reference `pagesDeleted` does.
    pub(in crate::editor_host::stored) fn pages_deleted(
        &mut self,
        pages: &[String],
        folder: Option<&str>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let gone: HashSet<&str> = pages.iter().map(String::as_str).collect();
        let deleted = |item: &Destination| item.page_id().is_some_and(|id| gone.contains(id));
        let shown = self
            .shown_page(cx)
            .is_some_and(|page| gone.contains(page.as_str()));
        let side = self
            .nav
            .side
            .as_ref()
            .is_some_and(|side| gone.contains(side.page.read(cx).id()));
        if side {
            self.nav.side = None;
        }
        self.nav.history.retain(|item| !deleted(item));
        match self.nav.workspace.close(deleted, None) {
            Some(next) => {
                self.nav.history.push(next.clone());
                self.show(&next, window, cx);
            }
            None if shown => self.visit(
                Destination::Browser {
                    view: BrowserView::Pages,
                },
                window,
                cx,
            ),
            None => {}
        }
        self.nav.workspace.retain_pages(|id| !gone.contains(id));
        if let Some(folder) = folder {
            self.nav.workspace.forget_folder(folder);
        }
        let index = self.index.as_ref().ok().cloned();
        for page in pages {
            let Some(open) = self.session.borrow_mut().forget(page) else {
                continue;
            };
            if let Some(index) = &index {
                index.update(cx, |index, cx| index.forget(&open, cx));
            }
            self.nav.editors.remove(&open.entity_id());
            self.nav.watched.remove(&open.entity_id());
            self.nav.references.remove(page);
        }
        self.nav
            .pages
            .retain(|listed| !gone.contains(listed.page.page.to_lossy_string().as_str()));
        let rows = self.file_rows().len();
        self.nav.browsers.files_selected =
            self.nav.browsers.files_selected.min(rows.saturating_sub(1));
        self.persist(cx);
        self.refresh_catalogue(cx);
        cx.notify();
    }
}
