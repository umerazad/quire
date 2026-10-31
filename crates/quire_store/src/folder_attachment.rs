//! Attaching a file to a folder notebook, as the reference `Vault.attach` does: the file is copied
//! into the notebook's `attachments` folder as `<prefix>-<file name>`, never replacing a file
//! already there, and synced to disk before its link is inserted.

use crate::durable_files::sync_directory;
use anyhow::{Context as _, Result, bail};
use std::{fs, io, io::Write as _, path::Path};

/// What to attach.
pub enum AttachData<'a> {
    File(&'a Path),
    Bytes(&'a [u8]),
}

/// The reference name: `<prefix>-<name>` with `<>:"/\|?*` replaced by `-`.
pub fn folder_attachment_name(prefix: &str, original: &str) -> String {
    let name: String = original
        .chars()
        .map(|character| {
            if "<>:\"/\\|?*".contains(character) {
                '-'
            } else {
                character
            }
        })
        .collect();
    format!("{prefix}-{name}")
}

/// Copies `data` into `root/attachments` under [`folder_attachment_name`] and returns its
/// notebook path, `attachments/<name>`. Refuses a linked `attachments` folder and an existing
/// file of the same name.
pub fn attach_to_folder(
    root: &Path,
    data: AttachData<'_>,
    original: &str,
    prefix: &str,
) -> Result<String> {
    let folder = root.join("attachments");
    match fs::create_dir(&folder) {
        Ok(()) => drop(sync_directory(root)),
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "Cannot create the attachments folder in {}. Check write access to the notebook folder, then attach again.",
                    root.display()
                )
            });
        }
    }
    if !fs::symlink_metadata(&folder).is_ok_and(|metadata| metadata.is_dir()) {
        bail!(
            "The attachments folder is a link or a file. Replace it with a directory inside the notebook and retry."
        );
    }
    let name = folder_attachment_name(prefix, original);
    let failed = || {
        format!(
            "Cannot copy {original} into the notebook's attachments folder. Check free space and write access, then attach again."
        )
    };
    let mut temporary = tempfile::Builder::new()
        .prefix(".quire-")
        .tempfile_in(&folder)
        .with_context(failed)?;
    match data {
        AttachData::File(source) => {
            if !fs::metadata(source).is_ok_and(|metadata| metadata.is_file()) {
                bail!(
                    "{} is not an ordinary file. Choose a file, not a folder or device.",
                    source.display()
                );
            }
            io::copy(&mut fs::File::open(source)?, temporary.as_file_mut()).with_context(failed)?;
        }
        AttachData::Bytes(bytes) => temporary
            .as_file_mut()
            .write_all(bytes)
            .with_context(failed)?,
    }
    temporary.as_file().sync_all().with_context(failed)?;
    match temporary.persist_noclobber(folder.join(&name)) {
        Ok(_) => {}
        Err(error) if error.error.kind() == io::ErrorKind::AlreadyExists => bail!(
            "attachments/{name} already exists in the notebook. Attach the file again to give it a new name."
        ),
        Err(error) => return Err(error.error).with_context(failed),
    }
    sync_directory(&folder).ok();
    Ok(format!("attachments/{name}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_the_reference() {
        assert_eq!(
            folder_attachment_name("1a2b3c4d", "a<b>c:d\"e|f?g*h\\i.png"),
            "1a2b3c4d-a-b-c-d-e-f-g-h-i.png"
        );
        assert_eq!(
            folder_attachment_name("00000000", "Report 2026.pdf"),
            "00000000-Report 2026.pdf"
        );
    }

    #[test]
    fn files_and_bytes_are_copied_without_replacing_anything() -> Result<()> {
        let root = tempfile::tempdir()?;
        let source = root.path().join("outside.pdf");
        fs::write(&source, b"%PDF")?;
        let notebook = root.path().join("notes");
        fs::create_dir(&notebook)?;
        assert_eq!(
            attach_to_folder(
                &notebook,
                AttachData::File(&source),
                "outside.pdf",
                "aaaaaaaa"
            )?,
            "attachments/aaaaaaaa-outside.pdf"
        );
        assert_eq!(
            fs::read(notebook.join("attachments/aaaaaaaa-outside.pdf"))?,
            b"%PDF"
        );
        assert_eq!(
            attach_to_folder(
                &notebook,
                AttachData::Bytes(b"png"),
                "pasted.png",
                "bbbbbbbb"
            )?,
            "attachments/bbbbbbbb-pasted.png"
        );
        let error = attach_to_folder(
            &notebook,
            AttachData::Bytes(b"other"),
            "pasted.png",
            "bbbbbbbb",
        )
        .unwrap_err()
        .to_string();
        assert_eq!(
            error,
            "attachments/bbbbbbbb-pasted.png already exists in the notebook. Attach the file again to give it a new name."
        );
        assert_eq!(
            fs::read(notebook.join("attachments/bbbbbbbb-pasted.png"))?,
            b"png"
        );
        let mut left: Vec<_> = fs::read_dir(notebook.join("attachments"))?
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            ["aaaaaaaa-outside.pdf", "bbbbbbbb-pasted.png"],
            "no temporary files stay"
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_attachments_folder_is_refused() -> Result<()> {
        let root = tempfile::tempdir()?;
        let notebook = root.path().join("notes");
        let elsewhere = root.path().join("elsewhere");
        fs::create_dir(&notebook)?;
        fs::create_dir(&elsewhere)?;
        std::os::unix::fs::symlink(&elsewhere, notebook.join("attachments"))?;
        let error = attach_to_folder(&notebook, AttachData::Bytes(b"x"), "x.png", "cccccccc")
            .unwrap_err()
            .to_string();
        assert_eq!(
            error,
            "The attachments folder is a link or a file. Replace it with a directory inside the notebook and retry."
        );
        assert_eq!(fs::read_dir(&elsewhere)?.count(), 0);
        Ok(())
    }
}
