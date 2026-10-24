//! Paged controller keyboard for names and browser searches.
//!
//! Jump and ScreenScraper keep their own established controls. This
//! keyboard deliberately offers ScreenScraper's character set minus the
//! characters the Favourites and theme filename contracts reject.

pub const COLUMNS: usize = 9;

const LOWER: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_.@";
const UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.@";
const SYMBOLS: &str = "!#$%&'()+,;=[]^`{}~";

pub fn character_is_offered(character: char) -> bool {
    character == ' '
        || LOWER.contains(character)
        || UPPER.contains(character)
        || SYMBOLS.contains(character)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Lower,
    Upper,
    Symbols,
    Pin,
}

impl Page {
    pub fn next(self) -> Self {
        match self {
            Self::Lower => Self::Upper,
            Self::Upper => Self::Symbols,
            Self::Symbols => Self::Lower,
            Self::Pin => Self::Pin,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lower => "lowercase",
            Self::Upper => "uppercase",
            Self::Symbols => "symbols",
            Self::Pin => "six digits",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Character(char),
    Space,
    Clear,
    Save,
    Cancel,
}

impl Key {
    pub fn label(self) -> String {
        match self {
            Self::Character(character) => character.to_string(),
            Self::Space => "SP".to_string(),
            Self::Clear => "Clear".to_string(),
            Self::Save => "Save".to_string(),
            Self::Cancel => "Cancel".to_string(),
        }
    }
}

/// Keys for one filename page. Editors and saved collections use Save and
/// Cancel; Favourites names finish with B, preserving their existing flow.
pub fn keys(page: Page, save_and_cancel: bool) -> Vec<Key> {
    let characters = match page {
        Page::Lower => LOWER,
        Page::Upper => UPPER,
        Page::Symbols => SYMBOLS,
        Page::Pin => "0123456789",
    };
    let mut keys: Vec<Key> = characters.chars().map(Key::Character).collect();
    if page != Page::Pin {
        keys.push(Key::Space);
    }
    keys.push(Key::Clear);
    if save_and_cancel {
        keys.push(Key::Save);
        keys.push(Key::Cancel);
    }
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pin_keyboard_never_offers_letters_or_space_and_cannot_change_pages() {
        assert_eq!(Page::Pin.next(), Page::Pin);
        let offered = keys(Page::Pin, false);
        assert_eq!(offered.len(), 11);
        assert_eq!(offered[0], Key::Character('0'));
        assert_eq!(offered[9], Key::Character('9'));
        assert_eq!(offered[10], Key::Clear);
        assert!(!offered.contains(&Key::Space));
    }

    #[test]
    fn pages_match_screenscraper_without_forbidden_filename_characters() {
        let offered: String = [Page::Lower, Page::Upper, Page::Symbols]
            .into_iter()
            .flat_map(|page| keys(page, false))
            .filter_map(|key| match key {
                Key::Character(character) => Some(character),
                _ => None,
            })
            .collect();
        for character in crate::favorites::BAD_CHARS {
            assert!(!offered.contains(character));
        }
        for character in "aA0-_.@!#$%&'()+,;=[]^`{}~".chars() {
            assert!(offered.contains(character), "missing {character:?}");
        }
    }

    #[test]
    fn filename_and_theme_keyboards_have_their_required_controls() {
        let filename = keys(Page::Lower, false);
        assert!(filename.ends_with(&[Key::Space, Key::Clear]));
        assert!(!filename.contains(&Key::Save));

        let theme = keys(Page::Symbols, true);
        assert!(theme.ends_with(&[Key::Space, Key::Clear, Key::Save, Key::Cancel]));
    }

    #[test]
    fn page_order_cycles_lower_upper_symbols() {
        assert_eq!(Page::Lower.next(), Page::Upper);
        assert_eq!(Page::Upper.next(), Page::Symbols);
        assert_eq!(Page::Symbols.next(), Page::Lower);
    }
}
