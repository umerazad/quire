//! Exporting a whole notebook as a folder of Markdown files. The reference app has no such
//! export, so this follows its page and import rules: each page is written at its notebook path
//! with its stored text, other files keep their relative paths so links still resolve, and the
//! report names the state plain Markdown cannot carry.

use crate::relocate::notebook_path;

/// What a finished notebook export wrote and what it left out.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExportReport {
    pub folder: String,
    pub pages: usize,
    pub files: usize,
    /// Pages whose competing versions were not exported; their shown version was.
    pub competing: Vec<String>,
    /// Pages with an unsaved recovery draft, which stays in the notebook.
    pub drafts: Vec<String>,
    /// Attachments the pages link to that this export could not copy.
    pub uncopied: Vec<String>,
}

/// The export path for a stored page or file path, refusing one that could leave the folder.
pub fn export_target(path: &str) -> Result<String, String> {
    notebook_path(path, false).map_err(|reason| {
        format!("Cannot export {path}: {reason} Rename it in Quire and export again; nothing was written.")
    })
}

fn named(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

fn listed(paths: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut shown = paths
        .iter()
        .take(SHOWN)
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    if paths.len() > SHOWN {
        shown.push_str(&format!(", and {} more", paths.len() - SHOWN));
    }
    shown
}

impl ExportReport {
    /// The report shown when the export finishes.
    pub fn summary(&self) -> String {
        let mut excluded = vec!["page history and earlier versions".to_owned()];
        if !self.competing.is_empty() {
            excluded.push(format!(
                "other competing versions of {} ({}; the shown version was exported)",
                named(self.competing.len(), "page", "pages"),
                listed(&self.competing)
            ));
        }
        if !self.drafts.is_empty() {
            excluded.push(format!(
                "unsaved recovery drafts of {} ({})",
                named(self.drafts.len(), "page", "pages"),
                listed(&self.drafts)
            ));
        }
        if !self.uncopied.is_empty() {
            excluded.push(format!(
                "{} the pages link to ({})",
                named(self.uncopied.len(), "attachment", "attachments"),
                listed(&self.uncopied)
            ));
        }
        excluded.push("sync logs, notebook settings, and this computer's records".to_owned());
        format!(
            "Exported {} and {} to {}. Not exported: {}.",
            named(self.pages, "page", "pages"),
            named(self.files, "other file", "other files"),
            self.folder,
            excluded.join("; ")
        )
    }
}

/// The question asked before an export would replace files already in the folder.
pub fn replace_question(folder: &str, existing: &[String]) -> (String, String) {
    (
        format!(
            "Replace {} in {folder}?",
            named(existing.len(), "file", "files")
        ),
        format!(
            "{} already exist: {}. Replacing them cannot be undone. Choose Cancel to pick an empty folder instead.",
            named(existing.len(), "file", "files"),
            listed(existing)
        ),
    )
}
