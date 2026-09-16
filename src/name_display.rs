//! Runtime-only game and folder name presentation.
//!
//! Rows keep their complete effective names. These choices are applied only
//! when names are ordered, searched or drawn, so changing one never rewrites a
//! cache, a gamelist, an Artwork Pack or a path used to launch a game.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum GameNameDisplay {
    #[default]
    Full,
    RemoveParentheses,
    RemoveBrackets,
    RemoveParenthesesAndBrackets,
    KeepRegion,
    KeepDiscIndex,
    KeepRegionAndDiscIndex,
}

impl GameNameDisplay {
    pub const ALL: [Self; 7] = [
        Self::Full,
        Self::RemoveParentheses,
        Self::RemoveBrackets,
        Self::RemoveParenthesesAndBrackets,
        Self::KeepRegion,
        Self::KeepDiscIndex,
        Self::KeepRegionAndDiscIndex,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Full => "Full",
            Self::RemoveParentheses => "Remove Parentheses",
            Self::RemoveBrackets => "Remove Brackets",
            Self::RemoveParenthesesAndBrackets => "Remove Parentheses and Brackets",
            Self::KeepRegion => "Keep Region",
            Self::KeepDiscIndex => "Keep Disc Index",
            Self::KeepRegionAndDiscIndex => "Keep Region and Disc Index",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .expect("every game-name mode is listed")
    }

    /// The visible form of one complete effective row name.
    ///
    /// The six shortened modes follow RetroArch's playlist label-display
    /// semantics: flat parenthesised and bracketed blocks are removed, while
    /// the Keep modes retain recognised No-Intro regions and/or disc markers.
    pub fn apply(self, name: &str) -> Cow<'_, str> {
        let remove = match self {
            Self::Full => return Cow::Borrowed(name),
            Self::RemoveParentheses => Remove::Parentheses,
            Self::RemoveBrackets => Remove::Brackets,
            Self::RemoveParenthesesAndBrackets => Remove::Both,
            Self::KeepRegion => Remove::ExceptRegion,
            Self::KeepDiscIndex => Remove::ExceptDisc,
            Self::KeepRegionAndDiscIndex => Remove::ExceptRegionOrDisc,
        };
        Cow::Owned(sanitise(name, remove))
    }
}

#[derive(Clone, Copy)]
enum Remove {
    Parentheses,
    Brackets,
    Both,
    ExceptRegion,
    ExceptDisc,
    ExceptRegionOrDisc,
}

const REGIONS: [&str; 20] = [
    "(Australia)",
    "(Brazil)",
    "(Canada)",
    "(China)",
    "(France)",
    "(Germany)",
    "(Hong Kong)",
    "(Italy)",
    "(Japan)",
    "(Korea)",
    "(Netherlands)",
    "(Spain)",
    "(Sweden)",
    "(USA)",
    "(World)",
    "(Europe)",
    "(Asia)",
    "(Japan, USA)",
    "(Japan, Europe)",
    "(USA, Europe)",
];

const DISC_MARKERS: [&str; 5] = ["(CD", "(Disc", "(Disk", "(Side", "(Tape"];

fn starts_with_ignoring_ascii_case(text: &str, prefix: &str) -> bool {
    text.get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
}

fn recognised(text: &str, choices: &[&str]) -> bool {
    choices
        .iter()
        .any(|choice| starts_with_ignoring_ascii_case(text, choice))
}

fn removes_at(text: &str, remove: Remove) -> bool {
    let Some(first) = text.as_bytes().first().copied() else {
        return false;
    };
    match remove {
        Remove::Parentheses => first == b'(',
        Remove::Brackets => first == b'[',
        Remove::Both => matches!(first, b'(' | b'['),
        Remove::ExceptRegion => matches!(first, b'(' | b'[') && !recognised(text, &REGIONS),
        Remove::ExceptDisc => matches!(first, b'(' | b'[') && !recognised(text, &DISC_MARKERS),
        Remove::ExceptRegionOrDisc => {
            matches!(first, b'(' | b'[')
                && !recognised(text, &REGIONS)
                && !recognised(text, &DISC_MARKERS)
        }
    }
}

fn closes(ch: char, remove: Remove) -> bool {
    match remove {
        Remove::Parentheses => ch == ')',
        Remove::Brackets => ch == ']',
        Remove::Both | Remove::ExceptRegion | Remove::ExceptDisc | Remove::ExceptRegionOrDisc => {
            matches!(ch, ')' | ']')
        }
    }
}

fn sanitise(name: &str, remove: Remove) -> String {
    let mut shown = String::with_capacity(name.len());
    let mut copying = true;
    let mut at = 0;
    while at < name.len() {
        let remainder = &name[at..];
        let ch = remainder
            .chars()
            .next()
            .expect("a valid string has a character before its end");
        if copying {
            if removes_at(remainder, remove) {
                copying = false;
            } else if ch != ' ' || (!shown.is_empty() && !shown.ends_with(' ')) {
                shown.push(ch);
            }
        } else if closes(ch, remove) {
            copying = true;
        }
        at += ch.len_utf8();
    }
    if shown.ends_with(' ') {
        shown.pop();
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_formats_the_documented_example() {
        let name = "Seiken Densetsu 3 (Japan) [T+ENG]";
        let expected = [
            "Seiken Densetsu 3 (Japan) [T+ENG]",
            "Seiken Densetsu 3 [T+ENG]",
            "Seiken Densetsu 3 (Japan)",
            "Seiken Densetsu 3",
            "Seiken Densetsu 3 (Japan)",
            "Seiken Densetsu 3",
            "Seiken Densetsu 3 (Japan)",
        ];
        for (mode, expected) in GameNameDisplay::ALL.into_iter().zip(expected) {
            assert_eq!(mode.apply(name), expected, "{mode:?}");
        }
    }

    #[test]
    fn keep_modes_retain_only_recognised_region_and_disc_blocks() {
        let name = "Game (USA, Europe) (Disc 2) (Rev 1) [T+ENG]";
        assert_eq!(
            GameNameDisplay::KeepRegion.apply(name),
            "Game (USA, Europe)"
        );
        assert_eq!(GameNameDisplay::KeepDiscIndex.apply(name), "Game (Disc 2)");
        assert_eq!(
            GameNameDisplay::KeepRegionAndDiscIndex.apply(name),
            "Game (USA, Europe) (Disc 2)"
        );
        assert_eq!(
            GameNameDisplay::KeepRegionAndDiscIndex.apply("Game (tape 1) (japan)"),
            "Game (tape 1) (japan)",
            "recognition is ASCII case-insensitive"
        );
    }

    #[test]
    fn shortened_modes_match_flat_playlist_block_behaviour() {
        assert_eq!(
            GameNameDisplay::RemoveParentheses.apply("A  (One)  B [Two]"),
            "A B [Two]"
        );
        assert_eq!(
            GameNameDisplay::RemoveBrackets.apply("A (One) [Two]"),
            "A (One)"
        );
        assert_eq!(
            GameNameDisplay::RemoveParenthesesAndBrackets.apply("A [One) B"),
            "A B",
            "combined mode closes at either flat-block delimiter"
        );
        assert_eq!(
            GameNameDisplay::RemoveParentheses.apply("日本語 (Japan)"),
            "日本語"
        );
    }

    #[test]
    fn full_is_borrowed_and_preserves_every_byte() {
        let name = "  Game  (Japan) [T+ENG]  ";
        assert!(matches!(
            GameNameDisplay::Full.apply(name),
            Cow::Borrowed(_)
        ));
        assert_eq!(GameNameDisplay::Full.apply(name), name);
    }
}
