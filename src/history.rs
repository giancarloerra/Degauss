//! The ten most recently handed-off game launches.
//!
//! This is runtime state, not a preference and not a copy of presentation
//! data. The originating system and exact launch target are enough to ask the
//! current cache for the name, artwork and metadata again when the collection
//! is opened.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::browse::Launch;
use crate::error::{DegaussError, Result};
use crate::settings::{SaveLabels, SaveOutcome};

const FORMAT: u32 = 1;
pub const CAP: usize = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    pub system: String,
    pub launch: Launch,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct History {
    format: u32,
    pub entries: Vec<Entry>,
}

impl Default for History {
    fn default() -> Self {
        Self {
            format: FORMAT,
            entries: Vec::new(),
        }
    }
}

const LABELS: SaveLabels = SaveLabels {
    creating_temporary: "creating temporary launch history",
    writing_temporary: "writing temporary launch history",
    flushing_temporary: "flushing temporary launch history",
    writing: "writing launch history",
    no_file_name: "launch-history path has no file name",
    could_not_reserve: "could not reserve a temporary launch-history file",
    installing: "installing launch history",
    flushing_directory: "flushing launch-history directory",
};

pub fn path_beside(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("last-played.toml")
}

impl History {
    /// A missing file is the normal empty state. A malformed or unsupported
    /// file is an error so it cannot be mistaken for an empty history and
    /// overwritten on the next launch.
    pub fn load(path: &Path) -> Result<Self> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(DegaussError::io("reading launch history", path, error)),
        };
        let mut history: Self = toml::from_str(&text)
            .map_err(|error| DegaussError::malformed("launch history", path, error.to_string()))?;
        if history.format != FORMAT {
            return Err(DegaussError::unsupported(
                "launch history",
                format!(
                    "{} uses format {}; this build reads format {FORMAT}",
                    path.display(),
                    history.format
                ),
            ));
        }
        history.entries.truncate(CAP);
        Ok(history)
    }

    pub fn remember(&mut self, entry: Entry) {
        self.entries
            .retain(|held| held.system != entry.system || held.launch != entry.launch);
        self.entries.insert(0, entry);
        self.entries.truncate(CAP);
    }

    pub fn save(&self, path: &Path) -> Result<SaveOutcome> {
        let text = toml::to_string_pretty(self)
            .map_err(|error| DegaussError::malformed("launch history", path, error.to_string()))?;
        crate::settings::install_beside_settings(&LABELS, path, &text)
    }
}

/// Update only after the launch command has been handed to MiSTer. Loading
/// again here preserves a clear malformed-file failure and prevents a stale
/// in-memory snapshot from overwriting a newer bounded history.
pub fn record(path: &Path, entry: Entry) -> Result<SaveOutcome> {
    let mut history = History::load(path)?;
    history.remember(entry);
    history.save(path)
}

/// Stable within the persisted identity contract and suitable for locating a
/// row again after a search or a return from a game. It is not shown.
pub fn entry_key(entry: &Entry) -> String {
    let target = match &entry.launch {
        Launch::File(path) => format!("f:{}", path.display()),
        Launch::AmigaVision { install, title } => {
            format!("a:{}|{title}", install.display())
        }
    };
    format!("{}|{target}", entry.system)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("degauss-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        path
    }

    fn entry(system: &str, game: &str) -> Entry {
        Entry {
            system: system.to_string(),
            launch: Launch::File(PathBuf::from(format!("/games/{system}/{game}"))),
            name: game.to_string(),
        }
    }

    #[test]
    fn missing_history_is_empty_and_round_trips() {
        let root = directory("roundtrip");
        let path = root.join("last-played.toml");
        let mut history = History::load(&path).unwrap();
        assert!(history.entries.is_empty());
        history.remember(entry("NES", "One.nes"));
        history.save(&path).unwrap();
        assert_eq!(History::load(&path).unwrap(), history);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn replay_moves_to_front_and_eleven_unique_games_keep_ten() {
        let mut history = History::default();
        for index in 0..11 {
            history.remember(entry("NES", &format!("Game {index}.nes")));
        }
        assert_eq!(history.entries.len(), CAP);
        assert_eq!(history.entries[0].name, "Game 10.nes");
        assert_eq!(history.entries[9].name, "Game 1.nes");

        let replay = history.entries[7].clone();
        history.remember(replay.clone());
        assert_eq!(history.entries.len(), CAP);
        assert_eq!(history.entries[0], replay);
    }

    #[test]
    fn same_target_in_two_systems_remains_distinct() {
        let launch = Launch::File(PathBuf::from("/games/shared/Game.rom"));
        let mut history = History::default();
        history.remember(Entry {
            system: "First".into(),
            launch: launch.clone(),
            name: "Game".into(),
        });
        history.remember(Entry {
            system: "Second".into(),
            launch,
            name: "Game".into(),
        });
        assert_eq!(history.entries.len(), 2);
        assert_ne!(
            entry_key(&history.entries[0]),
            entry_key(&history.entries[1])
        );
    }

    #[test]
    fn every_supported_launch_identity_round_trips_exactly() {
        let root = directory("launch-identities");
        let path = root.join("last-played.toml");
        let entries = vec![
            entry("NES", "Ordinary.nes"),
            Entry {
                system: "Arcade".into(),
                launch: Launch::File(PathBuf::from("/games/_Arcade/Example.mra")),
                name: "Arcade Example".into(),
            },
            Entry {
                system: "PSX".into(),
                launch: Launch::File(PathBuf::from("/games/PSX/Example.mgl")),
                name: "Disc Set".into(),
            },
            Entry {
                system: "SNES".into(),
                launch: Launch::File(PathBuf::from("/games/SNES/Collection.zip/Folder/Game.sfc")),
                name: "Archived Game".into(),
            },
            Entry {
                system: "Amiga".into(),
                launch: Launch::AmigaVision {
                    install: PathBuf::from("/games/Amiga/AmigaVision"),
                    title: "The Settlers".into(),
                },
                name: "The Settlers".into(),
            },
        ];
        let history = History {
            format: FORMAT,
            entries: entries.clone(),
        };
        history.save(&path).unwrap();
        assert_eq!(History::load(&path).unwrap().entries, entries);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_history_is_not_replaced_by_recording() {
        let root = directory("malformed");
        let path = root.join("last-played.toml");
        std::fs::write(&path, "not = [valid").unwrap();
        let before = std::fs::read(&path).unwrap();
        assert!(record(&path, entry("NES", "One.nes")).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        std::fs::remove_dir_all(root).unwrap();
    }
}
