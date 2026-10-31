//! Deleting, renaming, and moving pages and folders. A deletion plans what goes and how many
//! links break, then writes every tombstone or none, or in a folder notebook moves the files to
//! the system Trash. A rename or move, of one page or of a folder with every page inside, rewrites
//! every link that follows and writes every page or none, journaled in a folder notebook. All go
//! through `page_writes`.

use crate::{
    notebook_backend::NotebookBackend,
    notebook_session::OpenPage,
    page_writes::{self, PageSource, PageWrite},
    query_index::ask,
};
use gpui::{App, AppContext as _, Entity, Task};
use quire_core::{
    markdown::{copy_with_new_ids, page_name, page_title, safe_page_path},
    relocate::{
        MISSING_SOURCE, Relocation, folder_path, folder_relocations, notebook_path, relocate_pages,
        rename_target,
    },
    source_text::SourceText,
};
use quire_store::{
    index::{DeletionTarget, IndexClient},
    sync::format::page_id_for_path,
};
use std::collections::{BTreeSet, HashMap, HashSet};

pub const STALE_DELETION: &str =
    "A page changed after you chose to delete it. Nothing was deleted; try again.";

/// Paths listed by name in a folder deletion before "And N more."
pub const LISTED_PAGES: usize = 20;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    Page(String),
    Folder(String),
}

pub struct DeletionPlan {
    pub target: Target,
    /// The page's title, for a page.
    pub title: String,
    /// Each page that goes, by identity and path, ordered by path.
    pub pages: Vec<(String, String)>,
    pub broken_links: usize,
    /// Files that are not pages inside a folder going to the Trash with it.
    pub other_files: usize,
    /// Whether the files go to the system Trash (a folder notebook) rather than every computer
    /// removing them (a Quire notebook).
    pub trash: bool,
    sources: Vec<PageSource>,
}

fn count(number: usize, one: &str, many: &str) -> String {
    match number {
        1 => format!("1 {one}"),
        number => format!("{number} {many}"),
    }
}

impl DeletionPlan {
    pub fn folder(&self) -> Option<&str> {
        match &self.target {
            Target::Folder(folder) => Some(folder),
            Target::Page(_) => None,
        }
    }

    pub fn heading(&self) -> String {
        match self.folder() {
            Some(folder) => format!(
                "Delete {folder} with its {}{}?",
                count(self.pages.len(), "page", "pages"),
                match self.other_files {
                    0 => String::new(),
                    others => format!(" and {}", count(others, "other file", "other files")),
                }
            ),
            None => format!("Delete {}?", self.title),
        }
    }

    pub fn detail(&self) -> String {
        let links = match self.broken_links {
            0 => "No other page links here.".to_owned(),
            links => format!(
                "{} from other pages will point to a missing page.",
                count(links, "link", "links")
            ),
        };
        if self.trash {
            format!("{links} The files go to the system Trash, where you can restore them.")
        } else {
            format!("{links} Every computer on this notebook removes it too.")
        }
    }

    /// The paths a folder deletion names, and how many more it does not.
    pub fn listed(&self) -> (Vec<&str>, usize) {
        if self.folder().is_none() {
            return (Vec::new(), 0);
        }
        let shown = self
            .pages
            .iter()
            .take(LISTED_PAGES)
            .map(|(_, path)| path.as_str())
            .collect();
        (shown, self.pages.len().saturating_sub(LISTED_PAGES))
    }
}

/// Reads what a deletion takes from the index and each page's current text, open or stored.
pub fn plan_deletion(
    target: Target,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<DeletionPlan, String>> {
    cx.spawn(async move |cx| {
        let query = match &target {
            Target::Page(page) => DeletionTarget::Page(page.as_str().into()),
            Target::Folder(folder) => DeletionTarget::Folder(folder.clone()),
        };
        let deletion = ask(index.deletion(query)).await?;
        let title = deletion
            .pages
            .first()
            .map(|page| page.title.clone())
            .unwrap_or_default();
        let mut pages = Vec::new();
        let mut sources = Vec::new();
        for listed in deletion.pages {
            let id = listed.page.to_lossy_string();
            if let Some(source) = page_writes::read(id.clone(), &open_pages, &storage, cx).await? {
                pages.push((id, source.path.clone()));
                sources.push(source);
            }
        }
        if pages.is_empty() {
            return Err(match target {
                Target::Page(_) => {
                    "This page has no saved version to delete. Close its tab instead.".to_owned()
                }
                Target::Folder(folder) => {
                    format!("{folder} has no pages to delete. Choose another folder.")
                }
            });
        }
        let other_files = match (&target, storage.folder()) {
            (Target::Folder(folder), Some(client)) => {
                ask(client.other_files(folder.clone())).await?.len()
            }
            _ => 0,
        };
        Ok(DeletionPlan {
            trash: storage.folder().is_some(),
            target,
            title,
            pages,
            broken_links: deletion.broken_links,
            other_files,
            sources,
        })
    })
}

/// Deletes every planned page or none. Returns the deleted page identities.
pub fn delete(
    plan: DeletionPlan,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<Vec<String>, String>> {
    let folder = plan.folder().map(str::to_owned);
    let writes = plan
        .sources
        .into_iter()
        .map(|source| {
            let text = source.stored.as_str().into();
            source.write(text)
        })
        .collect();
    let ids: Vec<String> = plan.pages.into_iter().map(|(id, _)| id).collect();
    let work = page_writes::delete(writes, folder, storage, STALE_DELETION, cx);
    cx.spawn(async move |_| work.await.map(|_| ids))
}

pub const STALE_RENAME: &str =
    "A page changed while it was being renamed. Nothing was renamed; rename it again.";
pub const STALE_MOVE: &str =
    "A page changed after the preview. Nothing was moved; preview the move again.";
const NO_GRAMMAR: &str = "Cannot read links because the Markdown grammar is unavailable. Rebuild Quire with its bundled grammars.";

/// Where a renamed or moved page now lives and the title the page list will show for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Renamed {
    /// The page's identity before the rename. A folder notebook's page identity follows its
    /// path, so there it differs from `page`.
    pub previous: String,
    pub page: String,
    pub path: String,
    pub title: String,
}

/// What to relocate: a page renamed in its folder, a page moved to a path, or a folder moved
/// with every page inside it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Relocate {
    Rename { page: String, name: String },
    Move { page: String, path: String },
    Folder { folder: String, path: String },
}

/// One link that changes, in the page that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkUpdate {
    pub path: String,
    pub before: String,
    pub after: String,
}

/// A planned rename or move: the writes of every moved page and every page whose links follow,
/// with what the preview lists.
pub struct RelocationPlan {
    pub source: String,
    pub target: String,
    pub moved: Vec<Renamed>,
    /// Pages that stay where they are but get link updates, by path.
    pub updated: Vec<String>,
    pub updates: Vec<LinkUpdate>,
    trash: bool,
    writes: Vec<PageWrite>,
}

impl RelocationPlan {
    pub fn heading(&self) -> String {
        format!("{} → {}", self.source, self.target)
    }

    pub fn detail(&self) -> String {
        let pages = match self.updated.len() {
            0 => "No other page needs link updates.".to_owned(),
            1 => "1 other page needs link updates.".to_owned(),
            count => format!("{count} other pages need link updates."),
        };
        let moved = match self.moved.len() {
            1 => String::new(),
            count => format!("{count} pages move. "),
        };
        let kept = if self.trash {
            "The original files and saved history are retained for recovery."
        } else {
            "Pages keep their block IDs and saved history."
        };
        format!("{moved}{pages} {kept}")
    }
}

/// Reads the pages a relocation touches, rewrites their links, and returns the writes. None when
/// a rename keeps the name.
pub fn plan_relocation(
    request: Relocate,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<Option<RelocationPlan>, String>> {
    cx.spawn(async move |cx| {
        let (relocations, moved_pages, source, target) = match &request {
            Relocate::Rename { page, name } | Relocate::Move { page, path: name } => {
                let Some(source) =
                    page_writes::read(page.clone(), &open_pages, &storage, cx).await?
                else {
                    return Err(MISSING_SOURCE.into());
                };
                let (rename, target) = match &request {
                    Relocate::Rename { .. } => match rename_target(&source.path, name)? {
                        Some(target) => (true, target),
                        None => return Ok(None),
                    },
                    _ => (false, notebook_path(name, true)?),
                };
                let Some(facts) = ask(index.relocation(page.as_str().into())).await? else {
                    return Err(MISSING_SOURCE.into());
                };
                let taken = ask(index.path_taken(target.clone())).await?;
                let relocation = Relocation::new(
                    &source.path,
                    &target,
                    rename,
                    &facts.page.title,
                    facts.unique_title,
                    true,
                    |_| taken,
                )?;
                let target = relocation.target.clone();
                (vec![relocation], vec![page.clone()], source.path, target)
            }
            Relocate::Folder { folder, path } => {
                let listed = ask(index.pages()).await?;
                let titles: Vec<(String, String)> = listed
                    .iter()
                    .map(|listed| (listed.page.path.clone(), listed.page.title.clone()))
                    .collect();
                let taken: HashSet<String> =
                    titles.iter().map(|(path, _)| path.to_lowercase()).collect();
                let relocations = folder_relocations(folder, path, &titles, |path| {
                    taken.contains(&path.to_lowercase())
                })?;
                let pages = relocations
                    .iter()
                    .filter_map(|relocation| {
                        listed
                            .iter()
                            .find(|listed| listed.page.path == relocation.source)
                            .map(|listed| listed.page.page.to_lossy_string())
                    })
                    .collect();
                let target = folder_path(path)?;
                (relocations, pages, folder.clone(), target)
            }
        };
        let mut candidates: Vec<String> = moved_pages.clone();
        for page in &moved_pages {
            let Some(facts) = ask(index.relocation(page.as_str().into())).await? else {
                return Err(MISSING_SOURCE.into());
            };
            for candidate in facts.candidates {
                let id = candidate.page.to_lossy_string();
                if !candidates.contains(&id) {
                    candidates.push(id);
                }
            }
        }
        let mut analyzed = Vec::new();
        for id in candidates {
            let Some(source) = page_writes::read(id.clone(), &open_pages, &storage, cx).await?
            else {
                if moved_pages.contains(&id) {
                    return Err(MISSING_SOURCE.into());
                }
                continue;
            };
            let (original, normalized) = SourceText::prepare(source.stored.clone());
            let work = cx.update(|cx| crate::page_index::relocation_links(normalized.clone(), cx));
            let Some((references, urls)) = cx.background_spawn(work).await else {
                return Err(NO_GRAMMAR.into());
            };
            analyzed.push((source, original, normalized, references, urls));
        }
        let bases: Vec<String> = analyzed
            .iter()
            .flat_map(|(_, _, _, references, _)| references.iter())
            .map(|reference| {
                reference
                    .target
                    .split('#')
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let (_, resolved) = ask(index.with_generation(move |index| {
            bases
                .into_iter()
                .map(|base| Ok((base.clone(), index.find_page(&base)?.map(|page| page.path))))
                .collect::<Result<HashMap<_, _>, _>>()
        }))
        .await?;
        let folder = storage.folder().is_some();
        let mut plan = RelocationPlan {
            source,
            target,
            moved: Vec::new(),
            updated: Vec::new(),
            updates: Vec::new(),
            trash: folder,
            writes: Vec::new(),
        };
        for (source, original, normalized, references, urls) in analyzed {
            let Some(planned) = relocate_pages(
                &relocations,
                &source.path,
                &normalized,
                &references,
                &urls,
                |base| resolved.get(base).cloned().flatten(),
            ) else {
                continue;
            };
            let text = original.apply(planned.edits).map_err(|error| {
                format!(
                    "Could not rewrite links in {}: {error}. Preview the change again.",
                    source.path
                )
            })?;
            plan.updates
                .extend(planned.changes.into_iter().map(|change| LinkUpdate {
                    path: source.path.clone(),
                    before: change.before,
                    after: change.after,
                }));
            if moved_pages.contains(&source.page) {
                plan.moved.push(Renamed {
                    previous: source.page.clone(),
                    page: if folder {
                        page_id_for_path(&planned.path)
                    } else {
                        source.page.clone()
                    },
                    path: planned.path.clone(),
                    title: page_title(&planned.path, &text).to_lossy_string(),
                });
            } else {
                plan.updated.push(source.path.clone());
            }
            plan.writes.push(source.write_at(planned.path, text));
        }
        if plan.moved.len() != moved_pages.len() {
            return Err(MISSING_SOURCE.into());
        }
        plan.updated.sort();
        plan.updates.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(Some(plan))
    })
}

/// Plans renaming `page` to `name` in its folder, as the reference `renamePage` does: the file
/// name and title change together, every link to the page follows, and a front matter `title`
/// takes the new name. None when the name is unchanged or empty.
pub fn plan_rename(
    page: String,
    name: String,
    open_pages: Vec<Entity<OpenPage>>,
    index: IndexClient,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<Option<RelocationPlan>, String>> {
    plan_relocation(
        Relocate::Rename { page, name },
        open_pages,
        index,
        storage,
        cx,
    )
}

/// Writes the relocation and every link change or none, refusing with `stale` when a page
/// changed since planning. Returns how many pages were written.
pub fn relocate(
    plan: RelocationPlan,
    storage: NotebookBackend,
    stale: &'static str,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    page_writes::apply(plan.writes, storage, stale, cx)
}

/// Writes a rename and every link change or none.
pub fn rename(
    plan: RelocationPlan,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<usize, String>> {
    relocate(plan, storage, STALE_RENAME, cx)
}

pub const STALE_RESTORE: &str = "The page changed after this version was compared with it. Nothing was restored; review the changes shown now and restore again.";

/// Makes `text`, an earlier saved version, the page's newest version in one checked save, when
/// the page still reads `compared`, the text the preview showed it against. An open page's
/// unsaved typing is saved first, so it stays in history, and the page then shows the restored
/// text. Returns whether anything was written.
pub fn restore_version(
    page: String,
    text: String,
    compared: String,
    open_pages: Vec<Entity<OpenPage>>,
    storage: NotebookBackend,
    cx: &mut App,
) -> Task<Result<bool, String>> {
    cx.spawn(async move |cx| {
        let Some(source) = page_writes::read(page, &open_pages, &storage, cx).await? else {
            return Err(
                "This page has no saved version to restore over. Restore it as a new page instead."
                    .into(),
            );
        };
        if source.stored != compared {
            return Err(STALE_RESTORE.into());
        }
        if source.stored == text {
            return Ok(false);
        }
        let write = source.write(text.into());
        let applied = cx.update(|cx| page_writes::apply(vec![write], storage, STALE_RESTORE, cx));
        applied.await.map(|_| true)
    })
}

/// The title and text of a copy of `text`, a version of the page at `path`, saved as a new page
/// with fresh block IDs, as the reference `createSavedCopy` names it: the page title cut to 50
/// characters, then `label`, the date, and a short random suffix.
pub fn page_copy(
    path: &str,
    text: &str,
    label: &str,
    date: &str,
) -> Result<(String, String), String> {
    let source = page_title(path, text).to_lossy_string();
    let short: String = uuid::Uuid::new_v4()
        .simple()
        .to_string()
        .chars()
        .take(8)
        .collect();
    let title = format!(
        "{} ({label} {date} {short})",
        source.chars().take(50).collect::<String>()
    );
    let destination = safe_page_path(&title).map_err(|error| error.to_string())?;
    let copied = copy_with_new_ids(text, &destination, &[page_name(path), &source], || {
        format!("b-{}", uuid::Uuid::new_v4().simple())
    });
    Ok((title, copied))
}
