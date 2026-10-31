//! Moving a block to another page from the stored window: the page editor names the block, the
//! find palette picks the destination, and `block_move` writes every page or none. The window
//! then shows the destination page.

use super::*;
use crate::block_move::{MISSING_DESTINATION, MoveBlockToPage};
use crate::navigation::palette::{PagePalette, PaletteChoice, PaletteMode};
use gpui::DismissEvent;
use picker::Picker;
use quire_core::workspace::Destination;

const NO_INDEX: &str =
    "Moving a block needs the notebook index, which did not start. Reopen the notebook to retry.";
const NOT_OPEN: &str = "The block's page is no longer open. Open it, then move the block again.";

impl StoredWindow {
    pub(super) fn register_block_move(div: gpui::Div, cx: &mut gpui::Context<Self>) -> gpui::Div {
        div.on_action(cx.listener(Self::choose_block_destination))
    }

    fn choose_block_destination(
        &mut self,
        _: &MoveBlockToPage,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let Some(request) = crate::block_move::take_request(cx) else {
            return;
        };
        let source = request.buffer.upgrade().and_then(|buffer| {
            let pages = self.session.borrow().open_pages();
            pages.into_iter().find_map(|page| {
                let page = page.read(cx);
                (page.loaded()?.analysis.read(cx).source == buffer).then(|| page.id().to_owned())
            })
        });
        let Some(source) = source else {
            self.nav.error = Some(NOT_OPEN.into());
            cx.notify();
            return;
        };
        let Some(client) = self.index_client(cx) else {
            self.nav.error = Some(NO_INDEX.into());
            cx.notify();
            return;
        };
        let this = cx.entity().downgrade();
        let id = request.id;
        let choose = Rc::new(move |choice, _, window: &mut Window, cx: &mut App| {
            let (this, source, id) = (this.clone(), source.clone(), id.clone());
            window.defer(cx, move |window, cx| {
                this.update(cx, |this, cx| {
                    this.move_block(source, id, choice, window, cx)
                })
                .ok();
            });
        });
        let palette = PagePalette::new(client, PaletteMode::Find, choose);
        let picker = cx.new(|cx| Picker::uniform_list(palette, window, cx).reopenable(false, cx));
        cx.subscribe_in(&picker, window, |this, _, _: &DismissEvent, window, cx| {
            this.nav.palette = None;
            this.focus_active(window, cx);
            cx.notify();
        })
        .detach();
        picker.update(cx, |picker, cx| picker.focus(window, cx));
        self.nav.palette = Some(picker);
        cx.notify();
    }

    fn move_block(
        &mut self,
        source: String,
        id: String,
        choice: PaletteChoice,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let PaletteChoice::Open {
            page: destination, ..
        } = choice
        else {
            self.nav.error = Some(MISSING_DESTINATION.into());
            cx.notify();
            return;
        };
        let Some(index) = self.index_client(cx) else {
            self.nav.error = Some(NO_INDEX.into());
            cx.notify();
            return;
        };
        let (open_pages, storage) = {
            let session = self.session.borrow();
            (session.open_pages(), session.storage())
        };
        let planned = crate::block_move::plan(
            source,
            id,
            destination,
            open_pages,
            index,
            storage.clone(),
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let moved = match planned.await {
                Ok(plan) => {
                    let destination = plan.destination.clone();
                    let written = cx.update(|_, cx| crate::block_move::write(plan, storage, cx))?;
                    written.await.map(|_| destination)
                }
                Err(error) => Err(error),
            };
            this.update_in(cx, |this, window, cx| {
                match moved {
                    Ok(destination) => {
                        this.nav.error = None;
                        this.visit(Destination::page(destination), window, cx);
                    }
                    Err(error) => this.nav.error = Some(error.into()),
                }
                cx.notify();
            })
        })
        .detach();
    }
}
