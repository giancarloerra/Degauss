//! Settings the user changes from inside Degauss.
//!
//! The shipped `degauss.toml` is documentation as much as configuration: it
//! explains what each value does and why. Rewriting it from the options
//! screen would throw all of that away, so changes are written to a separate
//! `settings.toml` that overlays it. Delete that file and everything returns
//! to the documented defaults.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{DegaussError, Result};

/// Every value the options screen can change. All optional: an absent field
/// means "whatever the shipped configuration says".
/// Unknown keys are deliberately IGNORED here, unlike in the documented
/// configuration where a typo should be caught. This file is written by
/// Degauss, so an unknown key means it was written by a different version:
/// refusing to start because an old setting no longer exists would turn
/// every upgrade into a broken machine.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Settings {
    pub speed_step: Option<usize>,
    /// Fastest scroll speed that still loads a picture per row, as an index
    /// into the speed ladder.
    pub art_limit: Option<usize>,
    pub layout: Option<String>,
    /// Which typeface the interface is set in. Absent means whatever the
    /// shipped configuration says.
    #[serde(default)]
    pub font: Option<String>,
    /// Whether favourites are gathered at the top of a folder.
    pub favorites_first: Option<bool>,
    /// Whether picking a random game starts it, or only moves the cursor to
    /// it. Absent means only moving the cursor.
    #[serde(default)]
    pub random_launches: Option<bool>,
    /// Whether folders are listed after the games rather than before.
    pub folders_last: Option<bool>,
    /// The view each folder was last looked at in, where one was chosen
    /// for it. Only folders the view was changed in are here.
    #[serde(default)]
    pub folder_views: std::collections::BTreeMap<String, String>,
    /// Folders and games hidden one at a time, by the name each is known
    /// by. Systems are hidden by id in `hidden`; this is everything else.
    #[serde(default)]
    pub hidden_paths: Vec<String>,
    pub show_stats: Option<bool>,
    pub show_art: Option<bool>,
    pub present: Option<String>,
    /// Systems the user has hidden, by id. Hiding is per-system and
    /// reversible; nothing is ever removed from the table.
    #[serde(default)]
    pub hidden: Vec<String>,
    pub show_hidden: Option<bool>,
    /// Show the Other group: the cores that are not games. Off by default,
    /// because a list of games is what this is for.
    pub show_other: Option<bool>,
    /// Show the Utility group: test patterns and measurement cores.
    pub show_utility: Option<bool>,
    /// The strip along the bottom. Off by default: the screen is 240 lines
    /// and the list is what it is for.
    pub show_bar: Option<bool>,
    /// Show folders that hold nothing. Off by default: a card collects
    /// empty folders, and every one of them is a dead end to walk into.
    pub show_empty: Option<bool>,
    pub overscan_x: Option<u32>,
    pub overscan_y: Option<u32>,
    /// Seconds of being left alone before the screensaver starts. Zero off.
    pub screensaver_after: Option<u64>,
    /// Nudge the whole picture, in pixels. Screens are not all centred.
    pub shift_x: Option<i32>,
    pub shift_y: Option<i32>,
}

impl Settings {
    /// Read the overlay. A missing file is normal and means "no changes
    /// yet"; a corrupt one is reported rather than silently ignored, or the
    /// user's settings would vanish with no explanation.
    pub fn load(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text)
                .map_err(|e| DegaussError::malformed("settings", path, e.to_string())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
            Err(e) => Err(DegaussError::io("reading settings", path, e)),
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let text = toml::to_string_pretty(self)
            .map_err(|e| DegaussError::malformed("settings", path, e.to_string()))?;
        let body = format!(
            "# Written by Degauss when you change something in Options.\n\
             # The documented defaults live in degauss.toml; delete this file\n\
             # to go back to them.\n\n{text}"
        );
        std::fs::write(path, body).map_err(|e| DegaussError::io("writing settings", path, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_written_by_v0_2_0_still_load_with_every_key_meant() {
        // Migration carries settings.toml over as it is. A key the current
        // code renamed or retyped would go back to its default silently,
        // which reads as the user's choices being forgotten.
        let text = include_str!("../tests/fixtures/v0.2.0-settings.toml");
        let settings: Settings = toml::from_str(text).expect("v0.2.0 settings must keep parsing");
        assert_eq!(settings.font.as_deref(), Some("pixel"));
        assert_eq!(settings.layout.as_deref(), Some("details"));
        assert_eq!(settings.overscan_x, Some(5));
        assert_eq!(settings.hidden, ["PDP1", "VC4000"]);
        assert_eq!(settings.folder_views.len(), 2);
    }

    fn temp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "degauss-settings-{tag}-{}.toml",
            std::process::id()
        ))
    }

    #[test]
    fn a_missing_file_means_no_changes_rather_than_an_error() {
        let settings =
            Settings::load(Path::new("/definitely/not/here.toml")).expect("absent is ok");
        assert_eq!(settings, Settings::default());
    }

    #[test]
    fn what_is_saved_is_what_comes_back() {
        let path = temp_path("roundtrip");
        let settings = Settings {
            speed_step: Some(7),
            hidden: vec!["PSX".to_string()],
            art_limit: Some(0),
            layout: Some("covers".into()),
            show_stats: Some(true),
            overscan_x: Some(24),
            ..Default::default()
        };
        settings.save(&path).expect("saved");

        let read = Settings::load(&path).expect("read back");
        assert_eq!(read, settings);
        // Untouched values stay absent, so the documented defaults keep
        // applying rather than being frozen at whatever they were today.
        assert!(read.present.is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_setting_from_an_older_version_is_ignored_rather_than_fatal() {
        // A settings file written by an older build can name a key that no longer
        // exists. Refusing to start would leave the user with no way back in.
        let path = temp_path("legacy");
        std::fs::write(&path, "speed_step = 4\nmax_depth = 3\n").unwrap();

        let settings = Settings::load(&path).expect("an unknown key must not be fatal");
        assert_eq!(settings.speed_step, Some(4), "the keys we know still apply");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_corrupt_file_is_reported_not_ignored() {
        // Silently discarding settings would look like Degauss forgetting
        // them at random.
        let path = temp_path("corrupt");
        std::fs::write(&path, "speed_step = \"not a number\"").unwrap();
        assert!(Settings::load(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn the_saved_file_explains_itself() {
        let path = temp_path("comment");
        Settings {
            show_stats: Some(true),
            ..Default::default()
        }
        .save(&path)
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("degauss.toml"),
            "must point at the documented file"
        );
        std::fs::remove_file(&path).ok();
    }
}
