//! Quire's asset source: Zed's bundled assets, plus the reference app's tag icons (Lucide, ISC;
//! `assets/icons/tags/LICENSE`) under `quire/icons/tags/<tone>.svg` and the page toolbar's save
//! states under `quire/icons/save/`.

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
        "quire/icons/save/saved.svg",
        include_bytes!("../assets/icons/save/saved.svg"),
    ),
    (
        "quire/icons/save/pending.svg",
        include_bytes!("../assets/icons/save/pending.svg"),
    ),
    (
        "quire/icons/save/failed.svg",
        include_bytes!("../assets/icons/save/failed.svg"),
    ),
];

/// The workspace header's icons Zed lacks (Lucide, ISC; `assets/icons/chrome/LICENSE`): the
/// light and dark appearance toggle.
pub const CHROME_ICONS: &[(&str, &[u8])] = &[
    (
        "quire/icons/chrome/sun.svg",
        include_bytes!("../assets/icons/chrome/sun.svg"),
    ),
    (
        "quire/icons/chrome/moon.svg",
        include_bytes!("../assets/icons/chrome/moon.svg"),
    ),
];

fn save_icon(path: &str) -> Option<&'static [u8]> {
    SAVE_ICONS
        .iter()
        .chain(CHROME_ICONS)
        .find_map(|(icon, bytes)| (*icon == path).then_some(*bytes))
}

fn tag_icon(path: &str) -> Option<&'static [u8]> {
    let name = path.strip_prefix(PREFIX)?.strip_suffix(".svg")?;
    TAG_ICONS
        .iter()
        .find_map(|(icon, bytes)| (*icon == name).then_some(*bytes))
}

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        match tag_icon(path).or_else(|| save_icon(path)) {
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
            SAVE_ICONS
                .iter()
                .chain(CHROME_ICONS)
                .map(|(icon, _)| *icon)
                .filter(|icon| icon.starts_with(path))
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
        for (path, _) in SAVE_ICONS.iter().chain(CHROME_ICONS) {
            assert!(
                Assets.load(path).unwrap().unwrap().starts_with(b"<svg"),
                "{path}"
            );
            assert!(
                Assets
                    .list("quire/icons/")
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
