//! Turning a restored copy of a notebook database into a new notebook, as the reference app's
//! `restoreQuireBackup` does: a new notebook identity so the copy never mixes with the live
//! notebook in a synced folder, the same revision IDs and history, and recovery drafts kept for
//! review rather than settled automatically.

use super::{
    Draft,
    recovery::{pack, unpack},
};
use crate::database;
use anyhow::{Context as _, Result, bail};
use sqlez::connection::Connection;
use std::path::Path;

/// Rewrites the closed database at `path` as notebook `notebook`, written by `device`.
///
/// Publication state belongs to the old identity, so cursors, recorded batch hashes, a pending
/// batch, and observed revisions are cleared, and every revision returns to the outbox in
/// generation order: publishing the new notebook writes its whole history once. Each recovery
/// draft loses its base, so it opens for review and is never merged on its own.
pub(crate) fn adopt_restored(path: &Path, notebook: &str, device: &str) -> Result<()> {
    let connection = database::open_writable(path)?;
    database::immediate(&connection, || rewrite(&connection, notebook, device)).context(
        "Cannot give the restored notebook its new identity. The current notebook is unchanged; check free space where you restore and retry.",
    )
}

fn rewrite(connection: &Connection, notebook: &str, device: &str) -> Result<()> {
    connection
        .exec_bound("UPDATE meta SET notebook=?, device=?, uncertain=0 WHERE singleton=1")?((
        notebook, device,
    ))?;
    for sql in [
        "UPDATE outbox SET batch=NULL",
        "DELETE FROM publication",
        "DELETE FROM cursors",
        "DELETE FROM batch_hashes",
        "DELETE FROM observed_revisions",
        "DELETE FROM outbox",
        "INSERT INTO outbox(revision) SELECT id FROM revisions ORDER BY generation, created, id",
    ] {
        connection.exec(sql)?()?;
    }
    let drafts = connection
        .select::<(String, String, Vec<u8>)>("SELECT page,body_hash,body FROM drafts")?(
    )?;
    for (page, hash, body) in drafts {
        let mut draft: Draft = serde_json::from_slice(&unpack(&hash, &body)?)
            .context("A recovery draft in the backup is damaged. Choose another backup.")?;
        if draft.page.is_empty() {
            bail!("A recovery draft in the backup has no page. Choose another backup.");
        }
        draft.base.clear();
        let (hash, body) = pack(&serde_json::to_vec(&draft)?)?;
        connection.exec_bound("UPDATE drafts SET body_hash=?, body=? WHERE page=?")?((
            hash.as_str(),
            body,
            page.as_str(),
        ))?;
    }
    Ok(())
}
