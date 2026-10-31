use anyhow::Result;
use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_store::{
    index::{
        IndexConfig, IndexedBlock, IndexedLink, IndexedTag, IndexedTask, PageIndex, QueryIndex,
    },
    notebook::{Edit, NotebookStore},
    sync::json::JsString,
};
use std::path::Path;

fn save(store: &NotebookStore, page: &str, path: &str, text: &str) -> Result<String> {
    let base = store.heads(page)?;
    let ack = store.save(&Edit {
        page: page.into(),
        path: path.into(),
        text: text.into(),
        base,
        deleted: false,
        created: 1,
        recovery: None,
    })?;
    Ok(ack.revision_id().to_owned())
}

fn config(analyzer: i64) -> IndexConfig {
    IndexConfig {
        analyzer,
        layout: DailyLayout {
            folder: "daily".into(),
            title: "{YYYY}-{MM}-{DD}".into(),
        },
        calendar: DailyCalendar::utc(),
    }
}

fn link(kind: &str, target: &str, from: u32) -> IndexedLink {
    IndexedLink {
        kind: kind.into(),
        syntax: "wiki".into(),
        target: target.into(),
        from,
        to: from + target.len() as u32 + 4,
        text: format!("about {target}"),
        ..Default::default()
    }
}

fn block(id: Option<&str>, from: u32) -> IndexedBlock {
    IndexedBlock {
        key: id.map_or(format!("at-{from}"), str::to_owned),
        id: id.map(str::to_owned),
        from,
        to: from + 4,
        kind: "list".into(),
        text: "item".into(),
        ..Default::default()
    }
}

fn analyze_all(index: &QueryIndex, analyses: &[(&str, PageIndex)]) -> Result<()> {
    for pending in index.pending(256)? {
        let path = pending.path.as_str().unwrap().to_owned();
        let analysis = analyses
            .iter()
            .find(|(name, _)| *name == path)
            .map(|(_, analysis)| analysis.clone())
            .unwrap_or_default();
        assert!(index.put(&pending.revision, &analysis)?);
    }
    assert_eq!(index.unanalyzed()?, 0);
    Ok(())
}

fn paths(links: &[quire_store::index::Backlink]) -> Vec<(String, u32)> {
    links
        .iter()
        .map(|link| (link.source.path.clone(), link.link.from))
        .collect()
}

fn open(root: &Path) -> Result<(NotebookStore, std::path::PathBuf)> {
    let database = root.join("local.sqlite");
    Ok((NotebookStore::open(&database, "nb", "local")?, database))
}

#[test]
fn the_catalogue_follows_selected_heads_and_never_writes_the_notebook() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    let first = save(&store, "a", "Alpha.md", "one")?;
    save(&store, "b", "Beta.md", "two")?;
    let index = QueryIndex::open(&database, config(1))?;
    let pending = index.pending(256)?;
    assert_eq!(pending.len(), 2);
    assert!(pending[0].text.as_str().is_some());
    analyze_all(&index, &[])?;
    let second = save(&store, "a", "Alpha.md", "one more")?;
    assert_ne!(first, second);
    let pending = index.pending(256)?;
    assert_eq!(
        pending
            .iter()
            .map(|page| page.revision.as_str())
            .collect::<Vec<_>>(),
        [second.as_str()]
    );
    assert!(
        !index.put(&first, &PageIndex::default())?,
        "a superseded revision is dropped"
    );
    let before = std::fs::read(&database)?;
    analyze_all(&index, &[])?;
    assert_eq!(
        std::fs::read(&database)?,
        before,
        "the index must not write the notebook"
    );
    Ok(())
}

#[test]
fn find_page_prefers_names_then_unique_titles_then_dates() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "n", "Notes/Plan.md", "---\ntitle: Roadmap\n---\n")?;
    save(&store, "s1", "One.md", "---\ntitle: Shared\n---\n")?;
    save(&store, "s2", "Two.md", "---\ntitle: Shared\n---\n")?;
    save(&store, "d1", "2026-10-30.md", "")?;
    save(&store, "d2", "daily/2026-10-30.md", "")?;
    let index = QueryIndex::open(&database, config(1))?;
    let found = |target: &str| -> Result<Option<String>> {
        Ok(index.find_page(target)?.map(|page| page.path))
    };
    assert_eq!(found("notes/plan.MD")?, Some("Notes/Plan.md".into()));
    assert_eq!(found("Roadmap#^x")?, Some("Notes/Plan.md".into()));
    assert_eq!(
        found("shared")?,
        None,
        "a title shared by two pages is ambiguous"
    );
    assert_eq!(found("One")?, Some("One.md".into()));
    assert_eq!(
        found("daily/2026-10-30")?,
        Some("daily/2026-10-30.md".into())
    );
    assert_eq!(
        found("2026-10-30")?,
        Some("2026-10-30.md".into()),
        "an exact name wins over the date"
    );
    assert_eq!(
        found("October 30th, 2026")?,
        Some("daily/2026-10-30.md".into()),
        "the daily folder wins for dates"
    );
    assert_eq!(found("Missing")?, None);
    Ok(())
}

#[test]
fn backlinks_mentions_tags_and_block_references_follow_the_reference_rules() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(
        &store,
        "t",
        "Target.md",
        "---\ntitle: Goal\n---\n- x ^tid\n",
    )?;
    save(&store, "x", "b/Source.md", "")?;
    save(&store, "y", "a/Other.md", "")?;
    save(&store, "z", "Dup.md", "")?;
    save(&store, "day", "daily/2026-10-30.md", "")?;
    let index = QueryIndex::open(&database, config(1))?;
    analyze_all(
        &index,
        &[
            (
                "Target.md",
                PageIndex {
                    blocks: vec![block(Some("tid"), 2), block(Some("dup"), 9)],
                    ..Default::default()
                },
            ),
            (
                "b/Source.md",
                PageIndex {
                    links: vec![
                        link("page", "Target", 30),
                        link("page", "goal", 10),
                        link("block", "tid", 50),
                        link("block", "Target#^tid", 60),
                        link("block", "dup", 70),
                        link("embed", "Target", 80),
                        link("page", "2026-10-30", 90),
                        link("page", "Nowhere", 95),
                    ],
                    tagged: vec![IndexedTag {
                        tag: "#Plan".into(),
                        from: 3,
                        block_id: None,
                        snippet: "s".into(),
                    }],
                    ..Default::default()
                },
            ),
            (
                "a/Other.md",
                PageIndex {
                    links: vec![
                        link("page", "target.md", 5),
                        link("page", "October 30th, 2026", 6),
                    ],
                    tagged: vec![IndexedTag {
                        tag: "#plan".into(),
                        from: 1,
                        block_id: Some("q".into()),
                        snippet: "o".into(),
                    }],
                    ..Default::default()
                },
            ),
            (
                "Dup.md",
                PageIndex {
                    blocks: vec![block(Some("dup"), 0)],
                    ..Default::default()
                },
            ),
        ],
    )?;
    let target = JsString::from("t");
    assert_eq!(
        paths(&index.backlinks(&target)?),
        [
            ("a/Other.md".into(), 5),
            ("b/Source.md".into(), 10),
            ("b/Source.md".into(), 30),
            ("b/Source.md".into(), 50),
            ("b/Source.md".into(), 60),
            ("b/Source.md".into(), 80),
        ],
        "an ambiguous block ID is not a backlink"
    );
    assert_eq!(
        paths(&index.backlinks(&JsString::from("day"))?),
        [("a/Other.md".into(), 6), ("b/Source.md".into(), 90)]
    );
    assert_eq!(
        paths(&index.mentions("Nowhere")?),
        [("b/Source.md".into(), 95)]
    );
    assert_eq!(index.resolve_block("dup")?, None);
    assert_eq!(
        index.resolve_block("Dup#^dup")?.map(|page| page.path),
        Some("Dup.md".into())
    );
    let tagged = index.tagged_blocks("PLAN")?;
    assert_eq!(
        tagged
            .iter()
            .map(|hit| hit.source.path.as_str())
            .collect::<Vec<_>>(),
        ["a/Other.md", "b/Source.md"]
    );
    let names = index.tag_names()?;
    assert_eq!(names.len(), 1);
    assert_eq!((names[0].label.as_str(), names[0].count), ("#plan", 2));
    Ok(())
}

#[test]
fn damaged_foreign_or_outdated_indexes_rebuild_without_touching_notes() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "a", "A.md", "text")?;
    let index = QueryIndex::open(&database, config(1))?;
    analyze_all(&index, &[])?;
    drop(index);
    let index = QueryIndex::open(&database, config(1))?;
    assert_eq!(index.unanalyzed()?, 0, "a matching index is reused");
    drop(index);
    let index = QueryIndex::open(&database, config(2))?;
    assert_eq!(index.unanalyzed()?, 1, "a new analyzer version rebuilds");
    drop(index);
    std::fs::write(root.path().join("query-index.sqlite"), b"not a database")?;
    let _ = std::fs::remove_file(root.path().join("query-index.sqlite-wal"));
    let index = QueryIndex::open(&database, config(2))?;
    assert_eq!(index.unanalyzed()?, 1, "a damaged file is replaced");
    assert!(store.get(&store.heads("a")?[0])?.is_some());
    Ok(())
}

#[test]
fn overlays_shadow_committed_rows_and_release_after_the_final_analysis() -> Result<()> {
    use quire_store::index::Overlay;
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "s", "Source.md", "old")?;
    save(&store, "x", "X.md", "")?;
    save(&store, "y", "Y.md", "")?;
    let index = QueryIndex::open(&database, config(1))?;
    let to = |target: &str| PageIndex {
        links: vec![link("page", target, 2)],
        ..Default::default()
    };
    analyze_all(&index, &[("Source.md", to("X"))])?;
    let count = |page: &str| -> Result<usize> { Ok(index.backlinks(&JsString::from(page))?.len()) };
    assert_eq!((count("x")?, count("y")?), (1, 0));
    let generation = index.generation();
    let overlay = |target: &str| Overlay {
        path: "Source.md".into(),
        title: "Source".into(),
        index: to(target),
    };
    index.set_overlay(&JsString::from("s"), &overlay("Y"))?;
    assert!(index.generation() > generation);
    assert_eq!(
        (count("x")?, count("y")?),
        (0, 1),
        "the overlay replaces, never adds"
    );
    index.set_overlay(&JsString::from("s"), &overlay("Y"))?;
    assert_eq!(count("y")?, 1, "replacing an overlay does not duplicate it");

    index.set_overlay(
        &JsString::from("new"),
        &Overlay {
            path: "Fresh.md".into(),
            title: "Fresh".into(),
            index: to("X"),
        },
    )?;
    assert_eq!(
        index.find_page("fresh")?.map(|page| page.path),
        Some("Fresh.md".into())
    );
    assert_eq!(count("x")?, 1, "an unsaved page links like a saved one");
    index.release_overlay(&JsString::from("new"))?;
    assert_eq!(
        index.find_page("fresh")?,
        None,
        "a never-saved page leaves on release"
    );

    save(&store, "s", "Source.md", "new")?;
    index.release_overlay(&JsString::from("s"))?;
    assert_eq!(index.unanalyzed()?, 1);
    assert_eq!(
        count("y")?,
        1,
        "the overlay stays until the final save is analyzed"
    );
    analyze_all(&index, &[("Source.md", to("Y"))])?;
    assert_eq!(count("y")?, 1);
    let source = index.find_page("Source")?.unwrap();
    assert!(
        !source.revision.starts_with("overlay:"),
        "committed rows took over"
    );
    Ok(())
}

fn task(text: &str, state: &str, dates: quire_core::tasks::DateSources) -> IndexedTask {
    IndexedTask {
        key: text.into(),
        raw: format!("[ ] {text}"),
        text: text.into(),
        state: state.into(),
        dates,
        ..Default::default()
    }
}

fn due_by_link(target: &str) -> quire_core::tasks::DateSources {
    quire_core::tasks::DateSources {
        links: vec![target.into()],
        ..Default::default()
    }
}

#[test]
fn task_rows_keep_date_sources_through_layout_changes_and_overlays() -> Result<()> {
    use quire_store::index::{Overlay, TaskQuery};
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "w", "Work.md", "tasks")?;
    save(&store, "d", "daily/28.10.2026.md", "daily")?;
    let analyses = [
        (
            "Work.md",
            PageIndex {
                tasks: vec![
                    task("Dotted", "next", due_by_link("30.10.2026")),
                    task("Iso", "next", due_by_link("2026-10-31")),
                ],
                ..Default::default()
            },
        ),
        (
            "daily/28.10.2026.md",
            PageIndex {
                tasks: vec![task("On the day", "inbox", Default::default())],
                ..Default::default()
            },
        ),
    ];
    let dues = |index: &QueryIndex| -> Result<Vec<(String, Option<String>)>> {
        Ok(index
            .tasks()?
            .into_iter()
            .map(|task| (task.text, task.due))
            .collect())
    };
    {
        let index = QueryIndex::open(&database, config(1))?;
        analyze_all(&index, &analyses)?;
        assert_eq!(
            dues(&index)?,
            [
                ("On the day".into(), None),
                ("Dotted".into(), None),
                ("Iso".into(), Some("2026-10-31".into())),
            ]
        );
    }
    let dotted = IndexConfig {
        layout: DailyLayout {
            folder: "daily".into(),
            title: "{DD}.{MM}.{YYYY}".into(),
        },
        ..config(1)
    };
    let index = QueryIndex::open(&database, dotted)?;
    assert_eq!(index.unanalyzed()?, 0, "a layout change reparses nothing");
    assert_eq!(
        dues(&index)?,
        [
            ("On the day".into(), Some("2026-10-28".into())),
            ("Dotted".into(), Some("2026-10-30".into())),
            ("Iso".into(), Some("2026-10-31".into())),
        ],
        "dates come from the stored sources and the daily page under the new layout"
    );
    let board = |index: &QueryIndex| {
        index.task_board(&TaskQuery {
            view: quire_core::workspace::TaskView::Next,
            today: "2026-10-30".into(),
            limit: 40,
            ..Default::default()
        })
    };
    assert_eq!(board(&index)?.counts.due, 2);
    let overlay = |tasks: Vec<IndexedTask>| Overlay {
        path: "Work.md".into(),
        title: "Work".into(),
        index: PageIndex {
            tasks,
            front_tags: vec!["project".into()],
            ..Default::default()
        },
    };
    index.set_overlay(
        &JsString::from("w"),
        &overlay(vec![task("Typed", "next", Default::default())]),
    )?;
    index.set_overlay(
        &JsString::from("w"),
        &overlay(vec![
            task("Typed", "next", Default::default()),
            task("More", "waiting", Default::default()),
        ]),
    )?;
    let shown = board(&index)?;
    assert_eq!(
        shown
            .rows
            .iter()
            .map(|task| task.text.as_str())
            .collect::<Vec<_>>(),
        ["Typed"],
        "unsaved tasks replace the page's saved ones"
    );
    assert_eq!((shown.counts.next, shown.counts.waiting), (1, 1));
    assert_eq!(
        shown.projects[0].open, 2,
        "front matter overlays replace each other instead of colliding"
    );
    Ok(())
}

#[test]
fn linked_tasks_come_from_other_pages_linking_by_any_name() -> Result<()> {
    use quire_core::workspace::TaskStatus;
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "m", "Meeting.md", "m")?;
    save(&store, "w", "Work.md", "w")?;
    let linked = |text: &str, state: &str, links: &[&str]| IndexedTask {
        links: links.iter().map(|link| link.to_string()).collect(),
        ..task(text, state, Default::default())
    };
    let analyses = [
        (
            "Meeting.md",
            PageIndex {
                tasks: vec![linked("Own task", "next", &["Meeting"])],
                ..Default::default()
            },
        ),
        (
            "Work.md",
            PageIndex {
                tasks: vec![
                    linked("Send notes", "next", &["meeting"]),
                    linked("Booked room", "done", &["Meeting.md"]),
                    linked("Unrelated", "next", &["Elsewhere"]),
                ],
                ..Default::default()
            },
        ),
    ];
    let index = QueryIndex::open(&database, config(1))?;
    analyze_all(&index, &analyses)?;
    let tasks = index.linked_tasks("m")?;
    let texts: Vec<&str> = tasks.iter().map(|task| task.text.as_str()).collect();
    assert_eq!(texts, ["Send notes", "Booked room"]);
    let shown = |status| {
        tasks
            .iter()
            .filter(|task| task.has_status(status, "2026-10-30"))
            .count()
    };
    assert_eq!(
        TaskStatus::ALL.map(shown),
        [2, 1, 0, 1],
        "All, Open, Due, Done"
    );
    Ok(())
}

#[test]
fn block_choices_and_previews_read_live_blocks_and_refuse_ambiguous_ids() -> Result<()> {
    use quire_store::index::Overlay;
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "a", "Alpha.md", "a")?;
    save(&store, "b", "Beta.md", "b")?;
    let index = QueryIndex::open(&database, config(1))?;
    let text =
        |id: Option<&str>, from: u32, text: &str, kind: &str, parent: Option<&str>| IndexedBlock {
            text: text.into(),
            kind: kind.into(),
            parent: parent.map(str::to_owned),
            ..block(id, from)
        };
    let alpha = PageIndex {
        blocks: vec![
            text(Some("zebra"), 0, "Zebras graze at dawn", "list", None),
            text(None, 10, "zebra child", "list", Some("zebra")),
            text(None, 20, "zebra code", "code", None),
        ],
        ..Default::default()
    };
    let beta = PageIndex {
        blocks: vec![text(Some("dup"), 0, "Dawn chorus", "paragraph", None)],
        ..Default::default()
    };
    analyze_all(&index, &[("Alpha.md", alpha), ("Beta.md", beta)])?;
    let choices = |query: &str, limit: usize| -> Result<Vec<(String, String)>> {
        Ok(index
            .block_choices(query, limit)?
            .into_iter()
            .map(|choice| (choice.source.path, choice.key))
            .collect())
    };
    let pair = |path: &str, key: &str| (path.to_owned(), key.to_owned());
    assert_eq!(
        choices("ZEBRA", 10)?,
        [pair("Alpha.md", "zebra"), pair("Alpha.md", "at-10")],
        "code blocks are left out"
    );
    assert_eq!(
        choices("dawn", 10)?,
        [pair("Alpha.md", "zebra"), pair("Beta.md", "dup")]
    );
    assert_eq!(choices("dawn graze", 10)?, [pair("Alpha.md", "zebra")]);
    assert_eq!(
        choices("", 1)?.len(),
        1,
        "an empty query lists blocks up to the limit"
    );

    let preview = index.block_preview("zebra")?.unwrap();
    assert_eq!(
        (
            preview.source.path.as_str(),
            preview.text.as_str(),
            preview.children
        ),
        ("Alpha.md", "Zebras graze at dawn", true)
    );
    assert_eq!(index.block_preview("Alpha#^zebra")?.unwrap().id, "zebra");
    assert!(index.block_preview("Beta#^zebra")?.is_none());
    assert!(index.block_preview("missing")?.is_none());

    index.set_overlay(
        &JsString::from("a"),
        &Overlay {
            path: "Alpha.md".into(),
            title: "Alpha".into(),
            index: PageIndex {
                blocks: vec![text(Some("dup"), 0, "Unsaved dawn", "list", None)],
                ..Default::default()
            },
        },
    )?;
    assert!(
        index.block_preview("dup")?.is_none(),
        "an ID on two pages resolves to nothing"
    );
    assert!(index.block_preview("zebra")?.is_none());
    assert_eq!(
        choices("unsaved", 10)?,
        [pair("Alpha.md", "dup")],
        "unsaved text is offered"
    );
    Ok(())
}

#[test]
fn relocation_candidates_include_pages_not_analyzed_yet() -> Result<()> {
    let root = tempfile::tempdir()?;
    let (store, database) = open(root.path())?;
    save(&store, "o", "Old.md", "- old")?;
    save(&store, "l", "Links.md", "- [o](Old.md)")?;
    save(&store, "k", "Keep.md", "- keep")?;
    let index = QueryIndex::open(&database, config(1))?;
    analyze_all(&index, &[])?;
    let candidates = |index: &QueryIndex| -> Result<Vec<String>> {
        Ok(index
            .relocation(&JsString::from("o"))?
            .unwrap()
            .candidates
            .into_iter()
            .map(|page| page.path)
            .collect())
    };
    assert_eq!(candidates(&index)?, ["Old.md"], "no analysis names a link");
    // Another writer saves a page; until it is analyzed, its links are unknown, so it is read.
    save(&store, "l", "Links.md", "- [o](Old.md) edited")?;
    assert_eq!(candidates(&index)?, ["Old.md", "Links.md"]);
    analyze_all(&index, &[])?;
    assert_eq!(candidates(&index)?, ["Old.md"]);
    Ok(())
}
