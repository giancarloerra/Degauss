//! Configuration: which system to browse, how to launch it, and the
//! colour palette.
//!
//! The per-system block follows the `es_systems.xml` split used by the
//! EmulationStation family: per-game metadata in `gamelist.xml`, per-system
//! launch knowledge in a separate systems file. This is that file, in TOML,
//! with MiSTer values (core `.rbf` path, and the `index`/`type`/`delay` an
//! `.mgl` needs) instead of an emulator command line.

use std::path::Path;

use serde::de::{self, Deserializer};
use serde::Deserialize;

use crate::error::{DegaussError, Result};

/// 24-bit colour parsed from `"#rrggbb"` at load time, so a typo in the
/// palette fails when the file is read rather than painting something wrong.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Color { r, g, b }
    }

    pub fn parse(text: &str) -> std::result::Result<Self, String> {
        let hex = text.strip_prefix('#').unwrap_or(text);
        if hex.len() != 6 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(format!("expected #rrggbb, got {text:?}"));
        }
        let value = u32::from_str_radix(hex, 16).map_err(|e| e.to_string())?;
        Ok(Color::new(
            ((value >> 16) & 0xff) as u8,
            ((value >> 8) & 0xff) as u8,
            (value & 0xff) as u8,
        ))
    }
}

impl<'de> Deserialize<'de> for Color {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Color::parse(&text).map_err(de::Error::custom)
    }
}

/// Palette. Names are roles, not hues, so a theme can invert freely.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Colors {
    pub background: Color,
    pub panel: Color,
    pub text: Color,
    pub text_dim: Color,
    /// Selection bar / focus ring.
    pub accent: Color,
    /// Text drawn on top of `accent`.
    pub accent_text: Color,
    pub favorite: Color,
    /// State and activity: toggles, progress, anything that is "on".
    #[serde(default = "default_state")]
    pub state: Color,
    /// Raised surfaces: cards, the artwork plate, modal panels.
    #[serde(default = "default_surface")]
    pub surface: Color,
    /// The strip along the bottom. Darker than everything else so the small
    /// text on it, and the teal in particular, stays readable.
    #[serde(default = "default_bar")]
    pub bar: Color,
}

fn default_state() -> Color {
    Color::new(0x03, 0xa4, 0x9d)
}

fn default_bar() -> Color {
    Color::new(0x2a, 0x2a, 0x2a)
}

fn default_surface() -> Color {
    Color::new(0x49, 0x49, 0x49)
}

impl Default for Colors {
    /// The Degauss palette, in the values the card already runs: a mid-grey
    /// interface carrying the three colours from the wordmark, each in one
    /// role and no more. Yellow marks what is selected, teal marks state,
    /// red marks favourites.
    ///
    /// Grey rather than black on purpose. This is drawn for a CRT over
    /// analog RGB, where a black interface leaves nothing for the tube to
    /// key off and every panel edge disappears into the background; a lifted
    /// ground keeps the layout readable and stops black text areas blooming.
    fn default() -> Self {
        Colors {
            background: Color::new(0x6c, 0x6c, 0x6c),
            panel: Color::new(0x3a, 0x3a, 0x3a),
            text: Color::new(0xff, 0xff, 0xff),
            text_dim: Color::new(0xc4, 0xc4, 0xc4),
            accent: Color::new(0xff, 0xcd, 0x09),
            accent_text: Color::new(0x11, 0x11, 0x11),
            favorite: Color::new(0xfe, 0x2e, 0x1d),
            state: Color::new(0x03, 0xa4, 0x9d),
            surface: Color::new(0x49, 0x49, 0x49),
            bar: Color::new(0x2a, 0x2a, 0x2a),
        }
    }
}

/// How to build an `.mgl` for a given file extension.
///
/// `type`/`index`/`delay` are MiSTer's own MGL semantics: which slot of the
/// core the file is loaded into, and how long to wait after the core starts.
#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct LaunchRule {
    /// Extensions this rule covers, lowercase and without the dot.
    pub extensions: Vec<String>,
    #[serde(rename = "type")]
    pub kind: String,
    pub index: u8,
    pub delay: u8,
    /// Some cores need a reset after the file is handed over, or the game
    /// never starts. When set, the MGL carries a `<reset>` action after the
    /// file: wait this many seconds, then pulse reset.
    #[serde(default)]
    pub reset_delay: Option<u8>,
    /// How long to hold that reset. MiSTer defaults to a brief pulse when
    /// this is absent.
    #[serde(default)]
    pub reset_hold: Option<u8>,
    /// Extensions of a second file to mount alongside this one, when one is
    /// sitting in the same folder.
    ///
    /// A DOS game is a hard disk image and, often, a CD image beside it. The
    /// game boots from the disk and then asks for its CD, so mounting only
    /// what was selected starts a game that immediately cannot find itself.
    /// MiSTer's own shortcuts on a card mount both.
    #[serde(default)]
    pub companion_extensions: Vec<String>,
    /// The slot that companion goes into.
    #[serde(default)]
    pub companion_index: Option<u8>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SystemConfig {
    pub name: String,
    /// Absolute path to the folder holding the games.
    pub path: String,
    /// Extensions that count as launchable, lowercase and without the dot.
    pub extensions: Vec<String>,
    /// Core to load, in MiSTer's own form, e.g. `_Computer/C64`.
    pub rbf: String,
    #[serde(default)]
    pub launch: Vec<LaunchRule>,
    /// Set name for a core that presents itself as several systems.
    #[serde(default)]
    pub setname: Option<String>,
    /// Folder names the scan skips for this system.
    #[serde(default)]
    pub skip_folders: Vec<String>,
    /// Further folders holding this system's games, beyond `path`.
    #[serde(default)]
    pub extra_paths: Vec<String>,
}

impl SystemConfig {
    /// True when a file is one this system lists.
    pub fn accepts(&self, path: &Path) -> bool {
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            return false;
        };
        let ext = ext.to_ascii_lowercase();
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext))
    }

    /// The launch rule covering a file, if the config declares one.
    pub fn rule_for(&self, path: &Path) -> Option<&LaunchRule> {
        let ext = path
            .extension()
            .and_then(|e| e.to_str())?
            .to_ascii_lowercase();
        self.launch
            .iter()
            .find(|rule| rule.extensions.iter().any(|e| e.eq_ignore_ascii_case(&ext)))
    }
}

/// The `[app]` table of `degauss.toml`: artwork size and cache, the scroll
/// speed past which artwork is skipped, the starting view, the performance
/// readout and the overscan margins.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AppSettings {
    /// Longest edge a cover is scaled to before caching, in pixels. Raised
    /// automatically to at least half the framebuffer's larger side, since
    /// the preview layout draws artwork that big.
    #[serde(default = "default_cover_size")]
    pub cover_size: u32,
    /// How many decoded pictures to hold in memory.
    ///
    /// Two hundred is about 44 MB, and holds a large folder without
    /// evicting. Nothing is written to the card.
    #[serde(default = "default_art_cache")]
    pub art_cache: usize,
    /// The fastest scroll speed that still loads a picture for every row,
    /// as a step on the speed ladder.
    ///
    /// Above this step artwork is skipped while the list moves, so the
    /// scrolling stays smooth.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    #[serde(default = "default_art_limit")]
    pub art_limit: usize,
    /// Show the performance readout instead of the key hints.
    #[serde(default)]
    pub show_stats: bool,
    /// Which view to open in: "details", "tiled", "list" or "carousel".
    #[serde(default = "default_layout")]
    pub layout: String,
    /// What left and right do while browsing: "speed", "letter", "page" or
    /// "direction".
    ///
    /// The shipped degauss.toml documents this key as a commented line
    /// rather than an active one: `[app]` rejects keys it does not know,
    /// so an active key that older versions never heard of would stop
    /// them from starting after a downgrade. The live value is written to
    /// settings.toml by the Options screen, which tolerates unknown keys.
    #[serde(default = "default_left_right")]
    pub left_right: String,
    /// Which typeface to set the interface in: "smooth" or "pixel".
    #[serde(default = "default_font")]
    pub font: String,
    /// How much of the screen edge to keep clear, as a percentage of each
    /// axis.
    ///
    /// A television crops the edges of the picture. Without this the top
    /// and bottom bars land under the bezel and the only fix is turning on
    /// underscan, which shrinks everything. Five percent is a common safe
    /// margin; set it to zero on a display that shows every pixel.
    #[serde(default = "default_overscan")]
    pub overscan_x: u32,
    #[serde(default = "default_overscan")]
    pub overscan_y: u32,
}

fn default_layout() -> String {
    "details".to_string()
}

fn default_left_right() -> String {
    "speed".to_string()
}

fn default_font() -> String {
    crate::font::Font::default().label().to_string()
}

fn default_overscan() -> u32 {
    5
}

fn default_cover_size() -> u32 {
    160
}

fn default_art_cache() -> usize {
    200
}

/// Three times the baseline rate: step 3 of the ladder in `input.rs`. Fast
/// enough to cover normal browsing with a picture on every row, slow enough
/// that the decoding keeps up.
fn default_art_limit() -> usize {
    3
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub app: AppSettings,
    #[serde(default)]
    pub colors: Colors,
    /// Where to look for game folders.
    #[serde(default = "default_roots")]
    pub game_roots: Vec<String>,
    /// The top of the card, where the `_`-prefixed menu folders live. Read
    /// to find which group each installed core belongs to.
    #[serde(default = "default_menu_root")]
    pub menu_root: String,
}

fn default_menu_root() -> String {
    "/media/fat".to_string()
}

fn default_roots() -> Vec<String> {
    vec![
        "/media/fat/games".to_string(),
        // Arcade lives at /media/fat/_Arcade, outside the games folder.
        "/media/fat".to_string(),
    ]
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| DegaussError::io("reading config", path, e))?;
        Self::parse(&text, path)
    }

    pub fn parse(text: &str, origin: &Path) -> Result<Self> {
        let config: Config = toml::from_str(text)
            .map_err(|e| DegaussError::malformed("config", origin, e.to_string()))?;
        // Validated like the colours: a value nothing answers to would
        // otherwise silently mean the default, which reads as the
        // setting not working.
        if !LEFT_RIGHT_VALUES.contains(&config.app.left_right.as_str()) {
            return Err(DegaussError::malformed(
                "config",
                origin,
                format!(
                    "left_right {:?}: the choices are speed, letter, page and direction",
                    config.app.left_right
                ),
            ));
        }
        Ok(config)
    }
}

/// The words `left_right` accepts, the config-side twin of the modes the
/// interface steps through; a test in app.rs holds the two together.
pub const LEFT_RIGHT_VALUES: [&str; 4] = ["speed", "letter", "page", "direction"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_left_right_value_nothing_answers_to_is_refused_by_name() {
        // Silently meaning the default would read as the setting not
        // working; the refusal names the word and the choices.
        let err = Config::parse("[app]\nleft_right = \"leftr\"\n", Path::new("t"))
            .expect_err("a typo'd mode must not parse");
        let text = format!("{err}");
        assert!(text.contains("leftr"), "got: {text}");
        assert!(text.contains("letter"), "the choices are named: {text}");
    }

    #[test]
    fn a_config_from_every_release_that_used_the_old_folder_still_parses() {
        // The migration moves a user-edited degauss.toml over the shipped
        // one, so a config first written for v0.1.0 or v0.2.0 must stay
        // valid in every later version. These are the exact shipped bytes;
        // an edited file differs only in values and in keys REMOVED, and
        // removals only make it more permissive. What this pins is the
        // other direction: renaming or deleting a key in the current code
        // fails here before it can ship.
        for (tag, text) in [
            (
                "v0.1.0",
                include_str!("../tests/fixtures/v0.1.0-degauss.toml"),
            ),
            (
                "v0.2.0",
                include_str!("../tests/fixtures/v0.2.0-degauss.toml"),
            ),
        ] {
            Config::parse(text, Path::new(tag))
                .unwrap_or_else(|e| panic!("{tag} degauss.toml must keep parsing: {e}"));
        }
    }

    const SAMPLE: &str = r##"
[app]
overscan_x = 4
overscan_y = 6

[colors]
background = "#000000"
panel = "#111111"
text = "#ffffff"
text_dim = "#888888"
accent = "#ffcd09"
accent_text = "#03132d"
favorite = "#fe2e1d"
"##;

    fn parse(text: &str) -> Result<Config> {
        Config::parse(text, Path::new("degauss.toml"))
    }

    #[test]
    fn a_config_parses_and_fills_in_documented_defaults() {
        let config = parse(SAMPLE).expect("sample config parses");
        assert_eq!(config.colors.accent, Color::new(0xff, 0xcd, 0x09));
        assert_eq!(config.app.cover_size, 160, "defaults fill in");
        assert_eq!(config.app.layout, "details");
        assert_eq!(config.app.left_right, "speed");
        assert_eq!(config.app.font, "smooth");
        assert!(
            config.game_roots.iter().any(|r| r == "/media/fat/games"),
            "the usual games root applies without being written out"
        );
    }

    #[test]
    fn a_bad_colour_is_rejected_with_the_offending_value() {
        let text = SAMPLE.replace(r##"accent = "#ffcd09""##, r##"accent = "yellow""##);
        let err = parse(&text).expect_err("must reject a non-hex colour");
        assert!(err.to_string().contains("rrggbb"), "got: {err}");
    }

    #[test]
    fn unknown_keys_are_rejected_rather_than_ignored() {
        // A typo'd key that silently does nothing is how a setting "does not
        // apply" with no explanation.
        let text = SAMPLE.replace("[app]", "[app]\nrowz = 4");
        assert!(parse(&text).is_err());
    }

    #[test]
    fn extension_matching_is_case_insensitive_both_ways() {
        let system = SystemConfig {
            name: "C64".into(),
            path: "/games/C64".into(),
            extensions: vec!["d64".into(), "prg".into()],
            rbf: "_Computer/C64".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
            extra_paths: Vec::new(),
        };
        assert!(system.accepts(Path::new("/x/Game.D64")));
        assert!(system.accepts(Path::new("/x/game.prg")));
        assert!(!system.accepts(Path::new("/x/game.zip")));
        assert!(!system.accepts(Path::new("/x/noextension")));
    }

    #[test]
    fn the_launch_rule_for_a_file_comes_from_its_extension() {
        let system = SystemConfig {
            name: "C64".into(),
            path: "/games/C64".into(),
            extensions: vec!["d64".into(), "prg".into()],
            rbf: "_Computer/C64".into(),
            launch: vec![LaunchRule {
                extensions: vec!["d64".into()],
                kind: "s".into(),
                index: 0,
                delay: 1,
                reset_delay: None,
                reset_hold: None,
                companion_extensions: Vec::new(),
                companion_index: None,
            }],
            skip_folders: Vec::new(),
            setname: None,
            extra_paths: Vec::new(),
        };
        let rule = system.rule_for(Path::new("/x/Game.D64")).expect("d64 rule");
        assert_eq!(rule.kind, "s");
        assert_eq!(rule.index, 0);
        assert!(
            system.rule_for(Path::new("/x/Game.prg")).is_none(),
            "an extension with no rule must report none, not a guessed default"
        );
    }
}
