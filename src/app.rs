//! Degauss: browsing groups, systems and games, fast.
//!
//! Browsing has three levels, mirroring how MiSTer's own menu is organised:
//! the groups (Arcade, Console, Computer), then the systems in one, then the
//! games.
//!
//! Everything works with a stick and four buttons, because that is all a
//! MiSTer controller is guaranteed to have. A chooses. B goes
//! back, and at the top of the tree, where there is nowhere further back, it
//! opens the menu instead: that one rule is what makes options, help, hiding
//! and exit reachable without a keyboard.
//!
//! Three decisions are worth knowing about:
//!
//! * Systems are discovered by checking folders exist, never by counting
//!   what is in them. Walking a big library takes seconds on this hardware,
//!   and doing that for a hundred systems would mean a minute of nothing.
//! * A library is walked incrementally once opened, stepping between frames,
//!   so the screen keeps moving and says how far along it is instead of
//!   freezing.
//! * Artwork is fetched only once the selection has settled. Decoding a
//!   screenshot costs milliseconds, so fetching one per row at eleven rows a
//!   second would stutter. The delay is a setting, and zero is a legitimate
//!   thing to try.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{ComponentHandle, ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::browse::{self, Library, Place};
use crate::config::{Color as ConfigColor, Config, SystemConfig};
use crate::covers::{CoverCache, CoverStats};
use crate::error::Result;
use crate::font::Font;
use crate::input::{
    Action, InputReader, KeyEdge, RepeatConfig, Repeater, SPEED_START, SPEED_STEPS,
};
use crate::list_state::ListState;
use crate::metrics::{FrameTimer, StartupTimings};
use crate::options::{speed_badge, speed_label, OptionId, ADVANCED, OPTIONS};
use crate::render::{FrameWork, PresentMode, Presenter};
use crate::settings::Settings;
use crate::surface::Surface;
use crate::systems::FoundSystem;
use crate::{DegaussWindow, DetailLine, Row};

/// Which screen is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The wordmark, shown briefly on the way in.
    Splash,
    Browse,
    Menu,
    /// What can be done with the folder on screen, on its own button.
    Context,
    Options,
    /// Settings that exist to be measured or tuned once, not used.
    Advanced,
    Help,
    About,
    /// A picture at a time, when the machine has been left alone.
    Screensaver,
    /// A grid of letters, for jumping down a long list or narrowing it.
    Find,
    /// Which folder of MiSTer's favourites a game is going into.
    FavoriteFolder,
}

impl Screen {
    /// What the interface layer draws: 0 browse, 1 a plain list, 2 the
    /// wordmark.
    fn ui_index(self) -> i32 {
        match self {
            Screen::Browse => 0,
            Screen::Menu | Screen::Context | Screen::Options | Screen::Advanced | Screen::Help => 1,
            Screen::About | Screen::Splash => 2,
            Screen::Screensaver => 3,
            Screen::Find => 4,
            Screen::FavoriteFolder => 1,
        }
    }
}

/// How long the wordmark stays up before browsing starts. Long enough to
/// read, short enough that nobody waits for it; any button skips it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SPLASH_MS: u64 = 1400;

/// Something waiting on a yes or no.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Exit,
    Hide(usize),
}

/// What the browse screen is showing. Three levels, mirroring how MiSTer's
/// own menu is organised: the groups it shows at the top, then what is in
/// one, then the games.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Browsing {
    Categories,
    Systems,
    Games,
}

/// How the browse screen is laid out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// The list beside a picture, with what the gamelist knows under it.
    Details,
    /// A grid of pictures.
    Tiled,
    List,
    /// One picture across the middle of the screen with the neighbours
    /// either side of it, mostly off the edges.
    Carousel,
}

impl Layout {
    pub fn label(self) -> &'static str {
        match self {
            Layout::Details => "details",
            Layout::Tiled => "tiled",
            Layout::Carousel => "carousel",
            Layout::List => "list",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "details" => Some(Layout::Details),
            // What this view was called before it was named for what it
            // shows. A settings file written then still reads.
            "preview" => Some(Layout::Details),
            "tiled" => Some(Layout::Tiled),
            // What this view was called before.
            "covers" => Some(Layout::Tiled),
            "carousel" => Some(Layout::Carousel),
            "list" => Some(Layout::List),
            _ => None,
        }
    }

    fn index(self) -> i32 {
        match self {
            Layout::Details => 0,
            Layout::Tiled => 1,
            Layout::List => 2,
            Layout::Carousel => 3,
        }
    }

    fn prev(self) -> Self {
        match self {
            Layout::Details => Layout::List,
            Layout::Tiled => Layout::Details,
            Layout::Carousel => Layout::Tiled,
            Layout::List => Layout::Carousel,
        }
    }

    fn next(self) -> Self {
        match self {
            Layout::Details => Layout::Tiled,
            Layout::Tiled => Layout::Carousel,
            Layout::Carousel => Layout::List,
            Layout::List => Layout::Details,
        }
    }

    fn rows_have_art(self) -> bool {
        matches!(self, Layout::Tiled | Layout::Carousel)
    }

    /// True when entries are laid out in a grid rather than one per row.
    fn is_grid(self) -> bool {
        matches!(self, Layout::Tiled)
    }
}

/// How long a title sits still before it starts to walk, and how long the
/// walk takes. Both are also written into the interface, which does the
/// animation; these are here to say when to turn it round.
/// How long after the list stops before a picture is loaded, when scrolling
/// faster than the limit. Long enough that one more press does not pay for a
/// decode nobody sees, short enough not to feel like waiting.
const ART_AFTER_SCROLL_MS: u64 = 140;

/// Shown on the way in and on the About screen.
const COPYRIGHT: &str = "Copyright (C) 2026 Giancarlo Erra";

/// Named on the About screen because a licence nobody can find is a licence
/// nobody follows. The full text ships in LICENSE beside the program.
const LICENCE: &str = "PolyForm Noncommercial 1.0.0";

/// How fast the strip of pictures drifts across, in pixels per second.
/// Slow enough to read a screenshot, fast enough that no part of the
/// picture sits on the same phosphor for long.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SAVER_PIXELS_PER_SECOND: f32 = 24.0;

/// How long to wait before looking again, when a look found nothing.
const SAVER_RETRY_SECONDS: u64 = 15;

/// How long a single look may spend reading the card. Reading a system's
/// metadata takes seconds on this hardware, and this runs in the frame
/// loop: without a ceiling a bad look means a frozen button.
const SAVER_BUDGET_MS: u64 = 1500;

/// How many folders to look in within one system.
const SAVER_FOLDERS_SEARCHED: usize = 24;

/// How many pictures to take from one system before moving to another.
const SAVER_WANTED: usize = 400;

/// The most pictures to hold at once. Each is a decoded image in the cover
/// cache, so this is memory as much as it is variety.
const SAVER_POOL_MAX: usize = 24;

/// What the screensaver delay can be set to, in seconds. Zero is off.
const SAVER_CHOICES: [u64; 5] = [0, 60, 120, 300, 600];

const MARQUEE_WAIT_MS: u64 = 250;

/// The longest the lines under the picture are given to walk their width.
/// They travel at half a title's pace, so they need their own clock: on
/// the title's they would be cut off half way and snap back.
const DETAIL_TRAVEL_MS: u64 = 8000;
/// The longest a title is given to walk its width, and so the period its
/// clock runs at.
const MARQUEE_TRAVEL_MS: u64 = 1400;

/// What the contextual menu offers for the folder on screen.
///
/// One entry today. It has its own button because what you can do with a
/// folder is a different question from what you can do with the program,
/// and the answers will keep diverging: favourites and jump-to belong here,
/// not beside "Exit to MiSTer".
const RANDOM: &str = "Random game in this folder";

/// The same, drawn only from what has been kept. The heart is written as
/// a literal here for the same reason it is in the interface file: it is a
/// glyph, and glyphs reach the binary by being written down.
const RANDOM_FAVORITE: &str = "Random game in this folder (\u{2665} only)";

/// Switching how the list looks belongs with the folder you are looking at,
/// not two screens away in the settings.
const CHANGE_VIEW: &str = "Change view";

/// Move down a long list a letter at a time.
const JUMP: &str = "Jump to letter";

/// Narrow the folder on screen to the titles that match.
const SEARCH: &str = "Search this folder";

/// Put back everything the search took away.
const CLEAR_SEARCH: &str = "Clear search";

/// The id the favourites folder is listed under in the table.
const FAVORITES_ID: &str = "Favorites";

/// Take this row out of the list until it is asked for again.
const HIDE_THIS: &str = "Hide this";

/// Put it back.
const SHOW_THIS: &str = "Show this";

/// Keep this game where MiSTer's own favourites live.
const ADD_FAVORITE: &str = "Add to favourites";

/// Take it out again.
const REMOVE_FAVORITE: &str = "Remove from favourites";

/// The entry that spells out a folder name rather than picking one.
const NEW_FOLDER: &str = "New folder...";

/// What can be done where the cursor is, in groups.
///
/// A blank line between groups: a dozen entries in one column is a wall,
/// and the same four things are always in the same place if they are
/// grouped. The movement keys step over the blanks.
fn context_entries(
    browsing: Browsing,
    searching: bool,
    favorite: Option<bool>,
    hidden: Option<bool>,
) -> Vec<String> {
    let mut groups: Vec<Vec<String>> = Vec::new();

    if browsing == Browsing::Games {
        groups.push(vec![RANDOM.to_string(), RANDOM_FAVORITE.to_string()]);
    }
    // Only over something that can be played. A folder is not a favourite.
    match favorite {
        Some(true) => groups.push(vec![REMOVE_FAVORITE.to_string()]),
        Some(false) => groups.push(vec![ADD_FAVORITE.to_string()]),
        None => {}
    }
    // Jumping and searching are for the long lists, which is where the
    // games are. A list of three groups needs neither.
    if browsing != Browsing::Categories {
        let mut find = vec![JUMP.to_string(), SEARCH.to_string()];
        if searching {
            find.push(CLEAR_SEARCH.to_string());
        }
        groups.push(find);
    }
    if let Some(hidden) = hidden {
        groups.push(vec![if hidden {
            SHOW_THIS.to_string()
        } else {
            HIDE_THIS.to_string()
        }]);
    }
    groups.push(vec![CHANGE_VIEW.to_string()]);

    let mut entries = Vec::new();
    for group in groups {
        if !entries.is_empty() {
            entries.push(String::new());
        }
        entries.extend(group);
    }
    entries
}

/// The line drawn over a screensaver picture: what it is, and what it is
/// from.
///
/// Cut to something that fits across a picture on a 352 pixel screen. The
/// title gives way rather than the machine, because two screenshots from
/// the same system look alike and the name is what tells them apart.
fn saver_caption(title: &str, system: &str) -> String {
    const ROOM: usize = 34;
    let tail = format!(" - {system}");
    let room = ROOM.saturating_sub(tail.chars().count());
    let title = if title.chars().count() > room && room > 3 {
        let kept: String = title.chars().take(room - 3).collect();
        format!("{}...", kept.trim_end())
    } else {
        title.to_string()
    };
    format!("{title}{tail}")
}

/// Reading the card into the cache, one system per frame.
///
/// One per frame rather than all at once: reading a system takes seconds,
/// and a screen that says what it is doing and moves while it does it is
/// the difference between waiting and wondering.
struct Building {
    /// Indices into `all_systems`, in reverse so the next one is popped.
    left: Vec<usize>,
    done: usize,
    total: usize,
    index: crate::cache::Index,
    /// True when the whole cache was thrown away first, so a system whose
    /// file is still on the card is read again rather than skipped.
    forced: bool,
}

/// One picture in the screensaver's ring, with what to call it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SaverPicture {
    path: PathBuf,
    /// The title and the machine it is from, as one line: nothing else on
    /// that screen says what is being looked at.
    caption: String,
}

/// What picking a letter on the grid does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FindMode {
    /// Move the selection to the first entry at or after that letter.
    Jump,
    /// Add the letter to a filter over the folder on screen.
    Search,
    /// Spell out the name of a new favourites folder.
    NewFolder,
}

/// The grid, in reading order. Nine across and four down fills a screen
/// this size exactly, and every character on it is one a sorted list can
/// begin with.
const FIND_CELLS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";

/// How many across.
const FIND_COLUMNS: usize = 9;

/// Where picking a letter should land, given each entry's first character
/// and whether it is a folder.
///
/// Not a scan for the first entry at or after the letter. A folder is
/// listed before every file whatever it is called, so the first letters do
/// not climb steadily from the top of the list to the bottom: they climb
/// through the folders, drop back, and climb again through the files.
/// Looking for the first entry at or after the letter therefore stops in the
/// folders nearly every time, which makes the jump useless in any system
/// whose folder names span the alphabet.
///
/// So: an entry that actually begins with the letter, wherever it is;
/// failing that the first file after it, since the files are the body of
/// the list; failing that the first entry after it at all.
fn jump_target(entries: &[(char, bool)], key: char) -> Option<usize> {
    entries
        .iter()
        .position(|(first, _)| *first == key)
        .or_else(|| {
            entries
                .iter()
                .position(|(first, folder)| !*folder && *first > key)
        })
        .or_else(|| entries.iter().position(|(first, _)| *first > key))
}

/// The path inside a row's written-down name.
///
/// The name carries what kind of thing it is, because a folder and a game
/// at the same path are different things.
fn key_path(key: &str) -> Option<PathBuf> {
    let (kind, rest) = key.split_once(':')?;
    match kind {
        "d" | "a" | "f" => Some(PathBuf::from(rest.split('|').next().unwrap_or(rest))),
        _ => None,
    }
}

/// What a row launches, under the name that thing is known by.
///
/// A game by its path and an AmigaVision title by the made-up name it is
/// looked up under, so a favourite pointing at either compares equal to
/// the row it came from. Without this an Amiga favourite could never
/// match: it names a title, and the row it came from is not a file.
fn row_target(row: &browse::Row) -> Option<PathBuf> {
    match &row.kind {
        browse::Kind::Play(browse::Launch::File(path)) => Some(path.clone()),
        browse::Kind::Play(browse::Launch::AmigaVision { install, title }) => {
            Some(crate::favorites::amiga_key(install, title))
        }
        browse::Kind::Enter(_) => None,
    }
}

/// The name a row is written down under when it is hidden.
///
/// A folder by where it points and a game by what it launches, so the two
/// cannot collide and neither depends on what the row is called: renaming
/// a game in a gamelist must not unhide it.
fn row_key(row: &browse::Row) -> String {
    match &row.kind {
        browse::Kind::Enter(place) => place.key(),
        browse::Kind::Play(browse::Launch::File(path)) => format!("f:{}", path.display()),
        browse::Kind::Play(browse::Launch::AmigaVision { install, title }) => {
            format!("a:{}|{title}", install.display())
        }
    }
}

/// Where the cursor lands in a freshly listed folder: the remembered row
/// found again by what it is, or the fallback index when there is nothing
/// remembered or the row is gone. Identity is asked first because an index
/// goes stale whenever rows are added, hidden or resorted, and landing on
/// an arbitrary row looks like the list moved on its own.
fn reselect(rows: &[browse::Row], remembered: Option<&str>, fallback: usize) -> usize {
    remembered
        .and_then(|key| rows.iter().position(|row| row_key(row) == key))
        .unwrap_or(fallback)
}

/// A title with the characters MiSTer's favourites script refuses taken
/// out, so a name made here is one it would have made.
fn sanitise(name: &str) -> String {
    name.chars()
        .map(|c| {
            if crate::favorites::BAD_CHARS.contains(&c) {
                '-'
            } else {
                c
            }
        })
        .collect()
}

/// The first character of a sort key, for comparing against a grid cell.
/// A title beginning with anything that is not a letter or a digit sorts
/// before them all, which is where MiSTer's own menu puts it too.
fn first_letter(key: &str) -> char {
    key.chars().next().unwrap_or(' ').to_ascii_lowercase()
}

/// A title with its spaces and punctuation taken out, in capitals, so a
/// search can be typed on a grid that has neither.
fn squashed(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// One step of the path walked into a system: where it points, and where
/// the selection was standing when it was left.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Crumb {
    place: Place,
    selected: usize,
}

/// A seed taken from the clock, so "random" differs between runs.
fn seed_from_clock() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_nanos() as u64)
        .unwrap_or(0x2545_f491_4f6c_dd1d)
        | 1
}

/// xorshift64*. Small, no dependency, and far better than good enough for
/// picking a game: nothing here is cryptography.
fn next_random(seed: &mut u64) -> u64 {
    let mut x = *seed;
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    *seed = x;
    x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 16
}

/// What the menu offers depends on where it was opened from: hiding a
/// system only makes sense while looking at the list of systems.
fn menu_entries(browsing: Browsing, system: Option<&str>) -> Vec<String> {
    let mut entries = Vec::new();
    if browsing == Browsing::Systems {
        if let Some(name) = system {
            entries.push(format!("Hide {name}"));
        }
    }
    entries.push("Options".to_string());
    entries.push("Help".to_string());
    entries.push("About".to_string());
    entries.push("Exit to MiSTer".to_string());
    entries
}

/// Kept to about forty characters a line: the narrowest screen this runs on
/// is 352 pixels, and anything longer is silently cut off.
const HELP: [&str; 10] = [
    "Everything works with a stick and four",
    "buttons. No keyboard needed.",
    "",
    "Stick up/down: move through the list",
    "Stick left/right: change speed or setting",
    "",
    "A                open a folder, play a game",
    "B                go back, out of a folder",
    "X                the contextual menu",
    "Y                the main menu",
];

/// Pixel geometry for one layout, from the real framebuffer size.
#[derive(Debug, Clone, Copy)]
struct Geometry {
    chrome: f32,
    /// The strip along the bottom, slimmer than the title bar was.
    bar: f32,
    row_height: f32,
    body_font: f32,
    small_font: f32,
    pad: f32,
    art_width: f32,
    columns: usize,
    tile_width: f32,
    tile_height: f32,
    visible: usize,
    stride: usize,
    inset_x: f32,
    inset_y: f32,
}

impl Geometry {
    fn compute(
        layout: Layout,
        plain: bool,
        chrome_shown: bool,
        show_bar: bool,
        width: u32,
        height: u32,
        config: &Config,
    ) -> Self {
        // The safe rectangle first: everything else is measured inside it,
        // because a television crops the edges.
        let inset_x = (width as f32 * config.app.overscan_x as f32 / 100.0).round();
        let inset_y = (height as f32 * config.app.overscan_y as f32 / 100.0).round();
        let width = (width as f32 - inset_x * 2.0).max(64.0);
        let height = (height as f32 - inset_y * 2.0).max(48.0);

        // How much bigger this screen is than the one everything was
        // measured on. Exactly one at 240 lines, which is what keeps a
        // tube looking as it did; above that the ceilings rise with it,
        // because a ceiling set for 240 lines makes 720 look like 240 with
        // more space around it.
        let scale = (height / 240.0).max(1.0);
        let chrome = (height / 11.0).round().clamp(12.0, 30.0 * scale);
        // The bottom strip is deliberately thinner than the old title bar:
        // it carries small text and nothing else, and on a 240 line screen
        // every row it does not take is a row of games.
        let bar = if show_bar {
            (height / 15.0).round().clamp(10.0, 22.0)
        } else {
            0.0
        };
        // The title bar exists only on the settings screens, where there is
        // a name and an explanation that have nowhere else to go. Taking
        // its height off the list while browsing left a strip of nothing
        // along the bottom of the plain view: the room was reserved and
        // never drawn in.
        let body = (height - bar - if chrome_shown { chrome } else { 0.0 }).max(16.0);
        let pad = (width / 55.0).round().clamp(3.0, 14.0);

        let rows_layout = |rows: f32| {
            let row_height = (body / rows).floor().max(9.0);
            let visible = (body / row_height).floor().max(1.0) as usize;
            (row_height, visible)
        };

        if plain {
            let (row_height, visible) = rows_layout(10.0);
            return Geometry {
                chrome,
                bar,
                row_height,
                body_font: (row_height * 0.6).floor().max(8.0),
                small_font: (chrome * 0.5).floor().max(7.0),
                pad,
                art_width: 0.0,
                columns: 1,
                tile_width: 0.0,
                tile_height: 0.0,
                visible,
                stride: 1,
                inset_x,
                inset_y,
            };
        }

        match layout {
            Layout::Details => {
                let (row_height, visible) = rows_layout(8.0);
                Geometry {
                    chrome,
                    bar,
                    row_height,
                    body_font: (row_height * 0.62).floor().max(8.0),
                    small_font: (chrome * 0.5).floor().max(7.0),
                    pad,
                    // Half the screen each. The list needs room for a long
                    // title and the picture needs to be big enough to
                    // recognise a game from across a room.
                    art_width: (width * 0.5).round(),
                    columns: 1,
                    tile_width: 0.0,
                    tile_height: 0.0,
                    visible,
                    stride: 1,
                    inset_x,
                    inset_y,
                }
            }
            Layout::Carousel => {
                // One row across the whole body. The centre cover takes
                // half the width, which puts exactly half of each
                // neighbour on screen: enough to see what is coming
                // without pretending three things are equally in view.
                // Wide enough that the middle picture is the screen, narrow
                // enough that the two either side still show they are there.
                let tile_width = (width * 0.72).floor().max(48.0);
                Geometry {
                    chrome,
                    bar,
                    row_height: body,
                    body_font: (body * 0.09).floor().clamp(8.0, 20.0 * scale),
                    small_font: (body * 0.075).floor().clamp(7.0, 16.0 * scale),
                    pad,
                    art_width: 0.0,
                    columns: 1,
                    tile_width,
                    tile_height: body,
                    // The one either side, and the one in the middle.
                    visible: 3,
                    stride: 1,
                    inset_x,
                    inset_y,
                }
            }
            Layout::Tiled => {
                // Rows first, not columns. The screen this runs on is 352
                // by 240, and deciding the column count from a target cover
                // width there leaves a row's worth of empty space under the
                // grid. Choosing how many rows should fill the height and
                // fitting columns into what is left cannot: the tiles are
                // whatever size makes the rows meet the bottom.
                let target_rows = if body >= 380.0 { 3.0 } else { 2.0 };
                let tile_height = (body / target_rows).floor().max(28.0);
                // A little wider than tall: four by three artwork with a
                // caption under it.
                let columns = ((width / (tile_height * 1.25)).round() as usize).clamp(2, 8);
                let tile_width = (width / columns as f32).floor();
                let grid_rows = (body / tile_height).floor().max(1.0) as usize;
                Geometry {
                    chrome,
                    bar,
                    row_height: tile_height,
                    body_font: (tile_height * 0.18).floor().max(8.0),
                    small_font: (tile_height * 0.16).floor().max(7.0),
                    pad,
                    art_width: 0.0,
                    columns,
                    tile_width,
                    tile_height,
                    visible: columns * grid_rows,
                    stride: columns,
                    inset_x,
                    inset_y,
                }
            }
            Layout::List => {
                let (row_height, visible) = rows_layout(12.0);
                Geometry {
                    chrome,
                    bar,
                    row_height,
                    body_font: (row_height * 0.66).floor().max(8.0),
                    small_font: (chrome * 0.5).floor().max(7.0),
                    pad,
                    art_width: 0.0,
                    columns: 1,
                    tile_width: 0.0,
                    tile_height: 0.0,
                    visible,
                    stride: 1,
                    inset_x,
                    inset_y,
                }
            }
        }
    }
}

/// What the artwork cost, beyond what the cache reports.
#[derive(Debug, Clone, Copy, Default)]
pub struct ArtStats {
    pub loads: u64,
    /// Selection changes whose picture was never fetched because the scroll
    /// moved on. Shows the delay is doing its job.
    pub deferred: u64,
    pub worst_load_us: u64,
}

fn to_slint(color: ConfigColor) -> slint::Color {
    slint::Color::from_rgb_u8(color.r, color.g, color.b)
}

fn to_image(image: &crate::covers::RgbImage) -> slint::Image {
    let mut buffer = SharedPixelBuffer::<slint::Rgb8Pixel>::new(image.width, image.height);
    buffer.make_mut_bytes().copy_from_slice(&image.rgb);
    slint::Image::from_rgb8(buffer)
}

pub struct App {
    config: Config,
    settings: Settings,
    settings_path: PathBuf,

    systems: Vec<FoundSystem>,
    /// The groups present on this machine, with how many systems each has.
    categories: Vec<(String, usize)>,
    category_list: ListState,
    open_category: Option<String>,
    system_list: ListState,
    /// Which system is open, by its id from the table.
    ///
    /// Its id and not its place in the list: the list is rebuilt whenever a
    /// group is turned on or off or the empty systems are worked out, and a
    /// position taken before that points at a different machine afterwards.
    /// This decides which core a game is launched with, so pointing at a
    /// different machine is not a cosmetic mistake.
    open_system: Option<String>,
    /// The system currently open, and the folders entered inside it.
    library: Option<Library>,
    names: browse::DisplayNames,
    trail: Vec<Crumb>,
    /// The row every visited folder was left standing on, by the row's own
    /// key. The crumbs remember an index for walking straight back out;
    /// this remembers identity, so a folder that is re-entered later, or
    /// whose contents changed in between, still comes back to the same
    /// game. Saved with the position at launch and gone after a power
    /// cycle, like the rest of where the user was standing.
    left_at: Vec<crate::state::LeftAt>,
    /// The rows of the folder being shown.
    here: Vec<browse::Row>,
    game_list: ListState,
    menu_list: ListState,
    option_list: ListState,
    advanced_list: ListState,
    help_list: ListState,
    about_list: ListState,

    screen: Screen,
    browsing: Browsing,
    layout: Layout,
    geometry: Geometry,
    width: u32,
    height: u32,

    covers: CoverCache,
    ui: DegaussWindow,
    window: Rc<MinimalSoftwareWindow>,
    rows: Rc<VecModel<Row>>,

    speed: usize,
    show_art: bool,
    show_stats: bool,
    show_hidden: bool,
    /// Every system found, before hiding is applied. `systems` is the
    /// visible projection of this.
    all_systems: Vec<FoundSystem>,
    /// The folder on screen before a search narrowed it. Empty while
    /// nothing is being searched for, so the usual case pays nothing.
    all_here: Vec<browse::Row>,
    /// What is being searched for, in capitals and without spaces.
    filter: String,
    find_mode: FindMode,
    find_list: ListState,
    /// Which systems have nothing to play in them, once the card has been
    /// read for it. [`None`] until then.
    empty_systems: Option<HashSet<String>>,
    /// The open system's table entry, so it can be read on demand for the
    /// parts nobody wrote down.
    opened_config: Option<SystemConfig>,
    /// Systems something is hidden under, and what they really hold.
    corrected_counts: HashMap<String, usize>,
    /// Every game the cache knows about, for the About screen.
    total_games: usize,
    /// What MiSTer's own favourites folder holds, so a game can be marked
    /// wherever it is listed.
    favorites: crate::favorites::Favorites,
    /// Whether favourites are gathered at the top of a folder.
    favorites_first: bool,
    /// The typeface everything is set in.
    font: Font,
    random_launches: bool,
    /// Whether folders come after the games rather than before them.
    folders_last: bool,
    /// Where the written-down copy of the card lives.
    cache_dir: PathBuf,
    /// What is known about every system, read once at startup.
    index: Option<crate::cache::Index>,
    /// The open system's folders, when they have been written down.
    system_cache: Option<crate::cache::SystemCache>,
    /// Set while the card is being read into the cache, a system at a time
    /// so the screen can say how far it has got.
    build: Option<Building>,
    /// Which systems are worth looking in, narrowed as looks come back
    /// empty. Built on first use, not at startup: nothing should read the
    /// card before the first frame.
    saver_candidates: Option<Vec<usize>>,

    settled_since: Option<Instant>,
    /// Turns the selected title round at each end of its travel.
    marquee: Rc<slint::Timer>,
    /// The same for the lines under the picture, which travel at half the
    /// pace and would be cut off half way on the title's clock.
    detail_marquee: Rc<slint::Timer>,
    art_pending: bool,

    timer: FrameTimer,
    last_work: FrameWork,
    last_build: Duration,
    art: ArtStats,
    startup: StartupTimings,
    started: Instant,
    present_label: &'static str,

    pending: Option<Pending>,
    menu: Vec<String>,
    pending_present_switch: bool,
    /// A system whose metadata is to be read after the next frame is drawn.
    opening: Option<usize>,
    /// True when a group held one system and was stepped straight through.
    skipped_systems: bool,
    show_empty: bool,
    /// Show the group holding cores that are not games.
    show_other: bool,
    /// Show the group holding test and measurement cores.
    show_utility: bool,
    /// Show the strip along the bottom.
    show_bar: bool,
    /// Which logo each group is wearing at the moment.
    category_picks: std::collections::BTreeMap<String, PathBuf>,
    /// When something was last pressed, for deciding the machine is idle.
    last_input: Instant,
    /// The machine's own state for the bar, re-read on a timer.
    status: crate::status::Status,
    /// When the scroll speed last changed. The badge shows for a moment
    /// after, then goes away again: it answers a question only asked while
    /// the speed is being changed.
    speed_shown_at: Option<Instant>,
    /// The pictures the screensaver is walking through, kept in a ring
    /// rather than consumed, so it never runs out and stops.
    saver_pool: Vec<SaverPicture>,
    /// Pictures found and not yet shown, shuffled. The strip takes from
    /// here as it moves.
    saver_queue: Vec<SaverPicture>,
    /// How far the strip has travelled, in pixels.
    saver_offset: f32,
    saver_stepped: Instant,
    /// Where to go back to when it is woken.
    saver_return: Screen,
    /// Seed for picking a game at random.
    seed: u64,
    message: Option<String>,
    dirty: bool,
}

/// Everything read from disk before the interface exists: what the user
/// configured, what they have changed since, and what is on the card.
pub struct Loaded {
    pub config: Config,
    pub settings: Settings,
    pub settings_path: PathBuf,
    pub systems: Vec<FoundSystem>,
    /// The display names the card itself applies to cores and shortcuts.
    pub names: browse::DisplayNames,
}

impl App {
    pub fn new(
        loaded: Loaded,
        window: Rc<MinimalSoftwareWindow>,
        ui: DegaussWindow,
        startup: StartupTimings,
        width: u32,
        height: u32,
    ) -> Self {
        let show_empty = loaded.settings.show_empty.unwrap_or(false);
        let show_other = loaded.settings.show_other.unwrap_or(false);
        let show_utility = loaded.settings.show_utility.unwrap_or(false);
        let show_bar = loaded.settings.show_bar.unwrap_or(false);
        let Loaded {
            config,
            settings,
            settings_path,
            systems,
            names,
        } = loaded;
        let layout = settings
            .layout
            .as_deref()
            .and_then(Layout::parse)
            .or_else(|| Layout::parse(&config.app.layout))
            .unwrap_or(Layout::Details);
        // Margins are saved when changed and read back here. Without this the
        // Options screen would show the saved figure while the screen kept
        // the one from the config file, and the two would disagree.
        let mut config = config;
        if let Some(x) = settings.overscan_x {
            config.app.overscan_x = x;
        }
        if let Some(y) = settings.overscan_y {
            config.app.overscan_y = y;
        }

        // The drawing path is a setting like any other: saved when changed,
        // and read back here so a restart keeps it.
        let present_mode = settings
            .present
            .as_deref()
            .and_then(PresentMode::parse)
            .unwrap_or(PresentMode::Direct);
        let geometry = Geometry::compute(layout, false, false, show_bar, width, height, &config);

        let rows = Rc::new(VecModel::from(Vec::<Row>::new()));
        ui.set_rows(ModelRc::from(rows.clone()));

        let palette = &config.colors;
        ui.set_c_background(to_slint(palette.background));
        ui.set_c_panel(to_slint(palette.panel));
        ui.set_c_bar(to_slint(palette.bar));
        ui.set_c_surface(to_slint(palette.surface));
        ui.set_c_text(to_slint(palette.text));
        ui.set_c_text_dim(to_slint(palette.text_dim));
        ui.set_c_accent(to_slint(palette.accent));
        ui.set_c_accent_text(to_slint(palette.accent_text));
        ui.set_c_state(to_slint(palette.state));
        ui.set_c_favorite(to_slint(palette.favorite));

        let cover_size = config.app.cover_size.max(width.max(height) / 2);
        // Artwork with transparency is composited onto the colour it is
        // actually drawn on, which is the surface behind it.
        let ground = [palette.surface.r, palette.surface.g, palette.surface.b];
        let covers = CoverCache::new(cover_size, config.app.art_cache.max(8), ground);

        let system_count = systems.len();
        let mut app = App {
            speed: settings
                .speed_step
                .unwrap_or(SPEED_START)
                .min(SPEED_STEPS.len() - 1),
            show_art: settings.show_art.unwrap_or(true),
            show_stats: settings.show_stats.unwrap_or(config.app.show_stats),
            show_hidden: settings.show_hidden.unwrap_or(false),
            all_systems: Vec::new(),
            all_here: Vec::new(),
            filter: String::new(),
            find_mode: FindMode::Jump,
            find_list: ListState::new(FIND_CELLS.chars().count(), FIND_CELLS.chars().count()),
            empty_systems: None,
            favorites: crate::favorites::Favorites::default(),
            favorites_first: settings.favorites_first.unwrap_or(true),
            // The file the user edits, then the one Degauss writes, then the
            // typeface that always exists. A name neither of them recognises
            // is not worth refusing to start over.
            font: settings
                .font
                .as_deref()
                .and_then(Font::parse)
                .or_else(|| Font::parse(&config.app.font))
                .unwrap_or_default(),
            random_launches: settings.random_launches.unwrap_or(false),
            folders_last: settings.folders_last.unwrap_or(false),
            opened_config: None,
            corrected_counts: HashMap::new(),
            total_games: 0,
            cache_dir: crate::cache::dir_for(&settings_path),
            index: None,
            system_cache: None,
            build: None,
            saver_candidates: None,
            config,
            settings,
            settings_path,
            systems,
            categories: Vec::new(),
            category_list: ListState::new(0, geometry.visible),
            open_category: None,
            system_list: ListState::new(system_count, geometry.visible),
            open_system: None,
            library: None,
            names,
            trail: Vec::new(),
            left_at: Vec::new(),
            here: Vec::new(),
            game_list: ListState::new(0, geometry.visible),
            menu_list: ListState::new(0, geometry.visible),
            option_list: ListState::new(OPTIONS.len(), geometry.visible),
            advanced_list: ListState::new(ADVANCED.len(), geometry.visible),
            about_list: ListState::new(1, geometry.visible),
            help_list: ListState::new(HELP.len(), geometry.visible),
            screen: Screen::Splash,
            browsing: Browsing::Categories,
            layout,
            geometry,
            width,
            height,
            covers,
            ui,
            window,
            rows,
            settled_since: Some(Instant::now()),
            marquee: Rc::new(slint::Timer::default()),
            detail_marquee: Rc::new(slint::Timer::default()),
            art_pending: true,
            timer: FrameTimer::new(),
            last_work: FrameWork::default(),
            last_build: Duration::ZERO,
            art: ArtStats::default(),
            startup,
            started: Instant::now(),
            present_label: present_mode.label(),
            pending: None,
            menu: Vec::new(),
            show_empty,
            show_other,
            show_utility,
            show_bar,
            category_picks: std::collections::BTreeMap::new(),
            last_input: Instant::now(),
            status: crate::status::Status::read(),
            speed_shown_at: None,
            saver_pool: Vec::new(),
            saver_queue: Vec::new(),
            saver_offset: 0.0,
            saver_stepped: Instant::now(),
            saver_return: Screen::Browse,
            seed: seed_from_clock(),
            pending_present_switch: false,
            opening: None,
            skipped_systems: false,
            message: None,
            dirty: true,
        };
        app.all_systems = std::mem::take(&mut app.systems);
        app.rebuild_system_list();
        // What was written down last time, if anything. Reading it is a
        // few milliseconds against the seconds walking the card costs,
        // which is the whole reason it exists.
        app.reread_favorites();
        app.index = crate::cache::load_index(&app.cache_dir);
        app.correct_system_counts();
        match app.index.is_some() {
            true => app.apply_index(),
            // Nothing written down yet: read the card once, with the
            // wordmark and a line saying so on screen while it happens.
            false => app.start_build(false),
        }
        app.rebuild_system_list();
        app.ui.set_about_version(SharedString::from(format!(
            "version {}",
            env!("CARGO_PKG_VERSION")
        )));
        // What it can say without walking the card: how many systems have a
        // folder here. A total game count would mean reading every folder on
        // the machine before the first frame, which is the thing this does
        // not do.
        app.ui
            .set_about_line(SharedString::from(match app.total_games {
                0 => format!("{} systems on this card", app.all_systems.len()),
                games => format!("{} systems, {games} games", app.all_systems.len()),
            }));
        app.ui.set_about_copyright(SharedString::from(COPYRIGHT));
        app.ui.set_about_licence(SharedString::from(LICENCE));
        app.apply_geometry();
        app
    }

    /// Whether the strip along the bottom is drawn on the screen showing
    /// now.
    ///
    /// A menu is a place to read, so it keeps the strip whatever the
    /// setting says; browsing is where the height is worth more as another
    /// row of games, so that is what the setting covers.
    fn bar_here(&self) -> bool {
        match self.screen {
            Screen::Menu
            | Screen::Context
            | Screen::Options
            | Screen::Advanced
            | Screen::Help
            | Screen::Find
            | Screen::FavoriteFolder => true,
            Screen::Browse => self.show_bar,
            Screen::About | Screen::Splash | Screen::Screensaver => false,
        }
    }

    /// Whether the strip along the top is drawn on the screen showing now.
    ///
    /// The menus and the grid of letters carry a name and an explanation
    /// there; browsing carries neither, whatever view it is in.
    fn chrome_here(&self) -> bool {
        matches!(self.screen.ui_index(), 1 | 4)
    }

    fn plain_screen(&self) -> bool {
        self.screen != Screen::Browse || self.layout == Layout::List
    }

    /// Dismiss the wordmark and start browsing.
    fn leave_splash(&mut self) {
        if self.screen == Screen::Splash {
            self.screen = Screen::Browse;
            self.apply_geometry();
            self.touch_selection();
        }
    }

    fn apply_geometry(&mut self) {
        let geometry = Geometry::compute(
            self.layout,
            self.plain_screen(),
            self.chrome_here(),
            self.bar_here(),
            self.width,
            self.height,
            &self.config,
        );
        self.geometry = geometry;
        for list in [
            &mut self.category_list,
            &mut self.system_list,
            &mut self.game_list,
            &mut self.menu_list,
            &mut self.option_list,
            &mut self.advanced_list,
            &mut self.help_list,
            &mut self.about_list,
        ] {
            list.reshape(geometry.visible, 1);
        }
        // Only the browse screen uses a grid.
        if self.screen == Screen::Browse && self.layout == Layout::Tiled {
            self.active_list_mut()
                .reshape(geometry.visible, geometry.stride);
        }

        // A menu that has to scroll to show what it offers is a menu that
        // hides half of it. Where the entries do not fit, the rows are made
        // shorter until they do, which on the tube is the difference
        // between seeing every choice and seeing eight of twelve.
        if matches!(self.screen, Screen::Menu | Screen::Context)
            && !self.menu.is_empty()
            && self.menu.len() > geometry.visible
        {
            let body = geometry.row_height * geometry.visible as f32;
            let row_height = (body / self.menu.len() as f32).floor().max(9.0);
            self.geometry.row_height = row_height;
            self.geometry.visible = self.menu.len();
            self.geometry.body_font = (row_height * 0.6).floor().max(8.0);
            self.menu_list.reshape(self.menu.len(), 1);
        }
        let geometry = self.geometry;

        self.ui.set_screen(self.screen.ui_index());
        self.ui.set_layout(self.layout.index());
        self.ui.set_row_height(geometry.row_height);
        self.ui.set_body_font(geometry.body_font);
        self.ui.set_small_font(geometry.small_font);
        // The sizes above space the layout; these are the sizes text is
        // actually drawn at, which have to be sizes the typeface was baked
        // at. Asking for one it was not is not an error: it quietly draws
        // the largest smaller one, so the asking is done here instead.
        self.ui.set_font_family(self.font.family().into());
        self.ui
            .set_body_glyph(self.font.quantise(geometry.body_font));
        self.ui
            .set_small_glyph(self.font.quantise(geometry.small_font));
        // A system with no logo, and the mark standing in for a picture in
        // the favourites folder. Both are set relative to the body text.
        self.ui
            .set_caption_glyph(self.font.quantise(geometry.body_font * 1.7));
        self.ui
            .set_heart_glyph(self.font.quantise(geometry.body_font * 6.0));
        self.ui.set_pad(geometry.pad);
        self.ui.set_chrome_height(geometry.chrome);
        self.ui.set_bar_height(geometry.bar);
        self.ui.set_show_bar(self.bar_here());
        // The lines under the picture. Small, and never more than a third
        // of the panel: the picture is what is being looked at. Nothing at
        // all where there are no games, so the groups keep the whole panel
        // for the logo.
        self.ui
            .set_show_brand(self.screen == Screen::Browse && self.browsing == Browsing::Categories);
        self.apply_detail_panel();
        // 7) A handful of entries look lost against the top of the screen.
        self.ui.set_center_rows(
            self.screen == Screen::Browse && self.browsing == Browsing::Categories,
        );
        // Small, but never so small the strip is a smudge.
        let bar_font = (geometry.bar * 0.62).floor().max(7.0);
        self.ui.set_bar_font(bar_font);
        self.ui.set_bar_glyph(self.font.quantise(bar_font));
        // The full legend needs room the CRT does not have.
        self.ui.set_wide_bar(self.width >= 480);
        self.ui.set_art_width(geometry.art_width);
        self.ui.set_columns(geometry.columns as i32);
        self.ui.set_tile_width(geometry.tile_width);
        self.ui.set_tile_height(geometry.tile_height);
        if self.screen == Screen::Find {
            // The grid sizes itself from the screen rather than from the
            // list geometry, which is measured for rows of text.
            let cells = FIND_CELLS.chars().count();
            self.ui.set_columns(FIND_COLUMNS as i32);
            self.ui
                .set_grid_rows(cells.div_ceil(FIND_COLUMNS).max(1) as i32);
            self.ui.set_find_search(self.find_mode != FindMode::Jump);
        }
        // The margin, then the nudge: one side gains what the other gives
        // up, so the picture moves without changing size. Clamped so a
        // nudge larger than the margin cannot push an edge off the screen.
        let shift_x = (self.shift_x() as f32).clamp(-geometry.inset_x, geometry.inset_x);
        let shift_y = (self.shift_y() as f32).clamp(-geometry.inset_y, geometry.inset_y);
        self.ui.set_inset_left(geometry.inset_x + shift_x);
        self.ui.set_inset_right(geometry.inset_x - shift_x);
        self.ui.set_inset_top(geometry.inset_y + shift_y);
        self.ui.set_inset_bottom(geometry.inset_y - shift_y);
        self.dirty = true;
    }

    fn active_list(&self) -> &ListState {
        match self.screen {
            Screen::Browse => match self.browsing {
                Browsing::Categories => &self.category_list,
                Browsing::Systems => &self.system_list,
                Browsing::Games => &self.game_list,
            },
            Screen::Splash | Screen::Screensaver => &self.about_list,
            Screen::Menu | Screen::Context => &self.menu_list,
            Screen::Options => &self.option_list,
            Screen::Advanced => &self.advanced_list,
            Screen::Help => &self.help_list,
            Screen::About => &self.about_list,
            Screen::Find => &self.find_list,
            Screen::FavoriteFolder => &self.menu_list,
        }
    }

    fn active_list_mut(&mut self) -> &mut ListState {
        match self.screen {
            Screen::Browse => match self.browsing {
                Browsing::Categories => &mut self.category_list,
                Browsing::Systems => &mut self.system_list,
                Browsing::Games => &mut self.game_list,
            },
            Screen::Splash | Screen::Screensaver => &mut self.about_list,
            Screen::Menu | Screen::Context => &mut self.menu_list,
            Screen::Options => &mut self.option_list,
            Screen::Advanced => &mut self.advanced_list,
            Screen::Help => &mut self.help_list,
            Screen::About => &mut self.about_list,
            Screen::Find => &mut self.find_list,
            Screen::FavoriteFolder => &mut self.menu_list,
        }
    }

    /// Which settings list the current screen is showing.
    fn option_ids(&self) -> &'static [OptionId] {
        if self.screen == Screen::Advanced {
            &ADVANCED
        } else {
            &OPTIONS
        }
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn speed_ms(&self) -> u64 {
        SPEED_STEPS[self.speed.min(SPEED_STEPS.len() - 1)].1
    }

    fn shift_x(&self) -> i32 {
        self.settings.shift_x.unwrap_or(0).clamp(-64, 64)
    }

    fn shift_y(&self) -> i32 {
        self.settings.shift_y.unwrap_or(0).clamp(-64, 64)
    }

    /// The fastest scroll speed that still loads a picture for every row.
    ///
    /// Above it the pictures wait for the list to stop, because at eleven
    /// times the baseline rate nobody is looking at them anyway and the
    /// decoding is what makes the scrolling stop being smooth.
    fn art_limit(&self) -> usize {
        self.settings
            .art_limit
            .unwrap_or(self.config.app.art_limit)
            .min(SPEED_STEPS.len() - 1)
    }

    fn touch_selection(&mut self) {
        if self.art_pending {
            self.art.deferred += 1;
        }
        self.art_pending = true;
        self.settled_since = Some(Instant::now());
        self.restart_marquee();
        self.restart_detail_marquee();
        self.dirty = true;
    }

    /// How far up and down move.
    ///
    /// One row in a list, and one cover in a grid. A grid is read the way a
    /// page is: along the row, then down to the start of the next. Moving a
    /// whole row at a time would leave the covers beside this one with no
    /// key that reaches them, since left and right are the scroll speed
    /// here as they are everywhere else.
    fn browse_step(&self) -> isize {
        if self.screen == Screen::Browse && self.layout.is_grid() {
            1
        } else {
            self.active_list().stride() as isize
        }
    }

    /// A picture for a group: whichever system lent its logo this time, or
    /// a file named after the group if one was put in the logos folder.
    fn category_logo(&self, category: &str) -> Option<PathBuf> {
        self.category_picks.get(category).cloned()
    }

    /// How long the machine has been left alone before the screensaver
    /// starts, in seconds. Zero turns it off.
    fn screensaver_after(&self) -> u64 {
        self.settings.screensaver_after.unwrap_or(120)
    }

    /// Show something, or stay browsing if there is nothing to show.
    ///
    /// A screensaver that draws a blank screen is worse than none: it looks
    /// like the machine has died.
    fn enter_screensaver(&mut self) {
        self.refill_saver();
        if self.saver_pool.is_empty() {
            // Nothing found this time. Come back sooner than a whole idle
            // period, or a card that answers slowly once means no pictures
            // for another minute.
            self.last_input = Instant::now()
                - Duration::from_secs(self.screensaver_after().saturating_sub(SAVER_RETRY_SECONDS));
            return;
        }
        crate::note(&format!(
            "screensaver  starting with {} pictures",
            self.saver_pool.len()
        ));
        self.saver_queue.clear();
        self.saver_return = self.screen;
        self.screen = Screen::Screensaver;
        self.saver_offset = 0.0;
        self.saver_stepped = Instant::now();
        self.apply_geometry();
        self.dirty = true;
    }

    /// How wide one picture is on the strip.
    ///
    /// Tall enough to fill the screen and wide enough for a four by three
    /// screenshot to do so without letterboxing: a band of unchanging grey
    /// along the top and bottom is exactly what a screensaver is for
    /// avoiding on a tube.
    fn saver_cell(&self) -> f32 {
        (self.height as f32 * 4.0 / 3.0).floor().max(32.0)
    }

    /// Move the strip along, and top it up from another system now and then.
    ///
    /// It never stops on its own: pictures that have scrolled off the left
    /// are dropped, the queue refills the strip, and a fresh system is drawn
    /// on whenever the queue runs dry.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn advance_saver(&mut self, now: Instant) {
        let elapsed = now.duration_since(self.saver_stepped).as_secs_f32();
        self.saver_stepped = now;
        // A drift rather than a slide: fast enough to be moving, slow
        // enough that no part of the screen holds still for a tube.
        self.saver_offset += elapsed * SAVER_PIXELS_PER_SECOND;

        // A picture that has gone off the left is finished with. Dropping
        // it and taking the next off the queue is what stops the strip
        // being the same handful going round: the old ring had no way to
        // put anything new in it.
        let cell = self.saver_cell();
        while cell > 0.0 && self.saver_offset >= cell && !self.saver_pool.is_empty() {
            self.saver_pool.remove(0);
            self.saver_offset -= cell;
        }
        self.take_from_queue();

        // Out of pictures: another system, chosen the same way as the
        // first.
        if self.saver_queue.is_empty() && self.saver_pool.len() < SAVER_POOL_MAX {
            self.refill_saver();
        }
        self.dirty = true;
    }

    fn leave_screensaver(&mut self) {
        self.screen = self.saver_return;
        self.saver_pool.clear();
        self.saver_queue.clear();
        self.saver_offset = 0.0;
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }

    /// Gather pictures from one system picked at random.
    ///
    /// One system at a time, not a sweep of the card: reading a system's
    /// metadata is the expensive part, so it is worth several pictures once
    /// paid for. Systems with no metadata have no pictures and are skipped.
    /// The systems that could possibly show a picture.
    ///
    /// Covers only ever come from a `gamelist.xml`, so one stat per system
    /// rules most of them out before paying for a parse, which matters
    /// because this runs in the frame loop.
    fn saver_candidates(&mut self) -> &mut Vec<usize> {
        if self.saver_candidates.is_none() {
            self.saver_candidates = Some(
                (0..self.all_systems.len())
                    .filter(|&index| {
                        self.all_systems[index]
                            .paths
                            .iter()
                            .any(|root| root.join("gamelist.xml").is_file())
                    })
                    .collect(),
            );
        }
        self.saver_candidates.as_mut().expect("just filled")
    }

    /// Take a fresh handful of pictures from one system, in no order.
    ///
    /// One system at a time on purpose: a screensaver that jumps between
    /// machines every picture reads as a slideshow of somebody else's
    /// card. What it must not do is show the same twenty-four for ever,
    /// which is what it did when the pool was a ring: it filled once and
    /// then had no room to put anything new.
    ///
    /// So the pictures found are shuffled and queued, the strip takes them
    /// one at a time as it moves, and when the queue runs out another
    /// system is picked. Nothing is shown twice until a system is
    /// exhausted, and the order is never the order they sit on the card.
    fn refill_saver(&mut self) {
        let mut seed = self.seed;
        let began = Instant::now();

        loop {
            if began.elapsed() >= Duration::from_millis(SAVER_BUDGET_MS) {
                crate::note("screensaver  out of time, will look again shortly");
                break;
            }
            let candidates = self.saver_candidates();
            if candidates.is_empty() {
                break;
            }
            let at = (next_random(&mut seed) as usize) % candidates.len();
            let index = candidates[at];
            let Some(system) = self.all_systems.get(index) else {
                continue;
            };
            let name = system.name().to_string();
            let id = system.def.id.clone();
            let config = system.to_config();

            // What was written down, if it is there. Opening a system to
            // find pictures means parsing its gamelist, seconds of it, and
            // this runs while somebody is looking at the screen.
            let mut found: Vec<(PathBuf, String)> = Vec::new();
            match crate::cache::load_system(&self.cache_dir, &id) {
                Some(cache) => {
                    for folder in cache.folders.values() {
                        for row in &folder.rows {
                            if let Some(cover) = row.cover.clone() {
                                found.push((cover, row.name.clone()));
                            }
                            if found.len() >= SAVER_WANTED {
                                break;
                            }
                        }
                        if found.len() >= SAVER_WANTED {
                            break;
                        }
                    }
                }
                None => {
                    if let Ok(library) = Library::open_with_names(&config, self.names.clone()) {
                        found = library.covers(SAVER_FOLDERS_SEARCHED, SAVER_WANTED);
                    }
                }
            }

            if found.is_empty() {
                // Nothing here, and nothing will appear while this run
                // lasts: plenty of systems hold a gamelist and artwork but
                // no games. Drop it so the next look does not roll the
                // same dice again.
                crate::note(&format!("screensaver  nothing in {name}, dropped"));
                self.saver_candidates().swap_remove(at);
                continue;
            }

            // Shuffled where they are found, not where they are shown: a
            // card lists its games alphabetically and a screensaver that
            // walks an alphabet is a directory listing with pictures.
            for i in (1..found.len()).rev() {
                let j = (next_random(&mut seed) as usize) % (i + 1);
                found.swap(i, j);
            }

            self.saver_queue = found
                .into_iter()
                .map(|(path, title)| SaverPicture {
                    path,
                    caption: saver_caption(&title, &name),
                })
                .collect();
            self.seed = seed;
            crate::note(&format!(
                "screensaver  {} pictures from {name}",
                self.saver_queue.len()
            ));
            self.take_from_queue();
            return;
        }

        self.seed = seed;
    }

    /// Move pictures from the queue onto the strip until it is full.
    fn take_from_queue(&mut self) {
        while self.saver_pool.len() < SAVER_POOL_MAX {
            let Some(next) = self.saver_queue.pop() else {
                return;
            };
            self.saver_pool.push(next);
        }
    }

    /// Send the selected title back to its start and begin its wait again.
    ///
    /// Restarted on every move, so a title only walks once the list has been
    /// left alone on it. Scrolling past a hundred rows never starts one.
    fn restart_marquee(&mut self) {
        self.ui.set_marquee_end(false);
        self.marquee.stop();
        let ui = self.ui.as_weak();
        let timer = Rc::clone(&self.marquee);
        // The wait first, on its own. Starting a repeating timer here and
        // waiting for its first tick made the title sit still for the whole
        // period, wait plus travel together, before it moved at all.
        self.marquee.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(MARQUEE_WAIT_MS),
            move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_marquee_end(true);
                }
                // From here it turns round at each end, waiting the same
                // moment at each before setting off again.
                let ui = ui.clone();
                timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(MARQUEE_WAIT_MS + MARQUEE_TRAVEL_MS),
                    move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_marquee_end(!ui.get_marquee_end());
                        }
                    },
                );
            },
        );
    }

    /// The same again for the lines under the picture, on their own clock.
    fn restart_detail_marquee(&mut self) {
        self.ui.set_detail_end(false);
        self.detail_marquee.stop();
        let ui = self.ui.as_weak();
        let timer = Rc::clone(&self.detail_marquee);
        self.detail_marquee.start(
            slint::TimerMode::SingleShot,
            Duration::from_millis(MARQUEE_WAIT_MS),
            move || {
                if let Some(ui) = ui.upgrade() {
                    ui.set_detail_end(true);
                }
                let ui = ui.clone();
                timer.start(
                    slint::TimerMode::Repeated,
                    Duration::from_millis(MARQUEE_WAIT_MS + DETAIL_TRAVEL_MS),
                    move || {
                        if let Some(ui) = ui.upgrade() {
                            ui.set_detail_end(!ui.get_detail_end());
                        }
                    },
                );
            },
        );
    }

    /// Start reading a system's library.
    ///
    /// The walk is incremental: a large library holds thousands of
    /// directories and reading one takes milliseconds, so doing it all at
    /// once would freeze the screen for the best part of a minute. The loop
    /// steps it between frames and shows how far along it is.
    /// Say what is about to happen, then do it on the next frame.
    ///
    /// Reading a system means parsing its metadata: the Commodore 64's
    /// gamelist alone is forty thousand entries and its artwork directory
    /// holds sixteen thousand files, which is seconds of work on this
    /// hardware. Doing that in the frame the button was pressed shows a
    /// still screen with no explanation, so the message is drawn first and
    /// the work happens after it is on screen.
    fn open_selected_system(&mut self) {
        let Some(system) = self.systems.get(self.system_list.selected()) else {
            return;
        };
        self.message = Some(format!("Reading {}", system.name()));
        self.opening = Some(self.system_list.selected());
        self.dirty = true;
    }

    fn open_system_now(&mut self) {
        let Some(system) = self.systems.get(self.system_list.selected()) else {
            return;
        };
        let id = system.def.id.clone();
        let config: SystemConfig = system.to_config();
        let name = system.name().to_string();

        // What was written down, if anything. Where a system starts is
        // decided by how it was declared and costs nothing to work out, so
        // a system already written down is opened without being read: a large
        // system's gamelist runs to tens of megabytes and seconds of parsing.
        self.system_cache = crate::cache::load_system(&self.cache_dir, &id);
        self.opened_config = Some(config.clone());
        if self.system_cache.is_some() {
            self.library = None;
            self.trail.clear();
            self.open_system = Some(id);
            self.enter(browse::start_for(&config));
            return;
        }

        match Library::open_with_names(&config, self.names.clone()) {
            Ok(library) => {
                let start = library.start();
                self.library = Some(library);
                self.trail.clear();
                self.open_system = Some(id);
                self.enter(start);
            }
            Err(e) => {
                // Say what went wrong rather than showing an empty list.
                self.message = Some(format!("{name}: {e}"));
                self.dirty = true;
            }
        }
    }

    /// Write down the row the folder on screen is standing on, so coming
    /// back to this folder lands there again.
    ///
    /// Keyed by the system as well as the place, because place keys are
    /// only unique within one system: every system's root shares the same
    /// key. Nothing to write when no folder is on screen or it is empty.
    fn remember_here(&mut self) {
        if self.browsing != Browsing::Games {
            return;
        }
        let Some(system) = self.open_system.clone() else {
            return;
        };
        let Some(crumb) = self.trail.last() else {
            return;
        };
        let Some(row) = self.here.get(self.game_list.selected()) else {
            return;
        };
        crate::state::remember_left_at(
            &mut self.left_at,
            crate::state::LeftAt {
                system,
                place: crumb.place.key(),
                row: row_key(row),
            },
        );
    }

    /// Walk into a folder and show it.
    fn enter(&mut self, place: Place) {
        // By identity too: the crumb below keeps an index for walking
        // straight back out, but a later visit through a changed list
        // needs the row itself to find the place again.
        self.remember_here();
        if let Some(crumb) = self.trail.last_mut() {
            // Remember where we were standing, so coming back lands there
            // rather than at the top of a list of thousands.
            crumb.selected = self.game_list.selected();
        }
        self.trail.push(Crumb { place, selected: 0 });
        self.show_here();
    }

    /// Walk back out. False when there is nothing left to walk out of, and
    /// the caller should leave the system entirely.
    fn leave(&mut self) -> bool {
        // The folder being left keeps its place. This is the only moment
        // its cursor is still alive; without it, re-entering starts at the
        // top every time.
        self.remember_here();
        self.trail.pop();
        if self.trail.is_empty() {
            return false;
        }
        self.show_here();
        true
    }

    /// List whatever the trail currently points at.
    /// The rows of a place: what was written down if it is there, and the
    /// card itself if it is not.
    ///
    /// Written-down rows are unfiltered, because what is on the card does
    /// not depend on a setting. Hiding the empty ones is then a comparison
    /// rather than a walk of everything underneath.
    fn listing(&mut self, place: &Place) -> Result<Vec<browse::Row>> {
        if let Some(folder) = self.system_cache.as_ref().and_then(|c| c.get(place)) {
            let mut rows = folder.rows.clone();
            if !self.show_empty {
                rows.retain(|row| row.below != Some(0));
            }
            return Ok(rows);
        }
        self.open_library()?;
        let library = self.library.as_ref().ok_or_else(|| {
            crate::error::DegaussError::unsupported("browse", "no system open".to_string())
        })?;
        library.list(place, self.show_empty).map(|(rows, _)| rows)
    }

    /// Read the open system, for the parts of it nobody wrote down.
    fn open_library(&mut self) -> Result<()> {
        if self.library.is_some() {
            return Ok(());
        }
        let config = self.opened_config.clone().ok_or_else(|| {
            crate::error::DegaussError::unsupported("browse", "no system open".to_string())
        })?;
        self.library = Some(Library::open_with_names(&config, self.names.clone())?);
        Ok(())
    }

    /// Read MiSTer's favourites folder again.
    ///
    /// Cheap: a couple of hundred small files. Done on the way in and
    /// after anything is favourited, never on a timer.
    fn reread_favorites(&mut self) {
        let root = PathBuf::from(&self.config.menu_root).join(crate::favorites::FAVORITES_DIR);
        self.favorites = crate::favorites::Favorites::read(&root);
        crate::note(&format!(
            "favourites   {} in {}",
            self.favorites.len(),
            root.display()
        ));
    }

    fn favorites_root(&self) -> PathBuf {
        PathBuf::from(&self.config.menu_root).join(crate::favorites::FAVORITES_DIR)
    }

    /// Write the Favorites system's cache again, from the folder as it is
    /// now.
    ///
    /// Favourites is a system like any other, so its listing comes from the
    /// cache, and the cache is only rebuilt from the menu. But favouriting
    /// is the one change to the card Degauss makes itself, so it knows the
    /// exact moment that folder moved, and a shelf that shows yesterday's
    /// favourites until a full rebuild is asked for is wrong. The folder is
    /// small, so reading this one system again costs nothing worth noticing.
    /// A failure comes back to the caller instead of onto the screen:
    /// every caller redraws with `show_here`, which clears the message
    /// field, so a message set here would be wiped before it was drawn.
    /// The caller shows it after its redraw.
    fn refresh_favorites_system(&mut self) -> Option<String> {
        // The Favorites system is in the table only when its folder existed
        // at startup. With no folder there is no cache to refresh and
        // nothing is listed, so doing nothing is correct.
        let system = self.all_systems.iter().find(|s| s.def.id == FAVORITES_ID)?;
        let config = system.to_config();
        let library = match Library::open_with_names(&config, self.names.clone()) {
            Ok(library) => library,
            // Said out loud rather than quietly keeping the stale listing.
            Err(e) => return Some(format!("Favorites: {e}")),
        };
        let cache = crate::cache::build_system(&library);
        if let Err(e) = crate::cache::save_system(&self.cache_dir, FAVORITES_ID, &cache) {
            // Stop before the index learns the new summary: an index saying
            // one count while the cache file on disk holds another survives
            // a restart as a listing that disagrees with itself.
            crate::note(&format!("cache        {FAVORITES_ID} not written: {e}"));
            return Some(format!("Favorites not written: {e}"));
        }
        let mut error = None;
        if let Some(index) = self.index.as_mut() {
            // Only this system's summary is replaced. The index carries
            // every other system's too, and those are still right.
            index.systems.insert(
                FAVORITES_ID.to_string(),
                cache.summary(&browse::start_for(&config)),
            );
            // Written to disk only when no build is running. A forced
            // build has already emptied the cache folder and is filling a
            // fresh index one system at a time; writing this one to disk in
            // the middle of that would leave, after a power cut, an index
            // whose systems have no cache files, and startup trusts an
            // index that exists. The running build is told about the change
            // below and writes the finished index itself.
            if self.build.is_none() {
                if let Err(e) = crate::cache::save_index(&self.cache_dir, index) {
                    // The listing in memory is right either way, so the rest
                    // of the propagation still runs; only the disk is stale,
                    // and the user is told rather than left to find out at
                    // the next start.
                    crate::note(&format!("cache        index not written: {e}"));
                    error = Some(format!("Favorites index not written: {e}"));
                }
            }
            // The first favourite ever kept takes the count from nothing to
            // something, and a system holding nothing is hidden at the
            // root. The emptiness answers come from the index, so they are
            // worked out again.
            self.apply_index();
        }
        if let Some(build) = self.build.as_mut() {
            // A build runs a system per frame with the controls still
            // live, and what it finishes with replaces the index outright.
            // A favourite changed after the build already passed Favorites
            // would be overwritten by the summary the build saw, so the
            // build's copy is told too.
            build.index.systems.insert(
                FAVORITES_ID.to_string(),
                cache.summary(&browse::start_for(&config)),
            );
        }
        if self.open_system.as_deref() == Some(FAVORITES_ID) {
            // The rows on screen are answered from this while a system is
            // open. Removing a favourite from inside Favourites redraws
            // straight after, and must not redraw from the old copy. When
            // some other system is open its own cache is the one loaded
            // here, and replacing it would be wrong.
            self.system_cache = Some(cache);
        }
        self.rebuild_system_list();
        error
    }

    /// Mark what is favourited, and gather it if that is wanted.
    ///
    /// The card decides, not the gamelist: a favourite is a file MiSTer's
    /// own script wrote, and its `<favorite>` tag is a different thing that
    /// a scraper may or may not have filled in.
    /// Remember how this folder was being looked at.
    ///
    /// Only folders the view was actually changed in are written down, so
    /// the file does not grow with every folder ever opened.
    fn remember_view(&mut self) {
        if self.browsing != Browsing::Games {
            return;
        }
        let Some(crumb) = self.trail.last() else {
            return;
        };
        self.settings
            .folder_views
            .insert(crumb.place.key(), self.layout.label().to_string());
    }

    /// Look at this folder the way it was last looked at, if it was ever
    /// said. Otherwise leave the view alone: arriving somewhere new should
    /// not change what is on screen.
    fn recall_view(&mut self) {
        let Some(crumb) = self.trail.last() else {
            return;
        };
        let Some(name) = self.settings.folder_views.get(&crumb.place.key()).cloned() else {
            return;
        };
        let Some(layout) = Layout::parse(&name) else {
            return;
        };
        if layout != self.layout {
            self.layout = layout;
        }
    }

    /// Step over the blank lines that separate groups in a menu.
    ///
    /// They are there to be read, not chosen. Bounded by the number of
    /// entries, so a menu that somehow held nothing else cannot spin.
    fn skip_blank_menu(&mut self, delta: isize) {
        if !matches!(self.screen, Screen::Menu | Screen::Context) {
            return;
        }
        for _ in 0..self.menu.len() {
            let at = self.menu_list.selected();
            if self.menu.get(at).is_none_or(|entry| !entry.is_empty()) {
                return;
            }
            self.menu_list.move_items(delta);
        }
    }

    /// Give a favourite the picture and the words its game already has.
    ///
    /// A favourite is a file in a folder of its own, with no gamelist
    /// beside it, so on its own it has nothing to show. What it points at
    /// does. The target is read out of the favourite, the system that owns
    /// it is found by the folder the target sits in, and that system's
    /// written-down listing is walked once for all of them together: a
    /// pass over one system rather than a lookup per row.
    fn enrich_favorites(&mut self, rows: &mut [browse::Row]) {
        if self.open_system.as_deref() != Some(FAVORITES_ID) {
            return;
        }
        // What each row points at, and where it sits in this list.
        let mut wanted: HashMap<PathBuf, Vec<usize>> = HashMap::new();
        for (at, row) in rows.iter().enumerate() {
            let browse::Kind::Play(browse::Launch::File(path)) = &row.kind else {
                continue;
            };
            let Some(target) = crate::favorites::target_of(path) else {
                continue;
            };
            wanted.entry(target).or_default().push(at);
        }
        if wanted.is_empty() {
            return;
        }

        // Grouped by the system that owns them, so each is read once.
        let mut by_system: HashMap<String, Vec<PathBuf>> = HashMap::new();
        for target in wanted.keys() {
            if let Some(system) = self.owner_of(target) {
                by_system.entry(system).or_default().push(target.clone());
            }
        }

        for (id, targets) in by_system {
            let Some(cache) = crate::cache::load_system(&self.cache_dir, &id) else {
                continue;
            };
            let looking: HashSet<&PathBuf> = targets.iter().collect();
            for folder in cache.folders.values() {
                for cached in &folder.rows {
                    let Some(path) = row_target(cached) else {
                        continue;
                    };
                    if !looking.contains(&path) {
                        continue;
                    }
                    let Some(places) = wanted.get(&path) else {
                        continue;
                    };
                    for at in places {
                        if let Some(row) = rows.get_mut(*at) {
                            row.cover = cached.cover.clone();
                            row.details = cached.details.clone();
                        }
                    }
                }
            }
        }
    }

    /// Which system's folders a path sits in, deepest first so a system
    /// inside another system's folder answers for its own.
    fn owner_of(&self, path: &Path) -> Option<String> {
        self.all_systems
            .iter()
            .filter(|system| system.def.id != FAVORITES_ID)
            .filter(|system| system.paths.iter().any(|dir| path.starts_with(dir)))
            .max_by_key(|system| {
                system
                    .paths
                    .iter()
                    .filter(|dir| path.starts_with(dir))
                    .map(|dir| dir.as_os_str().len())
                    .max()
                    .unwrap_or(0)
            })
            .map(|system| system.def.id.clone())
    }

    /// Correct the counts for anything hidden underneath.
    ///
    /// The counts written down were counted before anything was hidden, so
    /// a folder whose only games are hidden would still claim to hold
    /// them. Costs nothing at all while nothing is hidden, which is the
    /// usual case; when something is, the written-down tree is walked and
    /// the hidden branches are not followed.
    fn correct_counts(&self, rows: &mut [browse::Row]) {
        if self.settings.hidden_paths.is_empty() {
            return;
        }
        let Some(cache) = self.system_cache.as_ref() else {
            return;
        };
        for row in rows.iter_mut() {
            if row.below.is_none() {
                continue;
            }
            if let browse::Kind::Enter(place) = &row.kind {
                row.below = Some(self.count_under(cache, place, 0));
            }
        }
    }

    /// How many playable things are under a place, skipping what is hidden.
    fn count_under(&self, cache: &crate::cache::SystemCache, place: &Place, depth: usize) -> usize {
        if depth > browse::MAX_DEPTH {
            return 0;
        }
        let Some(folder) = cache.get(place) else {
            return 0;
        };
        let mut held = 0;
        for row in &folder.rows {
            if self.settings.hidden_paths.contains(&row_key(row)) {
                continue;
            }
            match &row.kind {
                browse::Kind::Play(_) => held += 1,
                browse::Kind::Enter(inner) => held += self.count_under(cache, inner, depth + 1),
            }
        }
        held
    }

    /// Take out what has been hidden, unless hidden things are being shown.
    fn drop_hidden(&self, rows: &mut Vec<browse::Row>) {
        if self.show_hidden || self.settings.hidden_paths.is_empty() {
            return;
        }
        rows.retain(|row| !self.settings.hidden_paths.contains(&row_key(row)));
    }

    /// Whether the row under the cursor has been hidden, when there is one.
    /// Whether what the cursor is on has been hidden, when it is something
    /// that can be.
    ///
    /// A system is hidden by its id and everything else by what it points
    /// at, but from the menu they are one entry: whatever is under the
    /// cursor goes away and comes back the same way.
    fn selected_hidden(&self) -> Option<bool> {
        match self.browsing {
            Browsing::Games => {
                let row = self.here.get(self.game_list.selected())?;
                Some(self.settings.hidden_paths.contains(&row_key(row)))
            }
            Browsing::Systems => {
                let system = self.systems.get(self.system_list.selected())?;
                Some(self.settings.hidden.contains(&system.def.id))
            }
            Browsing::Categories => None,
        }
    }

    /// Hide the row under the cursor, or show it again.
    fn toggle_hidden(&mut self) {
        if self.browsing == Browsing::Systems {
            let Some(id) = self
                .systems
                .get(self.system_list.selected())
                .map(|system| system.def.id.clone())
            else {
                return;
            };
            match self.settings.hidden.iter().position(|held| *held == id) {
                Some(at) => {
                    self.settings.hidden.remove(at);
                }
                None => self.settings.hidden.push(id),
            }
            self.save_settings();
            self.rebuild_system_list();
            self.screen = Screen::Browse;
            self.apply_geometry();
            self.dirty = true;
            return;
        }
        let Some(row) = self.here.get(self.game_list.selected()) else {
            return;
        };
        let key = row_key(row);
        match self
            .settings
            .hidden_paths
            .iter()
            .position(|held| *held == key)
        {
            Some(at) => {
                self.settings.hidden_paths.remove(at);
            }
            None => self.settings.hidden_paths.push(key),
        }
        self.save_settings();
        self.correct_system_counts();
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.relist_here();
        self.dirty = true;
    }

    fn mark_favorites(&self, rows: &mut [browse::Row]) {
        for row in rows.iter_mut() {
            // By what the row launches, not by whether it is a file. An
            // AmigaVision title is not a file, and asking about it as one
            // meant every Amiga favourite was marked nowhere.
            if let Some(target) = row_target(row) {
                if self.favorites.holds(&target) {
                    row.favorite = true;
                }
            }
        }
        // Stable, so the alphabet inside each group survives. Folders lead
        // or trail as asked; favourites lead the things that are not
        // folders. With both left alone this is the order the card was
        // read in and nothing moves.
        let folders_last = self.folders_last;
        let favorites_first = self.favorites_first;
        rows.sort_by_key(|row| {
            let folder = if folders_last {
                u8::from(row.is_folder())
            } else {
                u8::from(!row.is_folder())
            };
            let favorite = if favorites_first {
                u8::from(!row.favorite)
            } else {
                0
            };
            (folder, favorite)
        });
    }

    fn show_here(&mut self) {
        let Some(crumb) = self.trail.last().cloned() else {
            return;
        };
        match self.listing(&crumb.place) {
            Ok(mut rows) => {
                self.recall_view();
                self.enrich_favorites(&mut rows);
                self.correct_counts(&mut rows);
                self.drop_hidden(&mut rows);
                self.mark_favorites(&mut rows);
                // A search belongs to the folder it was typed in.
                self.filter.clear();
                self.all_here.clear();
                self.here = rows;
                self.game_list = ListState::new(self.here.len(), self.geometry.visible);
                // The remembered row first, found again by what it is; the
                // crumb's index when there is none or it is gone. A fresh
                // crumb says zero, so a folder never visited starts at the
                // top, and select() clamps whatever comes out.
                let remembered = self.open_system.as_deref().and_then(|system| {
                    crate::state::recall_left_at(&self.left_at, system, &crumb.place.key())
                });
                let at = reselect(&self.here, remembered, crumb.selected);
                self.game_list.select(at);
                self.browsing = Browsing::Games;
                self.message = None;
                self.apply_geometry();
                self.touch_selection();
            }
            Err(e) => {
                // A folder that cannot be read says so. Showing it empty
                // would look like a folder with nothing in it, which is a
                // different and much less alarming thing.
                self.here.clear();
                self.game_list = ListState::new(0, self.geometry.visible);
                self.message = Some(format!("{e}"));
                self.dirty = true;
            }
        }
    }

    /// List the folder on screen again after something about it changed.
    ///
    /// The live cursor is written down first, because `show_here` restores
    /// the remembered row: re-listing without remembering landed on
    /// whatever row the folder was entered on, which is how favouriting or
    /// hiding a game deep in a long list snapped the cursor away from it.
    ///
    /// The crumb's index is refreshed too, because it is the fallback when
    /// the remembered row cannot be found again, and the action being
    /// re-listed for can be the one that removed that very row: hiding the
    /// game under the cursor must leave the cursor where it stood, on
    /// whatever slid into its place, not send it back to where the folder
    /// was entered.
    fn relist_here(&mut self) {
        self.remember_here();
        if let Some(crumb) = self.trail.last_mut() {
            crumb.selected = self.game_list.selected();
        }
        self.show_here();
    }

    /// The system that is open, found by its id.
    fn open_system_ref(&self) -> Option<&FoundSystem> {
        let id = self.open_system.as_deref()?;
        self.all_systems.iter().find(|system| system.def.id == id)
    }

    /// Where browsing currently is, for the title bar.
    fn here_label(&self) -> String {
        let system = self
            .open_system_ref()
            .map(|system| system.name().to_string())
            .unwrap_or_default();
        // The folders walked into since the system was opened. The system's
        // own folder is the first crumb and is already named by the system.
        let inside: Vec<&str> = self
            .trail
            .iter()
            .skip(1)
            .filter_map(|crumb| crumb.place.path().file_name())
            .filter_map(|name| name.to_str())
            .collect();
        if inside.is_empty() {
            system
        } else {
            format!("{system} / {}", inside.join(" / "))
        }
    }

    /// A game picked at random from the current folder.
    ///
    /// Picks among the games in this folder; if there are none it steps into
    /// a folder at random and asks again. Not a uniform draw across
    /// everything underneath, which would mean walking the whole subtree
    /// before answering: it picks a folder, then a game in it.
    fn random_here(&mut self, favorites_only: bool) -> Option<Outcome> {
        let mut place = self.trail.last()?.place.clone();
        let mut seed = self.seed;
        // The row itself, not where it sat. `show_here` hides rows and
        // gathers favourites to the top, so a position taken from the raw
        // listing points at a different game by the time it is used.
        let mut chosen: Option<(Place, String)> = None;

        for _ in 0..browse::MAX_DEPTH {
            // Through the same door browsing uses, so it reads what was
            // written down rather than opening the system again.
            let Ok(rows) = self.listing(&place) else {
                break;
            };
            let games: Vec<usize> = rows
                .iter()
                .enumerate()
                .filter(|(_, row)| !row.is_folder())
                .filter(|(_, row)| {
                    !favorites_only
                        || match &row.kind {
                            browse::Kind::Play(browse::Launch::File(path)) => {
                                self.favorites.holds(path)
                            }
                            browse::Kind::Play(browse::Launch::AmigaVision { install, title }) => {
                                self.favorites
                                    .holds(&crate::favorites::amiga_key(install, title))
                            }
                            _ => false,
                        }
                })
                .map(|(index, _)| index)
                .collect();
            if !games.is_empty() {
                let pick = games[(next_random(&mut seed) as usize) % games.len()];
                chosen = Some((place, row_key(&rows[pick])));
                break;
            }
            let folders: Vec<&browse::Row> = rows.iter().filter(|row| row.is_folder()).collect();
            if folders.is_empty() {
                break;
            }
            let pick = (next_random(&mut seed) as usize) % folders.len();
            let browse::Kind::Enter(next) = &folders[pick].kind else {
                break;
            };
            place = next.clone();
        }
        self.seed = seed;

        // Nothing kept anywhere under here: say so rather than leaving the
        // cursor where it was and looking like the button did nothing.
        if favorites_only && chosen.is_none() {
            self.message = Some("No favourites under this folder.".to_string());
            self.screen = Screen::Browse;
            self.apply_geometry();
            self.dirty = true;
            return None;
        }

        let mut outcome = None;
        match chosen {
            Some((place, key)) => {
                // Walk to it, so going back from the game lands in the
                // folder it actually came from.
                let depth = self.trail.len();
                if place != self.trail[depth - 1].place {
                    self.enter(place);
                }
                self.show_here();
                // Found again by what it is, in the list as it is actually
                // shown.
                if let Some(index) = self.here.iter().position(|row| row_key(row) == key) {
                    self.game_list.select(index);
                }
                self.screen = Screen::Browse;
                // Only if that is what the setting asks for. Landing on the
                // pick without starting it is how somebody looks at what
                // came up and rolls again without waiting for a core.
                if self.random_launches {
                    outcome = self.confirm_launch();
                }
            }
            None => {
                self.message = Some("No games under this folder.".to_string());
                self.screen = Screen::Browse;
                self.dirty = true;
            }
        }
        self.apply_geometry();
        self.touch_selection();
        outcome
    }

    /// Start the selected game. No question first: choosing a game in a list
    /// of games is not ambiguous, and a confirmation on every launch is a
    /// second press for every game anyone ever plays.
    ///
    /// Everything that can fail is decided here, while the interface is
    /// still up: once the outcome leaves the event loop the process is
    /// committed to leaving, and a missing core or a bad rule there would
    /// end Degauss instead of showing a line and staying.
    fn confirm_launch(&mut self) -> Option<Outcome> {
        let row = self.here.get(self.game_list.selected())?;
        let name = row.name.clone();
        let kind = row.kind.clone();
        let browse::Kind::Play(game) = kind else {
            self.message = Some("A folder is not a game.".to_string());
            self.dirty = true;
            return None;
        };
        let config = self.opened_config.clone()?;
        // A self-describing file names its own core, so a favourite or a
        // core file must not be blocked on the system's. Everything else
        // ends up in an MGL naming `config.rbf`, and handing MiSTer a core
        // it does not have replaces this process with nothing.
        let self_describing = match &game {
            browse::Launch::File(path) => !crate::launch::needs_system_core(path),
            browse::Launch::AmigaVision { .. } => false,
        };
        // Checked where MiSTer will look, not in the index the menu
        // grouping keeps. That index matches a core name anywhere at the
        // top of the card, so a support copy under _Arcade/cores or a
        // favourite's dangling link would answer for a core whose real
        // file is gone, and the launch would still end in MiSTer's own
        // "No rbf found!" with Degauss already gone.
        if !self_describing
            && !crate::systems::core_file_exists(Path::new(&self.config.menu_root), &config.rbf)
        {
            self.message = Some(format!(
                "{}: core {} is not on the card",
                config.name, config.rbf
            ));
            self.dirty = true;
            return None;
        }
        // A gamelist can name a file that was deleted or renamed since it
        // was written. The plan would build anyway and MiSTer would fail
        // after this process had already handed over, so the absence has to
        // become a line on screen here or never.
        let missing = match &game {
            // A favourite that is really an AmigaVision title points at an
            // installation, not at itself; the file that has to exist is
            // the one the rewritten MGL will mount.
            // An installation is a directory the launch writes into
            // (shared/ags_boot); a plain file under that name would pass an
            // existence check and fail after the hand-over.
            browse::Launch::File(path) => match crate::launch::amiga_marker(path) {
                Some((install, _)) => !install.is_dir(),
                None => !path.exists(),
            },
            browse::Launch::AmigaVision { install, .. } => !install.is_dir(),
        };
        if missing {
            self.message = Some(format!("{name}: its file is gone from the card"));
            self.dirty = true;
            return None;
        }
        // Building the plan only decides what would be written and sent;
        // nothing touches the card until `launch::execute`. So a plan that
        // cannot be built becomes a message here rather than an exit later.
        let mgl = Path::new("/tmp/degauss.mgl");
        let plan = match &game {
            browse::Launch::File(path) => crate::launch::plan(&config, path, mgl),
            browse::Launch::AmigaVision { install, title } => {
                crate::launch::plan_amiga_vision(&config, install, title, mgl)
            }
        };
        match plan {
            Ok(plan) => Some(Outcome::Launch {
                plan: Box::new(plan),
                name,
            }),
            Err(e) => {
                self.message = Some(format!("{e}"));
                self.dirty = true;
                None
            }
        }
    }

    /// Hide a system from the list. Nothing is deleted: it is remembered by
    /// id in settings.toml and comes back when hidden systems are shown.
    fn hide_system(&mut self, index: usize) {
        let Some(system) = self.systems.get(index) else {
            return;
        };
        let id = system.def.id.clone();
        if !self.settings.hidden.contains(&id) {
            self.settings.hidden.push(id);
        }
        self.save_settings();
        self.rebuild_system_list();
    }

    /// Apply hiding and the open group to produce the lists actually shown.
    /// Take what the cache says about every system.
    ///
    /// A system with nothing to play is hidden for the same reason an empty
    /// folder is: opening it shows a blank screen. Answered from the index
    /// rather than by walking the card.
    fn apply_index(&mut self) {
        let Some(index) = self.index.as_ref() else {
            return;
        };
        let empty: HashSet<String> = index
            .systems
            .iter()
            .filter(|(_, summary)| summary.games == 0)
            .map(|(id, _)| id.clone())
            .collect();
        let games: usize = index.systems.values().map(|s| s.games).sum();
        crate::note(&format!(
            "cache        {} systems, {games} games, {} hold nothing",
            index.systems.len(),
            empty.len()
        ));
        self.empty_systems = Some(empty);
        self.total_games = games;
        self.ui.set_about_line(SharedString::from(format!(
            "{} systems, {games} games",
            self.all_systems.len()
        )));
    }

    /// How many games one system holds, from the cache, less anything
    /// hidden underneath it.
    fn games_in(&self, id: &str) -> Option<usize> {
        if let Some(corrected) = self.corrected_counts.get(id) {
            return Some(*corrected);
        }
        self.index
            .as_ref()
            .and_then(|index| index.systems.get(id))
            .map(|summary| summary.games)
    }

    /// Work out the true count for any system something is hidden under.
    ///
    /// The counts were written down before anything was hidden, so a
    /// system whose folder is hidden would keep claiming its games. Only
    /// the systems actually affected are read again, which is normally
    /// none of them.
    fn correct_system_counts(&mut self) {
        self.corrected_counts.clear();
        if self.settings.hidden_paths.is_empty() {
            return;
        }
        let affected: HashSet<String> = self
            .settings
            .hidden_paths
            .iter()
            .filter_map(|key| key_path(key))
            .filter_map(|path| self.owner_of(&path))
            .collect();
        for id in affected {
            let Some(system) = self.all_systems.iter().find(|s| s.def.id == id) else {
                continue;
            };
            let start = browse::start_for(&system.to_config());
            let Some(cache) = crate::cache::load_system(&self.cache_dir, &id) else {
                continue;
            };
            let held = self.count_under(&cache, &start, 0);
            self.corrected_counts.insert(id, held);
        }
    }

    /// Begin reading the card into the cache.
    ///
    /// `forced` throws away what is already written down. Without it a
    /// system whose file is already there is left alone, which is what
    /// makes a second run cheap.
    fn start_build(&mut self, forced: bool) {
        if forced {
            crate::cache::clear(&self.cache_dir);
        }
        let total = self.all_systems.len();
        let mut left: Vec<usize> = (0..total).collect();
        // Popped from the end, so reverse to read them in the order they
        // are listed: the name on screen should march forwards.
        left.reverse();
        self.build = Some(Building {
            left,
            done: 0,
            total,
            index: crate::cache::Index::new(),
            forced,
        });
        // Not shown yet: it goes up when the reading starts, which is
        // after the wordmark has had its moment.
        self.dirty = true;
    }

    /// Read one system into the cache, and say so on screen.
    fn build_one_system(&mut self) {
        let Some(build) = self.build.as_mut() else {
            return;
        };
        let Some(index) = build.left.pop() else {
            // Done: write the index down and use it.
            let finished = self.build.take().expect("just checked");
            let index = finished.index;
            if let Err(e) = crate::cache::save_index(&self.cache_dir, &index) {
                crate::note(&format!("cache        index not written: {e}"));
            }
            self.index = Some(index);
            self.apply_index();
            self.rebuild_system_list();
            self.message = None;
            self.dirty = true;
            return;
        };
        build.done += 1;
        let done = build.done;
        let total = build.total;
        let forced = build.forced;

        let Some(system) = self.all_systems.get(index) else {
            return;
        };
        let id = system.def.id.clone();
        let name = system.name().to_string();
        let config = system.to_config();

        self.message = Some(format!("Caching the card\n\n{name}   {done} of {total}"));

        // Already written down and not being forced: nothing to read.
        let start = browse::start_for(&config);
        let summary = if !forced {
            crate::cache::load_system(&self.cache_dir, &id).map(|cache| cache.summary(&start))
        } else {
            None
        };
        let summary = summary.or_else(|| {
            let library = match Library::open_with_names(&config, self.names.clone()) {
                Ok(library) => library,
                Err(e) => {
                    crate::note(&format!("cache        {id} not read: {e}"));
                    return None;
                }
            };
            let cache = crate::cache::build_system(&library);
            let summary = cache.summary(&start);
            if let Err(e) = crate::cache::save_system(&self.cache_dir, &id, &cache) {
                crate::note(&format!("cache        {id} not written: {e}"));
            }
            Some(summary)
        });

        if let (Some(build), Some(summary)) = (self.build.as_mut(), summary) {
            build.index.systems.insert(id, summary);
        }
        self.dirty = true;
    }

    fn rebuild_system_list(&mut self) {
        // Nothing is hidden until the card has been read, which happens
        // once and only when something would be hidden by it.
        let empty = match (self.show_empty, self.empty_systems.as_ref()) {
            (false, Some(known)) => known.clone(),
            _ => HashSet::new(),
        };
        let hidden = &self.settings.hidden;
        let show_hidden = self.show_hidden;
        let visible: Vec<FoundSystem> = self
            .all_systems
            .iter()
            .filter(|s| show_hidden || !hidden.contains(&s.def.id))
            .filter(|s| !empty.contains(&s.def.id))
            .cloned()
            .collect();

        // Groups are whatever the visible systems belong to, in the order
        // MiSTer's own menu uses, and only when something is in them.
        // Favourites last: it is not a machine, it is a shelf of things
        // picked off the others.
        const ORDER: [&str; 6] = [
            "Arcade",
            "Console",
            "Computer",
            "Utility",
            "Other",
            "Favorites",
        ];
        let mut categories: Vec<(String, usize)> = Vec::new();
        for name in ORDER {
            // Other holds the cores that are not games, which is not what
            // anyone opened a game browser for. It is one switch away.
            if name == "Other" && !self.show_other {
                continue;
            }
            // Test patterns and measurement cores. Useful, and not what a
            // list of games is for.
            if name == "Utility" && !self.show_utility {
                continue;
            }
            let count = visible.iter().filter(|s| s.category() == name).count();
            if count > 0 {
                categories.push((name.to_string(), count));
            }
        }
        // Anything with a group we did not anticipate still gets shown.
        for system in &visible {
            let name = system.category();
            if !ORDER.contains(&name) && !categories.iter().any(|(c, _)| c == name) {
                let count = visible.iter().filter(|s| s.category() == name).count();
                categories.push((name.to_string(), count));
            }
        }

        self.systems = match self.open_category.as_deref() {
            Some(open) => visible
                .into_iter()
                .filter(|s| s.category() == open)
                .collect(),
            None => visible,
        };

        self.categories = categories;
        let selected_category = self
            .category_list
            .selected()
            .min(self.categories.len().saturating_sub(1));
        self.category_list = ListState::new(self.categories.len(), self.geometry.visible);
        self.category_list.select(selected_category);
        self.reroll_category_art();
        let count = self.systems.len();
        let selected = self.system_list.selected().min(count.saturating_sub(1));
        self.system_list = ListState::new(count, self.geometry.visible);
        self.system_list.move_items(selected as isize);
        self.dirty = true;
    }

    /// Choose which system lends its logo to each group, this time round.
    ///
    /// A different one on each visit. A group is not one machine, and always
    /// showing the same member's logo says it is.
    fn reroll_category_art(&mut self) {
        let mut seed = self.seed;
        let mut picks = std::collections::BTreeMap::new();
        for (name, _) in &self.categories {
            if let Some(explicit) = self.named_logo(name) {
                picks.insert(name.clone(), explicit);
                continue;
            }
            let logos: Vec<PathBuf> = self
                .all_systems
                .iter()
                .filter(|system| system.category() == name)
                .filter_map(|system| system.logo())
                .collect();
            if logos.is_empty() {
                continue;
            }
            let pick = (next_random(&mut seed) as usize) % logos.len();
            picks.insert(name.clone(), logos[pick].clone());
        }
        self.seed = seed;
        self.category_picks = picks;
    }

    /// A picture named after the group itself, if the user put one there.
    fn named_logo(&self, name: &str) -> Option<PathBuf> {
        let dir = self.all_systems.first()?.logo_dir.clone()?;
        ["png", "jpg"].iter().find_map(|extension| {
            let path = dir.join(format!("{name}.{extension}"));
            path.is_file().then_some(path)
        })
    }

    /// Open a group, showing the systems inside it.
    fn open_selected_category(&mut self) {
        let Some((name, _)) = self.categories.get(self.category_list.selected()) else {
            return;
        };
        self.open_category = Some(name.clone());
        self.rebuild_system_list();
        self.browsing = Browsing::Systems;

        // A group holding one system is a door with a corridor behind it.
        // Arcade is the case people meet: choosing Arcade showed a list
        // whose only entry was also called Arcade. Step through it, and
        // step back out of it in one press too.
        if self.systems.len() == 1 {
            self.skipped_systems = true;
            self.system_list.go_first();
            self.open_selected_system();
            return;
        }
        self.skipped_systems = false;
        self.apply_geometry();
        self.touch_selection();
    }

    fn adjust_option(&mut self, delta: isize) {
        let list = self.option_ids();
        let selected = self.active_list().selected();
        let Some(option) = list.get(selected).copied() else {
            return;
        };
        match option {
            OptionId::Speed => {
                self.speed = step(self.speed, delta, SPEED_STEPS.len());
                self.settings.speed_step = Some(self.speed);
            }
            OptionId::ArtLimit => {
                let next = step(self.art_limit(), delta, SPEED_STEPS.len());
                self.settings.art_limit = Some(next);
            }
            OptionId::Layout => {
                self.layout = self.layout.next();
                self.settings.layout = Some(self.layout.label().to_string());
                self.remember_view();
                self.apply_geometry();
            }
            OptionId::Font => {
                self.font = self.font.next();
                self.settings.font = Some(self.font.label().to_string());
                // Nothing about the layout moves, but every glyph on the
                // screen is now a different one.
                self.apply_geometry();
            }
            OptionId::ShowArt => {
                self.show_art = !self.show_art;
                self.settings.show_art = Some(self.show_art);
                self.touch_selection();
            }
            OptionId::ShowStats => {
                self.show_stats = !self.show_stats;
                self.settings.show_stats = Some(self.show_stats);
                // It takes a bar off the bottom of every screen, so the body
                // has to be measured again.
                self.apply_geometry();
            }
            OptionId::Present => {
                self.pending_present_switch = true;
            }
            OptionId::ShowHidden => {
                self.show_hidden = !self.show_hidden;
                self.settings.show_hidden = Some(self.show_hidden);
                self.rebuild_system_list();
            }
            OptionId::Screensaver => {
                let at = SAVER_CHOICES
                    .iter()
                    .position(|&c| c == self.screensaver_after())
                    .unwrap_or(2);
                let next = SAVER_CHOICES[step(at, delta, SAVER_CHOICES.len())];
                self.settings.screensaver_after = Some(next);
                self.last_input = Instant::now();
            }
            OptionId::ShiftX => {
                // A nudge wider than the margin moves nothing, so the number
                // stops where the picture stops.
                let limit = self.geometry.inset_x as i32;
                let next = (self.shift_x() + delta as i32).clamp(-limit, limit);
                self.settings.shift_x = Some(next);
                self.apply_geometry();
            }
            OptionId::ShiftY => {
                let limit = self.geometry.inset_y as i32;
                let next = (self.shift_y() + delta as i32).clamp(-limit, limit);
                self.settings.shift_y = Some(next);
                self.apply_geometry();
            }
            OptionId::ShowOther => {
                self.show_other = !self.show_other;
                self.settings.show_other = Some(self.show_other);
                self.rebuild_system_list();
            }
            OptionId::ShowUtility => {
                self.show_utility = !self.show_utility;
                self.settings.show_utility = Some(self.show_utility);
                self.rebuild_system_list();
            }
            OptionId::ShowBar => {
                self.show_bar = !self.show_bar;
                self.settings.show_bar = Some(self.show_bar);
                // Its height belongs to the list when it is not there.
                self.apply_geometry();
            }
            OptionId::ShowEmpty => {
                self.show_empty = !self.show_empty;
                self.settings.show_empty = Some(self.show_empty);
                // Turning it off asks a question the card has not been read
                // for yet.
                if !self.show_empty && self.empty_systems.is_none() {
                    self.start_build(false);
                }
                // The folder on screen and the list of systems were both
                // filtered by the old answer.
                self.rebuild_system_list();
                self.relist_here();
            }
            OptionId::FoldersLast => {
                self.folders_last = !self.folders_last;
                self.settings.folders_last = Some(self.folders_last);
                self.relist_here();
            }
            OptionId::FavoritesFirst => {
                self.favorites_first = !self.favorites_first;
                self.settings.favorites_first = Some(self.favorites_first);
                // The folder on screen was ordered by the old answer.
                self.relist_here();
            }
            OptionId::RandomLaunches => {
                self.random_launches = !self.random_launches;
                self.settings.random_launches = Some(self.random_launches);
            }
            OptionId::RebuildCache => {
                // Everything again, from the card: the point of asking for
                // it is that something changed that nothing noticed.
                self.start_build(true);
            }
            OptionId::ResetHidden => {
                let held = self.settings.hidden.len() + self.settings.hidden_paths.len();
                self.settings.hidden.clear();
                self.settings.hidden_paths.clear();
                self.save_settings();
                self.corrected_counts.clear();
                self.rebuild_system_list();
                self.relist_here();
                self.message = Some(format!("{held} shown again"));
                self.dirty = true;
            }
            OptionId::Advanced => {
                self.screen = Screen::Advanced;
                self.apply_geometry();
            }
            OptionId::OverscanX => {
                let current = self
                    .settings
                    .overscan_x
                    .unwrap_or(self.config.app.overscan_x);
                let next = (current as isize + delta).clamp(0, 15) as u32;
                self.settings.overscan_x = Some(next);
                self.config.app.overscan_x = next;
                self.apply_geometry();
            }
            OptionId::OverscanY => {
                let current = self
                    .settings
                    .overscan_y
                    .unwrap_or(self.config.app.overscan_y);
                let next = (current as isize + delta).clamp(0, 15) as u32;
                self.settings.overscan_y = Some(next);
                self.config.app.overscan_y = next;
                self.apply_geometry();
            }
        }
        self.dirty = true;
    }

    fn option_value(&self, option: OptionId) -> String {
        match option {
            OptionId::Speed => speed_label(self.speed),
            OptionId::ArtLimit => {
                let limit = self.art_limit();
                if limit + 1 >= SPEED_STEPS.len() {
                    "never skip".to_string()
                } else {
                    speed_badge(limit)
                }
            }
            OptionId::Layout => capitalised(self.layout.label()),
            OptionId::Font => capitalised(self.font.label()),
            OptionId::ShowArt => on_off(self.show_art),
            OptionId::ShowStats => on_off(self.show_stats),
            OptionId::Present => capitalised(self.present_label),
            OptionId::ShowHidden => on_off(self.show_hidden),
            OptionId::ShowEmpty => on_off(self.show_empty),
            OptionId::ShowOther => on_off(self.show_other),
            OptionId::ShowUtility => on_off(self.show_utility),
            OptionId::ShowBar => on_off(self.show_bar),
            OptionId::FavoritesFirst => on_off(self.favorites_first),
            OptionId::RandomLaunches => if self.random_launches {
                "Launches"
            } else {
                "Selects"
            }
            .to_string(),
            OptionId::FoldersLast => if self.folders_last {
                "After games"
            } else {
                "First"
            }
            .to_string(),
            OptionId::ResetHidden => {
                match self.settings.hidden.len() + self.settings.hidden_paths.len() {
                    0 => "Nothing hidden".to_string(),
                    held => format!("{held} hidden"),
                }
            }
            OptionId::RebuildCache => match self.total_games {
                0 => "Not read yet".to_string(),
                games => format!("{games} games"),
            },
            OptionId::Screensaver => match self.screensaver_after() {
                0 => "off".to_string(),
                60 => "1 minute".to_string(),
                seconds => format!("{} minutes", seconds / 60),
            },
            OptionId::ShiftX => format!("{:+} px", self.shift_x()),
            OptionId::ShiftY => format!("{:+} px", self.shift_y()),
            OptionId::Advanced => ">".to_string(),
            OptionId::OverscanX => format!(
                "{}%",
                self.settings
                    .overscan_x
                    .unwrap_or(self.config.app.overscan_x)
            ),
            OptionId::OverscanY => format!(
                "{}%",
                self.settings
                    .overscan_y
                    .unwrap_or(self.config.app.overscan_y)
            ),
        }
    }

    fn save_settings(&self) {
        if let Err(e) = self.settings.save(&self.settings_path) {
            // Not fatal: the interface still works, the change just will not
            // survive a restart, and the user should be told which it is.
            eprintln!("degauss: could not save settings: {e}");
        }
    }

    /// B everywhere. On the systems list there is nowhere further
    /// back, so it opens the menu instead: that is what makes a two-button
    /// controller enough to reach everything, including exit.
    fn go_back(&mut self) -> Option<Outcome> {
        match self.screen {
            Screen::Screensaver => self.leave_screensaver(),
            // Spelling out a folder name has no other way to say it is
            // finished: every button on the grid is a letter.
            Screen::Find if self.find_mode == FindMode::NewFolder => {
                let name = self.filter.clone();
                self.filter.clear();
                match crate::favorites::make_folder(&self.favorites_root(), &name) {
                    Ok(_) => self.add_favorite_in(&name),
                    Err(e) => {
                        self.message = Some(format!("{e}"));
                        self.screen = Screen::Browse;
                        self.apply_geometry();
                        self.dirty = true;
                    }
                }
            }
            Screen::Find | Screen::FavoriteFolder => {
                self.screen = Screen::Browse;
                self.apply_geometry();
                self.touch_selection();
            }
            Screen::Advanced => {
                self.save_settings();
                self.screen = Screen::Options;
                self.apply_geometry();
            }
            Screen::Options => {
                self.save_settings();
                self.screen = Screen::Menu;
                self.apply_geometry();
            }
            Screen::Help | Screen::About => {
                self.screen = Screen::Menu;
                self.apply_geometry();
            }
            Screen::Splash => self.leave_splash(),
            Screen::Menu | Screen::Context => {
                self.screen = Screen::Browse;
                self.apply_geometry();
            }
            Screen::Browse => match self.browsing {
                Browsing::Games => {
                    // Out of the folder first; only when there is no folder
                    // left does B leave the system.
                    if self.leave() {
                        return None;
                    }
                    self.open_system = None;
                    self.here.clear();
                    self.library = None;
                    self.system_cache = None;
                    self.opened_config = None;
                    self.trail.clear();
                    if self.skipped_systems {
                        // We stepped through this group on the way in.
                        self.skipped_systems = false;
                        self.browsing = Browsing::Categories;
                        self.open_category = None;
                        self.rebuild_system_list();
                        self.apply_geometry();
                        self.touch_selection();
                        self.dirty = true;
                        return None;
                    }
                    self.browsing = Browsing::Systems;
                    self.apply_geometry();
                    self.touch_selection();
                }
                Browsing::Systems => {
                    self.browsing = Browsing::Categories;
                    self.open_category = None;
                    self.rebuild_system_list();
                    self.apply_geometry();
                    self.touch_selection();
                }
                // The top of the tree: nowhere further back, so this is
                // where B reaches the menu instead.
                Browsing::Categories => self.open_menu(),
            },
        }
        self.dirty = true;
        None
    }

    fn open_menu(&mut self) {
        let system = if self.browsing == Browsing::Systems {
            self.systems
                .get(self.system_list.selected())
                .map(|s| s.name().to_string())
        } else {
            None
        };
        self.menu = menu_entries(self.browsing, system.as_deref());
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::Menu;
        self.apply_geometry();
    }

    /// What a contextual entry currently reads as, when it is a choice
    /// rather than an action.
    fn context_value(&self, index: usize) -> String {
        match self.menu.get(index).map(String::as_str) {
            Some(CHANGE_VIEW) => capitalised(self.layout.label()),
            _ => String::new(),
        }
    }

    /// Change the selected contextual entry the way left and right change a
    /// setting. A menu offering a choice should let the stick walk it
    /// rather than hiding it behind a press.
    fn adjust_context(&mut self, delta: isize) {
        let selected = self.menu_list.selected();
        if self.menu.get(selected).map(String::as_str) != Some(CHANGE_VIEW) {
            return;
        }
        self.layout = if delta >= 0 {
            self.layout.next()
        } else {
            self.layout.prev()
        };
        self.settings.layout = Some(self.layout.label().to_string());
        self.remember_view();
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }

    /// Open the grid of letters.
    fn open_find(&mut self, mode: FindMode) {
        self.find_mode = mode;
        let cells = FIND_CELLS.chars().count();
        self.find_list = ListState::new(cells, cells);
        self.find_list.reshape(cells, FIND_COLUMNS);
        self.screen = Screen::Find;
        self.apply_geometry();
    }

    /// Act on the cell under the cursor.
    fn pick_letter(&mut self) {
        let Some(letter) = FIND_CELLS.chars().nth(self.find_list.selected()) else {
            return;
        };
        match self.find_mode {
            FindMode::Jump => {
                // Back to browsing first: the selection to move is the
                // folder's, and while the grid is up the grid's is the one
                // that would move.
                self.screen = Screen::Browse;
                self.apply_geometry();
                self.jump_to(letter);
                self.touch_selection();
            }
            FindMode::Search => {
                self.filter.push(letter);
                self.apply_filter();
            }
            FindMode::NewFolder => {
                self.filter.push(letter);
                self.dirty = true;
            }
            #[allow(unreachable_patterns)]
            _ => {}
        }
    }

    /// Move the selection to the letter picked.
    fn jump_to(&mut self, letter: char) {
        let key = letter.to_ascii_lowercase();
        let entries: Vec<(char, bool)> = match self.browsing {
            Browsing::Games => self
                .here
                .iter()
                .map(|row| (first_letter(&row.sort_key), row.is_folder()))
                .collect(),
            Browsing::Systems => self
                .systems
                .iter()
                .map(|system| (first_letter(&system.name().to_lowercase()), false))
                .collect(),
            Browsing::Categories => return,
        };
        match jump_target(&entries, key) {
            Some(at) => self.active_list_mut().select(at),
            // Past the end of the alphabet as this list has it: the last
            // entry is the nearest thing to what was asked for.
            None => {
                self.active_list_mut().go_last();
            }
        }
    }

    /// Narrow the folder on screen to the titles that match.
    ///
    /// Spaces are dropped from both sides, because the grid has no space
    /// key and typing SUPERM should still find Super Mario.
    fn apply_filter(&mut self) {
        if self.all_here.is_empty() && !self.filter.is_empty() {
            self.all_here = std::mem::take(&mut self.here);
        }
        if self.filter.is_empty() {
            self.here = std::mem::take(&mut self.all_here);
        } else {
            let wanted = self.filter.as_str();
            self.here = self
                .all_here
                .iter()
                .filter(|row| squashed(&row.name).contains(wanted))
                .cloned()
                .collect();
        }
        self.game_list = ListState::new(self.here.len(), self.geometry.visible);
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }

    /// Put back everything a search took away.
    fn clear_filter(&mut self) {
        if self.filter.is_empty() {
            return;
        }
        self.filter.clear();
        self.apply_filter();
    }

    /// The game under the cursor, if what is under the cursor is a game.
    fn selected_game(&self) -> Option<PathBuf> {
        // What the cursor is on, not what is drawn: this is asked while a
        // menu is being built over the folder, so the screen has already
        // moved on from browsing.
        if self.browsing != Browsing::Games {
            return None;
        }
        match &self.here.get(self.game_list.selected())?.kind {
            browse::Kind::Play(browse::Launch::File(path)) => Some(path.clone()),
            // Not a file: AmigaVision keeps its library inside one image
            // and picks a title by name. Answered under a name of its own
            // so one map covers both kinds.
            browse::Kind::Play(browse::Launch::AmigaVision { install, title }) => {
                Some(crate::favorites::amiga_key(install, title))
            }
            _ => None,
        }
    }

    /// Offer the folders MiSTer's favourites are already kept in, and the
    /// chance to name another.
    fn open_favorite_folders(&mut self) {
        let mut entries = crate::favorites::folders(&self.favorites_root());
        entries.push(NEW_FOLDER.to_string());
        self.menu = entries;
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::FavoriteFolder;
        self.apply_geometry();
    }

    /// Keep the selected game in a folder of MiSTer's favourites.
    ///
    /// Written the way its own script writes one: an `.mgl` naming the core
    /// and the file for a game, a link for a core file. Nothing here is
    /// Degauss's own format, so a favourite made here is a favourite in the
    /// stock menu too.
    fn add_favorite_in(&mut self, folder: &str) {
        let Some(game) = self.selected_game() else {
            return;
        };
        let Some(config) = self.opened_config.clone() else {
            return;
        };
        let name = self
            .here
            .get(self.game_list.selected())
            .map(|row| row.name.clone())
            .unwrap_or_default();
        let target = self.favorites_root().join(folder);

        // A title rather than a file: written as an MGL that starts
        // AmigaVision, carrying the title in an element Main ignores.
        let amiga = match &self
            .here
            .get(self.game_list.selected())
            .map(|row| row.kind.clone())
        {
            Some(browse::Kind::Play(browse::Launch::AmigaVision { install, title })) => {
                Some((install.clone(), title.clone()))
            }
            _ => None,
        };
        if let Some((install, title)) = amiga {
            // The title is already the shown name, so sanitising it keeps
            // the favourite recognisable while making the name one the
            // card can hold.
            let outcome =
                crate::launch::favorite_mgl_amiga(&config, &install, &title).and_then(|mgl| {
                    crate::favorites::add_game(&target, &sanitise(&title), &mgl).map(|_| title)
                });
            let mut refresh_error = None;
            match outcome {
                Ok(what) => {
                    self.message = Some(format!("{what}\n\nkept in {folder}"));
                    self.reread_favorites();
                    refresh_error = self.refresh_favorites_system();
                }
                Err(e) => self.message = Some(format!("{e}")),
            }
            self.screen = Screen::Browse;
            self.apply_geometry();
            self.relist_here();
            if let Some(error) = refresh_error {
                // Set after show_here, which clears the message field as
                // part of its redraw; set before, the error would never be
                // seen.
                self.message = Some(error);
            }
            self.dirty = true;
            return;
        }

        let outcome = match crate::launch::favorite_mgl(&config, &game) {
            Ok(Some(mgl)) => {
                // The favourite is filed under the name the browser showed,
                // which the gamelist may have set, not under the file's own
                // stem: a favourite called "mslug" would be a stranger in a
                // list that has always said "Metal Slug".
                let fav_name = crate::favorites::favorite_name(&name, &game);
                crate::favorites::add_game(&target, &fav_name, &mgl).map(|_| fav_name)
            }
            // A core file is linked to, not described. The link keeps the
            // real filename, not the shown name: the stock script resolves
            // a link by its filename, and the browser derives the shown
            // name from the stem at browse time anyway.
            Ok(None) => {
                let file = game
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| name.clone());
                self.link_favorite(&target, &file, &game)
            }
            Err(e) => Err(e),
        };

        let mut refresh_error = None;
        match outcome {
            Ok(what) => {
                self.message = Some(format!("{what}\n\nkept in {folder}"));
                self.reread_favorites();
                refresh_error = self.refresh_favorites_system();
            }
            Err(e) => self.message = Some(format!("{e}")),
        }
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.relist_here();
        if let Some(error) = refresh_error {
            // Set after show_here, which clears the message field as
            // part of its redraw; set before, the error would never be
            // seen.
            self.message = Some(error);
        }
        self.dirty = true;
    }

    fn link_favorite(&self, folder: &Path, name: &str, game: &Path) -> Result<String> {
        crate::favorites::add_core(folder, name, game).map(|_| name.to_string())
    }

    /// Take a favourite away, by removing the file that makes it one.
    fn remove_favorite(&mut self) {
        let Some(game) = self.selected_game() else {
            return;
        };
        let Some(file) = self.favorites.file_for(&game).map(Path::to_path_buf) else {
            return;
        };
        let mut refresh_error = None;
        match crate::favorites::remove(&file) {
            Ok(()) => {
                self.reread_favorites();
                refresh_error = self.refresh_favorites_system();
            }
            Err(e) => self.message = Some(format!("{e}")),
        }
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.relist_here();
        if let Some(error) = refresh_error {
            // Set after show_here, which clears the message field as
            // part of its redraw; set before, the error would never be
            // seen.
            self.message = Some(error);
        }
        self.dirty = true;
    }

    /// What can be done with the folder on screen.
    fn open_context(&mut self) {
        self.menu = context_entries(
            self.browsing,
            !self.filter.is_empty(),
            self.selected_game().map(|path| self.favorites.holds(&path)),
            self.selected_hidden(),
        );
        if self.menu.is_empty() {
            return;
        }
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::Context;
        self.apply_geometry();
    }

    pub fn handle(&mut self, action: Action) -> Option<Outcome> {
        // Anything at all counts as somebody being here.
        self.last_input = Instant::now();
        if self.screen == Screen::Screensaver {
            // The press that wakes it does nothing else. Waking a screen by
            // launching whatever happened to be under the cursor would be a
            // nasty surprise.
            self.leave_screensaver();
            return None;
        }
        if self.screen == Screen::Splash {
            // Nobody should have to wait for a logo.
            let _ = action;
            self.leave_splash();
            return None;
        }

        if let Some(pending) = self.pending.clone() {
            match action {
                Action::Accept => {
                    self.pending = None;
                    self.message = None;
                    self.dirty = true;
                    match pending {
                        Pending::Exit => return Some(Outcome::Quit),
                        Pending::Hide(index) => {
                            self.hide_system(index);
                            self.screen = Screen::Browse;
                            self.apply_geometry();
                        }
                    }
                    return None;
                }
                // Anything else cancels: a question left on screen after an
                // unrelated press would be worse than asking again.
                _ => {
                    self.pending = None;
                    self.message = None;
                    self.dirty = true;
                    return None;
                }
            }
        }

        // A plain message follows the same rule as a question: the next
        // press dismisses it and is spent on the dismissing, so nothing can
        // be typed through it and pressing A twice on an unlaunchable game
        // re-raises the message instead of looking stuck. Build progress is
        // the exception: it repaints its own text every frame and the
        // controls stay live under it.
        if self.message.is_some() && self.build.is_none() {
            self.message = None;
            self.dirty = true;
            return None;
        }

        match action {
            Action::Up => {
                if self.screen == Screen::Find {
                    self.find_list.move_rows(-1);
                    self.dirty = true;
                } else {
                    let step = self.browse_step();
                    if self.active_list_mut().move_items(-step) {
                        self.skip_blank_menu(-1);
                        self.touch_selection();
                    }
                }
            }
            Action::Down => {
                if self.screen == Screen::Find {
                    self.find_list.move_rows(1);
                    self.dirty = true;
                } else {
                    let step = self.browse_step();
                    if self.active_list_mut().move_items(step) {
                        self.skip_blank_menu(1);
                        self.touch_selection();
                    }
                }
            }
            Action::Slower | Action::Faster => {
                let delta = if matches!(action, Action::Faster) {
                    1
                } else {
                    -1
                };
                if self.screen == Screen::Find {
                    self.find_list.move_items(delta);
                    self.dirty = true;
                } else if matches!(self.screen, Screen::Options | Screen::Advanced) {
                    self.adjust_option(delta);
                } else if self.screen == Screen::Context {
                    self.adjust_context(delta);
                } else {
                    self.speed = step(self.speed, delta, SPEED_STEPS.len());
                    self.settings.speed_step = Some(self.speed);
                    self.speed_shown_at = Some(Instant::now());
                    self.dirty = true;
                }
            }
            Action::Home => {
                if self.active_list_mut().go_first() {
                    self.touch_selection();
                }
            }
            Action::End => {
                if self.active_list_mut().go_last() {
                    self.touch_selection();
                }
            }
            Action::CycleLayout => {
                if self.screen == Screen::Browse {
                    self.layout = self.layout.next();
                    self.settings.layout = Some(self.layout.label().to_string());
                    self.remember_view();
                    self.apply_geometry();
                    self.touch_selection();
                }
            }
            Action::Menu => {
                if self.screen == Screen::Browse {
                    self.open_menu();
                } else if self.screen == Screen::Find && self.find_mode != FindMode::Jump {
                    // Y wipes what has been typed rather than leaving: on a
                    // grid the two spare buttons are the only edit keys
                    // there are.
                    self.filter.clear();
                    if self.find_mode == FindMode::Search {
                        self.apply_filter();
                    }
                    self.dirty = true;
                } else {
                    return self.go_back();
                }
            }
            Action::Context => {
                if self.screen == Screen::Browse {
                    self.open_context();
                } else if self.screen == Screen::Find && self.find_mode != FindMode::Jump {
                    self.filter.pop();
                    if self.find_mode == FindMode::Search {
                        self.apply_filter();
                    }
                    self.dirty = true;
                } else {
                    return self.go_back();
                }
            }
            Action::CyclePresent => self.pending_present_switch = true,
            Action::Accept => match self.screen {
                Screen::Screensaver => self.leave_screensaver(),
                Screen::Browse => match self.browsing {
                    Browsing::Categories => self.open_selected_category(),
                    Browsing::Systems => self.open_selected_system(),
                    Browsing::Games => {
                        if let Some(row) = self.here.get(self.game_list.selected()) {
                            match &row.kind {
                                browse::Kind::Enter(place) => {
                                    let place = place.clone();
                                    self.enter(place);
                                }
                                browse::Kind::Play(_) => return self.confirm_launch(),
                            }
                        }
                    }
                },
                Screen::Find => self.pick_letter(),
                Screen::FavoriteFolder => {
                    let choice = self
                        .menu
                        .get(self.menu_list.selected())
                        .cloned()
                        .unwrap_or_default();
                    if choice == NEW_FOLDER {
                        self.filter.clear();
                        self.open_find(FindMode::NewFolder);
                    } else {
                        self.add_favorite_in(&choice);
                    }
                }
                Screen::Menu | Screen::Context => {
                    let choice = self
                        .menu
                        .get(self.menu_list.selected())
                        .cloned()
                        .unwrap_or_default();
                    if choice == CHANGE_VIEW {
                        // Stays open, like a setting: the point is to see
                        // the view while choosing it.
                        self.adjust_context(1);
                    } else if choice == RANDOM {
                        return self.random_here(false);
                    } else if choice == RANDOM_FAVORITE {
                        return self.random_here(true);
                    } else if choice == HIDE_THIS || choice == SHOW_THIS {
                        self.toggle_hidden();
                    } else if choice == JUMP {
                        self.open_find(FindMode::Jump);
                    } else if choice == SEARCH {
                        self.open_find(FindMode::Search);
                    } else if choice == ADD_FAVORITE {
                        self.open_favorite_folders();
                    } else if choice == REMOVE_FAVORITE {
                        self.remove_favorite();
                    } else if choice == CLEAR_SEARCH {
                        self.clear_filter();
                        self.screen = Screen::Browse;
                        self.apply_geometry();
                    } else if choice.starts_with("Hide ") {
                        let index = self.system_list.selected();
                        let name = self
                            .systems
                            .get(index)
                            .map(|s| s.name().to_string())
                            .unwrap_or_default();
                        self.pending = Some(Pending::Hide(index));
                        self.message = Some(format!("Hide {name}?\n\nA yes, B no"));
                    } else if choice == "Options" {
                        self.screen = Screen::Options;
                        self.apply_geometry();
                    } else if choice == "Help" {
                        self.screen = Screen::Help;
                        self.apply_geometry();
                    } else if choice == "About" {
                        self.screen = Screen::About;
                        self.apply_geometry();
                    } else {
                        // Leaving is irreversible from the user's point of
                        // view, so it asks.
                        self.pending = Some(Pending::Exit);
                        self.message = Some("Leave Degauss?\n\nA yes, B no".to_string());
                    }
                    self.dirty = true;
                }
                Screen::Options | Screen::Advanced => self.adjust_option(1),
                Screen::Help | Screen::About => return self.go_back(),
                Screen::Splash => self.leave_splash(),
            },
            Action::Quit => return self.go_back(),
        }
        None
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn art_has_settled(&self, now: Instant) -> bool {
        // At or below the limit a picture is loaded for every row as it goes
        // past, which is what makes scrolling feel like looking through a
        // shelf rather than reading a list. Above it, decoding is what stops
        // the scrolling being smooth, so the pictures wait for it to stop.
        let debounce = if self.speed <= self.art_limit() {
            Duration::ZERO
        } else {
            Duration::from_millis(ART_AFTER_SCROLL_MS)
        };
        self.settled_since
            .is_some_and(|at| now.duration_since(at) >= debounce)
    }

    /// The picture for the current selection: a screenshot when browsing
    /// games, the system's own artwork when browsing systems, and nothing at
    /// all when neither exists, in which case the name is drawn instead.
    /// Whether the favourites folder is what is being looked at, so the
    /// mark can stand in for a picture nothing here will ever have.
    fn in_favorites(&self) -> bool {
        self.open_system.as_deref() == Some(FAVORITES_ID)
    }

    /// The picture, the words under it, and whether the heart should stand
    /// in for both.
    fn current_art(&self) -> (Option<PathBuf>, String, bool) {
        match (self.screen, self.browsing) {
            (Screen::Browse, Browsing::Games) => {
                match self.here.get(self.game_list.selected()) {
                    Some(row) => {
                        // Inside favourites a folder is a shelf the user
                        // made. It has no artwork and never will, and its
                        // name is already the row: the mark says more.
                        let heart =
                            self.in_favorites() && matches!(row.kind, browse::Kind::Enter(_));
                        (
                            row.cover.clone().or_else(|| {
                                if heart {
                                    return None;
                                }
                                // A folder has no picture of its own; the
                                // system's logo is better than a blank plate.
                                self.open_system_ref().and_then(|system| system.logo())
                            }),
                            row.name.clone(),
                            heart,
                        )
                    }
                    None => (None, String::new(), false),
                }
            }
            (Screen::Browse, Browsing::Systems) => {
                match self.systems.get(self.system_list.selected()) {
                    Some(system) => {
                        let heart = system.def.id == FAVORITES_ID;
                        let logo = if heart { None } else { system.logo() };
                        (logo, system.name().to_string(), heart)
                    }
                    None => (None, String::new(), false),
                }
            }
            (Screen::Browse, Browsing::Categories) => {
                match self.categories.get(self.category_list.selected()) {
                    Some((name, _)) => {
                        let heart = name == FAVORITES_ID;
                        let logo = if heart {
                            None
                        } else {
                            self.category_logo(name)
                        };
                        (logo, name.clone(), heart)
                    }
                    None => (None, String::new(), false),
                }
            }
            _ => (None, String::new(), false),
        }
    }

    /// A picture for a path, decoding it here or asking for it elsewhere.
    ///
    /// The one place that decides, so every list goes the same way.
    fn cover_for(&mut self, path: &std::path::Path) -> Option<slint::Image> {
        self.covers.get(path).map(to_image)
    }

    fn load_art(&mut self) {
        // The screensaver is nothing but a picture, so it wants one whatever
        // the browse layout happens to be.
        let wanted = self.show_art
            && match self.screen {
                Screen::Screensaver => true,
                Screen::Browse => self.layout == Layout::Details,
                _ => false,
            };
        if !wanted {
            self.art_pending = false;
            return;
        }

        let started = Instant::now();
        let (path, caption, heart) = self.current_art();
        self.ui.set_art_caption(SharedString::from(caption));
        self.ui.set_art_heart(heart);
        match path.and_then(|path| self.cover_for(&path)) {
            Some(image) => {
                self.ui.set_art(image);
                self.ui.set_has_art(true);
            }
            None => self.ui.set_has_art(false),
        }
        self.art.loads += 1;
        self.art.worst_load_us = self
            .art
            .worst_load_us
            .max(started.elapsed().as_micros() as u64);
        self.art_pending = false;
    }

    fn refresh(&mut self) {
        let (range, selected_in_window) = self.active_list().window();
        let mut rows = Vec::with_capacity(range.len());

        match self.screen {
            // A strip of pictures, drifting sideways. Enough of them to
            // cover the width plus one either side, taken from the ring so
            Screen::Find => {
                for cell in FIND_CELLS.chars() {
                    rows.push(Row {
                        title: SharedString::from(cell.to_string()),
                        favorite: false,
                        cover: slint::Image::default(),
                        has_cover: false,
                        value: SharedString::new(),
                    });
                }
            }
            // it never reaches an end.
            Screen::Screensaver => {
                let cell = self.saver_cell();
                let count = self.saver_pool.len();
                if count > 0 {
                    let travelled = (self.saver_offset / cell).floor();
                    let first = travelled as usize % count;
                    let sub = self.saver_offset - travelled * cell;
                    let needed = (self.width as f32 / cell).ceil() as usize + 2;
                    for step in 0..needed {
                        let picture = self.saver_pool[(first + step) % count].clone();
                        let (cover, has_cover) = match self.cover_for(&picture.path) {
                            Some(image) => (image, true),
                            None => (slint::Image::default(), false),
                        };
                        rows.push(Row {
                            title: SharedString::from(picture.caption.as_str()),
                            favorite: false,
                            cover,
                            has_cover,
                            value: SharedString::new(),
                        });
                    }
                    self.ui.set_saver_cell(cell);
                    self.ui.set_saver_offset(sub);
                }
            }
            Screen::Browse => {
                let with_art = self.show_art && self.layout.rows_have_art() && !self.art_pending;
                match self.browsing {
                    Browsing::Categories => {
                        for index in range {
                            let name = self.categories[index].0.clone();
                            let name = &name;
                            // No number beside a group. It counted systems,
                            // so Arcade read "1" while holding a thousand
                            // games, which answers a question nobody asked
                            // with a number that means something else.
                            let logo = self.category_logo(name);
                            let (cover, has_cover) = match logo.and_then(|p| self.cover_for(&p)) {
                                Some(image) => (image, true),
                                None => (slint::Image::default(), false),
                            };
                            rows.push(Row {
                                title: SharedString::from(name.as_str()),
                                favorite: false,
                                cover,
                                has_cover,
                                value: SharedString::new(),
                            });
                        }
                    }
                    Browsing::Systems => {
                        for index in range {
                            let logo = if with_art {
                                self.systems[index].logo()
                            } else {
                                None
                            };
                            let (cover, has_cover) = match logo.and_then(|p| self.cover_for(&p)) {
                                Some(image) => (image, true),
                                None => (slint::Image::default(), false),
                            };
                            let name = self.systems[index].name().to_string();
                            // How many games are in there, where the card
                            // has been read for it.
                            let held = self.games_in(&self.systems[index].def.id);
                            rows.push(Row {
                                title: SharedString::from(name.as_str()),
                                favorite: false,
                                cover,
                                has_cover,
                                value: match held {
                                    Some(games) => SharedString::from(games.to_string()),
                                    None => SharedString::new(),
                                },
                            });
                        }
                    }
                    Browsing::Games => {
                        // The same stand-in the picture panel uses: a game
                        // with no artwork of its own shows the logo of the
                        // system it belongs to, rather than a blank tile.
                        // Read once, because it is the same for every row.
                        let logo = if with_art {
                            self.open_system_ref().and_then(|system| system.logo())
                        } else {
                            None
                        };
                        for index in range {
                            let wanted = if with_art {
                                // Inside favourites a folder is a shelf the
                                // user made, and the panel marks it with a
                                // heart rather than a logo: no stand-in here
                                // either, so the two views agree.
                                let bare = self.in_favorites() && self.here[index].is_folder();
                                self.here[index].cover.clone().or_else(|| {
                                    if bare {
                                        None
                                    } else {
                                        logo.clone()
                                    }
                                })
                            } else {
                                None
                            };
                            let (cover, has_cover) = if with_art {
                                match wanted.and_then(|p| self.cover_for(&p)) {
                                    Some(image) => (image, true),
                                    None => (slint::Image::default(), false),
                                }
                            } else {
                                (slint::Image::default(), false)
                            };
                            let row = &self.here[index];
                            rows.push(Row {
                                // A folder is marked as one. Nothing else in
                                // the list says which rows can be entered.
                                title: SharedString::from(if row.is_folder() {
                                    format!("[ {} ]", row.name)
                                } else {
                                    row.name.clone()
                                }),
                                favorite: row.favorite,
                                cover,
                                has_cover,
                                // How much is in there, where somebody has
                                // counted. Folders only: a game is one game.
                                value: match row.below {
                                    Some(games) if row.is_folder() => {
                                        SharedString::from(games.to_string())
                                    }
                                    _ => SharedString::new(),
                                },
                            });
                        }
                    }
                }
            }
            Screen::Menu | Screen::FavoriteFolder => {
                for index in range {
                    rows.push(plain_row(&self.menu[index], ""));
                }
            }
            Screen::Context => {
                for index in range {
                    let value = self.context_value(index);
                    rows.push(plain_row(&self.menu[index], &value));
                }
            }
            Screen::Options | Screen::Advanced => {
                let list = self.option_ids();
                for index in range {
                    let option = list[index];
                    rows.push(plain_row(option.label(), &self.option_value(option)));
                }
            }
            Screen::About | Screen::Splash => {
                // The wordmark is drawn by the interface layer; there are no
                // rows to build.
            }
            Screen::Help => {
                for index in range {
                    rows.push(plain_row(HELP[index], ""));
                }
            }
        }

        self.rows.set_vec(rows);
        // Help fits on one screen and nothing on it can be chosen, so a
        // highlight there would be a cursor pointing at nothing. No index
        // matches -1.
        self.ui.set_selected(if self.screen == Screen::Help {
            -1
        } else {
            selected_in_window as i32
        });
        self.update_chrome();
        self.window.request_redraw();
        self.dirty = false;
    }

    /// How long the speed sits in the bar after it changes.
    const SPEED_BADGE_MS: u64 = 500;

    /// The lines under the picture: what the gamelist knows, labelled, in
    /// one fixed order.
    ///
    /// Six of them whatever the card holds. A panel whose lines move about
    /// as the selection changes has to be read again every time; one whose
    /// lines stay put can be glanced at.
    fn detail_lines(&self) -> Vec<DetailLine> {
        // Only where there are games. A list of groups has no metadata and
        // never will, and six empty labels beside Arcade say nothing.
        if self.screen != Screen::Browse || self.browsing != Browsing::Games {
            return Vec::new();
        }
        let details = self
            .here
            .get(self.game_list.selected())
            .filter(|row| !row.is_folder())
            .map(|row| row.details.clone())
            .unwrap_or_default();
        let values = details.values();
        browse::Details::LABELS
            .iter()
            .zip(values)
            .map(|(label, value)| DetailLine {
                label: SharedString::from(*label),
                value: SharedString::from(value),
            })
            .collect()
    }

    /// How much room the lines under the picture take, if any.
    ///
    /// Set here rather than with the rest of the geometry because it
    /// depends on what the cursor is on, and the cursor moves far more
    /// often than the shape of the screen does. Only over a game: a folder
    /// has nothing to say, and six empty labels beside it push the picture
    /// up the screen to make room for nothing.
    fn apply_detail_panel(&self) {
        let line = (self.geometry.small_font * 1.3).ceil().max(7.0);
        let wanted = line * browse::Details::LABELS.len() as f32;
        // A little air under the last line, on screens that have any to
        // spare. On 240 lines every one is spoken for, and adding it there
        // would move the picture on a tube that is already right.
        let room = if self.height > 240 {
            self.geometry.pad
        } else {
            0.0
        };
        let panel = (wanted + room).min((self.height as f32 * 0.34).floor());
        let over_game = self.screen == Screen::Browse
            && self.browsing == Browsing::Games
            && self
                .here
                .get(self.game_list.selected())
                .is_some_and(|row| !row.is_folder());
        // Not the carousel: it is a row of pictures, and six lines of text
        // under them leaves the picture too small to be the point of it.
        let wants = self.layout == Layout::Details && over_game;
        self.ui.set_detail_line(line);
        self.ui.set_detail_height(if wants { panel } else { 0.0 });
    }

    fn update_chrome(&self) {
        self.apply_detail_panel();
        self.ui
            .set_detail_lines(ModelRc::new(VecModel::from(self.detail_lines())));
        self.ui
            .set_clock(SharedString::from(self.status.clock.as_str()));
        self.ui.set_wifi(self.status.wifi);
        self.ui.set_bluetooth(self.status.bluetooth);
        // Just the multiple. The chevrons were a way of showing the speed
        // without reading it, and the badge is only up for a moment now.
        let (multiple, _) = SPEED_STEPS[self.speed.min(SPEED_STEPS.len() - 1)];
        self.ui
            .set_speed_badge(SharedString::from(if multiple.fract() == 0.0 {
                format!("{multiple:.0}x")
            } else {
                format!("{multiple:.1}x")
            }));
        self.ui.set_speed_visible(
            self.speed_shown_at
                .is_some_and(|at| at.elapsed() < Duration::from_millis(Self::SPEED_BADGE_MS)),
        );

        let list = self.active_list();
        let (heading, status) = match self.screen {
            Screen::Screensaver => (String::new(), String::new()),
            Screen::FavoriteFolder => ("Keep it in".to_string(), String::new()),
            Screen::Find => match self.find_mode {
                FindMode::NewFolder => (
                    format!("New folder {}", self.filter),
                    "B when done".to_string(),
                ),
                FindMode::Jump => ("Jump to letter".to_string(), String::new()),
                FindMode::Search => (
                    if self.filter.is_empty() {
                        "Search".to_string()
                    } else {
                        format!("Search {}", self.filter)
                    },
                    format!("{} found", self.here.len()),
                ),
            },
            Screen::Browse => {
                let name = match self.browsing {
                    Browsing::Categories => "Degauss".to_string(),
                    Browsing::Systems => self
                        .open_category
                        .clone()
                        .unwrap_or_else(|| "Systems".to_string()),
                    Browsing::Games => self.here_label(),
                };
                (
                    format!(
                        "{name}   {}/{}",
                        (list.selected() + 1).min(list.count()),
                        list.count()
                    ),
                    speed_badge(self.speed),
                )
            }
            Screen::Menu => ("Menu".to_string(), String::new()),
            Screen::Context => ("This folder".to_string(), String::new()),
            Screen::Options | Screen::Advanced => (
                if self.screen == Screen::Advanced {
                    "Advanced".to_string()
                } else {
                    "Options".to_string()
                },
                self.option_ids()
                    .get(self.active_list().selected())
                    .map(|o| o.help().to_string())
                    .unwrap_or_default(),
            ),
            Screen::Help => ("Help".to_string(), String::new()),
            Screen::About => ("About".to_string(), String::new()),
            Screen::Splash => (String::new(), String::new()),
        };

        self.ui.set_heading(SharedString::from(heading));
        self.ui.set_status(SharedString::from(status));
        self.ui.set_show_stats(self.show_stats);
        self.ui.set_stats(SharedString::from(self.stats_line()));
        // A plain message says how to leave, the way the questions already
        // carry their keys in their own text. Questions keep their "A yes,
        // B no" and build progress replaces itself every frame, so neither
        // takes the hint.
        let overlay = match &self.message {
            Some(text) if self.pending.is_none() && self.build.is_none() => {
                format!("{text}\n\nB close")
            }
            Some(text) => text.clone(),
            None => String::new(),
        };
        self.ui.set_overlay(SharedString::from(overlay));
    }
    fn stats_line(&self) -> String {
        let summary = self.timer.summary();
        format!(
            "{:.0} fps   avg {:.1}  p95 {:.1}  max {:.1} ms   build {:.1}   art {} loads {} skipped   {}",
            summary.fps(),
            summary.avg_us as f32 / 1000.0,
            summary.p95_us as f32 / 1000.0,
            summary.max_us as f32 / 1000.0,
            self.last_build.as_micros() as f32 / 1000.0,
            self.art.loads,
            self.art.deferred,
            self.present_label,
        )
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    /// The drawing path the settings asked for, so the presenter can be put
    /// into it before the first frame.
    pub fn present_mode(&self) -> PresentMode {
        PresentMode::parse(self.present_label).unwrap_or(PresentMode::Direct)
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn run(
        &mut self,
        surface: &mut dyn Surface,
        input: &mut InputReader,
        presenter: &mut Presenter,
    ) -> Result<Outcome> {
        let mut repeater = Repeater::new(RepeatConfig {
            interval: Duration::from_millis(self.speed_ms()),
            ..Default::default()
        });
        let mut first_frame_done = false;
        // Dropped for good if the device ever declines, so a framebuffer
        // without the ioctl costs one failed call rather than one per frame.
        let mut vsync_usable = true;

        loop {
            let now = Instant::now();

            slint::platform::update_timers_and_animations();

            for edge in input.poll() {
                let action = match edge {
                    KeyEdge::Down(action) => repeater.press(action, now),
                    KeyEdge::Up(action) => {
                        repeater.release(action);
                        None
                    }
                };
                if let Some(action) = action {
                    if let Some(outcome) = self.handle(action) {
                        self.save_settings();
                        return Ok(outcome);
                    }
                }
            }
            for action in repeater.tick(now) {
                if let Some(outcome) = self.handle(action) {
                    self.save_settings();
                    return Ok(outcome);
                }
            }

            let wanted = Duration::from_millis(self.speed_ms());
            if repeater.interval() != wanted {
                repeater.set_interval(wanted);
            }

            if self.screen == Screen::Splash
                && now.duration_since(self.started) >= Duration::from_millis(SPLASH_MS)
            {
                self.leave_splash();
            }

            // The clock moves and the speed badge expires; both live in the
            // bar, so both mean a redraw.
            if self.status.refresh(now) {
                self.dirty = true;
            }
            if self.speed_shown_at.is_some_and(|at| {
                now.duration_since(at) >= Duration::from_millis(Self::SPEED_BADGE_MS)
            }) {
                self.speed_shown_at = None;
                self.dirty = true;
            }

            // Left alone for long enough, show pictures instead.
            let idle = self.screensaver_after();
            if idle > 0
                && !matches!(self.screen, Screen::Screensaver | Screen::Splash)
                && now.duration_since(self.last_input) >= Duration::from_secs(idle)
            {
                self.enter_screensaver();
            }
            if self.screen == Screen::Screensaver {
                self.advance_saver(now);
            }

            if self.pending_present_switch {
                self.pending_present_switch = false;
                let next = presenter.mode().next();
                presenter.set_mode(next, &self.window);
                self.present_label = next.label();
                self.settings.present = Some(next.label().to_string());
                self.timer = FrameTimer::new();
                self.dirty = true;
            }

            // Timed from here: fetching artwork and building rows happen on
            // the way to a frame, and timing only the draw would report fast
            // frames while the screen visibly hitched.
            let frame_start = Instant::now();

            if self.art_pending && self.art_has_settled(now) {
                self.load_art();
                self.dirty = true;
            }
            if self.dirty {
                self.refresh();
            }
            self.last_build = frame_start.elapsed();

            let drew = if let Some(work) = presenter.draw(&self.window, surface)? {
                surface.present()?;
                self.last_work = work;
                self.timer.record(frame_start.elapsed());
                if !first_frame_done {
                    first_frame_done = true;
                    self.startup.first_frame_ms = self.started.elapsed().as_millis();
                }
                if self.show_stats {
                    self.update_chrome();
                    self.window.request_redraw();
                }
                true
            } else {
                if !repeater.anything_held() && self.opening.is_none() {
                    std::thread::sleep(Duration::from_millis(2));
                }
                false
            };

            // Paced by the display when it will say so. Without this the
            // loop draws into memory that is being scanned out, which tears
            // while the list is moving, and spins through frames the tube
            // never shows. Asked only after a frame was actually drawn, and
            // dropped for good the first time the device declines.
            if drew && vsync_usable && !surface.wait_for_vsync() {
                vsync_usable = false;
                crate::note("vsync        stopped answering; pacing without it");
            }

            // The message is on screen now, so the reading can happen.
            if self.opening.take().is_some() {
                self.open_system_now();
            }
            // One system per frame, so the count on screen keeps moving.
            // After the wordmark, not over it: the first thing anybody sees
            // should be the thing they started, not a progress message.
            if self.build.is_some() && first_frame_done && self.screen != Screen::Splash {
                self.build_one_system();
            }
        }
    }

    pub fn render_once(
        &mut self,
        surface: &mut dyn Surface,
        presenter: &mut Presenter,
    ) -> Result<()> {
        // Nobody is waiting on a still image, so the card is read before
        // the frame rather than after it: what is drawn then matches what
        // browsing would show.
        // Nobody is waiting on a still image, so the whole thing happens
        // here rather than a system at a time.
        while self.build.is_some() {
            self.build_one_system();
        }
        self.load_art();
        self.refresh();
        self.startup.first_frame_ms = self.started.elapsed().as_millis();
        for _ in 0..2 {
            let started = Instant::now();
            if let Some(work) = presenter.draw(&self.window, surface)? {
                self.last_work = work;
            }
            surface.present()?;
            self.timer.record(started.elapsed());
            self.update_chrome();
            self.window.request_redraw();
        }
        Ok(())
    }

    pub fn bench(
        &mut self,
        surface: &mut dyn Surface,
        presenter: &mut Presenter,
        frames: u32,
    ) -> Result<BenchReport> {
        self.active_list_mut().go_first();
        self.covers = CoverCache::new(
            self.config
                .app
                .cover_size
                .max(self.width.max(self.height) / 2),
            self.config.app.art_cache.max(8),
            [
                self.config.colors.surface.r,
                self.config.colors.surface.g,
                self.config.colors.surface.b,
            ],
        );
        self.timer = FrameTimer::new();
        self.art = ArtStats::default();
        self.dirty = true;

        let started = Instant::now();
        let mut drawn = 0u32;
        let mut render_total = Duration::ZERO;
        let mut blit_total = Duration::ZERO;
        let mut build_total = Duration::ZERO;

        for _ in 0..frames {
            if let Some(outcome) = self.handle(Action::Down) {
                return Err(crate::error::DegaussError::unsupported(
                    "benchmark",
                    format!("the loop asked to stop early: {outcome:?}"),
                ));
            }

            let frame_start = Instant::now();
            // Scrolling without pause means artwork never settles, so it is
            // loaded every frame here: the worst case, not the typical one.
            self.load_art();
            if self.dirty {
                self.refresh();
            }
            self.last_build = frame_start.elapsed();
            self.update_chrome();
            self.window.request_redraw();

            if let Some(work) = presenter.draw(&self.window, surface)? {
                surface.present()?;
                self.last_work = work;
                render_total += work.render;
                blit_total += work.blit;
                build_total += self.last_build;
                drawn += 1;
            }
            self.timer.record(frame_start.elapsed());

            let list = self.active_list();
            if list.selected() + 1 >= list.count() {
                self.active_list_mut().go_first();
                self.dirty = true;
            }
        }

        if let Some(text) = report_cover_failures(&self.covers) {
            println!("{text}");
        }

        Ok(BenchReport {
            frames_requested: frames,
            frames_drawn: drawn,
            wall: started.elapsed(),
            render_total,
            blit_total,
            build_total,
            summary: self.timer.summary(),
            covers: self.covers.stats,
            art: self.art,
        })
    }

    /// Open a system straight away, for the headless paths.
    /// Start browsing at once, with no wordmark.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn skip_splash(&mut self) {
        if self.screen == Screen::Splash {
            self.leave_splash();
        }
    }

    /// Where the user is standing, in a form that can be written down.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn position(&self) -> crate::state::State {
        let system = self.open_system.clone().unwrap_or_default();
        let trail: Vec<(Place, usize)> = self
            .trail
            .iter()
            .map(|crumb| (crumb.place.clone(), crumb.selected))
            .collect();
        crate::state::State::record(
            &system,
            self.open_category.as_deref().unwrap_or_default(),
            &trail,
            self.game_list.selected(),
            &self.left_at,
        )
    }

    /// Put the user back where they were before the game.
    ///
    /// Every step is allowed to fail without complaint. A card whose folders
    /// have moved since should land wherever the walk got to, which is where
    /// going back would have led anyway, rather than refusing to start.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn restore_position(&mut self, saved: &crate::state::State) {
        if saved.system.is_empty() {
            return;
        }
        if !saved.category.is_empty() {
            self.open_category = Some(saved.category.clone());
            self.rebuild_system_list();
            // Going in, a group holding one system is stepped straight
            // through, and going back has to step back out the same way.
            // Without this, coming out of a game into Arcade needed two
            // presses of B to reach the groups: one to a list whose only
            // entry was the system already open.
            self.skipped_systems = self.systems.len() == 1;
        }
        let Some(index) = self
            .systems
            .iter()
            .position(|system| system.def.id == saved.system)
        else {
            return;
        };
        self.system_list.go_first();
        self.system_list.move_items(index as isize);
        self.browsing = Browsing::Systems;
        // Seeded before the walk, so every folder the walk lists resolves
        // its remembered row on the way down, and an ordinary re-entry
        // after the resume agrees with the resume itself.
        self.left_at = saved.left_at.clone();
        self.open_system_now();
        if self.open_system.is_none() {
            return;
        }

        // The first place is the one opening the system already reached.
        // `places` stops at anything the card no longer has, so a renamed
        // folder lands on its parent instead of an error screen.
        for place in saved.places().into_iter().skip(1) {
            self.enter(place);
        }
        // Every level keeps the row it was left on. `enter` above wrote the
        // live cursor into each parent as it walked, which during a restore
        // is always zero, so the saved values go back in afterwards: without
        // this, Back out of a game landed at the top of every parent folder.
        for (crumb, saved_place) in self.trail.iter_mut().zip(saved.trail.iter()) {
            crumb.selected = saved_place.selected();
        }
        // The walk also wrote places down as it stepped, from cursors that
        // were the walk's own rather than the user's. The saved memory is
        // the truthful one, so it goes back in whole.
        self.left_at = saved.left_at.clone();
        self.game_list.select(saved.selected);
        self.touch_selection();
        self.apply_geometry();
    }

    pub fn open_system_by_index(&mut self, index: usize) {
        // Every system, including the ones a setting hides: the headless
        // paths address the table as it is written, and an index taken
        // against the whole list must not be read against a shorter one.
        // It was: hiding the systems that hold nothing moved every index
        // after the first of them, so asking for one system rendered
        // another.
        self.open_category = None;
        self.systems = self.all_systems.clone();
        self.system_list = ListState::new(self.systems.len(), self.geometry.visible);
        if index < self.systems.len() {
            self.system_list.select(index);
            self.browsing = Browsing::Systems;
            // Immediately, not on the next frame: the headless paths draw
            // one frame and stop, so there is no next frame to be read on.
            self.open_system_now();
        }
    }

    /// Open a screen directly, for the headless render path.
    pub fn set_screen(&mut self, screen: Screen) {
        // Through the same door the button uses. The menu's contents depend
        // on where it was opened from, and a render path that set the screen
        // directly drew an empty menu: a picture of something the app never
        // shows is worse than no picture.
        if screen == Screen::Screensaver {
            // Through its own door: it has to find a picture before it can
            // show one, and a screensaver drawing nothing is not a preview.
            self.enter_screensaver();
            return;
        }
        if screen == Screen::Menu {
            self.open_menu();
            return;
        }
        if screen == Screen::Context {
            // Same reason as the menu: what it offers depends on where it
            // was opened from.
            self.open_context();
            return;
        }
        if screen == Screen::Find {
            // Same reason: the grid is laid out by the door, not by the
            // screen it sets.
            self.open_find(FindMode::Jump);
            return;
        }
        self.screen = screen;
        self.apply_geometry();
    }

    /// Enter the search with a query already typed, the way it looks after
    /// somebody has picked those letters off the grid. For `--render`: the
    /// grid takes one letter per keypress and an image cannot press keys.
    pub fn search_for(&mut self, text: &str) {
        self.open_find(FindMode::Search);
        self.filter = squashed(text);
        self.apply_filter();
    }

    pub fn set_layout(&mut self, layout: Layout) {
        self.layout = layout;
        self.apply_geometry();
    }

    pub fn select(&mut self, index: usize) {
        self.active_list_mut().go_first();
        self.active_list_mut().move_items(index as isize);
        self.touch_selection();
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn covers(&self) -> &CoverCache {
        &self.covers
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn frame_summary(&self) -> crate::metrics::FrameSummary {
        self.timer.summary()
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn art_stats(&self) -> ArtStats {
        self.art
    }
}

fn plain_row(title: &str, value: &str) -> Row {
    Row {
        title: SharedString::from(title),
        favorite: false,
        cover: slint::Image::default(),
        has_cover: false,
        value: SharedString::from(value),
    }
}

fn on_off(value: bool) -> String {
    if value { "On" } else { "Off" }.to_string()
}

/// A setting's value with its first letter raised.
///
/// The stored form stays lower case, because that is what the settings
/// file has always held and what parses it back; only what is read on
/// screen changes.
fn capitalised(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Move along a list of choices without wrapping: running off either end
/// should stop, not jump to the other extreme.
/// Move through a setting's choices, round the ends.
///
/// A setting that stops at its last choice can only be walked one way, and
/// there is no other control to walk it back with.
fn step(current: usize, delta: isize, len: usize) -> usize {
    if len == 0 {
        return 0;
    }
    let len = len as isize;
    (current as isize + delta).rem_euclid(len) as usize
}

/// The artwork that could not be read, as text for whoever asked.
///
/// Returned rather than printed: on the device the console is behind the
/// picture, so the caller decides whether this goes to a log or to a
/// terminal somebody is actually looking at.
pub fn report_cover_failures(cache: &CoverCache) -> Option<String> {
    let failures: Vec<_> = cache.failures().take(10).collect();
    if failures.is_empty() {
        return None;
    }
    let mut out = format!("art failures ({} shown)", failures.len());
    for (path, reason) in failures {
        out.push_str(&format!("\n             {}: {reason}", path.display()));
    }
    Some(out)
}

#[derive(Debug, Clone, Copy)]
pub struct BenchReport {
    pub frames_requested: u32,
    pub frames_drawn: u32,
    pub wall: Duration,
    pub render_total: Duration,
    pub blit_total: Duration,
    pub build_total: Duration,
    pub summary: crate::metrics::FrameSummary,
    pub covers: CoverStats,
    pub art: ArtStats,
}

impl BenchReport {
    pub fn print(&self, label: &str) {
        println!("\n--- {label} ---");
        println!(
            "frames       {} drawn of {} requested in {:.2} s",
            self.frames_drawn,
            self.frames_requested,
            self.wall.as_secs_f32()
        );
        println!(
            "frame time   avg {:.2} ms   p95 {:.2} ms   max {:.2} ms",
            self.summary.avg_us as f32 / 1000.0,
            self.summary.p95_us as f32 / 1000.0,
            self.summary.max_us as f32 / 1000.0
        );
        println!("implied fps  {:.1}", self.summary.fps());
        if self.frames_drawn > 0 {
            let per =
                |total: Duration| total.as_micros() as f32 / 1000.0 / self.frames_drawn as f32;
            println!(
                "per frame    art+rows {:.2} ms   draw {:.2} ms   copy to screen {:.2} ms",
                per(self.build_total),
                per(self.render_total),
                per(self.blit_total)
            );
        }
        println!(
            "covers       {} decoded, avg {} us, worst {} us, {} evictions, {} hits, scale {} us avg, {} KB held",
            self.covers.decoded,
            self.covers.avg_decode_us(),
            self.covers.worst_decode_us,
            self.covers.evictions,
            self.covers.cache_hits,
            self.covers
                .scale_us_total
                .checked_div(self.covers.decoded)
                .unwrap_or(0),
            self.covers.bytes_held / 1024
        );
        println!(
            "art          {} loads, {} skipped by scrolling, worst load {} us",
            self.art.loads, self.art.deferred, self.art.worst_load_us
        );
    }
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Quit,

    /// A launch carries its finished plan rather than a row index: the plan
    /// is built while the interface is still up, so everything that can go
    /// wrong is a message on screen and never an exit. Boxed because a plan
    /// carries a whole MGL and an outcome moves by value.
    Launch {
        plan: Box<crate::launch::LaunchPlan>,
        name: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_setting_cycles_round_its_choices() {
        // Left and right are the only controls a setting has. One that
        // stopped at its last choice could be walked one way and never
        // walked back.
        assert_eq!(step(0, -1, 4), 3, "back from the first is the last");
        assert_eq!(step(3, 1, 4), 0, "on from the last is the first");
        assert_eq!(step(1, 1, 4), 2);
        assert_eq!(step(1, -1, 4), 0);
        assert_eq!(step(0, 0, 0), 0, "no choices is not a crash");
    }

    #[test]
    fn the_menu_offers_hiding_only_where_it_makes_sense() {
        let systems = menu_entries(Browsing::Systems, Some("Commodore 64"));
        assert!(
            systems.iter().any(|e| e == "Hide Commodore 64"),
            "hiding belongs on the systems list"
        );
        let games = menu_entries(Browsing::Games, Some("Commodore 64"));
        assert!(
            !games.iter().any(|e| e.starts_with("Hide ")),
            "there is nothing to hide while looking at games"
        );
        for menu in [&systems, &games] {
            assert!(menu.iter().any(|e| e == "Options"));
            assert!(menu.iter().any(|e| e.starts_with("Exit")));
        }
    }

    #[test]
    fn the_help_describes_every_button_that_does_something() {
        // If this stops being true the help is lying to someone holding a
        // pad with no keyboard in reach.
        let text = HELP.join(" ");
        assert!(text.contains("four"));
        for button in ["A ", "B ", "X ", "Y "] {
            assert!(text.contains(button), "{button} is not explained");
        }
        for line in HELP {
            // The narrowest screen this runs on is 352 pixels wide.
            assert!(line.len() <= 46, "too long to fit: {line:?}");
        }
    }

    #[test]
    fn layout_names_round_trip() {
        for layout in [Layout::Details, Layout::Tiled, Layout::List] {
            assert_eq!(Layout::parse(layout.label()), Some(layout));
        }
        assert_eq!(Layout::parse("nonsense"), None);
    }

    #[test]
    fn cycling_layouts_visits_every_one_and_returns() {
        // Every view must be reachable from the X menu, which only ever
        // steps forward: one that is not in the cycle cannot be chosen.
        let all = [
            Layout::Details,
            Layout::Tiled,
            Layout::Carousel,
            Layout::List,
        ];
        let mut seen = Vec::new();
        let mut layout = Layout::Details;
        for _ in 0..all.len() {
            seen.push(layout);
            layout = layout.next();
        }
        assert_eq!(layout, Layout::Details, "cycling must return to the start");
        for one in all {
            assert!(seen.contains(&one), "{one:?} is not in the cycle");
        }
        // And each label must survive a round trip through settings.toml.
        for one in all {
            assert_eq!(Layout::parse(one.label()), Some(one));
        }
    }

    #[test]
    fn a_search_ignores_spaces_and_punctuation_on_both_sides() {
        // The grid has no space key, so typing SUPERM has to find Super
        // Mario, and an apostrophe must not hide a title either.
        assert_eq!(squashed("Super Mario Bros."), "SUPERMARIOBROS");
        assert_eq!(squashed("Ghosts 'n Goblins"), "GHOSTSNGOBLINS");
        assert!(squashed("Super Mario Bros.").contains("SUPERM"));
        assert!(squashed("Ghosts 'n Goblins").contains("NGOB"));
    }

    #[test]
    fn a_jump_compares_the_first_character_in_lower_case() {
        assert_eq!(first_letter("zynaps"), 'z');
        assert_eq!(first_letter("1942"), '1');
        // Nothing at all sorts before every letter rather than panicking.
        assert_eq!(first_letter(""), ' ');
    }

    #[test]
    fn the_grid_fills_its_rows_exactly() {
        // A ragged last row would leave a hole the cursor walks into.
        let cells = FIND_CELLS.chars().count();
        assert_eq!(cells % FIND_COLUMNS, 0, "{cells} cells in {FIND_COLUMNS}");
        assert!(FIND_CELLS.chars().all(|c| c.is_ascii_alphanumeric()));
    }

    #[test]
    fn searching_and_jumping_are_offered_where_the_long_lists_are() {
        let games = context_entries(Browsing::Games, false, None, None);
        assert!(games.iter().any(|entry| entry == JUMP));
        assert!(games.iter().any(|entry| entry == SEARCH));
        // Nothing to clear until something has been typed.
        assert!(!games.iter().any(|entry| entry == CLEAR_SEARCH));
        assert!(context_entries(Browsing::Games, true, None, None)
            .iter()
            .any(|entry| entry == CLEAR_SEARCH));
        // Three groups need neither.
        let groups = context_entries(Browsing::Categories, false, None, None);
        assert!(!groups.iter().any(|entry| entry == JUMP));
    }

    #[test]
    fn a_jump_lands_on_the_letter_itself_wherever_it_sits() {
        // Folders first, then files, each sorted on its own: the first
        // letters climb twice, which is what broke this.
        let list = [
            ('_', true),
            ('a', true),
            ('c', true),
            ('s', true),
            ('a', false),
            ('b', false),
            ('m', false),
            ('z', false),
        ];
        // A letter the files have and the folders do not.
        assert_eq!(jump_target(&list, 'b'), Some(5));
        assert_eq!(jump_target(&list, 'm'), Some(6));
        assert_eq!(jump_target(&list, 'z'), Some(7));
        // A letter both have: the first one in the list, which is the
        // folder, because that is what is on screen first.
        assert_eq!(jump_target(&list, 'a'), Some(1));
    }

    #[test]
    fn a_jump_to_a_missing_letter_lands_after_it_among_the_files() {
        let list = [('c', true), ('s', true), ('a', false), ('m', false)];
        // No D anywhere. The next file is M, not the S folder above it.
        assert_eq!(jump_target(&list, 'd'), Some(3));
        // Past everything.
        assert_eq!(jump_target(&list, 'z'), None);
    }

    #[test]
    fn a_jump_works_where_every_entry_is_a_folder() {
        // Neo Geo keeps its games as folders, so there is no file run to
        // fall back to and the folders have to answer.
        let list = [('a', true), ('k', true), ('m', true)];
        assert_eq!(jump_target(&list, 'k'), Some(1));
        assert_eq!(jump_target(&list, 'l'), Some(2));
        assert_eq!(jump_target(&list, 'z'), None);
    }

    #[test]
    fn a_screensaver_caption_gives_up_the_title_before_the_machine() {
        // Two screenshots from the same system look alike, so the machine
        // is the half worth keeping.
        let long = saver_caption("The Great Giana Sisters Deluxe Edition", "Commodore 64");
        assert!(long.ends_with(" - Commodore 64"), "{long}");
        assert!(
            long.chars().count() <= 34,
            "{} chars: {long}",
            long.chars().count()
        );
        assert!(long.contains("..."), "{long}");
        // Short enough to fit is left alone.
        assert_eq!(saver_caption("Nemesis", "C64"), "Nemesis - C64");
    }

    #[test]
    fn favouriting_is_offered_over_a_game_and_not_over_a_folder() {
        // A folder is not a favourite, and offering to make one of it
        // would write a file pointing at nothing.
        let over_folder = context_entries(Browsing::Games, false, None, None);
        assert!(!over_folder.iter().any(|e| e == ADD_FAVORITE));
        assert!(!over_folder.iter().any(|e| e == REMOVE_FAVORITE));

        let over_game = context_entries(Browsing::Games, false, Some(false), None);
        assert!(over_game.iter().any(|e| e == ADD_FAVORITE));
        assert!(!over_game.iter().any(|e| e == REMOVE_FAVORITE));

        // Already one: the only thing left to do is stop.
        let over_favourite = context_entries(Browsing::Games, false, Some(true), None);
        assert!(over_favourite.iter().any(|e| e == REMOVE_FAVORITE));
        assert!(!over_favourite.iter().any(|e| e == ADD_FAVORITE));
    }

    #[test]
    fn the_contextual_menu_comes_in_groups_with_blanks_between_them() {
        // A dozen entries in one column is a wall. The blanks are read,
        // never chosen, and the movement keys step over them.
        let entries = context_entries(Browsing::Games, true, Some(false), Some(false));
        assert!(entries.iter().any(|e| e.is_empty()), "groups are separated");
        assert!(
            !entries.first().is_some_and(String::is_empty),
            "no blank first"
        );
        assert!(
            !entries.last().is_some_and(String::is_empty),
            "no blank last"
        );
        // Never two together, which would read as a missing entry.
        assert!(
            !entries
                .windows(2)
                .any(|pair| pair[0].is_empty() && pair[1].is_empty()),
            "{entries:?}"
        );
        // The order of the groups: chance, then keeping, then finding,
        // then hiding, then how it looks.
        let seen: Vec<&str> = entries.iter().map(String::as_str).collect();
        let at = |what: &str| seen.iter().position(|e| *e == what).expect(what);
        assert!(at(RANDOM) < at(ADD_FAVORITE));
        assert!(at(ADD_FAVORITE) < at(JUMP));
        assert!(at(JUMP) < at(HIDE_THIS));
        assert!(at(HIDE_THIS) < at(CHANGE_VIEW));
        // Both ways of picking something at random sit together.
        assert_eq!(at(RANDOM) + 1, at(RANDOM_FAVORITE));
    }

    #[test]
    fn a_tube_is_measured_exactly_as_it_was_and_a_big_screen_is_not() {
        // The ceilings on text size were set for 240 lines. Raising them
        // so a 720 line screen does not look like a 240 line one with more
        // space around it must not move a single pixel on the tube, which
        // is the only screen any of this was tuned on.
        // The shipped defaults, parsed the way the program parses them.
        let config = Config::parse("[app]\n", std::path::Path::new("test")).expect("defaults");
        for layout in [
            Layout::Details,
            Layout::Tiled,
            Layout::List,
            Layout::Carousel,
        ] {
            let crt = Geometry::compute(layout, false, false, false, 352, 240, &config);
            let hd = Geometry::compute(layout, false, false, false, 1280, 720, &config);
            assert!(
                hd.body_font > crt.body_font,
                "{layout:?}: text has to grow with the screen, {} to {}",
                crt.body_font,
                hd.body_font
            );
            assert!(hd.small_font > crt.small_font, "{layout:?}");
            assert!(hd.chrome > crt.chrome, "{layout:?}");
        }

        // And the numbers the tube actually gets, written down so a change
        // to them has to be deliberate.
        let crt = Geometry::compute(Layout::Details, false, false, false, 352, 240, &config);
        assert_eq!(crt.body_font, 16.0, "preview body text on the tube");
        assert_eq!(crt.small_font, 10.0, "preview small text on the tube");
        let plain = Geometry::compute(Layout::List, true, true, true, 352, 240, &config);
        assert_eq!(plain.chrome, 20.0, "title bar height on the tube");
        assert_eq!(plain.small_font, 10.0, "plain small text on the tube");
    }

    fn listed(names: &[&str]) -> Vec<browse::Row> {
        names
            .iter()
            .map(|name| browse::Row {
                name: (*name).into(),
                sort_key: name.to_lowercase(),
                kind: browse::Kind::Play(browse::Launch::File(PathBuf::from(format!(
                    "/games/{name}.d64"
                )))),
                cover: None,
                genre: None,
                favorite: false,
                below: None,
                details: browse::Details::default(),
            })
            .collect()
    }

    #[test]
    fn a_remembered_row_wins_over_the_crumb_index() {
        let rows = listed(&["Alpha", "Beta", "Gamma"]);
        let key = row_key(&rows[1]);
        assert_eq!(reselect(&rows, Some(&key), 0), 1);
    }

    #[test]
    fn a_remembered_row_is_found_where_it_moved_to() {
        // The folder was resorted since it was left: a favourite gathered
        // to the top must still be the row the cursor lands on, not the
        // row now sitting where it used to.
        let rows = listed(&["Alpha", "Beta", "Gamma"]);
        let key = row_key(&rows[2]);
        let resorted = listed(&["Gamma", "Alpha", "Beta"]);
        assert_eq!(reselect(&resorted, Some(&key), 2), 0);
    }

    #[test]
    fn a_row_that_is_gone_falls_back_to_the_crumb_index() {
        // Gone covers hidden too: a row hidden since the last visit is
        // dropped before this runs, and the memory must not resurrect it.
        let rows = listed(&["Alpha", "Beta"]);
        assert_eq!(reselect(&rows, Some("f:/games/Gone.d64"), 1), 1);
    }

    #[test]
    fn a_folder_never_visited_lands_where_the_crumb_says() {
        let rows = listed(&["Alpha", "Beta"]);
        assert_eq!(reselect(&rows, None, 0), 0, "the top, for a fresh crumb");
    }

    #[test]
    fn hiding_the_row_under_the_cursor_leaves_the_cursor_in_place() {
        // relist_here refreshes the crumb to the live cursor before
        // re-listing, so when the remembered row is the very one that
        // vanished, the fallback is where the cursor stood: the row that
        // slid into its place, not the row the folder was entered on.
        let before = listed(&["Alpha", "Beta", "Gamma"]);
        let key = row_key(&before[1]);
        let after = listed(&["Alpha", "Gamma"]);
        assert_eq!(reselect(&after, Some(&key), 1), 1, "Gamma slid into place");

        // Hiding the last row parks that fallback one past the end; the
        // clamp in select() is what keeps it on the new last row.
        let mut list = ListState::new(2, 10);
        list.select(reselect(&after, Some("f:/games/Gone.d64"), 2));
        assert_eq!(list.selected(), 1, "clamped to the new last row");
    }
}
