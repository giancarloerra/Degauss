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
    /// The scroll speed above which artwork stops being loaded per row.
    ArtLimit,
    Layout,
    /// Which typeface the interface is set in.
    Font,
    ShowArt,
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
    /// Not a setting: the door to the advanced list.
    Advanced,
}

/// What most people will ever want to change.
pub const OPTIONS: [OptionId; 16] = [
    OptionId::Speed,
    OptionId::ArtLimit,
    OptionId::RebuildCache,
    OptionId::Layout,
    OptionId::Font,
    OptionId::ShowBar,
    OptionId::ShowArt,
    OptionId::FavoritesFirst,
    OptionId::RandomLaunches,
    OptionId::FoldersLast,
    OptionId::ShowEmpty,
    OptionId::ResetHidden,
    OptionId::ShowHidden,
    OptionId::ShowOther,
    OptionId::ShowUtility,
    OptionId::Advanced,
];

/// Tuning and diagnostics: things you set once, or only while measuring.
/// Kept behind a door so the main list stays about using the thing.
pub const ADVANCED: [OptionId; 7] = [
    OptionId::Screensaver,
    OptionId::OverscanX,
    OptionId::OverscanY,
    OptionId::ShiftX,
    OptionId::ShiftY,
    OptionId::Present,
    OptionId::ShowStats,
];

impl OptionId {
    pub fn label(self) -> &'static str {
        match self {
            OptionId::Speed => "Scroll speed",
            OptionId::ArtLimit => "Skip artwork faster than",
            OptionId::Layout => "View",
            OptionId::Font => "Text",
            OptionId::ShowArt => "Artwork",
            OptionId::ShowHidden => "Show what you hid",
            OptionId::ShowEmpty => "Show empty folders",
            OptionId::ShowOther => "Show Other",
            OptionId::ShowUtility => "Show Utility",
            OptionId::ShowBar => "Bottom bar while browsing",
            OptionId::RebuildCache => "Rebuild all system lists",
            OptionId::FavoritesFirst => "Favourites first",
            OptionId::RandomLaunches => "Random",
            OptionId::FoldersLast => "Folders",
            OptionId::ResetHidden => "Unhide everything",
            OptionId::ShowStats => "Performance readout",
            OptionId::Present => "Drawing path",
            OptionId::OverscanX => "Edge margin, sides",
            OptionId::OverscanY => "Edge margin, top and bottom",
            OptionId::ShiftX => "Screen position, sideways",
            OptionId::ShiftY => "Screen position, up and down",
            OptionId::Screensaver => "Screensaver",
            OptionId::Advanced => "Advanced",
        }
    }

    /// A line of explanation, shown under the list. Settings nobody can
    /// explain are settings nobody should have.
    pub fn help(self) -> &'static str {
        match self {
            OptionId::Speed => "How fast a held direction moves through the list.",
            OptionId::ArtLimit => {
                "Above this scroll speed, pictures wait until the list stops moving."
            }
            OptionId::Layout => "Details, Tiled, Carousel or List.",
            OptionId::Font => {
                "Smooth for a monitor, Pixel on whole pixels for a tube. The 2s are bolder."
            }
            OptionId::ShowArt => "Turn artwork off entirely.",
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
                "Where folders sit inside a system: before the games or after them."
            }
            OptionId::ResetHidden => {
                "Put back everything you hid yourself, in every folder and every system."
            }
            OptionId::FavoritesFirst => {
                "Gather a folder's favourites at its top, in the same alphabet."
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
            OptionId::Advanced => "Tuning and diagnostics.",
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
    fn the_main_list_stays_short_enough_to_be_useful() {
        // Options nobody uses push out the ones people do. Anything that
        // exists for measurement belongs behind Advanced.
        //
        // Ten rows fit on screen, so the list already scrolls. Anything
        // further belongs behind Advanced.
        assert!(
            OPTIONS.len() <= 16,
            "the main options list has grown to {}",
            OPTIONS.len()
        );
        assert!(
            OPTIONS.contains(&OptionId::Advanced),
            "the door must be in the list"
        );
        for option in ADVANCED {
            assert!(!OPTIONS.contains(&option), "{option:?} is in both lists");
        }
    }

    #[test]
    fn every_option_has_a_label_and_an_explanation() {
        for option in OPTIONS.iter().chain(ADVANCED.iter()).copied() {
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
            assert!(!seen.contains(&option), "{option:?} listed twice");
            seen.push(option);
        }
    }

    #[test]
    fn the_speed_reads_as_a_multiple_of_the_baseline() {
        let baseline = crate::input::SPEED_START;
        assert!(speed_badge(baseline).contains("1x"));
        assert!(speed_label(baseline).contains("90 ms"));

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
