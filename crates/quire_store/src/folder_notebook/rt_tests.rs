//! Compatibility with folders the reference app already uses: its `.quire` settings, drafts,
//! history, Trash deletion, and journaled renames.

use super::tests::{entries, fixture};
use std::fs;

/// `.quire/notebook.json` exactly as the reference `writeNotebookSettings` writes it:
/// `JSON.stringify({ version: 1, daily: layout }, null, 2) + "\n"`.
const RT_SETTINGS: &str = "{\n  \"version\": 1,\n  \"daily\": {\n    \"folder\": \"Journal\",\n    \"title\": \"{DD}.{MM}.{YYYY}\"\n  }\n}\n";

#[test]
fn reference_settings_are_read_and_replaced_only_from_the_version_read() {
    let opened = fixture(&[("Note.md", "- a\n"), (".quire/notebook.json", RT_SETTINGS)]);
    let settings = opened.notebook.settings().unwrap().unwrap();
    assert_eq!(settings.text, RT_SETTINGS);

    let newer = RT_SETTINGS.replace("Journal", "Daily");
    let stale = opened
        .notebook
        .save_settings(&newer, Some("not-the-revision"))
        .unwrap_err();
    assert!(format!("{stale:#}").contains("changed in another application"));
    assert_eq!(
        fs::read_to_string(opened.folder.join(".quire/notebook.json")).unwrap(),
        RT_SETTINGS
    );

    let revision = opened
        .notebook
        .save_settings(&newer, Some(&settings.revision))
        .unwrap();
    assert_eq!(
        fs::read_to_string(opened.folder.join(".quire/notebook.json")).unwrap(),
        newer
    );
    assert_eq!(
        opened.notebook.settings().unwrap().unwrap().revision,
        revision
    );
    assert_eq!(
        entries(&opened.folder.join(".quire")),
        ["notebook.json"],
        "the temporary file is renamed into place"
    );
    assert!(
        opened
            .notebook
            .save_settings("not json", Some(&revision))
            .is_err()
    );
}

#[test]
fn settings_create_the_metadata_folder_only_when_saved_and_refuse_links() {
    let opened = fixture(&[("Note.md", "- a\n")]);
    assert_eq!(opened.notebook.settings().unwrap(), None);
    assert!(!opened.folder.join(".quire").exists());
    assert!(
        opened
            .notebook
            .save_settings(RT_SETTINGS, Some("anything"))
            .is_err(),
        "a base for a missing file is stale"
    );
    opened.notebook.save_settings(RT_SETTINGS, None).unwrap();
    assert_eq!(
        fs::read_to_string(opened.folder.join(".quire/notebook.json")).unwrap(),
        RT_SETTINGS
    );

    #[cfg(unix)]
    {
        let linked = fixture(&[("Note.md", "- a\n")]);
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), linked.folder.join(".quire")).unwrap();
        assert!(linked.notebook.settings().is_err());
        assert!(linked.notebook.save_settings(RT_SETTINGS, None).is_err());
        assert!(entries(elsewhere.path()).is_empty());
    }
}
