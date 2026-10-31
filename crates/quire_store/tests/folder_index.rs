//! A folder notebook and a Quire notebook holding the same pages answer queries identically.

use anyhow::Result;
use quire_core::daily::{DailyCalendar, DailyLayout};
use quire_store::{
    index::{IndexConfig, IndexedBlock, IndexedLink, PageIndex, QueryIndex},
    notebook::{Edit, NotebookStore},
    sync::{format::page_id_for_path, json::JsString},
};
use std::fs;

const PAGES: [(&str, &str); 4] = [
    ("Alpha.md", "- links to [[Beta]]\n- quokka\n"),
    ("Beta.md", "---\ntitle: Second\n---\n- beta body\n"),
    ("daily/2024-05-01.md", "- mentions [[Alpha]]\n"),
    ("Nested/Gamma.md", "- gamma links [[Alpha]] and [[Beta]]\n"),
];

fn config() -> IndexConfig {
    IndexConfig {
        analyzer: 1,
        layout: DailyLayout {
            folder: "daily".into(),
            title: "{YYYY}-{MM}-{DD}".into(),
        },
        calendar: DailyCalendar::utc(),
    }
}

/// A stand-in analyzer: one block per line, and a page link per `[[target]]`.
fn analyze(text: &str) -> PageIndex {
    let mut index = PageIndex::default();
    let mut offset = 0u32;
    for line in text.lines() {
        index.blocks.push(IndexedBlock {
            key: format!("at-{offset}"),
            from: offset,
            to: offset + line.len() as u32,
            kind: "list".into(),
            text: line.trim_start_matches("- ").into(),
            ..Default::default()
        });
        for (start, _) in line.match_indices("[[") {
            let end = line[start..]
                .find("]]")
                .map_or(line.len(), |end| start + end);
            index.links.push(IndexedLink {
                kind: "page".into(),
                syntax: "wiki".into(),
                target: line[start + 2..end].into(),
                from: offset + start as u32,
                to: offset + end as u32 + 2,
                text: line.into(),
                ..Default::default()
            });
        }
        offset += line.len() as u32 + 1;
    }
    index
}

fn analyze_all(index: &QueryIndex) -> Result<()> {
    for pending in index.pending(256)? {
        let analysis = analyze(pending.text.as_str().unwrap());
        assert!(index.put(&pending.revision, &analysis)?);
    }
    assert_eq!(index.unanalyzed()?, 0);
    Ok(())
}

fn answers(index: &QueryIndex) -> Result<Vec<String>> {
    let mut answers = Vec::new();
    for listed in index.pages()? {
        answers.push(format!(
            "page {} {} {:?} {:?}",
            listed.page.path, listed.page.title, listed.date, listed.page.page
        ));
    }
    for target in ["Alpha", "beta", "Second", "2024-05-01", "Gamma", "Missing"] {
        let found = index
            .find_page(target)?
            .map(|page| (page.page, page.path, page.title));
        answers.push(format!("find {target} {found:?}"));
    }
    for page in ["Alpha.md", "Beta.md"] {
        for link in index.backlinks(&JsString::from(page_id_for_path(page).as_str()))? {
            answers.push(format!(
                "backlink {page} {} {}",
                link.source.path, link.link.from
            ));
        }
    }
    for query in ["quokka", "body", "gamma", "Second"] {
        for hit in index.search(query)? {
            answers.push(format!(
                "search {query} {} {}",
                hit.source.path, hit.snippet
            ));
        }
    }
    Ok(answers)
}

#[test]
fn folder_and_store_notebooks_answer_queries_alike_and_follow_file_changes() -> Result<()> {
    let store_root = tempfile::tempdir()?;
    let database = store_root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "local")?;
    let folder = tempfile::tempdir()?;
    let state = tempfile::tempdir()?;
    for (path, text) in PAGES {
        store.save(&Edit {
            page: page_id_for_path(path),
            path: path.into(),
            text: text.into(),
            base: vec![],
            deleted: false,
            created: 1,
            recovery: None,
        })?;
        let target = folder.path().join(path);
        fs::create_dir_all(target.parent().unwrap())?;
        fs::write(target, text)?;
    }
    fs::create_dir_all(folder.path().join(".quire"))?;
    fs::write(folder.path().join(".quire/Hidden.md"), "- hidden quokka")?;
    let stored = QueryIndex::open(&database, config())?;
    let filed = QueryIndex::open_folder(folder.path(), state.path(), config())?;
    analyze_all(&stored)?;
    analyze_all(&filed)?;
    let expected = answers(&stored)?;
    assert!(
        expected
            .iter()
            .any(|answer| answer.starts_with("backlink Alpha.md"))
    );
    assert_eq!(answers(&filed)?, expected);

    fs::write(
        folder.path().join("Alpha.md"),
        "- links to [[Beta]]\n- wombat\n",
    )?;
    fs::remove_file(folder.path().join("Nested/Gamma.md"))?;
    let generation = filed.generation();
    assert_eq!(
        filed.search("wombat")?.len(),
        0,
        "unchanged until invalidated"
    );
    filed.invalidate();
    let beta = JsString::from(page_id_for_path("Beta.md").as_str());
    assert_eq!(filed.unanalyzed()?, 1);
    assert_eq!(
        filed.backlinks(&beta)?.len(),
        1,
        "a changed file keeps its links until its new text is analyzed"
    );
    assert!(
        filed.find_page("Gamma")?.is_none(),
        "a removed file leaves at once"
    );
    analyze_all(&filed)?;
    assert_eq!(filed.backlinks(&beta)?.len(), 1);
    assert!(filed.generation() > generation);
    assert_eq!(filed.search("wombat")?.len(), 1);
    assert!(filed.search("quokka")?.is_empty());
    assert!(filed.search("gamma")?.is_empty());
    assert!(filed.find_page("Gamma")?.is_none());
    drop(filed);

    let reopened = QueryIndex::open_folder(folder.path(), state.path(), config())?;
    assert_eq!(
        reopened.unanalyzed()?,
        0,
        "unchanged files are not analyzed again"
    );
    assert_eq!(reopened.search("wombat")?.len(), 1);
    Ok(())
}
