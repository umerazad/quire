//! The verification report a migration shows: per notebook, what `rt` holds beside what Quire
//! now holds, counted and hashed, with what remains for the user to do.

use super::inventory::Item;
use crate::sync::format::sha256;
use std::{collections::BTreeMap, fmt::Write as _, path::PathBuf};

/// One comparison between `rt`'s state and Quire's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    pub what: String,
    pub rt: String,
    pub quire: String,
    pub ok: bool,
}

impl Check {
    pub fn new(what: &str, rt: impl Into<String>, quire: impl Into<String>, ok: bool) -> Self {
        Self {
            what: what.into(),
            rt: rt.into(),
            quire: quire.into(),
            ok,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NotebookKind {
    Folder,
    Quire,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotebookReport {
    pub kind: NotebookKind,
    /// The notebook as `rt` names it.
    pub source: PathBuf,
    /// Where Quire keeps it: the state folder or the database.
    pub destination: PathBuf,
    pub checks: Vec<Check>,
    pub notes: Vec<String>,
}

impl NotebookReport {
    pub fn verified(&self) -> bool {
        self.checks.iter().all(|check| check.ok)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub source: PathBuf,
    pub inventory: Vec<Item>,
    pub notebooks: Vec<NotebookReport>,
    pub profiles: Vec<String>,
    pub problems: Vec<String>,
}

impl Report {
    pub fn verified(&self) -> bool {
        self.problems.is_empty() && self.notebooks.iter().all(NotebookReport::verified)
    }

    pub fn to_markdown(&self) -> String {
        let mut out = String::new();
        let _ = writeln!(out, "# Migration from rt\n");
        let _ = writeln!(out, "Source: `{}`\n", self.source.display());
        let _ = writeln!(
            out,
            "{}\n",
            if self.verified() {
                "Every check passed. rt's files were only read; rt can be used again at any time."
            } else {
                "Some checks need attention; see below. rt's files were only read; rt can be used again at any time."
            }
        );
        for notebook in &self.notebooks {
            let kind = match notebook.kind {
                NotebookKind::Folder => "Folder notebook",
                NotebookKind::Quire => "Quire notebook",
            };
            let _ = writeln!(
                out,
                "## {kind} {}\n\nQuire keeps it at `{}`.\n",
                notebook.source.display(),
                notebook.destination.display()
            );
            let _ = writeln!(
                out,
                "| Check | rt | Quire | Result |\n| --- | --- | --- | --- |"
            );
            for check in &notebook.checks {
                let _ = writeln!(
                    out,
                    "| {} | {} | {} | {} |",
                    check.what,
                    check.rt,
                    check.quire,
                    if check.ok { "ok" } else { "check" }
                );
            }
            for note in &notebook.notes {
                let _ = writeln!(out, "\n{note}");
            }
            let _ = writeln!(out);
        }
        if !self.profiles.is_empty() {
            let _ = writeln!(out, "## Profiles\n");
            for line in &self.profiles {
                let _ = writeln!(out, "- {line}");
            }
            let _ = writeln!(out);
        }
        if !self.problems.is_empty() {
            let _ = writeln!(out, "## Problems\n");
            for problem in &self.problems {
                let _ = writeln!(out, "- {problem}");
            }
            let _ = writeln!(out);
        }
        let _ = writeln!(
            out,
            "## Everything rt keeps\n\n| What | Where | Format | Quire |\n| --- | --- | --- | --- |"
        );
        for item in &self.inventory {
            let _ = writeln!(
                out,
                "| {} | `{}` | {} | {}: {} |",
                item.what,
                item.path.display(),
                item.format,
                item.disposition.label(),
                item.note
            );
        }
        out
    }
}

/// A count and a short hash over `path<TAB>hash` lines in path order, so two sides compare in
/// one cell.
pub(crate) fn summary(entries: &BTreeMap<String, String>) -> String {
    let lines: String = entries
        .iter()
        .map(|(path, hash)| format!("{path}\t{hash}\n"))
        .collect();
    format!("{} · {}", entries.len(), &sha256(lines)[..12])
}
