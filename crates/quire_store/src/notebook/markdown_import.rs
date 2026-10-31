use super::{CheckedSaves, Edit, NotebookStore};
use crate::{
    markdown_import::{PagesImported, SourcePage},
    sync::format::{page_id_for_path, sha256},
};
use anyhow::{Result, bail};
use quire_core::exchange::{
    file_links::{linked_files, rewrite_file_links},
    import::{path_key, plan_import},
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs,
    path::{Path, PathBuf},
};

/// The other files of a source folder, which an import into a Quire notebook copies as
/// attachments when an imported page links to them.
#[derive(Clone, Debug, Default)]
pub struct SourceFiles {
    pub root: PathBuf,
    /// Ordinary files relative to `root`, as `read_source` lists them.
    pub files: Vec<String>,
}

/// What an import with attachments wrote.
#[derive(Debug)]
pub struct FolderImported {
    pub pages: PagesImported,
    /// Source files copied as attachments, in name order.
    pub attached: Vec<String>,
}

impl NotebookStore {
    /// Creates every source page whose path no live page uses, ignoring case, in one checked
    /// save: all of them or none. Pages already here are left unchanged and reported as skipped.
    /// Refuses the whole import when a source path cannot be a notebook path.
    pub fn import_pages(&self, pages: &[SourcePage], created: u64) -> Result<PagesImported> {
        Ok(self
            .import_folder(pages, &SourceFiles::default(), created)?
            .pages)
    }

    /// [`Self::import_pages`], also copying each file of `source` that an imported page links to
    /// into the attachment cache under its content name, and pointing those links at
    /// `attachments/<name>`. When anything fails, the attachments this import added are removed
    /// again, so the notebook is left as it was.
    pub fn import_folder(
        &self,
        pages: &[SourcePage],
        source: &SourceFiles,
        created: u64,
    ) -> Result<FolderImported> {
        let paths: Vec<String> = pages.iter().map(|page| page.path.clone()).collect();
        let mut live = self.connection.select_bound::<String, i64>(
            "SELECT count(*) FROM heads h JOIN revisions r ON r.id=h.revision WHERE r.deleted=0 AND r.path_key=?",
        )?;
        let mut failure = None;
        let plan = plan_import(&paths, |key| {
            live(key.to_owned()).map_or_else(
                |error| {
                    failure.get_or_insert(error);
                    true
                },
                |rows| rows.first().is_some_and(|count| *count > 0),
            )
        })?;
        if let Some(error) = failure {
            return Err(error);
        }
        let texts: HashMap<&str, &str> = pages
            .iter()
            .map(|page| (page.path.as_str(), page.text.as_str()))
            .collect();
        let files: BTreeSet<&str> = source.files.iter().map(String::as_str).collect();
        let linked: BTreeSet<String> = plan
            .pages
            .iter()
            .flat_map(|(path, _)| linked_files(texts[path.as_str()], path))
            .filter(|file| files.contains(file.as_str()))
            .collect();
        let mut added = Vec::new();
        let result = self
            .attach_sources(&source.root, &linked, &mut added)
            .and_then(|references| {
                let edits = plan
                    .pages
                    .iter()
                    .map(|(source_path, path)| {
                        Ok(Edit {
                            page: self.import_page_id(path, created)?,
                            base: Vec::new(),
                            path: path.clone(),
                            text: rewrite_file_links(
                                texts[source_path.as_str()],
                                source_path,
                                |file| references.get(file).cloned(),
                            ),
                            deleted: false,
                            created,
                            recovery: None,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?;
                match self.save_many_if_heads(&edits)? {
                    CheckedSaves::Saved(saved) => Ok(FolderImported {
                        pages: PagesImported {
                            saved,
                            skipped: plan.skipped.clone(),
                        },
                        attached: references.into_keys().collect(),
                    }),
                    CheckedSaves::HeadsChanged { .. } | CheckedSaves::Appeared { .. } => bail!(
                        "Another change created one of the imported pages first. Nothing was imported; import again to skip the pages already here."
                    ),
                }
            });
        if result.is_err() && !added.is_empty() {
            self.forget_attachments(&added)?;
        }
        result
    }

    /// The ID for an imported page at `path`: the one its path derives, unless a deleted page at
    /// that path already has history under it.
    fn import_page_id(&self, path: &str, created: u64) -> Result<String> {
        let derived = page_id_for_path(path);
        Ok(if self.heads(&derived)?.is_empty() {
            derived
        } else {
            let seed = format!("{}\0{}\0{created}", path_key(path), self.device_id());
            format!("p-{}", &sha256(seed)[..32])
        })
    }

    /// Streams each file into the attachment cache, returning its link target by source path.
    /// Content names the store did not know before are pushed to `added`.
    fn attach_sources(
        &self,
        root: &Path,
        files: &BTreeSet<String>,
        added: &mut Vec<String>,
    ) -> Result<BTreeMap<String, String>> {
        files
            .iter()
            .map(|file| {
                let path = root.join(file);
                if !fs::symlink_metadata(&path).is_ok_and(|metadata| metadata.is_file()) {
                    bail!(
                        "Cannot copy {file}: it is no longer an ordinary file in the source folder. Restore it, then import again; nothing was imported."
                    );
                }
                let original = file.rsplit('/').next().unwrap_or(file);
                let (reference, new) = self.attach_file_reporting(&path, original)?;
                if new {
                    added.push(reference.trim_start_matches("attachments/").to_owned());
                }
                Ok((file.clone(), reference))
            })
            .collect()
    }
}

/// The queue bytes an import of `pages` reserves.
pub(crate) fn pages_size(pages: &[SourcePage]) -> usize {
    pages
        .iter()
        .map(|page| 256 + page.path.len() + page.text.len())
        .sum()
}

impl crate::worker::Client<NotebookStore> {
    pub fn import_pages(
        &self,
        pages: Vec<SourcePage>,
        created: u64,
    ) -> std::result::Result<crate::worker::Reply<PagesImported>, crate::worker::QueueError> {
        self.request(pages_size(&pages), move |store| {
            store.import_pages(&pages, created)
        })
    }

    pub fn import_folder(
        &self,
        pages: Vec<SourcePage>,
        source: SourceFiles,
        created: u64,
    ) -> std::result::Result<crate::worker::Reply<FolderImported>, crate::worker::QueueError> {
        self.request(pages_size(&pages), move |store| {
            store.import_folder(&pages, &source, created)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page(path: &str, text: &str) -> SourcePage {
        SourcePage {
            path: path.into(),
            text: text.into(),
        }
    }

    fn cached(store: &NotebookStore) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(store.directory.join("attachments"))
            .map(|entries| {
                entries
                    .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    #[test]
    fn a_refused_page_write_removes_only_the_attachments_it_added() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        fs::create_dir_all(source.join("img"))?;
        fs::write(source.join("img/new.png"), b"new picture")?;
        fs::write(source.join("old.png"), b"old picture")?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let old = store.attach(b"old picture", "old.png")?;
        let files = SourceFiles {
            root: source.clone(),
            files: vec!["img/new.png".into(), "old.png".into()],
        };
        let pages = [page("A.md", "- ![](img/new.png) ![](old.png)\n")];
        store.connection.exec("CREATE TEMP TRIGGER refuse_import BEFORE INSERT ON revisions BEGIN SELECT RAISE(ABORT,'injected save failure'); END")?()?;
        let error = store.import_folder(&pages, &files, 1).unwrap_err();
        assert!(
            format!("{error:#}").contains("injected save failure"),
            "{error:#}"
        );
        assert_eq!(cached(&store), [old.trim_start_matches("attachments/")]);
        let catalog: i64 = store
            .connection
            .select_row("SELECT count(*) FROM attachments")?()?
        .unwrap();
        assert_eq!(catalog, 1, "the attachment that was already here stays");

        store.connection.exec("DROP TRIGGER refuse_import")?()?;
        let imported = store.import_folder(&pages, &files, 2)?;
        assert_eq!(imported.attached, ["img/new.png", "old.png"]);
        assert_eq!(cached(&store).len(), 2);
        Ok(())
    }

    #[test]
    fn a_source_file_replaced_by_a_link_refuses_the_import() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("source");
        fs::create_dir_all(&source)?;
        fs::write(source.join("a.png"), b"a")?;
        fs::write(root.path().join("secret.png"), b"secret")?;
        #[cfg(unix)]
        std::os::unix::fs::symlink(root.path().join("secret.png"), source.join("b.png"))?;
        let store = NotebookStore::open(&root.path().join("local.sqlite"), "nb", "office")?;
        let files = SourceFiles {
            root: source,
            files: vec!["a.png".into(), "b.png".into()],
        };
        let error = store
            .import_folder(&[page("A.md", "![](a.png) ![](b.png)")], &files, 1)
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "Cannot copy b.png: it is no longer an ordinary file in the source folder. Restore it, then import again; nothing was imported."
        );
        assert!(cached(&store).is_empty(), "a.png was removed again");
        assert!(
            store
                .checkout(&page_id_for_path("A.md"), None)?
                .selected
                .is_none()
        );
        Ok(())
    }
}
