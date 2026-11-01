//! Quire's asset source: Zed's bundled assets, plus the reference app's tag icons (Lucide, ISC;
//! `assets/icons/tags/LICENSE`) under `quire/icons/tags/<tone>.svg` and the page toolbar's save
//! states under `icons/quire/save/`, the header's light and dark icons under
//! `icons/quire/chrome/`, the sidebar's Lucide icons that Zed lacks under `icons/quire/sidebar/`
//! (all under `icons/`, which `ui::Icon::from_path` reads as embedded SVGs), and Quire's themes
//! under `themes/quire/`.

use gpui::{AssetSource, Result, SharedString};
use std::borrow::Cow;

pub struct Assets;

const PREFIX: &str = "quire/icons/tags/";

macro_rules! icons {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../assets/icons/tags/", $name, ".svg")).as_slice())),*]
    };
}

/// Every tone's icon, by tone name.
const TAG_ICONS: &[(&str, &[u8])] = icons!(
    "inbox",
    "next",
    "open",
    "pending",
    "waiting",
    "active",
    "inactive",
    "someday",
    "maybe",
    "done",
    "ignore",
    "blocked",
    "project",
    "meeting",
    "book",
    "article",
    "quote",
    "insult",
    "synthesize",
    "idea",
    "wisdom",
    "question",
    "doc",
    "learning",
);

/// The asset path of the icon drawn before tags of the tone `name`.
pub fn tag_icon_path(name: &str) -> Option<SharedString> {
    TAG_ICONS
        .iter()
        .any(|(icon, _)| *icon == name)
        .then(|| format!("{PREFIX}{name}.svg").into())
}

/// The save indicator's shapes: a circle when saved, a diamond while saving, a warning triangle.
pub const SAVE_ICONS: &[(&str, &[u8])] = &[
    (
        "icons/quire/save/saved.svg",
        include_bytes!("../assets/icons/save/saved.svg"),
    ),
    (
        "icons/quire/save/pending.svg",
        include_bytes!("../assets/icons/save/pending.svg"),
    ),
    (
        "icons/quire/save/failed.svg",
        include_bytes!("../assets/icons/save/failed.svg"),
    ),
];

/// The sidebar's Lucide icons that Zed has no match for (`assets/icons/sidebar/LICENSE`).
pub const SIDEBAR_ICONS: &[(&str, &[u8])] = &[
    (
        "icons/quire/sidebar/book-open.svg",
        include_bytes!("../assets/icons/sidebar/book-open.svg"),
    ),
    (
        "icons/quire/sidebar/calendar-days.svg",
        include_bytes!("../assets/icons/sidebar/calendar-days.svg"),
    ),
    (
        "icons/quire/sidebar/file-plus-corner.svg",
        include_bytes!("../assets/icons/sidebar/file-plus-corner.svg"),
    ),
    (
        "icons/quire/sidebar/layout-template.svg",
        include_bytes!("../assets/icons/sidebar/layout-template.svg"),
    ),
    (
        "icons/quire/sidebar/upload.svg",
        include_bytes!("../assets/icons/sidebar/upload.svg"),
    ),
];

/// Quire Light and Quire Dark, built from the reference app's palettes (`roam-theme.css`, its
/// generated `dark-theme.css`, and `controls.css`). Zed loads every `themes/*.json` asset as a
/// bundled theme family; `preferences` makes them the defaults.
pub const THEMES: &[(&str, &[u8])] = &[(
    "themes/quire/quire.json",
    include_bytes!("../assets/themes/quire.json"),
)];

/// The workspace header's icons Zed lacks (Lucide, ISC; `assets/icons/chrome/LICENSE`): the
/// light and dark appearance toggle.
pub const CHROME_ICONS: &[(&str, &[u8])] = &[
    (
        "icons/quire/chrome/sun.svg",
        include_bytes!("../assets/icons/chrome/sun.svg"),
    ),
    (
        "icons/quire/chrome/moon.svg",
        include_bytes!("../assets/icons/chrome/moon.svg"),
    ),
];

/// Assets served at their own paths: the save, chrome, and sidebar icons and the themes.
fn named() -> impl Iterator<Item = &'static (&'static str, &'static [u8])> {
    SAVE_ICONS
        .iter()
        .chain(CHROME_ICONS)
        .chain(SIDEBAR_ICONS)
        .chain(THEMES)
}

fn named_asset(path: &str) -> Option<&'static [u8]> {
    named().find_map(|(icon, bytes)| (*icon == path).then_some(*bytes))
}

fn tag_icon(path: &str) -> Option<&'static [u8]> {
    let name = path.strip_prefix(PREFIX)?.strip_suffix(".svg")?;
    TAG_ICONS
        .iter()
        .find_map(|(icon, bytes)| (*icon == name).then_some(*bytes))
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match tag_icon(path).or_else(|| named_asset(path)) {
            Some(bytes) => Ok(Some(Cow::Borrowed(bytes))),
            None => assets::Assets.load(path),
        }
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let mut paths = assets::Assets.list(path)?;
        paths.extend(
            TAG_ICONS
                .iter()
                .map(|(name, _)| format!("{PREFIX}{name}.svg"))
                .filter(|icon| icon.starts_with(path))
                .map(SharedString::from),
        );
        paths.extend(
            named()
                .map(|(asset, _)| *asset)
                .filter(|asset| asset.starts_with(path))
                .map(SharedString::new_static),
        );
        Ok(paths)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use quire_core::tag_tones::TONES;

    #[test]
    fn every_tone_has_an_icon_that_loads_as_svg() {
        for tone in TONES {
            let path = tag_icon_path(tone.name).unwrap_or_else(|| panic!("{}", tone.name));
            let bytes = Assets.load(&path).unwrap().unwrap();
            assert!(bytes.starts_with(b"<svg"), "{path}");
            assert!(Assets.list("quire/icons/").unwrap().contains(&path));
        }
        for (path, _) in SAVE_ICONS.iter().chain(CHROME_ICONS).chain(SIDEBAR_ICONS) {
            assert!(
                path.starts_with("icons/"),
                "Icon::from_path draws {path} as an external image, which never shows; put it under icons/"
            );
            assert!(
                Assets.load(path).unwrap().unwrap().starts_with(b"<svg"),
                "{path}"
            );
            let folder = &path[..=path.rfind('/').unwrap()];
            assert!(
                Assets
                    .list(folder)
                    .unwrap()
                    .contains(&SharedString::from(*path))
            );
        }
        assert_eq!(TAG_ICONS.len(), TONES.len());
        assert!(
            Assets
                .load("fonts/lilex/Lilex-Regular.ttf")
                .unwrap()
                .is_some()
        );
    }
}
