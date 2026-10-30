use super::super::*;
use super::tests::{Notebook, active, draw, key, open_notebook, read, wait_until};
use crate::references::{Follow, Target};
use gpui::TestAppContext;
use quire_core::workspace::Destination;

pub(in crate::editor_host::stored) const PAGES: [(&str, &str, &str); 3] = [
    (
        "h",
        "Home.md",
        "- see [[Target]] and ((blk))\n- [[Missing Page]]\n",
    ),
    ("t", "Target.md", "- target ^blk\n"),
    ("o", "Other.md", "- other\n"),
];

pub(in crate::editor_host::stored) fn set_vim(vim: bool, cx: &mut TestAppContext) {
    cx.update(|cx| {
        settings::SettingsStore::update_global(cx, |store, cx| {
            store.set_user_settings(
                &format!(r#"{{"vim_mode":{vim},"session":{{"restore_unsaved_buffers":false}}}}"#),
                cx,
            )
        });
    });
}

fn side(notebook: &Notebook, cx: &mut TestAppContext) -> Option<String> {
    read(notebook, cx, |view, cx| {
        view.nav
            .side
            .as_ref()
            .map(|side| side.page.read(cx).id().to_owned())
    })
}

fn focus_home(notebook: &Notebook, cx: &mut TestAppContext) {
    wait_until(cx, "Home's editor", |cx| {
        read(notebook, cx, |view, cx| {
            view.page.read(cx).id() == "h" && view.editor.is_some()
        })
    });
    notebook
        .window
        .update(cx, |view, window, cx| view.focus_active(window, cx))
        .unwrap();
    draw(notebook, cx);
}

#[gpui::test]
fn vim_keys_follow_page_and_block_links_beside_and_to_backlinks(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "h", cx);
    focus_home(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "f T g d");
    wait_until(cx, "Target to open", |cx| active(&notebook, cx) == "t");

    cx.simulate_keystrokes(notebook.window.into(), &key("["));
    focus_home(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "f ( ctrl-w d");
    wait_until(cx, "the block's page beside", |cx| {
        side(&notebook, cx).as_deref() == Some("t")
    });
    assert_eq!(active(&notebook, cx), "h");

    focus_home(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "0 f T g shift-a");
    wait_until(cx, "Target's linked references", |cx| {
        active(&notebook, cx) == "t"
            && notebook
                .window
                .update(cx, |view, window, cx| {
                    view.shown_references(cx)
                        .is_some_and(|panel| panel.focus_handle(cx).contains_focused(window, cx))
                })
                .unwrap()
    });

    cx.simulate_keystrokes(notebook.window.into(), &key("["));
    focus_home(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "j f M g d");
    wait_until(cx, "the missing page to be created", |cx| {
        read(&notebook, cx, |view, cx| {
            view.page
                .read(cx)
                .loaded()
                .and_then(|loaded| loaded.path.clone())
                .as_deref()
                == Some("Missing Page.md")
        })
    });
}

#[gpui::test]
fn function_keys_follow_links_with_vim_off_and_plain_text_keeps_zed_behavior(
    cx: &mut TestAppContext,
) {
    let notebook = open_notebook(&PAGES, "h", cx);
    set_vim(false, cx);
    focus_home(&notebook, cx);
    cx.simulate_keystrokes(
        notebook.window.into(),
        "right right right right right right right right alt-f12",
    );
    wait_until(cx, "Target beside", |cx| {
        side(&notebook, cx).as_deref() == Some("t")
    });
    focus_home(&notebook, cx);
    cx.simulate_keystrokes(notebook.window.into(), "f12");
    wait_until(cx, "Target to open", |cx| active(&notebook, cx) == "t");
    let error = read(&notebook, cx, |view, _| view.nav.error.clone());
    assert_eq!(error, None);

    cx.simulate_keystrokes(notebook.window.into(), "f12");
    cx.run_until_parked();
    assert_eq!(
        active(&notebook, cx),
        "t",
        "a caret outside any link leaves Go to Definition to Zed"
    );
}

#[gpui::test]
fn a_newer_follow_replaces_an_unfinished_one(cx: &mut TestAppContext) {
    let notebook = open_notebook(&PAGES, "h", cx);
    focus_home(&notebook, cx);
    notebook
        .window
        .update(cx, |view, window, cx| {
            for target in ["Target", "Other"] {
                view.follow(
                    Follow {
                        target: Target::Page(target.into()),
                        beside: false,
                        backlinks: false,
                    },
                    window,
                    cx,
                );
            }
        })
        .unwrap();
    wait_until(cx, "the follow to finish", |cx| {
        read(&notebook, cx, |view, _| view.links.idle())
    });
    assert_eq!(active(&notebook, cx), "o");
    let recent = read(&notebook, cx, |view, _| view.nav.workspace.recent.clone());
    assert!(
        !recent.contains(&Destination::page("t")),
        "the superseded request never opened its page: {recent:?}"
    );

    notebook
        .window
        .update(cx, |view, window, cx| {
            view.follow(
                Follow {
                    target: Target::Block("nowhere".into()),
                    beside: false,
                    backlinks: false,
                },
                window,
                cx,
            )
        })
        .unwrap();
    wait_until(cx, "the follow to finish", |cx| {
        read(&notebook, cx, |view, _| view.links.idle())
    });
    let error = read(&notebook, cx, |view, _| view.nav.error.clone()).unwrap();
    assert!(error.contains("((nowhere))"), "{error}");
    assert_eq!(active(&notebook, cx), "o");
}
