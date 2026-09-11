//! Artwork Pack warnings already seen, remembered across a game.
//!
//! An incomplete Pack stays usable, and its warning is worth one look, not
//! one per start: launching a game ends this program, so a warning kept only
//! in memory came back on every return. This file holds, per source group,
//! the identity of the snapshot whose warning was dismissed. A pack that is
//! unavailable or invalid is never written here; those need acting on and
//! are shown again on every start.
//!
//! Kept apart from `state.toml`, which a cold start deletes, and from
//! `settings.toml`, which refuses to start on a broken value. This file may
//! be missing, stale or broken and the only cost is one more warning, which
//! the next acknowledgement then replaces. Read fresh each time it is
//! needed, never cached: deleting it while this program runs must not be
//! undone by the next save.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DegaussError, Result};
use crate::settings::SaveOutcome;

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
    /// Why the file read as empty, when it was there but could not be
    /// parsed: the warning it would have silenced says so, and that the
    /// next dismissal writes over it.
    #[serde(skip)]
    malformed: Option<String>,
}

impl Acknowledgements {
    /// Read it back. A missing file is the normal case, and one that cannot
    /// be parsed is read as empty with the reason kept in `malformed`, so
    /// the warning is shown once more, saying why, and the next
    /// acknowledgement replaces it. Nothing is logged here: the file is read
    /// again before that save, and the caller that puts the warning up
    /// writes the reason down once. A file that is there but cannot be read
    /// is an error: saving over it would throw away every acknowledgement
    /// it holds.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => match toml::from_str(&text) {
                Ok(parsed) => Ok(parsed),
                Err(error) => Ok(Self {
                    malformed: Some(error.to_string()),
                    ..Self::default()
                }),
            },
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(DegaussError::io(
                "reading artwork pack warnings",
                path,
                error,
            )),
        }
    }

    pub fn acknowledged(&self, group: &str, digest: &str) -> bool {
        self.degraded.get(group).is_some_and(|seen| seen == digest)
    }

    /// Why the file was read as empty, when that is what happened.
    pub fn malformed(&self) -> Option<&str> {
        self.malformed.as_deref()
    }

    /// Record the snapshot whose warning was dismissed. One entry per
    /// group: an updated pack's warning is for another state, and the old
    /// one is of no further use.
    pub fn acknowledge(&mut self, group: &str, digest: &str) {
        self.degraded.insert(group.to_string(), digest.to_string());
    }

    /// Written beside and moved into place: a file cut short must not be
    /// read back as a shorter list of warnings already seen. The directory
    /// is flushed afterwards like the settings are, so the move itself
    /// survives a power cut; when that flush fails the file is in place and
    /// the outcome says what could not be confirmed. A temporary file left
    /// by a failed write or move is removed, and when that fails too the
    /// error says so, as the settings writer does.
    pub fn save(&self, path: &Path) -> Result<SaveOutcome> {
        let text = toml::to_string_pretty(self).map_err(|error| {
            DegaussError::malformed("artwork pack warnings", path, error.to_string())
        })?;
        let body = format!(
            "# Written by Degauss when an incomplete Artwork Pack warning is dismissed.\n\
             # Delete this file and restart Degauss to see those warnings again.\n\n{text}"
        );
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        let temporary = parent.join(format!(".{FILE}.degauss-{}.tmp", std::process::id()));
        let mut file = std::fs::File::create(&temporary).map_err(|error| {
            DegaussError::io(
                "creating temporary artwork pack warnings",
                &temporary,
                error,
            )
        })?;
        let written: Result<()> = (|| {
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
            Ok(())
        })();
        drop(file);
        if let Err(error) = written {
            return Err(cleanup_temporary(&temporary, error));
        }
        if let Err(error) = std::fs::rename(&temporary, path) {
            let error = DegaussError::io("installing artwork pack warnings", path, error);
            return Err(cleanup_temporary(&temporary, error));
        }
        match std::fs::File::open(parent).and_then(|directory| directory.sync_all()) {
            Ok(()) => Ok(SaveOutcome::Durable),
            Err(error) => Ok(SaveOutcome::InstalledWithWarning(DegaussError::io(
                "flushing artwork pack warnings directory",
                parent,
                error,
            ))),
        }
    }
}

/// The failure that stopped the save, with the leftover temporary file
/// removed; when even that fails the error says so, because a stray file
/// beside the settings is worth knowing about.
fn cleanup_temporary(path: &Path, error: DegaussError) -> DegaussError {
    match std::fs::remove_file(path) {
        Ok(()) => error,
        Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => error,
        Err(cleanup) => DegaussError::unsupported(
            "writing artwork pack warnings",
            format!(
                "{error}; removing the temporary file {} also failed: {cleanup}",
                path.display()
            ),
        ),
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
        assert!(
            matches!(seen.save(&path).unwrap(), SaveOutcome::Durable),
            "a save into a writable directory is complete, file and directory alike"
        );

        let read = Acknowledgements::load(&path).unwrap();
        assert!(
            read.acknowledged("NES", "abc"),
            "a warning dismissed before a game must stay acknowledged in the next process"
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
        let read = Acknowledgements::load(&dir.join(FILE)).unwrap();
        assert!(!read.acknowledged("NES", "abc"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_malformed_file_acknowledges_nothing_and_is_replaced_by_the_next_save() {
        let dir = temp("malformed");
        let path = dir.join(FILE);
        std::fs::write(&path, "degraded = 3\n[[[").unwrap();
        let mut read = Acknowledgements::load(&path).unwrap();
        assert!(
            !read.acknowledged("NES", "abc"),
            "a broken file must cost one more warning, never a refusal to start"
        );
        assert!(
            read.malformed().is_some(),
            "the warning shown in its place must be able to say why the file was set aside"
        );
        read.acknowledge("NES", "abc");
        read.save(&path).unwrap();
        let replaced = Acknowledgements::load(&path).unwrap();
        assert!(replaced.acknowledged("NES", "abc"));
        assert!(
            replaced.malformed().is_none(),
            "the replacement reads back clean; the reason must not be written into it"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_file_that_cannot_be_read_is_an_error_not_an_empty_list() {
        let dir = temp("unreadable");
        let path = dir.join(FILE);
        std::fs::create_dir_all(&path).unwrap();
        let error = Acknowledgements::load(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("reading artwork pack warnings failed for"),
            "a read failure must be reported, not read as nothing acknowledged: {error}"
        );
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
        assert!(
            text.contains("restart Degauss"),
            "the reset instruction must say that a running program keeps its own list"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_whose_temporary_file_cannot_be_created_reports_its_error() {
        let dir = temp("blocked");
        let blocked = dir.join("not-a-directory");
        std::fs::write(&blocked, b"file").unwrap();
        let mut seen = Acknowledgements::default();
        seen.acknowledge("NES", "abc");
        let error = seen.save(&blocked.join(FILE)).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("creating temporary artwork pack warnings failed for"),
            "a failed save must say which step failed: {error}"
        );
        assert_eq!(
            std::fs::read(&blocked).unwrap(),
            b"file",
            "a failed save must leave what was in the way alone"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_save_that_cannot_move_into_place_reports_it_and_leaves_no_temporary_file() {
        let dir = temp("in-the-way");
        let path = dir.join(FILE);
        std::fs::create_dir(&path).unwrap();
        let mut seen = Acknowledgements::default();
        seen.acknowledge("NES", "abc");
        let error = seen.save(&path).unwrap_err();
        assert!(
            error
                .to_string()
                .starts_with("installing artwork pack warnings failed for"),
            "the move is the step that failed, and the error must say so: {error}"
        );
        assert!(
            !error.to_string().contains("also failed"),
            "a temporary file that was removed is not part of the failure: {error}"
        );
        let names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            names,
            vec![FILE.to_string()],
            "the written temporary file must not be left beside the settings"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_temporary_file_that_cannot_be_removed_is_part_of_the_reported_failure() {
        let dir = temp("stuck");
        let stuck = dir.join("stuck.tmp");
        std::fs::create_dir(&stuck).unwrap();
        let error = cleanup_temporary(
            &stuck,
            DegaussError::unsupported("writing artwork pack warnings", "the save failed"),
        );
        let text = error.to_string();
        assert!(
            text.contains("the save failed") && text.contains("also failed"),
            "both failures must reach the screen, or a stray file goes unexplained: {text}"
        );
        assert!(text.contains(&stuck.display().to_string()));
        let gone = cleanup_temporary(
            &dir.join("never-written.tmp"),
            DegaussError::unsupported("writing artwork pack warnings", "the save failed"),
        );
        assert_eq!(
            gone.to_string(),
            "writing artwork pack warnings unsupported: the save failed",
            "a temporary file that was never written is nothing to report"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
