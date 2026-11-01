//! `quire --measure-index <notebook folder> <state folder> [edits]`, built with `test-support`:
//! indexes a folder notebook headless with the app's own analysis loop, then simulates typing in
//! `Topic 0.md`, as the app's open page would feed it, and runs the queries an open page's views
//! re-run after each change. It prints one JSON object with the build time, per-edit query times
//! in milliseconds, and resident memory in KiB. Only the given folders are read or written; pass
//! an empty state folder to time a first-open build.

use super::*;
use std::time::Instant;

const PAGE: &str = "Topic 0.md";
/// About 12 characters a second, as the app bench types.
const TYPING_PAUSE: Duration = Duration::from_millis(80);
/// How often the build is checked for completion.
const POLL: Duration = Duration::from_millis(50);

fn resident_kib() -> Option<u64> {
    let output = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &std::process::id().to_string()])
        .output()
        .ok()?;
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

fn millis(started: Instant) -> f64 {
    (started.elapsed().as_secs_f64() * 1_000_000.).round() / 1000.
}

pub fn run(root: PathBuf, state: PathBuf, edits: usize) {
    let root = std::fs::canonicalize(&root)
        .expect("Cannot resolve the notebook folder. Pass an existing folder notebook.");
    std::fs::create_dir_all(&state)
        .expect("Cannot create the state folder. Pass a writable folder.");
    let state = std::fs::canonicalize(&state)
        .expect("Cannot resolve the state folder. Pass a writable folder.");
    gpui_platform::headless().run(move |cx| {
        settings::init(cx);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
        crate::markdown::init(cx)
            .expect("Cannot load the Markdown grammar. Rebuild Quire with its bundled grammars.");
        let started = Instant::now();
        let sync = IndexSync::spawn_folder(
            root.clone(),
            state,
            config(DailyLayout::default(), DailyCalendar::utc()),
            cx,
        )
        .expect("Cannot start the index. Check the notebook and state folders.");
        let client = sync.read(cx).client().clone();
        cx.spawn(async move |cx| {
            loop {
                cx.background_executor().timer(POLL).await;
                if let Some(error) = sync.read_with(cx, |sync, _| sync.error().cloned()) {
                    eprintln!("The index reported: {error}");
                }
                match ask(client.unanalyzed()).await {
                    Ok(0) => break,
                    Ok(left) => eprintln!("{left} pages left after {} ms", millis(started)),
                    Err(error) => eprintln!("Cannot count unanalyzed pages: {error}"),
                }
            }
            let build_ms = millis(started);
            let resident_after_build = resident_kib();
            let text = std::fs::read_to_string(root.join(PAGE)).expect(
                "The notebook has no Topic 0.md. Generate it with tools/synthetic-notebook.ts.",
            );
            let page = JsString::from(quire_store::sync::format::page_id_for_path(PAGE).as_str());
            let mut samples: Vec<[f64; 4]> = Vec::new();
            let mut resident = Vec::new();
            let mut typed = text.trim_end().to_owned();
            let typing = Instant::now();
            for edit in 0..edits {
                typed.push(if edit % 7 == 6 { ' ' } else { 'x' });
                let work = cx.update(|cx| analyze(typed.clone(), cx));
                let index = cx.background_spawn(work).await.expect(
                    "Cannot analyze the typed page. Rebuild Quire with its bundled grammars.",
                );
                let overlay = Overlay {
                    path: PAGE.into(),
                    title: "Topic 0".into(),
                    index,
                };
                let at = Instant::now();
                ask(client.set_overlay(page.clone(), overlay)).await.expect(
                    "Cannot store the typed page's analysis. Check the state folder and retry.",
                );
                let overlay_ms = millis(at);
                let at = Instant::now();
                ask(client.backlinks(page.clone()))
                    .await
                    .expect("Cannot list backlinks. Check the state folder and retry.");
                ask(client.linked_references(page.clone(), false))
                    .await
                    .expect("Cannot list linked references. Check the state folder and retry.");
                let references_ms = millis(at);
                let at = Instant::now();
                let id = page.to_lossy_string();
                ask(client.with_generation(move |index| index.linked_tasks(&id)))
                    .await
                    .expect("Cannot list linked tasks. Check the state folder and retry.");
                let linked_ms = millis(at);
                let at = Instant::now();
                ask(client.tag_names())
                    .await
                    .expect("Cannot list tags. Check the state folder and retry.");
                let tags_ms = millis(at);
                samples.push([overlay_ms, references_ms, linked_ms, tags_ms]);
                if edit % 25 == 24 {
                    resident.push((millis(typing), resident_kib()));
                }
                cx.background_executor().timer(TYPING_PAUSE).await;
            }
            let column = |at: usize| samples.iter().map(|sample| sample[at]).collect::<Vec<_>>();
            let report = serde_json::json!({
                "build_ms": build_ms,
                "resident_kib_after_build": resident_after_build,
                "overlay_ms": column(0),
                "references_ms": column(1),
                "linked_tasks_ms": column(2),
                "tag_names_ms": column(3),
                "resident_kib_while_typing": resident,
            });
            println!("{report}");
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
