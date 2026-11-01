use super::super::*;
use super::tests::{
    Notebook, active, draw, key, open_configured, open_notebook, read, settle, wait_until,
};
use gpui::TestAppContext;
use quire_core::daily::{DailyCalendar, DailyLayout, format_daily, write_settings};
use quire_store::{index::SETTINGS_PAGE, notebook::NotebookStore};

/// 2026-10-30T12:00:00Z, a date every inhabited time zone shares at that instant.
const NOW: i64 = 1_793_361_600_000;
const DAY: i64 = 86_400_000;

fn journal() -> DailyLayout {
    DailyLayout {
        folder: "Journal".into(),
        title: "{DD}.{MM}.{YYYY}".into(),
    }
}

fn title(layout: &DailyLayout, at: i64) -> (String, String) {
    let calendar = DailyCalendar::system().unwrap();
    let date = calendar.date_at(at).unwrap();
    (format_daily(&layout.title, &date, &calendar).unwrap(), date)
}

fn daily_rows(notebook: &Notebook, cx: &mut TestAppContext) -> Vec<(String, String)> {
    read(notebook, cx, |view, cx| {
        view.browser_rows(cx)
            .into_iter()
            .map(|row| (row.page, row.detail))
            .collect()
    })
}

fn set_now(notebook: &Notebook, now: i64, cx: &mut TestAppContext) {
    notebook
        .window
        .update(cx, |view, _, _| view.nav.daily.now = Rc::new(move || now))
        .unwrap();
}

fn settings_text(notebook: &Notebook) -> Option<String> {
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    let checkout = store.checkout(SETTINGS_PAGE, None).unwrap();
    checkout
        .selected
        .and_then(|record| record.root().field("text").as_str().map(str::to_owned))
}

fn journal_notebook(cx: &mut TestAppContext) -> (Notebook, String, String) {
    let (today, date) = title(&journal(), NOW);
    let settings = write_settings(&journal());
    let journal_path = format!("Journal/{today}.md");
    let other_path = format!("Other/{today}.md");
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            (SETTINGS_PAGE, "notebook-settings", &settings),
            ("o", &other_path, "- elsewhere\n"),
            ("j", &journal_path, "- journal\n"),
            ("x", "2026-01-01.md", "- new year\n"),
        ],
        "a",
        cx,
    );
    set_now(&notebook, NOW, cx);
    (notebook, today, date)
}

#[gpui::test]
fn configured_daily_layout_reaches_the_index_and_the_daily_browser(cx: &mut TestAppContext) {
    let (notebook, today, date) = journal_notebook(cx);
    read(&notebook, cx, |view, _| {
        assert_eq!(view.nav.daily.layout, journal());
        assert!(view.nav.daily.read_only.is_none());
        assert!(
            view.nav
                .pages
                .iter()
                .all(|listed| listed.page.page.as_str() != Some(SETTINGS_PAGE))
        );
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::ShowDaily);
    cx.run_until_parked();
    draw(&notebook, cx);
    assert_eq!(
        daily_rows(&notebook, cx),
        [
            ("j".into(), date.clone()),
            ("o".into(), format!("{date} · also {today}")),
            ("x".into(), "2026-01-01".into()),
        ]
    );
    let duplicates = read(&notebook, cx, |view, _| view.daily_duplicates());
    assert_eq!(duplicates, 1, "one date has two daily pages");
    assert_eq!(
        super::daily::duplicates_notice(duplicates).as_deref(),
        Some(
            "1 date has more than one daily page; the one in the daily folder opens. The Daily list marks the others with \"also\" to tidy them."
        )
    );
    assert_eq!(super::daily::duplicates_notice(0), None);
    assert!(
        super::daily::duplicates_notice(3)
            .unwrap()
            .starts_with("3 dates have")
    );
    cx.simulate_keystrokes(notebook.window.into(), &key("j"));
    wait_until(cx, "today's journal page", |cx| {
        active(&notebook, cx) == "j"
    });
}

#[gpui::test]
fn open_today_creates_the_missing_page_from_the_layout(cx: &mut TestAppContext) {
    let (notebook, _, _) = journal_notebook(cx);
    set_now(&notebook, NOW + DAY, cx);
    let (tomorrow, _) = title(&journal(), NOW + DAY);
    cx.simulate_keystrokes(notebook.window.into(), &key("j"));
    let expected = format!("Journal/{tomorrow}.md");
    wait_until(cx, "the created daily page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .loaded()
                .is_some_and(|page| page.path.as_deref() == Some(expected.as_str()))
        })
    });
    let created = active(&notebook, cx);
    let store = NotebookStore::open(&notebook.path, "nb", "local").unwrap();
    assert_eq!(
        store
            .checkout(&created, None)
            .unwrap()
            .selected
            .unwrap()
            .root()
            .field("text")
            .as_str(),
        Some(crate::navigation::NEW_PAGE_TEXT)
    );
    settle(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "escape");
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.visit(quire_core::workspace::Destination::page("a"), window, cx)
        })
        .unwrap();
    cx.simulate_keystrokes(notebook.window.into(), &key("j"));
    wait_until(cx, "the same page again", |cx| {
        active(&notebook, cx) == created
    });
}

#[gpui::test]
fn saving_a_layout_writes_settings_and_reindexes_dates(cx: &mut TestAppContext) {
    let (notebook, _, _) = journal_notebook(cx);
    cx.dispatch_action(notebook.window.into(), crate::navigation::ShowDaily);
    cx.run_until_parked();
    let type_layout = |folder: &str, title: &str, cx: &mut TestAppContext| {
        let (folder, title) = (folder.to_owned(), title.to_owned());
        notebook
            .window
            .update(cx, |view, window, cx| {
                if view.nav.daily.editing.is_none() {
                    view.edit_layout(window, cx);
                }
                let [folder_field, title_field] = view.nav.daily.editing.clone().unwrap();
                folder_field.update(cx, |editor, cx| editor.set_text(folder, window, cx));
                title_field.update(cx, |editor, cx| editor.set_text(title, window, cx));
                view.save_layout(cx);
            })
            .unwrap();
        cx.run_until_parked();
    };
    type_layout("Daily", "{YYYY}", cx);
    assert!(
        read(&notebook, cx, |view, _| view.nav.error.clone())
            .unwrap()
            .contains("{DD}")
    );
    assert_eq!(settings_text(&notebook), Some(write_settings(&journal())));

    let iso = DailyLayout {
        folder: "Daily/{YYYY}".into(),
        title: "{YYYY}-{MM}-{DD}".into(),
    };
    type_layout(&iso.folder, &iso.title, cx);
    wait_until(cx, "the saved layout", |cx| {
        read(&notebook, cx, |view, _| view.nav.daily.editing.is_none())
    });
    assert_eq!(settings_text(&notebook), Some(write_settings(&iso)));
    settle(&notebook, cx);
    draw(&notebook, cx);
    assert_eq!(
        daily_rows(&notebook, cx),
        [("x".into(), "2026-01-01".into())]
    );
}

#[gpui::test]
fn unreadable_settings_stay_read_only_and_unchanged(cx: &mut TestAppContext) {
    let notebook = open_notebook(
        &[
            ("a", "Alpha.md", "- start\n"),
            (SETTINGS_PAGE, "notebook-settings", "{\"version\":2}"),
        ],
        "a",
        cx,
    );
    read(&notebook, cx, |view, _| {
        assert_eq!(view.nav.daily.layout, DailyLayout::default());
        assert!(
            view.nav
                .daily
                .read_only
                .as_ref()
                .unwrap()
                .contains("newer Quire")
        );
    });
    notebook
        .window
        .update(cx, |view, window, cx| {
            view.edit_layout(window, cx);
            view.save_layout(cx);
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(settings_text(&notebook).as_deref(), Some("{\"version\":2}"));
}

/// Opens the journal notebook on `a` the way a launch does, with the clock at `now`.
fn launch_journal(now: i64, cx: &mut TestAppContext) -> Notebook {
    let (today, _) = title(&journal(), NOW);
    let settings = write_settings(&journal());
    let journal_path = format!("Journal/{today}.md");
    cx.update(|cx| init(cx).unwrap());
    open_configured(
        &[
            ("a", "Alpha.md", "- start\n"),
            (SETTINGS_PAGE, "notebook-settings", &settings),
            ("j", &journal_path, "- journal\n"),
        ],
        "a",
        |view| {
            view.nav.daily.now = Rc::new(move || now);
            view.nav.daily.open_on_launch = true;
        },
        cx,
    )
}

#[gpui::test]
fn a_launch_starts_on_todays_page_after_the_last_page(cx: &mut TestAppContext) {
    let notebook = launch_journal(NOW, cx);
    wait_until(cx, "today's journal page", |cx| {
        active(&notebook, cx) == "j"
    });
    read(&notebook, cx, |view, _| {
        assert!(!view.nav.daily.open_on_launch);
        assert!(view.nav.error.is_none());
    });
    cx.dispatch_action(notebook.window.into(), crate::navigation::GoBack);
    wait_until(cx, "the restored page behind it", |cx| {
        active(&notebook, cx) == "a"
    });
}

#[gpui::test]
fn a_launch_creates_todays_page_from_the_configured_layout(cx: &mut TestAppContext) {
    let notebook = launch_journal(NOW + DAY, cx);
    let expected = format!("Journal/{}.md", title(&journal(), NOW + DAY).0);
    wait_until(cx, "the created daily page", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .loaded()
                .is_some_and(|page| page.path.as_deref() == Some(expected.as_str()))
        })
    });
}

#[gpui::test]
fn opening_without_a_launch_keeps_the_restored_page(cx: &mut TestAppContext) {
    let (notebook, _, _) = journal_notebook(cx);
    settle(&notebook, cx);
    assert_eq!(active(&notebook, cx), "a");
}
