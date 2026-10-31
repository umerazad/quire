//! Fault injection through the publication commit path: every write, synchronization, and rename
//! step of installing an attachment or a change file fails, or the process "crashes" right after
//! it, and the store is reopened as a restarted process would. No acknowledged save is lost, no
//! partially written file ever carries a final name, and the retry publishes exactly the frozen
//! selection.

use super::{Edit, NotebookStore, attachment};
use crate::{
    durable_files::{Step, faults},
    sync::format::{self, Kind},
};
use anyhow::Result;
use std::{fs, path::Path};

const STEPS: [Step; 6] = [
    Step::Create,
    Step::Write,
    Step::SyncContents,
    Step::Rename,
    Step::SyncInstalled,
    Step::SyncDirectory,
];

fn edit(page: &str, base: Vec<String>, text: &str) -> Edit {
    Edit {
        page: page.into(),
        base,
        path: format!("{page}.md"),
        text: text.into(),
        created: 1,
        deleted: false,
        recovery: None,
    }
}

/// Every file in `directory` either carries a temporary name or a final name whose bytes are
/// complete: the frozen change file, or an attachment matching its content name.
fn only_complete_files(directory: &Path, frozen: &[(String, Vec<u8>)]) -> Result<usize> {
    let mut temporaries = 0;
    let Ok(entries) = fs::read_dir(directory) else {
        return Ok(0);
    };
    for entry in entries {
        let name = entry?.file_name().into_string().unwrap();
        let path = directory.join(&name);
        if name.starts_with(".quire-") {
            temporaries += 1;
        } else if format::parse_name(&name, Kind::Batch).is_some() {
            let expected = frozen.iter().find(|(frozen, _)| *frozen == name);
            assert_eq!(
                Some(fs::read(&path)?),
                expected.map(|(_, data)| data.clone()),
                "{name} is visible only with its complete frozen bytes"
            );
        } else {
            attachment::verify_file(&path, &name)?;
        }
    }
    Ok(temporaries)
}

fn fault_then_restart(step: Step, crash: bool, with_attachment: bool) -> Result<()> {
    let label = format!(
        "{step:?} {} (attachment {with_attachment})",
        if crash { "crash" } else { "failure" }
    );
    let root = tempfile::tempdir()?;
    let folder = root.path().join("notebook");
    fs::create_dir(&folder)?;
    fs::write(
        folder.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )?;
    let database = root.path().join("local.sqlite");
    let store = NotebookStore::open(&database, "nb", "office")?;
    let reference = if with_attachment {
        format!(" ![]({})", store.attach(b"picture bytes", "a.png")?)
    } else {
        String::new()
    };
    let first = store.save(&edit("one", vec![], &format!("- first{reference}")))?;
    let second = store.save(&edit("two", vec![], "- second"))?;
    let acknowledged = [
        (
            first.revision_id().to_owned(),
            format!("- first{reference}"),
        ),
        (second.revision_id().to_owned(), "- second".to_owned()),
    ];

    faults::arm(step, crash);
    let failed = store.publish_to(&folder, 100);
    assert!(!faults::pending(), "{label}: the fault fired");
    assert!(failed.is_err(), "{label}: publishing reports the fault");
    let frozen = store
        .pending_publication()?
        .map(|pending| vec![(pending.name, pending.data)])
        .unwrap_or_default();
    assert_eq!(frozen.len(), 1, "{label}: the selection stays frozen");
    let log = folder.join("devices/office/log");
    let temporaries = only_complete_files(&log, &frozen)?
        + only_complete_files(&folder.join("attachments"), &frozen)?;
    let left_temporary = crash && matches!(step, Step::Create | Step::Write | Step::SyncContents);
    assert_eq!(
        temporaries > 0,
        left_temporary,
        "{label}: only a crash before the rename leaves a temporary file"
    );
    drop(store);

    let store = NotebookStore::open(&database, "nb", "office")?;
    for (id, text) in &acknowledged {
        let revision = store
            .get(id)?
            .unwrap_or_else(|| panic!("{label}: {id} kept"));
        assert_eq!(
            revision.root().field("text").as_str(),
            Some(text.as_str()),
            "{label}"
        );
    }
    assert_eq!(store.heads("one")?, [first.revision_id()], "{label}");
    assert_eq!(store.heads("two")?, [second.revision_id()], "{label}");
    let ack = store
        .publish_to(&folder, 200)?
        .unwrap_or_else(|| panic!("{label}: the retry publishes"));
    assert_eq!(ack.cursor.seq, 1, "{label}");
    assert_eq!(ack.revisions, 2, "{label}");
    assert_eq!(
        ack.name, frozen[0].0,
        "{label}: the retry publishes the frozen selection"
    );
    assert!(store.outbox()?.is_empty(), "{label}");
    only_complete_files(&log, &frozen)?;
    let batches = fs::read_dir(&log)?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter(|name| format::parse_name(name, Kind::Batch).is_some())
        .count();
    assert_eq!(batches, 1, "{label}: one change file");
    if with_attachment {
        only_complete_files(&folder.join("attachments"), &frozen)?;
        assert_eq!(
            fs::read_dir(folder.join("attachments"))?
                .filter_map(|entry| entry.ok())
                .filter(|entry| !entry.file_name().to_string_lossy().starts_with(".quire-"))
                .count(),
            1,
            "{label}: the attachment is delivered"
        );
    }
    Ok(())
}

#[test]
fn every_publication_step_failing_or_crashing_loses_no_save_and_shows_no_partial_file() -> Result<()>
{
    for with_attachment in [false, true] {
        for step in STEPS {
            for crash in [false, true] {
                fault_then_restart(step, crash, with_attachment)?;
            }
        }
    }
    Ok(())
}
