//! What a folder notebook holds, counted and hashed for a migration report: each note and other
//! file by path with the SHA-256 of its bytes, and the drafts and saved versions Quire keeps for
//! it in its state folder.

use super::{FolderNotebook, files::list};
use crate::sync::format::sha256;
use anyhow::{Context as _, Result};
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderCensus {
    pub pages: BTreeMap<String, String>,
    /// Ordinary files that are not notes, such as attachments.
    pub others: BTreeMap<String, String>,
    /// Each draft's path and the editor session that recorded it; drafts imported from the
    /// reference app have a `reference:` session.
    pub drafts: Vec<(String, String)>,
    pub history: usize,
}

/// Every note and other file below `root` (dot entries skipped, as the notebook lists them) with
/// the SHA-256 of its bytes.
pub fn hashed_listing(
    root: &std::path::Path,
) -> Result<(BTreeMap<String, String>, BTreeMap<String, String>)> {
    let listing = list(root);
    let hash = |path: &String| -> Result<(String, String)> {
        let bytes = std::fs::read(root.join(path)).with_context(|| {
            format!("Cannot read {path} to verify it. Check that it is readable, then retry.")
        })?;
        Ok((path.clone(), sha256(&bytes)))
    };
    Ok((
        listing
            .files
            .iter()
            .map(|file| hash(&file.path))
            .collect::<Result<_>>()?,
        listing.others.iter().map(hash).collect::<Result<_>>()?,
    ))
}

impl FolderNotebook {
    pub fn census(&self) -> Result<FolderCensus> {
        let (pages, others) = hashed_listing(&self.root)?;
        let count = |sql: &str| -> Result<usize> {
            Ok(self.connection.select_row::<i64>(sql)?()?.unwrap_or(0) as usize)
        };
        Ok(FolderCensus {
            pages,
            others,
            drafts: self
                .connection
                .select::<(String, String)>("SELECT path,session FROM drafts ORDER BY path")?(
            )?,
            history: count("SELECT COUNT(*) FROM history")?,
        })
    }
}
