use super::NotebookStore;
use crate::{
    compression,
    sync::format::{MAX_BATCH_BYTES, sha256},
};
use anyhow::{Context as _, Result, ensure};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DraftStamp {
    pub session: String,
    pub version: u64,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Draft {
    pub page: String,
    pub path: String,
    pub text: String,
    pub base: Vec<String>,
    pub stamp: DraftStamp,
    pub created: u64,
}

pub(super) fn page_key(page: &str) -> String {
    serde_json::to_string(page).expect("Unicode page identity")
}
pub(super) fn version(stamp: &DraftStamp) -> Result<i64> {
    ensure!(
        !stamp.session.is_empty(),
        "Recovery snapshot has no editor session. Reopen the page before recording recovery."
    );
    i64::try_from(stamp.version).context(
        "Editor version exceeds the storage range. Preserve the buffer and reopen the page.",
    )
}
pub(crate) fn pack(bytes: &[u8]) -> Result<(String, Vec<u8>)> {
    ensure!(
        bytes.len() <= MAX_BATCH_BYTES,
        "Local record exceeds the storage limit. Preserve the buffer and split the page before saving."
    );
    Ok((sha256(bytes), zstd::stream::encode_all(bytes, 0)?))
}
pub(crate) fn unpack(hash: &str, bytes: &[u8]) -> Result<Vec<u8>> {
    let text = compression::decompress(bytes, MAX_BATCH_BYTES).context(
        "Stored notebook record is damaged. Preserve the database and restore a verified backup.",
    )?;
    ensure!(
        sha256(&text) == hash,
        "Stored notebook record failed its hash check. Preserve the database and use a verified backup."
    );
    Ok(text)
}

impl NotebookStore {
    pub fn draft(&self, page: &str) -> Result<Option<Draft>> {
        let row = self
            .connection
            .select_row_bound::<_, (String, i64, String, Vec<u8>)>(
                "SELECT session,version,body_hash,body FROM drafts WHERE page=?",
            )?(page_key(page))?;
        row.map(|(session, version, hash, body)| {
            let draft: Draft = serde_json::from_slice(&unpack(&hash, &body)?)?;
            ensure!(draft.page == page && draft.stamp.session == session && i64::try_from(draft.stamp.version)? == version,
                "Recovery metadata does not match its contents. Preserve the database and use a verified backup.");
            Ok(draft)
        }).transpose()
    }

    /// Returns only after the recovery snapshot is committed; this is not a page save.
    pub fn put_draft(&self, draft: &Draft, expected: Option<&DraftStamp>) -> Result<DraftStamp> {
        let incoming_version = version(&draft.stamp)?;
        let expected_version = expected.map(version).transpose()?;
        ensure!(
            !draft.page.is_empty(),
            "Recovery snapshot has no page identity. Reopen the page and retry."
        );
        self.transaction(|| {
            type Saved = (i64, Option<String>, String);
            let saved = self.connection.select_row_bound::<_, Saved>(
                "SELECT version,revision,base FROM editor_saves WHERE page=? AND session=?",
            )?((page_key(&draft.page), draft.stamp.session.as_str()))?;
            ensure!(saved.as_ref().is_none_or(|(version, _, _)| incoming_version > *version),
                "This recovery snapshot was already saved or superseded. Record the current buffer instead.");
            let mut incoming = Cow::Borrowed(draft);
            if let Some((_, Some(revision), original)) = &saved {
                let original: Vec<String> = serde_json::from_str(original)
                    .context("Saved editor ancestry is damaged. Keep the buffer and restore a verified backup.")?;
                if incoming.base == original && incoming.base != [revision.clone()] {
                    incoming.to_mut().base = vec![revision.clone()];
                }
            }
            let current = self.draft(&draft.page)?;
            if current.as_ref() == Some(incoming.as_ref()) { return Ok(draft.stamp.clone()); }
            let consumed_by_save = current.is_none()
                && expected.is_some_and(|stamp| stamp.session == draft.stamp.session)
                && saved.as_ref().is_some_and(|(saved, _, _)| expected_version.is_some_and(|expected| *saved >= expected));
            ensure!(current.as_ref().map(|d| &d.stamp) == expected || consumed_by_save,
                "Recovery changed while recording this snapshot. Keep the buffer and retry from the latest recovery version.");
            if let Some(current) = current && current.stamp.session == draft.stamp.session {
                ensure!(draft.stamp.version > current.stamp.version,
                    "A stale recovery snapshot cannot replace newer typing. Record the current buffer instead.");
            }
            self.write_draft(incoming.as_ref())?;
            Ok(draft.stamp.clone())
        })
    }

    pub(super) fn write_draft(&self, draft: &Draft) -> Result<()> {
        let (hash, body) = pack(&serde_json::to_vec(draft)?)?;
        self.connection.exec_bound("INSERT INTO drafts(page,session,version,body_hash,body) VALUES (?,?,?,?,?) ON CONFLICT(page) DO UPDATE SET session=excluded.session,version=excluded.version,body_hash=excluded.body_hash,body=excluded.body")?((page_key(&draft.page), draft.stamp.session.as_str(), version(&draft.stamp)?, hash, body))?;
        Ok(())
    }
}
