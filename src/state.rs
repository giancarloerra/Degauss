//! Where the user was, remembered across a game.
//!
//! Launching a game ends this program: the MGL is handed to MiSTer, MiSTer
//! loads the core and re-executes itself, and Degauss is started again when
//! the menu core comes back. Without this, coming out of a game lands at the
//! top of the tree, which after walking four folders into a collection is
//! the wrong place by a long way.
//!
//! Only the device path uses this: nothing resumes a position when drawing
//! a single frame to an image.
#![cfg_attr(not(target_os = "linux"), allow(dead_code))]
//!
//! Kept apart from `settings.toml` on purpose. That file is what the user
//! chose; this is where they happened to be standing, and mixing the two
//! would rewrite their settings on every launch.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::browse::Place;
use crate::error::{DegaussError, Result};

/// A place, in a form that survives being written down.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedPlace {
    kind: String,
    path: String,
    #[serde(default)]
    install: String,
    /// Where the cursor sat in THIS folder, so walking back out lands where
    /// the user left off at every level, not only the last one.
    #[serde(default)]
    selected: usize,
}

impl SavedPlace {
    fn of(place: &Place, selected: usize) -> Self {
        match place {
            Place::Roots => SavedPlace {
                kind: "roots".into(),
                path: String::new(),
                install: String::new(),
                selected,
            },
            Place::Dir(path) => SavedPlace {
                kind: "dir".into(),
                path: path.to_string_lossy().into_owned(),
                install: String::new(),
                selected,
            },
            Place::Archive(path) => SavedPlace {
                kind: "archive".into(),
                path: path.to_string_lossy().into_owned(),
                install: String::new(),
                selected,
            },
            Place::Listing { install, file } => SavedPlace {
                kind: "listing".into(),
                path: file.to_string_lossy().into_owned(),
                install: install.to_string_lossy().into_owned(),
                selected,
            },
        }
    }

    /// The place this describes, or nothing when the card no longer has it.
    ///
    /// A folder that has been renamed or removed since the game started is
    /// not an error: the walk simply stops there, which is the same place
    /// the user would reach by going back.
    fn to_place(&self) -> Option<Place> {
        match self.kind.as_str() {
            "roots" => Some(Place::Roots),
            "dir" => PathBuf::from(&self.path)
                .exists()
                .then(|| Place::Dir(PathBuf::from(&self.path))),
            "archive" => PathBuf::from(&self.path)
                .exists()
                .then(|| Place::Archive(PathBuf::from(&self.path))),
            "listing" => PathBuf::from(&self.path).exists().then(|| Place::Listing {
                install: PathBuf::from(&self.install),
                file: PathBuf::from(&self.path),
            }),
            _ => None,
        }
    }

    /// The cursor position this place was left at.
    pub fn selected(&self) -> usize {
        self.selected
    }
}

/// Everything needed to put the user back where they were.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct State {
    /// The system's id from the table, not its index: a table that gains an
    /// entry must not send the user to a different machine.
    #[serde(default)]
    pub system: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub trail: Vec<SavedPlace>,
    /// Where the cursor sat in the folder on screen.
    #[serde(default)]
    pub selected: usize,
}

impl State {
    pub fn record(system: &str, category: &str, trail: &[(Place, usize)], selected: usize) -> Self {
        State {
            system: system.to_string(),
            category: category.to_string(),
            trail: trail
                .iter()
                .map(|(place, selected)| SavedPlace::of(place, *selected))
                .collect(),
            selected,
        }
    }

    /// Stops at the first place the card no longer has. Skipping it and
    /// carrying on would open a folder underneath a parent that is gone.
    pub fn places(&self) -> Vec<Place> {
        self.trail.iter().map_while(SavedPlace::to_place).collect()
    }

    /// Read it back. A missing file is the normal case on a first run; a
    /// corrupt one is ignored rather than fatal, because being unable to say
    /// where somebody was is not a reason to refuse to start.
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path)
            .ok()
            .and_then(|text| toml::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| DegaussError::malformed("state", path, e.to_string()))?;
        std::fs::write(path, text.as_bytes())
            .map_err(|e| DegaussError::io("writing state", path, e))
    }
}

/// Marker saying the program is coming back from a game rather than being
/// started for the first time.
///
/// Take the written-down position, if it is the kind of start that should
/// have one.
///
/// Coming back from a game, it is read and kept. Coming up cold, it is
/// deleted and nothing is returned: a machine that has been off should
/// start at the top, not four folders inside whatever was played last,
/// and a file left lying there is the thing that would send it there.
pub fn take_position(resuming: bool, path: &Path) -> Option<State> {
    if !resuming {
        if path.exists() {
            let _ = std::fs::remove_file(path);
        }
        return None;
    }
    let saved = State::load(path);
    (!saved.system.is_empty()).then_some(saved)
}

/// In `/tmp` on purpose: it is gone after a power cycle, so a cold start
/// still gets the wordmark while returning from a game does not.
pub const RESUME_MARKER: &str = "/tmp/degauss.resume";

pub fn mark_resuming() {
    let _ = std::fs::write(RESUME_MARKER, b"1");
}

pub fn is_resuming() -> bool {
    Path::new(RESUME_MARKER).exists()
}

pub fn clear_resuming() {
    let _ = std::fs::remove_file(RESUME_MARKER);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resume_state_written_by_v0_2_0_still_resumes() {
        // State loading is unwrap_or_default, so a schema break here does
        // not crash: it silently throws the user's place away. This pins
        // that a v0.2.0 state still RESUMES rather than merely loading.
        let text = include_str!("../tests/fixtures/v0.2.0-state.toml");
        let state: State = toml::from_str(text).expect("v0.2.0 state must keep parsing");
        assert_eq!(state.system, "SNES");
        assert_eq!(state.selected, 42);
        assert_eq!(state.trail.len(), 2, "the walked path survives");
    }

    #[test]
    fn a_state_file_written_before_this_existed_still_reads() {
        let dir = std::env::temp_dir().join("degauss-state-old");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.toml");
        std::fs::write(
            &path,
            "system = \"C64\"\ncategory = \"Computer\"\nselected = 0\n",
        )
        .unwrap();
        let read = State::load(&path);
        assert_eq!(read.system, "C64");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cold_start_forgets_where_it_was() {
        let dir = std::env::temp_dir().join(format!("degauss-cold-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.toml");
        State::record("C64", "Computer", &[], 12)
            .save(&path)
            .unwrap();
        assert!(path.exists());

        assert!(
            take_position(false, &path).is_none(),
            "nothing to go back to"
        );
        assert!(
            !path.exists(),
            "and it is not left lying there for next time"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn coming_back_from_a_game_goes_back_to_where_it_was() {
        let dir = std::env::temp_dir().join(format!("degauss-warm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("state.toml");
        State::record("C64", "Computer", &[], 12)
            .save(&path)
            .unwrap();

        let saved = take_position(true, &path).expect("a position");
        assert_eq!(saved.system, "C64");
        assert_eq!(saved.selected, 12);
        assert!(
            path.exists(),
            "kept, in case the next game comes from here too"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_cold_start_with_nothing_written_down_is_not_an_error() {
        let dir = std::env::temp_dir().join(format!("degauss-none-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(take_position(false, &dir.join("state.toml")).is_none());
        assert!(take_position(true, &dir.join("state.toml")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }
}
