//! The storage behind an open notebook. Sessions, page persistence, and multi-page writes ask the
//! same page operations of either backend: a Quire notebook's store, which merges a save made
//! from an older version, or a folder notebook's files, which refuse it with a
//! [`FolderConflict`] carrying the disk version.

use quire_store::{
    folder_notebook::{FolderClient, FolderConflict},
    notebook::{CheckedSaves, Draft, DraftStamp, Edit, PageCheckout, SaveAck, SavedVersion},
    sync::json::Json,
    worker::{QueueError, Reply, StorageClient},
};

type Answer<T> = Result<Reply<T>, QueueError>;

#[derive(Clone)]
pub enum NotebookBackend {
    Store(StorageClient),
    Folder(FolderClient),
}

impl From<StorageClient> for NotebookBackend {
    fn from(client: StorageClient) -> Self {
        Self::Store(client)
    }
}

impl From<FolderClient> for NotebookBackend {
    fn from(client: FolderClient) -> Self {
        Self::Folder(client)
    }
}

impl NotebookBackend {
    pub fn folder(&self) -> Option<&FolderClient> {
        match self {
            Self::Folder(client) => Some(client),
            Self::Store(_) => None,
        }
    }

    /// Stops admitting work; accepted requests still finish.
    #[cfg(test)]
    pub fn close(&self) -> Result<(), QueueError> {
        match self {
            Self::Store(client) => client.close(),
            Self::Folder(client) => client.close(),
        }
    }

    pub fn checkout(&self, page: String, selected: Option<String>) -> Answer<PageCheckout> {
        match self {
            Self::Store(client) => client.checkout(page, selected),
            Self::Folder(client) => client.checkout(page, selected),
        }
    }

    pub fn heads(&self, page: String) -> Answer<Vec<String>> {
        match self {
            Self::Store(client) => client.heads(page),
            Self::Folder(client) => client.heads(page),
        }
    }

    pub fn get(&self, id: String) -> Answer<Option<Json>> {
        match self {
            Self::Store(client) => client.get(id),
            Self::Folder(client) => client.get(id),
        }
    }

    pub fn put_draft(&self, draft: Draft, expected: Option<DraftStamp>) -> Answer<DraftStamp> {
        match self {
            Self::Store(client) => client.put_draft(draft, expected),
            Self::Folder(client) => client.put_draft(draft, expected),
        }
    }

    pub fn save(&self, edit: Edit) -> Answer<SaveAck> {
        match self {
            Self::Store(client) => client.save(edit),
            Self::Folder(client) => client.save(edit),
        }
    }

    pub fn save_many_if_heads(&self, edits: Vec<Edit>) -> Answer<CheckedSaves> {
        match self {
            Self::Store(client) => client.save_many_if_heads(edits),
            Self::Folder(client) => client.save_many_if_heads(edits),
        }
    }

    /// The page's earlier saved versions, newest first.
    pub fn history(&self, page: String) -> Answer<Vec<SavedVersion>> {
        match self {
            Self::Store(client) => client.history(page),
            Self::Folder(client) => client.history(page),
        }
    }

    pub fn version_text(&self, page: String, id: String) -> Answer<String> {
        match self {
            Self::Store(client) => client.version_text(page, id),
            Self::Folder(client) => client.version_text(page, id),
        }
    }
}

/// The folder conflict behind a failed save, when that is why it failed.
pub fn folder_conflict(error: &anyhow::Error) -> Option<&FolderConflict> {
    error.downcast_ref::<FolderConflict>()
}
