//! Deleting the shown page or a Files item: the confirmation dialog, then closing everything the
//! deleted pages leave behind.

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
