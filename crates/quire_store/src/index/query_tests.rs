//! Search and tag tallies answer as the versions that ranked, snipped, or tallied everything
//! did, which are kept here as references, while reading only what they return or what changed.

use super::super::test_notebook::TestNotebook;
use super::*;

/// Search as it was: every match ranked and given a snippet, saved rows of open pages skipped.
/// Equal ranks keep saved rows first, then row order, as `search` now promises.
fn search_ranking_everything(index: &QueryIndex, query: &str) -> Result<Vec<SearchHit>> {
    let Some(expression) = search_expression(query) else {
        return Ok(Vec::new());
    };
    let rows: Vec<(PageRow, (String, Option<String>))> = index.connection.select_bound(&format!(
        "SELECT revision,page,path,title,snippet,block_id FROM (
           SELECT {PAGE_COLUMNS},snippet(search,2,'','',' … ',24) AS snippet,
             search.block_id AS block_id,bm25(search) AS rank,0 AS source,search.rowid AS row FROM search
           JOIN pages p ON p.revision=search.revision
           WHERE search MATCH ?1 AND p.deleted=0
             AND p.page NOT IN (SELECT page FROM temp.overlay_pages)
         UNION ALL
           SELECT {PAGE_COLUMNS},snippet(overlay_search,2,'','',' … ',24),
             overlay_search.block_id,bm25(overlay_search),1,overlay_search.rowid FROM temp.overlay_search
           JOIN temp.overlay_pages p ON p.revision=overlay_search.revision
           WHERE overlay_search MATCH ?1
         ) ORDER BY rank,source,row LIMIT 40"
    ))?(expression)?;
    rows.into_iter()
        .map(|(page, (snippet, block_id))| {
            Ok(SearchHit {
                source: page_ref(page)?,
                snippet,
                block_id,
                from: None,
            })
        })
        .collect()
}

const QUERIES: [&str; 10] = [
    "budget",
    "budg",
    "plan review",
    "ملاحظات",
    "Topic",
    "person",
    "quarter note design",
    "launch",
    "nothingmatches",
    "draft-storage",
];

fn searches(notebook: &TestNotebook) -> Result<()> {
    for query in QUERIES {
        assert_eq!(
            notebook.index.search(query)?,
            search_ranking_everything(&notebook.index, query)?,
            "{query}"
        );
    }
    Ok(())
}

#[test]
fn search_ranks_and_snips_as_ranking_every_match_did() -> Result<()> {
    let notebook = TestNotebook::new(300, 5)?;
    assert_eq!(notebook.index.search("budget")?.len(), SHOWN_HITS);
    searches(&notebook)?;
    for (path, seed) in [
        ("Topic 1.md", 1),
        ("daily/2024-01-02.md", 2),
        ("Garden.md", 3),
    ] {
        notebook.open(path, seed)?;
    }
    searches(&notebook)?;
    let scratch: Option<i64> = notebook
        .index
        .connection
        .select_row("SELECT count(*) FROM temp.search_snippets")?()?;
    assert_eq!(scratch, Some(0), "snippet rows are rolled back");
    Ok(())
}

/// A notebook where most of the best matches belong to an open page, so the first pass skips
/// them and the results still fill from the rest.
#[test]
fn search_fills_past_saved_rows_of_open_pages() -> Result<()> {
    let notebook = TestNotebook::new(40, 9)?;
    for path in notebook.paths.iter().take(30) {
        notebook.open(path, 4)?;
    }
    searches(&notebook)
}

/// Search reads as many ranked rows as it shows, plus the saved rows of open pages it skips.
#[test]
fn search_reads_only_the_rows_it_shows() -> Result<()> {
    let notebook = TestNotebook::new(300, 5)?;
    notebook.open("Topic 1.md", 1)?;
    let index = &notebook.index;
    let matches: Option<usize> = index
        .connection
        .select_row("SELECT count(*) FROM search WHERE search MATCH '\"budget\"*'")?(
    )?;
    let hidden: Option<usize> = index.connection.select_row(
        "SELECT count(*) FROM search_rows s JOIN pages p ON p.revision=s.revision
         WHERE p.path='Topic 1.md'",
    )?()?;
    index.rows_read.set(0);
    index.search("budget")?;
    let read = index.rows_read.get();
    assert!(
        read <= SHOWN_HITS + hidden.unwrap(),
        "read {read} of {matches:?} matching rows"
    );
    assert!(
        matches.unwrap() > 4 * read,
        "the notebook has many more matches"
    );
    Ok(())
}

fn fresh_tally(index: &QueryIndex) -> Result<Vec<TagCount>> {
    let kept = index.tag_tallies.borrow_mut().take();
    let names = index.tag_names();
    *index.tag_tallies.borrow_mut() = kept;
    names
}

/// After a save, tag names recount only the tags whose saved rows changed, and answer as a full
/// tally does.
#[test]
fn tag_tallies_recount_only_changed_tags() -> Result<()> {
    let notebook = TestNotebook::new(300, 13)?;
    let index = &notebook.index;
    let names = index.tag_names()?;
    assert!(names.len() > 10);
    assert_eq!(names, fresh_tally(index)?);
    for (round, path) in ["Topic 2.md", "Garden.md", "daily/2024-01-07.md"]
        .into_iter()
        .enumerate()
    {
        let before: Vec<String> = index.connection.select_bound(
            "SELECT DISTINCT t.tag_key FROM tagged t JOIN pages p ON p.revision=t.revision WHERE p.path=?",
        )?(path)?;
        notebook.save(path, &format!("- saved again {round}\n"))?;
        notebook.analyze(100 + round as u32)?;
        let after: Vec<String> = index.connection.select_bound(
            "SELECT DISTINCT t.tag_key FROM tagged t JOIN pages p ON p.revision=t.revision WHERE p.path=?",
        )?(path)?;
        let changed: BTreeSet<String> = before.into_iter().chain(after).collect();
        let rows: Option<usize> = index.connection.select_row_bound(
            "SELECT count(*) FROM tagged WHERE tag_key IN (SELECT value FROM json_each(?))",
        )?(serde_json::to_string(&changed)?.as_str())?;
        index.rows_read.set(0);
        let names = index.tag_names()?;
        assert_eq!(
            index.rows_read.get(),
            rows.unwrap(),
            "only the uses of {changed:?} are reread"
        );
        assert_eq!(names, fresh_tally(index)?, "{path}");
    }
    let tagged: Option<String> = index.connection.select_row(
        "SELECT p.path FROM tagged t JOIN pages p ON p.revision=t.revision
         WHERE p.path LIKE 'Topic %' ORDER BY p.path LIMIT 1",
    )?()?;
    std::fs::remove_file(notebook.root.path().join("notebook").join(tagged.unwrap()))?;
    index.invalidate();
    assert_eq!(
        index.tag_names()?,
        fresh_tally(index)?,
        "a deleted page's tags are recounted"
    );
    notebook.open("Topic 4.md", 8)?;
    assert_eq!(index.tag_names()?, fresh_tally(index)?);
    Ok(())
}
