//! The example notebook a first launch opens when no profile has a notebook yet, as the
//! reference app does: a folder of three sample pages in Quire's data folder, apart from the
//! person's own files.

use super::folder::FolderOptions;
use gpui::App;
use std::{
    io::Write as _,
    path::{Path, PathBuf},
};

/// The example notebook's folder inside Quire's data folder.
pub(in crate::editor_host) const FOLDER: &str = "Example notebook";
/// The page the example notebook opens on.
pub(in crate::editor_host) const FIRST_PAGE: &str = "A place to think.md";

/// The reference app's sample pages, file name and text.
pub(in crate::editor_host) const PAGES: [(&str, &str); 3] = [
    (
        FIRST_PAGE,
        "A notebook for ideas worth returning to. Everything here is a Markdown file on your computer.\n\n## Follow a thought\n\n- Start with a thought, then give it room to grow.\n  - Press **Enter** for another block.\n  - Use **Tab** and **Shift + Tab** to change its place in the outline.\n- Connect a page with [[Field notes]], or type `[[` to find one.\n- A reference follows the original thought: ((b-seed))\n\n## One thought, many places\n\nThe block below lives in **Field notes**. Click inside it to edit the original, right here.\n\n{{embed: ((b-seed))}}\n\n## Make it yours\n\n- Open a folder from the sidebar to use your own Markdown notes.\n- Switch to **Source** whenever you want the whole page as plain Markdown.\n- Use **⌘ / Ctrl + K** to find a page or a thought.\n\nThis example notebook is separate from your own files.\n",
    ),
    (
        "Field notes.md",
        "Small observations, loosely held.\n\n- The best notes leave room for the next thought. ^b-seed\n  - A block can travel without losing its connections.\n  - An embed is another window onto the same idea.\n- Keep the raw material close.\n  - A conversation, a question, a line from a book.\n- Return to [[A place to think]] when you need a fresh start.\n\n## An open question\n\nWhat would make it easier to pick up a thought where you left it?\n",
    ),
    (
        "Reading room.md",
        "A small shelf for things to come back to.\n\n- [ ] Read slowly enough to notice a question.\n- [ ] Connect one idea to [[Field notes]].\n- [ ] Write a note in your own words.\n\n> We write to find out what we think.\n\n## On the desk\n\n| A thought | A next step |\n| --- | --- |\n| A useful question | Follow it for a page |\n| An unfinished idea | Leave a trail for tomorrow |\n",
    ),
];

/// Makes the example notebook in `data`, writing only sample pages that are missing, and
/// returns its folder. A page that exists, edited or not, is never replaced.
pub(in crate::editor_host) fn ensure(data: &Path) -> Result<PathBuf, String> {
    let root = data.join(FOLDER);
    std::fs::create_dir_all(&root).map_err(|error| {
        format!(
            "Cannot make the example notebook folder {}: {error}. Check that Quire's data folder is writable, then reopen Quire.",
            root.display()
        )
    })?;
    PAGES.iter().try_for_each(|(name, text)| {
        let path = root.join(name);
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .and_then(|mut file| file.write_all(text.as_bytes()).and_then(|()| file.sync_all()));
        match written {
            Err(error) if error.kind() != std::io::ErrorKind::AlreadyExists => Err(format!(
                "Cannot write the example page {}: {error}. Check that Quire's data folder is writable, then reopen Quire.",
                path.display()
            )),
            _ => Ok(()),
        }
    })?;
    Ok(root)
}

/// Opens the example notebook in `data` on its first page, making it first. Opening it makes it
/// the active profile's notebook, so later launches return to it until another is chosen.
pub(in crate::editor_host) fn launch(data: &Path, cx: &mut App) -> Result<(), String> {
    let root = ensure(data)?;
    let canonical = std::fs::canonicalize(&root).map_err(|error| {
        format!(
            "Cannot open the example notebook {}: {error}. Check that Quire's data folder is readable, then reopen Quire.",
            root.display()
        )
    })?;
    let state = quire_store::folder_notebook::state_folder_in(data, &canonical);
    FolderOptions::new(canonical, Vec::new(), Some(state))
        .on_page(FIRST_PAGE)
        .open(cx)
}
