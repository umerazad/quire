//! Times index queries against a folder notebook's analyzed query index, without the app.
//!
//! `cargo run --release -p quire_store --example index_queries -- <notebook folder> <state folder>
//! <analyzer version>` opens `<state folder>/query-index.sqlite` (the app keeps it in
//! `folder-notebooks/<hash>` under its data folder) and prints one JSON object with each query's
//! times in milliseconds. Pass the app's analyzer version, which its `QUIRE_BENCH` report names;
//! another version would rebuild the index instead of reading it.

use anyhow::{Context as _, Result, ensure};
use quire_core::{
    daily::{DailyCalendar, DailyLayout},
    workspace::TaskView,
};
use quire_store::index::{IndexConfig, QueryIndex, TaskQuery};
use std::{path::PathBuf, time::Instant};

const RUNS: usize = 5;

fn times<T>(mut query: impl FnMut() -> Result<T>) -> Result<Vec<f64>> {
    (0..RUNS)
        .map(|_| {
            let started = Instant::now();
            query()?;
            Ok((started.elapsed().as_secs_f64() * 1_000_000.).round() / 1000.)
        })
        .collect()
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let usage = "Pass the notebook folder and its state folder: index_queries <notebook> <state> <analyzer version from the app's QUIRE_BENCH report>.";
    let root = PathBuf::from(args.next().context(usage)?);
    let state = PathBuf::from(args.next().context(usage)?);
    let analyzer: i64 = args
        .next()
        .context(usage)?
        .parse()
        .context("The analyzer version must be a whole number, such as 6.")?;
    ensure!(
        state.join("query-index.sqlite").exists(),
        "{} has no query-index.sqlite. Open the notebook in Quire until indexing finishes, then pass its state folder.",
        state.display()
    );
    let index = QueryIndex::open_folder(
        &root,
        &state,
        IndexConfig {
            analyzer,
            layout: DailyLayout::default(),
            calendar: DailyCalendar::utc(),
        },
    )?;
    let unanalyzed = index.unanalyzed()?;
    ensure!(
        unanalyzed * 100 <= index.pages()?.len(),
        "{unanalyzed} pages are not analyzed yet, so the queries would not be representative. Let Quire finish indexing and retry."
    );
    let pages = index.pages()?;
    let mut report = serde_json::Map::new();
    report.insert("pages".into(), pages.len().into());
    report.insert("unanalyzed".into(), unanalyzed.into());
    report.insert("page_list_ms".into(), times(|| index.pages())?.into());
    for view in [
        TaskView::Due,
        TaskView::Inbox,
        TaskView::Next,
        TaskView::Upcoming,
    ] {
        let query = TaskQuery {
            view,
            today: "2024-06-01".into(),
            filter: String::new(),
            project: None,
            limit: 200,
        };
        report.insert(
            format!("task_board_{view:?}_ms"),
            times(|| index.task_board(&query))?.into(),
        );
    }
    let backlinks: Vec<f64> = pages
        .iter()
        .take(20)
        .map(|page| times(|| index.backlinks(&page.page.page)))
        .collect::<Result<Vec<_>>>()?
        .concat();
    report.insert("backlinks_ms".into(), backlinks.into());
    for query in ["budget", "design storage", "ملاحظات"] {
        report.insert(
            format!("search_{query}_ms"),
            times(|| index.search(query))?.into(),
        );
    }
    report.insert("tag_names_ms".into(), times(|| index.tag_names())?.into());
    println!("{}", serde_json::Value::Object(report));
    Ok(())
}
