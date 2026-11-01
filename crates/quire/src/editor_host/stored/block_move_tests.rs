use super::move_tests::stored;
use super::tests::{Notebook, active, draw, open_notebook, read, wait_until};
use crate::block_move::MoveBlockToPage;
use crate::navigation::palette::PaletteItem;
use gpui::TestAppContext;
use quire_store::notebook::NotebookStore;

const PAGES: [(&str, &str, &str); 4] = [
    (
        "s",
        "Source.md",
        "- Keep ^keep\n- Move ^m\n  - Child ^c\n- After\n",
    ),
    ("d", "Dest.md", "# Dest\n"),
    (
        "r",
        "Refs.md",
        "- [[Source#^m]] ((c)) [[Source#^keep]] [[Source#^c|kid]]\n",
    ),
    ("o", "Other.md", "- unrelated\n"),
];

fn choose(notebook: &Notebook, query: &str, page: &str, cx: &mut TestAppContext) {
    cx.simulate_input(notebook.window.into(), query);
    wait_until(cx, "the destination in the palette", |cx| {
        read(notebook, cx, |view, cx| {
            view.nav.palette.as_ref().is_some_and(|picker| {
                let picker = picker.read(cx);
                picker.delegate.settled()
                    && matches!(
                        picker.delegate.items().first(),
                        Some(PaletteItem::Page { page: found, .. }) if found == page
                    )
            })
        })
    });
    cx.simulate_keystrokes(notebook.window.into(), "enter");
}

#[gpui::test]
fn moving_a_block_takes_its_children_and_retargets_page_references(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j");
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    assert!(read(&notebook, cx, |view, _| view.nav.palette.is_some()));
    choose(&notebook, "Dest", "d", cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    wait_until(cx, "the move", |_| stored(&store, "d").1.contains("Move"));
    assert_eq!(
        stored(&store, "s").1,
        "- Keep ^keep\n- After\n",
        "the block and its child leave the source"
    );
    assert_eq!(stored(&store, "d").1, "# Dest\n\n- Move ^m\n  - Child ^c\n");
    assert_eq!(
        stored(&store, "r").1,
        "- [[Dest#^m]] ((c)) [[Source#^keep]] [[Dest#^c|kid]]\n"
    );
    assert_eq!(stored(&store, "o").1, "- unrelated\n");
    wait_until(cx, "the destination to open", |cx| {
        active(&notebook, cx) == "d"
    });
    read(&notebook, cx, |view, _| assert!(view.nav.error.is_none()));
}

#[gpui::test]
fn a_block_without_an_id_gets_one_and_the_same_page_is_refused(cx: &mut TestAppContext) {
    let pages = [
        ("s", "Source.md", "- Plain\n- Other\n"),
        ("d", "Dest.md", ""),
    ];
    let notebook = open_notebook(&pages, "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    draw(&notebook, cx);
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    choose(&notebook, "Source", "s", cx);
    wait_until(cx, "the refusal", |cx| {
        read(&notebook, cx, |view, _| view.nav.error.is_some())
    });
    read(&notebook, cx, |view, _| {
        assert_eq!(
            view.nav.error.as_deref(),
            Some(crate::block_move::SAME_PAGE)
        )
    });
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(stored(&store, "d").1, "", "a refused move writes nothing");

    draw(&notebook, cx);
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    choose(&notebook, "Dest", "d", cx);
    wait_until(cx, "the move", |_| stored(&store, "d").1.contains("Plain"));
    let moved = stored(&store, "d").1;
    let id = moved
        .strip_prefix("- Plain ^")
        .and_then(|rest| rest.strip_suffix('\n'))
        .expect("the moved block keeps the ID it was given");
    assert!(!id.is_empty());
    assert_eq!(stored(&store, "s").1, "- Other\n");
}

/// Plans moving `^m` from Source to Dest as the command would, without writing it yet.
fn plan_move(notebook: &Notebook, cx: &mut TestAppContext) -> crate::block_move::Plan {
    let (open_pages, storage, index) = read(notebook, cx, |view, cx| {
        let session = view.session.borrow();
        (
            session.open_pages(),
            session.storage(),
            view.index_client(cx).unwrap(),
        )
    });
    let planned = cx.update(|cx| {
        crate::block_move::plan(
            "s".into(),
            "m".into(),
            "d".into(),
            open_pages,
            index,
            storage,
            cx,
        )
    });
    finish(planned, "the move's plan", cx).unwrap()
}

/// Runs a foreground task to its end while the test executor keeps working.
fn finish<T: 'static>(task: gpui::Task<T>, what: &str, cx: &mut TestAppContext) -> T {
    let done = std::rc::Rc::new(std::cell::RefCell::new(None));
    let slot = done.clone();
    cx.update(|cx| {
        cx.spawn(async move |_| {
            slot.replace(Some(task.await));
        })
        .detach()
    });
    wait_until(cx, what, |_| done.borrow().is_some());
    done.take().unwrap()
}

fn write_move(
    notebook: &Notebook,
    plan: crate::block_move::Plan,
    cx: &mut TestAppContext,
) -> Result<usize, String> {
    let storage = read(notebook, cx, |view, _| view.session.borrow().storage());
    let written = cx.update(|cx| crate::block_move::write(plan, storage, cx));
    finish(written, "the move's write", cx)
}

#[gpui::test]
fn a_move_made_stale_by_typing_in_the_source_writes_nothing(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    let plan = plan_move(&notebook, cx);
    assert_eq!(plan.writes.len(), 3, "source, destination, and Refs");
    let buffer = read(&notebook, cx, |view, cx| {
        view.page
            .read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    });
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(0..0, "- Typed\n")], None, cx)
    });
    assert_eq!(
        write_move(&notebook, plan, cx),
        Err(crate::block_move::STALE_MOVE.into())
    );
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(stored(&store, "d").1, "# Dest\n");
    assert_eq!(stored(&store, "r").1, PAGES[2].2);
    assert!(stored(&store, "s").1.contains("- Move ^m\n  - Child ^c\n"));
    assert_eq!(
        buffer.read_with(cx, |buffer, _| buffer.text()),
        "- Typed\n- Keep ^keep\n- Move ^m\n  - Child ^c\n- After\n",
        "the typing stays and the block stays on its page"
    );
}

#[gpui::test]
fn a_move_made_stale_by_another_writer_leaves_every_page_as_it_was(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "s", cx);
    wait_until(cx, "Source's editor", |cx| {
        read(&notebook, cx, |view, _| view.editor.is_some())
    });
    let plan = plan_move(&notebook, cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let base = store.heads("d").unwrap();
    store
        .save(&quire_store::notebook::Edit {
            page: "d".into(),
            base,
            path: "Dest.md".into(),
            text: "# Dest\n- written elsewhere\n".into(),
            deleted: false,
            created: 1,
            recovery: None,
        })
        .unwrap();
    assert_eq!(
        write_move(&notebook, plan, cx),
        Err(crate::block_move::STALE_MOVE.into())
    );
    assert_eq!(
        stored(&store, "s").1,
        PAGES[0].2,
        "the source keeps the block"
    );
    assert_eq!(stored(&store, "d").1, "# Dest\n- written elsewhere\n");
    assert_eq!(stored(&store, "r").1, PAGES[2].2);
}

#[gpui::test]
fn moving_a_block_in_a_folder_notebook_rewrites_its_files(cx: &mut TestAppContext) {
    let files: Vec<(&str, &str)> = PAGES.iter().map(|(_, path, text)| (*path, *text)).collect();
    let opened = super::folder_tests::open_folder(&files, "Source.md", cx);
    let notebook = &opened.notebook;
    let file = |name: &str| std::fs::read_to_string(opened.folder.path().join(name)).unwrap();
    wait_until(cx, "Source's editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    // A move retargets the references the index knows, so wait until it has read Refs.md.
    let client = read(notebook, cx, |view, cx| view.index_client(cx).unwrap());
    let source = quire_store::sync::format::page_id_for_path("Source.md");
    let refs = quire_store::sync::format::page_id_for_path("Refs.md");
    wait_until(cx, "the index to see Refs.md link to Source", |_| {
        futures::executor::block_on(client.relocation(source.as_str().into()).unwrap())
            .unwrap()
            .is_some_and(|facts| {
                facts
                    .candidates
                    .iter()
                    .any(|candidate| candidate.page.to_lossy_string() == refs)
            })
    });
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j");
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    let dest = quire_store::sync::format::page_id_for_path("Dest.md");
    choose(notebook, "Dest", &dest, cx);
    // The notes are written one after another; the destination opens once all are.
    wait_until(cx, "the move to finish and open the destination", |cx| {
        active(notebook, cx) == dest
    });
    assert_eq!(file("Source.md"), "- Keep ^keep\n- After\n");
    assert_eq!(file("Dest.md"), "# Dest\n\n- Move ^m\n  - Child ^c\n");
    assert_eq!(
        file("Refs.md"),
        "- [[Dest#^m]] ((c)) [[Source#^keep]] [[Dest#^c|kid]]\n"
    );
    assert_eq!(file("Other.md"), "- unrelated\n");
    read(notebook, cx, |view, _| assert!(view.nav.error.is_none()));
}

/// A destination note that cannot be written fails the whole move: the source note keeps the
/// block and no reference is retargeted.
/// Unix only: it makes the destination folder read-only through its mode bits.
#[cfg(unix)]
#[gpui::test]
fn a_failed_folder_move_leaves_the_source_note_intact(cx: &mut TestAppContext) {
    use std::os::unix::fs::PermissionsExt as _;
    let files = [
        ("Source.md", PAGES[0].2),
        ("Locked/Dest.md", "# Dest\n"),
        ("Refs.md", "- [[Source#^m]] ((c))\n"),
    ];
    let opened = super::folder_tests::open_folder(&files, "Source.md", cx);
    let notebook = &opened.notebook;
    let locked = opened.folder.path().join("Locked");
    let file = |name: &str| std::fs::read_to_string(opened.folder.path().join(name)).unwrap();
    wait_until(cx, "Source's editor", |cx| {
        read(notebook, cx, |view, _| view.editor.is_some())
    });
    let mode = |mode| std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(mode));
    mode(0o555).unwrap();
    let unlock = util::defer(|| {
        mode(0o755).ok();
    });
    draw(notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j");
    cx.dispatch_action(notebook.window.into(), MoveBlockToPage);
    cx.run_until_parked();
    let dest = quire_store::sync::format::page_id_for_path("Locked/Dest.md");
    choose(notebook, "Dest", &dest, cx);
    wait_until(cx, "the failure", |cx| {
        read(notebook, cx, |view, _| view.nav.error.is_some())
    });
    drop(unlock);
    assert_eq!(file("Source.md"), PAGES[0].2);
    assert_eq!(file("Locked/Dest.md"), "# Dest\n");
    assert_eq!(file("Refs.md"), "- [[Source#^m]] ((c))\n");
    assert_eq!(
        active(notebook, cx),
        quire_store::sync::format::page_id_for_path("Source.md")
    );
}
