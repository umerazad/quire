use anyhow::Result;
use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_store::{
    index::{IndexConfig, IndexedBlock, IndexedLink, IndexedTag, PageIndex, QueryIndex},
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
