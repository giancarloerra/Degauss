//! The options screen.
//!
//! Every setting the interface exposes lives here in one list, so adding a
//! setting means adding one entry rather than touching a screen, a config
//! struct and a persistence path separately.
//!
//! Values are changed with left and right, the same keys that change scroll
//! speed while browsing, so nothing new has to be learned. Changes are
//! written to `settings.toml` when leaving the screen.

use crate::input::SPEED_STEPS;

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
    /// The strip along the bottom of the screen, while browsing.
    ShowBar,
    /// Read the card again into the written-down copy of it.
    RebuildCache,
    /// Gather favourites at the top of a folder.
    FavoritesFirst,
    /// Add or remove a favourite by holding X for one second.
    HoldXFavorite,
    RandomLaunches,
    /// List folders after the games rather than before them.
    FoldersLast,
    /// Show everything that has been hidden, in every folder, again.
    ResetHidden,
    ShowStats,
    Present,
    OverscanX,
    OverscanY,
    /// Move the whole picture sideways, for a screen that is not centred.
    ShiftX,
    ShiftY,
    /// How long the machine is left alone before pictures take the screen.
    Screensaver,
    /// Not a setting: the door to the developer list.
    Advanced,
    /// Not a setting: a blank line separating one group from the next.
    /// There to be read, never chosen; the cursor steps over it.
    Spacer,
}

/// Everything on the one screen, in groups a blank row apart: moving
/// through lists, what the screen looks like, how a folder is ordered,
/// what is shown at all, fitting the physical screen, the machine-wide
/// acts, and the door to the diagnostics.
pub const OPTIONS: &[OptionId] = &[
    OptionId::Speed,
    OptionId::ArtLimit,
    OptionId::LeftRight,
    OptionId::Spacer,
    OptionId::Theme,
    OptionId::Layout,
    OptionId::Font,
    OptionId::ShowArt,
    OptionId::ArtworkScale,
    OptionId::ShowBar,
    OptionId::Spacer,
    OptionId::FavoritesFirst,
    OptionId::HoldXFavorite,
    OptionId::FoldersLast,
    OptionId::RandomLaunches,
    OptionId::Spacer,
    OptionId::ShowOther,
    OptionId::ShowUtility,
    OptionId::ShowEmpty,
    OptionId::ShowHidden,
    OptionId::ResetHidden,
    OptionId::Spacer,
    OptionId::OverscanX,
    OptionId::OverscanY,
    OptionId::ShiftX,
    OptionId::ShiftY,
    OptionId::Spacer,
    OptionId::Screensaver,
    OptionId::RebuildCache,
    OptionId::Spacer,
    OptionId::Advanced,
];

/// Tuning and diagnostics: things you set once, or only while measuring.
/// Kept behind a door so the main list stays about using the thing.
pub const ADVANCED: [OptionId; 2] = [OptionId::Present, OptionId::ShowStats];

impl OptionId {
    pub fn label(self) -> &'static str {
        match self {
            OptionId::Speed => "Scroll speed",
            OptionId::LeftRight => "Left and right behaviour",
            OptionId::ArtLimit => "Skip artwork faster than",
            OptionId::Layout => "View",
            OptionId::Font => "Text",
            OptionId::Theme => "Theme",
            OptionId::ShowArt => "Artwork",
            OptionId::ArtworkScale => "Artwork scale factor",
            OptionId::ShowHidden => "Show what you hid",
            OptionId::ShowEmpty => "Show systems with no games",
            OptionId::ShowOther => "Show Other folder",
            OptionId::ShowUtility => "Show Utility folder",
            OptionId::ShowBar => "Bottom bar while browsing",
            OptionId::RebuildCache => "Rebuild all system lists",
            OptionId::FavoritesFirst => "Favourites first",
            OptionId::HoldXFavorite => "Hold X (1s) to add/remove fav",
            OptionId::RandomLaunches => "Random game behaviour",
            OptionId::FoldersLast => "Folders before games",
            OptionId::ResetHidden => "Unhide everything",
            OptionId::ShowStats => "Performance readout",
            OptionId::Present => "Drawing path",
            OptionId::OverscanX => "Edge margin, sides",
            OptionId::OverscanY => "Edge margin, top and bottom",
            OptionId::ShiftX => "Screen position, sideways",
            OptionId::ShiftY => "Screen position, up and down",
            OptionId::Screensaver => "Screensaver",
            OptionId::Advanced => "Developer",
            OptionId::Spacer => "",
        }
    }

    /// A line of explanation, shown under the list. Settings nobody can
    /// explain are settings nobody should have.
    pub fn help(self) -> &'static str {
        match self {
            OptionId::Speed => "How fast a held direction moves through the list.",
            OptionId::LeftRight => {
                "What left and right do in a folder. Direction lets up and down move a whole row in Tiled."
            }
            OptionId::ArtLimit => {
                "Above this scroll speed, pictures wait until the list stops moving."
            }
            OptionId::Layout => "Details, Tiled, Carousel or List.",
            OptionId::Font => {
                "Smooth for a monitor, Pixel on whole pixels for a tube. The 2s are bolder."
            }
            OptionId::Theme => {
                "A palette from the themes folder, read at start. Standard is degauss.toml."
            }
            OptionId::ShowArt => "Turn artwork off entirely.",
            OptionId::ArtworkScale => {
                "Correct game artwork for a 4:3 or 16:9 display. Logos and the screensaver stay unchanged."
            }
            OptionId::ShowHidden => {
                "Show what you hid yourself, from Hide this. Not the same as empty ones."
            }
            OptionId::ShowEmpty => {
                "Show folders and systems with no games in them. They are hidden by default, whether or not you hid anything."
            }
            OptionId::ShowOther => "Show the Other group: the cores that are not games.",
            OptionId::ShowUtility => "Show the Utility group: test patterns and measurement cores.",
            OptionId::ShowBar => "The strip with the time and the buttons. Menus always keep it.",
            OptionId::FoldersLast => {
                "On, folders lead a system's listing; off, the games come first."
            }
            OptionId::ResetHidden => {
                "Put back everything you hid yourself, in every folder and every system."
            }
            OptionId::FavoritesFirst => {
                "Gather a folder's favourites at its top, in the same alphabet."
            }
            OptionId::HoldXFavorite => {
                "Hold X for one second over a game to add or remove it. Off in the master Favourites system."
            }
            OptionId::RandomLaunches => {
                "Whether a random pick starts the game, or only moves to it so you can look first."
            }
            OptionId::RebuildCache => {
                "Read the card again. Do this after adding games, cores or artwork."
            }
            OptionId::ShowStats => "Replace the key hints with frame timings.",
            OptionId::Present => "Draw into the screen directly, or into memory first.",
            OptionId::OverscanX => "Keep this much of each side clear of the bezel.",
            OptionId::OverscanY => "Keep this much of the top and bottom clear.",
            OptionId::ShiftX => {
                "Nudge the picture left or right, for a screen that sits off centre."
            }
            OptionId::ShiftY => "Nudge the picture up or down, for a screen that sits off centre.",
            OptionId::Screensaver => {
                "How long to wait, with nothing pressed, before showing pictures."
            }
            OptionId::Advanced => "Diagnostics: the drawing path and the readout.",
            OptionId::Spacer => "",
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
    fn the_list_is_grouped_and_the_groups_are_well_formed() {
        // The one screen holds everything, kept readable by blank rows
        // between groups. A spacer at either end or two in a row would
        // draw as dead space; the door sits last; the developer list
        // holds only the diagnostics.
        assert!(OPTIONS.first() != Some(&OptionId::Spacer));
        assert!(OPTIONS.last() == Some(&OptionId::Advanced));
        for pair in OPTIONS.windows(2) {
            assert!(
                pair[0] != OptionId::Spacer || pair[1] != OptionId::Spacer,
                "two spacers in a row"
            );
        }
        let rows = OPTIONS.iter().filter(|o| **o != OptionId::Spacer).count();
        assert!(
            rows <= 25,
            "the options list has grown to {rows} real rows; review the 240-line layout before adding another"
        );
        for option in ADVANCED {
            assert!(!OPTIONS.contains(&option), "{option:?} is in both lists");
        }
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
    fn the_held_x_option_follows_favourites_first() {
        // Both settings govern favourites while browsing. Keeping the
        // shortcut here, rather than behind Developer, makes the placement
        // part of the Options contract instead of an incidental array order.
        let favourites = OPTIONS
            .iter()
            .position(|option| *option == OptionId::FavoritesFirst)
            .expect("Favourites first is in Options");
        assert_eq!(OPTIONS.get(favourites + 1), Some(&OptionId::HoldXFavorite));
        assert_eq!(
            OptionId::HoldXFavorite.label(),
            "Hold X (1s) to add/remove fav"
        );
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
