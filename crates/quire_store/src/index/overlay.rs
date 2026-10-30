//! Open pages shadow their committed rows with live editor analysis. Overlay rows live in TEMP
//! tables on the index connection, so they are bounded by open pages and vanish with the worker.
//! Queries read `live_*` views: a page's committed rows drop out while it has an overlay.

use super::{PageIndex, QueryIndex, strip_markdown};
use crate::sync::json::JsString;
use anyhow::Result;
use sqlez::connection::Connection;

pub(super) const TABLES: &[&str] = &[
    "CREATE TEMP TABLE overlay_pages (revision TEXT PRIMARY KEY, page TEXT NOT NULL UNIQUE, path TEXT NOT NULL, name_key TEXT NOT NULL, title TEXT NOT NULL, title_key TEXT NOT NULL, date TEXT, deleted INTEGER NOT NULL, analyzed INTEGER NOT NULL)",
    "CREATE INDEX temp.overlay_pages_by_name ON overlay_pages(name_key)",
    "CREATE INDEX temp.overlay_pages_by_title ON overlay_pages(title_key)",
    "CREATE INDEX temp.overlay_pages_by_date ON overlay_pages(date)",
    "CREATE TEMP TABLE overlay_blocks (revision TEXT NOT NULL, ordinal INTEGER NOT NULL, key TEXT NOT NULL, id TEXT, start INTEGER NOT NULL, finish INTEGER NOT NULL, content_from INTEGER NOT NULL, content_to INTEGER NOT NULL, subtree_to INTEGER NOT NULL, depth INTEGER NOT NULL, kind TEXT NOT NULL, parent TEXT, text TEXT NOT NULL, content TEXT NOT NULL, UNIQUE(revision, ordinal))",
    "CREATE INDEX temp.overlay_blocks_by_id ON overlay_blocks(id)",
    "CREATE TEMP TABLE overlay_links (revision TEXT NOT NULL, ordinal INTEGER NOT NULL, kind TEXT NOT NULL, syntax TEXT NOT NULL, target TEXT NOT NULL, label TEXT, media TEXT, start INTEGER NOT NULL, finish INTEGER NOT NULL, text TEXT NOT NULL, page_key TEXT, block_key TEXT, date TEXT, UNIQUE(revision, ordinal))",
    "CREATE INDEX temp.overlay_links_by_page ON overlay_links(page_key)",
    "CREATE INDEX temp.overlay_links_by_block ON overlay_links(block_key)",
    "CREATE INDEX temp.overlay_links_by_date ON overlay_links(date)",
    "CREATE TEMP TABLE overlay_tagged (revision TEXT NOT NULL, ordinal INTEGER NOT NULL, tag TEXT NOT NULL, tag_key TEXT NOT NULL, start INTEGER NOT NULL, block_id TEXT, snippet TEXT NOT NULL, UNIQUE(revision, ordinal))",
    "CREATE INDEX temp.overlay_tagged_by_key ON overlay_tagged(tag_key)",
    "CREATE TEMP TABLE overlay_front_tags (revision TEXT NOT NULL, ordinal INTEGER NOT NULL, tag TEXT NOT NULL, UNIQUE(revision, ordinal))",
    "CREATE TEMP TABLE overlay_tasks (revision TEXT NOT NULL, ordinal INTEGER NOT NULL, key TEXT NOT NULL, id TEXT, start INTEGER NOT NULL, raw TEXT NOT NULL, text TEXT NOT NULL, state TEXT NOT NULL, tags TEXT NOT NULL, links TEXT NOT NULL, dates TEXT NOT NULL, completed TEXT, created TEXT, due TEXT, UNIQUE(revision, ordinal))",
    "CREATE TEMP VIEW live_tasks AS
       SELECT revision,ordinal,key,id,start,raw,text,state,tags,links,dates,completed,created,due FROM main.tasks
       UNION ALL SELECT revision,ordinal,key,id,start,raw,text,state,tags,links,dates,completed,created,due FROM temp.overlay_tasks",
    "CREATE TEMP VIEW live_pages AS
       SELECT revision,page,path,name_key,title,title_key,date,deleted,analyzed FROM main.pages
         WHERE page NOT IN (SELECT page FROM temp.overlay_pages)
       UNION ALL SELECT revision,page,path,name_key,title,title_key,date,deleted,analyzed FROM temp.overlay_pages",
    "CREATE TEMP VIEW live_blocks AS
       SELECT revision,ordinal,key,id,start,finish,content_from,content_to,subtree_to,depth,kind,parent,text,content FROM main.blocks
       UNION ALL SELECT revision,ordinal,key,id,start,finish,content_from,content_to,subtree_to,depth,kind,parent,text,content FROM temp.overlay_blocks",
    "CREATE TEMP VIEW live_links AS
       SELECT revision,ordinal,kind,syntax,target,label,media,start,finish,text,page_key,block_key,date FROM main.links
       UNION ALL SELECT revision,ordinal,kind,syntax,target,label,media,start,finish,text,page_key,block_key,date FROM temp.overlay_links",
    "CREATE TEMP VIEW live_tagged AS
       SELECT revision,ordinal,tag,tag_key,start,block_id,snippet FROM main.tagged
       UNION ALL SELECT revision,ordinal,tag,tag_key,start,block_id,snippet FROM temp.overlay_tagged",
    "CREATE TEMP VIEW live_front_tags AS
       SELECT revision,ordinal,tag FROM main.front_tags
       UNION ALL SELECT revision,ordinal,tag FROM temp.overlay_front_tags",
];

pub(super) fn create(connection: &Connection) -> Result<()> {
    for table in TABLES {
        connection.exec(table)?()?;
    }
    Ok(())
}

/// The live state of an open page: its current path, title, and editor analysis.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overlay {
    pub path: String,
    pub title: String,
    pub index: PageIndex,
}

impl QueryIndex {
    /// Replaces `page`'s overlay. Opening a page, every analyzed edit, and renames call this.
    pub fn set_overlay(&self, page: &JsString, overlay: &Overlay) -> Result<()> {
        let page = page.to_json();
        self.releasing.borrow_mut().remove(&page);
        self.transaction(|| {
            self.remove_overlay(&page)?;
            let revision = format!("overlay:{page}");
            self.connection.exec_bound(
                "INSERT INTO temp.overlay_pages(revision,page,path,name_key,title,title_key,date,deleted,analyzed) VALUES (?,?,?,?,?,?,?,0,1)",
            )?((
                revision.as_str(),
                page.as_str(),
                overlay.path.as_str(),
                strip_markdown(&overlay.path).to_lowercase(),
                overlay.title.as_str(),
                overlay.title.to_lowercase(),
                self.date(&overlay.title),
            ))?;
            self.insert_rows("temp.overlay_", &revision, &overlay.index)
        })?;
        self.bump();
        Ok(())
    }

    /// Drops `page`'s overlay once the index holds analysis for its final committed head, so a
    /// closed page never vanishes from queries while its last save is still being analyzed.
    pub fn release_overlay(&self, page: &JsString) -> Result<()> {
        self.releasing.borrow_mut().insert(page.to_json());
        self.refresh()?;
        self.settle_releases()
    }

    pub(super) fn settle_releases(&self) -> Result<()> {
        let pending: Vec<String> = self.releasing.borrow().iter().cloned().collect();
        for page in pending {
            let waiting: Option<i64> = self.connection.select_row_bound(
                "SELECT count(*) FROM main.pages WHERE page=? AND analyzed=0 AND deleted=0",
            )?(page.as_str())?;
            if waiting == Some(0) {
                self.transaction(|| self.remove_overlay(&page))?;
                self.releasing.borrow_mut().remove(&page);
                self.bump();
            }
        }
        Ok(())
    }

    fn remove_overlay(&self, page: &str) -> Result<()> {
        let revision = format!("overlay:{page}");
        for table in [
            "overlay_pages",
            "overlay_blocks",
            "overlay_links",
            "overlay_tagged",
            "overlay_front_tags",
            "overlay_tasks",
        ] {
            self.connection
                .exec_bound(&format!("DELETE FROM temp.{table} WHERE revision=?"))?(
                revision.as_str(),
            )?;
        }
        Ok(())
    }

    /// Changes whenever committed rows, analysis, or overlays change. Results computed under an
    /// older generation are stale.
    pub fn generation(&self) -> u64 {
        self.generation.get()
    }

    pub(super) fn bump(&self) {
        self.generation.set(self.generation.get() + 1);
    }
}
