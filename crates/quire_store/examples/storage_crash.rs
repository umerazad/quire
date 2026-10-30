use anyhow::{Context as _, Result, bail};
use quire_store::{
    database::open_writable,
    notebook::{BatchImport, Draft, DraftStamp, Edit, NotebookStore, RescanLimits},
};
use serde_json::json;
use std::{
    io::{self, Write},
    path::Path,
};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mode = args.get(1).context(
        "Specify attach, save, draft, select, publish, import, rescan, interrupt, or inspect and a database path.",
    )?;
    let path = Path::new(
        args.get(2)
            .context("Specify an absolute test database path.")?,
    );
    let version = args
        .get(3)
        .map(String::as_str)
        .unwrap_or("1")
        .parse::<u64>()?;
    let text = args.get(4).cloned().unwrap_or_default();
    let store = NotebookStore::open(path, "nb-crash", "device-crash")?;
    let page = "p-process";
    let ready = match mode.as_str() {
        "attach" => {
            let reference = store.attach(text.as_bytes(), "probe.bin")?;
            json!({"mode":mode,"reference":reference})
        }
        "save" | "draft" => {
            let base = store.heads(page)?;
            let stamp = DraftStamp {
                session: "probe".into(),
                version,
            };
            let draft = Draft {
                page: page.into(),
                path: "Crash.md".into(),
                text: text.clone(),
                base: base.clone(),
                stamp: stamp.clone(),
                created: version,
            };
            let current = store.draft(page)?;
            store.put_draft(&draft, current.as_ref().map(|draft| &draft.stamp))?;
            if mode == "save" {
                let saved = store.save(&Edit {
                    page: page.into(),
                    path: "Crash.md".into(),
                    text,
                    base,
                    recovery: Some(stamp),
                    deleted: false,
                    created: version,
                })?;
                json!({"mode":mode,"id":saved.revision_id()})
            } else {
                json!({"mode":mode})
            }
        }
        "select" => {
            // This probe has no provider files; the real publisher must scan first.
            let cursor = store.cursor(store.device_id())?;
            store.mark_reconciled(cursor.as_ref())?;
            let selected = store
                .prepare_publication(version)?
                .context("No revision to select.")?;
            json!({"mode":mode,"name":selected.name})
        }
        "publish" => {
            let folder = path
                .parent()
                .context("Database has no parent folder.")?
                .join("notebook");
            let published = store
                .publish_to(&folder, version)?
                .context("No revision to publish.")?;
            json!({"mode":mode,"name":published.name,"seq":published.cursor.seq,"hash":published.cursor.hash})
        }
        "rescan" => {
            let folder = path
                .parent()
                .context("Database has no parent folder.")?
                .join("notebook");
            let report = store.rescan(&folder, RescanLimits::default(), None)?;
            if !report.problems.is_empty()
                || !report.waiting.is_empty()
                || report.continuation.is_some()
            {
                bail!("The crash probe's rescan is incomplete: {report:?}");
            }
            json!({"mode":mode,"applied":report.applied,"batches":report.batches,"attachments":report.attachments_completed})
        }
        "import" => {
            let file = Path::new(&text);
            let name = file
                .file_name()
                .and_then(|name| name.to_str())
                .context("Specify the incoming batch file as the final argument.")?;
            let BatchImport::Applied(imported) =
                store.import_batch("peer-crash", name, &std::fs::read(file)?)?
            else {
                bail!("The crash probe's incoming batch did not apply.");
            };
            json!({"mode":mode,"seq":imported.cursor.seq,"hash":imported.cursor.hash,"applied":imported.applied})
        }
        "interrupt" => {
            let connection = open_writable(path)?;
            connection.exec("BEGIN IMMEDIATE")?()?;
            connection.exec("DELETE FROM heads")?()?;
            println!("{}", json!({"mode":mode}));
            io::stdout().flush()?;
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            bail!("The crash probe resumed instead of being killed.");
        }
        "inspect" => {
            let head = store.heads(page)?.into_iter().next();
            let text = head
                .map(|id| -> Result<_> {
                    Ok(store
                        .get(&id)?
                        .context("Saved head is missing.")?
                        .root()
                        .field("text")
                        .as_str()
                        .map(str::to_owned))
                })
                .transpose()?
                .flatten();
            let versions = store
                .heads(page)?
                .into_iter()
                .map(|id| -> Result<_> {
                    let record = store.get(&id)?.context("An imported head is missing.")?;
                    Ok(json!({"id":id,"text":record.root().field("text").as_str()}))
                })
                .collect::<Result<Vec<_>>>()?;
            println!(
                "{}",
                json!({"text":text,"versions":versions,"draft":store.draft(page)?,"outbox":store.outbox()?,"pending":store.pending_publication()?.map(|pending| pending.name),"cursor":store.cursor(store.device_id())?,"peerCursor":store.cursor("peer-crash")?,"peerBatchHash":store.batch_hash("peer-crash",1)?,"pendingAttachments":store.pending_attachments(None,100)?})
            );
            return Ok(());
        }
        _ => bail!(
            "Unknown crash probe mode. Use attach, save, draft, select, publish, import, rescan, interrupt, or inspect."
        ),
    };
    println!("{ready}");
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    bail!("The crash probe resumed instead of being killed.");
}
