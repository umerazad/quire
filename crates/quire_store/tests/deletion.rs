#![cfg(unix)]

use anyhow::Result;
use quire_store::notebook::{CheckedSaves, Edit, NotebookStore, RescanLimits};
use std::{fs, path::Path};

fn notebook(root: &Path) -> Result<()> {
    fs::create_dir(root)?;
    fs::write(
        root.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )?;
    Ok(())
}

fn edit(page: &str, path: &str, text: &str) -> Edit {
    Edit {
        page: page.into(),
        base: vec![],
        path: path.into(),
        text: text.into(),
        deleted: false,
        created: 1,
        recovery: None,
    }
}

fn deletion(store: &NotebookStore, page: &str, path: &str, text: &str) -> Result<Edit> {
    Ok(Edit {
        base: store.heads(page)?,
        deleted: true,
        created: 2,
        ..edit(page, path, text)
    })
}

fn head(store: &NotebookStore, page: &str) -> Result<quire_store::sync::json::Json> {
    let heads = store.heads(page)?;
    assert_eq!(heads.len(), 1, "{page} must have one head");
    Ok(store.get(&heads[0])?.unwrap())
}

// Ports rt `tests/engine.test.ts` "deletes pages and folders on every computer, refusing a stale
// deletion" (`957da33d4766`).
#[test]
fn deletes_pages_and_folders_on_every_computer_refusing_a_stale_deletion() -> Result<()> {
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    notebook(&folder)?;
    let (office_dir, home_dir) = (root.path().join("office"), root.path().join("home"));
    fs::create_dir(&office_dir)?;
    fs::create_dir(&home_dir)?;
    let office = NotebookStore::open(&office_dir.join("local.sqlite"), "nb", "office")?;
    let home = NotebookStore::open(&home_dir.join("local.sqlite"), "nb", "home")?;
    office.save(&edit("plan", "Projects/Plan.md", "- plan\n"))?;
    office.save(&edit("notes", "Projects/Deep/Notes.md", "- notes\n"))?;
    office.save(&edit("keep", "Keep.md", "- keep\n"))?;
    office.save(&edit("near", "Projects2/Near.md", "- near\n"))?;
    office.publish_to(&folder, 10)?;
    home.rescan(&folder, RescanLimits::default(), None)?;

    let mut stale = deletion(&office, "keep", "Keep.md", "- keep\n")?;
    stale.base = vec!["0".repeat(64)];
    assert!(matches!(
        office.delete_if_heads(&[stale], None)?,
        CheckedSaves::HeadsChanged { page, .. } if page == "keep"
    ));

    let plan = deletion(&office, "plan", "Projects/Plan.md", "- plan\n")?;
    let notes = deletion(&office, "notes", "Projects/Deep/Notes.md", "- notes\n")?;
    let outbox = office.outbox()?;
    match office.delete_if_heads(std::slice::from_ref(&plan), Some("Projects"))? {
        CheckedSaves::Appeared { path } => assert_eq!(path, "Projects/Deep/Notes.md"),
        other => panic!("a folder deletion must name every page in it, got {other:?}"),
    }
    assert_eq!(office.outbox()?, outbox);
    assert_eq!(
        head(&office, "plan")?.root().field("deleted").as_bool(),
        Some(false)
    );
    assert!(
        office
            .delete_if_heads(std::slice::from_ref(&plan), Some("Elsewhere"))
            .is_err(),
        "a page outside the folder is refused"
    );
    let mut edited = plan.clone();
    edited.deleted = false;
    assert!(office.delete_if_heads(&[edited], None).is_err());

    assert_eq!(
        office
            .pages_in_folder("Projects")?
            .into_iter()
            .map(|(_, path)| path)
            .collect::<Vec<_>>(),
        ["Projects/Deep/Notes.md", "Projects/Plan.md"]
    );
    let CheckedSaves::Saved(acks) = office.delete_if_heads(&[plan, notes], Some("Projects"))?
    else {
        panic!("a current folder deletion must be written");
    };
    assert_eq!(acks.len(), 2);
    assert!(office.pages_in_folder("Projects")?.is_empty());
    assert_eq!(office.pages_in_folder("Projects2")?.len(), 1);
    let tombstone = head(&office, "plan")?;
    assert_eq!(tombstone.root().field("deleted").as_bool(), Some(true));
    assert_eq!(tombstone.root().field("text").as_str(), Some("- plan\n"));
    assert_eq!(
        tombstone
            .root()
            .field("parents")
            .elements()
            .unwrap()
            .count(),
        1,
        "the deletion follows the page's history"
    );

    office.publish_to(&folder, 11)?;
    let report = home.rescan(&folder, RescanLimits::default(), None)?;
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    for page in ["plan", "notes"] {
        assert_eq!(
            head(&home, page)?.root().field("deleted").as_bool(),
            Some(true)
        );
    }
    assert_eq!(
        head(&home, "keep")?.root().field("deleted").as_bool(),
        Some(false)
    );
    assert!(home.pages_in_folder("Projects")?.is_empty());
    Ok(())
}
