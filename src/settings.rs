//! Settings the user changes from inside Degauss.
//!
//! The shipped `degauss.toml` is documentation as much as configuration: it
//! explains what each value does and why. Rewriting it from the options
//! screen would throw all of that away, so changes are written to a separate
//! `settings.toml` that overlays it. Delete that file and everything returns
//! to the documented defaults.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{DegaussError, Result};
use crate::name_display::GameNameDisplay;

/// Preference applies only when both supported launch variants are installed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CorePreference {
    #[default]
    StandardFirst,
    RetroAchievementsFirst,
}

/// A face button whose short press keeps its normal meaning while a
/// one-second hold may run one browsing action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HoldButton {
    A,
    B,
    X,
    Y,
}

impl HoldButton {
    pub const ALL: [Self; 4] = [Self::A, Self::B, Self::X, Self::Y];

    pub const fn index(self) -> usize {
        match self {
            Self::A => 0,
            Self::B => 1,
            Self::X => 2,
            Self::Y => 3,
        }
    }
}

/// An optional browsing action performed after a face button is held for
/// one second. The order is also the order used by the Options rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HoldShortcut {
    #[default]
    None,
    CycleView,
    RandomGame,
    RandomFavourite,
    AddRemoveFavourite,
    GameInformation,
    SearchThisFolder,
    JumpToLetter,
    Actions,
    Menu,
}

impl HoldShortcut {
    pub const ALL: [Self; 10] = [
        Self::None,
        Self::CycleView,
        Self::RandomGame,
        Self::RandomFavourite,
        Self::AddRemoveFavourite,
        Self::GameInformation,
        Self::SearchThisFolder,
        Self::JumpToLetter,
        Self::Actions,
        Self::Menu,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::CycleView => "Cycle View",
            Self::RandomGame => "Random Game",
            Self::RandomFavourite => "Random Favourite",
            Self::AddRemoveFavourite => "Add/Remove Favourite",
            Self::GameInformation => "Game Information",
            Self::SearchThisFolder => "Search This Folder",
            Self::JumpToLetter => "Jump to Letter",
            Self::Actions => "Actions",
            Self::Menu => "Menu",
        }
    }

    pub fn step(self, delta: isize) -> Self {
        let current = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or_default() as isize;
        let next = (current + delta).rem_euclid(Self::ALL.len() as isize) as usize;
        Self::ALL[next]
    }
}

impl CorePreference {
    pub fn next(self) -> Self {
        match self {
            Self::StandardFirst => Self::RetroAchievementsFirst,
            Self::RetroAchievementsFirst => Self::StandardFirst,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::StandardFirst => "Standard first",
            Self::RetroAchievementsFirst => "RetroAchievements first",
        }
    }
}

/// Which available source an Automatic system tries first.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AutomaticDataSource {
    /// Preserve the behaviour shipped before this setting existed.
    #[default]
    GamelistFirst,
    ArtworkPackFirst,
}

/// How the complete frontend is turned on the physical framebuffer.
///
/// The framebuffer mode itself is left alone. Quarter turns only swap the
/// logical dimensions handed to the existing UI and rotate the finished
/// scene in Slint's software renderer.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ScreenRotation {
    #[default]
    Off,
    Clockwise,
    CounterClockwise,
}

impl ScreenRotation {
    pub const ALL: [Self; 3] = [Self::Off, Self::Clockwise, Self::CounterClockwise];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Off => "Off",
            Self::Clockwise => "90° Clockwise",
            Self::CounterClockwise => "90° Counterclockwise",
        }
    }

    pub fn parse_cli(text: &str) -> Option<Self> {
        match text {
            "off" => Some(Self::Off),
            "cw" => Some(Self::Clockwise),
            "ccw" => Some(Self::CounterClockwise),
            _ => None,
        }
    }

    pub fn step(self, delta: isize) -> Self {
        let current = Self::ALL
            .iter()
            .position(|candidate| *candidate == self)
            .unwrap_or_default() as isize;
        Self::ALL[(current + delta).rem_euclid(Self::ALL.len() as isize) as usize]
    }

    pub const fn is_portrait(self) -> bool {
        !matches!(self, Self::Off)
    }

    pub const fn logical_size(self, physical_width: u32, physical_height: u32) -> (u32, u32) {
        if self.is_portrait() {
            (physical_height, physical_width)
        } else {
            (physical_width, physical_height)
        }
    }
}

impl AutomaticDataSource {
    pub fn next(self) -> Self {
        match self {
            Self::GamelistFirst => Self::ArtworkPackFirst,
            Self::ArtworkPackFirst => Self::GamelistFirst,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GamelistFirst => "Gamelist First",
            Self::ArtworkPackFirst => "Artwork Pack First",
        }
    }
}

/// Views chosen for exact places in the browser. The shape is deliberately
/// nested rather than encoded into one string key: category names, system ids
/// and filesystem paths are user-controlled and must not be able to collide.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CustomViews {
    /// The master Categories screen.
    #[serde(default)]
    pub categories: Option<String>,
    /// One Systems screen per category name.
    #[serde(default)]
    pub systems: std::collections::BTreeMap<String, String>,
    /// One map per stable system id, keyed by the existing `Place::key()`.
    #[serde(default)]
    pub games: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
}

impl CustomViews {
    pub fn is_empty(&self) -> bool {
        self.categories.is_none() && self.systems.is_empty() && self.games.is_empty()
    }

    pub fn len(&self) -> usize {
        usize::from(self.categories.is_some())
            + self.systems.len()
            + self
                .games
                .values()
                .map(std::collections::BTreeMap::len)
                .sum::<usize>()
    }
}

/// A settings replacement can be installed before the filesystem reports a
/// directory-flush failure. That state is not a failed install: callers must
/// keep the installed file and surface the durability warning.
#[derive(Debug)]
pub enum SaveOutcome {
    Durable,
    InstalledWithWarning(DegaussError),
}

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
    /// Optional top-level category opened after startup. Absent means Home.
    /// If the saved category is unavailable, startup remains at Home without
    /// replacing the saved choice.
    #[serde(default)]
    pub start_folder: Option<String>,
    /// What left and right do while browsing: "speed", "letter", "page"
    /// or "direction". Absent means speed, which is how Degauss always
    /// behaved.
    #[serde(default)]
    pub left_right: Option<String>,
    /// Which typeface the interface is set in. Absent means whatever the
    /// shipped configuration says.
    #[serde(default)]
    pub font: Option<String>,
    /// Whether the user changed Text after selecting the current theme. This
    /// keeps that explicit choice across a restart without allowing one
    /// theme's default to become the next theme's system font.
    #[serde(default)]
    pub theme_font_override: Option<bool>,
    /// The theme the palette comes from, by file stem. Absent means the
    /// standard palette: the `[colors]` block of `degauss.toml`.
    #[serde(default)]
    pub theme: Option<String>,
    /// Whether favourites are gathered at the top of a folder.
    pub favorites_first: Option<bool>,
    /// Maximum visible entries in the optional Last Played collection.
    /// Absent and zero are Off; retained history remains separate.
    #[serde(default)]
    pub last_played: Option<u8>,
    /// Legacy shortcut fields retained for settings written by and read by
    /// releases before configurable hold actions. New explicit bindings win.
    #[serde(default)]
    pub hold_x_favorite: Option<bool>,
    #[serde(default)]
    pub hold_y_random: Option<bool>,
    /// Configurable one-second face-button holds. Optional fields distinguish
    /// old settings, which need legacy migration, from an explicit None.
    #[serde(default)]
    pub hold_a_shortcut: Option<HoldShortcut>,
    #[serde(default)]
    pub hold_b_shortcut: Option<HoldShortcut>,
    #[serde(default)]
    pub hold_x_shortcut: Option<HoldShortcut>,
    #[serde(default)]
    pub hold_y_shortcut: Option<HoldShortcut>,
    /// Whether picking a random game starts it, or only moves the cursor to
    /// it. Absent means only moving the cursor.
    #[serde(default)]
    pub random_launches: Option<bool>,
    /// Whether folders are listed after the games rather than before.
    pub folders_last: Option<bool>,
    /// Views written by releases through v0.3.0, keyed only by `Place::key()`.
    /// Migrated at startup only when one installed system can safely own the
    /// key; retained here so existing settings files continue to deserialize.
    #[serde(default)]
    pub folder_views: std::collections::BTreeMap<String, String>,
    /// Optional views for exact browse places. Missing means the global
    /// `layout` applies there.
    #[serde(default, skip_serializing_if = "CustomViews::is_empty")]
    pub custom_views: CustomViews,
    /// Folders and games hidden one at a time, by the name each is known
    /// by. Systems are hidden by id in `hidden`; this is everything else.
    #[serde(default)]
    pub hidden_paths: Vec<String>,
    pub show_stats: Option<bool>,
    pub show_art: Option<bool>,
    /// How game artwork is horizontally corrected for the physical display:
    /// "framebuffer", "4:3" or "16:9". Absent keeps the original
    /// framebuffer-pixel behaviour.
    #[serde(default)]
    pub artwork_scale: Option<String>,
    /// How Details balances the list, the picture and the compact lines:
    /// "information" or "large-artwork". Absent means information, the
    /// layout Degauss always drew.
    #[serde(default)]
    pub details_style: Option<String>,
    /// Runtime-only presentation of the complete effective row name.
    /// Absent preserves full names from releases before this setting.
    #[serde(default)]
    pub game_name_display: Option<GameNameDisplay>,
    /// Whether folder rows receive Degauss's outer `[ name ]` marker.
    /// Absent preserves the marker used by earlier releases.
    #[serde(default)]
    pub folder_brackets: Option<bool>,
    /// Show the selected position and total while browsing games. Absent
    /// preserves the visible counter used by earlier releases.
    #[serde(default)]
    pub show_game_position: Option<bool>,
    pub present: Option<String>,
    /// Optional full-interface quarter turn. Absent preserves the landscape
    /// orientation used by every release before TATE support.
    #[serde(default)]
    pub screen_rotation: Option<ScreenRotation>,
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
    /// Show the optional top-level browser of installed core launchers.
    /// Absent is off so existing installations keep their released home.
    #[serde(default)]
    pub show_cores: Option<bool>,
    /// Show the optional top-level Core Updates browser. Absent is
    /// off so existing installations do no network or matching work.
    #[serde(default)]
    pub show_misterzine: Option<bool>,
    /// Nightly cores are visible unless explicitly switched off.
    pub show_unstable: Option<bool>,
    pub show_scripts: Option<bool>,
    /// Absent preserves standard-first launches.
    pub core_preference: Option<CorePreference>,
    /// Preferred source for systems left on Automatic. Absent preserves
    /// the Gamelist-first behaviour shipped before this setting existed.
    #[serde(default)]
    pub automatic_data_source: Option<AutomaticDataSource>,
    /// Present handheld systems in their own category. Absent is off, so
    /// existing installations retain MiSTer's Console grouping.
    #[serde(default)]
    pub separate_handheld_category: Option<bool>,
    /// Explicit per-system core version. Absence uses the global preference.
    /// Values are standard, ra, or an exact menu-relative Unstable RBF path.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub core_choices: BTreeMap<String, String>,
    /// Explicit per-system compatible core family. Absence is Automatic.
    /// Values are stable profile IDs declared by the systems table.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub launch_cores: BTreeMap<String, String>,
    /// Explicit per-game compatible core family, grouped by owning system.
    /// Missing systems and games inherit the per-system Launch Core setting.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub game_launch_cores: BTreeMap<String, BTreeMap<String, String>>,
    /// The browse strip is visible by default. Both On and Off are explicit
    /// saved choices; only an absent value follows the default.
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
    /// Local MiSTer Artwork Pack installations explicitly selected per
    /// supported data-source group. These take precedence over Gamelist
    /// choices in settings written manually or by older versions.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub artwork_pack_roots: BTreeMap<String, String>,
    /// Explicit Gamelist choices, including systems without gamelist.xml.
    /// Absence from both source settings means Automatic.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub gamelist_sources: BTreeSet<String>,
}

impl Settings {
    /// Resolve explicit bindings first, then the two compatible legacy flags.
    /// A and B had no legacy shortcut and therefore resolve to None.
    pub fn hold_shortcut(&self, button: HoldButton) -> HoldShortcut {
        match button {
            HoldButton::A => self.hold_a_shortcut.unwrap_or_default(),
            HoldButton::B => self.hold_b_shortcut.unwrap_or_default(),
            HoldButton::X => self.hold_x_shortcut.unwrap_or_else(|| {
                if self.hold_x_favorite.unwrap_or(false) {
                    HoldShortcut::AddRemoveFavourite
                } else {
                    HoldShortcut::None
                }
            }),
            HoldButton::Y => self.hold_y_shortcut.unwrap_or_else(|| {
                if self.hold_y_random.unwrap_or(false) {
                    HoldShortcut::RandomGame
                } else {
                    HoldShortcut::None
                }
            }),
        }
    }

    pub fn resolved_hold_shortcuts(&self) -> [HoldShortcut; 4] {
        HoldButton::ALL.map(|button| self.hold_shortcut(button))
    }

    pub fn set_hold_shortcut(&mut self, button: HoldButton, shortcut: HoldShortcut) {
        match button {
            HoldButton::A => self.hold_a_shortcut = Some(shortcut),
            HoldButton::B => self.hold_b_shortcut = Some(shortcut),
            HoldButton::X => {
                self.hold_x_shortcut = Some(shortcut);
                self.hold_x_favorite = Some(shortcut == HoldShortcut::AddRemoveFavourite);
            }
            HoldButton::Y => {
                self.hold_y_shortcut = Some(shortcut);
                self.hold_y_random = Some(shortcut == HoldShortcut::RandomGame);
            }
        }
    }

    /// Write explicit resolved bindings and keep the two legacy booleans
    /// truthful for an older Degauss release reading this settings file.
    fn with_resolved_hold_shortcuts(&self) -> Self {
        let mut resolved = self.clone();
        let shortcuts = self.resolved_hold_shortcuts();
        for button in HoldButton::ALL {
            resolved.set_hold_shortcut(button, shortcuts[button.index()]);
        }
        resolved.hold_x_favorite =
            Some(shortcuts[HoldButton::X.index()] == HoldShortcut::AddRemoveFavourite);
        resolved.hold_y_random = Some(shortcuts[HoldButton::Y.index()] == HoldShortcut::RandomGame);
        resolved
    }

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

    pub fn save(&self, path: &Path) -> Result<SaveOutcome> {
        self.save_with_directory_sync(path, |parent| {
            std::fs::File::open(parent).and_then(|directory| directory.sync_all())
        })
    }

    fn save_with_directory_sync(
        &self,
        path: &Path,
        sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
    ) -> Result<SaveOutcome> {
        let settings = self.with_resolved_hold_shortcuts();
        let text = toml::to_string_pretty(&settings)
            .map_err(|e| DegaussError::malformed("settings", path, e.to_string()))?;
        let body = format!(
            "# Written by Degauss when you change something in Options.\n\
             # The documented defaults live in degauss.toml; delete this file\n\
             # to go back to them.\n\n{text}"
        );
        install_with_directory_sync(&SETTINGS_LABELS, path, &body, sync_directory)
    }
}

/// The words each step of a save is reported under. One set per file
/// written beside the settings, so a failure names the file it was for.
pub(crate) struct SaveLabels {
    pub creating_temporary: &'static str,
    pub writing_temporary: &'static str,
    pub flushing_temporary: &'static str,
    /// The save as a whole: a path without a file name, a temporary file
    /// that could not be reserved, or one that could not be removed after
    /// a failure.
    pub writing: &'static str,
    /// The details under `writing` for the first two of those, so each
    /// writer's message names its own file.
    pub no_file_name: &'static str,
    pub could_not_reserve: &'static str,
    pub installing: &'static str,
    pub flushing_directory: &'static str,
}

const SETTINGS_LABELS: SaveLabels = SaveLabels {
    creating_temporary: "creating temporary settings",
    writing_temporary: "writing temporary settings",
    flushing_temporary: "flushing temporary settings",
    writing: "writing settings",
    no_file_name: "settings path has no file name",
    could_not_reserve: "could not reserve a temporary settings file",
    installing: "installing settings",
    flushing_directory: "flushing settings directory",
};

/// Write `body` to a temporary file beside `path`, flush it and move it
/// into place, then flush the directory so the move itself survives a
/// power cut. A file cut short is never read back as a shorter one. When
/// the directory flush fails the file is in place and the outcome says
/// what could not be confirmed. Shared with the other writer that installs
/// a file beside the settings.
pub(crate) fn install_beside_settings(
    labels: &SaveLabels,
    path: &Path,
    body: &str,
) -> Result<SaveOutcome> {
    install_with_directory_sync(labels, path, body, |parent| {
        std::fs::File::open(parent).and_then(|directory| directory.sync_all())
    })
}

fn install_with_directory_sync(
    labels: &SaveLabels,
    path: &Path,
    body: &str,
    sync_directory: impl FnOnce(&Path) -> std::io::Result<()>,
) -> Result<SaveOutcome> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| DegaussError::unsupported(labels.writing, labels.no_file_name))?;
    let (temporary, mut handle) = temporary_file(labels, parent, file_name)?;

    let write_result: Result<()> = (|| {
        handle
            .write_all(body.as_bytes())
            .map_err(|error| DegaussError::io(labels.writing_temporary, &temporary, error))?;
        handle
            .sync_all()
            .map_err(|error| DegaussError::io(labels.flushing_temporary, &temporary, error))?;
        Ok(())
    })();
    if let Err(error) = write_result {
        drop(handle);
        return Err(cleanup_temporary(labels.writing, &temporary, error));
    }
    drop(handle);

    if let Err(error) = std::fs::rename(&temporary, path) {
        let error = DegaussError::io(labels.installing, path, error);
        return Err(cleanup_temporary(labels.writing, &temporary, error));
    }

    match sync_directory(parent) {
        Ok(()) => Ok(SaveOutcome::Durable),
        Err(error) => Ok(SaveOutcome::InstalledWithWarning(DegaussError::io(
            labels.flushing_directory,
            parent,
            error,
        ))),
    }
}

/// The failure that stopped a save, with the leftover temporary file
/// removed; when even that fails the error says so under `what`, because
/// a stray file beside the settings is worth knowing about.
fn cleanup_temporary(what: &'static str, path: &Path, error: DegaussError) -> DegaussError {
    match std::fs::remove_file(path) {
        Ok(()) => error,
        Err(cleanup) if cleanup.kind() == std::io::ErrorKind::NotFound => error,
        Err(cleanup) => DegaussError::unsupported(
            what,
            format!(
                "{error}; removing the temporary file {} also failed: {cleanup}",
                path.display()
            ),
        ),
    }
}

/// A fresh temporary file beside the target: never an existing one, so a
/// leftover of another run is not written over and read back as this one.
fn temporary_file(
    labels: &SaveLabels,
    parent: &Path,
    file_name: &std::ffi::OsStr,
) -> Result<(PathBuf, std::fs::File)> {
    for attempt in 0..1000 {
        let mut temporary_name = OsString::from(".");
        temporary_name.push(file_name);
        temporary_name.push(format!(".degauss-{}-{attempt}.tmp", std::process::id()));
        let temporary = parent.join(temporary_name);
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(DegaussError::io(
                    labels.creating_temporary,
                    &temporary,
                    error,
                ));
            }
        }
    }
    Err(DegaussError::unsupported(
        labels.writing,
        labels.could_not_reserve,
    ))
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
        assert_eq!(settings.start_folder, None);
        assert_eq!(settings.show_game_position, None);
        assert!(settings.show_game_position.unwrap_or(true));
        assert_eq!(settings.theme_font_override, None);
        assert_eq!(settings.layout.as_deref(), Some("details"));
        assert!(settings.artwork_scale.is_none());
        assert!(
            settings.details_style.is_none(),
            "an older settings file must keep the Information layout it was drawn with"
        );
        assert_eq!(settings.game_name_display, None);
        assert_eq!(
            settings.game_name_display.unwrap_or_default(),
            GameNameDisplay::Full
        );
        assert_eq!(settings.folder_brackets, None);
        assert!(settings.folder_brackets.unwrap_or(true));
        assert_eq!(settings.overscan_x, Some(5));
        assert_eq!(settings.hidden, ["PDP1", "VC4000"]);
        assert_eq!(settings.folder_views.len(), 2);
        assert!(settings.custom_views.is_empty());
        assert_eq!(
            settings.hold_x_favorite, None,
            "an older settings file must leave the opt-in shortcut off"
        );
        assert_eq!(settings.hold_y_random, None);
        assert_eq!(settings.resolved_hold_shortcuts(), [HoldShortcut::None; 4]);
        assert!(
            settings.artwork_pack_roots.is_empty(),
            "v0.2.0 installations have no explicit Pack choices"
        );
        assert!(settings.gamelist_sources.is_empty());
        assert_eq!(settings.automatic_data_source, None);
        assert_eq!(
            settings.automatic_data_source.unwrap_or_default(),
            AutomaticDataSource::GamelistFirst
        );
        assert_eq!(settings.separate_handheld_category, None);
        assert_eq!(settings.show_cores, None);
        assert!(!settings.show_cores.unwrap_or(false));
        assert_eq!(settings.last_played, None);
        assert_eq!(settings.screen_rotation, None);
        assert_eq!(
            settings.screen_rotation.unwrap_or_default(),
            ScreenRotation::Off
        );
        assert_eq!(settings.show_misterzine, None);
        assert!(!settings.show_misterzine.unwrap_or(false));
    }

    #[test]
    fn settings_written_by_v0_3_0_still_load_without_freezing_new_defaults() {
        let text = include_str!("../tests/fixtures/v0.3.0-settings.toml");
        let settings: Settings = toml::from_str(text).expect("v0.3.0 settings must keep parsing");
        assert_eq!(settings.font.as_deref(), Some("pixel 2"));
        assert_eq!(settings.start_folder, None);
        assert_eq!(settings.show_game_position, None);
        assert!(settings.show_game_position.unwrap_or(true));
        assert_eq!(settings.theme_font_override, None);
        assert_eq!(settings.theme.as_deref(), Some("Blue-Orange"));
        assert_eq!(settings.left_right.as_deref(), Some("page"));
        assert_eq!(settings.layout.as_deref(), Some("carousel"));
        assert_eq!(settings.overscan_x, Some(4));
        assert_eq!(settings.hidden, ["PDP1"]);
        assert_eq!(settings.folder_views.len(), 2);
        assert!(settings.custom_views.is_empty());
        assert!(settings.artwork_scale.is_none());
        assert!(settings.details_style.is_none());
        assert_eq!(settings.game_name_display, None);
        assert_eq!(
            settings.game_name_display.unwrap_or_default(),
            GameNameDisplay::Full
        );
        assert_eq!(settings.folder_brackets, None);
        assert!(settings.folder_brackets.unwrap_or(true));
        assert_eq!(settings.last_played, None);
        assert_eq!(settings.hold_x_favorite, None);
        assert_eq!(settings.hold_y_random, None);
        assert_eq!(settings.resolved_hold_shortcuts(), [HoldShortcut::None; 4]);
        assert!(
            settings.artwork_pack_roots.is_empty(),
            "v0.3.0 installations have no explicit Pack choices"
        );
        assert!(settings.gamelist_sources.is_empty());
        assert_eq!(settings.automatic_data_source, None);
        assert_eq!(
            settings.automatic_data_source.unwrap_or_default(),
            AutomaticDataSource::GamelistFirst
        );
        assert_eq!(settings.show_cores, None);
        assert!(!settings.show_cores.unwrap_or(false));
        assert_eq!(settings.screen_rotation, None);
        assert_eq!(
            settings.screen_rotation.unwrap_or_default(),
            ScreenRotation::Off
        );
        assert_eq!(settings.show_misterzine, None);
        assert!(!settings.show_misterzine.unwrap_or(false));
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
            start_folder: Some("Computer".into()),
            artwork_scale: Some("4:3".into()),
            details_style: Some("large-artwork".into()),
            game_name_display: Some(GameNameDisplay::KeepRegionAndDiscIndex),
            folder_brackets: Some(false),
            show_game_position: Some(false),
            left_right: Some("letter".into()),
            font: Some("pixel".into()),
            theme_font_override: Some(true),
            theme: Some("amber".into()),
            hold_x_favorite: Some(true),
            hold_y_random: Some(true),
            hold_a_shortcut: Some(HoldShortcut::CycleView),
            hold_b_shortcut: Some(HoldShortcut::GameInformation),
            hold_x_shortcut: Some(HoldShortcut::AddRemoveFavourite),
            hold_y_shortcut: Some(HoldShortcut::RandomGame),
            custom_views: CustomViews {
                categories: Some("list".into()),
                systems: [("Console".into(), "tiled".into())].into(),
                games: [(
                    "PSX".into(),
                    [(
                        "d:/media/fat/games/PSX".into(),
                        "unknown-future-view".into(),
                    )]
                    .into(),
                )]
                .into(),
            },
            show_stats: Some(true),
            separate_handheld_category: Some(true),
            overscan_x: Some(24),
            artwork_pack_roots: [("SuperGrafx".into(), "/media/fat/docs".into())].into(),
            gamelist_sources: ["NES".into()].into(),
            automatic_data_source: Some(AutomaticDataSource::ArtworkPackFirst),
            screen_rotation: Some(ScreenRotation::CounterClockwise),
            ..Default::default()
        };
        settings.save(&path).expect("saved");

        let read = Settings::load(&path).expect("read back");
        assert_eq!(read, settings.with_resolved_hold_shortcuts());
        // Untouched values stay absent, so the documented defaults keep
        // applying rather than being frozen at whatever they were today.
        assert!(read.present.is_none());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn screen_rotation_is_explicit_and_rejects_unknown_values() {
        let absent: Settings = toml::from_str("").unwrap();
        assert_eq!(
            absent.screen_rotation.unwrap_or_default(),
            ScreenRotation::Off
        );

        let clockwise: Settings = toml::from_str("screen_rotation = \"clockwise\"\n").unwrap();
        assert_eq!(clockwise.screen_rotation, Some(ScreenRotation::Clockwise));
        assert!(toml::from_str::<Settings>("screen_rotation = \"sideways\"\n").is_err());
    }

    #[test]
    fn screen_rotation_cycles_both_ways_and_swaps_only_quarter_turn_dimensions() {
        assert_eq!(ScreenRotation::Off.step(1), ScreenRotation::Clockwise);
        assert_eq!(
            ScreenRotation::Clockwise.step(1),
            ScreenRotation::CounterClockwise
        );
        assert_eq!(
            ScreenRotation::CounterClockwise.step(1),
            ScreenRotation::Off
        );
        assert_eq!(
            ScreenRotation::Off.step(-1),
            ScreenRotation::CounterClockwise
        );
        assert_eq!(ScreenRotation::Off.logical_size(352, 240), (352, 240));
        assert_eq!(ScreenRotation::Clockwise.logical_size(352, 240), (240, 352));
        assert_eq!(
            ScreenRotation::CounterClockwise.logical_size(352, 240),
            (240, 352)
        );
    }

    #[test]
    fn replacing_settings_is_atomic_and_leaves_no_temporary_file() {
        let path = temp_path("replace");
        std::fs::write(&path, "show_stats = false\n").unwrap();
        Settings {
            show_stats: Some(true),
            ..Default::default()
        }
        .save(&path)
        .expect("replacement succeeds");

        assert_eq!(Settings::load(&path).unwrap().show_stats, Some(true));
        let parent = path.parent().unwrap();
        let prefix = format!(
            ".{}.degauss-{}-",
            path.file_name().unwrap().to_string_lossy(),
            std::process::id()
        );
        assert!(!std::fs::read_dir(parent).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(&prefix)
        }));
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_directory_flush_failure_reports_an_installed_file_without_rolling_it_back() {
        let path = temp_path("directory-flush");
        let settings = Settings {
            theme: Some("Saved Theme".to_string()),
            ..Default::default()
        };
        let outcome = settings
            .save_with_directory_sync(&path, |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::Unsupported,
                    "directory sync unavailable",
                ))
            })
            .expect("the replacement itself completed");

        match outcome {
            SaveOutcome::InstalledWithWarning(error) => {
                assert!(error.to_string().contains("flushing settings directory"));
            }
            SaveOutcome::Durable => panic!("the injected flush failure must be reported"),
        }
        assert_eq!(
            Settings::load(&path).expect("installed settings remain readable"),
            settings.with_resolved_hold_shortcuts(),
            "a post-install warning must not roll back the installed file"
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn legacy_hold_flags_migrate_only_to_the_equivalent_buttons() {
        let legacy: Settings =
            toml::from_str("hold_x_favorite = true\nhold_y_random = true\n").unwrap();
        assert_eq!(
            legacy.resolved_hold_shortcuts(),
            [
                HoldShortcut::None,
                HoldShortcut::None,
                HoldShortcut::AddRemoveFavourite,
                HoldShortcut::RandomGame,
            ]
        );

        let absent: Settings = toml::from_str("").unwrap();
        assert_eq!(absent.resolved_hold_shortcuts(), [HoldShortcut::None; 4]);

        let disabled: Settings =
            toml::from_str("hold_x_favorite = false\nhold_y_random = false\n").unwrap();
        assert_eq!(disabled.resolved_hold_shortcuts(), [HoldShortcut::None; 4]);
    }

    #[test]
    fn explicit_hold_bindings_win_over_legacy_flags() {
        let settings: Settings = toml::from_str(
            "hold_x_favorite = true\n\
             hold_y_random = true\n\
             hold_x_shortcut = 'none'\n\
             hold_y_shortcut = 'cycle-view'\n",
        )
        .unwrap();
        assert_eq!(settings.hold_shortcut(HoldButton::X), HoldShortcut::None);
        assert_eq!(
            settings.hold_shortcut(HoldButton::Y),
            HoldShortcut::CycleView
        );
    }

    #[test]
    fn every_face_button_accepts_every_shortcut_without_changing_the_others() {
        for button in HoldButton::ALL {
            for shortcut in HoldShortcut::ALL {
                let mut settings = Settings::default();
                settings.set_hold_shortcut(button, shortcut);
                for candidate in HoldButton::ALL {
                    assert_eq!(
                        settings.hold_shortcut(candidate),
                        if candidate == button {
                            shortcut
                        } else {
                            HoldShortcut::None
                        }
                    );
                }
            }
        }
    }

    #[test]
    fn saving_materialises_bindings_and_keeps_legacy_flags_truthful() {
        let path = temp_path("hold-migration");
        let settings = Settings {
            hold_x_favorite: Some(true),
            hold_y_random: Some(true),
            hold_a_shortcut: Some(HoldShortcut::SearchThisFolder),
            hold_x_shortcut: Some(HoldShortcut::RandomFavourite),
            ..Default::default()
        };
        settings.save(&path).unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let saved = Settings::load(&path).unwrap();
        assert_eq!(
            saved.resolved_hold_shortcuts(),
            [
                HoldShortcut::SearchThisFolder,
                HoldShortcut::None,
                HoldShortcut::RandomFavourite,
                HoldShortcut::RandomGame,
            ]
        );
        assert_eq!(saved.hold_x_favorite, Some(false));
        assert_eq!(saved.hold_y_random, Some(true));
        for key in [
            "hold_a_shortcut",
            "hold_b_shortcut",
            "hold_x_shortcut",
            "hold_y_shortcut",
        ] {
            assert!(text.contains(key), "{key} was not materialised");
        }
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn hold_shortcut_order_and_labels_are_stable() {
        let mut shortcut = HoldShortcut::None;
        let labels: Vec<_> = (0..HoldShortcut::ALL.len())
            .map(|_| {
                let label = shortcut.label();
                shortcut = shortcut.step(1);
                label
            })
            .collect();
        assert_eq!(shortcut, HoldShortcut::None);
        assert_eq!(
            labels,
            [
                "None",
                "Cycle View",
                "Random Game",
                "Random Favourite",
                "Add/Remove Favourite",
                "Game Information",
                "Search This Folder",
                "Jump to Letter",
                "Actions",
                "Menu",
            ]
        );
        assert_eq!(HoldShortcut::None.step(-1), HoldShortcut::Menu);
    }

    #[test]
    fn a_failed_atomic_install_preserves_the_occupied_destination() {
        let root = std::env::temp_dir().join(format!(
            "degauss-settings-install-failure-{}",
            std::process::id()
        ));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir(&root).unwrap();
        let path = root.join("settings.toml");
        std::fs::create_dir(&path).unwrap();
        std::fs::write(path.join("marker"), "unchanged").unwrap();

        assert!(Settings::default().save(&path).is_err());
        assert_eq!(
            std::fs::read_to_string(path.join("marker")).unwrap(),
            "unchanged"
        );
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            1,
            "a failed install must remove its temporary file only"
        );
        std::fs::remove_dir_all(&root).ok();
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

    #[test]
    fn old_settings_keep_standard_preference_and_show_nightlies() {
        let old: Settings =
            toml::from_str(include_str!("../tests/fixtures/v0.2.0-settings.toml")).unwrap();
        assert!(old.show_other.unwrap());
        assert!(!Settings::default().show_other.unwrap_or(false));
        for settings in [Settings::default(), old] {
            assert!(settings.show_unstable.unwrap_or(true));
            assert_eq!(
                settings.core_preference.unwrap_or_default(),
                CorePreference::StandardFirst
            );
            assert!(!settings.show_utility.unwrap_or(false));
        }
    }

    #[test]
    fn nightly_visibility_and_core_preference_round_trip_without_hiding_items() {
        let settings = Settings {
            show_unstable: Some(false),
            core_preference: Some(CorePreference::RetroAchievementsFirst),
            hidden: vec!["NES".into()],
            hidden_paths: vec!["d:/games/example".into()],
            ..Settings::default()
        };
        let encoded = toml::to_string(&settings).unwrap();
        let decoded: Settings = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.show_unstable, Some(false));
        assert_eq!(
            decoded.core_preference,
            Some(CorePreference::RetroAchievementsFirst)
        );
        assert_eq!(decoded.hidden, settings.hidden);
        assert_eq!(decoded.hidden_paths, settings.hidden_paths);
        assert_eq!(
            CorePreference::StandardFirst.next().next(),
            CorePreference::StandardFirst
        );
    }

    #[test]
    fn automatic_data_source_cycles_and_old_settings_keep_gamelist_first() {
        let old: Settings =
            toml::from_str(include_str!("../tests/fixtures/v0.3.0-settings.toml")).unwrap();
        assert_eq!(old.automatic_data_source, None);
        assert_eq!(
            old.automatic_data_source.unwrap_or_default(),
            AutomaticDataSource::GamelistFirst
        );
        assert_eq!(
            AutomaticDataSource::GamelistFirst.next(),
            AutomaticDataSource::ArtworkPackFirst
        );
        assert_eq!(
            AutomaticDataSource::ArtworkPackFirst.next(),
            AutomaticDataSource::GamelistFirst
        );
    }

    #[test]
    fn per_system_core_choices_preserve_legacy_defaults_and_exact_nightly_paths() {
        let old: Settings =
            toml::from_str(include_str!("../tests/fixtures/v0.2.0-settings.toml")).unwrap();
        assert!(old.core_choices.is_empty());
        assert!(!toml::to_string(&Settings::default())
            .unwrap()
            .contains("core_choices"));
        let mut settings = Settings::default();
        settings.core_choices.insert(
            "NES".into(),
            "_Unstable/NES_unstable_20260907_a1b2.rbf".into(),
        );
        settings.core_choices.insert("SMS".into(), "ra".into());
        let encoded = toml::to_string(&settings).unwrap();
        let decoded: Settings = toml::from_str(&encoded).unwrap();
        assert_eq!(decoded.core_choices, settings.core_choices);
        assert_eq!(decoded.core_preference, None);
    }

    #[test]
    fn per_system_launch_cores_are_optional_stable_profile_ids() {
        let old: Settings =
            toml::from_str(include_str!("../tests/fixtures/v0.2.0-settings.toml")).unwrap();
        assert!(old.launch_cores.is_empty());
        assert!(!toml::to_string(&Settings::default())
            .unwrap()
            .contains("launch_cores"));
        let mut settings = Settings::default();
        settings
            .launch_cores
            .insert("NeoGeoPocketColor".into(), "jtngpc".into());
        let decoded: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(decoded.launch_cores, settings.launch_cores);
        assert!(decoded.core_choices.is_empty());
    }

    #[test]
    fn per_game_launch_cores_are_sparse_and_backward_compatible() {
        let old: Settings =
            toml::from_str(include_str!("../tests/fixtures/v0.2.0-settings.toml")).unwrap();
        assert!(old.game_launch_cores.is_empty());
        assert!(!toml::to_string(&Settings::default())
            .unwrap()
            .contains("game_launch_cores"));

        let mut settings = Settings::default();
        settings.game_launch_cores.insert(
            "N64".into(),
            [("f:/media/fat/games/N64/Game.z64".into(), "n64-80mhz".into())].into(),
        );
        let decoded: Settings = toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(decoded.game_launch_cores, settings.game_launch_cores);
        assert!(decoded.launch_cores.is_empty());
    }

    #[test]
    fn a_temporary_file_that_cannot_be_removed_is_part_of_the_reported_failure() {
        let dir =
            std::env::temp_dir().join(format!("degauss-settings-stuck-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let stuck = dir.join("stuck.tmp");
        std::fs::create_dir_all(&stuck).unwrap();
        let error = cleanup_temporary(
            "writing settings",
            &stuck,
            DegaussError::unsupported("writing settings", "the save failed"),
        );
        let text = error.to_string();
        assert!(
            text.contains("the save failed") && text.contains("also failed"),
            "both failures must reach the screen, or a stray file goes unexplained: {text}"
        );
        assert!(text.contains(&stuck.display().to_string()));
        let gone = cleanup_temporary(
            "writing artwork pack warnings",
            &dir.join("never-written.tmp"),
            DegaussError::unsupported("writing artwork pack warnings", "the save failed"),
        );
        assert_eq!(
            gone.to_string(),
            "writing artwork pack warnings unsupported: the save failed",
            "a temporary file that was never written is nothing to report, whichever writer asks"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_path_without_a_file_name_is_refused_in_each_writers_own_words() {
        // The install is shared, the words are not: a failure must name the
        // file it was for, and the settings writer must say exactly what it
        // said before the install was shared, so nothing reads as a new
        // failure after an update.
        let error = Settings::default().save(Path::new("/")).unwrap_err();
        assert_eq!(
            error.to_string(),
            "writing settings unsupported: settings path has no file name"
        );
        let error = crate::pack_health::Acknowledgements::default()
            .save(Path::new("/"))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "writing artwork pack warnings unsupported: artwork pack warnings path has no file name"
        );
    }
}
