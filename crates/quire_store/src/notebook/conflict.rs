use super::{Edit, NotebookStore, SaveAck};
use anyhow::{Context as _, Result, ensure};

/// One competing version of a page that two writers changed from the same version and whose
/// merge failed, so every version is kept until someone chooses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictVersion {
    pub revision: String,
    /// The writer that saved it, which names its computer through that writer's status file.
    pub device: String,
    /// Milliseconds since the epoch.
    pub created: u64,
    pub path: String,
    pub text: String,
    pub deleted: bool,
}

impl NotebookStore {
    /// The competing versions of `page` in the store's head order, the first being the one shown.
    /// Empty when the page has one version.
    pub fn conflict_versions(&self, page: &str) -> Result<Vec<ConflictVersion>> {
        self.transaction_with("BEGIN", || {
            let heads = self.heads(page)?;
            if heads.len() < 2 {
                return Ok(Vec::new());
            }
            heads
                .into_iter()
                .map(|id| {
                    let record = self.get(&id)?.context(
                        "A competing page version is missing. Preserve the database and restore a verified backup.",
                    )?;
                    let root = record.root();
                    let string = |name: &str| {
                        root.field(name).as_str().map(str::to_owned).context(
                            "A competing version holds text this app cannot show. Open the notebook with a compatible Quire version to choose.",
                        )
                    };
                    Ok(ConflictVersion {
                        device: string("device")?,
                        path: string("path")?,
                        text: string("text")?,
                        created: root.field("created").as_f64().unwrap_or(0.0) as u64,
                        deleted: root.field("deleted").as_bool() == Some(true),
                        revision: id,
                    })
                })
                .collect()
        })
    }

    /// Ends a conflict with `text`, saved as following every competing version so each writer
    /// that imports it sees one version again. `heads` are the versions the choice was made
    /// from; when they changed meanwhile nothing is saved. The page keeps the shown version's
    /// path and lives on, even when that version was a deletion.
    pub fn resolve_conflict(
        &self,
        page: &str,
        heads: &[String],
        text: String,
        created: u64,
    ) -> Result<SaveAck> {
        self.transaction(|| {
            let current = self.heads(page)?;
            ensure!(
                current.len() > 1,
                "This page no longer has competing versions. Reload it to see the current version."
            );
            ensure!(
                current == heads,
                "This page's versions changed while you chose. Nothing was saved; review the versions again and choose."
            );
            let records = current
                .iter()
                .map(|id| self.get(id))
                .collect::<Result<Vec<_>>>()?;
            let path_of = |live: bool| {
                records.iter().flatten().find_map(|record| {
                    let root = record.root();
                    (!live || root.field("deleted").as_bool() == Some(false))
                        .then(|| root.field("path").as_str().map(str::to_owned))
                        .flatten()
                })
            };
            let path = path_of(true).or_else(|| path_of(false)).context(
                "A competing version has no readable path. Open the notebook with a compatible Quire version to choose.",
            )?;
            self.save_one(&Edit {
                page: page.to_owned(),
                base: current,
                path,
                text,
                deleted: false,
                created,
                recovery: None,
            })
        })
    }
}
