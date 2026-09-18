//! The Options pages.
//!
//! Pages share the same setting identities, labels and value handlers.
//!
//! For ordered values left chooses the previous value and right or A chooses
//! the next; Boolean and two-choice values toggle with any of them. Action
//! rows respond only to A. Changes are written to `settings.toml` when leaving
//! the screen.

use crate::input::SPEED_STEPS;
use crate::settings::HoldButton;

/// One adjustable setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionId {
    Speed,
    /// What left and right do while browsing: speed, letter, page or
    /// direction.
    LeftRight,
    /// The scroll speed above which artwork stops being loaded per row.
    ArtLimit,
    Layout,
    /// Remove every place-specific view after confirmation.
    ResetCustomViews,
    /// How Details shares its width between the list and the picture, and
    /// whether the compact lines sit under the picture.
    DetailsStyle,
    /// Runtime-only formatting of the final effective game or folder name.
    GameNameDisplay,
    /// Whether folder rows receive Degauss's outer square-bracket marker.
    FolderBrackets,
    /// Which typeface the interface is set in.
    Font,
    /// Which named palette from the themes folder is on, if any.
    Theme,
    ShowArt,
    /// Correct game artwork for the physical aspect ratio of a display
    /// whose framebuffer pixels are not square.
    ArtworkScale,
    ShowHidden,
    ShowEmpty,
    /// Show the group holding the cores that are not games.
    ShowOther,
    /// Show the group holding the test and measurement cores.
    ShowUtility,
    /// Show the cached browser of installed cores.
    ShowCores,
    /// Show the collection of nightly cores.
    ShowUnstable,
    ShowScripts,
    /// Preferred installed variant for ordinary game launches.
    CorePreference,
    /// Preferred available source for systems left on Automatic.
    AutomaticDataSource,
    /// Present recognised handheld systems in their own home category.
    SeparateHandheldCategory,
    /// The strip along the bottom of the screen, while browsing.
    ShowBar,
    /// Read the card again into the written-down copy of it.
    RebuildCache,
    /// Open ScreenScraper for every supported system on the card.
    ScrapeAll,
    /// Gather favourites at the top of a folder.
    FavoritesFirst,
    /// How many retained launches the optional top-level collection shows.
    LastPlayed,
    /// Browsing action assigned to a one-second face-button hold.
    HoldA,
    HoldB,
    HoldX,
    HoldY,
    RandomLaunches,
    /// List folders after the games rather than before them.
    FoldersLast,
    /// Show everything that has been hidden, in every folder, again.
    ResetHidden,
    ShowStats,
    Present,
    /// Turn the complete frontend while leaving the MiSTer video mode alone.
    ScreenRotation,
    OverscanX,
    OverscanY,
    /// Move the whole picture sideways, for a screen that is not centred.
    ShiftX,
    ShiftY,
    /// How long the machine is left alone before pictures take the screen.
    Screensaver,
    /// Not a setting: the door to the developer list.
    #[allow(dead_code)]
    Advanced,
    /// Not a setting: a blank line separating one group from the next.
    /// There to be read, never chosen; the cursor steps over it.
    Spacer,
}

/// The original flat ordering, retained for existing callers.
#[allow(dead_code)]
pub const OPTIONS: &[OptionId] = &[
    OptionId::Speed,
    OptionId::ArtLimit,
    OptionId::LeftRight,
    OptionId::Spacer,
    OptionId::HoldA,
    OptionId::HoldB,
    OptionId::HoldX,
    OptionId::HoldY,
    OptionId::RandomLaunches,
    OptionId::Spacer,
    OptionId::Theme,
    OptionId::Layout,
    OptionId::ResetCustomViews,
    OptionId::DetailsStyle,
    OptionId::GameNameDisplay,
    OptionId::FolderBrackets,
    OptionId::Font,
    OptionId::ShowArt,
    OptionId::ArtworkScale,
    OptionId::ShowBar,
    OptionId::Spacer,
    OptionId::FavoritesFirst,
    OptionId::LastPlayed,
    OptionId::FoldersLast,
    OptionId::CorePreference,
    OptionId::AutomaticDataSource,
    OptionId::SeparateHandheldCategory,
    OptionId::Spacer,
    OptionId::ShowOther,
    OptionId::ShowUtility,
    OptionId::ShowCores,
    OptionId::ShowUnstable,
    OptionId::ShowScripts,
    OptionId::ShowEmpty,
    OptionId::ShowHidden,
    OptionId::ResetHidden,
    OptionId::Spacer,
    OptionId::ScreenRotation,
    OptionId::OverscanX,
    OptionId::OverscanY,
    OptionId::ShiftX,
    OptionId::ShiftY,
    OptionId::Spacer,
    OptionId::Screensaver,
    OptionId::RebuildCache,
    OptionId::ScrapeAll,
    OptionId::Spacer,
    OptionId::Advanced,
];

/// Tuning and diagnostics: things you set once, or only while measuring.
/// Kept behind a door so the main list stays about using the thing.
pub const ADVANCED: [OptionId; 2] = [OptionId::Present, OptionId::ShowStats];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OptionsPage {
    Navigation,
    Shortcuts,
    Appearance,
    Library,
    Display,
    Developer,
}

impl OptionsPage {
    pub const ALL: [Self; 6] = [
        Self::Navigation,
        Self::Shortcuts,
        Self::Appearance,
        Self::Library,
        Self::Display,
        Self::Developer,
    ];

    pub const fn index(self) -> usize {
        match self {
            Self::Navigation => 0,
            Self::Shortcuts => 1,
            Self::Appearance => 2,
            Self::Library => 3,
            Self::Display => 4,
            Self::Developer => 5,
        }
    }

    pub const fn key(self) -> &'static str {
        match self {
            Self::Navigation => "navigation",
            Self::Shortcuts => "shortcuts",
            Self::Appearance => "appearance",
            Self::Library => "library",
            Self::Display => "display",
            Self::Developer => "developer",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|page| page.key() == text)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Navigation => "Navigation",
            Self::Shortcuts => "Shortcuts",
            Self::Appearance => "Appearance",
            Self::Library => "Library",
            Self::Display => "Display",
            Self::Developer => "Developer",
        }
    }

    pub const fn help(self) -> &'static str {
        match self {
            Self::Navigation => "Scrolling, directional controls and random-game behaviour.",
            Self::Shortcuts => "Choose optional one-second holds for the four face buttons.",
            Self::Appearance => "Theme, global view, text, artwork and the screensaver.",
            Self::Library => "Ordering, visible systems, core preference and library maintenance.",
            Self::Display => "Fit the picture to the screen with margins and position controls.",
            Self::Developer => "Drawing path and frame timing diagnostics.",
        }
    }

    pub const fn ids(self) -> &'static [OptionId] {
        match self {
            Self::Navigation => &[
                OptionId::Speed,
                OptionId::ArtLimit,
                OptionId::LeftRight,
                OptionId::RandomLaunches,
            ],
            Self::Shortcuts => &[
                OptionId::HoldA,
                OptionId::HoldB,
                OptionId::HoldX,
                OptionId::HoldY,
            ],
            Self::Appearance => &[
                OptionId::Theme,
                OptionId::Layout,
                OptionId::ResetCustomViews,
                OptionId::DetailsStyle,
                OptionId::GameNameDisplay,
                OptionId::FolderBrackets,
                OptionId::Font,
                OptionId::ShowArt,
                OptionId::ArtworkScale,
                OptionId::ShowBar,
                OptionId::Screensaver,
            ],
            Self::Library => &[
                OptionId::FavoritesFirst,
                OptionId::LastPlayed,
                OptionId::FoldersLast,
                OptionId::CorePreference,
                OptionId::AutomaticDataSource,
                OptionId::SeparateHandheldCategory,
                OptionId::ShowOther,
                OptionId::ShowUtility,
                OptionId::ShowCores,
                OptionId::ShowUnstable,
                OptionId::ShowScripts,
                OptionId::ShowEmpty,
                OptionId::ShowHidden,
                OptionId::ResetHidden,
                OptionId::RebuildCache,
                OptionId::ScrapeAll,
            ],
            Self::Display => &[
                OptionId::ScreenRotation,
                OptionId::OverscanX,
                OptionId::OverscanY,
                OptionId::ShiftX,
                OptionId::ShiftY,
            ],
            Self::Developer => &ADVANCED,
        }
    }
}

impl OptionId {
    pub fn label(self) -> &'static str {
        match self {
            OptionId::Speed => "Scroll Speed",
            OptionId::LeftRight => "Left and Right Behaviour",
            OptionId::ArtLimit => "Skip Artwork Faster Than",
            OptionId::Layout => "View",
            OptionId::ResetCustomViews => "Reset All Custom Views",
            OptionId::DetailsStyle => "Details Style",
            OptionId::GameNameDisplay => "Game Name Display",
            OptionId::FolderBrackets => "Folder Brackets",
            OptionId::Font => "Text",
            OptionId::Theme => "Theme",
            OptionId::ShowArt => "Artwork",
            OptionId::ArtworkScale => "Artwork Scale Factor",
            OptionId::ShowHidden => "Show What You Hid",
            OptionId::ShowEmpty => "Show Systems with No Games",
            OptionId::ShowOther => "Show Other Folder",
            OptionId::ShowUtility => "Show Utility Folder",
            OptionId::ShowCores => "Show Cores",
            OptionId::ShowUnstable => "Show Unstable Folder",
            OptionId::ShowScripts => "Show Scripts Folder",
            OptionId::CorePreference => "Core Preference",
            OptionId::AutomaticDataSource => "Automatic Data Source",
            OptionId::SeparateHandheldCategory => "Separate Handheld Category",
            OptionId::ShowBar => "Bottom Bar While Browsing",
            OptionId::RebuildCache => "Rebuild All System Lists",
            OptionId::ScrapeAll => "Scrape All Systems",
            OptionId::FavoritesFirst => "Favourites First",
            OptionId::LastPlayed => "Last Played",
            OptionId::HoldA => "Hold A",
            OptionId::HoldB => "Hold B",
            OptionId::HoldX => "Hold X",
            OptionId::HoldY => "Hold Y",
            OptionId::RandomLaunches => "Random Game Behaviour",
            OptionId::FoldersLast => "Folders Before Games",
            OptionId::ResetHidden => "Unhide Everything",
            OptionId::ShowStats => "Performance Readout",
            OptionId::Present => "Drawing Path",
            OptionId::ScreenRotation => "Screen Rotation",
            OptionId::OverscanX => "Edge Margin, Sides",
            OptionId::OverscanY => "Edge Margin, Top and Bottom",
            OptionId::ShiftX => "Screen Position, Sideways",
            OptionId::ShiftY => "Screen Position, Up and Down",
            OptionId::Screensaver => "Screensaver",
            OptionId::Advanced => "Developer",
            OptionId::Spacer => "",
        }
    }

    /// A line of explanation, shown under the list. Settings nobody can
    /// explain are settings nobody should have.
    pub fn help(self) -> &'static str {
        match self {
            OptionId::Speed => "Set how quickly the selection moves while a direction is held.",
            OptionId::LeftRight => {
                "Choose speed changes, letter jumps, page jumps or movement. Direction uses rows in grids."
            }
            OptionId::ArtLimit => {
                "Above this speed, artwork waits until scrolling stops."
            }
            OptionId::Layout => {
                "Default view for places without a custom view. Actions changes only the current place."
            }
            OptionId::ResetCustomViews => {
                "Remove all custom views after confirmation. Every place will use the global view."
            }
            OptionId::DetailsStyle => {
                "Information keeps the summary under the picture. Large Artwork uses the whole column."
            }
            OptionId::GameNameDisplay => {
                "Choose which region, disc, version and translation tags remain visible. Files and saved data are unchanged."
            }
            OptionId::FolderBrackets => {
                "Show or hide Degauss's outer [ name ] marker on folder rows. Brackets inside names are unchanged."
            }
            OptionId::Font => {
                "Choose Smooth or Pixel text. Smooth 2 and Pixel 2 use bolder lettering."
            }
            OptionId::Theme => {
                "Left/right choose a theme. A edits colours, logo and default text."
            }
            OptionId::ShowArt => "Show or hide library artwork. Manual-match previews remain available.",
            OptionId::ArtworkScale => {
                "Match game artwork to the display shape. Logos and screensaver images are unchanged."
            }
            OptionId::ShowHidden => {
                "Display items hidden with Hide This without clearing their hidden settings."
            }
            OptionId::ShowEmpty => {
                "Show systems and folders with no games. Manually hidden items stay hidden."
            }
            OptionId::ShowOther => "Show the Other category and its installed cores.",
            OptionId::ShowUtility => "Show the Utility category for test patterns and measurement cores.",
            OptionId::ShowCores => {
                "Show a cached top-level browser for launching installed Standard, RA and Unstable cores without a game."
            }
            OptionId::ShowUnstable => "Show the Unstable category for nightly core builds.",
            OptionId::ShowScripts => "Show Scripts in the main menu. Run installed scripts and return to Degauss when they finish.",
            OptionId::CorePreference => "Choose the preferred core when standard and RetroAchievements versions are both installed.",
            OptionId::AutomaticDataSource => {
                "Choose which available source systems left on Automatic try first when they are entered."
            }
            OptionId::SeparateHandheldCategory => {
                "Show recognised handheld systems in a separate Handheld category. Off keeps MiSTer's Console grouping."
            }
            OptionId::ShowBar => "Show the clock, connections and button hints while browsing. Menus keep the bar.",
            OptionId::FoldersLast => {
                "On puts folders before games. Off puts games before folders."
            }
            OptionId::ResetHidden => {
                "Unhide all manually hidden systems, folders and games after confirmation."
            }
            OptionId::FavoritesFirst => {
                "Show favourites first in each folder, keeping them in alphabetical order."
            }
            OptionId::LastPlayed => {
                "Show up to this many recent successful launches before Favourites, newest first."
            }
            OptionId::HoldA | OptionId::HoldB | OptionId::HoldX | OptionId::HoldY => {
                "Hold for one second to run this action while browsing. A short press keeps the button's normal action."
            }
            OptionId::RandomLaunches => {
                "Choose whether a random game is selected for browsing or launched immediately."
            }
            OptionId::RebuildCache => {
                "Rescan all systems after changing games, cores or artwork on the card."
            }
            OptionId::ScrapeAll => {
                "Open scraping settings for all supported systems. Artwork Pack systems are skipped."
            }
            OptionId::ShowStats => "Replace button hints with rendering and frame-time measurements.",
            OptionId::Present => "Direct draws into the framebuffer. Staged draws into memory before copying the frame.",
            OptionId::ScreenRotation => {
                "Rotate the complete frontend for a vertical CRT. This does not change MiSTer video modes or game rotation."
            }
            OptionId::OverscanX => "Leave side margins so the display does not crop the interface.",
            OptionId::OverscanY => "Leave top and bottom margins so the display does not crop the interface.",
            OptionId::ShiftX => {
                "Move the interface left or right within the available side margins."
            }
            OptionId::ShiftY => "Move the interface up or down within the available top and bottom margins.",
            OptionId::Screensaver => {
                "Choose the idle time before the artwork screensaver starts, or turn it off."
            }
            OptionId::Advanced => "Press A for diagnostics: the drawing path and the readout.",
            OptionId::Spacer => "",
        }
    }

    pub const fn hold_button(self) -> Option<HoldButton> {
        match self {
            Self::HoldA => Some(HoldButton::A),
            Self::HoldB => Some(HoldButton::B),
            Self::HoldX => Some(HoldButton::X),
            Self::HoldY => Some(HoldButton::Y),
            _ => None,
        }
    }
}

/// The speed as a multiple of the baseline rate, for display.
pub fn speed_label(step: usize) -> String {
    let (multiplier, ms) = SPEED_STEPS[step.min(SPEED_STEPS.len() - 1)];
    let chevrons = ">".repeat(chevron_count(step));
    if multiplier.fract() == 0.0 {
        format!("{chevrons} {multiplier:.0}x  ({ms} ms)")
    } else {
        format!("{chevrons} {multiplier:.2}x  ({ms} ms)")
    }
}

/// A short form for the title bar, where there is no room for milliseconds.
pub fn speed_badge(step: usize) -> String {
    let (multiplier, _) = SPEED_STEPS[step.min(SPEED_STEPS.len() - 1)];
    let chevrons = ">".repeat(chevron_count(step));
    if multiplier.fract() == 0.0 {
        format!("{chevrons} {multiplier:.0}x")
    } else {
        format!("{chevrons} {multiplier:.2}x")
    }
}

/// One chevron at or below the baseline, then one more per step, so the
/// speed reads at a glance without counting.
fn chevron_count(step: usize) -> usize {
    let baseline = crate::input::SPEED_START;
    if step <= baseline {
        1
    } else {
        (step - baseline + 1).min(8)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_original_flat_list_keeps_its_grouping() {
        assert!(OPTIONS.first() != Some(&OptionId::Spacer));
        assert!(OPTIONS.last() == Some(&OptionId::Advanced));
        for pair in OPTIONS.windows(2) {
            assert!(
                pair[0] != OptionId::Spacer || pair[1] != OptionId::Spacer,
                "two spacers in a row"
            );
        }
        for option in ADVANCED {
            assert!(!OPTIONS.contains(&option), "{option:?} is in both lists");
        }
    }

    #[test]
    fn pages_expose_every_existing_control_exactly_once() {
        let expected: Vec<_> = OPTIONS
            .iter()
            .chain(ADVANCED.iter())
            .copied()
            .filter(|option| !matches!(option, OptionId::Advanced | OptionId::Spacer))
            .collect();
        let mut seen = Vec::new();
        for page in OptionsPage::ALL {
            assert!(!page.ids().is_empty(), "{page:?} is empty");
            for option in page.ids().iter().copied() {
                assert!(expected.contains(&option), "{option:?} is not a control");
                assert!(!seen.contains(&option), "{option:?} appears on two pages");
                seen.push(option);
            }
        }
        for option in &expected {
            assert!(
                seen.contains(option),
                "{option:?} is missing from the pages"
            );
        }
        assert_eq!(seen.len(), expected.len());
    }

    #[test]
    fn pages_have_stable_keys_indices_and_explanations() {
        let mut keys = Vec::new();
        for (index, page) in OptionsPage::ALL.into_iter().enumerate() {
            assert_eq!(page.index(), index);
            assert_eq!(OptionsPage::parse(page.key()), Some(page));
            assert!(!keys.contains(&page.key()), "duplicate page key");
            keys.push(page.key());
            assert!(!page.label().is_empty());
            assert!(page.help().len() > 20);
        }
        assert_eq!(
            keys,
            [
                "navigation",
                "shortcuts",
                "appearance",
                "library",
                "display",
                "developer"
            ]
        );
        assert_eq!(OptionsPage::parse(""), None);
        assert_eq!(OptionsPage::parse("unknown"), None);
    }

    #[test]
    fn shortcuts_have_their_own_page_and_library_ordering_stays_separate() {
        assert_eq!(
            OptionsPage::Shortcuts.ids(),
            &[
                OptionId::HoldA,
                OptionId::HoldB,
                OptionId::HoldX,
                OptionId::HoldY
            ]
        );
        assert!(OptionsPage::Navigation
            .ids()
            .contains(&OptionId::RandomLaunches));
        assert!(OptionsPage::Library
            .ids()
            .contains(&OptionId::FavoritesFirst));
        assert!(OptionsPage::Library.ids().contains(&OptionId::FoldersLast));
        assert!(OptionsPage::Library.ids().windows(3).any(|ids| ids
            == [
                OptionId::CorePreference,
                OptionId::AutomaticDataSource,
                OptionId::SeparateHandheldCategory,
            ]));
        assert_eq!(OptionsPage::Developer.ids(), &ADVANCED);
    }

    #[test]
    fn every_option_has_a_label_and_an_explanation() {
        for option in OPTIONS.iter().chain(ADVANCED.iter()).copied() {
            if option == OptionId::Spacer {
                continue;
            }
            assert!(!option.label().is_empty());
            assert!(
                option.help().len() > 20,
                "{:?} needs a real explanation",
                option
            );
            assert!(!option.help().contains("same alphabet."));
            assert!(!option.help().contains("more actions"));
        }
    }

    #[test]
    fn the_options_list_has_no_duplicates() {
        let mut seen = Vec::new();
        for option in OPTIONS.iter().chain(ADVANCED.iter()).copied() {
            if option == OptionId::Spacer {
                continue;
            }
            assert!(!seen.contains(&option), "{option:?} listed twice");
            seen.push(option);
        }
    }

    #[test]
    fn every_hold_row_maps_to_its_face_button() {
        for (option, button, label) in [
            (OptionId::HoldA, HoldButton::A, "Hold A"),
            (OptionId::HoldB, HoldButton::B, "Hold B"),
            (OptionId::HoldX, HoldButton::X, "Hold X"),
            (OptionId::HoldY, HoldButton::Y, "Hold Y"),
        ] {
            assert_eq!(option.hold_button(), Some(button));
            assert_eq!(option.label(), label);
        }
        assert_eq!(OptionId::FavoritesFirst.hold_button(), None);
    }

    #[test]
    fn scrape_all_follows_rebuild_all_system_lists() {
        let rebuild = OPTIONS
            .iter()
            .position(|option| *option == OptionId::RebuildCache)
            .expect("Rebuild all system lists is in Options");
        assert_eq!(OPTIONS.get(rebuild + 1), Some(&OptionId::ScrapeAll));
        let library = OptionsPage::Library.ids();
        let rebuild = library
            .iter()
            .position(|option| *option == OptionId::RebuildCache)
            .expect("Rebuild All System Lists is in Library");
        assert_eq!(library.get(rebuild + 1), Some(&OptionId::ScrapeAll));
    }

    #[test]
    fn automatic_data_source_follows_core_preference_in_library() {
        let library = OptionsPage::Library.ids();
        let core = library
            .iter()
            .position(|option| *option == OptionId::CorePreference)
            .expect("Core Preference is in Library");
        assert_eq!(library.get(core + 1), Some(&OptionId::AutomaticDataSource));
    }

    #[test]
    fn the_speed_reads_as_a_multiple_of_the_familiar_rate() {
        // 1x is the rate a conventional frontend scrolls at, whatever the
        // fresh-start default is set to; the fresh start sits at 3x.
        assert!(speed_badge(1).contains("1x"));
        assert!(speed_label(1).contains("90 ms"));
        assert!(speed_badge(crate::input::SPEED_START).contains("3x"));

        let fastest = SPEED_STEPS.len() - 1;
        assert!(speed_badge(fastest).contains("12x"));
        assert!(speed_label(fastest).contains("7 ms"));
    }

    #[test]
    fn faster_settings_show_more_chevrons() {
        let baseline = crate::input::SPEED_START;
        let slow = speed_badge(baseline).matches('>').count();
        let fast = speed_badge(SPEED_STEPS.len() - 1).matches('>').count();
        assert!(fast > slow, "the badge should grow with the speed");
    }

    #[test]
    fn a_step_beyond_the_ladder_is_clamped_rather_than_panicking() {
        assert!(!speed_badge(999).is_empty());
        assert!(!speed_label(999).is_empty());
    }
}
