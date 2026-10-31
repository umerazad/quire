//! Tags with their own look, from the reference `tag-tones.ts`: an ink and background colour for
//! the light and dark appearances. Nested tags such as `#book/fiction` keep their family's tone.

use unicode_normalization::UnicodeNormalization;

/// One tone: its tag name and `0xRRGGBB` ink and background for light, then dark.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tone {
    pub name: &'static str,
    pub light: (u32, u32),
    pub dark: (u32, u32),
}

const fn tone(name: &'static str, light: (u32, u32), dark: (u32, u32)) -> Tone {
    Tone { name, light, dark }
}

pub const TONES: [Tone; 24] = [
    tone("inbox", (0x235b96, 0xe9f2fc), (0x83bdff, 0x1b1d1f)),
    tone("next", (0x176954, 0xe6f4ed), (0x7fcab1, 0x1b1e1c)),
    tone("open", (0x3f6680, 0xe8f0f5), (0x8fb9d6, 0x1d1f20)),
    tone("pending", (0x9a5414, 0xfdeedd), (0xe59960, 0x201c18)),
    tone("waiting", (0x875610, 0xfff3d9), (0xdca668, 0x1c1913)),
    tone("active", (0x1f7a46, 0xe3f5ea), (0x6fc48b, 0x1a1f1c)),
    tone("inactive", (0x626b76, 0xeff1f3), (0xa4adb9, 0x1c1d1d)),
    tone("someday", (0x69518c, 0xf1ecf8), (0xbfa6e8, 0x1f1e21)),
    tone("maybe", (0x5d5f8f, 0xeeeff8), (0xa8abe0, 0x1d1d1f)),
    tone("done", (0x3d7357, 0xeaf3eb), (0x86be9f, 0x1c1e1c)),
    tone("ignore", (0x757b84, 0xf1f2f4), (0x9ba2ab, 0x1b1b1c)),
    tone("blocked", (0xa14343, 0xfbecec), (0xf58e8a, 0x201c1c)),
    tone("project", (0x485c94, 0xedf0fc), (0x9ab3f1, 0x1c1d20)),
    tone("meeting", (0x765184, 0xf5ecf7), (0xc9a1d9, 0x1f1d1f)),
    tone("book", (0x7a5232, 0xf6ede4), (0xd9ac89, 0x201e1c)),
    tone("article", (0x4a5d73, 0xebeff4), (0xa4b9d2, 0x1e1f20)),
    tone("quote", (0x8a3f62, 0xf9e9f0), (0xed98bc, 0x221e1f)),
    tone("insult", (0xb03a2e, 0xfdebe7), (0xfb8877, 0x211d1c)),
    tone("synthesize", (0x6b46c1, 0xf0eafd), (0xb3a0fc, 0x201f23)),
    tone("idea", (0x886817, 0xfaf4dc), (0xc7a55b, 0x1c1a14)),
    tone("wisdom", (0x0e6470, 0xe0f2f3), (0x7cc7d4, 0x1c2020)),
    tone("question", (0xa8326a, 0xfbe8f1), (0xfd7fb4, 0x221d20)),
    tone("doc", (0x0b6890, 0xe1f1f8), (0x70beea, 0x1c2022)),
    tone("learning", (0x4b7314, 0xedf5e1), (0x94c067, 0x1b1d18)),
];

/// The look of every other tag: the reference's neutral label.
pub const NEUTRAL: Tone = tone("neutral", (0x42647d, 0xeef1f4), (0x94b9d5, 0x1c1d1d));

/// The tone of a tag written as `#name`, `#[[name]]`, or `name`, by its family before any `/`.
pub fn tone_index(tag: &str) -> Option<usize> {
    let bare = tag.strip_prefix('#').unwrap_or(tag);
    let bare = bare
        .strip_prefix("[[")
        .and_then(|inner| inner.strip_suffix("]]"))
        .unwrap_or(bare);
    let name: String = bare.nfc().collect::<String>().to_lowercase();
    let family = name.split('/').next().unwrap_or_default();
    let family = if family == "synthesise" {
        "synthesize"
    } else {
        family
    };
    TONES.iter().position(|tone| tone.name == family)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_tone_matches_its_own_name_and_family() {
        for (index, tone) in TONES.iter().enumerate() {
            assert_eq!(tone_index(&format!("#{}", tone.name)), Some(index));
        }
        assert_eq!(tone_index("#Book/Fiction"), tone_index("book"));
        assert_eq!(tone_index("#[[waiting]]"), tone_index("waiting"));
        assert_eq!(tone_index("synthesise"), tone_index("synthesize"));
        assert_eq!(tone_index("#constructor"), None);
        assert_eq!(tone_index("#books"), None);
        for added in ["question", "doc", "learning"] {
            assert!(tone_index(added).is_some(), "{added}");
        }
    }
}
