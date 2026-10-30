use super::*;
use crate::notebook_session::{NotebookSession, OpenPage};
use std::{collections::HashMap, path::PathBuf};
use theme::ActiveTheme;

pub(super) struct OpenOptions {
    database: PathBuf,
    notebook: String,
    writer: String,
    page: String,
}

impl OpenOptions {
    pub fn parse(arguments: Vec<String>) -> Result<Option<Self>, String> {
        const FLAGS: [&str; 4] = ["--notebook-db", "--notebook-id", "--writer-id", "--page-id"];
        if !arguments
            .iter()
            .any(|argument| FLAGS.contains(&argument.split('=').next().unwrap_or_default()))
        {
            return Ok(None);
        }
        let mut values = HashMap::new();
        let mut arguments = arguments.into_iter();
        while let Some(flag) = arguments.next() {
            if !FLAGS.contains(&flag.as_str()) {
                return Err(format!(
                    "Unsupported stored-page argument {flag:?}. Pass --notebook-db, --notebook-id, --writer-id, and --page-id with their values; open editor harnesses separately."
                ));
            }
            let value = arguments
                .next()
                .filter(|value| !value.is_empty() && !value.starts_with("--"))
                .ok_or_else(|| {
                    format!("Missing value for {flag}. Pass a nonempty value after the flag.")
                })?;
            if values.insert(flag.clone(), value).is_some() {
                return Err(format!(
                    "Repeated stored-page argument {flag}. Pass each option once."
                ));
            }
        }
        let mut take = |flag| {
            values.remove(flag).ok_or_else(|| format!("Missing stored-page argument {flag}. Pass --notebook-db, --notebook-id, --writer-id, and --page-id with the original database identities."))
        };
        Ok(Some(Self {
            database: take("--notebook-db")?.into(),
            notebook: take("--notebook-id")?,
            writer: take("--writer-id")?,
            page: take("--page-id")?,
        }))
    }

    pub fn open(self, cx: &mut App) -> Result<(), String> {
        let mut session = NotebookSession::open(self.database, self.notebook, self.writer)?;
        let page = session.open_page(self.page, cx);
        let options = gpui::WindowOptions {
            window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::centered(
                None,
                gpui::size(gpui::px(1100.), gpui::px(760.)),
                cx,
            ))),
            ..Default::default()
        };
        cx.open_window(options, move |window, cx| {
            window.set_window_title("Quire Native");
            cx.new(|cx| StoredWindow::new(session, page, window, cx))
        }).map_err(|error| format!("Cannot open Quire's window: {error}. Check desktop and graphics access, then retry."))?;
        cx.activate(true);
        Ok(())
    }
}

struct StoredWindow {
    _session: NotebookSession,
    page: Entity<OpenPage>,
    editor: Option<Entity<NotebookWindow>>,
}

impl StoredWindow {
    fn new(
        session: NotebookSession,
        page: Entity<OpenPage>,
        window: &mut Window,
        cx: &mut gpui::Context<Self>,
    ) -> Self {
        cx.observe_in(&page, window, |this, _, window, cx| {
            this.attach_editor(window, cx);
            cx.notify();
        })
        .detach();
        let mut this = Self {
            _session: session,
            page,
            editor: None,
        };
        this.attach_editor(window, cx);
        this
    }

    fn attach_editor(&mut self, window: &mut Window, cx: &mut gpui::Context<Self>) {
        if self.editor.is_some() {
            return;
        }
        let Some(page) = self.page.read(cx).loaded() else {
            return;
        };
        let analysis = page.analysis.clone();
        let title = page.path.as_deref().unwrap_or("Unsaved page");
        window.set_window_title(&format!("{title} · Quire Native"));
        eprintln!(
            "Quire page preview ready: {} revision bodies, {} heads, recovery={}, deleted={}, {:.1} ms to buffer ready.",
            page.loaded_revision_bodies,
            page.heads.len(),
            page.recovered,
            page.deleted,
            page.load_time.as_secs_f64() * 1000.
        );
        self.editor = Some(cx.new(|cx| NotebookWindow::for_page(analysis, None, window, cx)));
    }
}

impl Render for StoredWindow {
    fn render(&mut self, _: &mut Window, cx: &mut gpui::Context<Self>) -> impl IntoElement {
        let page = self.page.read(cx);
        let header = gpui::div()
            .p_2()
            .flex()
            .items_center()
            .gap_2()
            .child(Label::new(page.status()))
            .when(page.can_retry(), |div| {
                div.child(
                    Button::new("retry-page", "Retry page").on_click(cx.listener(
                        |this, _, _, cx| {
                            this.page.update(cx, |page, cx| page.retry(cx));
                        },
                    )),
                )
            });
        gpui::div()
            .size_full()
            .flex()
            .flex_col()
            .bg(cx.theme().colors().editor_background)
            .child(header)
            .when(page.is_loading(), |div| {
                div.child(gpui::div().p_2().child("Opening notebook storage…"))
            })
            .when_some(self.editor.clone(), |div, editor| {
                div.child(gpui::div().flex_1().min_h_0().child(editor))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arguments() -> Vec<String> {
        [
            "--notebook-db",
            "/tmp/notebook with spaces.sqlite",
            "--notebook-id",
            "nb",
            "--writer-id",
            "local",
            "--page-id",
            "p",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    #[test]
    fn explicit_open_arguments_keep_original_identities() {
        assert!(
            OpenOptions::parse(vec!["--write-harness".into()])
                .unwrap()
                .is_none()
        );
        let parsed = OpenOptions::parse(arguments()).unwrap().unwrap();
        assert_eq!(
            parsed.database,
            PathBuf::from("/tmp/notebook with spaces.sqlite")
        );
        assert_eq!(parsed.notebook, "nb");
        assert_eq!(parsed.writer, "local");
        assert_eq!(parsed.page, "p");
    }

    #[test]
    fn incomplete_duplicate_or_mixed_open_arguments_fail_before_startup() {
        for args in [
            vec!["--notebook-id".into()],
            vec!["--notebook-db".into(), "--notebook-id".into()],
            arguments()[..6].to_vec(),
            [arguments(), vec!["--page-id".into(), "other".into()]].concat(),
            [arguments(), vec!["--excerpt-harness".into()]].concat(),
            [arguments(), vec!["--input-harness".into()]].concat(),
            vec!["--notebook-db=/tmp/local.sqlite".into()],
        ] {
            assert!(OpenOptions::parse(args).is_err());
        }
    }

    #[gpui::test]
    fn stored_recovery_draws_in_two_read_only_views_with_conflicts_and_tombstones(
        cx: &mut gpui::TestAppContext,
    ) {
        use quire_store::notebook::{Draft, DraftStamp, Edit, NotebookStore};
        cx.update(|cx| init(cx).unwrap());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("local.sqlite");
        let store = NotebookStore::open(&path, "nb", "local").unwrap();
        let mut edit = Edit {
            page: "p".into(),
            base: vec![],
            path: "Note.md".into(),
            text: "committed".into(),
            deleted: false,
            created: 1,
            recovery: None,
        };
        let base = store.save(&edit).unwrap();
        edit.base = vec![base.revision_id().into()];
        edit.deleted = true;
        edit.text.clear();
        store.save(&edit).unwrap();
        edit.base.clear();
        edit.deleted = false;
        edit.text = "other branch".into();
        store.save(&edit).unwrap();
        let text = "- [ ] recovered اردو #work ^b-one\n";
        let draft = Draft {
            page: "p".into(),
            path: "Recovered.md".into(),
            text: text.into(),
            base: vec![base.revision_id().into()],
            stamp: DraftStamp {
                session: "previous".into(),
                version: 7,
            },
            created: 2,
        };
        store.put_draft(&draft, None).unwrap();
        let heads = store.heads("p").unwrap();
        let outbox = store.outbox().unwrap();
        drop(store);
        let mut session = NotebookSession::open(path.clone(), "nb".into(), "local".into()).unwrap();
        let page = cx.update(|cx| session.open_page("p".into(), cx));
        let window =
            cx.add_window(|window, cx| StoredWindow::new(session, page.clone(), window, cx));
        crate::notebook_session::tests::wait(&page, cx);
        cx.executor().run_until_parked();
        window
            .update(cx, |view, window, cx| {
                let page = view.page.read(cx);
                let loaded = page.loaded().unwrap();
                assert!(loaded.deleted);
                assert!(loaded.recovered);
                assert_eq!(loaded.heads.len(), 2);
                assert!(page.status().contains("2 conflicting versions"));
                assert!(page.status().contains("selected version is deleted"));
                assert!(page.status().contains("recovered unsaved changes"));
                let editor = view.editor.as_ref().unwrap();
                let (source, second) = {
                    let editor = editor.read(cx);
                    (editor.source.clone(), editor.second.clone())
                };
                assert!(source.read(cx).read_only(cx));
                assert!(second.read(cx).read_only(cx));
                assert_eq!(source.read(cx).text(cx), text);
                assert_eq!(second.read(cx).text(cx), text);
                crate::editor_projection::bind(&second, window, cx).unwrap();
            })
            .unwrap();
        cx.executor().run_until_parked();
        cx.update_window(window.into(), |_, window, cx| window.draw(cx).clear(cx))
            .unwrap();
        cx.simulate_keystrokes(window.into(), "i x escape d d");
        window
            .update(cx, |view, _, cx| {
                let editor = view.editor.as_ref().unwrap().read(cx);
                assert_eq!(editor.source.read(cx).text(cx), text);
                assert_eq!(editor.second.read(cx).text(cx), text);
            })
            .unwrap();
        let store = NotebookStore::open(&path, "nb", "local").unwrap();
        assert_eq!(store.heads("p").unwrap(), heads);
        assert_eq!(store.outbox().unwrap(), outbox);
        assert_eq!(store.draft("p").unwrap(), Some(draft));
    }
}
