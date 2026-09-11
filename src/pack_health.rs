//! Artwork Pack warnings already seen, remembered across a game.
//!
//! An incomplete Pack stays usable, and its warning is worth one look, not
//! one per start: launching a game ends this program, so a warning kept only
//! in memory came back on every return. This file holds, per source group,
//! the identity of the snapshot whose warning was shown. A pack that is
//! unavailable or invalid is never written here; those need acting on and
//! are shown every time.
//!
//! Kept apart from `state.toml`, which a cold start deletes, and from
//! `settings.toml`, which refuses to start on a broken value. This file may
//! be missing, stale or broken and the only cost is one more warning, which
//! the next acknowledgement then replaces.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DegaussError, Result};

pub const FILE: &str = "artwork-pack-warnings.toml";

/// Beside the settings, which is beside the configuration.
pub fn path_beside(settings_path: &Path) -> PathBuf {
    settings_path.parent().unwrap_or(Path::new(".")).join(FILE)
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Acknowledgements {
    /// Source group to the health digest whose incomplete warning was shown.
    #[serde(default)]
    degraded: BTreeMap<String, String>,
}

impl Acknowledgements {
    /// Read it back. A missing file is the normal case; one that cannot be
    /// read or parsed is written to the log and read as empty, so the
    /// warning is shown once more and the next acknowledgement replaces it.
    pub fn load(path: &Path) -> Self {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(parsed) => parsed,
                Err(error) => {
                    crate::note(&format!(
                        "artwork pack warnings: {} is malformed: {error}",
                        path.display()
                    ));
                    Self::default()
                }
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(error) => {
                crate::note(&format!(
                    "artwork pack warnings: reading {} failed: {error}",
                    path.display()
                ));
                Self::default()
            }
        }
    }

    pub fn acknowledged(&self, group: &str, digest: &str) -> bool {
        self.degraded.get(group).is_some_and(|seen| seen == digest)
    }

    /// Record the snapshot whose warning is being shown. One entry per
    /// group: an updated pack's warning is for another state, and the old
    /// one is of no further use.
    pub fn acknowledge(&mut self, group: &str, digest: &str) {
        self.degraded.insert(group.to_string(), digest.to_string());
    }

    /// Written beside and moved into place: a file cut short must not be
    /// read back as a shorter list of warnings already seen.
    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self).map_err(|error| {
            DegaussError::malformed("artwork pack warnings", path, error.to_string())
        })?;
        let body = format!(
            "# Written by Degauss when an incomplete Artwork Pack warning is shown.\n\
             # Delete this file to see those warnings again.\n\n{text}"
        );
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = parent.join(format!(".{FILE}.{}.part", std::process::id()));
        let outcome = (|| {
            let mut file = std::fs::File::create(&temporary).map_err(|error| {
                DegaussError::io("writing temporary artwork pack warnings", &temporary, error)
            })?;
            file.write_all(body.as_bytes()).map_err(|error| {
                DegaussError::io("writing temporary artwork pack warnings", &temporary, error)
            })?;
            file.sync_all().map_err(|error| {
                DegaussError::io(
                    "flushing temporary artwork pack warnings",
                    &temporary,
                    error,
                )
            })?;
            drop(file);
            std::fs::rename(&temporary, path)
                .map_err(|error| DegaussError::io("installing artwork pack warnings", path, error))
        })();
        if outcome.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("degauss-pack-health-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_saved_acknowledgement_reads_back_for_its_group_only() {
        let dir = temp("round-trip");
        let path = path_beside(&dir.join("settings.toml"));
        assert_eq!(path, dir.join(FILE));
        let mut seen = Acknowledgements::default();
        seen.acknowledge("NES", "abc");
        seen.save(&path).unwrap();

        let read = Acknowledgements::load(&path);
        assert!(
            read.acknowledged("NES", "abc"),
            "a warning shown before a game must stay acknowledged in the next process"
        );
        assert!(
            !read.acknowledged("NES", "abd"),
            "a changed snapshot is a different warning"
        );
        assert!(
            !read.acknowledged("SNES", "abc"),
            "one group's acknowledgement must not silence another group's pack"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_missing_file_acknowledges_nothing() {
        let dir = temp("missing");
        let read = Acknowledgements::load(&dir.join(FILE));
        assert!(!read.acknowledged("NES", "abc"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_malformed_file_acknowledges_nothing_and_is_replaced_by_the_next_save() {
        let dir = temp("malformed");
        let path = dir.join(FILE);
        std::fs::write(&path, "degraded = 3\n[[[").unwrap();
        let mut read = Acknowledgements::load(&path);
        assert!(
            !read.acknowledged("NES", "abc"),
            "a broken file must cost one more warning, never a refusal to start"
        );
        read.acknowledge("NES", "abc");
        read.save(&path).unwrap();
        assert!(Acknowledgements::load(&path).acknowledged("NES", "abc"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_new_snapshot_replaces_the_old_one_and_leaves_other_groups_alone() {
        let mut seen = Acknowledgements::default();
        seen.acknowledge("NES", "first");
        seen.acknowledge("SNES", "other");
        seen.acknowledge("NES", "second");
        assert!(
            !seen.acknowledged("NES", "first"),
            "the pack was updated; the old snapshot's warning was for another state"
        );
        assert!(seen.acknowledged("NES", "second"));
        assert!(seen.acknowledged("SNES", "other"));
    }

    #[test]
    fn saving_leaves_no_temporary_file_beside_the_warnings() {
        let dir = temp("no-part");
        let path = dir.join(FILE);
        let mut seen = Acknowledgements::default();
        seen.acknowledge("NES", "abc");
        seen.save(&path).unwrap();
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec![FILE.to_string()]);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("# Written by Degauss"),
            "the file says what it is for and how to reset it"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_that_cannot_install_reports_its_error() {
        let dir = temp("blocked");
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"file").unwrap();
        let mut seen = Acknowledgements::default();
        seen.acknowledge("NES", "abc");
        let error = seen.save(&blocked.join(FILE)).unwrap_err();
        assert!(
            error.to_string().contains("artwork pack warnings"),
            "a failed save must say what failed: {error}"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
