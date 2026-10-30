use anyhow::{Context as _, Result, bail, ensure};
use quire_store::notebook::{Edit, NotebookStore, RescanLimits, RescanWait};
use quire_store::sync::format::{self, Expected, Kind};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::{BTreeSet, HashMap},
    fs::{self, File, OpenOptions},
    io::{BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
};

#[derive(Deserialize)]
struct Request {
    op: String,
    #[serde(default)]
    peer: String,
    #[serde(default)]
    notebook: String,
    #[serde(default)]
    device: String,
    #[serde(default)]
    id: String,
    #[serde(default)]
    page: String,
    #[serde(default)]
    pages: Vec<String>,
    #[serde(default)]
    now: u64,
    edit: Option<Edit>,
    #[serde(default)]
    data: Vec<u8>,
    #[serde(default)]
    original: String,
}

struct Peer {
    store: NotebookStore,
    directory: PathBuf,
}

impl Peer {
    fn scan(&self, name: &str) -> Result<Value> {
        let mut continuation = None;
        let mut pages = BTreeSet::new();
        let mut applied = 0;
        let mut batches = 0;
        let mut problems = Vec::new();
        let mut waiting = Vec::new();
        for _ in 0..1024 {
            let report = self.store.rescan(
                &self.directory.join(name),
                RescanLimits::default(),
                continuation.as_ref(),
            )?;
            applied += report.applied;
            batches += report.batches;
            for page in report.pages {
                pages.insert(
                    page.page
                        .as_str()
                        .context("Schedule page identity is not valid UTF-8.")?
                        .to_owned(),
                );
            }
            problems.extend(report.problems.into_iter().map(|problem| json!({"device":problem.device.unwrap_or_default(),"file":problem.file,"reason":problem.reason,"newerFormat":problem.newer_format})));
            waiting.extend(report.waiting.into_iter().map(|wait| {
                let device = match &wait {
                    RescanWait::Sequence { device, .. }
                    | RescanWait::File { device, .. }
                    | RescanWait::Parents { device, .. } => device.as_str(),
                    RescanWait::Attachment { .. } => "",
                };
                json!({"device":device,"reason":format!("{wait:?}")})
            }));
            continuation = report.continuation;
            if continuation.is_none() {
                return Ok(
                    json!({"applied":applied,"batches":batches,"pages":pages,"problems":problems,"waiting":waiting,"missingAttachments":self.store.pending_attachments(None,2000)?}),
                );
            }
        }
        bail!("Schedule scan exceeded 1024 continuations. Retain the fixture and replay this seed.")
    }
}

fn execute(root: &Path, peers: &mut HashMap<String, Peer>, request: Request) -> Result<Value> {
    match request.op.as_str() {
        "reset" => {
            peers.clear();
            fs::remove_dir_all(root)?;
            fs::create_dir(root)?;
            return Ok(Value::Null);
        }
        "open" => {
            ensure!(
                !request.peer.is_empty()
                    && request
                        .peer
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
                "Invalid schedule peer key. Use ASCII letters, digits, or hyphens."
            );
            ensure!(
                !peers.contains_key(&request.peer),
                "Schedule peer is already open. Reset the fixture first."
            );
            let directory = root.join(&request.peer);
            fs::create_dir(&directory)?;
            for name in ["incoming", "outgoing"] {
                let folder = directory.join(name);
                fs::create_dir(&folder)?;
                fs::write(
                    folder.join("quire-notebook.json"),
                    serde_json::to_vec(&json!({"format":1,"id":request.notebook,"created":1}))?,
                )?;
            }
            let store = NotebookStore::open(
                &directory.join("local.sqlite"),
                &request.notebook,
                &request.device,
            )?;
            let result = json!({"directory":directory});
            peers.insert(request.peer, Peer { store, directory });
            return Ok(result);
        }
        "rebuild" | "reopen" => {
            let peer = peers
                .get(&request.peer)
                .context("Schedule peer is missing. Open it before rebuilding.")?;
            ensure!(
                request.op == "reopen"
                    || (peer.store.outbox()?.is_empty()
                        && peer.store.pending_publication()?.is_none()),
                "Schedule rebuild would discard local saves. Publish them before rebuilding."
            );
            let peer = peers.remove(&request.peer).unwrap();
            let notebook = peer.store.notebook_id().to_owned();
            let device = peer.store.device_id().to_owned();
            let directory = peer.directory;
            drop(peer.store);
            for name in if request.op == "rebuild" {
                &["local.sqlite", "local.sqlite-wal", "local.sqlite-shm"][..]
            } else {
                &[]
            } {
                match fs::remove_file(directory.join(name)) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            let store = NotebookStore::open(&directory.join("local.sqlite"), &notebook, &device)?;
            peers.insert(request.peer, Peer { store, directory });
            return Ok(Value::Null);
        }
        _ => {}
    }
    let peer = peers
        .get(&request.peer)
        .context("Schedule peer is missing. Open it before sending operations.")?;
    let store = &peer.store;
    Ok(match request.op.as_str() {
        "get" => json!(
            store
                .get(&request.id)?
                .map(|revision| revision.canonical())
                .transpose()?
        ),
        "heads" => json!(store.heads(&request.page)?),
        "outbox" => json!(store.outbox()?),
        "attach" => json!(store.attach(&request.data, &request.original)?),
        "pages" => json!(
            request
                .pages
                .into_iter()
                .filter_map(|page| match store.heads(&page) {
                    Ok(heads) if !heads.is_empty() => Some(Ok(page)),
                    Ok(_) => None,
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>>>()?
        ),
        "save" => {
            let edit = request
                .edit
                .context("Schedule save has no edit. Include a complete Edit value.")?;
            let result = store.save(&edit)?;
            json!({"revision":result.revision.canonical()?,"heads":result.heads,"conflict":result.conflict,"saved":result.changed,"replaced":result.replaced})
        }
        "select" => {
            let report = peer.scan("outgoing")?;
            ensure!(
                report["problems"].as_array().unwrap().is_empty(),
                "Own folder reconciliation failed: {report}. Retain the fixture and replay this seed."
            );
            let pending = store.prepare_publication(request.now)?;
            let pending = if let Some(pending) = pending {
                let batch = format::decode(
                    &pending.name,
                    &pending.data,
                    Kind::Batch,
                    Expected {
                        notebook: store.notebook_id(),
                        device: store.device_id(),
                    },
                )?;
                let attachments = batch.value.root().field("attachments").elements().unwrap().map(|name| {
                    let name = name.as_str().unwrap();
                    let data = fs::read(peer.directory.join("attachments").join(name))?;
                    ensure!(format::attachment_hash(name) == Some(format::sha256(&data).as_str()), "Schedule attachment content changed. Retain the fixture and inspect its cache.");
                    Ok(json!({"name":name,"data":data}))
                }).collect::<Result<Vec<_>>>()?;
                json!({"name":pending.name,"data":pending.data,"revisions":pending.revisions.len(),"attachments":attachments})
            } else {
                Value::Null
            };
            json!({"pending":pending,"pages":report["pages"]})
        }
        "uncertain" => {
            let pending = store.pending_publication()?.context("Schedule publication is no longer pending. Inspect its delivered file before retrying.")?;
            store.publication_uncertain(&pending)?;
            Value::Null
        }
        "complete" => {
            let pending = store.pending_publication()?.context("Schedule publication is no longer pending. Inspect its delivered file before retrying.")?;
            let file = peer
                .directory
                .join("outgoing/devices")
                .join(store.device_id())
                .join("log")
                .join(&pending.name);
            ensure!(
                fs::read(file)? == pending.data,
                "Selected schedule bytes were not delivered. Retain the fixture and replay this seed."
            );
            let ack = store
                .publish_to(&peer.directory.join("outgoing"), request.now)?
                .context("Schedule publisher did not acknowledge the selected file.")?;
            ensure!(
                ack.name == pending.name,
                "Schedule publication selection changed. Retain the fixture and replay this seed."
            );
            json!({"revisions":ack.revisions})
        }
        "import" => peer.scan("incoming")?,
        _ => bail!(
            "Unknown schedule operation {}. Use the documented replica probe commands.",
            request.op
        ),
    })
}

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().collect();
    ensure!(
        args.len() == 4,
        "Specify request FIFO, reply FIFO, and a fresh fixture root."
    );
    let input = File::open(&args[1])?;
    let mut output = OpenOptions::new().write(true).open(&args[2])?;
    let root = Path::new(&args[3]).join("peers");
    fs::create_dir(&root)
        .context("Cannot claim the schedule peers directory. Use a fresh fixture root.")?;
    let mut peers = HashMap::new();
    for line in BufReader::new(input).lines() {
        let line = line?;
        let request = serde_json::from_str::<Request>(&line);
        if request.as_ref().is_ok_and(|request| request.op == "quit") {
            writeln!(output, "{}", json!({"result":null}))?;
            output.flush()?;
            break;
        }
        let result = request
            .map_err(anyhow::Error::from)
            .and_then(|request| execute(&root, &mut peers, request));
        let reply = match result {
            Ok(result) => json!({"result":result}),
            Err(error) => json!({"error":format!("{error:#}")}),
        };
        writeln!(output, "{reply}")?;
        output.flush()?;
    }
    Ok(())
}
