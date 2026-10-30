use super::{Cursor, NotebookStore, revision::referenced_attachments};
use crate::sync::{
    format::{self, Expected, Kind},
    json::Json,
};
use anyhow::{Context as _, Result, ensure};
use std::collections::BTreeSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingPublication {
    pub seq: u32,
    pub hash: String,
    pub name: String,
    pub data: Vec<u8>,
    pub revisions: Vec<String>,
}

impl NotebookStore {
    pub fn pending_publication(&self) -> Result<Option<PendingPublication>> {
        type Row = (i64, String, Option<String>, String, Vec<u8>, String);
        let row = self.connection.select_row::<Row>(
            "SELECT seq,hash,previous,name,data,revisions FROM publication WHERE singleton=1",
        )?()?;
        row.map(|(seq, hash, previous, name, data, ids)| {
            let decoded = format::decode(&name, &data, Kind::Batch, Expected { notebook: &self.notebook, device: &self.device })?;
            let cursor = self.cursor(&self.device)?;
            ensure!(seq == i64::from(cursor.as_ref().map_or(1, |cursor| cursor.seq + 1)) && previous.as_deref() == cursor.as_ref().map(|cursor| cursor.hash.as_str()),
                "Pending publication no longer follows the recorded cursor. Preserve the database and reconcile the writer's folder.");
            let revisions: Vec<String> = serde_json::from_str(&ids)?;
            let actual: Vec<_> = decoded.value.root().field("revisions").elements().unwrap().map(|revision| revision.field("id").as_str().unwrap().to_owned()).collect();
            let selected: Vec<String> = self.connection.select("SELECT revision FROM outbox WHERE batch=1 ORDER BY position")?()?;
            ensure!(decoded.hash == hash && decoded.value.root().field("seq").as_f64() == Some(seq as f64) && decoded.value.root().field("previous").as_str() == previous.as_deref() && actual == revisions && selected == revisions,
                "Pending publication metadata changed. Preserve the database and reconcile the writer's folder.");
            Ok(PendingPublication { seq: u32::try_from(seq)?, hash, name, data, revisions })
        }).transpose()
    }

    /// Commits the exact bytes and selection before any provider-folder I/O begins.
    pub fn prepare_publication(&self, created: u64) -> Result<Option<PendingPublication>> {
        self.transaction(|| {
            if let Some(pending) = self.pending_publication()? { return Ok(Some(pending)); }
            let certain = self.connection.select_row::<i32>("SELECT uncertain FROM meta WHERE singleton=1")?()? == Some(0);
            ensure!(certain, "The writer's publication state is uncertain. Reconcile its change folder before publishing.");
            let candidates: Vec<String> = self.connection.select("SELECT revision FROM outbox WHERE batch IS NULL ORDER BY position LIMIT 2000")?()?;
            if candidates.is_empty() { return Ok(None); }
            let mut ids = Vec::new();
            let mut records = Vec::new();
            let mut attachments = BTreeSet::new();
            let mut bytes = 0;
            for id in candidates {
                let record = self.get(&id)?.context("An outbox revision is missing. Preserve the database and restore a verified backup.")?;
                let text = record.root().field("text").as_js_string().unwrap();
                let size = text.as_bytes().len();
                if !ids.is_empty() && bytes + size > 4 * 1024 * 1024 { break; }
                self.ensure_attachments(record.root().field("text"))?;
                bytes += size;
                attachments.extend(referenced_attachments(text.as_bytes()));
                ids.push(id);
                records.push(record.canonical()?);
            }
            let cursor = self.cursor(&self.device)?;
            let seq = cursor.as_ref().map_or(1, |cursor| cursor.seq + 1);
            let previous = cursor.map(|cursor| cursor.hash);
            let payload = format!("{{\"notebook\":{},\"device\":{},\"seq\":{seq},\"previous\":{},\"created\":{created},\"revisions\":[{}],\"attachments\":{}}}",
                serde_json::to_string(&self.notebook)?, serde_json::to_string(&self.device)?, serde_json::to_string(&previous)?, records.join(","), serde_json::to_string(&attachments)?);
            let encoded = format::encode(Json::parse(payload.as_bytes())?, Kind::Batch)?;
            self.connection.exec_bound("INSERT INTO publication(singleton,seq,hash,previous,name,data,revisions) VALUES (1,?,?,?,?,?,?)")?((seq, encoded.hash.as_str(), previous.as_deref(), encoded.name.as_str(), encoded.data.as_slice(), serde_json::to_string(&ids)?))?;
            for id in &ids { self.connection.exec_bound("UPDATE outbox SET batch=1 WHERE revision=? AND batch IS NULL")?(id.as_str())?; }
            Ok(Some(PendingPublication { seq, hash: encoded.hash, name: encoded.name, data: encoded.data, revisions: ids }))
        })
    }

    /// Use only after durable folder write or verified adoption of these exact bytes.
    /// This acknowledges publication to the folder, not delivery to another device.
    pub fn complete_publication(&self, expected: &PendingPublication) -> Result<Cursor> {
        self.transaction(|| {
            let actual = self.pending_publication()?.context("The selected publication is no longer pending. Reconcile the writer's folder before completing it.")?;
            ensure!(actual == *expected, "Publication changed while writing its file. Keep the pending revisions and reconcile the writer's folder.");
            self.record_batch_hash(&self.device, expected.seq, &expected.hash)?;
            self.connection.exec_bound("INSERT INTO cursors(device,seq,hash) VALUES (?,?,?) ON CONFLICT(device) DO UPDATE SET seq=excluded.seq,hash=excluded.hash")?((self.device.as_str(), expected.seq, expected.hash.as_str()))?;
            self.connection.exec("DELETE FROM outbox WHERE batch=1")?()?;
            self.connection.exec("DELETE FROM publication WHERE singleton=1")?()?;
            self.connection.exec("UPDATE meta SET uncertain=0 WHERE singleton=1")?()?;
            Ok(Cursor { seq: expected.seq, hash: expected.hash.clone() })
        })
    }

    pub fn publication_uncertain(&self, expected: &PendingPublication) -> Result<()> {
        self.transaction(|| {
            ensure!(self.pending_publication()?.as_ref() == Some(expected), "The failed publication is no longer current. Reconcile the writer's folder before retrying.");
            self.connection.exec("UPDATE meta SET uncertain=1 WHERE singleton=1")?()?;
            Ok(())
        })
    }
}
