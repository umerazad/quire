use super::*;
use crate::{
    notebook_session::{NotebookSession, tests::wait},
    query_index::{IndexSync, default_config},
};
use gpui::TestAppContext;
use quire_core::daily::DailyCalendar;
use quire_store::notebook::NotebookStore;

struct Notebook {
    _root: tempfile::TempDir,
    store: NotebookStore,
    session: NotebookSession,
    index: IndexClient,
    _sync: Entity<IndexSync>,
}

fn save(store: &NotebookStore, page: &str, text: &str) -> String {
    let base = store.heads(page).unwrap();
    store
        .save(&Edit {
            page: page.into(),
            base,
            path: format!("{page}.md"),
            text: text.into(),
            deleted: false,
            created: 1,
            recovery: None,
        })
        .unwrap()
        .revision_id()
        .to_owned()
}

fn stored(store: &NotebookStore, page: &str) -> String {
    let checkout = store.checkout(page, None).unwrap();
    checkout
        .selected
        .unwrap()
        .root()
        .field("text")
        .as_str()
        .unwrap()
        .to_owned()
}

async fn notebook(pages: &[(&str, &str)], cx: &mut TestAppContext) -> Notebook {
    cx.executor().allow_parking();
    cx.update(|cx| crate::editor_host::init(cx).unwrap());
    let root = tempfile::tempdir().unwrap();
    let database = root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "local").unwrap();
    for (page, text) in pages {
        save(&store, page, text);
    }
    let session = NotebookSession::open(database.clone(), "nb".into(), "local".into()).unwrap();
    let sync = cx
        .update(|cx| IndexSync::spawn(database, default_config(DailyCalendar::utc()), cx))
        .unwrap();
    let index = cx.read(|cx| sync.read(cx).client().clone());
    crate::query_index::tests::settle(&index, cx).await;
    Notebook {
        _root: root,
        store,
        session,
        index,
        _sync: sync,
    }
}

fn open(notebook: &mut Notebook, page: &str, cx: &mut TestAppContext) -> Entity<Buffer> {
    let open = cx.update(|cx| notebook.session.open_page(page.into(), cx));
    wait(&open, cx);
    cx.update(|cx| notebook.session.enable_editing(cx));
    cx.run_until_parked();
    cx.read(|cx| {
        open.read(cx)
            .persistence()
            .unwrap()
            .read(cx)
            .buffer()
            .clone()
    })
}

async fn preview(
    notebook: &Notebook,
    rename: Option<&str>,
    cx: &mut TestAppContext,
) -> Result<TagEditPlan, String> {
    let retag = Retag::new(&["#area"], rename).unwrap();
    let work = cx.update(|cx| {
        plan(
            retag,
            notebook.session.open_pages(),
            notebook.index.clone(),
            notebook.session.client().clone(),
            cx,
        )
    });
    work.await
}

async fn run(
    notebook: &Notebook,
    plan: TagEditPlan,
    cx: &mut TestAppContext,
) -> Result<usize, String> {
    let work = cx.update(|cx| apply(plan, notebook.session.client().clone(), cx));
    let result = work.await;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        cx.run_until_parked();
        let idle = cx.read(|cx| {
            notebook.session.open_pages().iter().all(|page| {
                page.read(cx)
                    .persistence()
                    .is_none_or(|persistence| persistence.read(cx).is_idle())
            })
        });
        if idle {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "open pages did not settle after the edit"
        );
        std::thread::yield_now();
    }
    result
}

use language::Buffer;

#[gpui::test]
async fn renames_closed_and_open_pages_keeping_unsaved_typing(cx: &mut TestAppContext) {
    let mut notebook = notebook(
        &[
            ("Closed", "- #area/home chores\r\n- `#area` stays\r\n"),
            ("Open", "- #area one\n"),
            ("Other", "- #areas stays\n"),
        ],
        cx,
    )
    .await;
    let buffer = open(&mut notebook, "Open", cx);
    buffer.update(cx, |buffer, cx| {
        buffer.edit([(12..12, "typed\n")], None, cx)
    });

    let plan = preview(&notebook, Some("job"), cx).await.unwrap();
    let pages: Vec<_> = plan.pages.iter().map(|page| page.path.as_str()).collect();
    assert_eq!(pages, ["Closed.md", "Open.md"]);
    let changes: Vec<_> = plan.pages.iter().flat_map(|page| &page.changes).collect();
    assert_eq!(
        changes,
        [
            &TagChange {
                before: "#area/home".into(),
                after: "#job/home".into()
            },
            &TagChange {
                before: "#area".into(),
                after: "#job".into()
            },
        ]
    );
    assert_eq!(run(&notebook, plan, cx).await, Ok(2));

    assert_eq!(
        stored(&notebook.store, "Closed"),
        "- #job/home chores\r\n- `#area` stays\r\n"
    );
    assert_eq!(stored(&notebook.store, "Open"), "- #job one\ntyped\n");
    assert_eq!(stored(&notebook.store, "Other"), "- #areas stays\n");
    assert_eq!(cx.read(|cx| buffer.read(cx).text()), "- #job one\ntyped\n");
}

#[gpui::test]
async fn typing_after_the_preview_refuses_the_whole_edit(cx: &mut TestAppContext) {
    let mut notebook = notebook(&[("Closed", "- #area\n"), ("Open", "- #area one\n")], cx).await;
    let buffer = open(&mut notebook, "Open", cx);
    let plan = preview(&notebook, None, cx).await.unwrap();
    buffer.update(cx, |buffer, cx| buffer.edit([(0..0, "x")], None, cx));
    assert_eq!(run(&notebook, plan, cx).await, Err(STALE_PREVIEW.into()));
    assert_eq!(stored(&notebook.store, "Closed"), "- #area\n");
}

#[gpui::test]
async fn a_closed_page_saved_after_the_preview_refuses_the_whole_edit(cx: &mut TestAppContext) {
    let mut notebook = notebook(&[("Closed", "- #area\n"), ("Open", "- #area one\n")], cx).await;
    open(&mut notebook, "Open", cx);
    let plan = preview(&notebook, None, cx).await.unwrap();
    save(&notebook.store, "Closed", "- #area changed\n");
    assert_eq!(run(&notebook, plan, cx).await, Err(STALE_PREVIEW.into()));
    assert_eq!(stored(&notebook.store, "Open"), "- #area one\n");
}
