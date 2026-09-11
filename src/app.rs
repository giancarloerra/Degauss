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

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::platform::software_renderer::MinimalSoftwareWindow;
use slint::{ComponentHandle, ModelRc, SharedPixelBuffer, SharedString, VecModel};

use crate::browse::{self, Library, Place};
use crate::config::{Color as ConfigColor, Colors, Config, SystemConfig};
use crate::covers::{BudgetedCover, CoverCache, CoverStats};
use crate::error::{DegaussError, Result};
use crate::font::Font;
use crate::input::{
    Action, InputReader, KeyEdge, RepeatConfig, Repeater, SPEED_START, SPEED_STEPS,
};
use crate::list_state::ListState;
use crate::metrics::{FrameTimer, StartupTimings};
#[cfg(test)]
use crate::options::OPTIONS;
use crate::options::{speed_badge, speed_label, OptionId, OptionsPage, ADVANCED};
use crate::render::{FrameWork, PresentMode, Presenter};
use crate::settings::{CustomViews, SaveOutcome, Settings};
use crate::surface::Surface;
use crate::systems::{is_favorites, FoundSystem, SystemDef};

#[cfg(test)]
#[path = "ui_acceptance_tests.rs"]
mod ui_acceptance_tests;
use crate::theme::{Theme, ThemeSet};
use crate::theme_editor::{
    EditorEffect, EditorMode, ThemeEditor, EDITOR_ROWS, NAME_CANCEL, NAME_CELLS, NAME_COLUMNS,
    NAME_SAVE,
};
use crate::{DegaussWindow, DetailLine, Row};

/// Which screen is in front.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// The wordmark, shown briefly on the way in.
    Splash,
    Browse,
    Menu,
    Scripts,
    /// What can be done with the folder on screen, on its own button.
    Context,
    OptionsRoot,
    Options,
    Information,
    /// On-device palette and wordmark editor.
    ThemeEditor,
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
    /// ScreenScraper settings for one pending scope.
    Scraper,
    /// On-screen entry of a ScreenScraper username or password.
    ScraperKeyboard,
    /// A blocking ScreenScraper run and its final report.
    ScraperProgress,
    /// A PNG or JPEG from the logos folder to use for one master category.
    CategoryImage,
    /// Candidate matches for one unresolved game, with artwork preview.
    ScraperMatches,
    /// Gamelist or a local MiSTer Artwork Pack for one supported system.
    GameDataSource,
    /// Automatically discovered Pack installations and manual browsing.
    ArtworkPackLocation,
    /// A bounded directory-only browser for a Pack installation.
    ArtworkPackDirectory,
    /// Blocking progress while the newly selected source cache is built.
    SourceProgress,
}

impl Screen {
    /// What the interface layer draws: 0 browse, 1 a plain list, 2 the
    /// wordmark.
    fn ui_index(self) -> i32 {
        match self {
            Screen::Browse => 0,
            Screen::Menu
            | Screen::Scripts
            | Screen::Context
            | Screen::OptionsRoot
            | Screen::Options
            | Screen::Advanced
            | Screen::Help
            | Screen::Scraper
            | Screen::ScraperProgress
            | Screen::CategoryImage
            | Screen::ScraperMatches
            | Screen::GameDataSource
            | Screen::ArtworkPackLocation
            | Screen::ArtworkPackDirectory
            | Screen::SourceProgress => 1,
            Screen::About | Screen::Splash => 2,
            Screen::Screensaver => 3,
            Screen::Find | Screen::ScraperKeyboard => 4,
            Screen::ThemeEditor => 5,
            Screen::Information => 6,
            Screen::FavoriteFolder => 1,
        }
    }
}

fn compact_separators(screen: Screen) -> bool {
    screen == Screen::Context
}

// A model-backed repeated row can settle one renderer pass after the Rust
// model is replaced. Two complete frames cover both the old scene and that
// settled scene; one complete frame left old, right-aligned text visible on
// MiSTer's single reused framebuffer until the row was highlighted.
const COMPLETE_REPAINT_FRAMES: u8 = 2;

fn invalidate_geometry(pending_complete_repaints: &mut u8, dirty: &mut bool) {
    *pending_complete_repaints = COMPLETE_REPAINT_FRAMES;
    *dirty = true;
}

fn take_complete_repaint(pending_complete_repaints: &mut u8) -> bool {
    if *pending_complete_repaints == 0 {
        false
    } else {
        *pending_complete_repaints -= 1;
        true
    }
}

fn context_window(
    menu: &[String],
    state: &ListState,
    visible_rows: usize,
) -> (std::ops::Range<usize>, usize) {
    let weights: Vec<usize> = menu
        .iter()
        .map(|entry| if entry.is_empty() { 1 } else { 2 })
        .collect();
    state.weighted_window(&weights, visible_rows.saturating_mul(2))
}

/// How long the wordmark stays up before browsing starts. Long enough to
/// read, short enough that nobody waits for it; any button skips it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
const SPLASH_MS: u64 = 1400;

/// Something waiting on a yes or no.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Pending {
    Exit,
    RunScript(Box<crate::scripts::Launch>),
    Hide(usize),
    ResetCustomViews,
    ResetHidden,
    StartScrape,
    SaveScraperPassword,
    AcceptScraperStorageForStart,
    AcceptScraperStorageForSearch,
    ClearScraperLogin,
    CancelScrape,
    ClearCategoryImage(ImageTarget),
    UseScraperMatch(Box<crate::scraper::Match>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionInput {
    Previous,
    Next,
    Activate,
}

impl OptionInput {
    fn delta(self) -> isize {
        match self {
            OptionInput::Previous => -1,
            OptionInput::Next | OptionInput::Activate => 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OptionOperation {
    None,
    Adjust(isize),
    OpenThemeEditor,
    ConfirmResetCustomViews,
    ConfirmResetHidden,
    RebuildCache,
    OpenScraperAll,
    OpenAdvanced,
}

/// Translate controls to option semantics before any state can change. The
/// exhaustive value list makes a future action row a compile-time decision
/// instead of silently inheriting left/right adjustment.
fn option_operation(option: OptionId, input: OptionInput) -> OptionOperation {
    match option {
        OptionId::Spacer => OptionOperation::None,
        OptionId::ResetCustomViews => match input {
            OptionInput::Activate => OptionOperation::ConfirmResetCustomViews,
            OptionInput::Previous | OptionInput::Next => OptionOperation::None,
        },
        OptionId::ResetHidden => match input {
            OptionInput::Activate => OptionOperation::ConfirmResetHidden,
            OptionInput::Previous | OptionInput::Next => OptionOperation::None,
        },
        OptionId::RebuildCache => match input {
            OptionInput::Activate => OptionOperation::RebuildCache,
            OptionInput::Previous | OptionInput::Next => OptionOperation::None,
        },
        OptionId::ScrapeAll => match input {
            OptionInput::Activate => OptionOperation::OpenScraperAll,
            OptionInput::Previous | OptionInput::Next => OptionOperation::None,
        },
        OptionId::Advanced => match input {
            OptionInput::Activate => OptionOperation::OpenAdvanced,
            OptionInput::Previous | OptionInput::Next => OptionOperation::None,
        },
        OptionId::Theme => match input {
            OptionInput::Activate => OptionOperation::OpenThemeEditor,
            OptionInput::Previous => OptionOperation::Adjust(-1),
            OptionInput::Next => OptionOperation::Adjust(1),
        },
        OptionId::Speed
        | OptionId::LeftRight
        | OptionId::ArtLimit
        | OptionId::Layout
        | OptionId::Font
        | OptionId::ShowArt
        | OptionId::ArtworkScale
        | OptionId::ShowHidden
        | OptionId::ShowEmpty
        | OptionId::ShowOther
        | OptionId::ShowUtility
        | OptionId::ShowUnstable
        | OptionId::ShowScripts
        | OptionId::CorePreference
        | OptionId::ShowBar
        | OptionId::FavoritesFirst
        | OptionId::HoldXFavorite
        | OptionId::HoldYRandom
        | OptionId::RandomLaunches
        | OptionId::FoldersLast
        | OptionId::ShowStats
        | OptionId::Present
        | OptionId::OverscanX
        | OptionId::OverscanY
        | OptionId::ShiftX
        | OptionId::ShiftY
        | OptionId::Screensaver => OptionOperation::Adjust(input.delta()),
    }
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

/// The exact browse place whose optional view is being resolved. Games-level
/// places carry the stable system id as well as the place key because roots
/// and even absolute folder paths can legitimately belong to several systems.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ViewPlace {
    Categories,
    Systems(String),
    Games { system: String, place: String },
}

impl ViewPlace {
    fn get<'a>(&self, views: &'a CustomViews) -> Option<&'a str> {
        match self {
            ViewPlace::Categories => views.categories.as_deref(),
            ViewPlace::Systems(category) => views.systems.get(category).map(String::as_str),
            ViewPlace::Games { system, place } => views
                .games
                .get(system)
                .and_then(|places| places.get(place))
                .map(String::as_str),
        }
    }

    fn set(&self, views: &mut CustomViews, layout: String) {
        match self {
            ViewPlace::Categories => views.categories = Some(layout),
            ViewPlace::Systems(category) => {
                views.systems.insert(category.clone(), layout);
            }
            ViewPlace::Games { system, place } => {
                views
                    .games
                    .entry(system.clone())
                    .or_default()
                    .insert(place.clone(), layout);
            }
        }
    }

    fn remove(&self, views: &mut CustomViews) -> bool {
        match self {
            ViewPlace::Categories => views.categories.take().is_some(),
            ViewPlace::Systems(category) => views.systems.remove(category).is_some(),
            ViewPlace::Games { system, place } => {
                let Some(places) = views.games.get_mut(system) else {
                    return false;
                };
                let removed = places.remove(place).is_some();
                if places.is_empty() {
                    views.games.remove(system);
                }
                removed
            }
        }
    }
}

/// Whether a released, place-only view key can belong to this system. This is
/// deliberately lexical: the saved key and discovered roots were both made
/// from the same paths on the card, while canonicalising would reject a path
/// merely because removable storage is currently unavailable.
fn legacy_view_belongs_to(key: &str, roots: &[PathBuf]) -> bool {
    let under_a_root = |path: &str| roots.iter().any(|root| Path::new(path).starts_with(root));
    if key == "r:" {
        return roots.len() > 1;
    }
    if let Some(path) = key.strip_prefix("d:").or_else(|| key.strip_prefix("a:")) {
        return under_a_root(path);
    }
    let Some(pair) = key.strip_prefix("l:") else {
        return false;
    };
    // A path can technically contain `|`. Try every possible split and keep
    // the key only when both halves point into this one system.
    pair.match_indices('|').any(|(at, _)| {
        let install = &pair[..at];
        let file = &pair[at + 1..];
        under_a_root(install) && under_a_root(file)
    })
}

/// Move view entries written by releases through v0.3.0 into the exact-place
/// structure. An entry is retained only when precisely one installed system
/// can own it; guessing would apply somebody's view to the wrong library.
fn migrate_legacy_views(settings: &mut Settings, systems: &[FoundSystem]) {
    let legacy = std::mem::take(&mut settings.folder_views);
    for (place, layout) in legacy {
        let mut owners: Vec<&str> = Vec::new();
        for system in systems {
            if legacy_view_belongs_to(&place, &system.paths)
                && !owners.contains(&system.def.id.as_str())
            {
                owners.push(&system.def.id);
            }
        }
        if let [system] = owners.as_slice() {
            settings
                .custom_views
                .games
                .entry((*system).to_string())
                .or_default()
                .entry(place)
                .or_insert(layout);
        }
    }
}

fn owner_candidates<'a>(systems: &'a [FoundSystem], path: &Path) -> Vec<&'a FoundSystem> {
    if !path.exists()
        && !crate::favorites::is_amiga_key(path)
        && crate::zip::split_member_path(path).is_none()
    {
        return Vec::new();
    }
    let candidates: Vec<(&FoundSystem, usize)> = systems
        .iter()
        .filter(|system| !is_favorites(system.category()))
        .filter_map(|system| {
            let depth = system
                .paths
                .iter()
                .filter(|dir| path.starts_with(dir))
                .map(|dir| dir.as_os_str().len())
                .max()
                .unwrap_or(0);
            (depth > 0).then_some((system, depth))
        })
        .collect();
    let Some(deepest) = candidates.iter().map(|(_, depth)| *depth).max() else {
        return Vec::new();
    };
    let mut deepest_candidates: Vec<&FoundSystem> = candidates
        .into_iter()
        .filter(|(_, depth)| *depth == deepest)
        .map(|(system, _)| system)
        .collect();
    if deepest_candidates.len() > 1 {
        let accepting: Vec<&FoundSystem> = deepest_candidates
            .iter()
            .copied()
            .filter(|system| system.to_config().accepts(path))
            .collect();
        if !accepting.is_empty() {
            deepest_candidates = accepting;
        }
    }
    deepest_candidates
}

fn finish_owner(mut candidates: Vec<&FoundSystem>) -> Option<String> {
    if candidates.len() == 1 {
        return Some(candidates[0].def.id.clone());
    }
    let group = candidates
        .first()
        .and_then(|system| crate::artwork_pack::source_group(&system.def.id));
    if group.is_some()
        && candidates
            .iter()
            .all(|system| crate::artwork_pack::source_group(&system.def.id) == group)
    {
        candidates.sort_by(|left, right| left.def.id.cmp(&right.def.id));
        return candidates.first().map(|system| system.def.id.clone());
    }
    None
}

pub(crate) fn owner_of_path(systems: &[FoundSystem], path: &Path) -> Option<String> {
    finish_owner(owner_candidates(systems, path))
}

pub(crate) fn owner_of_favorite(
    systems: &[FoundSystem],
    reference: &crate::favorites::FavoriteReference,
) -> Option<String> {
    let mut candidates = owner_candidates(systems, &reference.owner_target);
    if candidates.is_empty() && reference.owner_target != reference.cache_target {
        candidates = owner_candidates(systems, &reference.cache_target);
    }
    if candidates.len() <= 1 {
        return finish_owner(candidates);
    }

    if let Some(rbf) = reference.rbf.as_deref() {
        let matching: Vec<&FoundSystem> = candidates
            .iter()
            .copied()
            .filter(|system| system.def.rbf.trim().eq_ignore_ascii_case(rbf.trim()))
            .collect();
        if !matching.is_empty() {
            candidates = matching;
        }
    }

    if let Some(setname) = reference.setname.as_deref() {
        let matching: Vec<&FoundSystem> = candidates
            .iter()
            .copied()
            .filter(|system| {
                system
                    .to_config()
                    .setname
                    .as_deref()
                    .is_some_and(|candidate| candidate.eq_ignore_ascii_case(setname.trim()))
            })
            .collect();
        if !matching.is_empty() {
            candidates = matching;
        }
    } else if reference.mgl && reference.rbf.is_some() {
        // With a shared core, omission is meaningful: MiSTer starts the
        // core's default system. A setname-bearing sibling is therefore not
        // the owner of that MGL.
        let defaults: Vec<&FoundSystem> = candidates
            .iter()
            .copied()
            .filter(|system| {
                system
                    .to_config()
                    .setname
                    .as_deref()
                    .is_none_or(str::is_empty)
            })
            .collect();
        if !defaults.is_empty() {
            candidates = defaults;
        }
    }

    finish_owner(candidates)
}

/// Project the currently selected source onto rows stored in MiSTer's
/// favourites tree. The favourite itself carries no presentation data, so its
/// live target and owning system decide which independent cache/provider is
/// allowed to supply it.
fn enrich_favorite_rows(
    rows: &mut [browse::Row],
    systems: &[FoundSystem],
    artwork_pack_roots: &std::collections::BTreeMap<String, String>,
    cache_dir: &Path,
    mut load_provider: impl FnMut(
        &str,
        &Path,
        &crate::cache::SystemCache,
    ) -> Option<crate::artwork_pack::Provider>,
) {
    // What each row points at, and where it sits in this list.
    let mut wanted: HashMap<PathBuf, (crate::favorites::FavoriteReference, Vec<usize>)> =
        HashMap::new();
    for (at, row) in rows.iter().enumerate() {
        let browse::Kind::Play(browse::Launch::File(path)) = &row.kind else {
            continue;
        };
        let Some(reference) = crate::favorites::reference_of_with_systems(path, systems) else {
            continue;
        };
        wanted
            .entry(reference.cache_target.clone())
            .or_insert_with(|| (reference, Vec::new()))
            .1
            .push(at);
    }
    if wanted.is_empty() {
        return;
    }

    // Grouped by the system that owns them, so each cache/provider is read
    // once even when several favourites point into it.
    let mut by_system: HashMap<String, Vec<PathBuf>> = HashMap::new();
    for (target, (reference, _)) in &wanted {
        if let Some(system) = owner_of_favorite(systems, reference) {
            by_system.entry(system).or_default().push(target.clone());
        }
    }

    for (id, targets) in by_system {
        let pack_root =
            crate::artwork_pack::selected_root(artwork_pack_roots, &id).map(Path::to_path_buf);
        let cache = if pack_root.is_some() {
            let data = crate::cache::load_artwork_pack_data(cache_dir, &id);
            data.map(|data| data.cache)
        } else {
            crate::cache::load_system(cache_dir, &id)
        };
        let Some(cache) = cache else {
            continue;
        };
        let provider = pack_root
            .as_deref()
            .and_then(|root| load_provider(&id, root, &cache));
        let looking: HashSet<&PathBuf> = targets.iter().collect();
        for folder in cache.folders.values() {
            let mut cached_rows = folder.rows.clone();
            if let Some(provider) = provider.as_ref() {
                provider.apply_prepared(&mut cached_rows);
            }
            for cached in &cached_rows {
                let Some(path) = row_target(cached) else {
                    continue;
                };
                if !looking.contains(&path) {
                    continue;
                }
                let Some((_, places)) = wanted.get(&path) else {
                    continue;
                };
                for at in places {
                    if let Some(row) = rows.get_mut(*at) {
                        row.name = cached.name.clone();
                        row.sort_key = cached.sort_key.clone();
                        row.cover = cached.cover.clone();
                        row.genre = cached.genre.clone();
                        row.details = cached.details.clone();
                    }
                }
            }
        }
    }
}

fn clear_custom_views(settings: &mut Settings) -> usize {
    let removed = settings.custom_views.len();
    settings.custom_views = CustomViews::default();
    settings.folder_views.clear();
    removed
}

fn clear_hidden(settings: &mut Settings) -> usize {
    let removed = settings.hidden.len() + settings.hidden_paths.len();
    settings.hidden.clear();
    settings.hidden_paths.clear();
    removed
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
    /// Two columns of text, in reading order.
    MultiList,
    /// A dense grid of artwork without permanent captions.
    Gallery,
}

impl Layout {
    #[cfg(test)]
    const ALL: [Layout; 6] = [
        Layout::Details,
        Layout::Tiled,
        Layout::Carousel,
        Layout::List,
        Layout::MultiList,
        Layout::Gallery,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Layout::Details => "details",
            Layout::Tiled => "tiled",
            Layout::Carousel => "carousel",
            Layout::List => "list",
            Layout::MultiList => "multi-list",
            Layout::Gallery => "gallery",
        }
    }

    fn shown(self) -> &'static str {
        match self {
            Layout::Details => "Details",
            Layout::Tiled => "Tiled",
            Layout::List => "List",
            Layout::Carousel => "Carousel",
            Layout::MultiList => "Multi list",
            Layout::Gallery => "Gallery",
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
            "multi-list" => Some(Layout::MultiList),
            "gallery" => Some(Layout::Gallery),
            _ => None,
        }
    }

    fn index(self) -> i32 {
        match self {
            Layout::Details => 0,
            Layout::Tiled => 1,
            Layout::List => 2,
            Layout::Carousel => 3,
            Layout::MultiList => 4,
            Layout::Gallery => 5,
        }
    }

    fn prev(self) -> Self {
        match self {
            Layout::Details => Layout::Gallery,
            Layout::Tiled => Layout::Details,
            Layout::Carousel => Layout::Tiled,
            Layout::List => Layout::Carousel,
            Layout::MultiList => Layout::List,
            Layout::Gallery => Layout::MultiList,
        }
    }

    fn next(self) -> Self {
        match self {
            Layout::Details => Layout::Tiled,
            Layout::Tiled => Layout::Carousel,
            Layout::Carousel => Layout::List,
            Layout::List => Layout::MultiList,
            Layout::MultiList => Layout::Gallery,
            Layout::Gallery => Layout::Details,
        }
    }

    fn rows_have_art(self) -> bool {
        matches!(self, Layout::Tiled | Layout::Carousel | Layout::Gallery)
    }

    /// True when entries are laid out in a grid rather than one per row.
    fn is_grid(self) -> bool {
        matches!(self, Layout::Tiled | Layout::MultiList | Layout::Gallery)
    }
}

fn resolved_layout(temporary: Option<Layout>, custom: Option<&str>, global: Layout) -> Layout {
    temporary
        .or_else(|| custom.and_then(Layout::parse))
        .unwrap_or(global)
}

/// How game artwork is corrected when framebuffer pixels are not square on
/// the physical display. The stored names are part of the settings format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ArtworkScale {
    /// Preserve the original behaviour: fit artwork in framebuffer pixels.
    #[default]
    Framebuffer,
    /// Correct artwork for a display whose physical picture is 4:3.
    FourThree,
    /// Correct artwork for a display whose physical picture is 16:9.
    SixteenNine,
}

impl ArtworkScale {
    const ALL: [ArtworkScale; 3] = [
        ArtworkScale::Framebuffer,
        ArtworkScale::FourThree,
        ArtworkScale::SixteenNine,
    ];

    fn setting(self) -> &'static str {
        match self {
            ArtworkScale::Framebuffer => "framebuffer",
            ArtworkScale::FourThree => "4:3",
            ArtworkScale::SixteenNine => "16:9",
        }
    }

    fn shown(self) -> &'static str {
        match self {
            ArtworkScale::Framebuffer => "Framebuffer",
            ArtworkScale::FourThree => "4:3",
            ArtworkScale::SixteenNine => "16:9",
        }
    }

    fn parse(text: &str) -> Option<Self> {
        match text {
            "framebuffer" => Some(ArtworkScale::Framebuffer),
            "4:3" => Some(ArtworkScale::FourThree),
            "16:9" => Some(ArtworkScale::SixteenNine),
            _ => None,
        }
    }

    fn index(self) -> usize {
        ArtworkScale::ALL
            .iter()
            .position(|&scale| scale == self)
            .expect("every artwork scale is listed")
    }

    /// Horizontal scale in framebuffer coordinates that makes source
    /// artwork retain its own aspect ratio on the selected physical display.
    fn horizontal(self, width: u32, height: u32) -> f32 {
        let display_aspect = match self {
            ArtworkScale::Framebuffer => return 1.0,
            ArtworkScale::FourThree => 4.0 / 3.0,
            ArtworkScale::SixteenNine => 16.0 / 9.0,
        };
        assert!(
            width > 0 && height > 0,
            "framebuffer dimensions are non-zero"
        );
        (width as f32 / height as f32) / display_aspect
    }
}

/// What left and right do while browsing.
///
/// Speed is how Degauss always behaved and stays the default. The other
/// three exist because the two most reachable directions on the stick were
/// permanently spent on a value many people set once: in a folder of
/// twelve thousand games a letter or a page is worth more than a speed
/// change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Horizontal {
    /// Step the scroll speed up and down the ladder.
    #[default]
    Speed,
    /// Jump to the previous or next first letter in the list.
    Letter,
    /// Move a screenful at a time.
    Page,
    /// Move the cursor itself. In the Tiled view this frees up and down to
    /// move a whole row, which is how a grid wants to be driven.
    Direction,
}

impl Horizontal {
    /// Every mode, in the order the option steps through them.
    pub const ALL: [Horizontal; 4] = [
        Horizontal::Speed,
        Horizontal::Letter,
        Horizontal::Page,
        Horizontal::Direction,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Horizontal::Speed => "speed",
            Horizontal::Letter => "letter",
            Horizontal::Page => "page",
            Horizontal::Direction => "direction",
        }
    }

    /// The name the Options row shows. Separate from `label`, which is
    /// the token settings.toml stores: the stored word must never change,
    /// or a written-down choice stops meaning anything on the next start.
    pub fn shown(self) -> &'static str {
        match self {
            Horizontal::Speed => "Scroll speed change",
            Horizontal::Letter => "Letter",
            Horizontal::Page => "Page",
            Horizontal::Direction => "Direction",
        }
    }

    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "speed" => Some(Horizontal::Speed),
            "letter" => Some(Horizontal::Letter),
            "page" => Some(Horizontal::Page),
            "direction" => Some(Horizontal::Direction),
            _ => None,
        }
    }

    fn index(self) -> usize {
        Horizontal::ALL
            .iter()
            .position(|&mode| mode == self)
            .unwrap_or(0)
    }

    /// The word after the arrows in the wide bottom bar, which must say
    /// what the keys actually do now that it depends on a setting.
    fn legend_word(self) -> &'static str {
        match self {
            Horizontal::Speed => "Speed",
            Horizontal::Letter => "Letter",
            Horizontal::Page => "Page",
            Horizontal::Direction => "Move",
        }
    }

    /// The Help line for the stick's sideways travel, filled into HELP at
    /// draw time because what it describes depends on this setting.
    fn help_line(self) -> &'static str {
        match self {
            Horizontal::Speed => "Stick left/right: change speed or setting",
            Horizontal::Letter => "Stick left/right: letter jump or setting",
            Horizontal::Page => "Stick left/right: page jump or setting",
            Horizontal::Direction => "Stick left/right: move or change setting",
        }
    }
}

/// How long a title sits still before it starts to walk, and how long the
/// walk takes. Both are also written into the interface, which does the
/// animation; these are here to say when to turn it round.
/// How long after the list stops before a picture is loaded, when scrolling
/// faster than the limit. Long enough that one more press does not pay for a
/// decode nobody sees, short enough not to feel like waiting.
const ART_AFTER_SCROLL_MS: u64 = 140;

/// At most this many previously unseen Gallery files are decoded before a
/// frame is handed to the renderer. Cached images and remembered failures
/// do not spend the budget.
const GALLERY_DECODE_BUDGET: usize = 2;

/// Gallery has no permanent captions. The selected title uses the same
/// half-second confirmation period as the speed badge.
const GALLERY_TITLE_MS: u64 = 500;

/// How long the speed sits above the bar after it changes.
const SPEED_BADGE_MS: u64 = 500;

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
const RANDOM: &str = "Random Game";

/// The same, drawn only from what has been kept. The heart is written as
/// a literal here for the same reason it is in the interface file: it is a
/// glyph, and glyphs reach the binary by being written down.
const RANDOM_FAVORITE: &str = "Random Favourite";
const GAME_INFORMATION: &str = "Game Information";

/// Switching how the list looks belongs with the folder you are looking at,
/// not two screens away in the settings.
const CHANGE_VIEW: &str = "Change View";

/// Stop overriding the global view in the exact place on screen.
const USE_GLOBAL_VIEW: &str = "Use Global View";

/// Move down a long list a letter at a time.
const JUMP: &str = "Jump to Letter";

/// Narrow the folder on screen to the titles that match.
const SEARCH: &str = "Search This Folder";

/// Put back everything the search took away.
const CLEAR_SEARCH: &str = "Clear Search";

/// The id the favourites folder is listed under in the table.
const FAVORITES_ID: &str = "Favorites";

/// Take this row out of the list until it is asked for again.
const HIDE_THIS: &str = "Hide This";

/// Read the system being browsed off the card again, leaving every other
/// system's listing as it was.
const REBUILD_SYSTEM: &str = "Rebuild This System List";

/// Put it back.
const SHOW_THIS: &str = "Show This";

/// Keep this game where MiSTer's own favourites live.
const ADD_FAVORITE: &str = "Add to Favourites";

/// Take it out again.
const REMOVE_FAVORITE: &str = "Remove From Favourites";

/// ScreenScraper entry points. These strings are also the action identifiers
/// while their menu is open, so they stay in one place.
const SCRAPE_SYSTEM: &str = "Scrape This System";
const SCRAPE_FOLDER: &str = "Scrape This Folder";
const SCRAPE_GAME: &str = "Scrape This Game";
const GAME_DATA_SOURCE: &str = "Game Data Source";
const CORE_VERSION: &str = "Core Version";
const USE_DEFAULT_CORE_VERSION: &str = "Use Default Core Version";
const SOURCE_GAMELIST: &str = "Gamelist";
const SOURCE_AUTOMATIC: &str = "Automatic";
const SOURCE_ARTWORK_PACK: &str = "Artwork Pack";
const CHOOSE_PACK_DIRECTORY: &str = "Choose Another Directory...";
const USE_PACK_DIRECTORY: &str = "Use This Directory";

fn source_choice_rows(mode: crate::artwork_source::Mode) -> Vec<String> {
    [SOURCE_AUTOMATIC, SOURCE_GAMELIST, SOURCE_ARTWORK_PACK]
        .into_iter()
        .enumerate()
        .map(|(index, label)| {
            if index
                == match mode {
                    crate::artwork_source::Mode::Automatic => 0,
                    crate::artwork_source::Mode::Gamelist => 1,
                    crate::artwork_source::Mode::ArtworkPack => 2,
                }
            {
                format!("{label} (Current)")
            } else {
                label.to_string()
            }
        })
        .collect()
}

fn directory_allowed_for(directory: &Path, game_roots: &[String]) -> bool {
    let canonical = std::fs::canonicalize(directory).unwrap_or_else(|_| directory.to_path_buf());
    if canonical.starts_with("/media") {
        return true;
    }
    game_roots.iter().any(|root| {
        let root = PathBuf::from(root);
        let allowed = root.parent().unwrap_or(root.as_path());
        let allowed = std::fs::canonicalize(allowed).unwrap_or_else(|_| allowed.to_path_buf());
        canonical.starts_with(allowed)
    })
}

fn pack_browser_root_at(media_root: &Path, game_roots: &[String]) -> PathBuf {
    media_root
        .is_dir()
        .then(|| media_root.to_path_buf())
        .or_else(|| {
            game_roots
                .iter()
                .map(Path::new)
                .filter_map(Path::parent)
                .find(|path| path.is_dir())
                .map(Path::to_path_buf)
        })
        .unwrap_or_else(|| media_root.to_path_buf())
}

fn artwork_directory_children(
    directory: &Path,
    game_roots: &[String],
    ancestry: &[PathBuf],
) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut children = Vec::new();
    let mut identities = HashSet::new();
    for entry in std::fs::read_dir(directory)? {
        let Ok(entry) = entry else {
            continue;
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Ok(canonical) = std::fs::canonicalize(&path) else {
            continue;
        };
        if !directory_allowed_for(&canonical, game_roots)
            || ancestry.contains(&canonical)
            || !identities.insert(canonical.clone())
        {
            continue;
        }
        children.push((entry.file_name().to_string_lossy().into_owned(), canonical));
    }
    children.sort_by(|left, right| {
        left.0
            .to_lowercase()
            .cmp(&right.0.to_lowercase())
            .then_with(|| left.0.cmp(&right.0))
    });
    Ok(children)
}

fn pack_folders_at(system_id: &str, docs_root: &Path) -> String {
    let present: Vec<&str> = crate::artwork_pack::expected_folders(system_id)
        .iter()
        .copied()
        .filter(|folder| docs_root.join(folder).join("Artwork").is_dir())
        .collect();
    if present.is_empty() {
        "No Matching Pack".to_string()
    } else {
        present.join(", ")
    }
}

fn pack_location_label(system_id: &str, docs_root: &Path) -> String {
    const CRT_ROOM: usize = 46;
    let prefix = format!("[{}] ", pack_folders_at(system_id, docs_root));
    let room = CRT_ROOM.saturating_sub(prefix.chars().count());
    format!(
        "{prefix}{}",
        shortened_path_tail(&docs_root.to_string_lossy(), room)
    )
}

fn shortened_path_tail(value: &str, room: usize) -> String {
    if value.chars().count() <= room {
        return value.to_string();
    }
    if room <= 3 {
        return ".".repeat(room);
    }
    let kept: String = value
        .chars()
        .rev()
        .take(room - 3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    let kept = kept
        .find('/')
        .filter(|index| *index > 0)
        .map(|index| &kept[index..])
        .unwrap_or(&kept);
    format!("...{kept}")
}

fn artwork_pack_health_message(
    system: &str,
    health: crate::artwork_pack::ProviderHealth,
) -> Option<String> {
    match health {
        crate::artwork_pack::ProviderHealth::Ready => None,
        crate::artwork_pack::ProviderHealth::Degraded => Some(format!(
            "Artwork Pack for {system} is incomplete.\nSome artwork or metadata may be missing."
        )),
        crate::artwork_pack::ProviderHealth::Unavailable => Some(format!(
            "Artwork Pack for {system} is unavailable.\nReconnect its storage or select Gamelist."
        )),
        crate::artwork_pack::ProviderHealth::Invalid => Some(format!(
            "Artwork Pack for {system} is invalid.\nRepair it or select Gamelist."
        )),
    }
}

fn artwork_pack_error_action(error: &crate::error::DegaussError) -> &'static str {
    match error {
        crate::error::DegaussError::Io { .. } => "Check the selected storage and try again.",
        crate::error::DegaussError::Malformed { what, .. } if *what == "game descriptor" => {
            "Repair the game descriptor; see degauss.log."
        }
        crate::error::DegaussError::Malformed { .. } => "Repair the Artwork Pack and try again.",
        crate::error::DegaussError::Unsupported { .. } => {
            "Check degauss.log for details, then try again."
        }
    }
}

fn source_change_failure_message(error: &crate::error::DegaussError) -> String {
    format!(
        "Game data source was not changed.\n{}",
        artwork_pack_error_action(error)
    )
}

fn source_progress_rows(
    progress: &crate::source_cache::Progress,
    compact: bool,
    subject: &str,
) -> Vec<(String, String)> {
    let mut rows = vec![
        (subject.to_string(), progress.current.clone()),
        (
            "Progress".to_string(),
            format!("{} / {}", progress.system, progress.systems),
        ),
    ];
    if !compact {
        rows.extend([
            ("Folders".to_string(), progress.folders.to_string()),
            ("Games".to_string(), progress.games.to_string()),
            (
                "CRC scan".to_string(),
                format!(
                    "{} files, {:.1} MiB",
                    progress.hashed_files,
                    progress.hashed_bytes as f64 / (1024.0 * 1024.0)
                ),
            ),
        ]);
    }
    rows
}

fn persist_source_choice(
    current: &Settings,
    path: &Path,
    group: &str,
    target: &crate::source_cache::Target,
) -> Result<(Settings, &'static str, Option<String>)> {
    let mut changed = current.clone();
    let label = match target {
        crate::source_cache::Target::Gamelist => {
            changed.artwork_pack_roots.remove(group);
            changed.gamelist_sources.insert(group.to_string());
            SOURCE_GAMELIST
        }
        crate::source_cache::Target::ArtworkPack { docs_root } => {
            changed.gamelist_sources.remove(group);
            changed
                .artwork_pack_roots
                .insert(group.to_string(), docs_root.to_string_lossy().into_owned());
            SOURCE_ARTWORK_PACK
        }
    };
    let warning = match changed.save(path)? {
        SaveOutcome::Durable => None,
        SaveOutcome::InstalledWithWarning(warning) => Some(warning.to_string()),
    };
    Ok((changed, label, warning))
}

fn persist_source_mode(
    current: &Settings,
    path: &Path,
    group: &str,
    target: &crate::source_cache::Target,
    automatic: bool,
) -> Result<(Settings, &'static str, Option<String>)> {
    if !automatic {
        return persist_source_choice(current, path, group, target);
    }
    let mut changed = current.clone();
    changed.artwork_pack_roots.remove(group);
    changed.gamelist_sources.remove(group);
    let warning = match changed.save(path)? {
        SaveOutcome::Durable => None,
        SaveOutcome::InstalledWithWarning(warning) => Some(warning.to_string()),
    };
    Ok((changed, SOURCE_AUTOMATIC, warning))
}

fn update_index_summaries(
    index: &mut crate::cache::Index,
    systems: &[FoundSystem],
    caches: &[crate::cache::StagedSystemCache],
) {
    for staged in caches {
        let Some(system) = systems.iter().find(|system| system.def.id == staged.id) else {
            continue;
        };
        index.systems.insert(
            staged.id.clone(),
            staged
                .cache
                .summary(&browse::start_for(&system.to_config())),
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScraperRow {
    Username,
    Password,
    Images,
    ImageType,
    Metadata,
    Start,
    Search,
    ClearLogin,
    Back,
}

const SCRAPER_ROWS: [ScraperRow; 8] = [
    ScraperRow::Username,
    ScraperRow::Password,
    ScraperRow::Images,
    ScraperRow::ImageType,
    ScraperRow::Metadata,
    ScraperRow::Start,
    ScraperRow::ClearLogin,
    ScraperRow::Back,
];
const SCRAPER_GAME_ROWS: [ScraperRow; 9] = [
    ScraperRow::Username,
    ScraperRow::Password,
    ScraperRow::Images,
    ScraperRow::ImageType,
    ScraperRow::Metadata,
    ScraperRow::Start,
    ScraperRow::Search,
    ScraperRow::ClearLogin,
    ScraperRow::Back,
];

fn scraper_rows(scope: &crate::scraper::Scope) -> &'static [ScraperRow] {
    if matches!(scope, crate::scraper::Scope::Game { .. }) {
        &SCRAPER_GAME_ROWS
    } else {
        &SCRAPER_ROWS
    }
}

const SCRAPER_PROGRESS_ROWS: usize = 12;

impl ScraperRow {
    fn label(self) -> &'static str {
        match self {
            Self::Username => "Username",
            Self::Password => "Password",
            Self::Images => "Images",
            Self::ImageType => "Image Type",
            Self::Metadata => "Metadata",
            Self::Start => "Start Scraping",
            Self::Search => "Search Manually",
            Self::ClearLogin => "Clear Saved Login",
            Self::Back => "Back",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScraperField {
    Username,
    Password,
    SearchTerm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScraperKeyboardPage {
    Lower,
    Upper,
    Symbols,
}

impl ScraperKeyboardPage {
    fn next(self) -> Self {
        match self {
            Self::Lower => Self::Upper,
            Self::Upper => Self::Symbols,
            Self::Symbols => Self::Lower,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Lower => "lowercase",
            Self::Upper => "uppercase",
            Self::Symbols => "symbols",
        }
    }
}

const SCRAPER_KEYBOARD_COLUMNS: usize = 9;
const SCRAPER_LOWER: &str = "abcdefghijklmnopqrstuvwxyz0123456789-_.@";
const SCRAPER_UPPER: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_.@";
const SCRAPER_SYMBOLS: &str = "!\"#$%&'()*+,/:;<=>?[\\]^`{|}~";

fn scraper_keyboard_keys(page: ScraperKeyboardPage) -> Vec<(String, char)> {
    let characters = match page {
        ScraperKeyboardPage::Lower => SCRAPER_LOWER,
        ScraperKeyboardPage::Upper => SCRAPER_UPPER,
        ScraperKeyboardPage::Symbols => SCRAPER_SYMBOLS,
    };
    characters
        .chars()
        .map(|character| (character.to_string(), character))
        .chain(std::iter::once(("SP".to_string(), ' ')))
        .collect()
}

fn scraper_draft_display(field: ScraperField, value: &str) -> String {
    match field {
        ScraperField::Username | ScraperField::SearchTerm => value.to_string(),
        ScraperField::Password => "*".repeat(value.chars().count()),
    }
}

fn scraper_search_error(error: &crate::scraper::Error) -> &'static str {
    use crate::scraper::ErrorKind;
    match error.kind {
        ErrorKind::Configuration | ErrorKind::Local => "Search could not start",
        ErrorKind::Authentication => "ScreenScraper rejected the login",
        ErrorKind::Unavailable | ErrorKind::Server => "ScreenScraper is unavailable",
        ErrorKind::RateLimited => "ScreenScraper rate limit reached",
        ErrorKind::DailyQuota => "Daily request allowance exhausted",
        ErrorKind::FailedQuota => "Failed-search allowance exhausted",
        ErrorKind::NotFound => "No Matches",
        ErrorKind::MalformedResponse => "ScreenScraper response was unreadable",
        ErrorKind::Transport => "Network connection failed",
        ErrorKind::Timeout => "ScreenScraper timed out",
        ErrorKind::Cancelled => "Search Cancelled",
    }
}

fn scraper_match_year(matched: &crate::scraper::Match) -> &str {
    matched
        .metadata
        .releasedate
        .as_deref()
        .and_then(|value| value.get(..4))
        .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
        .unwrap_or("")
}

fn scraper_preview_is_current(
    expected_match_id: Option<&str>,
    selected: Option<&crate::scraper::Match>,
    event_match_id: &str,
) -> bool {
    expected_match_id == Some(event_match_id)
        && selected.map(|matched| matched.id.as_str()) == Some(event_match_id)
}

fn scraper_scope_only_artwork_pack(
    scope: &crate::scraper::Scope,
    systems: &[FoundSystem],
    roots: &std::collections::BTreeMap<String, String>,
) -> bool {
    let selected = |system_id: &str| crate::artwork_pack::selected_root(roots, system_id).is_some();
    match scope {
        crate::scraper::Scope::All => {
            let mut candidates = systems
                .iter()
                .filter(|system| !is_favorites(system.category()));
            let Some(first) = candidates.next() else {
                return false;
            };
            selected(&first.def.id) && candidates.all(|system| selected(&system.def.id))
        }
        crate::scraper::Scope::System { system_id, .. }
        | crate::scraper::Scope::Folder { system_id, .. }
        | crate::scraper::Scope::Game { system_id, .. } => selected(system_id),
    }
}

fn needs_manual_scraper_match(
    scope: &crate::scraper::Scope,
    eligible: bool,
    progress: &crate::scraper::Progress,
) -> bool {
    eligible
        && matches!(scope, crate::scraper::Scope::Game { .. })
        && (progress.not_found > 0 || progress.ambiguous > 0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ScraperTerminal {
    Finished,
    Cancelled,
    Failed(String),
}

impl ScraperTerminal {
    fn label(&self) -> &str {
        match self {
            Self::Finished => "Finished",
            Self::Cancelled => "Cancelled",
            Self::Failed(_) => "Stopped with error",
        }
    }
}

/// Choose a fixed picture for the category or system under the cursor.
const CHANGE_CATEGORY_IMAGE: &str = "Change Image";

/// Remove only the copy managed by the picker, revealing the ordinary
/// named image or random-logo behaviour underneath.
const CLEAR_CATEGORY_IMAGE: &str = "Clear Custom Image";

#[derive(Debug, Clone, PartialEq, Eq)]
enum ImageTarget {
    Category(String),
    System { id: String, name: String },
}

impl ImageTarget {
    fn label(&self) -> &str {
        match self {
            Self::Category(name) | Self::System { name, .. } => name,
        }
    }

    fn has_override(&self, logo_dir: &Path) -> bool {
        match self {
            Self::Category(name) => crate::category_images::has_override(logo_dir, name),
            Self::System { id, .. } => crate::category_images::has_system_override(logo_dir, id),
        }
    }

    fn install(&self, logo_dir: &Path, source: &Path) -> Result<PathBuf> {
        match self {
            Self::Category(name) => crate::category_images::install(logo_dir, name, source),
            Self::System { id, .. } => crate::category_images::install_system(logo_dir, id, source),
        }
    }

    fn clear(&self, logo_dir: &Path) -> Result<bool> {
        match self {
            Self::Category(name) => crate::category_images::clear(logo_dir, name),
            Self::System { id, .. } => crate::category_images::clear_system(logo_dir, id),
        }
    }
}

/// What the optional held-X gesture can do to the selected row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FavoriteChange {
    Add,
    Remove,
}

/// Decide the held-X operation without touching the card, so the boundaries
/// that protect folders, other screens and the master Favourites system can
/// be pinned by tests.
fn favorite_change(
    enabled: bool,
    screen: Screen,
    browsing: Browsing,
    in_favorites: bool,
    has_game: bool,
    favorite: bool,
) -> Option<FavoriteChange> {
    if !enabled
        || screen != Screen::Browse
        || browsing != Browsing::Games
        || in_favorites
        || !has_game
    {
        return None;
    }
    Some(if favorite {
        FavoriteChange::Remove
    } else {
        FavoriteChange::Add
    })
}

/// Favourites keeps its familiar heart only when it has no real image.
/// Suppressing an image unconditionally made `Favorites.png` discoverable
/// by the category code but impossible to see in the Details view.
fn logo_or_favorite_heart(category: &str, logo: Option<PathBuf>) -> (Option<PathBuf>, bool) {
    let heart = is_favorites(category) && logo.is_none();
    (logo, heart)
}

fn effective_system_logo(logo_dir: Option<&Path>, system: &FoundSystem) -> Option<PathBuf> {
    logo_dir
        .and_then(|directory| crate::category_images::system_image(directory, &system.def.id))
        .or_else(|| system.logo())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct ContextActions {
    scrape_scope: bool,
    scrape_game: bool,
    image_override: Option<bool>,
    game_data_source: bool,
    core_version: bool,
    core_version_override: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ContextPage {
    Game,
    Find,
    Library,
    Appearance,
}

impl ContextPage {
    const ALL: [Self; 4] = [Self::Game, Self::Find, Self::Library, Self::Appearance];

    fn index(self) -> usize {
        Self::ALL.iter().position(|page| *page == self).unwrap()
    }

    fn label(self) -> &'static str {
        match self {
            Self::Game => "Game",
            Self::Find => "Find",
            Self::Library => "Library",
            Self::Appearance => "Appearance",
        }
    }

    fn help(self) -> &'static str {
        match self {
            Self::Game => "Game information, random choices and favourites.",
            Self::Find => "Jump, search and control the selected item's visibility.",
            Self::Library => "Scraping, core choice, game data and system rebuilding.",
            Self::Appearance => "The current place's view and category or system images.",
        }
    }

    fn contains(self, action: &str) -> bool {
        match self {
            Self::Game => matches!(
                action,
                GAME_INFORMATION | RANDOM | RANDOM_FAVORITE | ADD_FAVORITE | REMOVE_FAVORITE
            ),
            Self::Find => matches!(action, JUMP | SEARCH | CLEAR_SEARCH | HIDE_THIS | SHOW_THIS),
            Self::Library => matches!(
                action,
                SCRAPE_SYSTEM
                    | SCRAPE_FOLDER
                    | SCRAPE_GAME
                    | CORE_VERSION
                    | USE_DEFAULT_CORE_VERSION
                    | GAME_DATA_SOURCE
                    | REBUILD_SYSTEM
            ),
            Self::Appearance => matches!(
                action,
                CHANGE_CATEGORY_IMAGE | CLEAR_CATEGORY_IMAGE | CHANGE_VIEW | USE_GLOBAL_VIEW
            ),
        }
    }
}

fn context_help(action: &str) -> &'static str {
    match action {
        GAME_INFORMATION => "Read the selected game's complete metadata and description.",
        RANDOM => "Pick a game from the open folder using Random Game Behaviour.",
        RANDOM_FAVORITE => {
            "Pick only from favourites under the open folder, using Random Game Behaviour."
        }
        ADD_FAVORITE => "Choose a Favourites folder for the selected game.",
        REMOVE_FAVORITE => "Remove the selected favourite. The original game is not deleted.",
        JUMP => "Jump to the first entry beginning with the chosen letter.",
        SEARCH => "Filter this list by name. Back keeps the search until it is cleared.",
        CLEAR_SEARCH => "Clear the search and show the full current list again.",
        HIDE_THIS => "Hide the selected item from browsing without deleting it.",
        SHOW_THIS => "Remove this item's hidden setting so it is normally visible again.",
        SCRAPE_SYSTEM => "Open scraping settings for the entire selected system.",
        SCRAPE_FOLDER => "Open scraping settings for this folder and its contents.",
        SCRAPE_GAME => "Scrape the selected game or search manually for a different match.",
        CORE_VERSION => "Choose an installed core version for this system, or use Default.",
        USE_DEFAULT_CORE_VERSION => {
            "Remove this system's core override and use the global core preference."
        }
        GAME_DATA_SOURCE => "Choose the image and metadata source for this system.",
        REBUILD_SYSTEM => "Rescan this entire system, including all its folders.",
        CHANGE_CATEGORY_IMAGE => "Choose the image shown for the selected category or system.",
        CLEAR_CATEGORY_IMAGE => {
            "Remove this custom image after confirmation and restore the default image."
        }
        CHANGE_VIEW => "Set the view for this place only. Other folders and systems are unchanged.",
        USE_GLOBAL_VIEW => "Remove this place's custom view and follow the global View setting.",
        _ => "",
    }
}

fn category_image_preview(
    choices: &[crate::category_images::Choice],
    selected: usize,
) -> (Option<PathBuf>, String, bool, bool) {
    choices
        .get(selected)
        .map(|choice| {
            (
                Some(choice.path.clone()),
                choice.label.clone(),
                false,
                false,
            )
        })
        .unwrap_or_else(|| (None, String::new(), false, false))
}

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
    custom_view: bool,
    actions: ContextActions,
) -> Vec<String> {
    let mut groups: Vec<Vec<String>> = Vec::new();

    if favorite.is_some() {
        groups.push(vec![GAME_INFORMATION.to_string()]);
    }
    if browsing == Browsing::Games {
        groups.push(vec![RANDOM.to_string(), RANDOM_FAVORITE.to_string()]);
    }
    // Only over something that can be played. A folder is not a favourite.
    match favorite {
        Some(true) => groups.push(vec![REMOVE_FAVORITE.to_string()]),
        Some(false) => groups.push(vec![ADD_FAVORITE.to_string()]),
        None => {}
    }
    if actions.scrape_scope || actions.scrape_game {
        let mut scrape = Vec::new();
        if actions.scrape_scope {
            scrape.push(
                if browsing == Browsing::Systems {
                    SCRAPE_SYSTEM
                } else {
                    SCRAPE_FOLDER
                }
                .to_string(),
            );
        }
        if actions.scrape_game {
            scrape.push(SCRAPE_GAME.to_string());
        }
        groups.push(scrape);
    }
    if actions.core_version {
        let mut versions = vec![CORE_VERSION.to_string()];
        if actions.core_version_override {
            versions.push(USE_DEFAULT_CORE_VERSION.to_string());
        }
        groups.push(versions);
    }
    if actions.game_data_source {
        groups.push(vec![GAME_DATA_SOURCE.to_string()]);
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
    // Only inside a system: at the levels above there is no one system
    // to read again.
    if browsing == Browsing::Games {
        groups.push(vec![REBUILD_SYSTEM.to_string()]);
    }
    if let Some(has_override) = actions.image_override {
        let mut images = vec![CHANGE_CATEGORY_IMAGE.to_string()];
        if has_override {
            images.push(CLEAR_CATEGORY_IMAGE.to_string());
        }
        groups.push(images);
    }
    let mut views = vec![CHANGE_VIEW.to_string()];
    if custom_view {
        views.push(USE_GLOBAL_VIEW.to_string());
    }
    groups.push(views);

    groups.into_iter().flatten().collect()
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

fn shortened_label(value: &str, room: usize) -> String {
    let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
    if value.chars().count() <= room {
        return value;
    }
    if room <= 3 {
        return ".".repeat(room);
    }
    let kept: String = value.chars().take(room.saturating_sub(3)).collect();
    format!("{}...", kept.trim_end())
}

fn compact_detail_text(details: &browse::Details) -> (String, String) {
    let released = details.released.trim();
    let year = released
        .get(..4)
        .filter(|value| value.bytes().all(|byte| byte.is_ascii_digit()))
        .unwrap_or("");
    let players = details.players.trim();
    let players = if players.is_empty() {
        String::new()
    } else if players
        .chars()
        .all(|ch| ch.is_ascii_digit() || matches!(ch, '-' | '–' | '—' | ' '))
    {
        format!(
            "{players} {}",
            if players == "1" { "Player" } else { "Players" }
        )
    } else {
        players.to_string()
    };
    let summary = match (year.is_empty(), players.is_empty()) {
        (false, false) => format!("{year} · {players}"),
        (false, true) => year.to_string(),
        (true, false) => players,
        (true, true) => String::new(),
    };
    let publisher = if details.publisher.trim().is_empty() {
        String::new()
    } else {
        details.publisher.trim().to_string()
    };
    (summary, publisher)
}

fn game_information(row: &browse::Row) -> String {
    let mut text = row.name.clone();
    let values = [
        ("Genre", row.genre.as_deref().unwrap_or("")),
        ("Publisher", row.details.publisher.as_str()),
        ("Developer", row.details.developer.as_str()),
        ("Released", row.details.released.as_str()),
        ("Players", row.details.players.as_str()),
        ("Language", row.details.lang.as_str()),
    ];
    for (label, value) in values {
        text.push_str(&format!("\n\n{label}: {value}"));
    }
    text.push_str("\n\nDescription\n");
    text.push_str(&row.details.desc);
    text
}

struct ReadingInformation {
    row: browse::Row,
    job: crate::information_job::Job,
    cancelling: bool,
}

enum Discovery {
    Queued(crate::index_job::DiscoveryRequest),
    Running(crate::index_job::DiscoveryJob),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct IndexDisplay {
    current: Option<usize>,
    done: usize,
    total: usize,
    folders: usize,
    games: usize,
    elapsed: u64,
    cancelling: bool,
    discovering: bool,
    folder_revision: u64,
}

#[derive(Clone, Default)]
struct IndexOverview {
    title: String,
    state: String,
    subject: String,
    folder: String,
    done: usize,
    total: usize,
    folders: usize,
    games: usize,
    elapsed: u64,
    determinate: bool,
    problem: String,
    report: String,
}

struct Building {
    /// Indices into `all_systems`, in reverse so the next one is popped.
    left: Vec<usize>,
    done: usize,
    total: usize,
    index: crate::cache::Index,
    /// True when the whole cache was thrown away first, so a system whose
    /// file is still on the card is read again rather than skipped.
    forced: bool,
    current: Option<usize>,
    job: Option<crate::index_job::Job>,
    discovery: Option<Discovery>,
    /// Dispatch only after the current scope has reached the surface.
    awaiting_frame: bool,
    displayed: Option<IndexDisplay>,
    cancelling: bool,
    single: bool,
    folders: usize,
    games: usize,
    folders_done: usize,
    games_done: usize,
    folder: String,
    folder_revision: u64,
    started: Instant,
}

impl Building {
    fn display(&self) -> IndexDisplay {
        IndexDisplay {
            current: self.current,
            done: self.done,
            total: self.total,
            folders: self.folders_done + self.folders,
            games: self.games_done + self.games,
            elapsed: self.started.elapsed().as_secs(),
            cancelling: self.cancelling,
            discovering: self.discovery.is_some(),
            folder_revision: self.folder_revision,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceRecoveryPurpose {
    /// A selected Pack has no complete source-neutral cache yet. Resume the
    /// system entry after the background repair finishes.
    OpenSystem,
    /// The user explicitly chose Rebuild This System List.
    RefreshSystem,
    /// A forced or incomplete-cache pass queued by Rebuild All Systems Lists.
    FullBuild,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SourceOperation {
    /// A choice explicitly made in the Game Data Source screen. Settings are
    /// committed only after every source-group cache is installed.
    Switch,
    /// Recreate a missing, stale or deliberately rebuilt Pack cache without
    /// changing the already selected source.
    Recover(SourceRecoveryPurpose),
}

#[derive(Clone, Copy)]
enum SourceResolutionAction {
    Startup,
    OpenSystem,
    Rebuild(Instant),
    RefreshSystem,
    Automatic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderJobPurpose {
    /// Refresh or open a provider already selected in settings.
    Runtime,
    /// Validate structurally detected locations before offering them to the
    /// user. This never reads or prepares a source cache.
    LocationDiscovery,
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

/// Where picking a letter should land, given each entry's first character,
/// whether it is a folder, and whether it is a favourite.
///
/// Not a scan for the first entry at or after the letter. A folder is
/// listed before every file whatever it is called, so the first letters do
/// not climb steadily from the top of the list to the bottom: they climb
/// through the folders, drop back, and climb again through the files.
/// Looking for the first entry at or after the letter therefore stops in the
/// folders nearly every time, which makes the jump useless in any system
/// whose folder names span the alphabet.
///
/// When favourites have been gathered at the top, their rows sit outside the
/// alphabetical non-favourite body of the list. A jump skips that leading
/// group so it lands where browsing can continue alphabetically.
/// The Favourites system passes `false`: its favourite rows are the body
/// itself and there may be no non-favourite body to use.
///
/// So: an eligible entry that actually begins with the letter, wherever it
/// is; failing that the first eligible file after it, since the files are the
/// body of the list; failing that the first eligible entry after it at all.
fn jump_target(entries: &[(char, bool, bool)], key: char, skip_favorites: bool) -> Option<usize> {
    let eligible = |favorite: bool| !skip_favorites || !favorite;
    entries
        .iter()
        .position(|(first, _, favorite)| *first == key && eligible(*favorite))
        .or_else(|| {
            entries.iter().position(|(first, folder, favorite)| {
                !*folder && *first > key && eligible(*favorite)
            })
        })
        .or_else(|| {
            entries
                .iter()
                .position(|(first, _, favorite)| *first > key && eligible(*favorite))
        })
}

/// Where stepping a letter at a time lands: the first row of the next or
/// previous run of rows sharing a first letter.
///
/// The runs are taken in the order the list shows them, not in an
/// alphabet: folders sort before files, so the first letters climb twice,
/// and a step that sorted them would jump between the two climbs. Past
/// either end the step wraps to the run at the other end, the way a move
/// from the edge of the list already does.
///
/// The caller guarantees a non-empty list and a selection inside it.
fn letter_target(letters: &[char], selected: usize, delta: isize) -> usize {
    let current = letters[selected];
    if delta > 0 {
        (selected + 1..letters.len())
            .find(|&at| letters[at] != current)
            .unwrap_or(0)
    } else {
        // Back over the rest of the current run, then over the whole of
        // the run before it, to land on that run's first row.
        let mut start = selected;
        while start > 0 && letters[start - 1] == current {
            start -= 1;
        }
        let end = if start == 0 {
            letters.len() - 1
        } else {
            start - 1
        };
        let target = letters[end];
        let mut at = end;
        while at > 0 && letters[at - 1] == target {
            at -= 1;
        }
        at
    }
}

/// How far one press of up or down moves.
///
/// One row everywhere, except in a grid it depends on what left and right
/// are doing. While they move the cursor sideways, up and down can step a
/// whole visual row, which is how a grid reads. In every other mode they
/// step one cover at a time, because left and right are spent on the
/// setting and a whole-row step would leave the covers beside the
/// selected one with no key that reaches them.
fn vertical_step(grid: bool, direction_mode: bool, stride: usize) -> isize {
    if grid && !direction_mode {
        1
    } else {
        stride as isize
    }
}

fn temporary_visible(started: Option<Instant>, now: Instant, duration: Duration) -> bool {
    started.is_some_and(|at| now.saturating_duration_since(at) < duration)
}

/// The title and speed use the same position above the bar. Where both are
/// still current, the one caused by the latest input owns that position.
fn transient_badges(
    speed_started: Option<Instant>,
    gallery_started: Option<Instant>,
    now: Instant,
) -> (bool, bool) {
    let speed_visible =
        temporary_visible(speed_started, now, Duration::from_millis(SPEED_BADGE_MS));
    let gallery_visible = temporary_visible(
        gallery_started,
        now,
        Duration::from_millis(GALLERY_TITLE_MS),
    );
    if speed_visible && gallery_visible {
        if gallery_started > speed_started {
            (false, true)
        } else {
            (true, false)
        }
    } else {
        (speed_visible, gallery_visible)
    }
}

/// Whether a browse row carries the favourite mark. Existing layouts keep
/// their established treatment of the Favourites containers, whose heart is
/// shown in the large-art panel. The new layouts have no such panel, so the
/// same containers carry the mark in their row instead.
fn browse_row_favorite(layout: Layout, favorite: bool, favorites_container: bool) -> bool {
    favorite || (favorites_container && matches!(layout, Layout::MultiList | Layout::Gallery))
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

fn scraper_search_title(name: &str) -> String {
    name.split_whitespace().collect::<Vec<_>>().join(" ")
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
fn menu_entries(browsing: Browsing, system: Option<&str>, show_scripts: bool) -> Vec<String> {
    let mut entries = Vec::new();
    if browsing == Browsing::Systems {
        if let Some(name) = system {
            entries.push(format!("Hide {name}"));
        }
    }
    entries.push("Options".to_string());
    if show_scripts {
        entries.push("Scripts".to_string());
    }
    entries.push("Help".to_string());
    entries.push("About".to_string());
    entries.push("Exit to MiSTer".to_string());
    entries
}

/// Which HELP row describes the stick's sideways travel. What that row
/// says depends on the Left and right setting, so it is filled in when
/// the screen is drawn rather than written into the array.
const HELP_LR_ROW: usize = 4;

/// Kept to about forty characters a line: the narrowest screen this runs on
/// is 352 pixels, and anything longer is silently cut off.
const HELP: [&str; 10] = [
    "Everything works with a stick and four",
    "buttons. No keyboard needed.",
    "",
    "Stick up/down: move through the list",
    // HELP_LR_ROW: replaced at draw time by Horizontal::help_line().
    "",
    "",
    "A                open a folder, play a game",
    "B                go back, out of a folder",
    "X (Tab)          Actions",
    "Y (Space)        Menu",
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
        let bar = (height / 15.0).round().clamp(10.0, 22.0);
        // Reserve exactly the title bar that this screen draws.
        let body =
            (height - if show_bar { bar } else { 0.0 } - if chrome_shown { chrome } else { 0.0 })
                .max(16.0);
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
            Layout::MultiList => {
                let target_rows = if body >= 380.0 { 18.0 } else { 12.0 };
                let (row_height, visual_rows) = rows_layout(target_rows);
                Geometry {
                    chrome,
                    bar,
                    row_height,
                    body_font: (row_height * 0.66).floor().max(8.0),
                    small_font: (chrome * 0.5).floor().max(7.0),
                    pad,
                    art_width: 0.0,
                    columns: 2,
                    tile_width: (width / 2.0).floor(),
                    tile_height: row_height,
                    visible: visual_rows * 2,
                    stride: 2,
                    inset_x,
                    inset_y,
                }
            }
            Layout::Gallery => {
                // No caption is reserved under a thumbnail, so substantially
                // more rows fit than in Tiled while every cell remains large
                // enough to identify on the 352x240 framebuffer.
                let target_rows = if body >= 380.0 { 6.0 } else { 4.0 };
                let tile_height = (body / target_rows).floor().max(24.0);
                let columns = ((width / (tile_height * 1.15)).round() as usize).clamp(3, 12);
                let tile_width = (width / columns as f32).floor();
                let grid_rows = (body / tile_height).floor().max(1.0) as usize;
                Geometry {
                    chrome,
                    bar,
                    row_height: tile_height,
                    body_font: (tile_height * 0.16).floor().max(8.0),
                    small_font: (tile_height * 0.15).floor().max(7.0),
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

/// Push a palette into every Slint colour property, and tint the wordmark
/// while at it: the two travel together, or the wordmark would keep a
/// theme's colour after the theme went away.
fn push_palette(ui: &DegaussWindow, palette: &Colors, logo: Option<ConfigColor>, logo_opacity: u8) {
    assert!(logo_opacity <= 100, "logo colour mix must be 0 through 100");
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
    // Tint presence is separate from the percentage: no logo colour means
    // the original artwork regardless of a retained logo_opacity value.
    ui.set_logo_tint_enabled(logo.is_some());
    ui.set_logo_tint(logo.map_or_else(|| slint::Color::from_argb_u8(0, 0, 0, 0), to_slint));
    ui.set_logo_tint_mix(logo_opacity as f32 / 100.0);
}

fn theme_editor_help(mode: EditorMode, selected: usize, custom_source: bool) -> &'static str {
    match mode {
        EditorMode::Browse if selected == 0 => "<> Change   A Change   B Back",
        EditorMode::Browse if selected == 11 => "<> Change   A Edit   B Back",
        EditorMode::Browse if matches!(selected, 12 | 13) => "<> Change   A Change   B Back",
        EditorMode::Browse if selected == 14 => "A Save As   B Back",
        EditorMode::Browse if selected == 15 && custom_source => "A Delete   B Back",
        EditorMode::Browse if matches!(selected, 15 | 16) => "A Cancel   B Back",
        EditorMode::Browse => "A Edit   B Back   X Swap",
        EditorMode::Hex => "A Use B Back X RGB Y Reset",
        EditorMode::Picker => "A Apply   B Cancel   X Hex   Y Reset",
        EditorMode::Swap => "A Swap   B Cancel",
        EditorMode::Name => "A Type B Back X Del Y Clear",
        EditorMode::Discard => "A Choose   B Keep Editing",
        EditorMode::Delete => "A Choose   B Keep Theme",
    }
}

fn theme_editor_visible_items(mode: EditorMode, total: usize, browse_visible: usize) -> usize {
    if mode == EditorMode::Browse {
        browse_visible
    } else {
        // Modal choices and the name keyboard are complete surfaces rather
        // than scrolling option lists. Keeping every cell in the model also
        // keeps their selected index aligned with the grid.
        total.max(1)
    }
}

fn initial_present_mode(
    saved: Option<&str>,
    default: PresentMode,
    explicit: Option<PresentMode>,
) -> PresentMode {
    explicit
        .or_else(|| saved.and_then(PresentMode::parse))
        .unwrap_or(default)
}

fn resolved_font(setting: Option<&str>, configured: &str) -> Font {
    setting
        .and_then(Font::parse)
        .or_else(|| Font::parse(configured))
        .unwrap_or_default()
}

fn effective_theme_font(theme_font: Option<Font>, system_font: Font, user_override: bool) -> Font {
    if user_override {
        system_font
    } else {
        theme_font.unwrap_or(system_font)
    }
}

/// Recover the independent Text choice when reading settings written by the
/// earlier theme-font implementation. It copied a theme default into
/// `settings.font`; equality therefore identifies its automatic write, while
/// a different valid value identifies a later manual Text change. Released
/// settings had no font-bearing themes, so their existing meaning is
/// unchanged.
fn restored_theme_fonts(
    theme_font: Option<Font>,
    saved_font: Option<&str>,
    configured_font: &str,
    saved_override: Option<bool>,
) -> (Font, Font, bool) {
    let configured_font = resolved_font(None, configured_font);
    let saved_font = saved_font.and_then(Font::parse);
    let legacy_automatic_write = saved_override.is_none()
        && theme_font.is_some()
        && saved_font.is_some()
        && saved_font == theme_font;
    let system_font = if legacy_automatic_write {
        configured_font
    } else {
        saved_font.unwrap_or(configured_font)
    };
    let user_override = saved_override.unwrap_or_else(|| {
        theme_font.is_some() && saved_font.is_some() && saved_font != theme_font
    });
    (
        system_font,
        effective_theme_font(theme_font, system_font, user_override),
        user_override,
    )
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
    themes_dir: PathBuf,

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
    /// The system each group was left standing on, by the group's name.
    /// By id, not index: the list is rebuilt per group, and a position
    /// taken in Computer points at a different machine in Console.
    category_system: std::collections::BTreeMap<String, String>,
    /// The rows of the folder being shown.
    here: Vec<browse::Row>,
    game_list: ListState,
    menu_list: ListState,
    context_actions: Vec<String>,
    context_page: Option<ContextPage>,
    context_root_selection: usize,
    context_page_selections: [usize; 4],
    options_page: OptionsPage,
    options_root_list: ListState,
    option_lists: [ListState; 5],
    advanced_list: ListState,
    theme_editor_list: ListState,
    help_list: ListState,
    about_list: ListState,
    scraper_settings_path: PathBuf,
    scraper_settings: crate::scraper::ScraperSettings,
    /// A malformed or unreadable optional scraper file blocks only the
    /// scraper. It is never replaced with defaults behind the user's back.
    scraper_settings_problem: Option<String>,
    scraper_list: ListState,
    scraper_scope: crate::scraper::Scope,
    scraper_return: Screen,
    scraper_matches_return: Screen,
    scraper_keyboard_field: ScraperField,
    scraper_keyboard_page: ScraperKeyboardPage,
    scraper_keyboard_draft: String,
    scraper_keyboard_list: ListState,
    scraper_progress: crate::scraper::Progress,
    scraper_progress_list: ListState,
    scraper_details: bool,
    scraper_refresh_job: Option<crate::index_job::Job>,
    scraper_refresh_id: Option<String>,
    scraper_refresh_folder: String,
    scraper_refresh_folders: usize,
    scraper_refresh_games: usize,
    scraper_job: Option<crate::scraper::Job>,
    /// Manual resolution exists only for one-game scrapes. Batch scopes
    /// never populate or enter these controls.
    scraper_matches: Vec<crate::scraper::Match>,
    scraper_match_list: ListState,
    scraper_search_term: String,
    scraper_search_job: Option<crate::scraper::SearchJob>,
    scraper_search_status: String,
    scraper_preview_job: Option<crate::scraper::PreviewJob>,
    scraper_preview_match_id: Option<String>,
    scraper_preview_image: Option<crate::covers::RgbImage>,
    scraper_preview_caption: String,
    scraper_manual_resolution_eligible: bool,
    scraper_open_matches_after_refresh: bool,
    scraper_pending_terminal: Option<ScraperTerminal>,
    scraper_terminal: Option<ScraperTerminal>,
    scraper_refresh_queue: VecDeque<String>,
    scraper_cache_refresh_active: bool,
    scraper_cancelling: bool,
    /// System whose source is being selected, retained while picker screens
    /// cover the browse context.
    source_system_id: Option<String>,
    source_locations: Vec<PathBuf>,
    source_directory: Option<PathBuf>,
    /// Canonical directories from the picker root to the current directory.
    /// Children resolving to an identity already in this ancestry are hidden,
    /// preventing a symlink from taking the picker around a cycle.
    source_directory_history: Vec<PathBuf>,
    source_directory_entries: Vec<Option<PathBuf>>,
    source_progress: crate::source_cache::Progress,
    source_job: Option<crate::source_cache::Job>,
    source_operation: Option<SourceOperation>,
    /// Source groups whose CRC coverage must follow a global rebuild. Each is
    /// handled by the same cancellable worker as an explicit source change.
    source_recovery_queue: VecDeque<String>,
    source_recovery_warnings: Vec<String>,
    /// A recovery cancelled or failed in this run must not immediately start
    /// again every time the same system is entered.
    source_recovery_suppressed: HashSet<String>,
    source_cancelling: bool,
    /// Read-only Pack provider snapshots are parsed away from the render/input
    /// loop. Requests queued during an index build start as soon as that build
    /// releases the card.
    provider_job: Option<crate::provider_job::Job>,
    provider_job_purpose: ProviderJobPurpose,
    provider_requests: Vec<crate::provider_job::Request>,
    provider_loading: HashSet<String>,
    /// A Pack system entry waiting for its provider snapshot. The source-neutral
    /// cache remains untouched while this read runs.
    provider_pending_open: Option<String>,
    provider_cancelling: bool,

    screen: Screen,
    browsing: Browsing,
    /// The default selected in Options. It is independent of the effective
    /// view of the place currently on screen.
    global_layout: Layout,
    /// The effective view after command-line, custom and global precedence.
    layout: Layout,
    /// A temporary command-line choice used by render, bench and selftest.
    /// It is never written to settings and always wins while present.
    layout_override: Option<Layout>,
    /// What left and right do while browsing.
    horizontal: Horizontal,
    geometry: Geometry,
    width: u32,
    height: u32,

    covers: CoverCache,
    /// Group previews sit on the page background rather than the game surface.
    group_covers: CoverCache,
    /// Gallery thumbnails are decoded to their actual cell size. Keeping
    /// them separate prevents the dense view from filling the large-art
    /// cache with unnecessarily large images.
    gallery_covers: CoverCache,
    ui: DegaussWindow,
    window: Rc<MinimalSoftwareWindow>,
    rows: Rc<VecModel<Row>>,

    speed: usize,
    show_art: bool,
    artwork_scale: ArtworkScale,
    show_stats: bool,
    show_hidden: bool,
    /// Every system found, before hiding is applied. `systems` is the
    /// visible projection of this.
    all_systems: Vec<FoundSystem>,
    /// The full table of known systems, kept so a rebuild can ask the
    /// card again which of them are there.
    table: Vec<SystemDef>,
    /// Where system logos live, for the same second look.
    logo_dir: Option<PathBuf>,
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
    /// Whether a one-second X hold adds or removes the selected favourite.
    hold_x_favorite: bool,
    hold_y_random: bool,
    /// The typeface everything is set in.
    font: Font,
    /// The persistent Text option, independent of a theme's optional default.
    /// A theme without a valid font always resolves back to this value rather
    /// than inheriting whichever theme was selected before it.
    system_font: Font,
    /// The themes the folder held at startup, in the order the Options row
    /// cycles them.
    themes: Vec<Theme>,
    /// Which of them is on, as an index into `themes`. [`None`] is the
    /// standard palette: the `[colors]` block the user configured.
    active_theme: Option<usize>,
    /// Present only while the dedicated editor screen is open.
    theme_editor: Option<ThemeEditor>,
    /// Number of following frames that must cover the whole screen. Theme
    /// changes recolour pixels no property change touches, and a screen or
    /// geometry change can leave a repeated row's old model alive for one
    /// renderer pass. Partial redraws would retain those old pixels.
    pending_complete_repaints: u8,
    random_launches: bool,
    /// Whether folders come after the games rather than before them.
    folders_last: bool,
    /// Where the written-down copy of the card lives.
    cache_dir: PathBuf,
    /// What is known about every system, read once at startup.
    index: Option<crate::cache::Index>,
    /// The open system's folders, when they have been written down.
    system_cache: Option<crate::cache::SystemCache>,
    /// Current read-only Pack snapshot. Present only for a Pack-selected
    /// system, including an unusable snapshot whose health is shown.
    artwork_provider: Option<crate::artwork_pack::Provider>,
    /// Parsed providers retained by stable system identity. Re-entry performs
    /// a cheap source fingerprint check and reuses the parsed TSV snapshot
    /// only while its root, language and Pack files are unchanged.
    artwork_provider_cache: HashMap<String, crate::artwork_pack::Provider>,
    effective_artwork_pack_roots: std::collections::BTreeMap<String, String>,
    artwork_source_errors: std::collections::BTreeMap<String, String>,
    source_resolution: Option<crate::artwork_source::Job>,
    source_resolution_cancelled: bool,
    source_resolution_groups: HashSet<String>,
    source_resolution_action: SourceResolutionAction,
    source_switch_automatic: bool,
    /// Systems whose persisted Pack cache no longer provides complete current
    /// identities for the provider that was just read.
    provider_recovery_needed: HashSet<String>,
    /// A provider/cache pair checked since the last system-entry request.
    /// Consuming this token lets a startup prewarm open immediately; a later
    /// re-entry schedules another worker-side filesystem check.
    provider_validated: HashSet<String>,
    /// Provider-health notices already shown for the current source snapshot.
    /// A changed provider invalidates its group's entries so a new problem is
    /// still reported, while ordinary re-entry does not repeat the same modal.
    pack_health_shown: HashSet<String>,
    /// Set while the card is being read into the cache, a system at a time
    /// so the screen can say how far it has got.
    build: Option<Building>,
    index_return_screen: Screen,
    index_details: bool,
    index_terminal: Option<IndexOverview>,
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
    information: Option<ReadingInformation>,
    menu: Vec<String>,
    scripts_browser: Option<crate::scripts::Browser>,
    scripts_directory: PathBuf,
    scripts_entries: Vec<crate::scripts::Entry>,
    pending_present_switch: bool,
    /// A system whose metadata is to be read after the next frame is drawn.
    opening: Option<usize>,
    /// A system whose cache is to be written again after the next frame
    /// is drawn, so the message saying so is on screen before the read
    /// starts. A press in that one frame can dismiss the message early;
    /// the read still happens.
    refreshing: Option<String>,
    /// A line to show when the running build finishes. The build repaints
    /// its progress every frame and takes the message down when it is
    /// done, so anything that must be read afterwards waits here.
    message_after_build: Option<String>,
    /// True when a group held one system and was stepped straight through.
    skipped_systems: bool,
    show_empty: bool,
    /// Show the group holding cores that are not games.
    show_other: bool,
    /// Show the group holding test and measurement cores.
    show_utility: bool,
    show_unstable: bool,
    /// Show the strip along the bottom.
    show_bar: bool,
    /// Which logo each group is wearing at the moment.
    category_picks: std::collections::BTreeMap<String, PathBuf>,
    /// The files offered by the category/system image picker while it is open.
    category_image_choices: Vec<crate::category_images::Choice>,
    /// The category or system whose image is being chosen.
    category_image_target: Option<ImageTarget>,
    /// When something was last pressed, for deciding the machine is idle.
    last_input: Instant,
    /// The machine's own state for the bar, re-read on a timer.
    status: crate::status::Status,
    /// When the scroll speed last changed. The badge shows for a moment
    /// after, then goes away again: it answers a question only asked while
    /// the speed is being changed.
    speed_shown_at: Option<Instant>,
    /// When Gallery last changed selection. Its title is temporary rather
    /// than taking permanent room from every thumbnail.
    gallery_title_shown_at: Option<Instant>,
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
    /// The full table of known systems, so a rebuild can ask the card
    /// again which of them are there.
    pub table: Vec<SystemDef>,
    /// Where system logos live, for the same second look.
    pub logo_dir: Option<PathBuf>,
    /// Themes live beside the configuration and editor saves return here.
    pub themes_dir: PathBuf,
    /// What the themes folder held, with a line for each file that did
    /// not load.
    pub themes: ThemeSet,
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
        let show_unstable = loaded.settings.show_unstable.unwrap_or(true);
        let show_bar = loaded.settings.show_bar.unwrap_or(true);
        let Loaded {
            config,
            mut settings,
            settings_path,
            systems,
            names,
            table,
            logo_dir,
            themes_dir,
            themes,
        } = loaded;
        let ThemeSet {
            themes,
            problems: mut theme_problems,
        } = themes;
        let scraper_settings_path = crate::scraper::ScraperSettings::path_beside(&settings_path);
        let (scraper_settings, scraper_settings_problem) =
            match crate::scraper::ScraperSettings::load(&scraper_settings_path) {
                Ok(settings) => (settings, None),
                Err(error) => (
                    crate::scraper::ScraperSettings::default(),
                    Some(error.to_string()),
                ),
            };
        migrate_legacy_views(&mut settings, &systems);
        // The saved theme is looked up by name. A name the folder no longer
        // answers to, whatever the reason, falls back to the standard
        // palette with a line saying so: silently picking another theme
        // would repaint the screen in colours nobody chose. The name stays
        // in settings.toml untouched, so putting the file back is enough.
        let active_theme = match settings.theme.as_deref() {
            None => None,
            Some(name) => {
                // Without regard to case: the card's filesystem cannot
                // tell Amber from amber, so the remembered name must not
                // either.
                let found = themes
                    .iter()
                    .position(|theme| theme.name.eq_ignore_ascii_case(name));
                if found.is_none() {
                    // "Did not load" rather than "is missing": the file may
                    // be there and broken, in which case the folder's own
                    // problem line above this one says what is wrong.
                    theme_problems.push(format!("Theme {name} did not load; using standard."));
                }
                found
            }
        };
        let global_layout = settings
            .layout
            .as_deref()
            .and_then(Layout::parse)
            .or_else(|| Layout::parse(&config.app.layout))
            .unwrap_or(Layout::Details);
        let layout = global_layout;
        let horizontal = settings
            .left_right
            .as_deref()
            .and_then(Horizontal::parse)
            .or_else(|| Horizontal::parse(&config.app.left_right))
            .unwrap_or_default();
        let artwork_scale = settings
            .artwork_scale
            .as_deref()
            .and_then(ArtworkScale::parse)
            .unwrap_or_default();
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

        // Applied over the user's configured colours, so the splash is
        // already in the saved theme rather than flashing the base palette
        // for a frame.
        let palette = match active_theme {
            Some(at) => themes[at].file.apply(&config.colors),
            None => config.colors.clone(),
        };
        let logo = active_theme.and_then(|at| themes[at].file.logo);
        let logo_opacity = active_theme
            .map(|at| themes[at].file.effective_logo_opacity())
            .unwrap_or(100);
        push_palette(&ui, &palette, logo, logo_opacity);
        let (system_font, font, restored_override) = restored_theme_fonts(
            active_theme.and_then(|at| themes[at].file.font),
            settings.font.as_deref(),
            &config.app.font,
            settings.theme_font_override,
        );
        if active_theme.is_some() {
            settings.theme_font_override = Some(restored_override);
        }

        let cover_size = config.app.cover_size.max(width.max(height) / 2);
        // Artwork with transparency is composited onto the colour it is
        // actually drawn on, which is the surface behind it.
        let ground = [palette.surface.r, palette.surface.g, palette.surface.b];
        let covers = CoverCache::new(cover_size, config.app.art_cache.max(8), ground);
        let group_covers = CoverCache::new(
            cover_size,
            8,
            [
                palette.background.r,
                palette.background.g,
                palette.background.b,
            ],
        );
        let gallery_geometry = Geometry::compute(
            Layout::Gallery,
            false,
            false,
            show_bar,
            width,
            height,
            &config,
        );
        let gallery_edge = gallery_geometry
            .tile_width
            .max(gallery_geometry.tile_height)
            .ceil()
            .max(1.0) as u32;
        let gallery_covers = CoverCache::new(
            gallery_edge,
            config.app.art_cache.max(gallery_geometry.visible).max(8),
            ground,
        );

        let system_count = systems.len();
        let effective_artwork_pack_roots = settings.artwork_pack_roots.clone();
        let mut app = App {
            speed: settings
                .speed_step
                .unwrap_or(SPEED_START)
                .min(SPEED_STEPS.len() - 1),
            show_art: settings.show_art.unwrap_or(true),
            artwork_scale,
            show_stats: settings.show_stats.unwrap_or(config.app.show_stats),
            show_hidden: settings.show_hidden.unwrap_or(false),
            all_systems: Vec::new(),
            table,
            logo_dir,
            all_here: Vec::new(),
            filter: String::new(),
            find_mode: FindMode::Jump,
            find_list: ListState::new(FIND_CELLS.chars().count(), FIND_CELLS.chars().count()),
            empty_systems: None,
            favorites: crate::favorites::Favorites::default(),
            favorites_first: settings.favorites_first.unwrap_or(true),
            hold_x_favorite: settings.hold_x_favorite.unwrap_or(false),
            hold_y_random: settings.hold_y_random.unwrap_or(false),
            // The file the user edits, then the one Degauss writes, then the
            // typeface that always exists. A name neither of them recognises
            // is not worth refusing to start over.
            font,
            system_font,
            themes,
            active_theme,
            theme_editor: None,
            pending_complete_repaints: 0,
            random_launches: settings.random_launches.unwrap_or(false),
            folders_last: settings.folders_last.unwrap_or(false),
            opened_config: None,
            corrected_counts: HashMap::new(),
            total_games: 0,
            cache_dir: crate::cache::dir_for(&settings_path),
            index: None,
            system_cache: None,
            artwork_provider: None,
            artwork_provider_cache: HashMap::new(),
            effective_artwork_pack_roots,
            artwork_source_errors: std::collections::BTreeMap::new(),
            source_resolution: None,
            source_resolution_cancelled: false,
            source_resolution_groups: HashSet::new(),
            source_resolution_action: SourceResolutionAction::Startup,
            source_switch_automatic: false,
            provider_recovery_needed: HashSet::new(),
            provider_validated: HashSet::new(),
            pack_health_shown: HashSet::new(),
            build: None,
            index_return_screen: Screen::Browse,
            index_details: false,
            index_terminal: None,
            saver_candidates: None,
            config,
            settings,
            settings_path,
            themes_dir,
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
            category_system: std::collections::BTreeMap::new(),
            here: Vec::new(),
            game_list: ListState::new(0, geometry.visible),
            menu_list: ListState::new(0, geometry.visible),
            context_actions: Vec::new(),
            context_page: None,
            context_root_selection: 0,
            context_page_selections: [0; 4],
            options_page: OptionsPage::Navigation,
            options_root_list: ListState::new(OptionsPage::ALL.len(), geometry.visible),
            option_lists: OptionsPage::ALL
                .map(|page| ListState::new(page.ids().len(), geometry.visible)),
            advanced_list: ListState::new(ADVANCED.len(), geometry.visible),
            theme_editor_list: ListState::new(EDITOR_ROWS, geometry.visible),
            about_list: ListState::new(1, geometry.visible),
            help_list: ListState::new(HELP.len(), geometry.visible),
            scraper_settings_path,
            scraper_settings,
            scraper_settings_problem,
            scraper_list: ListState::new(SCRAPER_ROWS.len(), geometry.visible),
            scraper_scope: crate::scraper::Scope::All,
            scraper_return: Screen::Menu,
            scraper_matches_return: Screen::ScraperProgress,
            scraper_keyboard_field: ScraperField::Username,
            scraper_keyboard_page: ScraperKeyboardPage::Lower,
            scraper_keyboard_draft: String::new(),
            scraper_keyboard_list: ListState::new(
                scraper_keyboard_keys(ScraperKeyboardPage::Lower).len(),
                scraper_keyboard_keys(ScraperKeyboardPage::Lower).len(),
            ),
            scraper_progress: crate::scraper::Progress::default(),
            scraper_progress_list: ListState::new(SCRAPER_PROGRESS_ROWS, geometry.visible),
            scraper_details: false,
            scraper_refresh_job: None,
            scraper_refresh_id: None,
            scraper_refresh_folder: String::new(),
            scraper_refresh_folders: 0,
            scraper_refresh_games: 0,
            scraper_job: None,
            scraper_matches: Vec::new(),
            scraper_match_list: ListState::new(0, geometry.visible),
            scraper_search_term: String::new(),
            scraper_search_job: None,
            scraper_search_status: String::new(),
            scraper_preview_job: None,
            scraper_preview_match_id: None,
            scraper_preview_image: None,
            scraper_preview_caption: String::new(),
            scraper_manual_resolution_eligible: false,
            scraper_open_matches_after_refresh: false,
            scraper_pending_terminal: None,
            scraper_terminal: None,
            scraper_refresh_queue: VecDeque::new(),
            scraper_cache_refresh_active: false,
            scraper_cancelling: false,
            source_system_id: None,
            source_locations: Vec::new(),
            source_directory: None,
            source_directory_history: Vec::new(),
            source_directory_entries: Vec::new(),
            source_progress: crate::source_cache::Progress::default(),
            source_job: None,
            source_operation: None,
            source_recovery_queue: VecDeque::new(),
            source_recovery_warnings: Vec::new(),
            source_recovery_suppressed: HashSet::new(),
            source_cancelling: false,
            provider_job: None,
            provider_job_purpose: ProviderJobPurpose::Runtime,
            provider_requests: Vec::new(),
            provider_loading: HashSet::new(),
            provider_pending_open: None,
            provider_cancelling: false,
            screen: Screen::Splash,
            browsing: Browsing::Categories,
            global_layout,
            layout,
            layout_override: None,
            horizontal,
            geometry,
            width,
            height,
            covers,
            group_covers,
            gallery_covers,
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
            information: None,
            menu: Vec::new(),
            scripts_browser: None,
            scripts_directory: PathBuf::new(),
            scripts_entries: Vec::new(),
            show_empty,
            show_other,
            show_utility,
            show_unstable,
            show_bar,
            category_picks: std::collections::BTreeMap::new(),
            category_image_choices: Vec::new(),
            category_image_target: None,
            last_input: Instant::now(),
            status: crate::status::Status::read(),
            speed_shown_at: None,
            gallery_title_shown_at: None,
            saver_pool: Vec::new(),
            saver_queue: Vec::new(),
            saver_offset: 0.0,
            saver_stepped: Instant::now(),
            saver_return: Screen::Browse,
            seed: seed_from_clock(),
            pending_present_switch: false,
            opening: None,
            refreshing: None,
            message_after_build: None,
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
        // A theme file that did not load, or a saved theme that is gone, is
        // said out loud on the first screen. Any press takes it down.
        if !theme_problems.is_empty() {
            app.message = Some(theme_problems.join("\n"));
        }
        app.resolve_artwork_sources(None, SourceResolutionAction::Startup);
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
            | Screen::Scripts
            | Screen::Context
            | Screen::OptionsRoot
            | Screen::Options
            | Screen::Information
            | Screen::Advanced
            | Screen::ThemeEditor
            | Screen::Help
            | Screen::Find
            | Screen::FavoriteFolder
            | Screen::Scraper
            | Screen::ScraperKeyboard
            | Screen::ScraperProgress
            | Screen::CategoryImage
            | Screen::ScraperMatches
            | Screen::GameDataSource
            | Screen::ArtworkPackLocation
            | Screen::ArtworkPackDirectory
            | Screen::SourceProgress => true,
            Screen::Browse => self.show_bar,
            Screen::About | Screen::Splash | Screen::Screensaver => false,
        }
    }

    /// Whether the strip along the top is drawn on the screen showing now.
    ///
    /// Gallery reserves its full height for the dense artwork grid.
    fn chrome_here(&self) -> bool {
        matches!(self.screen.ui_index(), 1 | 4 | 5 | 6)
            || (self.screen == Screen::Browse && self.layout != Layout::Gallery)
    }

    fn plain_screen(&self) -> bool {
        self.screen != Screen::Browse || self.layout == Layout::List
    }

    /// Dismiss the wordmark and start browsing.
    fn leave_splash(&mut self) {
        if self.screen == Screen::Splash {
            self.screen = Screen::Browse;
            self.resolve_view();
            self.apply_geometry();
            self.touch_selection();
        }
    }

    fn apply_geometry(&mut self) {
        let mut geometry = Geometry::compute(
            self.layout,
            self.plain_screen(),
            self.chrome_here(),
            self.bar_here(),
            self.width,
            self.height,
            &self.config,
        );
        if self.screen == Screen::Browse
            && self.browsing == Browsing::Games
            && self.layout == Layout::Details
        {
            // The approved compact preview leaves more room for game titles.
            geometry.art_width *= 0.84;
        }
        let help_height = if matches!(
            self.screen,
            Screen::OptionsRoot
                | Screen::Options
                | Screen::Advanced
                | Screen::Context
                | Screen::Scripts
                | Screen::Scraper
                | Screen::GameDataSource
                | Screen::ArtworkPackLocation
        ) {
            self.font.quantise(geometry.small_font) * 4.2 + geometry.pad * 1.5
        } else {
            0.0
        };
        if help_height > 0.0 {
            geometry.visible = ((geometry.visible as f32 * geometry.row_height - help_height)
                / geometry.row_height)
                .floor()
                .max(1.0) as usize;
        }
        self.ui.set_plain_help_height(help_height);
        self.geometry = geometry;
        let (gallery_edge, gallery_capacity, _) = self.gallery_cache_spec();
        if self.gallery_covers.max_edge() != gallery_edge
            || self.gallery_covers.capacity() != gallery_capacity
        {
            self.gallery_covers = self.fresh_gallery_cover_cache();
        }
        for list in [
            &mut self.category_list,
            &mut self.system_list,
            &mut self.game_list,
            &mut self.menu_list,
            &mut self.options_root_list,
            &mut self.advanced_list,
            &mut self.theme_editor_list,
            &mut self.help_list,
            &mut self.about_list,
            &mut self.scraper_list,
            &mut self.scraper_progress_list,
            &mut self.scraper_match_list,
        ] {
            list.reshape(geometry.visible, 1);
        }
        for list in &mut self.option_lists {
            list.reshape(geometry.visible, 1);
        }
        // Only the browse screen uses a grid.
        if self.screen == Screen::Browse && self.layout.is_grid() {
            self.active_list_mut()
                .reshape(geometry.visible, geometry.stride);
        }

        if self.screen == Screen::ThemeEditor {
            let body = geometry.row_height * geometry.visible as f32;
            self.geometry.visible = EDITOR_ROWS;
            self.geometry.row_height = (body / EDITOR_ROWS as f32).floor().max(9.0);
            self.geometry.body_font = (self.geometry.row_height * 0.58).floor().max(7.0);
            self.theme_editor_list.reshape(EDITOR_ROWS, 1);
        }
        let geometry = self.geometry;

        self.ui.set_screen(self.screen.ui_index());
        self.ui.set_layout(self.layout.index());
        self.ui
            .set_category_image_picker(self.screen == Screen::CategoryImage);
        self.ui
            .set_compact_separators(compact_separators(self.screen));
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
        self.ui
            .set_scraper_match_picker(self.screen == Screen::ScraperMatches);
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
        self.ui
            .set_find_filtering(self.screen == Screen::Find && self.find_mode == FindMode::Search);
        if self.screen == Screen::Find {
            // The grid sizes itself from the screen rather than from the
            // list geometry, which is measured for rows of text.
            let cells = FIND_CELLS.chars().count();
            self.ui.set_columns(FIND_COLUMNS as i32);
            self.ui
                .set_grid_rows(cells.div_ceil(FIND_COLUMNS).max(1) as i32);
            self.ui.set_find_search(self.find_mode != FindMode::Jump);
        } else if self.screen == Screen::ScraperKeyboard {
            let cells = scraper_keyboard_keys(self.scraper_keyboard_page).len();
            self.scraper_keyboard_list
                .reshape(cells, SCRAPER_KEYBOARD_COLUMNS);
            self.ui.set_columns(SCRAPER_KEYBOARD_COLUMNS as i32);
            self.ui
                .set_grid_rows(cells.div_ceil(SCRAPER_KEYBOARD_COLUMNS).max(1) as i32);
            self.ui.set_find_search(false);
        }
        self.ui.set_grid_help(SharedString::from(match self.screen {
            Screen::ScraperKeyboard if self.scraper_keyboard_field == ScraperField::SearchTerm => {
                "A Type  B Search  X Del  Y Page"
            }
            Screen::ScraperKeyboard => "A Type  B Done  X Del  Y Page",
            Screen::Find if self.find_mode == FindMode::NewFolder => "A Type B Done X Del Y Clear",
            Screen::Find if self.find_mode == FindMode::Search => "A Type B Back X Del Y Clear",
            Screen::Find => "A Pick   B Back",
            _ => "",
        }));
        self.ui
            .set_plain_help(SharedString::from(match self.screen {
                Screen::Context | Screen::Scraper | Screen::Options | Screen::Advanced => {
                    self.menu_controls()
                }
                Screen::ScraperProgress => "Up/Down Scroll   B Overview",
                Screen::ScraperMatches if self.scraper_search_job.is_some() => {
                    "Searching   B Cancel"
                }
                Screen::ScraperMatches if self.scraper_matches.is_empty() => "B Back   X Search",
                Screen::ScraperMatches => "A Use   B Back   X Search",
                Screen::GameDataSource | Screen::ArtworkPackLocation if self.width < 480 => {
                    "↑↓ Choose  A Select  B Back"
                }
                Screen::GameDataSource | Screen::ArtworkPackLocation => {
                    "Up/Down Choose   A Select   B Back"
                }
                Screen::ArtworkPackDirectory if self.width < 480 => "↑↓ Choose  A Use  B Parent",
                Screen::ArtworkPackDirectory => "Up/Down Choose   A Open/Use   B Parent",
                Screen::SourceProgress if self.source_cancelling || self.provider_cancelling => {
                    "Stopping safely   Please wait"
                }
                Screen::SourceProgress => "B Cancel",
                Screen::OptionsRoot => "A Open   B Back",
                Screen::Scripts => "A Open/Run   B Parent",
                Screen::Menu | Screen::FavoriteFolder => "A Select   B Back",
                Screen::Help => "↑↓ Read   B Back",
                Screen::Information => "↑↓ Read   ←→ Page   B/X Actions",
                _ => "",
            }));
        // The margin, then the nudge: one side gains what the other gives
        // up, so the picture moves without changing size. Clamped so a
        // nudge larger than the margin cannot push an edge off the screen.
        let shift_x = (self.shift_x() as f32).clamp(-geometry.inset_x, geometry.inset_x);
        let shift_y = (self.shift_y() as f32).clamp(-geometry.inset_y, geometry.inset_y);
        self.ui.set_inset_left(geometry.inset_x + shift_x);
        self.ui.set_inset_right(geometry.inset_x - shift_x);
        self.ui.set_inset_top(geometry.inset_y + shift_y);
        self.ui.set_inset_bottom(geometry.inset_y - shift_y);
        // Reused-buffer damage is correct only while the old and new scene
        // have the same geometry. Menus add chrome and browsing removes it;
        // on the real single-buffer framebuffer, pixels from that removed
        // chrome otherwise survive over the first browse row.
        invalidate_geometry(&mut self.pending_complete_repaints, &mut self.dirty);
    }

    fn active_list(&self) -> &ListState {
        match self.screen {
            Screen::Browse => match self.browsing {
                Browsing::Categories => &self.category_list,
                Browsing::Systems => &self.system_list,
                Browsing::Games => &self.game_list,
            },
            Screen::Splash | Screen::Screensaver => &self.about_list,
            Screen::Menu | Screen::Context | Screen::Scripts => &self.menu_list,
            Screen::OptionsRoot => &self.options_root_list,
            Screen::Options => &self.option_lists[self.options_page.index()],
            Screen::Information => &self.menu_list,
            Screen::Advanced => &self.advanced_list,
            Screen::ThemeEditor => &self.theme_editor_list,
            Screen::Help => &self.help_list,
            Screen::About => &self.about_list,
            Screen::Find => &self.find_list,
            Screen::FavoriteFolder
            | Screen::CategoryImage
            | Screen::GameDataSource
            | Screen::ArtworkPackLocation
            | Screen::ArtworkPackDirectory
            | Screen::SourceProgress => &self.menu_list,
            Screen::Scraper => &self.scraper_list,
            Screen::ScraperKeyboard => &self.scraper_keyboard_list,
            Screen::ScraperProgress => &self.scraper_progress_list,
            Screen::ScraperMatches => &self.scraper_match_list,
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
            Screen::Menu | Screen::Context | Screen::Scripts => &mut self.menu_list,
            Screen::OptionsRoot => &mut self.options_root_list,
            Screen::Options => &mut self.option_lists[self.options_page.index()],
            Screen::Information => &mut self.menu_list,
            Screen::Advanced => &mut self.advanced_list,
            Screen::ThemeEditor => &mut self.theme_editor_list,
            Screen::Help => &mut self.help_list,
            Screen::About => &mut self.about_list,
            Screen::Find => &mut self.find_list,
            Screen::FavoriteFolder
            | Screen::CategoryImage
            | Screen::GameDataSource
            | Screen::ArtworkPackLocation
            | Screen::ArtworkPackDirectory
            | Screen::SourceProgress => &mut self.menu_list,
            Screen::Scraper => &mut self.scraper_list,
            Screen::ScraperKeyboard => &mut self.scraper_keyboard_list,
            Screen::ScraperProgress => &mut self.scraper_progress_list,
            Screen::ScraperMatches => &mut self.scraper_match_list,
        }
    }

    /// Which settings list the current screen is showing.
    fn option_ids(&self) -> &'static [OptionId] {
        if self.screen == Screen::Advanced {
            &ADVANCED
        } else {
            self.options_page.ids()
        }
    }

    fn menu_controls(&self) -> &'static str {
        if self.screen == Screen::Context && self.context_is_root() {
            return "A Open   B Back";
        }
        let adjustable = match self.screen {
            Screen::Options | Screen::Advanced => self
                .option_ids()
                .get(self.active_list().selected())
                .is_some_and(|id| {
                    matches!(
                        option_operation(*id, OptionInput::Next),
                        OptionOperation::Adjust(_)
                    )
                }),
            Screen::Context => self
                .menu
                .get(self.menu_list.selected())
                .is_some_and(|action| matches!(action.as_str(), CHANGE_VIEW | CORE_VERSION)),
            Screen::Scraper => matches!(
                scraper_rows(&self.scraper_scope).get(self.scraper_list.selected()),
                Some(ScraperRow::Images | ScraperRow::ImageType | ScraperRow::Metadata)
            ),
            _ => false,
        };
        if !adjustable {
            "A Select   B Back"
        } else if self.screen == Screen::Scraper {
            if self.width < 480 {
                "←→ Set  A Select  B Back"
            } else {
                "Up/Down Choose   Left/Right Change   A Select   B Back"
            }
        } else {
            "←→ Change   A Select   B Back"
        }
    }

    pub fn open_options_page(&mut self, page: OptionsPage) {
        self.options_page = page;
        self.options_root_list.select(page.index());
        self.screen = Screen::Options;
        self.apply_geometry();
    }

    fn open_information(&mut self) {
        let Some(row) = self.here.get(self.game_list.selected()).filter(|row| {
            self.browsing == Browsing::Games && matches!(row.kind, browse::Kind::Play(_))
        }) else {
            return;
        };
        let mut row = row.clone();
        let request = self.information_request(&row);
        match request.and_then(crate::information_job::start) {
            Ok(job) => {
                row.details.desc = "Reading full description...".to_string();
                self.information = Some(ReadingInformation {
                    row: row.clone(),
                    job,
                    cancelling: false,
                });
            }
            Err(error) => row.details.desc = format!("Could not read description: {error}"),
        }
        self.ui
            .set_information_text(SharedString::from(game_information(&row)));
        self.ui.set_information_offset(0.0);
        self.screen = Screen::Information;
        self.apply_geometry();
        self.touch_selection();
    }

    fn information_request(&self, row: &browse::Row) -> Result<crate::information_job::Request> {
        let browse::Kind::Play(mut launch) = row.kind.clone() else {
            return Err(DegaussError::unsupported("game information", "not a game"));
        };
        let id = if self.in_favorites() {
            let browse::Launch::File(path) = &launch else {
                return Err(DegaussError::unsupported(
                    "game information",
                    "invalid favourite",
                ));
            };
            let reference = crate::favorites::reference_of_with_systems(path, &self.all_systems)
                .ok_or_else(|| {
                    DegaussError::unsupported("game information", "favourite target is unavailable")
                })?;
            let id = owner_of_favorite(&self.all_systems, &reference).ok_or_else(|| {
                DegaussError::unsupported(
                    "game information",
                    "favourite's owning system is unavailable or ambiguous",
                )
            })?;
            launch = match crate::launch::amiga_marker(path) {
                Some((install, title)) => browse::Launch::AmigaVision { install, title },
                None => browse::Launch::File(reference.cache_target),
            };
            id
        } else {
            self.open_system
                .clone()
                .ok_or_else(|| DegaussError::unsupported("game information", "no system is open"))?
        };
        let system = self
            .all_systems
            .iter()
            .find(|system| system.def.id == id)
            .ok_or_else(|| {
                DegaussError::unsupported("game information", "owning system is unavailable")
            })?;
        if let Some(error) = self.source_problem(&id) {
            return Err(DegaussError::unsupported("game data source", error));
        }
        let source = if self.pack_selected(&id) {
            let provider = self
                .artwork_provider_cache
                .get(&id)
                .cloned()
                .or_else(|| {
                    (self.open_system.as_deref() == Some(id.as_str()))
                        .then(|| self.artwork_provider.clone())
                        .flatten()
                })
                .ok_or_else(|| {
                    DegaussError::unsupported(
                        "game information",
                        "Artwork Pack metadata is unavailable; reopen the system",
                    )
                })?;
            crate::information_job::Source::ArtworkPack(provider)
        } else {
            crate::information_job::Source::Gamelist
        };
        Ok(crate::information_job::Request {
            launch,
            config: system.to_config(),
            source,
        })
    }

    fn poll_information(&mut self) {
        let event = self
            .information
            .as_mut()
            .and_then(|reading| reading.job.try_recv());
        let Some(event) = event else {
            return;
        };
        let mut reading = self.information.take().expect("active information read");
        if reading.cancelling || matches!(event, crate::information_job::Event::Cancelled) {
            self.screen = Screen::Context;
            self.apply_geometry();
            return;
        }
        reading.row.details.desc = match event {
            crate::information_job::Event::Ready(description) => description.unwrap_or_default(),
            crate::information_job::Event::Failed(error) => {
                format!("Could not read description: {error}")
            }
            crate::information_job::Event::Cancelled => unreachable!(),
        };
        self.ui
            .set_information_text(SharedString::from(game_information(&reading.row)));
        self.dirty = true;
    }

    fn handle_information(&mut self, action: Action) {
        let limit = self.ui.get_information_max_scroll().max(0.0);
        let current = self.ui.get_information_offset();
        let line = self.geometry.small_font * 2.0;
        let page = (self.height as f32 - self.geometry.chrome - self.geometry.bar) * 0.8;
        let next = match action {
            Action::Up => current - line,
            Action::Down => current + line,
            Action::Slower | Action::PageUp => current - page,
            Action::Faster | Action::PageDown => current + page,
            Action::Home => 0.0,
            Action::End => limit,
            Action::Quit | Action::Context => {
                if let Some(reading) = self.information.as_mut() {
                    reading.job.cancel();
                    reading.cancelling = true;
                    reading.row.details.desc = "Stopping description read...".to_string();
                    self.ui
                        .set_information_text(SharedString::from(game_information(&reading.row)));
                    self.dirty = true;
                    return;
                }
                self.screen = Screen::Context;
                self.apply_geometry();
                return;
            }
            _ => current,
        };
        self.ui.set_information_offset(next.clamp(0.0, limit));
        self.dirty = true;
    }

    fn scroll_message(&mut self, action: Action) -> bool {
        let limit = self.ui.get_overlay_max_scroll().max(0.0);
        if limit == 0.0 {
            return false;
        }
        let current = self.ui.get_overlay_offset();
        let line = self.geometry.small_font * 2.0;
        let page = self.height as f32 * 0.6;
        let next = match action {
            Action::Up => current - line,
            Action::Down => current + line,
            Action::Slower | Action::PageUp => current - page,
            Action::Faster | Action::PageDown => current + page,
            Action::Home => 0.0,
            Action::End => limit,
            _ => return false,
        };
        self.ui.set_overlay_offset(next.clamp(0.0, limit));
        self.window.request_redraw();
        true
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn speed_ms(&self) -> u64 {
        SPEED_STEPS[self.speed.min(SPEED_STEPS.len() - 1)].1
    }

    /// True while a held left or right should repeat: browsing in every
    /// setting except the speed ladder. Direction scrolls like a held
    /// stick, and a held Letter or Page walks on at the same cadence;
    /// only the ladder stays one step per press, because a held repeat
    /// would run the whole ladder off one touch.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    fn horizontal_scrolls(&self) -> bool {
        self.screen == Screen::Browse && self.horizontal != Horizontal::Speed
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
        let now = Instant::now();
        self.settled_since = Some(now);
        self.gallery_title_shown_at =
            if self.screen == Screen::Browse && self.layout == Layout::Gallery {
                Some(now)
            } else {
                None
            };
        self.restart_marquee();
        self.restart_detail_marquee();
        self.dirty = true;
    }

    /// How far up and down move. The decision itself lives in
    /// [`vertical_step`]: whether a grid steps one cover or a whole row
    /// depends on whether left and right are free to reach the covers
    /// either side of the selected one.
    fn browse_step(&self) -> isize {
        vertical_step(
            self.screen == Screen::Browse && self.layout.is_grid(),
            self.horizontal == Horizontal::Direction,
            self.active_list().stride(),
        )
    }

    /// A picture for a group: whichever system lent its logo this time, or
    /// a file named after the group if one was put in the logos folder.
    fn category_logo(&self, category: &str) -> Option<PathBuf> {
        self.category_picks.get(category).cloned()
    }

    /// A picker-managed system image wins over the long-standing explicit
    /// path or `<system id>.png`/`.jpg` conventions. Clearing the managed
    /// copy therefore restores the previous behaviour without touching it.
    fn system_logo(&self, system: &FoundSystem) -> Option<PathBuf> {
        effective_system_logo(self.logo_dir.as_deref(), system)
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
    /// Gamelist systems need a metadata file; Pack systems can provide their
    /// own pictures and therefore remain candidates even with no gamelist.
    fn saver_candidates(&mut self) -> &mut Vec<usize> {
        if self.saver_candidates.is_none() {
            self.saver_candidates = Some(
                (0..self.all_systems.len())
                    .filter(|&index| {
                        let system = &self.all_systems[index];
                        if self.source_problem(&system.def.id).is_some() {
                            return false;
                        }
                        self.pack_selected(&system.def.id)
                            || system
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
            let pack_root =
                crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, &id)
                    .map(Path::to_path_buf);
            let pack_selected = pack_root.is_some();
            let provider = pack_root
                .as_deref()
                .and_then(|root| self.cached_artwork_provider(&id, root, false));
            if pack_selected && provider.is_none() {
                if let Some(group) = crate::artwork_pack::source_group(&id) {
                    self.queue_provider_group(group, false);
                    self.start_provider_job_if_ready();
                }
                break;
            }
            let pack_data = pack_root
                .as_ref()
                .and_then(|_| crate::cache::load_artwork_pack_data(&self.cache_dir, &id));
            if pack_selected && (pack_data.is_none() || self.provider_recovery_needed.contains(&id))
            {
                crate::note(&format!(
                    "screensaver  Pack cache for {name} is not ready, skipped"
                ));
                self.saver_candidates().swap_remove(at);
                continue;
            }
            let cached = pack_data.map(|data| data.cache).or_else(|| {
                (!pack_selected)
                    .then(|| crate::cache::load_system(&self.cache_dir, &id))
                    .flatten()
            });
            let mut found: Vec<(PathBuf, String)> = Vec::new();
            match cached {
                Some(cache) => {
                    for folder in cache.folders.values() {
                        let mut rows = folder.rows.clone();
                        if let Some(provider) = provider.as_ref() {
                            provider.apply_prepared(&mut rows);
                        }
                        for row in &rows {
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
                    let library = Library::open_with_names(&config, self.names.clone());
                    if let Ok(library) = library {
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
        self.remember_system_here();
        let Some(system) = self.systems.get(self.system_list.selected()) else {
            return;
        };
        self.message = Some(format!("Reading {}", system.name()));
        self.opening = Some(self.system_list.selected());
        self.dirty = true;
    }

    fn open_system_now(&mut self) {
        let group = self
            .systems
            .get(self.system_list.selected())
            .and_then(|system| crate::artwork_pack::source_group(&system.def.id));
        if let Some(group) = group {
            self.resolve_artwork_sources(Some(group), SourceResolutionAction::OpenSystem);
        } else if self.source_resolution.is_some() {
            self.source_resolution_action = SourceResolutionAction::OpenSystem;
        } else {
            self.open_system_now_with_provider(None);
        }
    }

    fn open_system_now_with_provider(
        &mut self,
        supplied_provider: Option<crate::artwork_pack::Provider>,
    ) {
        let Some(system) = self.systems.get(self.system_list.selected()) else {
            return;
        };
        let id = system.def.id.clone();
        let config: SystemConfig = system.to_config();
        let name = system.name().to_string();
        if let Some(problem) = self.source_problem(&id) {
            self.message = Some(format!(
                "{name}: game data source could not be resolved\n{problem}"
            ));
            self.dirty = true;
            return;
        }

        // What was written down, if anything. Where a system starts is
        // decided by how it was declared and costs nothing to work out, so
        // a system already written down is opened without being read: a large
        // system's gamelist runs to tens of megabytes and seconds of parsing.
        let pack_root = crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, &id)
            .map(Path::to_path_buf);
        let pack_data = pack_root
            .as_ref()
            .and_then(|_| crate::cache::load_artwork_pack_data(&self.cache_dir, &id));
        if let Some(root) = pack_root.as_deref() {
            let supplied_provider = supplied_provider.filter(|provider| {
                provider.system_id == id
                    && provider
                        .configuration_matches(root, self.scraper_settings.language.as_deref())
            });
            let provider =
                supplied_provider.or_else(|| self.cached_artwork_provider(&id, root, true));
            let Some(provider) = provider else {
                self.wait_for_artwork_provider(&id);
                return;
            };
            self.artwork_provider_cache
                .insert(id.clone(), provider.clone());
            self.artwork_provider = Some(provider);
            let needs_recovery = self.provider_recovery_needed.contains(&id)
                || pack_data.is_none()
                || (self
                    .artwork_provider
                    .as_ref()
                    .is_some_and(|provider| provider.health.usable())
                    && !pack_data
                        .as_ref()
                        .is_some_and(|data| data.fingerprints_complete));
            if needs_recovery && self.begin_source_recovery(&id, SourceRecoveryPurpose::OpenSystem)
            {
                return;
            }
            self.system_cache = pack_data.map(|data| data.cache);
        } else {
            self.system_cache = crate::cache::load_system(&self.cache_dir, &id);
            self.artwork_provider = None;
        }
        if self.screen == Screen::SourceProgress && self.source_job.is_none() {
            self.screen = Screen::Browse;
            self.apply_geometry();
        }
        self.opened_config = Some(config.clone());
        if self.system_cache.is_some() {
            self.library = None;
            self.trail.clear();
            self.open_system = Some(id);
            self.enter(browse::start_for(&config));
            self.show_pack_health_once();
            return;
        }

        if pack_root.is_some() {
            self.library = None;
            self.message = Some(format!(
                "Artwork Pack cache for {name} is unavailable. Reopen the system to retry its background refresh."
            ));
            self.dirty = true;
            return;
        }
        let library = Library::open_with_names(&config, self.names.clone());
        match library {
            Ok(library) => {
                let start = library.start();
                self.library = Some(library);
                self.trail.clear();
                self.open_system = Some(id);
                self.enter(start);
                self.show_pack_health_once();
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

    /// Write down the system the open group is standing on, so coming
    /// back to this group lands there again, whatever was visited in
    /// between: the systems list is one ListState shared by every group,
    /// and its bare index means a different machine in each one.
    fn remember_system_here(&mut self) {
        let Some(category) = self.open_category.clone() else {
            return;
        };
        let Some(system) = self.systems.get(self.system_list.selected()) else {
            return;
        };
        self.category_system.insert(category, system.def.id.clone());
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
        let mut rows = if let Some(folder) = self.system_cache.as_ref().and_then(|c| c.get(place)) {
            let mut rows = folder.rows.clone();
            if !self.show_empty {
                rows.retain(|row| row.below != Some(0));
            }
            rows
        } else {
            self.open_library()?;
            let library = self.library.as_ref().ok_or_else(|| {
                crate::error::DegaussError::unsupported("browse", "no system open".to_string())
            })?;
            library.list(place, self.show_empty)?.0
        };
        if let Some(provider) = self.artwork_provider.as_ref() {
            provider.apply_prepared(&mut rows);
            rows.sort_by(|left, right| {
                right
                    .is_folder()
                    .cmp(&left.is_folder())
                    .then_with(|| left.sort_key.cmp(&right.sort_key))
            });
        }
        if self
            .opened_config
            .as_ref()
            .is_some_and(|config| config.preserve_rbf_stem)
        {
            for row in &mut rows {
                if let browse::Kind::Play(browse::Launch::File(path)) = &row.kind {
                    if path
                        .extension()
                        .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
                    {
                        if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                            row.name = stem.to_string();
                            row.sort_key = stem.to_ascii_lowercase();
                        }
                    }
                }
            }
            rows.sort_by(|left, right| {
                right
                    .is_folder()
                    .cmp(&left.is_folder())
                    .then_with(|| left.sort_key.cmp(&right.sort_key))
            });
        }
        Ok(rows)
    }

    /// Read the open system, for the parts of it nobody wrote down.
    fn open_library(&mut self) -> Result<()> {
        if self.library.is_some() {
            return Ok(());
        }
        let config = self.opened_config.clone().ok_or_else(|| {
            crate::error::DegaussError::unsupported("browse", "no system open".to_string())
        })?;
        if self
            .open_system
            .as_deref()
            .is_some_and(|id| self.pack_selected(id))
        {
            return Err(crate::error::DegaussError::unsupported(
                "Artwork Pack cache",
                "the selected folder is missing from its complete background cache; rebuild this system list",
            ));
        }
        self.library = Some(Library::open_with_names(&config, self.names.clone())?);
        Ok(())
    }

    /// Read MiSTer's favourites folder again.
    ///
    /// Cheap: a couple of hundred small files. Done on the way in and
    /// after anything is favourited, never on a timer.
    fn reread_favorites(&mut self) {
        let root = PathBuf::from(&self.config.menu_root).join(crate::favorites::FAVORITES_DIR);
        self.favorites = crate::favorites::Favorites::read_with_systems(&root, &self.all_systems);
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
    fn refresh_favorites_system(&mut self) -> Option<String> {
        let id = self
            .all_systems
            .iter()
            .find(|system| is_favorites(system.category()))?
            .def
            .id
            .clone();
        self.refresh_system(&id)
    }

    /// Write one system's cache again, from the card as it is now. Only
    /// that system's file and its line in the index change: everything
    /// else written down stays as it was.
    ///
    /// A failure comes back to the caller instead of onto the screen:
    /// every caller redraws with `show_here`, which clears the message
    /// field, so a message set here would be wiped before it was drawn.
    /// The caller shows it after its redraw.
    fn refresh_system(&mut self, id: &str) -> Option<String> {
        // A system is in the table only when its folder existed at
        // discovery. With no folder there is no cache to refresh and
        // nothing is listed, so doing nothing is correct.
        let system = self.all_systems.iter().find(|s| s.def.id == id)?;
        let name = system.name().to_string();
        // The folders the system was found under were fixed at discovery,
        // and a configured folder can have appeared since: a rebuild that
        // reads the card as it is must read all of it. Only the folders
        // are asked again; which menu folder the core sits in is a
        // full-rebuild question.
        let roots: Vec<PathBuf> = self.config.game_roots.iter().map(PathBuf::from).collect();
        let paths = match crate::systems::existing_folders_checked(&system.def, &roots) {
            Ok(paths) => paths,
            Err(error) => return Some(format!("{name}: {error}")),
        };
        if paths.is_empty() {
            // Every folder gone. Said out loud, and the cache is left as
            // it was: what to do about a vanished system is the full
            // rebuild's decision, not this one's.
            return Some(format!("{name}: no folder for it is on the card"));
        }
        let system = {
            let entry = self
                .all_systems
                .iter_mut()
                .find(|s| s.def.id == id)
                .expect("found above");
            entry.paths = paths;
            entry.clone()
        };
        let config = system.to_config();
        // The open system's config decides where its listing starts and
        // what an on-demand read walks; both moved with the folders just
        // asked again. Kept in step, or a rebuilt system whose first
        // folder changed kept showing the old start until it was left
        // and reopened.
        if self.open_system.as_deref() == Some(id) {
            self.opened_config = Some(config.clone());
            self.library = None;
        }
        let pack = self.pack_selected(id);
        if pack {
            return if self.begin_source_recovery(id, SourceRecoveryPurpose::RefreshSystem) {
                None
            } else {
                Some(format!(
                    "{name}: the Artwork Pack cache refresh could not start"
                ))
            };
        }
        let library = match Library::open_with_names(&config, self.names.clone()) {
            Ok(library) => library,
            // Said out loud rather than quietly keeping the stale listing.
            Err(e) => return Some(format!("{name}: {e}")),
        };
        let mut warnings = Vec::new();
        let cache = match crate::cache::build_system_checked(&library, &mut warnings) {
            Ok(cache) => cache,
            Err(error) => return Some(format!("{name}: {error}")),
        };
        // An archive or member the scan left out is said out loud with the
        // rest of the message, and logged like a full build's warning.
        for warning in &mut warnings {
            *warning = format!("{name}: {warning}");
            crate::note(warning);
        }
        let mut next_index = self.index.clone().unwrap_or_default();
        next_index
            .systems
            .insert(id.to_string(), cache.summary(&browse::start_for(&config)));
        match crate::cache::save_system_with_index(&self.cache_dir, id, &cache, &next_index) {
            Ok(installed) => warnings.extend(installed),
            Err(error) => return Some(format!("{name}: {error}")),
        }
        self.index = Some(next_index);
        self.apply_index();
        let error = (!warnings.is_empty()).then(|| warnings.join("\n"));
        if let Some(build) = self.build.as_mut() {
            // A build runs a system per frame with the controls still
            // live, and what it finishes with replaces the index outright.
            // A system refreshed after the build already passed it would
            // be overwritten by the summary the build saw, so the build's
            // copy is told too.
            build
                .index
                .systems
                .insert(id.to_string(), cache.summary(&browse::start_for(&config)));
        }
        if self.open_system.as_deref() == Some(id) {
            // The rows on screen are answered from this while a system is
            // open. Removing a favourite from inside Favourites redraws
            // straight after, and must not redraw from the old copy. When
            // some other system is open its own cache is the one loaded
            // here, and replacing it would be wrong.
            self.system_cache = Some(cache);
            self.artwork_provider = None;
        }
        // Whatever is hidden under this system was counted against the
        // old cache, so the corrected counts are worked out again. Costs
        // nothing when nothing is hidden.
        self.correct_system_counts();
        self.rebuild_system_list();
        error
    }

    /// Mark what is favourited, and gather it if that is wanted.
    ///
    /// The card decides, not the gamelist: a favourite is a file MiSTer's
    /// own script wrote, and its `<favorite>` tag is a different thing that
    /// a scraper may or may not have filled in.
    /// Identify the exact browse place under any menu currently covering it.
    fn current_view_place(&self) -> Option<ViewPlace> {
        match self.browsing {
            Browsing::Categories => Some(ViewPlace::Categories),
            Browsing::Systems => self
                .open_category
                .as_ref()
                .map(|category| ViewPlace::Systems(category.clone())),
            Browsing::Games => Some(ViewPlace::Games {
                system: self.open_system.clone()?,
                place: self.trail.last()?.place.key(),
            }),
        }
    }

    fn context_system_id(&self) -> Option<&str> {
        match self.browsing {
            Browsing::Systems => self
                .systems
                .get(self.system_list.selected())
                .map(|system| system.def.id.as_str()),
            Browsing::Games => self.open_system.as_deref(),
            Browsing::Categories => None,
        }
    }

    fn pack_selected(&self, system_id: &str) -> bool {
        crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, system_id).is_some()
    }

    fn source_problem(&self, id: &str) -> Option<&str> {
        self.artwork_source_errors
            .get(crate::artwork_pack::source_group(id)?)
            .map(String::as_str)
    }

    fn source_label(&self, id: &str) -> String {
        let effective = if self.source_resolution.is_some()
            && crate::artwork_pack::source_group(id)
                .is_some_and(|group| self.source_resolution_groups.contains(group))
        {
            "Checking"
        } else if self.source_problem(id).is_some() {
            "Unresolved"
        } else if self.pack_selected(id) {
            SOURCE_ARTWORK_PACK
        } else {
            SOURCE_GAMELIST
        };
        match crate::artwork_source::mode(&self.settings, id) {
            crate::artwork_source::Mode::Automatic => format!("Automatic: {effective}"),
            crate::artwork_source::Mode::Gamelist => SOURCE_GAMELIST.to_string(),
            crate::artwork_source::Mode::ArtworkPack => SOURCE_ARTWORK_PACK.to_string(),
        }
    }

    fn resolve_artwork_sources(&mut self, group: Option<&str>, action: SourceResolutionAction) {
        if self.source_resolution.is_some() {
            if matches!(action, SourceResolutionAction::OpenSystem) {
                self.source_resolution_action = action;
            }
            return;
        }
        let systems: Vec<_> = self
            .all_systems
            .iter()
            .filter(|system| {
                group.is_none_or(|group| {
                    crate::artwork_pack::source_group(&system.def.id) == Some(group)
                })
            })
            .cloned()
            .collect();
        self.source_resolution_groups = systems
            .iter()
            .filter_map(|system| {
                crate::artwork_pack::source_group(&system.def.id).map(str::to_string)
            })
            .collect();
        let mut settings = self.settings.clone();
        if matches!(action, SourceResolutionAction::Automatic) {
            for group in &self.source_resolution_groups {
                settings.artwork_pack_roots.remove(group);
                settings.gamelist_sources.remove(group);
            }
        }
        match crate::artwork_source::Job::start(systems, settings) {
            Ok(job) => {
                self.source_resolution = Some(job);
                self.source_resolution_cancelled = false;
                self.source_resolution_action = action;
                if !matches!(
                    action,
                    SourceResolutionAction::Startup | SourceResolutionAction::Rebuild(_)
                ) {
                    self.message = Some("Checking game data source...\n\nB Cancel".into());
                }
            }
            Err(error) => {
                if !matches!(action, SourceResolutionAction::Automatic) {
                    for group in &self.source_resolution_groups {
                        self.artwork_source_errors
                            .insert(group.clone(), error.to_string());
                    }
                }
                if matches!(
                    action,
                    SourceResolutionAction::Startup | SourceResolutionAction::Rebuild(_)
                ) {
                    self.build = None;
                    self.ui.set_index_active(false);
                }
                self.message = Some(error.to_string());
            }
        }
        self.dirty = true;
    }

    fn poll_artwork_sources(&mut self) {
        let Some(result) = self
            .source_resolution
            .as_mut()
            .and_then(|job| job.try_recv())
        else {
            return;
        };
        self.source_resolution = None;
        let result = if std::mem::take(&mut self.source_resolution_cancelled) {
            Ok(None)
        } else {
            result
        };
        let action = self.source_resolution_action;
        let resolution = match result {
            Ok(Some(resolution)) => resolution,
            Ok(None) => {
                if !matches!(action, SourceResolutionAction::Automatic) {
                    for group in &self.source_resolution_groups {
                        self.artwork_source_errors.insert(
                            group.clone(),
                            "Game data source check cancelled; reopen the system to retry".into(),
                        );
                    }
                }
                if matches!(
                    action,
                    SourceResolutionAction::Startup | SourceResolutionAction::Rebuild(_)
                ) {
                    self.build = None;
                    self.ui.set_index_active(false);
                }
                self.message = Some("Game data source check cancelled".into());
                self.dirty = true;
                return;
            }
            Err(error) => {
                if !matches!(action, SourceResolutionAction::Automatic) {
                    for group in &self.source_resolution_groups {
                        self.artwork_source_errors
                            .insert(group.clone(), error.to_string());
                    }
                }
                if matches!(
                    action,
                    SourceResolutionAction::Startup | SourceResolutionAction::Rebuild(_)
                ) {
                    self.build = None;
                    self.ui.set_index_active(false);
                }
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };
        if matches!(action, SourceResolutionAction::Automatic) {
            if !resolution.errors.is_empty() {
                self.message = Some(
                    resolution
                        .errors
                        .values()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join("\n"),
                );
                self.dirty = true;
                return;
            }
            let Some(group) = self
                .source_system_id
                .as_deref()
                .and_then(crate::artwork_pack::source_group)
            else {
                return;
            };
            let target = match resolution.roots.get(group) {
                Some(root) => crate::source_cache::Target::ArtworkPack {
                    docs_root: root.into(),
                },
                None => crate::source_cache::Target::Gamelist,
            };
            self.source_switch_automatic = true;
            self.begin_source_switch(target);
            return;
        }
        let groups = std::mem::take(&mut self.source_resolution_groups);
        for group in groups {
            let before = self.effective_artwork_pack_roots.get(&group).cloned();
            let next = resolution.roots.get(&group).cloned();
            if before != next {
                self.invalidate_artwork_provider_group(&group);
                self.saver_candidates = None;
                self.saver_pool.clear();
                self.saver_queue.clear();
            }
            self.effective_artwork_pack_roots.remove(&group);
            self.artwork_source_errors.remove(&group);
            if let Some(root) = next {
                self.effective_artwork_pack_roots
                    .insert(group.clone(), root);
            }
            if let Some(error) = resolution.errors.get(&group) {
                self.artwork_source_errors.insert(group, error.clone());
            }
        }
        self.message = (!resolution.errors.is_empty()).then(|| {
            resolution
                .errors
                .values()
                .cloned()
                .collect::<Vec<_>>()
                .join("\n")
        });
        match action {
            SourceResolutionAction::Startup => {
                let missing_pack_cache = self.all_systems.iter().any(|system| {
                    self.pack_selected(&system.def.id)
                        && !crate::cache::load_artwork_pack_data(&self.cache_dir, &system.def.id)
                            .is_some_and(|data| data.fingerprints_complete)
                });
                if self.build.is_some() || missing_pack_cache {
                    self.build = None;
                    self.start_build(false);
                }
                self.queue_all_selected_providers();
                self.start_provider_job_if_ready();
            }
            SourceResolutionAction::OpenSystem => self.open_system_now_with_provider(None),
            SourceResolutionAction::RefreshSystem => self.rebuild_open_system_resolved(),
            SourceResolutionAction::Rebuild(started) => {
                self.build = None;
                self.start_build(true);
                if let Some(build) = self.build.as_mut() {
                    build.started = started;
                }
            }
            SourceResolutionAction::Automatic => unreachable!(),
        }
        self.dirty = true;
    }

    fn load_selected_system_cache(&self, system_id: &str) -> Option<crate::cache::SystemCache> {
        if self.pack_selected(system_id) {
            crate::cache::load_artwork_pack_system(&self.cache_dir, system_id)
        } else {
            crate::cache::load_system(&self.cache_dir, system_id)
        }
    }

    fn cached_artwork_provider(
        &mut self,
        system_id: &str,
        docs_root: &Path,
        recheck_source: bool,
    ) -> Option<crate::artwork_pack::Provider> {
        let provider = self.artwork_provider_cache.get(system_id).cloned();
        if provider.as_ref().is_some_and(|provider| {
            provider.configuration_matches(docs_root, self.scraper_settings.language.as_deref())
        }) {
            if !recheck_source || self.provider_validated.remove(system_id) {
                return provider;
            }
            // Keep the parsed snapshot available to the worker. Its cheap
            // source recheck and every persisted-ROM stat happen there.
            return None;
        }
        if let Some(group) = crate::artwork_pack::source_group(system_id) {
            self.invalidate_artwork_provider_group(group);
        }
        None
    }

    fn invalidate_artwork_provider_group(&mut self, group: &str) {
        self.invalidate_decoded_pack_group(group);
        if self.artwork_provider.as_ref().is_some_and(|provider| {
            crate::artwork_pack::source_group(&provider.system_id) == Some(group)
        }) {
            self.artwork_provider = None;
        }
        self.artwork_provider_cache
            .retain(|system_id, _| crate::artwork_pack::source_group(system_id) != Some(group));
        self.provider_recovery_needed
            .retain(|system_id| crate::artwork_pack::source_group(system_id) != Some(group));
        self.provider_validated
            .retain(|system_id| crate::artwork_pack::source_group(system_id) != Some(group));
        self.provider_requests
            .retain(|request| crate::artwork_pack::source_group(&request.system_id) != Some(group));
        let prefix = format!("{group}\0");
        self.pack_health_shown
            .retain(|shown| !shown.starts_with(&prefix));
    }

    fn invalidate_provider_ids(&mut self, system_ids: &HashSet<String>) {
        let groups: HashSet<String> = system_ids
            .iter()
            .filter_map(|system_id| {
                crate::artwork_pack::source_group(system_id).map(str::to_string)
            })
            .collect();
        for group in groups {
            self.invalidate_artwork_provider_group(&group);
        }
    }

    fn invalidate_decoded_pack_group(&mut self, group: &str) {
        let mut docs_roots = HashSet::new();
        if let Some(root) = self.effective_artwork_pack_roots.get(group) {
            docs_roots.insert(PathBuf::from(root));
        }
        for provider in self.artwork_provider_cache.values() {
            if crate::artwork_pack::source_group(&provider.system_id) == Some(group) {
                docs_roots.insert(provider.docs_root.clone());
            }
        }
        if let Some(provider) = self.artwork_provider.as_ref() {
            if crate::artwork_pack::source_group(&provider.system_id) == Some(group) {
                docs_roots.insert(provider.docs_root.clone());
            }
        }

        let mut artwork_roots = Vec::new();
        for docs_root in docs_roots {
            for system in &self.all_systems {
                if crate::artwork_pack::source_group(&system.def.id) != Some(group) {
                    continue;
                }
                for folder in crate::artwork_pack::expected_folders(&system.def.id) {
                    let artwork = docs_root.join(folder).join("Artwork");
                    if !artwork_roots.contains(&artwork) {
                        artwork_roots.push(artwork);
                    }
                }
            }
        }
        for root in &artwork_roots {
            self.covers.invalidate_under(root);
            self.gallery_covers.invalidate_under(root);
        }
        self.saver_pool.retain(|picture| {
            !artwork_roots
                .iter()
                .any(|root| picture.path.starts_with(root))
        });
        self.saver_queue.retain(|picture| {
            !artwork_roots
                .iter()
                .any(|root| picture.path.starts_with(root))
        });
        self.saver_candidates = None;
    }

    fn store_provider_snapshots(&mut self, snapshots: Vec<crate::provider_job::Snapshot>) {
        let language = self.scraper_settings.language.clone();
        let changed_groups: HashSet<String> = snapshots
            .iter()
            .filter(|snapshot| snapshot.source_changed)
            .filter(|snapshot| {
                crate::artwork_pack::selected_root(
                    &self.effective_artwork_pack_roots,
                    &snapshot.provider.system_id,
                )
                .is_some_and(|root| {
                    snapshot
                        .provider
                        .configuration_matches(root, language.as_deref())
                })
            })
            .filter_map(|snapshot| {
                crate::artwork_pack::source_group(&snapshot.provider.system_id).map(str::to_string)
            })
            .collect();
        for group in changed_groups {
            self.invalidate_artwork_provider_group(&group);
        }
        for snapshot in snapshots {
            let provider = snapshot.provider;
            self.provider_requests
                .retain(|request| request.system_id != provider.system_id);
            let current_root = crate::artwork_pack::selected_root(
                &self.effective_artwork_pack_roots,
                &provider.system_id,
            );
            if current_root
                .is_some_and(|root| provider.configuration_matches(root, language.as_deref()))
            {
                let system_id = provider.system_id.clone();
                if snapshot.cache_needs_recovery {
                    self.provider_recovery_needed.insert(system_id.clone());
                } else {
                    self.provider_recovery_needed.remove(&system_id);
                }
                self.provider_validated.insert(system_id.clone());
                self.artwork_provider_cache.insert(system_id, provider);
            }
        }
    }

    fn store_source_providers(&mut self, providers: Vec<crate::artwork_pack::Provider>) {
        let language = self.scraper_settings.language.as_deref();
        for provider in providers {
            let current_root = crate::artwork_pack::selected_root(
                &self.effective_artwork_pack_roots,
                &provider.system_id,
            );
            if !current_root.is_some_and(|root| provider.configuration_matches(root, language)) {
                continue;
            }
            let system_id = provider.system_id.clone();
            self.provider_recovery_needed.remove(&system_id);
            self.provider_validated.insert(system_id.clone());
            self.artwork_provider_cache.insert(system_id, provider);
        }
    }

    fn queue_all_selected_providers(&mut self) {
        let mut groups = HashSet::new();
        for system in &self.all_systems {
            let Some(group) = crate::artwork_pack::source_group(&system.def.id) else {
                continue;
            };
            if self.effective_artwork_pack_roots.contains_key(group) {
                groups.insert(group.to_string());
            }
        }
        let mut groups: Vec<String> = groups.into_iter().collect();
        groups.sort();
        for group in groups {
            self.queue_provider_group(&group, false);
        }
    }

    fn queue_provider_group(&mut self, group: &str, first: bool) {
        let Some(root) = self
            .effective_artwork_pack_roots
            .get(group)
            .map(PathBuf::from)
        else {
            return;
        };
        let mut requested: Vec<crate::provider_job::Request> = self
            .all_systems
            .iter()
            .filter(|system| crate::artwork_pack::source_group(&system.def.id) == Some(group))
            .filter(|system| {
                !self.provider_loading.contains(&system.def.id)
                    && !self
                        .provider_requests
                        .iter()
                        .any(|request| request.system_id == system.def.id)
            })
            .map(|system| crate::provider_job::Request {
                system_id: system.def.id.clone(),
                system_name: system.name().to_string(),
                docs_root: root.clone(),
                synopsis_language: self.scraper_settings.language.clone(),
                cache_dir: self.cache_dir.clone(),
                validate_location_only: false,
                cached_provider: self.artwork_provider_cache.get(&system.def.id).cloned(),
            })
            .collect();
        if first {
            let queued = std::mem::take(&mut self.provider_requests);
            let (mut same_group, other_groups): (Vec<_>, Vec<_>) =
                queued.into_iter().partition(|request| {
                    crate::artwork_pack::source_group(&request.system_id) == Some(group)
                });
            same_group.append(&mut requested);
            same_group.extend(other_groups);
            self.provider_requests = same_group;
        } else {
            self.provider_requests.extend(requested);
        }
    }

    fn start_provider_job_if_ready(&mut self) {
        if self.provider_job.is_some()
            || self.provider_requests.is_empty()
            || self.build.is_some()
            || self.source_job.is_some()
        {
            return;
        }
        let requests = std::mem::take(&mut self.provider_requests);
        let ids: HashSet<String> = requests
            .iter()
            .map(|request| request.system_id.clone())
            .collect();
        match crate::provider_job::start(requests) {
            Ok(job) => {
                self.provider_loading.extend(ids);
                self.provider_job = Some(job);
                self.provider_job_purpose = ProviderJobPurpose::Runtime;
                self.provider_cancelling = false;
            }
            Err(error) => {
                self.invalidate_provider_ids(&ids);
                crate::note(&format!("artwork pack provider: {error}"));
                if self.provider_pending_open.take().is_some() {
                    self.screen = Screen::Browse;
                    self.message = Some(format!(
                        "Artwork Pack read failed; system not opened.\n{}",
                        artwork_pack_error_action(&error)
                    ));
                    self.apply_geometry();
                }
                self.dirty = true;
            }
        }
    }

    fn wait_for_artwork_provider(&mut self, system_id: &str) {
        let Some(group) = crate::artwork_pack::source_group(system_id) else {
            return;
        };
        self.provider_pending_open = Some(system_id.to_string());
        self.queue_provider_group(group, true);
        self.source_progress = crate::source_cache::Progress {
            current: "Artwork Pack database".to_string(),
            system: 0,
            systems: self
                .all_systems
                .iter()
                .filter(|system| crate::artwork_pack::source_group(&system.def.id) == Some(group))
                .count(),
            ..Default::default()
        };
        self.show_source_progress();
        self.start_provider_job_if_ready();
    }

    fn artwork_pack_scraper_exclusions(&self) -> HashSet<String> {
        self.all_systems
            .iter()
            .filter(|system| self.pack_selected(&system.def.id))
            .map(|system| system.def.id.clone())
            .collect()
    }

    fn scraper_scope_only_artwork_pack(&self) -> bool {
        scraper_scope_only_artwork_pack(
            &self.scraper_scope,
            &self.all_systems,
            &self.effective_artwork_pack_roots,
        )
    }

    fn show_pack_health_once(&mut self) {
        let Some(provider) = self.artwork_provider.as_ref() else {
            return;
        };
        if provider.health == crate::artwork_pack::ProviderHealth::Ready {
            return;
        }
        let group = crate::artwork_pack::source_group(&provider.system_id)
            .unwrap_or(provider.system_id.as_str());
        let detail = provider
            .diagnostics
            .first()
            .map(String::as_str)
            .unwrap_or("the selected installation could not be read");
        let shown_key = format!(
            "{group}\0{}\0{}\0{detail}",
            provider.docs_root.display(),
            provider.health.label()
        );
        if !self.pack_health_shown.insert(shown_key) {
            return;
        }
        let system = provider.system_id.clone();
        let root = provider.docs_root.display().to_string();
        let health = provider.health;
        let detail = detail.to_string();
        crate::note(&format!(
            "artwork pack {} at {}: {detail}",
            health.label(),
            root
        ));
        self.message = artwork_pack_health_message(&system, health);
        self.dirty = true;
    }

    fn has_custom_view(&self) -> bool {
        self.current_view_place()
            .and_then(|place| place.get(&self.settings.custom_views))
            .is_some()
    }

    /// Resolve from scratch. A missing or unrecognised custom value means the
    /// global default, never whatever view the previous place happened to use.
    fn resolve_view(&mut self) {
        let custom = self
            .current_view_place()
            .and_then(|place| place.get(&self.settings.custom_views))
            .map(str::to_string);
        self.layout = resolved_layout(self.layout_override, custom.as_deref(), self.global_layout);
    }

    /// Create or update the custom view for the exact place on screen.
    fn remember_view(&mut self) {
        let Some(place) = self.current_view_place() else {
            return;
        };
        place.set(
            &mut self.settings.custom_views,
            self.layout.label().to_string(),
        );
    }

    /// Remove only this place's override. Existence, not parseability, is the
    /// criterion so an unknown value can still be removed explicitly.
    fn use_global_view(&mut self) {
        let Some(place) = self.current_view_place() else {
            return;
        };
        place.remove(&mut self.settings.custom_views);
        self.resolve_view();
        if !self.save_settings() {
            self.apply_geometry();
            return;
        }
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }

    /// Step over the blank lines that separate groups in a menu, and the
    /// spacer rows that do the same on the options screen.
    ///
    /// They are there to be read, not chosen. Bounded by the number of
    /// entries, so a list that somehow held nothing else cannot spin.
    fn skip_blank_menu(&mut self, delta: isize) {
        match self.screen {
            Screen::Menu | Screen::Context => {
                for _ in 0..self.menu.len() {
                    let at = self.menu_list.selected();
                    if self.menu.get(at).is_none_or(|entry| !entry.is_empty()) {
                        return;
                    }
                    self.menu_list.move_items(delta);
                }
            }
            Screen::Options | Screen::Advanced => {
                let ids = self.option_ids();
                for _ in 0..ids.len() {
                    let at = self.active_list().selected();
                    if ids.get(at) != Some(&OptionId::Spacer) {
                        return;
                    }
                    self.active_list_mut().move_items(delta);
                }
            }
            _ => {}
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
        if !self.in_favorites() {
            return;
        }
        // Clone only the small ownership/settings inputs so provider reuse can
        // continue through `self` while the pure enrichment path borrows them.
        let systems = self.all_systems.clone();
        let artwork_pack_roots = self.effective_artwork_pack_roots.clone();
        let cache_dir = self.cache_dir.clone();
        let mut missing_groups = HashSet::new();
        for row in rows.iter() {
            let browse::Kind::Play(browse::Launch::File(path)) = &row.kind else {
                continue;
            };
            let Some(reference) = crate::favorites::reference_of_with_systems(path, &systems)
            else {
                continue;
            };
            let Some(id) = owner_of_favorite(&systems, &reference) else {
                continue;
            };
            if crate::artwork_pack::selected_root(&artwork_pack_roots, &id).is_some()
                && !self.artwork_provider_cache.contains_key(&id)
            {
                if let Some(group) = crate::artwork_pack::source_group(&id) {
                    missing_groups.insert(group.to_string());
                }
            }
        }
        for group in missing_groups {
            self.queue_provider_group(&group, false);
        }
        self.start_provider_job_if_ready();
        enrich_favorite_rows(
            rows,
            &systems,
            &artwork_pack_roots,
            &cache_dir,
            |id, _, _| self.artwork_provider_cache.get(id).cloned(),
        );
        if !self.artwork_source_errors.is_empty() {
            for row in rows.iter_mut() {
                let browse::Kind::Play(browse::Launch::File(path)) = &row.kind else {
                    continue;
                };
                let unresolved = crate::favorites::reference_of_with_systems(path, &systems)
                    .and_then(|reference| owner_of_favorite(&systems, &reference))
                    .is_some_and(|id| self.source_problem(&id).is_some());
                if unresolved {
                    row.cover = None;
                    row.details = Default::default();
                    row.genre = None;
                }
            }
        }
    }

    /// Which system's folders a path sits in, deepest first so a system
    /// inside another system's folder answers for its own.
    fn owner_of(&self, path: &Path) -> Option<String> {
        owner_of_path(&self.all_systems, path)
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
        if self.in_favorites() {
            // Everything playable on this shelf is a favourite by
            // definition, but the lookup below is keyed by what a
            // favourite points AT, and these rows are the favourite files
            // themselves: asked the usual way, the shelf showed no hearts.
            for row in rows.iter_mut() {
                if matches!(row.kind, browse::Kind::Play(_)) {
                    row.favorite = true;
                }
            }
        } else {
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
        // Once a system and place exist this is a Games-level destination,
        // even if reading it fails. Resolve before the attempt so an error
        // cannot leave the previous place's view or the wrong browse level.
        self.browsing = Browsing::Games;
        self.resolve_view();
        self.apply_geometry();
        match self.listing(&crumb.place) {
            Ok(mut rows) => {
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
        let mut config = self.opened_config.clone()?;
        if self.in_favorites() {
            if let browse::Launch::File(path) = &game {
                if let Some(reference) =
                    crate::favorites::reference_of_with_systems(path, &self.all_systems)
                {
                    if let Some(id) = owner_of_favorite(&self.all_systems, &reference) {
                        if let Some(owner) =
                            self.all_systems.iter().find(|system| system.def.id == id)
                        {
                            config = owner.to_config();
                        }
                    }
                    if crate::zip::split_member_path(&reference.owner_target).is_some() {
                        if let Err(error) =
                            crate::zip::validate_member_for_launch(&reference.owner_target)
                        {
                            self.message = Some(error.to_string());
                            self.dirty = true;
                            return None;
                        }
                    }
                }
            }
        }
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
        let core_system = self.core_system_id().or_else(|| self.open_system.clone());
        let selected_core = core_system
            .as_ref()
            .and_then(|id| self.settings.core_choices.get(id))
            .map(String::as_str);
        let ra_first = self.settings.core_preference.unwrap_or_default()
            == crate::settings::CorePreference::RetroAchievementsFirst;
        if !self_describing {
            if let Err(error) = crate::core_choices::resolve(
                &config,
                Path::new(&self.config.menu_root),
                selected_core,
                ra_first,
            ) {
                self.message = Some(error.to_string());
                self.dirty = true;
                return None;
            }
        }
        // A gamelist can name a file that was deleted or renamed since it
        // was written. The plan would build anyway and MiSTer would fail
        // after this process had already handed over, so the absence has to
        // become a line on screen here or never.
        if let browse::Launch::File(path) = &game {
            if crate::zip::split_member_path(path).is_some() {
                if let Err(error) = crate::zip::validate_member_for_launch(path) {
                    self.message = Some(error.to_string());
                    self.dirty = true;
                    return None;
                }
            }
        }
        let missing = match &game {
            // A favourite that is really an AmigaVision title points at an
            // installation, not at itself; the file that has to exist is
            // the one the rewritten MGL will mount.
            // An installation is a directory the launch writes into
            // (shared/ags_boot); a plain file under that name would pass an
            // existence check and fail after the hand-over.
            browse::Launch::File(path) => match crate::launch::amiga_marker(path) {
                Some((install, _)) => !install.is_dir(),
                None => crate::zip::split_member_path(path).is_none() && !path.exists(),
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
            browse::Launch::File(path) => crate::launch::plan_with_choice(
                &config,
                path,
                mgl,
                Path::new(&self.config.menu_root),
                ra_first,
                selected_core,
            ),
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
            let Some(cache) = self.load_selected_system_cache(&id) else {
                continue;
            };
            let held = self.count_under(&cache, &start, 0);
            self.corrected_counts.insert(id, held);
        }
    }

    /// Look at the card again for which systems and cores are there.
    ///
    /// Discovery normally happens once, before the interface exists. A
    /// full rebuild exists to pick up whatever changed behind Degauss's
    /// back, and part of what can change is which systems there are at
    /// all, so it walks the same ground startup walked: the menu folders
    /// for cores, then the game folders against the table.
    fn apply_discovered_systems(&mut self, found: Vec<FoundSystem>) {
        let open = self.open_system.clone();
        let open_name = open.as_deref().and_then(|id| {
            self.all_systems
                .iter()
                .find(|s| s.def.id == id)
                .map(|s| s.name().to_string())
        });
        self.all_systems = found;
        // The screensaver's shortlist holds positions into the list that
        // was just replaced, so it is built again on next use.
        self.saver_candidates = None;
        // The system being browsed can be among what vanished. Its trail
        // points into folders nothing can list any more, so browsing
        // walks back to the top rather than failing folder by folder.
        if let (Some(id), Some(name)) = (open.as_deref(), open_name) {
            if !self.all_systems.iter().any(|s| s.def.id == id) {
                self.close_vanished_system(&name);
            }
        }
        self.rebuild_system_list();
    }

    /// Leave the system that just stopped existing, the way walking out
    /// of it would.
    ///
    /// Everything held about it points at folders that are no longer
    /// listed anywhere. Said when the build finishes rather than now:
    /// the build repaints its progress every frame, so a line shown here
    /// would be painted over before anyone read it.
    fn close_vanished_system(&mut self, name: &str) {
        self.open_system = None;
        self.here.clear();
        self.library = None;
        self.system_cache = None;
        self.artwork_provider = None;
        self.opened_config = None;
        self.trail.clear();
        self.skipped_systems = false;
        self.open_category = None;
        self.browsing = Browsing::Categories;
        self.resolve_view();
        self.apply_geometry();
        self.message_after_build = Some(format!("{name} is no longer on the card"));
    }

    /// Begin reading the card into the cache.
    ///
    /// `forced` replaces each system only after a complete successful read.
    /// Without it an existing cache is reused, keeping a second run cheap.
    fn start_build(&mut self, forced: bool) {
        // One at a time. Replacing a build in flight would lose its progress;
        // the existing progress message already explains the active work.
        if self.build.is_some() || self.source_job.is_some() || self.refreshing.is_some() {
            return;
        }
        if self.provider_job.is_some() {
            self.message = Some("Wait for the current Artwork Pack read to finish.".to_string());
            self.dirty = true;
            return;
        }
        self.index_terminal = None;
        self.index_details = false;
        // Keep the last complete caches until their replacement is committed.
        self.index_return_screen = if self.screen == Screen::Splash {
            Screen::Browse
        } else {
            self.screen
        };
        let total = self.all_systems.len();
        self.source_recovery_queue.clear();
        self.source_recovery_warnings.clear();
        let mut queued = HashSet::new();
        for system in &self.all_systems {
            let id = &system.def.id;
            let Some(group) = crate::artwork_pack::source_group(id) else {
                continue;
            };
            if !self.pack_selected(id) || queued.contains(group) {
                continue;
            }
            let complete = crate::cache::load_artwork_pack_data(&self.cache_dir, id)
                .is_some_and(|data| data.fingerprints_complete);
            if forced || !complete || self.provider_recovery_needed.contains(id) {
                queued.insert(group.to_string());
                self.source_recovery_suppressed.remove(group);
                self.source_recovery_queue.push_back(group.to_string());
            }
        }
        let mut left: Vec<usize> = (0..total)
            .filter(|at| self.source_problem(&self.all_systems[*at].def.id).is_none())
            .collect();
        let skipped_sources = total - left.len();
        for problem in self
            .artwork_source_errors
            .values()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.build_warning(format!(
                "Game data source unresolved; system cache preserved: {problem}"
            ));
        }
        // Popped from the end, so reverse to read them in the order they
        // are listed: the name on screen should march forwards.
        left.reverse();
        let mut index = self.index.clone().unwrap_or_default();
        index
            .systems
            .retain(|id, _| self.all_systems.iter().any(|s| &s.def.id == id));
        self.build = Some(Building {
            left,
            done: skipped_sources,
            total,
            index,
            forced,
            current: None,
            job: None,
            discovery: None,
            awaiting_frame: false,
            displayed: None,
            cancelling: false,
            single: false,
            folders: 0,
            games: 0,
            folders_done: 0,
            games_done: 0,
            folder: String::new(),
            folder_revision: 0,
            started: Instant::now(),
        });
        self.pause_index_marquees();
        // Not shown yet: it goes up when the reading starts, which is
        // after the wordmark has had its moment.
        self.dirty = true;
    }

    /// Read one system into the cache, and say so on screen.
    fn index_message(&self, elapsed: u64) -> Option<String> {
        let build = self.build.as_ref()?;
        let name = build
            .current
            .and_then(|index| self.all_systems.get(index))
            .map(|system| system.name())
            .unwrap_or("Preparing");
        let scope = if build.single {
            "Indexing This System"
        } else {
            "Indexing All Systems"
        };
        let activity = if build.cancelling {
            format!("Cancelling safely: {name}")
        } else if self.source_resolution.is_some() {
            "Preparing: checking game data sources".to_string()
        } else if build.discovery.is_some() {
            "Preparing: discovering systems and cores".to_string()
        } else if self.source_job.is_some() {
            format!(
                "Preparing Artwork Pack: {}",
                if self.source_progress.current.is_empty() {
                    name
                } else {
                    &self.source_progress.current
                }
            )
        } else {
            format!("Scanning: {name}")
        };
        Some(format!("{scope}\n\n{activity}\n{}\n{} / {} systems processed\n{} folders   {} games read\n{} seconds elapsed\n\n{}",
            build.folder, build.done, build.total, build.folders_done + build.folders,
            build.games_done + build.games, elapsed,
            self.message_after_build.as_deref().unwrap_or("No problems reported")))
    }

    fn index_overview(&self) -> Option<IndexOverview> {
        let Some(build) = self.build.as_ref() else {
            return self.index_terminal.clone();
        };
        Some(IndexOverview {
            title: if build.single {
                "Index This System"
            } else {
                "Index All Systems"
            }
            .into(),
            state: if build.cancelling {
                "Cancelling"
            } else if build.discovery.is_some() || self.source_resolution.is_some() {
                "Preparing"
            } else {
                "Running"
            }
            .into(),
            subject: self
                .source_job
                .as_ref()
                .filter(|_| !self.source_progress.current.is_empty())
                .map(|_| self.source_progress.current.clone())
                .or_else(|| {
                    build
                        .current
                        .and_then(|at| self.all_systems.get(at))
                        .map(|system| system.name().to_string())
                })
                .unwrap_or_else(|| {
                    if self.source_resolution.is_some() {
                        "Checking Game Data Sources".into()
                    } else {
                        "Discovering systems and cores".into()
                    }
                }),
            folder: build.folder.clone(),
            done: build.done,
            total: build.total,
            folders: build.folders_done + build.folders,
            games: build.games_done + build.games,
            elapsed: build.started.elapsed().as_secs(),
            determinate: !build.single && build.discovery.is_none() && build.total > 0,
            problem: self.message_after_build.clone().unwrap_or_default(),
            report: self
                .index_message(build.started.elapsed().as_secs())
                .unwrap_or_default(),
        })
    }

    fn update_operation_ui(&self) {
        self.ui.set_operation_kind(0);
        self.ui.set_operation_percent(SharedString::default());
        if let Some(view) = self.index_overview() {
            let active = self.build.is_some();
            self.ui.set_operation_kind(1);
            self.ui.set_operation_details(self.index_details);
            self.ui.set_operation_title(view.title.into());
            self.ui.set_operation_state(
                if view.state == "Finished With Problems" {
                    "Problems".to_string()
                } else {
                    view.state
                }
                .into(),
            );
            self.ui.set_operation_subject(view.subject.into());
            self.ui.set_operation_activity(
                if view.folder.is_empty() {
                    String::new()
                } else {
                    format!("Folder: {}", view.folder)
                }
                .into(),
            );
            self.ui.set_operation_determinate(view.determinate);
            self.ui
                .set_operation_fraction(view.done as f32 / view.total.max(1) as f32);
            self.ui.set_operation_progress(
                format!("{} / {} Systems Processed", view.done, view.total).into(),
            );
            self.ui.set_operation_note(
                if view.determinate {
                    "System Progress, Not Time Remaining"
                } else if active {
                    "Reading Folders; Total Not Yet Known"
                } else {
                    "System List Update Finished"
                }
                .into(),
            );
            self.ui
                .set_operation_counts(ModelRc::new(VecModel::from(vec![
                    DetailLine {
                        label: "Folders Read".into(),
                        value: view.folders.to_string().into(),
                    },
                    DetailLine {
                        label: "Games Found".into(),
                        value: view.games.to_string().into(),
                    },
                    DetailLine {
                        label: "Elapsed".into(),
                        value: format!("{}s", view.elapsed).into(),
                    },
                ])));
            self.ui.set_operation_problem(view.problem.into());
            self.ui.set_operation_controls(
                if self.index_details {
                    "Up/Down Scroll   B Overview"
                } else if active {
                    "A Details   B Cancel"
                } else {
                    "A Details   B Back"
                }
                .into(),
            );
            return;
        }
        if self.screen != Screen::ScraperProgress {
            return;
        }
        let progress = &self.scraper_progress;
        let finishing = self.scraper_pending_terminal.is_some();
        let state = if finishing {
            "Refreshing Lists"
        } else if matches!(self.scraper_terminal, Some(ScraperTerminal::Failed(_))) {
            "Failed"
        } else if let Some(terminal) = &self.scraper_terminal {
            terminal.label()
        } else if self.scraper_cancelling {
            "Cancelling"
        } else {
            progress.phase.label()
        };
        let known = progress.total > 0
            && !matches!(
                progress.phase,
                crate::scraper::Phase::Account | crate::scraper::Phase::Enumerating
            );
        let subject = self.scraper_scope_label();
        let mut activity = if finishing && !self.scraper_refresh_folder.is_empty() {
            format!("Folder: {}", self.scraper_refresh_folder)
        } else if !progress.current.is_empty() {
            progress.current.clone()
        } else {
            progress.activity.clone()
        };
        if activity == subject {
            activity.clear();
        }
        let problem = match &self.scraper_terminal {
            Some(ScraperTerminal::Failed(detail)) => detail.clone(),
            _ => progress.last_problem.clone().unwrap_or_default(),
        };
        self.ui.set_operation_kind(2);
        self.ui.set_operation_details(self.scraper_details);
        self.ui.set_operation_title("ScreenScraper".into());
        self.ui.set_operation_state(state.into());
        self.ui.set_operation_subject(subject.into());
        self.ui.set_operation_activity(activity.into());
        self.ui.set_operation_determinate(known && !finishing);
        self.ui.set_operation_fraction(
            (progress.completed as f32 / progress.total.max(1) as f32).min(1.0),
        );
        self.ui.set_operation_progress(
            if finishing {
                format!(
                    "{} Folders   {} Games Read",
                    self.scraper_refresh_folders, self.scraper_refresh_games
                )
            } else {
                format!("{} / {} Games", progress.completed, progress.total)
            }
            .into(),
        );
        self.ui.set_operation_note(
            if finishing {
                "Updating Images and Metadata in the Lists".to_string()
            } else if !known && progress.total == 0 && self.scraper_terminal.is_none() {
                "Total Not Yet Known".to_string()
            } else if progress.deduplicated_aliases > 0 {
                format!("Linked Copies Skipped: {}", progress.deduplicated_aliases)
            } else {
                String::new()
            }
            .into(),
        );
        self.ui
            .set_operation_counts(ModelRc::new(VecModel::from(vec![
                DetailLine {
                    label: "Written".into(),
                    value: progress.updated.to_string().into(),
                },
                DetailLine {
                    label: "Unchanged".into(),
                    value: progress.unchanged.to_string().into(),
                },
                DetailLine {
                    label: "Unresolved".into(),
                    value: (progress.not_found + progress.ambiguous + progress.no_media)
                        .to_string()
                        .into(),
                },
                DetailLine {
                    label: "Failed".into(),
                    value: progress.failed.to_string().into(),
                },
            ])));
        self.ui.set_operation_problem(problem.into());
        self.ui.set_operation_controls(
            if self.scraper_details {
                "Up/Down Scroll   B Overview"
            } else if finishing || self.scraper_cancelling {
                "A Details   Finishing Safely"
            } else if self.scraper_job.is_some() {
                "A Details   B Cancel"
            } else {
                "A Details   B Back"
            }
            .into(),
        );
    }

    fn pause_index_marquees(&mut self) {
        self.marquee.stop();
        self.detail_marquee.stop();
        self.ui.set_marquee_end(false);
        self.ui.set_detail_end(false);
    }

    /// Counters do not change the covered list, artwork or metadata models.
    fn publish_index_progress(&mut self) -> bool {
        let Some(build) = self.build.as_ref() else {
            return false;
        };
        let display = build.display();
        if build.displayed == Some(display) {
            return false;
        }
        self.message = if self.index_details {
            self.index_message(display.elapsed)
        } else {
            None
        };
        self.build.as_mut().expect("active build").displayed = Some(display);
        self.ui.set_index_active(true);
        self.ui.set_overlay_dismissible(false);
        self.ui.set_overlay_offset(0.0);
        self.ui
            .set_index_fraction(display.done as f32 / display.total.max(1) as f32);
        self.ui.set_overlay(SharedString::from(
            self.message.as_deref().unwrap_or_default(),
        ));
        self.update_operation_ui();
        self.window.request_redraw();
        true
    }

    fn index_frame_presented(&mut self) {
        if let Some(build) = self.build.as_mut() {
            build.awaiting_frame = false;
        }
    }

    fn discover_for_build(&mut self) {
        let build = self.build.as_mut().expect("active discovery");
        let discovery = build.discovery.take().expect("discovery phase");
        let result = match discovery {
            Discovery::Queued(_) if build.cancelling => Some(Ok(None)),
            Discovery::Queued(request) if build.awaiting_frame => {
                build.discovery = Some(Discovery::Queued(request));
                self.publish_index_progress();
                return;
            }
            Discovery::Queued(request) => match crate::index_job::DiscoveryJob::start(request) {
                Ok(job) => {
                    build.discovery = Some(Discovery::Running(job));
                    return;
                }
                Err(error) => Some(Err(error)),
            },
            Discovery::Running(mut job) => {
                let result = job.try_recv();
                if result.is_none() {
                    build.discovery = Some(Discovery::Running(job));
                    self.publish_index_progress();
                    return;
                }
                result
            }
        };
        let preparation = self.build.take().expect("active discovery");
        match result.expect("completed discovery") {
            _ if preparation.cancelling => {
                self.message = Some("Indexing cancelled during preparation".to_string());
            }
            Ok(None) => {
                self.message = Some("Indexing cancelled during preparation".to_string());
            }
            Err(error) => self.message = Some(error.to_string()),
            Ok(Some(found)) => {
                self.apply_discovered_systems(found);
                let started = preparation.started;
                self.build = Some(preparation);
                self.resolve_artwork_sources(None, SourceResolutionAction::Rebuild(started));
                return;
            }
        }
        if let Some(problem) = self.message.take() {
            let cancelled =
                preparation.cancelling || problem == "Indexing cancelled during preparation";
            let report = format!(
                "{problem}\n{} seconds elapsed\nNo system lists were replaced.",
                preparation.started.elapsed().as_secs()
            );
            self.index_terminal = Some(IndexOverview {
                title: "Index All Systems".into(),
                state: if cancelled { "Cancelled" } else { "Failed" }.into(),
                subject: "System Discovery".into(),
                elapsed: preparation.started.elapsed().as_secs(),
                problem: if cancelled { String::new() } else { problem },
                report: report.clone(),
                ..IndexOverview::default()
            });
            if self.index_details {
                self.message = Some(report);
            }
        }
        self.ui.set_index_active(false);
        self.last_input = Instant::now();
        self.touch_selection();
    }

    fn build_warning(&mut self, message: String) {
        crate::note(&message);
        self.message_after_build = Some(match self.message_after_build.take() {
            Some(previous) => format!("{previous}\n{message}"),
            None => message,
        });
    }

    fn build_one_system(&mut self) {
        self.poll_artwork_sources();
        if self.source_resolution.is_some() {
            return;
        }
        self.poll_source_cache();
        if self.source_job.is_some() {
            self.publish_index_progress();
            return;
        }
        if self
            .build
            .as_ref()
            .is_some_and(|build| build.discovery.is_some())
        {
            self.discover_for_build();
            return;
        }
        let event = self
            .build
            .as_mut()
            .and_then(|build| build.job.as_mut())
            .and_then(crate::index_job::Job::try_recv);
        if let Some(event) = event {
            match event {
                crate::index_job::Event::Progress {
                    folders,
                    games,
                    folder,
                } => {
                    let build = self.build.as_mut().expect("active build");
                    build.folders = folders;
                    build.games = games;
                    if build.folder != folder {
                        build.folder = folder;
                        build.folder_revision += 1;
                    }
                }
                crate::index_job::Event::Ready {
                    index,
                    cache,
                    warnings,
                    folders,
                    games,
                    elapsed,
                    summary,
                } => {
                    let build = self.build.as_mut().expect("active build");
                    build.index = index;
                    build.done += 1;
                    build.job = None;
                    let current = build.current.take();
                    build.folders_done += folders;
                    build.games_done += games;
                    build.folders = 0;
                    build.games = 0;
                    if let Some(cache) = cache {
                        self.system_cache = Some(cache);
                        self.library = None;
                    }
                    let name = current
                        .and_then(|at| self.all_systems.get(at))
                        .map(|system| {
                            crate::note(&format!(
                                "index {}: {:.3}s, {} games",
                                system.def.id,
                                elapsed.as_secs_f64(),
                                summary.map_or(0, |value| value.games)
                            ));
                            system.name().to_string()
                        });
                    // Named the way a failure is, so the report says which
                    // system's archive was left out.
                    for warning in warnings {
                        self.build_warning(match &name {
                            Some(name) => format!("{name}: {warning}"),
                            None => warning,
                        });
                    }
                }
                crate::index_job::Event::Cancelled { index } => {
                    let build = self.build.as_mut().expect("active build");
                    build.index = index;
                    build.job = None;
                    build.current = None;
                    build.cancelling = true;
                }
                crate::index_job::Event::Failed { error, index } => {
                    let build = self.build.as_mut().expect("active build");
                    build.index = index;
                    build.job = None;
                    let current = build.current.take();
                    build.done += 1;
                    build.folders_done += build.folders;
                    build.games_done += build.games;
                    build.folders = 0;
                    build.games = 0;
                    let name = current
                        .and_then(|at| self.all_systems.get(at))
                        .map(|system| system.name())
                        .unwrap_or("System");
                    self.build_warning(format!("{name}: {error}"));
                }
            }
            self.publish_index_progress();
            if self.build.as_ref().is_some_and(|build| build.job.is_some()) {
                return;
            }
        }

        let Some(build) = self.build.as_mut() else {
            return;
        };
        if build.job.is_some() {
            self.publish_index_progress();
            return;
        }
        if build.cancelling {
            build.left.clear();
            build.current = None;
            self.source_recovery_queue.clear();
        }
        if let Some(current) = build.current {
            if build.awaiting_frame {
                self.publish_index_progress();
                return;
            }
            let system = &self.all_systems[current];
            let id = system.def.id.clone();
            if let Some(group) = crate::artwork_pack::source_group(&id) {
                if let Some(at) = self
                    .source_recovery_queue
                    .iter()
                    .position(|queued| queued == group)
                {
                    self.source_recovery_queue.remove(at);
                    if !self.begin_source_recovery(&id, SourceRecoveryPurpose::FullBuild) {
                        let cause = self.message.take().unwrap_or_else(|| {
                            "Artwork Pack cache preparation could not start".into()
                        });
                        self.build_warning(format!("{id}: {cause}"));
                    }
                    return;
                }
            }
            let request = crate::index_job::Request {
                config: system.to_config(),
                names: self.names.clone(),
                cache_dir: self.cache_dir.clone(),
                forced: build.forced,
                artwork_pack: crate::artwork_pack::selected_root(
                    &self.effective_artwork_pack_roots,
                    &id,
                )
                .is_some(),
                retain_cache: self.open_system.as_deref() == Some(id.as_str()),
                id,
                index: std::mem::take(&mut build.index),
            };
            match crate::index_job::start(request) {
                Ok(job) => build.job = Some(job),
                Err(failure) => {
                    build.index = failure.index;
                    build.current = None;
                    build.done += 1;
                    self.build_warning(failure.error.to_string());
                }
            }
            return;
        }
        if let Some(index) = build.left.pop() {
            build.current = Some(index);
            build.folders = 0;
            build.games = 0;
            build.folder.clear();
            build.folder_revision += 1;
            build.awaiting_frame = true;
            self.publish_index_progress();
            return;
        }

        let mut overview = self.index_overview().expect("active build");
        let finished = self.build.take().expect("active build");
        if let Err(error) = crate::cache::save_index(&self.cache_dir, &finished.index) {
            self.build_warning(format!("Cache index not written: {error}"));
        }
        self.index = Some(finished.index);
        self.apply_index();
        self.correct_system_counts();
        self.rebuild_system_list();
        if self.open_system.is_some() && self.browsing == Browsing::Games {
            self.relist_here();
        }
        let result = format!(
            "Indexing {}\n{} / {} systems processed in {:.1}s",
            if finished.cancelling {
                "cancelled"
            } else {
                "finished"
            },
            finished.done,
            finished.total,
            finished.started.elapsed().as_secs_f64()
        );
        crate::note(&result.replace('\n', "   "));
        if !self.source_recovery_queue.is_empty() {
            if let Some(warnings) = &self.message_after_build {
                self.source_recovery_warnings.push(warnings.clone());
            }
        }
        overview.state = if finished.cancelling {
            "Cancelled"
        } else if self.message_after_build.is_some() {
            "Finished With Problems"
        } else {
            "Complete"
        }
        .into();
        overview.subject = if finished.single {
            self.open_system
                .as_deref()
                .and_then(|id| self.all_systems.iter().find(|system| system.def.id == id))
                .map(|system| system.name().to_string())
                .unwrap_or_else(|| "System List".into())
        } else {
            "All Systems".into()
        };
        overview.folder.clear();
        overview.problem = self.message_after_build.take().unwrap_or_default();
        overview.report = format!(
            "{result}\n{} folders   {} games read\n\n{}",
            overview.folders,
            overview.games,
            if overview.problem.is_empty() {
                "No problems reported"
            } else {
                &overview.problem
            }
        );
        overview.determinate = finished.total > 0;
        self.message = None;
        if finished.forced || finished.cancelling || !overview.problem.is_empty() {
            if self.index_details {
                self.message = Some(overview.report.clone());
            }
            self.index_terminal = Some(overview);
        }
        self.last_input = Instant::now();
        self.ui.set_index_active(false);
        if !finished.cancelling && self.start_next_source_recovery() {
            self.index_terminal = None;
            self.message = None;
            self.dirty = true;
            return;
        }
        self.touch_selection();
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
        const ORDER: [&str; 7] = [
            "Arcade",
            "Console",
            "Computer",
            "Utility",
            "Other",
            "Unstable",
            FAVORITES_ID,
        ];
        let mut categories: Vec<(String, usize)> = Vec::new();
        for name in ORDER {
            // Other holds the cores that are not games, which is not what
            // anyone opened a game browser for. It is one switch away.
            if name == "Other" && !self.show_other {
                continue;
            }
            if name == "Unstable" && !self.show_unstable {
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
                .filter_map(|system| self.system_logo(system))
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
        crate::category_images::fixed_image(self.logo_dir.as_deref()?, name)
    }

    /// Open a group, showing the systems inside it.
    fn open_selected_category(&mut self) {
        let Some((name, _)) = self.categories.get(self.category_list.selected()) else {
            return;
        };
        let name = name.clone();
        self.open_category = Some(name.clone());
        self.rebuild_system_list();
        // Back to the system this group was left on, found by id in the
        // freshly filtered list; a group never left keeps the clamped
        // index the rebuild produced, exactly as before.
        if let Some(id) = self.category_system.get(name.as_str()) {
            if let Some(at) = self.systems.iter().position(|s| &s.def.id == id) {
                self.system_list.select(at);
            }
        }
        self.browsing = Browsing::Systems;
        self.resolve_view();

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

    /// The palette on screen: the active theme laid over the `[colors]`
    /// the user configured. Always the base underneath, never the previous
    /// theme, so cycling forward and back lands exactly where it started.
    fn effective_palette(&self) -> Colors {
        match self.active_theme {
            Some(at) => self.themes[at].file.apply(&self.config.colors),
            None => self.config.colors.clone(),
        }
    }

    /// Where the Theme row stands in its ring: standard first, then the
    /// themes in folder order.
    fn theme_position(&self) -> usize {
        self.active_theme.map_or(0, |at| at + 1)
    }

    fn effective_logo(&self) -> Option<ConfigColor> {
        self.active_theme.and_then(|at| self.themes[at].file.logo)
    }

    fn effective_logo_opacity(&self) -> u8 {
        self.active_theme
            .map(|at| self.themes[at].file.effective_logo_opacity())
            .unwrap_or(100)
    }

    fn effective_font(&self) -> Font {
        effective_theme_font(
            self.active_theme.and_then(|at| self.themes[at].file.font),
            self.system_font,
            self.active_theme.is_some() && self.settings.theme_font_override.unwrap_or(false),
        )
    }

    /// A cover cache with nothing in it, composited against the surface
    /// colour that is actually on screen.
    fn fresh_cover_cache(&self) -> CoverCache {
        let palette = self.effective_palette();
        CoverCache::new(
            self.config
                .app
                .cover_size
                .max(self.width.max(self.height) / 2),
            self.config.app.art_cache.max(8),
            [palette.surface.r, palette.surface.g, palette.surface.b],
        )
    }

    fn fresh_group_cover_cache(&self) -> CoverCache {
        let palette = self.effective_palette();
        CoverCache::new(
            self.config
                .app
                .cover_size
                .max(self.width.max(self.height) / 2),
            8,
            [
                palette.background.r,
                palette.background.g,
                palette.background.b,
            ],
        )
    }

    /// Thumbnail edge and capacity for the Gallery browse rectangle. The
    /// cache holds at least one complete visible page, or an eviction would
    /// force the same page to decode again forever on large framebuffers.
    fn gallery_cache_spec(&self) -> (u32, usize, [u8; 3]) {
        let geometry = Geometry::compute(
            Layout::Gallery,
            false,
            false,
            self.show_bar,
            self.width,
            self.height,
            &self.config,
        );
        let edge = geometry
            .tile_width
            .max(geometry.tile_height)
            .ceil()
            .max(1.0) as u32;
        let palette = self.effective_palette();
        (
            edge,
            self.config.app.art_cache.max(geometry.visible).max(8),
            [palette.surface.r, palette.surface.g, palette.surface.b],
        )
    }

    fn fresh_gallery_cover_cache(&self) -> CoverCache {
        let (edge, capacity, ground) = self.gallery_cache_spec();
        CoverCache::new(edge, capacity, ground)
    }

    /// Put the effective palette everywhere colour lives: the Slint
    /// properties, the wordmark tint, and the cover cache, whose decoded
    /// pictures were composited against the old surface colour and would
    /// keep it in their corners until evicted.
    fn apply_palette(&mut self) {
        let palette = self.effective_palette();
        push_palette(
            &self.ui,
            &palette,
            self.effective_logo(),
            self.effective_logo_opacity(),
        );
        self.covers = self.fresh_cover_cache();
        self.group_covers = self.fresh_group_cover_cache();
        self.gallery_covers = self.fresh_gallery_cover_cache();
        self.touch_selection();
    }

    fn open_theme_editor(&mut self) {
        self.theme_editor = Some(ThemeEditor::new(
            &self.config.colors,
            &self.themes,
            self.active_theme,
            self.font,
            self.system_font,
        ));
        self.screen = Screen::ThemeEditor;
        self.apply_geometry();
        self.preview_theme_editor();
        self.sync_theme_editor();
    }

    fn preview_theme_editor(&mut self) {
        let (palette, logo, logo_opacity, font) = {
            let Some(editor) = self.theme_editor.as_ref() else {
                return;
            };
            (
                editor.draft.palette.clone(),
                editor.draft.logo,
                editor.draft.logo_opacity,
                editor.draft.font,
            )
        };
        if self.font != font {
            self.font = font;
            // Changing colour is the common editor operation and needs no
            // layout rebuild. Recompute only when the draft's typeface
            // changes so the continuous picker stays as light as before.
            self.apply_geometry();
        }
        push_palette(&self.ui, &palette, logo, logo_opacity);
        invalidate_geometry(&mut self.pending_complete_repaints, &mut self.dirty);
    }

    fn sync_theme_editor(&mut self) {
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        let rows = editor.rows();
        self.theme_editor_list = ListState::new(
            rows.len(),
            theme_editor_visible_items(editor.mode, rows.len(), self.geometry.visible),
        );
        if editor.selected_for_ui() >= 0 {
            self.theme_editor_list
                .select(editor.selected_for_ui() as usize);
        }
        self.ui.set_theme_editor_mode(editor.mode.ui_index());
        self.ui.set_theme_selected_role(
            editor
                .selected_role()
                .map(|role| role.index() as i32)
                .unwrap_or(-1),
        );
        self.ui.set_theme_hex_digits(ModelRc::new(VecModel::from(
            editor
                .hex_text()
                .chars()
                .skip(1)
                .map(|character| SharedString::from(character.to_string()))
                .collect::<Vec<_>>(),
        )));
        self.ui.set_theme_hex_cursor(editor.hex_cursor as i32);
        self.ui
            .set_theme_edit_color(to_slint(editor.editing_color()));
        self.ui
            .set_theme_edit_hex(SharedString::from(editor.hex_text()));
        self.ui
            .set_theme_picker_channel(editor.picker_channel as i32);
        self.ui.set_theme_name_save_index(NAME_SAVE as i32);
        self.ui.set_theme_name_cancel_index(NAME_CANCEL as i32);
        self.ui
            .set_theme_name(SharedString::from(editor.name.as_str()));
        self.ui.set_columns(if editor.mode == EditorMode::Name {
            NAME_COLUMNS as i32
        } else {
            1
        });
        self.ui.set_grid_rows(if editor.mode == EditorMode::Name {
            NAME_CELLS.div_ceil(NAME_COLUMNS) as i32
        } else {
            1
        });
        self.dirty = true;
    }

    fn close_theme_editor(&mut self) {
        self.theme_editor = None;
        self.screen = Screen::Options;
        self.font = self.effective_font();
        self.apply_palette();
        self.apply_geometry();
        invalidate_geometry(&mut self.pending_complete_repaints, &mut self.dirty);
    }

    fn save_theme_editor(&mut self, name: &str) {
        let Some(editor) = self.theme_editor.as_ref() else {
            return;
        };
        let file = editor.draft.file();
        let path = match crate::theme::save_new(&self.themes_dir, name, &file, &self.config.colors)
        {
            Ok(path) => path,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };

        let loaded = crate::theme::load_available(&self.themes_dir);
        let Some(active_theme) = loaded
            .themes
            .iter()
            .position(|theme| theme.name.eq_ignore_ascii_case(name))
        else {
            let rollback = std::fs::remove_file(&path);
            self.message = Some(match rollback {
                Ok(()) => format!("Saved theme {name:?} did not reload"),
                Err(error) => format!(
                    "Saved theme {name:?} did not reload; removing {} also failed: {error}",
                    path.display()
                ),
            });
            self.dirty = true;
            return;
        };

        let mut settings = self.settings.clone();
        settings.theme = Some(loaded.themes[active_theme].name.clone());
        settings.theme_font_override = Some(false);
        let settings_warning = match settings.save(&self.settings_path) {
            Ok(SaveOutcome::Durable) => None,
            Ok(SaveOutcome::InstalledWithWarning(warning)) => Some(warning.to_string()),
            Err(error) => {
                let rollback = std::fs::remove_file(&path);
                self.message = Some(match rollback {
                    Ok(()) => error.to_string(),
                    Err(rollback_error) => format!(
                        "{error}; removing {} also failed: {rollback_error}",
                        path.display()
                    ),
                });
                self.dirty = true;
                return;
            }
        };

        self.themes = loaded.themes;
        self.active_theme = Some(active_theme);
        self.settings = settings;
        self.close_theme_editor();
        if let Some(warning) = settings_warning {
            self.message = Some(format!("Theme {name:?} was saved; {warning}"));
            self.dirty = true;
        }
    }

    fn delete_theme_editor(&mut self, name: &str) {
        let Some(theme) = self
            .themes
            .iter()
            .find(|theme| theme.name.eq_ignore_ascii_case(name))
            .cloned()
        else {
            self.message = Some(format!("Theme {name:?} is no longer available"));
            self.dirty = true;
            return;
        };
        let staged = match crate::theme::stage_editor_theme_deletion(&self.themes_dir, &theme) {
            Ok(staged) => staged,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };

        let deleting_active = self
            .settings
            .theme
            .as_deref()
            .is_some_and(|active| active.eq_ignore_ascii_case(&theme.name));
        let previous_settings = self.settings.clone();
        let mut settings = previous_settings.clone();
        let mut settings_warning = None;
        if deleting_active {
            settings.theme = None;
            settings.theme_font_override = Some(false);
            match settings.save(&self.settings_path) {
                Ok(SaveOutcome::Durable) => {}
                Ok(SaveOutcome::InstalledWithWarning(warning)) => {
                    settings_warning = Some(warning.to_string())
                }
                Err(error) => {
                    let restore_error = staged.restore().err();
                    if restore_error.is_some() {
                        self.themes = crate::theme::load_available(&self.themes_dir).themes;
                        self.active_theme = self.settings.theme.as_deref().and_then(|active| {
                            self.themes
                                .iter()
                                .position(|held| held.name.eq_ignore_ascii_case(active))
                        });
                        self.close_theme_editor();
                    }
                    self.message = Some(if let Some(restore_error) = restore_error {
                        format!("{error}; {restore_error}")
                    } else {
                        error.to_string()
                    });
                    self.dirty = true;
                    return;
                }
            }
        }

        if let Err(deletion_error) = staged.commit() {
            let theme_restore = staged.restore();
            let mut settings_after_failure = settings;
            let mut recovery = Vec::new();
            match theme_restore {
                Ok(()) if deleting_active => match previous_settings.save(&self.settings_path) {
                    Ok(SaveOutcome::Durable) => settings_after_failure = previous_settings,
                    Ok(SaveOutcome::InstalledWithWarning(warning)) => {
                        settings_after_failure = previous_settings;
                        recovery.push(format!("restoring settings reported: {warning}"));
                    }
                    Err(error) => recovery.push(format!("restoring settings failed: {error}")),
                },
                Ok(()) => {}
                Err(error) => recovery.push(error.to_string()),
            }
            self.themes = crate::theme::load_available(&self.themes_dir).themes;
            self.settings = settings_after_failure;
            self.active_theme = self.settings.theme.as_deref().and_then(|active| {
                self.themes
                    .iter()
                    .position(|held| held.name.eq_ignore_ascii_case(active))
            });
            self.close_theme_editor();
            self.message = Some(if recovery.is_empty() {
                format!("Theme {name:?} was not deleted: {deletion_error}")
            } else {
                format!(
                    "Theme {name:?} could not be deleted: {deletion_error}; {}",
                    recovery.join("; ")
                )
            });
            self.dirty = true;
            return;
        }

        let loaded = crate::theme::load_available(&self.themes_dir);
        self.themes = loaded.themes;
        self.settings = settings;
        self.active_theme = self.settings.theme.as_deref().and_then(|active| {
            self.themes
                .iter()
                .position(|held| held.name.eq_ignore_ascii_case(active))
        });
        self.close_theme_editor();
        self.message = Some(if let Some(warning) = settings_warning {
            format!("Theme {name:?} deleted; {warning}")
        } else {
            format!("Theme {name:?} deleted")
        });
        self.dirty = true;
    }

    fn apply_theme_editor_effect(&mut self, effect: EditorEffect) {
        match effect {
            EditorEffect::None => {}
            EditorEffect::PreviewChanged => self.preview_theme_editor(),
            EditorEffect::Save(name) => {
                self.save_theme_editor(&name);
                return;
            }
            EditorEffect::Delete(name) => {
                self.delete_theme_editor(&name);
                return;
            }
            EditorEffect::Close => {
                self.close_theme_editor();
                return;
            }
            EditorEffect::Error(error) => self.message = Some(error),
        }
        self.sync_theme_editor();
    }

    fn handle_theme_editor(&mut self, action: Action) {
        if message_consumes_input(self.message.is_some(), self.build.is_some()) {
            if self.scroll_message(action) {
                return;
            }
            self.message = None;
            self.dirty = true;
            return;
        }
        let Some(editor) = self.theme_editor.as_mut() else {
            self.close_theme_editor();
            return;
        };
        let effect = match action {
            Action::Up => editor.vertical(-1),
            Action::Down => editor.vertical(1),
            Action::Slower => editor.horizontal(-1),
            Action::Faster => editor.horizontal(1),
            Action::Accept => editor.accept(),
            Action::Quit => editor.back(),
            Action::Context => editor.x(),
            Action::Menu => editor.y(),
            Action::PageUp
            | Action::PageDown
            | Action::Home
            | Action::End
            | Action::CyclePresent
            | Action::FavoriteShortcut
            | Action::RandomShortcut => EditorEffect::None,
        };
        self.apply_theme_editor_effect(effect);
    }

    fn handle_option_input(&mut self, input: OptionInput) {
        let list = self.option_ids();
        let selected = self.active_list().selected();
        let Some(option) = list.get(selected).copied() else {
            return;
        };
        match option_operation(option, input) {
            OptionOperation::None => (),
            OptionOperation::Adjust(delta) => self.adjust_option_value(option, delta),
            OptionOperation::OpenThemeEditor => self.open_theme_editor(),
            OptionOperation::ConfirmResetCustomViews => {
                self.pending = Some(Pending::ResetCustomViews);
                self.message = Some("Reset all custom views?\n\nA yes, B no".to_string());
                self.dirty = true;
            }
            OptionOperation::ConfirmResetHidden => {
                self.pending = Some(Pending::ResetHidden);
                self.message = Some("Unhide everything?\n\nA yes, B no".to_string());
                self.dirty = true;
            }
            OptionOperation::RebuildCache => self.rebuild_all_systems(),
            OptionOperation::OpenScraperAll => {
                self.open_scraper(crate::scraper::Scope::All, Screen::Options)
            }
            OptionOperation::OpenAdvanced => {
                self.screen = Screen::Advanced;
                self.apply_geometry();
                self.dirty = true;
            }
        }
    }

    fn adjust_option_value(&mut self, option: OptionId, delta: isize) {
        match option {
            // The cursor never rests on one, but a stale index after a
            // screen change must do nothing rather than something.
            OptionId::Spacer => {}
            OptionId::Speed => {
                self.speed = step(self.speed, delta, SPEED_STEPS.len());
                self.settings.speed_step = Some(self.speed);
            }
            OptionId::ArtLimit => {
                let next = step(self.art_limit(), delta, SPEED_STEPS.len());
                self.settings.art_limit = Some(next);
            }
            OptionId::Layout => {
                self.global_layout = if delta < 0 {
                    self.global_layout.prev()
                } else {
                    self.global_layout.next()
                };
                self.settings.layout = Some(self.global_layout.label().to_string());
                self.resolve_view();
                self.apply_geometry();
            }
            OptionId::LeftRight => {
                let at = step(self.horizontal.index(), delta, Horizontal::ALL.len());
                self.horizontal = Horizontal::ALL[at];
                self.settings.left_right = Some(self.horizontal.label().to_string());
            }
            OptionId::Font => {
                self.font = if delta < 0 {
                    self.font.prev()
                } else {
                    self.font.next()
                };
                self.system_font = self.font;
                self.settings.font = Some(self.font.label().to_string());
                self.settings.theme_font_override = Some(self.active_theme.is_some());
                // Nothing about the layout moves, but every glyph on the
                // screen is now a different one.
                self.apply_geometry();
            }
            OptionId::Theme => {
                // One ring: standard, then the folder in name order.
                let at = step(self.theme_position(), delta, self.themes.len() + 1);
                self.active_theme = at.checked_sub(1);
                self.settings.theme = self.active_theme.map(|at| self.themes[at].name.clone());
                // Theme defaults never overwrite the persistent Text option.
                // A missing or invalid default resolves to that system font,
                // not to the theme that happened to be selected previously.
                self.settings.theme_font_override = Some(false);
                let before = self.font;
                self.font = self.effective_font();
                if self.font != before {
                    self.apply_geometry();
                }
                self.apply_palette();
                // Colour lives in corners no ordinary frame touches, so
                // the whole screen is drawn again rather than only what
                // Slint saw change.
                invalidate_geometry(&mut self.pending_complete_repaints, &mut self.dirty);
            }
            OptionId::ShowArt => {
                self.show_art = !self.show_art;
                self.settings.show_art = Some(self.show_art);
                self.touch_selection();
            }
            OptionId::ArtworkScale => {
                let at = step(self.artwork_scale.index(), delta, ArtworkScale::ALL.len());
                self.artwork_scale = ArtworkScale::ALL[at];
                self.settings.artwork_scale = Some(self.artwork_scale.setting().to_string());
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
            OptionId::ShowUnstable => {
                self.show_unstable = !self.show_unstable;
                self.settings.show_unstable = Some(self.show_unstable);
                self.rebuild_system_list();
            }
            OptionId::ShowScripts => {
                self.settings.show_scripts = Some(!self.settings.show_scripts.unwrap_or(true));
            }
            OptionId::CorePreference => {
                self.settings.core_preference =
                    Some(self.settings.core_preference.unwrap_or_default().next());
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
            OptionId::HoldXFavorite => {
                self.hold_x_favorite = !self.hold_x_favorite;
                self.settings.hold_x_favorite = Some(self.hold_x_favorite);
            }
            OptionId::HoldYRandom => {
                self.hold_y_random = !self.hold_y_random;
                self.settings.hold_y_random = Some(self.hold_y_random);
            }
            OptionId::RandomLaunches => {
                self.random_launches = !self.random_launches;
                self.settings.random_launches = Some(self.random_launches);
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
            OptionId::ResetCustomViews
            | OptionId::RebuildCache
            | OptionId::ScrapeAll
            | OptionId::ResetHidden
            | OptionId::Advanced => {
                debug_assert!(false, "action row reached value adjustment")
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
            OptionId::Layout => self.global_layout.shown().to_string(),
            OptionId::ResetCustomViews => match self.settings.custom_views.len() {
                0 => "None".to_string(),
                count => format!("{count} set"),
            },
            OptionId::LeftRight => self.horizontal.shown().to_string(),
            OptionId::Font => self.font.shown().to_string(),
            // The names are file names, shown as the user wrote them; only
            // the built-in state has a word of its own.
            OptionId::Theme => match self.active_theme {
                Some(at) => self.themes[at].name.clone(),
                None => "Standard".to_string(),
            },
            OptionId::ShowArt => on_off(self.show_art),
            OptionId::ArtworkScale => self.artwork_scale.shown().to_string(),
            OptionId::ShowStats => on_off(self.show_stats),
            OptionId::Present => capitalised(self.present_label),
            OptionId::ShowHidden => on_off(self.show_hidden),
            OptionId::ShowEmpty => on_off(self.show_empty),
            OptionId::ShowOther => on_off(self.show_other),
            OptionId::ShowUtility => on_off(self.show_utility),
            OptionId::ShowUnstable => on_off(self.show_unstable),
            OptionId::ShowScripts => on_off(self.settings.show_scripts.unwrap_or(true)),
            OptionId::CorePreference => self
                .settings
                .core_preference
                .unwrap_or_default()
                .label()
                .to_string(),
            OptionId::ShowBar => on_off(self.show_bar),
            OptionId::FavoritesFirst => on_off(self.favorites_first),
            OptionId::HoldXFavorite => on_off(self.hold_x_favorite),
            OptionId::HoldYRandom => on_off(self.hold_y_random),
            OptionId::RandomLaunches => if self.random_launches {
                "Launches"
            } else {
                "Selects"
            }
            .to_string(),
            // The label reads Folders before games, so On is the leading
            // position and the stored folders_last flag inverts.
            OptionId::FoldersLast => on_off(!self.folders_last),
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
            OptionId::ScrapeAll => ">".to_string(),
            OptionId::Screensaver => match self.screensaver_after() {
                0 => "off".to_string(),
                60 => "1 minute".to_string(),
                seconds => format!("{} minutes", seconds / 60),
            },
            OptionId::ShiftX => format!("{:+} px", self.shift_x()),
            OptionId::ShiftY => format!("{:+} px", self.shift_y()),
            OptionId::Advanced => ">".to_string(),
            OptionId::Spacer => String::new(),
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

    fn save_settings(&mut self) -> bool {
        match self.settings.save(&self.settings_path) {
            Ok(SaveOutcome::Durable) => true,
            Ok(SaveOutcome::InstalledWithWarning(warning)) => {
                self.message = Some(format!("Settings installed, but durability could not be confirmed: {warning}\n\nClose this message, then go back to retry saving."));
                self.dirty = true;
                false
            }
            Err(error) => {
                eprintln!("degauss: could not save settings: {error}");
                self.message = Some(format!("Settings not saved: {error}\n\nChanges remain active in this session. Close this message, then go back to retry saving."));
                self.dirty = true;
                false
            }
        }
    }

    fn reset_custom_views(&mut self) {
        // The legacy map is normally empty after startup migration. Clear it
        // too so a reset in the same run cannot resurrect an old view later.
        let removed = clear_custom_views(&mut self.settings);
        let saved = self.save_settings();
        self.resolve_view();
        self.apply_geometry();
        if saved {
            self.message = Some(format!("{removed} custom views reset"));
        }
        self.dirty = true;
    }

    fn reset_hidden(&mut self) {
        let held = clear_hidden(&mut self.settings);
        let saved = self.save_settings();
        let save_message = self.message.take();
        self.corrected_counts.clear();
        self.rebuild_system_list();
        self.relist_here();
        if saved {
            self.message = Some(format!("{held} shown again"));
        } else {
            self.message = save_message;
        }
        self.dirty = true;
    }

    fn rebuild_all_systems(&mut self) {
        // One rebuild at a time: a second ask would swap the system list from
        // under the running build. Its progress message already answers A.
        if self.build.is_some() || self.source_job.is_some() || self.refreshing.is_some() {
            return;
        }
        if self.provider_job.is_some() {
            self.message = Some("Wait for the current Artwork Pack read to finish.".to_string());
            self.dirty = true;
            return;
        }
        self.index_return_screen = self.screen;
        self.index_terminal = None;
        self.index_details = false;
        // Keep the old systems and index until this read finishes. No card
        // access is dispatched until Preparing has reached the surface.
        self.build = Some(Building {
            left: Vec::new(),
            done: 0,
            total: 0,
            index: self.index.clone().unwrap_or_default(),
            forced: true,
            current: None,
            job: None,
            discovery: Some(Discovery::Queued(crate::index_job::DiscoveryRequest {
                menu_root: PathBuf::from(&self.config.menu_root),
                roots: self.config.game_roots.iter().map(PathBuf::from).collect(),
                table: self.table.clone(),
                logo_dir: self.logo_dir.clone(),
            })),
            awaiting_frame: true,
            displayed: None,
            cancelling: false,
            single: false,
            folders: 0,
            games: 0,
            folders_done: 0,
            games_done: 0,
            folder: String::new(),
            folder_revision: 0,
            started: Instant::now(),
        });
        self.pause_index_marquees();
        self.publish_index_progress();
        self.dirty = true;
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
                self.resolve_view();
                self.apply_geometry();
                self.touch_selection();
            }
            Screen::Scraper => {
                if self.save_scraper_settings() {
                    self.screen = self.scraper_return;
                    self.apply_geometry();
                }
            }
            Screen::ScraperKeyboard => self.finish_scraper_keyboard(),
            Screen::ScraperProgress => {
                self.handle_scraper_progress(Action::Quit);
            }
            Screen::CategoryImage => self.leave_category_image_picker(),
            Screen::ScraperMatches => self.leave_scraper_matches(),
            Screen::GameDataSource => {
                self.screen = Screen::Browse;
                self.reopen_context_for(GAME_DATA_SOURCE);
                if let Some(index) = self.menu.iter().position(|entry| entry == GAME_DATA_SOURCE) {
                    self.menu_list.select(index);
                }
            }
            Screen::ArtworkPackLocation => self.open_game_data_source(),
            Screen::ArtworkPackDirectory => self.leave_artwork_pack_directory(),
            Screen::Scripts => self.leave_scripts(),
            Screen::SourceProgress => {
                if let Some(job) = &self.source_job {
                    job.cancel();
                    self.source_cancelling = true;
                    self.dirty = true;
                } else if let Some(job) = &self.provider_job {
                    job.cancel();
                    self.provider_cancelling = true;
                    self.dirty = true;
                }
            }
            Screen::Advanced => {
                if !self.save_settings() {
                    return None;
                }
                self.screen = Screen::OptionsRoot;
                self.apply_geometry();
            }
            Screen::ThemeEditor => self.close_theme_editor(),
            Screen::Options => {
                if !self.save_settings() {
                    return None;
                }
                self.screen = Screen::OptionsRoot;
                self.apply_geometry();
            }
            Screen::OptionsRoot => {
                self.open_menu();
            }
            Screen::Information => {
                self.screen = Screen::Context;
                self.apply_geometry();
            }
            Screen::Help | Screen::About => {
                self.screen = Screen::Menu;
                self.apply_geometry();
            }
            Screen::Splash => self.leave_splash(),
            Screen::Menu | Screen::Context => {
                if self.screen == Screen::Context {
                    // A contextual change is deliberately saved on close so
                    // stepping through several previews does not write the
                    // card for every press.
                    if !self.save_settings() {
                        return None;
                    }
                    self.remember_context_selection();
                    if self.context_page.is_some() {
                        self.show_context_page(None);
                        return None;
                    }
                }
                self.screen = Screen::Browse;
                self.resolve_view();
                self.apply_geometry();
                // Asked for again on the way back in: a theme change in
                // Options rebuilt the cover cache while a menu screen was
                // up, and load_art drops the pending ask on screens that
                // show no artwork, so without this the details panel would
                // keep a picture composited against the old surface colour.
                self.touch_selection();
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
                    self.artwork_provider = None;
                    self.opened_config = None;
                    self.trail.clear();
                    if self.skipped_systems {
                        // We stepped through this group on the way in.
                        self.skipped_systems = false;
                        self.remember_system_here();
                        self.browsing = Browsing::Categories;
                        self.open_category = None;
                        self.rebuild_system_list();
                        self.resolve_view();
                        self.apply_geometry();
                        self.touch_selection();
                        self.dirty = true;
                        return None;
                    }
                    self.browsing = Browsing::Systems;
                    self.resolve_view();
                    self.apply_geometry();
                    self.touch_selection();
                }
                Browsing::Systems => {
                    self.remember_system_here();
                    self.browsing = Browsing::Categories;
                    self.open_category = None;
                    self.rebuild_system_list();
                    self.resolve_view();
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
        self.menu = menu_entries(
            self.browsing,
            system.as_deref(),
            self.settings.show_scripts.unwrap_or(true),
        );
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::Menu;
        self.apply_geometry();
    }

    fn open_scripts(&mut self) {
        match crate::scripts::Browser::open(Path::new(&self.config.menu_root)) {
            Ok(browser) => {
                let root = browser.root().to_path_buf();
                self.scripts_browser = Some(browser);
                self.show_scripts_directory(root, None);
            }
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
            }
        }
    }

    fn show_scripts_directory(&mut self, directory: PathBuf, selected: Option<&Path>) {
        let Some(browser) = &self.scripts_browser else {
            return;
        };
        match browser.list(&directory) {
            Ok(entries) => {
                let selected = selected
                    .and_then(|path| entries.iter().position(|entry| entry.path == path))
                    .unwrap_or(0);
                self.menu = entries
                    .iter()
                    .map(|entry| {
                        if entry.is_directory {
                            format!("[ {} ]", entry.name)
                        } else {
                            entry.name.clone()
                        }
                    })
                    .collect();
                self.menu_list = ListState::new(entries.len(), self.geometry.visible);
                self.menu_list.select(selected);
                self.scripts_entries = entries;
                self.scripts_directory = directory;
                self.screen = Screen::Scripts;
                self.apply_geometry();
            }
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
            }
        }
    }

    fn activate_script(&mut self) {
        let Some(entry) = self.scripts_entries.get(self.menu_list.selected()).cloned() else {
            return;
        };
        if entry.is_directory {
            self.show_scripts_directory(entry.path, None);
            return;
        }
        let Some(browser) = &self.scripts_browser else {
            return;
        };
        let launch = std::env::current_exe()
            .map_err(|error| DegaussError::io("locate frontend executable", "degauss", error))
            .and_then(|executable| {
                crate::scripts::Launch::prepare(
                    browser.root(),
                    &entry.path,
                    &executable,
                    std::env::args_os().skip(1).collect(),
                )
            });
        match launch {
            Ok(launch) => {
                self.pending = Some(Pending::RunScript(Box::new(launch)));
                self.message = Some(format!(
                    "Run {}?\n\nOnly run scripts you trust.\nDegauss returns when it finishes.\n\nA Run   B Cancel",
                    entry.name
                ));
            }
            Err(error) => self.message = Some(error.to_string()),
        }
        self.dirty = true;
    }

    fn leave_scripts(&mut self) {
        let Some(browser) = &self.scripts_browser else {
            self.open_menu();
            return;
        };
        if self.scripts_directory == browser.root() {
            self.open_menu();
            if let Some(selected) = self.menu.iter().position(|entry| entry == "Scripts") {
                self.menu_list.select(selected);
            }
        } else if let Some(parent) = self.scripts_directory.parent().map(Path::to_path_buf) {
            let selected = self.scripts_directory.clone();
            self.show_scripts_directory(parent, Some(&selected));
        }
    }

    pub fn resume_scripts(&mut self, script: &Path) {
        self.open_menu();
        self.open_scripts();
        if self.message.is_some() {
            return;
        }
        if let Some(parent) = script.parent() {
            match parent.canonicalize() {
                Ok(parent) => {
                    let selected = parent.join(script.file_name().unwrap_or_default());
                    self.show_scripts_directory(parent, Some(&selected));
                }
                Err(error) => {
                    self.message = Some(
                        DegaussError::io("resolve returning Scripts directory", parent, error)
                            .to_string(),
                    );
                    self.dirty = true;
                }
            }
        }
    }

    /// What a contextual entry currently reads as, when it is a choice
    /// rather than an action.
    fn context_value(&self, index: usize) -> String {
        if self.context_is_root() {
            return ">".to_string();
        }
        match self.menu.get(index).map(String::as_str) {
            Some(CORE_VERSION) => self
                .core_system_id()
                .and_then(|id| self.settings.core_choices.get(&id))
                .map(|key| match key.as_str() {
                    "standard" => "Standard".to_string(),
                    "ra" => "RetroAchievements".to_string(),
                    _ => key.clone(),
                })
                .unwrap_or_else(|| "Default".to_string()),
            Some(CHANGE_VIEW) => self.layout.shown().to_string(),
            Some(GAME_DATA_SOURCE) => self
                .context_system_id()
                .map(|id| self.source_label(id))
                .unwrap_or_default(),
            _ => String::new(),
        }
    }

    /// Change the selected contextual entry the way left and right change a
    /// setting. A menu offering a choice should let the stick walk it
    /// rather than hiding it behind a press.
    fn adjust_context(&mut self, delta: isize) {
        let selected = self.menu_list.selected();
        if self.menu.get(selected).map(String::as_str) == Some(CORE_VERSION) {
            self.adjust_core_version(delta);
            return;
        }
        if self.menu.get(selected).map(String::as_str) != Some(CHANGE_VIEW) {
            return;
        }
        self.layout = if delta >= 0 {
            self.layout.next()
        } else {
            self.layout.prev()
        };
        self.remember_view();
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }

    fn context_scope(&self) -> String {
        let action = self
            .menu
            .get(self.menu_list.selected())
            .map(String::as_str)
            .unwrap_or("");
        if matches!(
            action,
            GAME_INFORMATION | ADD_FAVORITE | REMOVE_FAVORITE | HIDE_THIS | SHOW_THIS
        ) {
            match self.browsing {
                Browsing::Games => {
                    if let Some(row) = self.here.get(self.game_list.selected()) {
                        return row.name.clone();
                    }
                }
                Browsing::Systems => {
                    if let Some(system) = self.systems.get(self.system_list.selected()) {
                        return system.name().to_string();
                    }
                }
                Browsing::Categories => {}
            }
        }
        if matches!(action, CHANGE_CATEGORY_IMAGE | CLEAR_CATEGORY_IMAGE) {
            if let Some(target) = self.selected_image_target() {
                return target.label().to_string();
            }
        }
        if matches!(action, SCRAPE_SYSTEM | SCRAPE_FOLDER | SCRAPE_GAME) {
            if let Some(scope) = self.scraper_scope_from_context(action) {
                return match scope {
                    crate::scraper::Scope::System { display_name, .. }
                    | crate::scraper::Scope::Folder { display_name, .. } => display_name,
                    crate::scraper::Scope::Game { title, .. } => title,
                    crate::scraper::Scope::All => "All Systems".to_string(),
                };
            }
        }
        if matches!(
            action,
            CORE_VERSION | USE_DEFAULT_CORE_VERSION | GAME_DATA_SOURCE | REBUILD_SYSTEM
        ) {
            if self.in_favorites() && matches!(action, CORE_VERSION | USE_DEFAULT_CORE_VERSION) {
                return "Original Game System".to_string();
            }
            if let Some(id) = self.context_system_id() {
                if let Some(system) = self.all_systems.iter().find(|system| system.def.id == id) {
                    return system.name().to_string();
                }
            }
        }
        match self.browsing {
            Browsing::Categories => "Home".to_string(),
            Browsing::Systems => self
                .open_category
                .clone()
                .unwrap_or_else(|| "Systems".to_string()),
            Browsing::Games => self.here_label(),
        }
    }

    fn core_system_id(&self) -> Option<String> {
        let id = if self.in_favorites() && self.browsing == Browsing::Games {
            let path = self.selected_game()?;
            let reference = crate::favorites::reference_of_with_systems(&path, &self.all_systems)?;
            owner_of_favorite(&self.all_systems, &reference)?
        } else {
            self.context_system_id()?.to_string()
        };
        self.all_systems
            .iter()
            .find(|system| system.def.id == id && !system.def.rbf.is_empty())
            .map(|_| id)
    }

    fn adjust_core_version(&mut self, delta: isize) {
        let Some(id) = self.core_system_id() else {
            return;
        };
        let Some(system) = self.all_systems.iter().find(|system| system.def.id == id) else {
            return;
        };
        let available = match crate::core_choices::available(
            &system.to_config(),
            Path::new(&self.config.menu_root),
        ) {
            Ok(choices) => choices,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };
        let mut choices = vec![(String::new(), "Default".to_string())];
        choices.extend(
            available
                .into_iter()
                .map(|choice| (choice.key, choice.label)),
        );
        let current = self
            .settings
            .core_choices
            .get(&id)
            .map(String::as_str)
            .unwrap_or("");
        let at = choices
            .iter()
            .position(|(key, _)| key == current)
            .unwrap_or(0);
        let next = (at as isize + delta.signum()).rem_euclid(choices.len() as isize) as usize;
        let (key, label) = &choices[next];
        let mut settings = self.settings.clone();
        if key.is_empty() {
            settings.core_choices.remove(&id);
        } else {
            settings.core_choices.insert(id, key.clone());
        }
        match settings.save(&self.settings_path) {
            Ok(outcome) => {
                self.settings = settings;
                self.reopen_context_for(CORE_VERSION);
                if let Some(index) = self.menu.iter().position(|entry| entry == CORE_VERSION) {
                    self.menu_list.select(index);
                }
                self.message = match outcome {
                    crate::settings::SaveOutcome::Durable => None,
                    crate::settings::SaveOutcome::InstalledWithWarning(warning) => {
                        Some(format!("Core Version: {label}\n{warning}"))
                    }
                };
            }
            Err(error) => self.message = Some(error.to_string()),
        }
        self.dirty = true;
    }

    /// Clear the saved override without consulting any mounted core folder.
    /// This remains usable when enumeration failed or a selected file vanished.
    fn use_default_core_version(&mut self) {
        let Some(id) = self.core_system_id() else {
            return;
        };
        let mut settings = self.settings.clone();
        if settings.core_choices.remove(&id).is_none() {
            return;
        }
        match settings.save(&self.settings_path) {
            Ok(outcome) => {
                self.settings = settings;
                self.screen = Screen::Browse;
                self.apply_geometry();
                self.message = Some(match outcome {
                    crate::settings::SaveOutcome::Durable => "Core Version: Default".to_string(),
                    crate::settings::SaveOutcome::InstalledWithWarning(warning) => {
                        format!("Core Version: Default\n{warning}")
                    }
                });
            }
            Err(error) => self.message = Some(error.to_string()),
        }
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

    /// Move to the previous or next run of rows sharing a first letter,
    /// in whatever list is on screen, narrowed or not.
    fn letter_step(&mut self, delta: isize) {
        let letters: Vec<char> = match self.browsing {
            Browsing::Games => self
                .here
                .iter()
                .map(|row| first_letter(&row.sort_key))
                .collect(),
            Browsing::Systems => self
                .systems
                .iter()
                .map(|system| first_letter(&system.name().to_lowercase()))
                .collect(),
            // The groups are a handful of fixed names; the letter grid
            // does not offer them either.
            Browsing::Categories => return,
        };
        if letters.is_empty() {
            return;
        }
        let target = letter_target(&letters, self.active_list().selected(), delta);
        if target != self.active_list().selected() {
            self.active_list_mut().select(target);
            self.touch_selection();
        }
    }

    /// Move a screenful at a time. From the middle of the list this stops
    /// at the ends and wraps only from them, which is the edge rule
    /// [`ListState::move_items`] already applies to every move.
    fn page_step(&mut self, delta: isize) {
        let visible = if self.screen == Screen::Context {
            context_window(&self.menu, &self.menu_list, self.geometry.visible)
                .0
                .len()
        } else {
            self.active_list().visible()
        } as isize;
        if self.active_list_mut().move_items(delta * visible) {
            self.skip_blank_menu(delta);
            self.touch_selection();
        }
    }

    /// Move the selection to the letter picked.
    fn jump_to(&mut self, letter: char) {
        let key = letter.to_ascii_lowercase();
        // Rows gathered into the leading favourite group sit outside the
        // alphabetical non-favourite body. Skip that group only when the body
        // exists in the current, possibly filtered list. Inside Favourites
        // every game is a favourite and is therefore the body itself.
        let skip_favorites = self.browsing == Browsing::Games
            && self.favorites_first
            && !self.in_favorites()
            && self
                .here
                .iter()
                .any(|row| !row.is_folder() && !row.favorite);
        let entries: Vec<(char, bool, bool)> = match self.browsing {
            Browsing::Games => self
                .here
                .iter()
                .map(|row| (first_letter(&row.sort_key), row.is_folder(), row.favorite))
                .collect(),
            Browsing::Systems => self
                .systems
                .iter()
                .map(|system| (first_letter(&system.name().to_lowercase()), false, false))
                .collect(),
            Browsing::Categories => return,
        };
        match jump_target(&entries, key, skip_favorites) {
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

    /// The operation a held X would perform now. The same answer enables the
    /// timer and handles its result, so a row that cannot be changed never
    /// acquires a delayed X press.
    fn favorite_change(&self) -> Option<FavoriteChange> {
        let selected = self.selected_game();
        let favorite = selected
            .as_deref()
            .is_some_and(|game| self.favorites.holds(game));
        favorite_change(
            self.hold_x_favorite,
            self.screen,
            self.browsing,
            self.in_favorites(),
            selected.is_some(),
            favorite,
        )
    }

    fn random_shortcut_enabled(&self) -> bool {
        self.hold_y_random
            && self.screen == Screen::Browse
            && self.browsing == Browsing::Games
            && !self.trail.is_empty()
            && self.message.is_none()
            && self.pending.is_none()
            && self.build.is_none()
            && self.index_terminal.is_none()
            && self.refreshing.is_none()
            && self.scraper_refresh_job.is_none()
            && self.source_job.is_none()
            && self.provider_job.is_none()
            && self.scraper_job.is_none()
            && self.scraper_pending_terminal.is_none()
    }

    /// Offer the folders MiSTer's favourites are already kept in, and the
    /// chance to name another.
    fn open_favorite_folders(&mut self) {
        let mut entries = match crate::favorites::folders(&self.favorites_root()) {
            Ok(entries) => entries,
            Err(e) => {
                self.screen = Screen::Browse;
                self.apply_geometry();
                self.message = Some(format!("{e}"));
                self.dirty = true;
                return;
            }
        };
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

        let outcome = match crate::launch::favorite_mgl_with_choice(
            &config,
            &game,
            Path::new(&self.config.menu_root),
            self.settings.core_preference.unwrap_or_default()
                == crate::settings::CorePreference::RetroAchievementsFirst,
            self.open_system
                .as_ref()
                .and_then(|id| self.settings.core_choices.get(id))
                .map(String::as_str),
        ) {
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
    ///
    /// Everywhere else the file is looked up by the game it points at; on
    /// the Favourites shelf the row under the cursor IS that file, and
    /// the lookup, keyed by targets, would find nothing for it.
    fn remove_favorite(&mut self) {
        let Some(game) = self.selected_game() else {
            return;
        };
        let file = if self.in_favorites() {
            Some(game.clone())
        } else {
            self.favorites.file_for(&game).map(Path::to_path_buf)
        };
        let Some(file) = file else {
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

    fn selected_category_name(&self) -> Option<&str> {
        self.categories
            .get(self.category_list.selected())
            .map(|(name, _)| name.as_str())
    }

    fn selected_image_target(&self) -> Option<ImageTarget> {
        match self.browsing {
            Browsing::Categories => self
                .selected_category_name()
                .map(|name| ImageTarget::Category(name.to_string())),
            Browsing::Systems => {
                self.systems
                    .get(self.system_list.selected())
                    .map(|system| ImageTarget::System {
                        id: system.def.id.clone(),
                        name: system.name().to_string(),
                    })
            }
            Browsing::Games => None,
        }
    }

    fn open_category_image_picker(&mut self) {
        let Some(target) = self.selected_image_target() else {
            return;
        };
        let Some(logo_dir) = self.logo_dir.as_deref() else {
            self.message = Some("The logos folder is unavailable.".to_string());
            return;
        };
        let choices = match crate::category_images::choices(logo_dir) {
            Ok(choices) => choices,
            Err(error) => {
                crate::note(&format!("custom art could not list logos: {error}"));
                self.message = Some("Degauss could not read the logos folder.".to_string());
                return;
            }
        };
        if choices.is_empty() {
            self.message = Some("No PNG or JPG images were found in the logos folder.".to_string());
            return;
        }
        self.menu = choices.iter().map(|choice| choice.label.clone()).collect();
        self.category_image_choices = choices;
        self.category_image_target = Some(target);
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::CategoryImage;
        self.apply_geometry();
        // The chooser is also the preview. Ask for the first image through
        // the same path every later selection change uses so entering the
        // screen never shows a stale picture from browsing underneath it.
        self.touch_selection();
    }

    fn leave_category_image_picker(&mut self) {
        self.category_image_choices.clear();
        self.category_image_target = None;
        self.screen = Screen::Browse;
        self.reopen_context_for(CHANGE_CATEGORY_IMAGE);
        if let Some(index) = self
            .menu
            .iter()
            .position(|entry| entry == CHANGE_CATEGORY_IMAGE)
        {
            self.menu_list.select(index);
        }
        self.dirty = true;
    }

    fn refresh_category_image(&mut self) {
        self.category_image_choices.clear();
        self.category_image_target = None;
        self.covers = self.fresh_cover_cache();
        self.group_covers = self.fresh_group_cover_cache();
        self.reroll_category_art();
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.touch_selection();
        self.dirty = true;
    }

    fn choose_category_image(&mut self) {
        let Some(choice) = self
            .category_image_choices
            .get(self.menu_list.selected())
            .cloned()
        else {
            return;
        };
        let Some(target) = self.category_image_target.clone() else {
            return;
        };
        let Some(logo_dir) = self.logo_dir.as_deref() else {
            self.message = Some("The logos folder is unavailable.".to_string());
            return;
        };
        match target.install(logo_dir, &choice.path) {
            Ok(_) => self.refresh_category_image(),
            Err(error) => {
                crate::note(&format!("custom art could not be saved: {error}"));
                self.message =
                    Some("That image could not be saved. Choose another PNG or JPG.".to_string());
                self.dirty = true;
            }
        }
    }

    fn clear_category_image(&mut self, target: &ImageTarget) {
        let Some(logo_dir) = self.logo_dir.as_deref() else {
            self.message = Some("The logos folder is unavailable.".to_string());
            return;
        };
        match target.clear(logo_dir) {
            Ok(_) => self.refresh_category_image(),
            Err(error) => {
                crate::note(&format!("custom art could not be cleared: {error}"));
                self.message = Some("Degauss could not clear the custom image.".to_string());
                self.screen = Screen::Browse;
                self.apply_geometry();
                self.dirty = true;
            }
        }
    }

    fn open_game_data_source(&mut self) {
        let Some(system_id) = self.context_system_id().map(str::to_string) else {
            return;
        };
        if !crate::artwork_pack::supports(&system_id) {
            return;
        }
        self.source_system_id = Some(system_id.clone());
        let mode = crate::artwork_source::mode(&self.settings, &system_id);
        self.menu = source_choice_rows(mode);
        if mode == crate::artwork_source::Mode::Automatic {
            let effective = if self.source_problem(&system_id).is_some() {
                "Unresolved"
            } else if self.pack_selected(&system_id) {
                SOURCE_ARTWORK_PACK
            } else {
                SOURCE_GAMELIST
            };
            self.menu[0] = format!("Automatic (Current: {effective})");
        }
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.menu_list.select(match mode {
            crate::artwork_source::Mode::Automatic => 0,
            crate::artwork_source::Mode::Gamelist => 1,
            crate::artwork_source::Mode::ArtworkPack => 2,
        });
        self.screen = Screen::GameDataSource;
        self.message = None;
        self.apply_geometry();
        self.dirty = true;
    }

    fn source_choice_is_shared(&self) -> bool {
        self.source_system_id
            .as_deref()
            .and_then(crate::artwork_pack::source_group)
            == Some("NeoGeo")
    }

    fn compact_provider_progress(&self) -> bool {
        self.provider_pending_open.is_some()
            || self.provider_job_purpose == ProviderJobPurpose::LocationDiscovery
    }

    fn source_progress_subject(&self) -> &'static str {
        if self.provider_job_purpose == ProviderJobPurpose::LocationDiscovery {
            "Location"
        } else {
            "System"
        }
    }

    fn show_source_progress(&mut self) {
        self.menu = source_progress_rows(
            &self.source_progress,
            self.compact_provider_progress(),
            self.source_progress_subject(),
        )
        .into_iter()
        .map(|(label, _)| label)
        .collect();
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::SourceProgress;
        self.message = None;
        self.apply_geometry();
        self.dirty = true;
    }

    fn open_artwork_pack_locations(&mut self) {
        let Some(system_id) = self.source_system_id.clone() else {
            return;
        };
        let candidates =
            crate::artwork_pack::discover_candidate_roots(&system_id, &self.config.game_roots);

        if candidates.is_empty() {
            self.source_locations.clear();
            self.open_artwork_pack_directory(self.default_pack_browser_root(), true);
            return;
        }

        if self.provider_job.is_some()
            || !self.provider_requests.is_empty()
            || self.source_job.is_some()
            || self.build.is_some()
        {
            self.message = Some("Wait for the current library update to finish.".to_string());
            self.dirty = true;
            return;
        }

        let requests = candidates
            .iter()
            .map(|root| crate::provider_job::Request {
                system_id: system_id.clone(),
                system_name: root.to_string_lossy().into_owned(),
                docs_root: root.clone(),
                synopsis_language: self.scraper_settings.language.clone(),
                cache_dir: self.cache_dir.clone(),
                validate_location_only: true,
                cached_provider: None,
            })
            .collect();
        match crate::provider_job::start(requests) {
            Ok(job) => {
                self.source_locations = candidates;
                self.provider_job = Some(job);
                self.provider_job_purpose = ProviderJobPurpose::LocationDiscovery;
                self.provider_cancelling = false;
                self.source_progress = crate::source_cache::Progress {
                    current: "Artwork Pack locations".to_string(),
                    system: 0,
                    systems: self.source_locations.len(),
                    ..Default::default()
                };
                self.show_source_progress();
            }
            Err(error) => {
                crate::note(&format!("artwork pack location discovery: {error}"));
                self.message = Some(format!(
                    "Artwork Pack locations could not be checked.\n{}",
                    artwork_pack_error_action(&error)
                ));
                self.dirty = true;
            }
        }
    }

    fn show_artwork_pack_locations(&mut self) {
        let Some(system_id) = self.source_system_id.as_deref() else {
            return;
        };
        self.menu = self
            .source_locations
            .iter()
            .map(|root| pack_location_label(system_id, root))
            .collect();
        self.menu.push(CHOOSE_PACK_DIRECTORY.to_string());
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        if let Some(saved) =
            crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, system_id)
        {
            if let Some(index) = self.source_locations.iter().position(|root| root == saved) {
                self.menu_list.select(index);
            }
        }
        self.screen = Screen::ArtworkPackLocation;
        self.message = None;
        self.apply_geometry();
        self.dirty = true;
    }

    fn default_pack_browser_root(&self) -> PathBuf {
        pack_browser_root_at(Path::new("/media"), &self.config.game_roots)
    }

    fn directory_allowed(&self, directory: &Path) -> bool {
        directory_allowed_for(directory, &self.config.game_roots)
    }

    fn open_artwork_pack_directory(&mut self, directory: PathBuf, initial: bool) -> bool {
        let canonical = std::fs::canonicalize(&directory).unwrap_or(directory);
        if !self.directory_allowed(&canonical) {
            self.message =
                Some("That directory is outside the available MiSTer storage.".to_string());
            self.dirty = true;
            return false;
        }
        let mut history = if initial {
            vec![canonical.clone()]
        } else {
            self.source_directory_history.clone()
        };
        if !initial && history.last() != Some(&canonical) {
            if history.contains(&canonical) {
                self.message = Some("That directory leads back to one already open.".to_string());
                self.dirty = true;
                return false;
            }
            history.push(canonical.clone());
        }

        let valid_root = self
            .source_system_id
            .as_deref()
            .and_then(|system_id| crate::artwork_pack::normalize_docs_root(system_id, &canonical));
        let children =
            match artwork_directory_children(&canonical, &self.config.game_roots, &history) {
                Ok(children) => children,
                Err(error) => {
                    crate::note(&format!(
                        "artwork pack directory {}: {error}",
                        canonical.display()
                    ));
                    self.message = Some("That directory cannot be read.".to_string());
                    self.dirty = true;
                    return false;
                }
            };

        self.source_directory = Some(canonical);
        self.source_directory_history = history;
        self.source_directory_entries.clear();
        self.menu.clear();
        if let (Some(system_id), Some(root)) =
            (self.source_system_id.as_deref(), valid_root.as_ref())
        {
            self.menu.push(format!(
                "{USE_PACK_DIRECTORY} ({})",
                pack_folders_at(system_id, root)
            ));
            self.source_directory_entries.push(None);
        }
        for (name, path) in children {
            self.menu.push(format!("[ {name} ]"));
            self.source_directory_entries.push(Some(path));
        }
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.screen = Screen::ArtworkPackDirectory;
        self.message = None;
        self.apply_geometry();
        self.dirty = true;
        true
    }

    fn activate_artwork_pack_location(&mut self) {
        let selected = self.menu_list.selected();
        if selected < self.source_locations.len() {
            let root = self.source_locations[selected].clone();
            self.begin_source_switch(crate::source_cache::Target::ArtworkPack { docs_root: root });
        } else {
            self.open_artwork_pack_directory(self.default_pack_browser_root(), true);
        }
    }

    fn activate_artwork_pack_directory(&mut self) {
        let Some(choice) = self
            .source_directory_entries
            .get(self.menu_list.selected())
            .cloned()
        else {
            return;
        };
        match choice {
            Some(directory) => {
                self.open_artwork_pack_directory(directory, false);
            }
            None => {
                let Some(system_id) = self.source_system_id.as_deref() else {
                    return;
                };
                let Some(current) = self.source_directory.as_deref() else {
                    return;
                };
                let Some(root) = crate::artwork_pack::normalize_docs_root(system_id, current)
                else {
                    self.message =
                        Some("This directory does not contain a valid Artwork Pack.".to_string());
                    self.dirty = true;
                    return;
                };
                self.begin_source_switch(crate::source_cache::Target::ArtworkPack {
                    docs_root: root,
                });
            }
        }
    }

    fn leave_artwork_pack_directory(&mut self) {
        if self.source_directory.is_none() {
            self.open_artwork_pack_locations();
            return;
        }
        if self.source_directory_history.len() <= 1 {
            if self.source_locations.is_empty() {
                self.open_game_data_source();
            } else {
                self.show_artwork_pack_locations();
            }
            return;
        }
        let previous_history = self.source_directory_history.clone();
        self.source_directory_history.pop();
        let parent = self.source_directory_history.last().cloned();
        if !parent.is_some_and(|parent| self.open_artwork_pack_directory(parent, false)) {
            self.source_directory_history = previous_history;
        }
    }

    fn begin_source_switch(&mut self, target: crate::source_cache::Target) {
        let Some(group) = self
            .source_system_id
            .as_deref()
            .and_then(crate::artwork_pack::source_group)
        else {
            return;
        };
        let unchanged = !self.artwork_source_errors.contains_key(group)
            && match &target {
                crate::source_cache::Target::Gamelist => {
                    !self.effective_artwork_pack_roots.contains_key(group)
                }
                crate::source_cache::Target::ArtworkPack { docs_root } => self
                    .effective_artwork_pack_roots
                    .get(group)
                    .is_some_and(|root| Path::new(root) == docs_root),
            };
        if unchanged {
            match persist_source_mode(
                &self.settings,
                &self.settings_path,
                group,
                &target,
                self.source_switch_automatic,
            ) {
                Ok((settings, _, warning)) => {
                    self.settings = settings;
                    self.open_game_data_source();
                    self.message = warning;
                }
                Err(error) => self.message = Some(source_change_failure_message(&error)),
            }
            self.source_switch_automatic = false;
            self.dirty = true;
            return;
        }
        if self.source_job.is_some()
            || self.provider_job.is_some()
            || self.build.is_some()
            || self.refreshing.is_some()
        {
            self.message = Some("Wait for the current library update to finish.".to_string());
            self.dirty = true;
            return;
        }
        let Some(system_id) = self.source_system_id.as_deref() else {
            return;
        };
        let Some(group) = crate::artwork_pack::source_group(system_id) else {
            return;
        };
        self.start_source_job(target, group, SourceOperation::Switch);
    }

    fn begin_source_recovery(&mut self, system_id: &str, purpose: SourceRecoveryPurpose) -> bool {
        let Some(group) = crate::artwork_pack::source_group(system_id) else {
            return false;
        };
        if (purpose == SourceRecoveryPurpose::OpenSystem
            && self.source_recovery_suppressed.contains(group))
            || self.source_job.is_some()
            || self.provider_job.is_some()
            || self.build.as_ref().is_some_and(|build| {
                purpose != SourceRecoveryPurpose::FullBuild || build.job.is_some()
            })
            || self.refreshing.is_some()
        {
            return false;
        }
        if purpose != SourceRecoveryPurpose::OpenSystem {
            self.source_recovery_suppressed.remove(group);
        }
        let Some(docs_root) =
            crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, system_id)
                .map(Path::to_path_buf)
        else {
            return false;
        };
        if purpose != SourceRecoveryPurpose::FullBuild {
            self.source_recovery_warnings.clear();
        }
        self.source_system_id = Some(system_id.to_string());
        self.start_source_job(
            crate::source_cache::Target::ArtworkPack { docs_root },
            group,
            SourceOperation::Recover(purpose),
        );
        self.source_job.is_some()
    }

    fn start_source_job(
        &mut self,
        target: crate::source_cache::Target,
        group: &str,
        operation: SourceOperation,
    ) {
        let require_usable_provider = matches!(operation, SourceOperation::Switch);
        let systems: Vec<crate::source_cache::System> = self
            .all_systems
            .iter()
            .filter(|system| crate::artwork_pack::source_group(&system.def.id) == Some(group))
            .map(|system| crate::source_cache::System {
                id: system.def.id.clone(),
                name: system.name().to_string(),
                config: system.to_config(),
            })
            .collect();
        let request = crate::source_cache::Request {
            target,
            systems,
            names: self.names.clone(),
            synopsis_language: self.scraper_settings.language.clone(),
            cache_dir: self.cache_dir.clone(),
            require_usable_provider,
        };
        match crate::source_cache::start(request) {
            Ok(job) => {
                self.source_job = Some(job);
                self.source_operation = Some(operation);
                self.source_progress = crate::source_cache::Progress::default();
                self.source_cancelling = false;
                if operation == SourceOperation::Recover(SourceRecoveryPurpose::FullBuild)
                    && self.build.is_some()
                {
                    self.publish_index_progress();
                } else {
                    self.show_source_progress();
                }
            }
            Err(error) => {
                self.source_operation = None;
                crate::note(&format!("game source  could not start: {error}"));
                self.message = Some(match operation {
                    SourceOperation::Switch => source_change_failure_message(&error),
                    SourceOperation::Recover(_) => format!(
                        "Artwork Pack refresh could not start.\n{}",
                        artwork_pack_error_action(&error)
                    ),
                });
                self.dirty = true;
            }
        }
    }

    fn poll_source_cache(&mut self) {
        loop {
            let event = self
                .source_job
                .as_mut()
                .and_then(crate::source_cache::Job::try_recv);
            let Some(event) = event else {
                break;
            };
            match event {
                crate::source_cache::Event::Progress(progress) => {
                    if let Some(build) = self.build.as_mut().filter(|_| {
                        self.source_operation
                            == Some(SourceOperation::Recover(SourceRecoveryPurpose::FullBuild))
                    }) {
                        build.folders = progress.folders;
                        build.games = progress.games;
                        build.displayed = None;
                    }
                    self.source_progress = progress;
                    self.dirty = true;
                }
                crate::source_cache::Event::Cancelled(progress) => {
                    self.source_job = None;
                    self.source_progress = progress;
                    self.source_cancelling = false;
                    let operation = self.source_operation.take();
                    self.finish_source_terminal(operation, None, true);
                    break;
                }
                crate::source_cache::Event::Failed { error, progress } => {
                    self.source_job = None;
                    self.source_progress = progress;
                    self.source_cancelling = false;
                    crate::note(&format!("game source  {error}"));
                    let operation = self.source_operation.take();
                    self.finish_source_terminal(operation, Some(error), false);
                    break;
                }
                crate::source_cache::Event::Staged {
                    target,
                    prepared,
                    providers,
                    progress,
                    warnings,
                } => {
                    if self.source_cancelling && self.build.is_some() {
                        self.source_job = None;
                        self.source_progress = progress;
                        self.source_cancelling = false;
                        let operation = self.source_operation.take();
                        self.finish_source_terminal(operation, None, true);
                        break;
                    }
                    self.source_job = None;
                    self.source_progress = progress;
                    self.source_cancelling = false;
                    match self.source_operation.take() {
                        Some(SourceOperation::Switch) => {
                            self.finish_source_switch(target, prepared, providers, warnings)
                        }
                        Some(SourceOperation::Recover(purpose)) => self
                            .finish_source_recovery(target, prepared, providers, warnings, purpose),
                        None => {
                            crate::note("game source  worker completed without an operation");
                            self.screen = Screen::Browse;
                            self.message = Some(
                                "The library update finished in an unknown state. Restart Degauss before changing the data source."
                                    .to_string(),
                            );
                            self.dirty = true;
                        }
                    }
                    break;
                }
            }
        }
    }

    fn poll_provider_job(&mut self) {
        loop {
            let event = self
                .provider_job
                .as_mut()
                .and_then(crate::provider_job::Job::try_recv);
            let Some(event) = event else {
                break;
            };
            match event {
                crate::provider_job::Event::Progress(progress) => {
                    if self.provider_pending_open.is_some()
                        || self.provider_job_purpose == ProviderJobPurpose::LocationDiscovery
                    {
                        self.source_progress.current = progress.current;
                        self.source_progress.system = progress.system;
                        self.source_progress.systems = progress.systems;
                        self.dirty = true;
                    }
                }
                crate::provider_job::Event::Loaded {
                    snapshots,
                    progress,
                } => {
                    let purpose = self.provider_job_purpose;
                    self.provider_job = None;
                    self.provider_job_purpose = ProviderJobPurpose::Runtime;
                    self.provider_loading.clear();
                    self.provider_cancelling = false;
                    self.source_progress.current = progress.current;
                    self.source_progress.system = progress.system;
                    self.source_progress.systems = progress.systems;
                    if purpose == ProviderJobPurpose::LocationDiscovery {
                        let mut seen = HashSet::new();
                        self.source_locations = snapshots
                            .into_iter()
                            .filter_map(|snapshot| {
                                let provider = snapshot.provider;
                                if provider.health.usable() {
                                    seen.insert(provider.docs_root.clone())
                                        .then_some(provider.docs_root)
                                } else {
                                    crate::note(&format!(
                                        "artwork pack candidate {}: {}",
                                        provider.docs_root.display(),
                                        provider.status_line()
                                    ));
                                    None
                                }
                            })
                            .collect();
                        if self.source_locations.is_empty() {
                            self.open_artwork_pack_directory(
                                self.default_pack_browser_root(),
                                true,
                            );
                            self.message = Some(
                                "No complete Artwork Pack was found. Choose its docs directory."
                                    .to_string(),
                            );
                        } else {
                            self.show_artwork_pack_locations();
                        }
                        self.start_provider_job_if_ready();
                        break;
                    }
                    self.store_provider_snapshots(snapshots);

                    if let Some(system_id) = self.provider_pending_open.clone() {
                        if let Some(provider) = self.artwork_provider_cache.get(&system_id).cloned()
                        {
                            self.provider_pending_open = None;
                            self.provider_validated.remove(&system_id);
                            self.open_system_now_with_provider(Some(provider));
                        } else if !self
                            .provider_requests
                            .iter()
                            .any(|request| request.system_id == system_id)
                        {
                            self.provider_pending_open = None;
                            self.screen = Screen::Browse;
                            self.message = Some(
                                "The selected Artwork Pack changed before it could be read. Choose it again or restart Degauss."
                                    .to_string(),
                            );
                            self.apply_geometry();
                            self.dirty = true;
                        }
                    } else if self.browsing == Browsing::Games && self.in_favorites() {
                        self.relist_here();
                    }
                    self.start_provider_job_if_ready();
                    break;
                }
                crate::provider_job::Event::Cancelled(progress) => {
                    let purpose = self.provider_job_purpose;
                    self.provider_job = None;
                    self.provider_job_purpose = ProviderJobPurpose::Runtime;
                    let loading = std::mem::take(&mut self.provider_loading);
                    self.invalidate_provider_ids(&loading);
                    self.provider_requests.clear();
                    self.provider_cancelling = false;
                    self.source_progress.current = progress.current;
                    if purpose == ProviderJobPurpose::LocationDiscovery {
                        self.source_locations.clear();
                        self.open_game_data_source();
                        self.message = Some("Artwork Pack search cancelled.".to_string());
                    } else if self.provider_pending_open.take().is_some() {
                        self.screen = Screen::Browse;
                        self.message = Some("Artwork Pack reading cancelled.".to_string());
                        self.apply_geometry();
                        self.dirty = true;
                    }
                    break;
                }
                crate::provider_job::Event::Failed { error, progress } => {
                    let purpose = self.provider_job_purpose;
                    self.provider_job = None;
                    self.provider_job_purpose = ProviderJobPurpose::Runtime;
                    let loading = std::mem::take(&mut self.provider_loading);
                    self.invalidate_provider_ids(&loading);
                    self.provider_cancelling = false;
                    self.source_progress.current = progress.current;
                    crate::note(&format!("artwork pack provider: {error}"));
                    if purpose == ProviderJobPurpose::LocationDiscovery {
                        self.source_locations.clear();
                        self.open_game_data_source();
                        self.message = Some(format!(
                            "Artwork Pack locations could not be checked.\n{}",
                            artwork_pack_error_action(&error)
                        ));
                    } else if self.provider_pending_open.take().is_some() {
                        self.screen = Screen::Browse;
                        self.message = Some(format!(
                            "Artwork Pack read failed; system not opened.\n{}",
                            artwork_pack_error_action(&error)
                        ));
                        self.apply_geometry();
                        self.dirty = true;
                    } else {
                        self.message = Some(format!(
                            "Artwork Pack background refresh failed.\n{}",
                            artwork_pack_error_action(&error)
                        ));
                        self.dirty = true;
                    }
                    self.start_provider_job_if_ready();
                    break;
                }
            }
        }
    }

    fn finish_source_terminal(
        &mut self,
        operation: Option<SourceOperation>,
        error: Option<crate::error::DegaussError>,
        cancelled: bool,
    ) {
        match operation {
            Some(SourceOperation::Switch) => {
                self.open_game_data_source();
                self.message = Some(match error {
                    Some(error) => source_change_failure_message(&error),
                    None if cancelled => "Game data source change cancelled.".to_string(),
                    None => "Game data source was not changed.".to_string(),
                });
            }
            Some(SourceOperation::Recover(purpose)) => {
                if let Some(system_id) = self.source_system_id.as_deref() {
                    if let Some(group) = crate::artwork_pack::source_group(system_id) {
                        self.source_recovery_suppressed.insert(group.to_string());
                    }
                }
                self.screen = Screen::Browse;
                let name = self
                    .source_system_id
                    .as_deref()
                    .and_then(|id| {
                        self.all_systems
                            .iter()
                            .find(|system| system.def.id == id)
                            .map(|system| system.name().to_string())
                    })
                    .unwrap_or_else(|| "this system".to_string());
                if purpose == SourceRecoveryPurpose::FullBuild {
                    if let Some(build) = self.build.as_mut() {
                        if cancelled {
                            build.cancelling = true;
                        } else {
                            build.done += 1;
                        }
                        build.current = None;
                        build.folders = 0;
                        build.games = 0;
                        build.displayed = None;
                        if let Some(error) = error {
                            self.build_warning(format!("{name}: {error}"));
                        } else if !cancelled {
                            self.build_warning(format!(
                                "{name}: Artwork Pack cache preparation stopped without a result"
                            ));
                        }
                        self.message = None;
                        self.publish_index_progress();
                        self.dirty = true;
                        return;
                    }
                    if cancelled {
                        self.source_recovery_queue.clear();
                        self.message = Some(format!(
                            "System-list rebuild finished, but the Artwork Pack cache refresh for {name} was cancelled. The previous complete cache remains in use."
                        ));
                    } else if let Some(error) = error.as_ref() {
                        self.source_recovery_warnings
                            .push(format!("{name}: {}", artwork_pack_error_action(error)));
                        if !self.start_next_source_recovery() {
                            self.message = Some(format!(
                                "Library rebuild finished with problems:\n{}",
                                self.source_recovery_warnings.join("\n")
                            ));
                        }
                    } else {
                        self.source_recovery_queue.clear();
                        self.message = Some(
                            "Library rebuild finished, but an Artwork Pack cache stopped without a result."
                                .to_string(),
                        );
                    }
                    if self.source_job.is_none() {
                        self.screen = self.index_return_screen;
                    }
                    self.apply_geometry();
                    self.dirty = true;
                    return;
                }
                let open_system_cache_available =
                    self.source_system_id.as_deref().is_some_and(|id| {
                        crate::cache::load_artwork_pack_system(&self.cache_dir, id).is_some()
                    });
                self.source_recovery_queue.clear();
                if purpose == SourceRecoveryPurpose::OpenSystem {
                    let provider = self
                        .source_system_id
                        .as_deref()
                        .and_then(|id| self.artwork_provider_cache.get(id))
                        .cloned();
                    self.open_system_now_with_provider(provider);
                }
                self.message = Some(match (purpose, error) {
                    (SourceRecoveryPurpose::OpenSystem, Some(error))
                        if !open_system_cache_available =>
                    {
                        format!(
                            "{name} Pack cache failed; system not opened.\n{}",
                            artwork_pack_error_action(&error)
                        )
                    }
                    (SourceRecoveryPurpose::OpenSystem, Some(_)) => format!(
                        "{name} Pack cache was not refreshed.\nThe previous complete cache remains in use."
                    ),
                    (SourceRecoveryPurpose::OpenSystem, None)
                        if cancelled && !open_system_cache_available =>
                    {
                        format!(
                            "Artwork Pack cache refresh for {name} was cancelled, so the system was not opened. Reopen it to retry."
                        )
                    }
                    (SourceRecoveryPurpose::OpenSystem, None) if cancelled => format!(
                        "Artwork Pack cache refresh for {name} cancelled. Games remain browseable, but some artwork matches may be unavailable until it is refreshed."
                    ),
                    (SourceRecoveryPurpose::RefreshSystem, Some(error)) => format!(
                        "{name} list was not rebuilt.\n{}\nThe previous complete cache remains in use.",
                        artwork_pack_error_action(&error)
                    ),
                    (SourceRecoveryPurpose::RefreshSystem, None) if cancelled => format!(
                        "{name} list rebuild cancelled. The previous complete cache remains in use."
                    ),
                    (SourceRecoveryPurpose::FullBuild, _) => {
                        unreachable!("full-build terminals return above")
                    }
                    (_, None) => "Artwork Pack cache was not refreshed.".to_string(),
                });
                self.apply_geometry();
                self.dirty = true;
            }
            None => {
                self.screen = Screen::Browse;
                self.message = Some(match error {
                    Some(error) => format!(
                        "Library update failed.\n{}",
                        artwork_pack_error_action(&error)
                    ),
                    None => "Library update stopped without a result.".to_string(),
                });
                self.apply_geometry();
                self.dirty = true;
            }
        }
    }

    fn finish_source_recovery(
        &mut self,
        target: crate::source_cache::Target,
        prepared: crate::cache::PreparedCacheGroup,
        providers: Vec<crate::artwork_pack::Provider>,
        scan_warnings: Vec<String>,
        purpose: SourceRecoveryPurpose,
    ) {
        let crate::source_cache::Target::ArtworkPack { .. } = target else {
            self.finish_source_terminal(
                Some(SourceOperation::Recover(purpose)),
                Some(crate::error::DegaussError::unsupported(
                    "Artwork Pack cache",
                    "the recovery worker returned the wrong source",
                )),
                false,
            );
            return;
        };
        let mut next_index = self
            .build
            .as_ref()
            .map(|build| build.index.clone())
            .or_else(|| self.index.clone())
            .unwrap_or_default();
        update_index_summaries(&mut next_index, &self.all_systems, prepared.caches());
        let (caches, install_warnings) = match prepared
            .with_index(&self.cache_dir, &next_index)
            .and_then(|prepared| prepared.install())
        {
            Ok(installed) => installed,
            Err(error) => {
                crate::note(&format!(
                    "game source  cache recovery install failed: {error}"
                ));
                self.finish_source_terminal(
                    Some(SourceOperation::Recover(purpose)),
                    Some(error),
                    false,
                );
                return;
            }
        };
        // What the scan left out comes first: it is the cause, and a later
        // storage warning must not push it out of the message.
        for warning in scan_warnings.into_iter().chain(install_warnings) {
            crate::note(&format!("cache        recovery warning: {warning}"));
            self.source_recovery_warnings.push(warning);
        }

        let group = self
            .source_system_id
            .as_deref()
            .and_then(crate::artwork_pack::source_group);
        if let Some(group) = group {
            self.invalidate_artwork_provider_group(group);
            self.source_recovery_suppressed.remove(group);
        }
        self.store_source_providers(providers);
        if purpose == SourceRecoveryPurpose::FullBuild {
            if let Some(build) = self.build.as_mut() {
                build.index = next_index;
                for staged in caches {
                    let summary = build.index.systems[&staged.id];
                    build.done += 1;
                    build.folders_done += staged.cache.folders.len();
                    build.games_done += summary.games;
                    crate::note(&format!(
                        "index {}: Artwork Pack cache prepared, {} games",
                        staged.id, summary.games
                    ));
                    if self.open_system.as_deref() == Some(staged.id.as_str()) {
                        self.artwork_provider =
                            self.artwork_provider_cache.get(&staged.id).cloned();
                        self.system_cache = Some(staged.cache);
                        self.library = None;
                    }
                }
                build.left.retain(|at| {
                    crate::artwork_pack::source_group(&self.all_systems[*at].def.id) != group
                });
                build.current = None;
                build.folders = 0;
                build.games = 0;
                build.displayed = None;
                for warning in std::mem::take(&mut self.source_recovery_warnings) {
                    self.build_warning(warning);
                }
                self.message = None;
                self.publish_index_progress();
                self.dirty = true;
                return;
            }
        }
        self.index = Some(next_index);
        self.apply_index();
        self.correct_system_counts();
        self.rebuild_system_list();
        self.screen = Screen::Browse;
        if let Some(group) = group {
            self.reload_open_pack_group(group);
        }

        match purpose {
            SourceRecoveryPurpose::OpenSystem => {
                self.open_system_now();
                if !self.source_recovery_warnings.is_empty() {
                    self.message = Some(
                        "Artwork Pack cache refreshed with problems.\nSee degauss.log for details."
                            .to_string(),
                    );
                } else if self.message.is_none() {
                    self.message = Some("Artwork Pack cache refreshed.".to_string());
                }
            }
            SourceRecoveryPurpose::RefreshSystem => {
                let name = self
                    .source_system_id
                    .as_deref()
                    .and_then(|id| {
                        self.all_systems
                            .iter()
                            .find(|system| system.def.id == id)
                            .map(|system| system.name().to_string())
                    })
                    .unwrap_or_else(|| "System".to_string());
                self.message = Some(if self.source_recovery_warnings.is_empty() {
                    format!("{name} list rebuilt.")
                } else {
                    format!("{name} list rebuilt with problems.\nSee degauss.log for details.")
                });
            }
            SourceRecoveryPurpose::FullBuild => {
                if !self.start_next_source_recovery() {
                    self.screen = self.index_return_screen;
                    self.message = Some(if self.source_recovery_warnings.is_empty() {
                        "Library rebuild complete.".to_string()
                    } else {
                        format!(
                            "Library rebuild finished with problems:\n{}",
                            self.source_recovery_warnings.join("\n")
                        )
                    });
                }
            }
        }
        self.apply_geometry();
        self.dirty = true;
    }

    fn reload_open_pack_group(&mut self, group: &str) {
        let Some(id) = self.open_system.clone().filter(|id| {
            crate::artwork_pack::source_group(id.as_str()) == Some(group) && self.pack_selected(id)
        }) else {
            return;
        };
        let data = crate::cache::load_artwork_pack_data(&self.cache_dir, &id);
        self.system_cache = data.map(|data| data.cache);
        self.library = None;
        let root = crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, &id)
            .map(Path::to_path_buf);
        self.artwork_provider = root
            .as_deref()
            .and_then(|root| self.cached_artwork_provider(&id, root, false));
        if self.browsing == Browsing::Games {
            self.relist_here();
        }
    }

    fn start_next_source_recovery(&mut self) -> bool {
        while let Some(group) = self.source_recovery_queue.pop_front() {
            let system_id = self
                .all_systems
                .iter()
                .find(|system| {
                    crate::artwork_pack::source_group(&system.def.id) == Some(group.as_str())
                        && self.pack_selected(&system.def.id)
                })
                .map(|system| system.def.id.clone());
            if let Some(system_id) = system_id {
                return self.begin_source_recovery(&system_id, SourceRecoveryPurpose::FullBuild);
            }
        }
        false
    }

    fn finish_source_switch(
        &mut self,
        target: crate::source_cache::Target,
        prepared: crate::cache::PreparedCacheGroup,
        providers: Vec<crate::artwork_pack::Provider>,
        mut warnings: Vec<String>,
    ) {
        let Some(system_id) = self.source_system_id.clone() else {
            return;
        };
        let Some(group) = crate::artwork_pack::source_group(&system_id) else {
            return;
        };
        let caches = match prepared.install() {
            Ok((caches, installed)) => {
                warnings.extend(installed);
                caches
            }
            Err(error) => {
                crate::note(&format!("game source  cache install failed: {error}"));
                self.open_game_data_source();
                self.message = Some(source_change_failure_message(&error));
                return;
            }
        };

        let (settings, label, settings_warning) = match persist_source_mode(
            &self.settings,
            &self.settings_path,
            group,
            &target,
            self.source_switch_automatic,
        ) {
            Ok(saved) => saved,
            Err(error) => {
                crate::note(&format!("game source  settings unchanged: {error}"));
                self.open_game_data_source();
                self.message = Some(source_change_failure_message(&error));
                return;
            }
        };
        warnings.extend(settings_warning);
        self.settings = settings;
        self.source_switch_automatic = false;
        self.artwork_source_errors.remove(group);
        match &target {
            crate::source_cache::Target::Gamelist => {
                self.effective_artwork_pack_roots.remove(group);
            }
            crate::source_cache::Target::ArtworkPack { docs_root } => {
                self.effective_artwork_pack_roots
                    .insert(group.into(), docs_root.to_string_lossy().into_owned());
            }
        }
        if let Some(index) = self.index.as_mut() {
            update_index_summaries(index, &self.all_systems, &caches);
            if let Err(error) = crate::cache::save_index(&self.cache_dir, index) {
                crate::note(&format!("cache        source index not written: {error}"));
                warnings.push(format!("the system index was not saved: {error}"));
            }
        }
        self.invalidate_artwork_provider_group(group);
        self.store_source_providers(providers);
        self.source_recovery_suppressed.remove(group);
        self.covers = self.fresh_cover_cache();
        self.group_covers = self.fresh_group_cover_cache();
        self.gallery_covers = self.fresh_gallery_cover_cache();
        self.saver_candidates = None;
        self.saver_pool.clear();
        self.saver_queue.clear();

        if self
            .open_system
            .as_deref()
            .is_some_and(|id| crate::artwork_pack::source_group(id) == Some(group))
        {
            let id = self.open_system.clone().unwrap_or_default();
            match target {
                crate::source_cache::Target::Gamelist => {
                    self.system_cache = crate::cache::load_system(&self.cache_dir, &id);
                }
                crate::source_cache::Target::ArtworkPack { .. } => {
                    let data = crate::cache::load_artwork_pack_data(&self.cache_dir, &id);
                    self.system_cache = data.map(|data| data.cache);
                }
            }
            self.library = None;
            let pack_root =
                crate::artwork_pack::selected_root(&self.effective_artwork_pack_roots, &id)
                    .map(Path::to_path_buf);
            self.artwork_provider = pack_root
                .as_deref()
                .and_then(|root| self.cached_artwork_provider(&id, root, false));
            self.provider_validated.remove(&id);
            if self.browsing == Browsing::Games {
                self.relist_here();
            }
        }
        self.correct_system_counts();
        self.rebuild_system_list();
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.touch_selection();
        for warning in &warnings {
            crate::note(&format!("game source  installed with warning: {warning}"));
        }
        self.message = Some(match warnings.is_empty() {
            false => format!("Now using {label}.\nFinished with problems; see degauss.log."),
            true => format!("Now using {label}."),
        });
        self.dirty = true;
    }

    /// What can be done with the folder on screen.
    fn open_context(&mut self) {
        self.context_page = None;
        self.refresh_context();
    }

    fn context_is_root(&self) -> bool {
        self.browsing != Browsing::Categories && self.context_page.is_none()
    }

    fn remember_context_selection(&mut self) {
        if let Some(page) = self.context_page {
            self.context_page_selections[page.index()] = self.menu_list.selected();
        } else {
            self.context_root_selection = self.menu_list.selected();
        }
    }

    fn show_context_page(&mut self, page: Option<ContextPage>) {
        self.context_page = page;
        self.menu = if self.browsing == Browsing::Categories {
            self.context_page = None;
            self.context_actions.clone()
        } else if let Some(page) = page {
            self.context_actions
                .iter()
                .filter(|action| page.contains(action))
                .cloned()
                .collect()
        } else {
            ContextPage::ALL
                .into_iter()
                .filter(|page| {
                    self.context_actions
                        .iter()
                        .any(|action| page.contains(action))
                })
                .map(|page| page.label().to_string())
                .collect()
        };
        let selected = self
            .context_page
            .map(|page| self.context_page_selections[page.index()])
            .unwrap_or(self.context_root_selection);
        self.menu_list = ListState::new(self.menu.len(), self.geometry.visible);
        self.menu_list.select(selected);
        self.screen = Screen::Context;
        self.apply_geometry();
        self.dirty = true;
    }

    fn reopen_context_for(&mut self, action: &str) {
        self.context_page = if self.browsing == Browsing::Categories {
            None
        } else {
            ContextPage::ALL
                .into_iter()
                .find(|page| page.contains(action))
        };
        self.refresh_context();
        if let Some(index) = self.menu.iter().position(|entry| entry == action) {
            self.menu_list.select(index);
        }
    }

    fn refresh_context(&mut self) {
        // Inside the Favourites shelf the rows ARE the favourite files, so
        // the target-keyed lookup below answers false for every one of
        // them and the menu offered to Add what is already there. On the
        // shelf the answer is known without asking.
        let favorite = if self.in_favorites() {
            self.selected_game().map(|_| true)
        } else {
            self.selected_game().map(|path| self.favorites.holds(&path))
        };
        let context_system = self.context_system_id().map(str::to_string);
        let pack_selected = context_system
            .as_deref()
            .is_some_and(|system| self.pack_selected(system));
        let scrape_scope = !pack_selected
            && match self.browsing {
                Browsing::Systems => self
                    .systems
                    .get(self.system_list.selected())
                    .is_some_and(|system| !is_favorites(system.category())),
                Browsing::Games => self
                    .open_system_ref()
                    .is_some_and(|system| !is_favorites(system.category())),
                Browsing::Categories => false,
            };
        let scrape_game = scrape_scope
            && self.browsing == Browsing::Games
            && self
                .here
                .get(self.game_list.selected())
                .is_some_and(|row| matches!(row.kind, browse::Kind::Play(_)));
        let image_override = self.selected_image_target().map(|target| {
            self.logo_dir
                .as_deref()
                .is_some_and(|logo_dir| target.has_override(logo_dir))
        });
        let game_data_source = context_system
            .as_deref()
            .is_some_and(crate::artwork_pack::supports);
        self.context_actions = context_entries(
            self.browsing,
            !self.filter.is_empty(),
            favorite,
            self.selected_hidden(),
            self.has_custom_view(),
            ContextActions {
                scrape_scope,
                scrape_game,
                image_override,
                game_data_source,
                core_version: self.core_system_id().is_some(),
                core_version_override: self
                    .core_system_id()
                    .is_some_and(|id| self.settings.core_choices.contains_key(&id)),
            },
        );
        if self.context_actions.is_empty() {
            return;
        }
        self.show_context_page(self.context_page);
    }

    fn scraper_scope_from_context(&self, choice: &str) -> Option<crate::scraper::Scope> {
        match choice {
            SCRAPE_SYSTEM => {
                let system = self.systems.get(self.system_list.selected())?;
                if is_favorites(system.category()) || system.paths.is_empty() {
                    return None;
                }
                let place = if system.paths.len() == 1 {
                    Place::Dir(system.paths[0].clone())
                } else {
                    Place::Roots
                };
                Some(crate::scraper::Scope::System {
                    system_id: system.def.id.clone(),
                    place,
                    display_name: system.name().to_string(),
                })
            }
            SCRAPE_FOLDER => {
                let system_id = self.open_system.clone()?;
                if self.in_favorites() {
                    return None;
                }
                let (place, display_name) = match self.here.get(self.game_list.selected()) {
                    Some(row) => match &row.kind {
                        browse::Kind::Enter(place) => (place.clone(), row.name.clone()),
                        browse::Kind::Play(_) => {
                            (self.trail.last()?.place.clone(), self.here_label())
                        }
                    },
                    None => (self.trail.last()?.place.clone(), self.here_label()),
                };
                Some(crate::scraper::Scope::Folder {
                    system_id,
                    place,
                    display_name,
                })
            }
            SCRAPE_GAME => {
                let system_id = self.open_system.clone()?;
                if self.in_favorites() {
                    return None;
                }
                let row = self.here.get(self.game_list.selected())?;
                let browse::Kind::Play(launch) = &row.kind else {
                    return None;
                };
                Some(crate::scraper::Scope::Game {
                    system_id,
                    launch: launch.clone(),
                    title: row.name.clone(),
                })
            }
            _ => None,
        }
    }

    fn open_scraper(&mut self, scope: crate::scraper::Scope, return_to: Screen) {
        let problem = match &scope {
            crate::scraper::Scope::All => self
                .artwork_source_errors
                .values()
                .next()
                .map(String::as_str),
            crate::scraper::Scope::System { system_id, .. }
            | crate::scraper::Scope::Folder { system_id, .. }
            | crate::scraper::Scope::Game { system_id, .. } => self.source_problem(system_id),
        };
        if let Some(problem) = problem {
            self.message = Some(format!(
                "Scraping cannot start while a game data source is unresolved:\n{problem}"
            ));
            self.dirty = true;
            return;
        }
        let only_artwork_pack = scraper_scope_only_artwork_pack(
            &scope,
            &self.all_systems,
            &self.effective_artwork_pack_roots,
        );
        if !only_artwork_pack {
            if let Some(problem) = &self.scraper_settings_problem {
                self.message = Some(format!(
                    "ScreenScraper settings could not be loaded. Fix or remove screenscraper.toml.\n\n{problem}"
                ));
                self.dirty = true;
                return;
            }
        }
        if self.scraper_job.is_some()
            || self.scraper_pending_terminal.is_some()
            || self.scraper_cache_refresh_active
        {
            return;
        }
        self.scraper_scope = scope;
        self.scraper_return = return_to;
        self.scraper_list = ListState::new(
            scraper_rows(&self.scraper_scope).len(),
            self.geometry.visible,
        );
        self.scraper_matches_return = Screen::ScraperProgress;
        self.scraper_matches.clear();
        self.scraper_match_list = ListState::new(0, self.geometry.visible);
        self.scraper_search_term = match &self.scraper_scope {
            crate::scraper::Scope::Game { title, .. } => title.clone(),
            _ => String::new(),
        };
        self.scraper_search_job = None;
        self.scraper_search_status.clear();
        self.scraper_preview_job = None;
        self.scraper_preview_match_id = None;
        self.scraper_preview_image = None;
        self.scraper_preview_caption.clear();
        self.scraper_manual_resolution_eligible = false;
        self.scraper_open_matches_after_refresh = false;
        self.scraper_terminal = None;
        self.message = None;
        self.screen = Screen::Scraper;
        self.apply_geometry();
        self.dirty = true;
    }

    fn scraper_scope_label(&self) -> String {
        let room = if self.width < 480 { 24 } else { 48 };
        shortened_label(self.scraper_scope.label(), room)
    }

    fn scraper_system_id(&self) -> Option<&str> {
        match &self.scraper_scope {
            crate::scraper::Scope::All => None,
            crate::scraper::Scope::System { system_id, .. }
            | crate::scraper::Scope::Folder { system_id, .. }
            | crate::scraper::Scope::Game { system_id, .. } => Some(system_id),
        }
    }

    fn scraper_search_settings(&self) -> crate::scraper::ScraperSettings {
        let mut settings = self.scraper_settings.clone();
        if let Some(system_id) = self.scraper_system_id() {
            settings.media_type = settings.media_type_for(system_id).to_string();
        }
        settings
    }

    fn scraper_value(&self, row: ScraperRow) -> String {
        match row {
            ScraperRow::Username if self.scraper_settings.username.is_empty() => {
                "Not set".to_string()
            }
            ScraperRow::Username => self.scraper_settings.username.clone(),
            ScraperRow::Password if self.scraper_settings.password.is_empty() => {
                "Not set".to_string()
            }
            ScraperRow::Password => "Saved".to_string(),
            ScraperRow::Images => self.scraper_settings.image_policy.label().to_string(),
            ScraperRow::ImageType => {
                let media_type = match self.scraper_system_id() {
                    Some(id) => self.scraper_settings.media_type_override(id),
                    None => Some(self.scraper_settings.media_type.as_str()),
                };
                match media_type {
                    None => "Use Global".to_string(),
                    Some(value) if value.eq_ignore_ascii_case("ss") => "Screenshot".to_string(),
                    Some(value) if value.eq_ignore_ascii_case("box-2D") => {
                        "Box Art (2D)".to_string()
                    }
                    Some(value) if value.eq_ignore_ascii_case("box-3D") => {
                        "Box Art (3D)".to_string()
                    }
                    Some(value) => value.to_string(),
                }
            }
            ScraperRow::Metadata => self.scraper_settings.metadata_policy.label().to_string(),
            ScraperRow::Start | ScraperRow::Search => ">".to_string(),
            ScraperRow::ClearLogin | ScraperRow::Back => String::new(),
        }
    }

    fn scraper_selected_help(&self) -> &'static str {
        match scraper_rows(&self.scraper_scope).get(self.scraper_list.selected()).copied() {
            Some(ScraperRow::Username) => "Enter the username for the ScreenScraper account.",
            Some(ScraperRow::Password) => "Enter the account password. Saving it on the card requires storage confirmation.",
            Some(ScraperRow::Images) => "Choose no image downloads, missing images only, or replacement of existing images.",
            Some(ScraperRow::ImageType) if self.scraper_system_id().is_some() => "Choose this system's artwork type, or Use Global to follow Default Image.",
            Some(ScraperRow::ImageType) => "Choose the default artwork type for systems without their own image choice.",
            Some(ScraperRow::Metadata) => "Choose no metadata changes, filling empty fields, or replacement of existing fields.",
            Some(ScraperRow::Start) => "Start scraping this scope with the selected image and metadata settings.",
            Some(ScraperRow::Search) => "Search again, even if matched. Image and metadata settings still apply.",
            Some(ScraperRow::ClearLogin) => "Remove the saved ScreenScraper username and password after confirmation.",
            Some(ScraperRow::Back) => "Save these settings and return to the previous menu.",
            None => "",
        }
    }

    fn adjust_scraper(&mut self, delta: isize) {
        match scraper_rows(&self.scraper_scope)
            .get(self.scraper_list.selected())
            .copied()
        {
            Some(ScraperRow::Images) => {
                self.scraper_settings.image_policy = self.scraper_settings.image_policy.step(delta);
            }
            Some(ScraperRow::ImageType) => {
                let choices = [None, Some("ss"), Some("box-2D"), Some("box-3D")];
                let id = self.scraper_system_id().map(str::to_string);
                let choices = if id.is_some() {
                    &choices[..]
                } else {
                    &choices[1..]
                };
                let current = match id.as_deref() {
                    Some(id) => self.scraper_settings.media_type_override(id),
                    None => Some(self.scraper_settings.media_type.as_str()),
                };
                let at = choices.iter().position(|value| match (*value, current) {
                    (None, None) => true,
                    (Some(a), Some(b)) => a.eq_ignore_ascii_case(b),
                    _ => false,
                });
                let next = at.map_or_else(
                    || if delta < 0 { choices.len() - 1 } else { 0 },
                    |at| step(at, delta, choices.len()),
                );
                if let Some(id) = id {
                    if let Err(error) = self
                        .scraper_settings
                        .set_media_type_override(&id, choices[next])
                    {
                        self.message = Some(error.to_string());
                    }
                } else if let Some(value) = choices[next] {
                    self.scraper_settings.media_type = value.to_string();
                }
            }
            Some(ScraperRow::Metadata) => {
                self.scraper_settings.metadata_policy =
                    self.scraper_settings.metadata_policy.step(delta);
            }
            _ => return,
        }
        self.dirty = true;
    }

    fn activate_scraper_row(&mut self) {
        match scraper_rows(&self.scraper_scope)
            .get(self.scraper_list.selected())
            .copied()
        {
            Some(ScraperRow::Username) => self.open_scraper_keyboard(ScraperField::Username),
            Some(ScraperRow::Password) => self.open_scraper_keyboard(ScraperField::Password),
            Some(ScraperRow::Images | ScraperRow::ImageType | ScraperRow::Metadata) => {
                self.adjust_scraper(1)
            }
            Some(ScraperRow::Start) => self.confirm_scrape(),
            Some(ScraperRow::Search) => self.open_manual_scraper_search(),
            Some(ScraperRow::ClearLogin) => {
                self.pending = Some(Pending::ClearScraperLogin);
                self.message =
                    Some("Clear the saved ScreenScraper login?\n\nA yes, B no".to_string());
                self.dirty = true;
            }
            Some(ScraperRow::Back) => {
                let _ = self.go_back();
            }
            None => {}
        }
    }

    fn save_scraper_settings(&mut self) -> bool {
        self.replace_scraper_settings(self.scraper_settings.clone())
    }

    /// Persist a scraper-settings change before exposing it in the UI. This
    /// matters most for "Clear saved login": a failed card write must not
    /// claim that credentials disappeared while the old file still exists.
    fn replace_scraper_settings(&mut self, settings: crate::scraper::ScraperSettings) -> bool {
        match settings.save(&self.scraper_settings_path) {
            Ok(()) => {
                self.scraper_settings = settings;
                true
            }
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                false
            }
        }
    }

    fn open_scraper_keyboard(&mut self, field: ScraperField) {
        self.scraper_keyboard_field = field;
        self.scraper_keyboard_page = ScraperKeyboardPage::Lower;
        self.scraper_keyboard_draft = match field {
            ScraperField::Username => self.scraper_settings.username.clone(),
            ScraperField::Password => self.scraper_settings.password.clone(),
            ScraperField::SearchTerm => self.scraper_search_term.clone(),
        };
        let cells = scraper_keyboard_keys(self.scraper_keyboard_page).len();
        self.scraper_keyboard_list = ListState::new(cells, cells);
        self.scraper_keyboard_list
            .reshape(cells, SCRAPER_KEYBOARD_COLUMNS);
        self.screen = Screen::ScraperKeyboard;
        self.apply_geometry();
        self.dirty = true;
    }

    fn cycle_scraper_keyboard(&mut self) {
        self.scraper_keyboard_page = self.scraper_keyboard_page.next();
        let cells = scraper_keyboard_keys(self.scraper_keyboard_page).len();
        self.scraper_keyboard_list = ListState::new(cells, cells);
        self.scraper_keyboard_list
            .reshape(cells, SCRAPER_KEYBOARD_COLUMNS);
        self.apply_geometry();
        self.dirty = true;
    }

    fn pick_scraper_key(&mut self) {
        let keys = scraper_keyboard_keys(self.scraper_keyboard_page);
        let Some((_, character)) = keys.get(self.scraper_keyboard_list.selected()) else {
            return;
        };
        let limit = match self.scraper_keyboard_field {
            ScraperField::Username => 80,
            ScraperField::Password => 256,
            ScraperField::SearchTerm => 160,
        };
        if self.scraper_keyboard_draft.chars().count() < limit {
            self.scraper_keyboard_draft.push(*character);
            self.dirty = true;
        }
    }

    fn finish_scraper_keyboard(&mut self) {
        if self.scraper_keyboard_field == ScraperField::SearchTerm {
            let term = scraper_search_title(&self.scraper_keyboard_draft);
            self.scraper_keyboard_draft.clear();
            self.screen = Screen::ScraperMatches;
            self.apply_geometry();
            if term.is_empty() {
                self.scraper_search_status = "Enter a game title".to_string();
                self.dirty = true;
                return;
            }
            self.scraper_search_term = term;
            self.start_scraper_search();
            return;
        }
        self.screen = Screen::Scraper;
        self.apply_geometry();
        match self.scraper_keyboard_field {
            ScraperField::Username => {
                let mut settings = self.scraper_settings.clone();
                settings.username = std::mem::take(&mut self.scraper_keyboard_draft);
                self.replace_scraper_settings(settings);
            }
            ScraperField::Password if self.scraper_keyboard_draft.is_empty() => {
                let mut settings = self.scraper_settings.clone();
                settings.password.clear();
                settings.accepted_plaintext_warning = false;
                self.replace_scraper_settings(settings);
            }
            ScraperField::Password if self.scraper_settings.accepted_plaintext_warning => {
                let mut settings = self.scraper_settings.clone();
                settings.password = std::mem::take(&mut self.scraper_keyboard_draft);
                self.replace_scraper_settings(settings);
            }
            ScraperField::Password => {
                self.pending = Some(Pending::SaveScraperPassword);
                self.message = Some(
                    "ScreenScraper password will be stored as plain text on the card.\n\nA save, B cancel"
                        .to_string(),
                );
            }
            ScraperField::SearchTerm => unreachable!("handled before saving scraper login"),
        }
        self.dirty = true;
    }

    fn scraper_platform_id(&self) -> Option<u32> {
        let crate::scraper::Scope::Game { system_id, .. } = &self.scraper_scope else {
            return None;
        };
        crate::scraper::platform_id(system_id, &self.scraper_settings.system_ids)
    }

    fn current_scraper_match(&self) -> Option<&crate::scraper::Match> {
        self.scraper_matches.get(self.scraper_match_list.selected())
    }

    fn set_scraper_matches(&mut self, matches: Vec<crate::scraper::Match>) {
        self.scraper_preview_job = None;
        self.scraper_preview_match_id = None;
        self.scraper_preview_image = None;
        self.scraper_matches = matches;
        self.scraper_match_list =
            ListState::new(self.scraper_matches.len().max(1), self.geometry.visible);
        self.scraper_search_status = if self.scraper_matches.is_empty() {
            format!(
                "No Matches for {}",
                shortened_label(&self.scraper_search_term, 32)
            )
        } else if self.scraper_matches.len() == 1 {
            "1 Match".to_string()
        } else {
            format!("{} Matches", self.scraper_matches.len())
        };
        self.start_scraper_preview();
        self.apply_geometry();
        self.dirty = true;
    }

    fn open_scraper_matches(&mut self, matches: Vec<crate::scraper::Match>) {
        if !matches!(self.scraper_scope, crate::scraper::Scope::Game { .. }) {
            crate::note("scraper      refused manual matching outside a one-game scrape");
            return;
        }
        self.scraper_search_job = None;
        self.scraper_matches_return = Screen::ScraperProgress;
        self.screen = Screen::ScraperMatches;
        self.set_scraper_matches(matches);
    }

    fn clear_scraper_preview(&mut self) {
        self.scraper_preview_job = None;
        self.scraper_preview_match_id = None;
        self.scraper_preview_image = None;
        self.scraper_preview_caption.clear();
        self.ui.set_art(slint::Image::default());
        self.ui.set_has_art(false);
        self.ui.set_art_caption(SharedString::default());
        self.ui.set_art_heart(false);
        self.ui.set_art_scale_x(1.0);
    }

    fn leave_scraper_matches(&mut self) {
        self.scraper_search_job = None;
        self.clear_scraper_preview();
        self.screen = self.scraper_matches_return;
        self.apply_geometry();
        self.dirty = true;
    }

    fn open_manual_scraper_search(&mut self) {
        if !matches!(self.scraper_scope, crate::scraper::Scope::Game { .. }) {
            return;
        }
        if self.build.is_some() || self.refreshing.is_some() {
            self.message = Some("Wait for the current library update to finish.".to_string());
        } else if self.scraper_settings.username.is_empty()
            || self.scraper_settings.password.is_empty()
        {
            self.message = Some("Set the ScreenScraper username and password first.".to_string());
        } else if self.scraper_settings.image_policy == crate::scraper::ImagePolicy::Off
            && self.scraper_settings.metadata_policy == crate::scraper::MetadataPolicy::Off
        {
            self.message = Some("Enable images, metadata, or both first.".to_string());
        } else if !self.scraper_settings.accepted_plaintext_warning {
            self.pending = Some(Pending::AcceptScraperStorageForSearch);
            self.message = Some("ScreenScraper password will be stored as plain text on the card.\n\nA accept, B cancel".to_string());
        } else if self.save_scraper_settings() {
            self.open_scraper_matches(Vec::new());
            self.scraper_matches_return = Screen::Scraper;
            self.start_scraper_search();
        }
        self.dirty = true;
    }

    fn start_scraper_search(&mut self) {
        let Some(system_id) = self.scraper_platform_id() else {
            self.scraper_search_status = "This System cannot be searched".to_string();
            self.dirty = true;
            return;
        };
        let term = scraper_search_title(&self.scraper_search_term);
        if term.is_empty() {
            self.scraper_search_status = "Enter a game title".to_string();
            self.dirty = true;
            return;
        }
        let developer = match crate::scraper::DeveloperCredentials::embedded() {
            Ok(credentials) => credentials,
            Err(error) => {
                crate::note(&format!("scraper      search could not start: {error}"));
                self.scraper_search_status = "Search could not start".to_string();
                self.dirty = true;
                return;
            }
        };
        let request = crate::scraper::SearchRequest {
            system_id,
            term: term.clone(),
            settings: self.scraper_search_settings(),
            developer,
        };
        let job = match crate::scraper::start_search(request) {
            Ok(job) => job,
            Err(error) => {
                crate::note(&format!("scraper      search could not start: {error}"));
                self.scraper_search_status = scraper_search_error(&error).to_string();
                self.dirty = true;
                return;
            }
        };
        self.scraper_search_term = term;
        self.scraper_search_job = Some(job);
        self.scraper_preview_job = None;
        self.scraper_preview_match_id = None;
        self.scraper_preview_image = None;
        self.scraper_preview_caption = "Searching...".to_string();
        self.scraper_search_status = "Checking account".to_string();
        self.apply_geometry();
        self.dirty = true;
    }

    fn poll_scraper_search(&mut self) {
        loop {
            let event = self
                .scraper_search_job
                .as_mut()
                .and_then(crate::scraper::SearchJob::try_recv);
            let Some(event) = event else {
                break;
            };
            match event {
                crate::scraper::SearchEvent::Activity(activity) => {
                    self.scraper_search_status = activity;
                    self.dirty = true;
                }
                crate::scraper::SearchEvent::Finished {
                    matches,
                    account,
                    requests_started,
                    failed_searches,
                } => {
                    self.scraper_search_job = None;
                    self.scraper_progress.account = Some(account);
                    // This account snapshot was taken immediately before the
                    // manual query, so these counters describe work after
                    // that snapshot without double-counting the earlier run.
                    self.scraper_progress.requests_started = requests_started;
                    self.scraper_progress.failed_searches = failed_searches;
                    self.set_scraper_matches(matches);
                    break;
                }
                crate::scraper::SearchEvent::Cancelled => {
                    self.scraper_search_job = None;
                    self.scraper_search_status = "Search Cancelled".to_string();
                    self.start_scraper_preview();
                    self.apply_geometry();
                    self.dirty = true;
                    break;
                }
                crate::scraper::SearchEvent::Failed(error) => {
                    self.scraper_search_job = None;
                    crate::note(&format!("scraper      manual search failed: {error}"));
                    self.scraper_search_status = scraper_search_error(&error).to_string();
                    self.start_scraper_preview();
                    self.apply_geometry();
                    self.dirty = true;
                    break;
                }
            }
        }
    }

    fn start_scraper_preview(&mut self) {
        self.scraper_preview_job = None;
        self.scraper_preview_match_id = None;
        self.scraper_preview_image = None;
        let Some(candidate) = self.current_scraper_match().cloned() else {
            self.scraper_preview_caption = "No Matches".to_string();
            self.art_pending = true;
            self.dirty = true;
            return;
        };
        let Some(media) = candidate.media.clone() else {
            self.scraper_preview_caption = "No Image".to_string();
            self.art_pending = true;
            self.dirty = true;
            return;
        };
        let Some(max_download_speed) = self
            .scraper_progress
            .account
            .as_ref()
            .and_then(|account| account.max_download_speed)
        else {
            self.scraper_preview_caption = "Preview Unavailable".to_string();
            self.art_pending = true;
            self.dirty = true;
            return;
        };
        let developer = match crate::scraper::DeveloperCredentials::embedded() {
            Ok(credentials) => credentials,
            Err(error) => {
                crate::note(&format!("scraper      preview could not start: {error}"));
                self.scraper_preview_caption = "Preview Unavailable".to_string();
                self.art_pending = true;
                self.dirty = true;
                return;
            }
        };
        let palette = self.effective_palette();
        let request = crate::scraper::PreviewRequest {
            match_id: candidate.id.clone(),
            media,
            settings: self.scraper_settings.clone(),
            developer,
            max_download_speed,
            max_edge: self.width.max(self.height),
            ground: [palette.surface.r, palette.surface.g, palette.surface.b],
        };
        match crate::scraper::start_preview(request) {
            Ok(job) => {
                self.scraper_preview_job = Some(job);
                self.scraper_preview_match_id = Some(candidate.id);
                self.scraper_preview_caption = "Loading Image...".to_string();
            }
            Err(error) => {
                crate::note(&format!("scraper      preview could not start: {error}"));
                self.scraper_preview_caption = "Preview Unavailable".to_string();
            }
        }
        self.art_pending = true;
        self.dirty = true;
    }

    fn poll_scraper_preview(&mut self) {
        let event = self
            .scraper_preview_job
            .as_mut()
            .and_then(crate::scraper::PreviewJob::try_recv);
        let Some(event) = event else {
            return;
        };
        self.scraper_preview_job = None;
        match event {
            crate::scraper::PreviewEvent::Finished { match_id, image }
                if scraper_preview_is_current(
                    self.scraper_preview_match_id.as_deref(),
                    self.current_scraper_match(),
                    &match_id,
                ) =>
            {
                self.scraper_preview_image = Some(image);
                self.scraper_preview_caption.clear();
            }
            crate::scraper::PreviewEvent::Missing { match_id }
                if scraper_preview_is_current(
                    self.scraper_preview_match_id.as_deref(),
                    self.current_scraper_match(),
                    &match_id,
                ) =>
            {
                self.scraper_preview_image = None;
                self.scraper_preview_caption = "No Image".to_string();
            }
            crate::scraper::PreviewEvent::Failed { match_id, error }
                if scraper_preview_is_current(
                    self.scraper_preview_match_id.as_deref(),
                    self.current_scraper_match(),
                    &match_id,
                ) =>
            {
                crate::note(&format!("scraper      artwork preview failed: {error}"));
                self.scraper_preview_image = None;
                self.scraper_preview_caption = "Preview Unavailable".to_string();
            }
            crate::scraper::PreviewEvent::Cancelled
            | crate::scraper::PreviewEvent::Finished { .. }
            | crate::scraper::PreviewEvent::Missing { .. }
            | crate::scraper::PreviewEvent::Failed { .. } => {
                // A cancelled or stale result belongs to a highlight that
                // has already moved. It must not replace the current image.
                return;
            }
        }
        self.art_pending = true;
        self.dirty = true;
    }

    fn confirm_scraper_match(&mut self) {
        let Some(candidate) = self.current_scraper_match().cloned() else {
            return;
        };
        let candidate_name =
            shortened_label(&candidate.name, if self.width < 480 { 22 } else { 42 });
        let game_name = shortened_label(
            self.scraper_scope.label(),
            if self.width < 480 { 22 } else { 42 },
        );
        self.pending = Some(Pending::UseScraperMatch(Box::new(candidate)));
        self.message = Some(format!(
            "Use {candidate_name} for {game_name}?\n\nA Use, B Cancel"
        ));
        self.dirty = true;
    }

    fn begin_scraper_job(&mut self, job: crate::scraper::Job, manual_resolution: bool) {
        self.clear_scraper_preview();
        let scope_label = self.scraper_scope_label();
        self.scraper_progress = crate::scraper::Progress {
            scope: scope_label,
            phase: crate::scraper::Phase::Enumerating,
            ..Default::default()
        };
        self.scraper_progress_list = ListState::new(SCRAPER_PROGRESS_ROWS, self.geometry.visible);
        self.scraper_details = false;
        self.scraper_job = Some(job);
        self.scraper_manual_resolution_eligible = manual_resolution;
        self.scraper_open_matches_after_refresh = false;
        self.scraper_pending_terminal = None;
        self.scraper_terminal = None;
        self.scraper_refresh_queue.clear();
        self.scraper_cache_refresh_active = false;
        self.scraper_cancelling = false;
        self.message = None;
        self.screen = Screen::ScraperProgress;
        self.apply_geometry();
        self.dirty = true;
    }

    fn start_selected_scraper_match(&mut self, matched: crate::scraper::Match) {
        let developer = match crate::scraper::DeveloperCredentials::embedded() {
            Ok(credentials) => credentials,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };
        let request = crate::scraper::Request {
            scope: self.scraper_scope.clone(),
            scope_label: self.scraper_scope_label(),
            systems: self.all_systems.clone(),
            names: self.names.clone(),
            settings: self.scraper_settings.clone(),
            developer: Some(developer),
            artwork_pack_system_ids: self.artwork_pack_scraper_exclusions(),
        };
        let job = match crate::scraper::start_selected(request, matched) {
            Ok(job) => job,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };
        self.scraper_search_job = None;
        self.scraper_preview_job = None;
        self.scraper_preview_image = None;
        self.begin_scraper_job(job, false);
    }

    fn confirm_scrape(&mut self) {
        if self.build.is_some() || self.refreshing.is_some() {
            self.message = Some("Wait for the current library update to finish.".to_string());
            self.dirty = true;
            return;
        }
        let only_artwork_pack = self.scraper_scope_only_artwork_pack();
        if !only_artwork_pack
            && (self.scraper_settings.username.is_empty()
                || self.scraper_settings.password.is_empty())
        {
            self.message = Some("Set the ScreenScraper username and password first.".to_string());
            self.dirty = true;
            return;
        }
        if !only_artwork_pack
            && self.scraper_settings.image_policy == crate::scraper::ImagePolicy::Off
            && self.scraper_settings.metadata_policy == crate::scraper::MetadataPolicy::Off
        {
            self.message = Some("Enable images, metadata, or both first.".to_string());
            self.dirty = true;
            return;
        }
        if !only_artwork_pack && !self.scraper_settings.accepted_plaintext_warning {
            self.pending = Some(Pending::AcceptScraperStorageForStart);
            self.message = Some(
                "ScreenScraper password will be stored as plain text on the card.\n\nA accept, B cancel"
                    .to_string(),
            );
            self.dirty = true;
            return;
        }
        self.pending = Some(Pending::StartScrape);
        let scope = self.scraper_scope_label();
        self.message = Some(if only_artwork_pack {
            format!("Scrape {scope}?\nArtwork Pack systems will be skipped\nA start, B cancel")
        } else {
            format!(
                "Scrape {}?\n{} images, {} metadata\nA start, B cancel",
                scope,
                self.scraper_settings.image_policy.label(),
                self.scraper_settings.metadata_policy.label(),
            )
        });
        self.dirty = true;
    }

    fn start_scrape_now(&mut self) {
        let only_artwork_pack = self.scraper_scope_only_artwork_pack();
        if !only_artwork_pack && !self.save_scraper_settings() {
            return;
        }
        let developer = if only_artwork_pack {
            None
        } else {
            match crate::scraper::DeveloperCredentials::embedded() {
                Ok(credentials) => credentials,
                Err(error) => {
                    self.message = Some(error.to_string());
                    self.dirty = true;
                    return;
                }
            }
            .into()
        };
        let request = crate::scraper::Request {
            scope: self.scraper_scope.clone(),
            scope_label: self.scraper_scope_label(),
            systems: self.all_systems.clone(),
            names: self.names.clone(),
            settings: self.scraper_settings.clone(),
            developer,
            artwork_pack_system_ids: self.artwork_pack_scraper_exclusions(),
        };
        let job = match crate::scraper::start(request) {
            Ok(job) => job,
            Err(error) => {
                self.message = Some(error.to_string());
                self.dirty = true;
                return;
            }
        };
        let manual_resolution = matches!(self.scraper_scope, crate::scraper::Scope::Game { .. });
        self.scraper_matches.clear();
        self.scraper_preview_image = None;
        self.begin_scraper_job(job, manual_resolution);
    }

    fn scraper_progress_rows(&self) -> Vec<(String, String)> {
        let state = if self.scraper_pending_terminal.is_some() {
            "Refreshing Degauss lists".to_string()
        } else if let Some(terminal) = &self.scraper_terminal {
            terminal.label().to_string()
        } else if self.scraper_cancelling {
            "Cancelling safely".to_string()
        } else if !self.scraper_progress.activity.is_empty() {
            self.scraper_progress.activity.clone()
        } else {
            self.scraper_progress.phase.label().to_string()
        };
        let requests = self
            .scraper_progress
            .requests_left()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string());
        let failed = self
            .scraper_progress
            .failed_left()
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string());
        let speed = self
            .scraper_progress
            .account
            .as_ref()
            .and_then(|account| account.max_download_speed)
            .map(|value| format!("{value} KB/s"))
            .unwrap_or_else(|| "? KB/s".to_string());
        let max_workers = self
            .scraper_progress
            .account
            .as_ref()
            .and_then(|account| account.max_threads)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string());
        let per_minute = self
            .scraper_progress
            .account
            .as_ref()
            .and_then(|account| account.max_requests_per_minute)
            .map(|value| value.to_string())
            .unwrap_or_else(|| "?".to_string());
        let problem = match &self.scraper_terminal {
            Some(ScraperTerminal::Failed(detail)) => detail.clone(),
            _ => self
                .scraper_progress
                .last_problem
                .clone()
                .unwrap_or_else(|| "None".to_string()),
        };
        let current = if self.scraper_progress.current.is_empty() {
            "-".to_string()
        } else {
            self.scraper_progress.current.clone()
        };
        let compact = self.width < 480;
        let problems_label = if compact { "Issues" } else { "Skipped/errors" };
        let allowance_label = if compact {
            "Quota est."
        } else {
            "Lookup allowance est."
        };
        let allowance = if compact {
            format!("{requests} req, {failed} miss")
        } else {
            format!("{requests} requests, {failed} misses")
        };
        let throughput = if compact {
            format!(
                "{}/{max_workers} jobs, {per_minute}/min, {speed}",
                self.scraper_progress.workers
            )
        } else {
            format!(
                "{} active / {max_workers} max workers, {per_minute} requests/min, {speed}",
                self.scraper_progress.workers
            )
        };
        vec![
            ("Status".to_string(), state),
            ("Scope".to_string(), self.scraper_progress.scope.clone()),
            (format!("Current: {current}"), String::new()),
            (
                "Progress".to_string(),
                format!(
                    "{} / {}",
                    self.scraper_progress.completed, self.scraper_progress.total
                ),
            ),
            (
                "Written".to_string(),
                self.scraper_progress.updated.to_string(),
            ),
            (
                "Unchanged".to_string(),
                self.scraper_progress.unchanged.to_string(),
            ),
            (
                "Linked copies".to_string(),
                self.scraper_progress.deduplicated_aliases.to_string(),
            ),
            (
                "Unresolved".to_string(),
                format!(
                    "{} missing, {} ambiguous, {} no image",
                    self.scraper_progress.not_found,
                    self.scraper_progress.ambiguous,
                    self.scraper_progress.no_media
                ),
            ),
            (
                problems_label.to_string(),
                if compact {
                    format!(
                        "{} pack, {} unsup, {} shared, {} sys, {} failed",
                        self.scraper_progress.skipped_artwork_pack,
                        self.scraper_progress.unsupported_systems,
                        self.scraper_progress.ambiguous_targets,
                        self.scraper_progress.system_errors,
                        self.scraper_progress.failed
                    )
                } else {
                    format!(
                        "{} Artwork Pack, {} unsupported, {} shared, {} system, {} failed",
                        self.scraper_progress.skipped_artwork_pack,
                        self.scraper_progress.unsupported_systems,
                        self.scraper_progress.ambiguous_targets,
                        self.scraper_progress.system_errors,
                        self.scraper_progress.failed
                    )
                },
            ),
            (allowance_label.to_string(), allowance),
            ("Throughput".to_string(), throughput),
            (format!("Last problem: {problem}"), String::new()),
        ]
    }

    fn poll_scraper(&mut self) {
        self.poll_scraper_cache_refresh();
        loop {
            let event = self
                .scraper_job
                .as_mut()
                .and_then(crate::scraper::Job::try_recv);
            let Some(event) = event else {
                break;
            };
            match event {
                crate::scraper::Event::Progress(progress) => {
                    self.scraper_progress = progress;
                    self.dirty = true;
                }
                crate::scraper::Event::Finished(progress) => {
                    self.scraper_job = None;
                    self.scraper_progress = progress;
                    self.scraper_open_matches_after_refresh = needs_manual_scraper_match(
                        &self.scraper_scope,
                        self.scraper_manual_resolution_eligible,
                        &self.scraper_progress,
                    );
                    self.scraper_manual_resolution_eligible = false;
                    self.begin_scraper_finish(ScraperTerminal::Finished);
                    break;
                }
                crate::scraper::Event::Cancelled(progress) => {
                    self.scraper_job = None;
                    self.scraper_progress = progress;
                    self.begin_scraper_finish(ScraperTerminal::Cancelled);
                    break;
                }
                crate::scraper::Event::Failed { error, progress } => {
                    self.scraper_job = None;
                    self.scraper_progress = progress;
                    self.begin_scraper_finish(ScraperTerminal::Failed(
                        error.user_message().to_string(),
                    ));
                    break;
                }
            }
        }
    }

    fn begin_scraper_finish(&mut self, terminal: ScraperTerminal) {
        self.scraper_cancelling = false;
        self.scraper_pending_terminal = Some(terminal);
        self.scraper_progress.phase = crate::scraper::Phase::Finishing;
        self.scraper_refresh_queue = self
            .scraper_progress
            .updated_systems
            .iter()
            .cloned()
            .collect();
        self.start_next_scraper_refresh();
        self.dirty = true;
    }

    fn start_next_scraper_refresh(&mut self) {
        if let Some(id) = self.scraper_refresh_queue.pop_front() {
            let name = self
                .all_systems
                .iter()
                .find(|system| system.def.id == id)
                .map(|system| system.name().to_string())
                .unwrap_or_else(|| id.clone());
            self.scraper_progress.current = format!("Refreshing {name}");
            self.scraper_cache_refresh_active = true;
            self.scraper_refresh_folder.clear();
            self.scraper_refresh_folders = 0;
            self.scraper_refresh_games = 0;
            self.refreshing = Some(id);
        } else {
            self.scraper_cache_refresh_active = false;
            self.scraper_progress.current.clear();
            self.scraper_terminal = self.scraper_pending_terminal.take();
            if self.scraper_open_matches_after_refresh {
                self.scraper_open_matches_after_refresh = false;
                let matches = std::mem::take(&mut self.scraper_progress.manual_matches);
                self.open_scraper_matches(matches);
                return;
            }
        }
        self.apply_geometry();
    }

    fn finish_scraper_refresh(&mut self, error: Option<String>) {
        if let Some(error) = error {
            crate::note(&format!(
                "scraper      library refresh failed: {}",
                squashed(&error)
            ));
            self.scraper_progress.system_errors += 1;
            self.scraper_progress.last_problem = Some(format!("Library refresh failed: {error}"));
        }
        self.scraper_cache_refresh_active = false;
        self.start_next_scraper_refresh();
    }

    fn start_scraper_cache_refresh(&mut self, id: String) {
        let prepared = (|| -> Result<crate::index_job::Request> {
            let at = self
                .all_systems
                .iter()
                .position(|system| system.def.id == id)
                .ok_or_else(|| {
                    DegaussError::unsupported(
                        "library refresh",
                        format!("system {id} is no longer available"),
                    )
                })?;
            let roots = self
                .config
                .game_roots
                .iter()
                .map(PathBuf::from)
                .collect::<Vec<_>>();
            let paths =
                crate::systems::existing_folders_checked(&self.all_systems[at].def, &roots)?;
            if paths.is_empty() {
                return Err(DegaussError::unsupported(
                    "library refresh",
                    format!("no folder for {id} is on the card"),
                ));
            }
            self.all_systems[at].paths = paths;
            Ok(crate::index_job::Request {
                id: id.clone(),
                config: self.all_systems[at].to_config(),
                names: self.names.clone(),
                cache_dir: self.cache_dir.clone(),
                forced: true,
                artwork_pack: false,
                index: self.index.clone().unwrap_or_default(),
                retain_cache: self.open_system.as_deref() == Some(id.as_str()),
            })
        })();
        match prepared {
            Ok(request) => match crate::index_job::start(request) {
                Ok(job) => {
                    self.scraper_refresh_id = Some(id);
                    self.scraper_refresh_job = Some(job);
                }
                Err(failure) => self.finish_scraper_refresh(Some(failure.error.to_string())),
            },
            Err(error) => self.finish_scraper_refresh(Some(error.to_string())),
        }
        self.dirty = true;
    }

    fn poll_scraper_cache_refresh(&mut self) {
        loop {
            let Some(event) = self
                .scraper_refresh_job
                .as_mut()
                .and_then(crate::index_job::Job::try_recv)
            else {
                return;
            };
            let error = match event {
                crate::index_job::Event::Progress {
                    folders,
                    games,
                    folder,
                } => {
                    self.scraper_refresh_folders = folders;
                    self.scraper_refresh_games = games;
                    self.scraper_refresh_folder = folder;
                    self.dirty = true;
                    continue;
                }
                crate::index_job::Event::Ready {
                    index,
                    cache,
                    warnings,
                    folders,
                    games,
                    ..
                } => {
                    self.index = Some(index);
                    self.scraper_refresh_folders = folders;
                    self.scraper_refresh_games = games;
                    if let Some(id) = &self.scraper_refresh_id {
                        if let Some(system) =
                            self.all_systems.iter().find(|system| system.def.id == *id)
                        {
                            for path in &system.paths {
                                self.covers.invalidate_under(path);
                                self.gallery_covers.invalidate_under(path);
                            }
                            if self.open_system.as_deref() == Some(id.as_str()) {
                                self.opened_config = Some(system.to_config());
                                self.library = None;
                                self.system_cache = cache;
                                self.artwork_provider = None;
                            }
                        }
                    }
                    self.apply_index();
                    self.correct_system_counts();
                    self.rebuild_system_list();
                    if self.browsing == Browsing::Games
                        && self.open_system == self.scraper_refresh_id
                    {
                        self.relist_here();
                    }
                    (!warnings.is_empty()).then(|| warnings.join("\n"))
                }
                crate::index_job::Event::Failed { error, index } => {
                    self.index = Some(index);
                    Some(error.to_string())
                }
                crate::index_job::Event::Cancelled { index } => {
                    self.index = Some(index);
                    Some("The library refresh was cancelled before completion".into())
                }
            };
            self.scraper_refresh_job = None;
            self.scraper_refresh_id = None;
            self.finish_scraper_refresh(error);
            self.dirty = true;
            return;
        }
    }

    fn close_scraper_progress(&mut self) {
        if self.scraper_job.is_some()
            || self.scraper_pending_terminal.is_some()
            || self.scraper_cache_refresh_active
        {
            return;
        }
        self.scraper_terminal = None;
        self.screen = self.scraper_return;
        self.resolve_view();
        self.apply_geometry();
        self.touch_selection();
    }

    fn handle_scraper_keyboard(&mut self, action: Action) {
        match action {
            Action::Up => {
                self.scraper_keyboard_list.move_rows(-1);
            }
            Action::Down => {
                self.scraper_keyboard_list.move_rows(1);
            }
            Action::Slower | Action::PageUp => {
                self.scraper_keyboard_list.move_items(-1);
            }
            Action::Faster | Action::PageDown => {
                self.scraper_keyboard_list.move_items(1);
            }
            Action::Home => {
                self.scraper_keyboard_list.go_first();
            }
            Action::End => {
                self.scraper_keyboard_list.go_last();
            }
            Action::Accept => self.pick_scraper_key(),
            Action::Quit => self.finish_scraper_keyboard(),
            Action::Context => {
                self.scraper_keyboard_draft.pop();
            }
            Action::Menu => self.cycle_scraper_keyboard(),
            Action::CyclePresent | Action::FavoriteShortcut | Action::RandomShortcut => {}
        }
        self.dirty = true;
    }

    fn handle_scraper_progress(&mut self, action: Action) {
        if !self.scraper_details {
            match action {
                Action::Accept => {
                    self.scraper_details = true;
                    self.restart_marquee();
                }
                Action::Quit if self.scraper_job.is_some() && !self.scraper_cancelling => {
                    self.pending = Some(Pending::CancelScrape);
                    self.message = Some(format!(
                        "Cancel scrape for {}?\n\nA yes, B no",
                        self.scraper_scope_label()
                    ));
                }
                Action::Quit => self.close_scraper_progress(),
                _ => {}
            }
            self.dirty = true;
            return;
        }
        let selected = self.scraper_progress_list.selected();
        match action {
            Action::Up => {
                self.scraper_progress_list.move_items(-1);
            }
            Action::Down => {
                self.scraper_progress_list.move_items(1);
            }
            Action::Slower | Action::PageUp => {
                let page = self.scraper_progress_list.visible() as isize;
                self.scraper_progress_list.move_items(-page);
            }
            Action::Faster | Action::PageDown => {
                let page = self.scraper_progress_list.visible() as isize;
                self.scraper_progress_list.move_items(page);
            }
            Action::Home => {
                self.scraper_progress_list.go_first();
            }
            Action::End => {
                self.scraper_progress_list.go_last();
            }
            Action::Quit => self.scraper_details = false,
            Action::Accept => {}
            Action::Menu
            | Action::Context
            | Action::CyclePresent
            | Action::FavoriteShortcut
            | Action::RandomShortcut => {}
        }
        if selected != self.scraper_progress_list.selected() {
            self.restart_marquee();
        }
        self.dirty = true;
    }

    fn handle_scraper_matches(&mut self, action: Action) {
        if let Some(job) = &self.scraper_search_job {
            if action == Action::Quit {
                job.cancel();
                self.scraper_search_status = "Cancelling Search".to_string();
                self.dirty = true;
            }
            return;
        }

        let moved = match action {
            Action::Up => self.scraper_match_list.move_items(-1),
            Action::Down => self.scraper_match_list.move_items(1),
            Action::PageUp => {
                let page = self.scraper_match_list.visible() as isize;
                self.scraper_match_list.move_items(-page)
            }
            Action::PageDown => {
                let page = self.scraper_match_list.visible() as isize;
                self.scraper_match_list.move_items(page)
            }
            Action::Home => self.scraper_match_list.go_first(),
            Action::End => self.scraper_match_list.go_last(),
            Action::Accept => {
                self.confirm_scraper_match();
                false
            }
            Action::Context => {
                self.open_scraper_keyboard(ScraperField::SearchTerm);
                false
            }
            Action::Quit => {
                self.leave_scraper_matches();
                false
            }
            Action::Slower
            | Action::Faster
            | Action::Menu
            | Action::CyclePresent
            | Action::FavoriteShortcut
            | Action::RandomShortcut => false,
        };
        if moved {
            self.restart_marquee();
            self.start_scraper_preview();
        }
        self.dirty = true;
    }

    /// Read the open system off the card again, from the contextual menu.
    ///
    /// Say what is about to happen, then do it on the next frame, exactly
    /// as opening an unread system does: the read takes seconds on a big
    /// system, and a still screen with no explanation looks hung.
    fn rebuild_open_system(&mut self) {
        if self.build.is_some()
            || self.source_job.is_some()
            || self.provider_job.is_some()
            || self.source_resolution.is_some()
        {
            self.message = Some("Wait for the current library update to finish.".into());
            self.dirty = true;
            return;
        }
        let group = self
            .open_system
            .as_deref()
            .and_then(crate::artwork_pack::source_group);
        if let Some(group) = group {
            self.resolve_artwork_sources(Some(group), SourceResolutionAction::RefreshSystem);
        } else {
            self.rebuild_open_system_resolved();
        }
    }

    fn rebuild_open_system_resolved(&mut self) {
        if let Some(problem) = self
            .open_system
            .as_deref()
            .and_then(|id| self.source_problem(id))
        {
            self.message = Some(problem.to_string());
            self.dirty = true;
            return;
        }
        self.index_return_screen = self.screen;
        self.index_terminal = None;
        self.index_details = false;
        self.screen = Screen::Browse;
        self.apply_geometry();
        self.dirty = true;
        // A full build is already reading every system, this one
        // included, and its progress message is on screen; a second read
        // of the same folders would be the same work twice.
        if self.build.is_some() || self.source_job.is_some() || self.provider_job.is_some() {
            return;
        }
        let Some(id) = self.open_system.clone() else {
            return;
        };
        if self.pack_selected(&id) {
            self.begin_source_recovery(&id, SourceRecoveryPurpose::RefreshSystem);
            return;
        }
        let Some(at) = self
            .all_systems
            .iter()
            .position(|system| system.def.id == id)
        else {
            return;
        };
        let roots = self
            .config
            .game_roots
            .iter()
            .map(PathBuf::from)
            .collect::<Vec<_>>();
        match crate::systems::existing_folders_checked(&self.all_systems[at].def, &roots) {
            Ok(paths) if !paths.is_empty() => self.all_systems[at].paths = paths,
            Ok(_) => {
                self.message = Some("No folder for this system is on the card".to_string());
                return;
            }
            Err(error) => {
                self.message = Some(error.to_string());
                return;
            }
        }
        self.opened_config = Some(self.all_systems[at].to_config());
        self.library = None;
        self.build = Some(Building {
            left: vec![at],
            done: 0,
            total: 1,
            index: self.index.clone().unwrap_or_default(),
            forced: true,
            current: None,
            job: None,
            discovery: None,
            awaiting_frame: false,
            displayed: None,
            cancelling: false,
            single: true,
            folders: 0,
            games: 0,
            folders_done: 0,
            games_done: 0,
            folder: String::new(),
            folder_revision: 0,
            started: Instant::now(),
        });
        self.pause_index_marquees();
        self.publish_index_progress();
    }

    pub fn handle(&mut self, action: Action) -> Option<Outcome> {
        if self.source_resolution.is_some() && self.build.is_none() {
            if matches!(action, Action::Quit) {
                self.source_resolution_cancelled = true;
                if let Some(job) = &self.source_resolution {
                    job.cancel();
                }
            }
            self.dirty = true;
            return None;
        }
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
        if self.screen == Screen::ThemeEditor {
            self.handle_theme_editor(action);
            return None;
        }

        if self.build.is_some() || self.index_terminal.is_some() {
            match action {
                Action::Accept if !self.index_details => {
                    self.index_details = true;
                    self.message = self.index_overview().map(|view| view.report);
                }
                Action::Quit if self.index_details => {
                    self.index_details = false;
                    self.message = None;
                }
                Action::Quit if self.build.is_some() => {
                    let build = self.build.as_mut().expect("active build");
                    build.cancelling = true;
                    if let Some(job) = &build.job {
                        job.cancel();
                    }
                    if let Some(Discovery::Running(job)) = &build.discovery {
                        job.cancel();
                    }
                    if let Some(job) = &self.source_resolution {
                        self.source_resolution_cancelled = true;
                        job.cancel();
                    }
                    if let Some(job) = &self.source_job {
                        self.source_cancelling = true;
                        job.cancel();
                    }
                    build.displayed = None;
                    self.publish_index_progress();
                }
                Action::Quit => {
                    self.index_terminal = None;
                    self.message = None;
                    self.screen = self.index_return_screen;
                    self.resolve_view();
                    self.apply_geometry();
                    self.touch_selection();
                }
                _ if self.index_details => {
                    self.scroll_message(action);
                }
                _ => {}
            }
            self.dirty = true;
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
                        Pending::RunScript(launch) => {
                            if let Err(error) = launch.validate() {
                                self.message = Some(error.to_string());
                            } else if self.save_settings() {
                                return Some(Outcome::Script(launch));
                            }
                        }
                        Pending::Hide(index) => {
                            self.hide_system(index);
                            self.screen = Screen::Browse;
                            self.apply_geometry();
                        }
                        Pending::ResetCustomViews => self.reset_custom_views(),
                        Pending::ResetHidden => self.reset_hidden(),
                        Pending::StartScrape => self.start_scrape_now(),
                        Pending::SaveScraperPassword => {
                            let mut settings = self.scraper_settings.clone();
                            settings.password = std::mem::take(&mut self.scraper_keyboard_draft);
                            settings.accepted_plaintext_warning = true;
                            self.replace_scraper_settings(settings);
                        }
                        Pending::AcceptScraperStorageForStart => {
                            let mut settings = self.scraper_settings.clone();
                            settings.accepted_plaintext_warning = true;
                            if self.replace_scraper_settings(settings) {
                                self.confirm_scrape();
                            }
                        }
                        Pending::AcceptScraperStorageForSearch => {
                            let mut settings = self.scraper_settings.clone();
                            settings.accepted_plaintext_warning = true;
                            if self.replace_scraper_settings(settings) {
                                self.open_manual_scraper_search();
                            }
                        }
                        Pending::ClearScraperLogin => {
                            let mut settings = self.scraper_settings.clone();
                            settings.clear_login();
                            if self.replace_scraper_settings(settings) {
                                self.scraper_keyboard_draft.clear();
                            }
                        }
                        Pending::CancelScrape => {
                            if let Some(job) = &self.scraper_job {
                                job.cancel();
                                self.scraper_cancelling = true;
                                self.scraper_progress.current =
                                    "Waiting for the current request".to_string();
                                self.apply_geometry();
                            }
                        }
                        Pending::ClearCategoryImage(target) => self.clear_category_image(&target),
                        Pending::UseScraperMatch(matched) => {
                            self.start_selected_scraper_match(*matched);
                        }
                    }
                    return None;
                }
                // Anything else cancels: a question left on screen after an
                // unrelated press would be worse than asking again.
                _ => {
                    if pending == Pending::SaveScraperPassword {
                        self.scraper_keyboard_draft.clear();
                    }
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
        // controls stay live under it. The Left and right modes in the
        // dispatch below depend on this return coming first: a press that
        // dismisses a message must not also jump a letter or a page.
        if message_consumes_input(self.message.is_some(), self.build.is_some()) {
            if self.scroll_message(action) {
                return None;
            }
            self.message = None;
            self.dirty = true;
            return None;
        }

        if self.screen == Screen::Information {
            self.handle_information(action);
            return None;
        }

        if self.screen == Screen::ScraperKeyboard {
            self.handle_scraper_keyboard(action);
            return None;
        }
        if self.screen == Screen::ScraperProgress {
            self.handle_scraper_progress(action);
            return None;
        }
        if self.screen == Screen::ScraperMatches {
            self.handle_scraper_matches(action);
            return None;
        }
        if self.screen == Screen::SourceProgress {
            if matches!(action, Action::Quit | Action::Context | Action::Menu) {
                if let Some(job) = &self.source_job {
                    job.cancel();
                    self.source_cancelling = true;
                    self.dirty = true;
                } else if let Some(job) = &self.provider_job {
                    job.cancel();
                    self.provider_cancelling = true;
                    self.dirty = true;
                }
            }
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
                } else if self.screen == Screen::CategoryImage {
                    self.page_step(delta);
                } else if matches!(self.screen, Screen::Options | Screen::Advanced) {
                    self.handle_option_input(if delta < 0 {
                        OptionInput::Previous
                    } else {
                        OptionInput::Next
                    });
                } else if self.screen == Screen::Scraper {
                    self.adjust_scraper(delta);
                } else if self.screen == Screen::Context {
                    self.adjust_context(delta);
                } else if self.screen == Screen::OptionsRoot {
                    return None;
                } else {
                    // The Left and right setting applies only while
                    // browsing. The other screens that fall through to
                    // here, Menu, Help, About and the favourite folder
                    // picker, keep changing the speed as they always have.
                    let mode = if self.screen == Screen::Browse {
                        self.horizontal
                    } else {
                        Horizontal::Speed
                    };
                    match mode {
                        Horizontal::Speed => {
                            self.speed = step(self.speed, delta, SPEED_STEPS.len());
                            self.settings.speed_step = Some(self.speed);
                            // The badge answers "what speed is it now".
                            // The other modes move the list, which is its
                            // own confirmation, so this is the only arm
                            // that raises it.
                            self.speed_shown_at = Some(Instant::now());
                            self.dirty = true;
                        }
                        Horizontal::Letter => self.letter_step(delta),
                        Horizontal::Page => self.page_step(delta),
                        Horizontal::Direction => {
                            if self.active_list_mut().move_items(delta) {
                                self.touch_selection();
                            }
                        }
                    }
                }
            }
            Action::PageUp | Action::PageDown => {
                // Keyboard only, and always a page whatever Left and right
                // is set to: two keys named after the movement should not
                // change meaning under a setting about the stick.
                let delta = if matches!(action, Action::PageDown) {
                    1
                } else {
                    -1
                };
                if self.screen == Screen::Find {
                    // The letter grid has no pages. One cell is what these
                    // keys moved when they shared the left and right
                    // actions, and it stays.
                    self.find_list.move_items(delta);
                    self.dirty = true;
                } else {
                    self.page_step(delta);
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
            Action::FavoriteShortcut => match self.favorite_change() {
                Some(FavoriteChange::Add) => self.open_favorite_folders(),
                Some(FavoriteChange::Remove) => self.remove_favorite(),
                None => {}
            },
            Action::RandomShortcut => {
                if self.random_shortcut_enabled() {
                    return self.random_here(false);
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
                Screen::CategoryImage => self.choose_category_image(),
                Screen::Menu | Screen::Context => {
                    let was_context = self.screen == Screen::Context;
                    let choice = self
                        .menu
                        .get(self.menu_list.selected())
                        .cloned()
                        .unwrap_or_default();
                    if was_context {
                        self.remember_context_selection();
                        if self.context_is_root() {
                            if let Some(page) = ContextPage::ALL
                                .into_iter()
                                .find(|page| page.label() == choice)
                            {
                                self.show_context_page(Some(page));
                            }
                            return None;
                        }
                    }
                    // Any action other than continuing to adjust the view
                    // closes or replaces Context. Persist a view already
                    // chosen there before that path can launch or fail.
                    if was_context
                        && choice != CHANGE_VIEW
                        && choice != USE_GLOBAL_VIEW
                        && choice != USE_DEFAULT_CORE_VERSION
                        && !self.save_settings()
                    {
                        return None;
                    }
                    if choice == CHANGE_VIEW || choice == CORE_VERSION {
                        // Stays open, like a setting: the point is to see
                        // the view while choosing it.
                        self.adjust_context(1);
                    } else if choice == USE_DEFAULT_CORE_VERSION {
                        self.use_default_core_version();
                    } else if choice == USE_GLOBAL_VIEW {
                        self.use_global_view();
                    } else if choice == CHANGE_CATEGORY_IMAGE {
                        self.open_category_image_picker();
                    } else if choice == CLEAR_CATEGORY_IMAGE {
                        if let Some(target) = self.selected_image_target() {
                            self.message = Some(format!(
                                "Clear custom image for {}?\n\nA yes, B no",
                                target.label()
                            ));
                            self.pending = Some(Pending::ClearCategoryImage(target));
                        }
                    } else if choice == GAME_DATA_SOURCE {
                        self.open_game_data_source();
                    } else if choice == GAME_INFORMATION {
                        self.open_information();
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
                    } else if choice == REBUILD_SYSTEM {
                        self.rebuild_open_system();
                    } else if choice == CLEAR_SEARCH {
                        self.clear_filter();
                        self.screen = Screen::Browse;
                        self.apply_geometry();
                    } else if matches!(choice.as_str(), SCRAPE_SYSTEM | SCRAPE_FOLDER | SCRAPE_GAME)
                    {
                        if let Some(scope) = self.scraper_scope_from_context(&choice) {
                            self.open_scraper(scope, Screen::Context);
                        } else {
                            self.message = Some("That selection cannot be scraped.".to_string());
                        }
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
                        self.screen = Screen::OptionsRoot;
                        self.apply_geometry();
                    } else if choice == "Scripts" {
                        self.open_scripts();
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
                Screen::Options | Screen::Advanced => {
                    self.handle_option_input(OptionInput::Activate)
                }
                Screen::OptionsRoot => {
                    if let Some(page) = OptionsPage::ALL.get(self.options_root_list.selected()) {
                        self.open_options_page(*page);
                    }
                }
                Screen::Information => {}
                Screen::Scraper => self.activate_scraper_row(),
                Screen::GameDataSource => match self.menu_list.selected() {
                    0 => {
                        if let Some(group) = self
                            .source_system_id
                            .as_deref()
                            .and_then(crate::artwork_pack::source_group)
                        {
                            self.resolve_artwork_sources(
                                Some(group),
                                SourceResolutionAction::Automatic,
                            );
                        }
                    }
                    1 => {
                        self.source_switch_automatic = false;
                        self.begin_source_switch(crate::source_cache::Target::Gamelist);
                    }
                    2 => {
                        self.source_switch_automatic = false;
                        self.open_artwork_pack_locations();
                    }
                    _ => {}
                },
                Screen::ArtworkPackLocation => self.activate_artwork_pack_location(),
                Screen::ArtworkPackDirectory => self.activate_artwork_pack_directory(),
                Screen::Scripts => self.activate_script(),
                Screen::Help | Screen::About => return self.go_back(),
                Screen::Splash => self.leave_splash(),
                // Routed before this match so editor controls can assign X
                // and Y without inheriting menu behaviour.
                Screen::ThemeEditor
                | Screen::ScraperKeyboard
                | Screen::ScraperProgress
                | Screen::ScraperMatches
                | Screen::SourceProgress => {}
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
        // Explicit pickers show the image being chosen, so every highlighted
        // item is previewed immediately regardless of the browse speed.
        let debounce = if matches!(self.screen, Screen::CategoryImage | Screen::ScraperMatches)
            || self.speed <= self.art_limit()
        {
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
        self.open_system.as_ref().is_some_and(|id| {
            self.all_systems
                .iter()
                .any(|system| &system.def.id == id && is_favorites(system.category()))
        })
    }

    /// The picture, the words under it, whether the heart should stand in
    /// for both, and whether the picture is game artwork rather than a logo.
    fn current_art(&self) -> (Option<PathBuf>, String, bool, bool) {
        match (self.screen, self.browsing) {
            (Screen::CategoryImage, _) => {
                category_image_preview(&self.category_image_choices, self.menu_list.selected())
            }
            (Screen::Browse | Screen::Information, Browsing::Games) => {
                match self.here.get(self.game_list.selected()) {
                    Some(row) => {
                        // Inside favourites a folder is a shelf the user
                        // made. It has no artwork and never will, and its
                        // name is already the row: the mark says more.
                        let heart =
                            self.in_favorites() && matches!(row.kind, browse::Kind::Enter(_));
                        let game_art =
                            matches!(&row.kind, browse::Kind::Play(_)) && row.cover.is_some();
                        (
                            row.cover.clone().or_else(|| {
                                if heart {
                                    return None;
                                }
                                // A folder has no picture of its own; the
                                // system's logo is better than a blank plate.
                                self.open_system_ref()
                                    .and_then(|system| self.system_logo(system))
                            }),
                            row.name.clone(),
                            heart,
                            game_art,
                        )
                    }
                    None => (None, String::new(), false, false),
                }
            }
            (Screen::Browse, Browsing::Systems) => {
                match self.systems.get(self.system_list.selected()) {
                    Some(system) => {
                        let (logo, heart) =
                            logo_or_favorite_heart(system.category(), self.system_logo(system));
                        (logo, system.name().to_string(), heart, false)
                    }
                    None => (None, String::new(), false, false),
                }
            }
            (Screen::Browse, Browsing::Categories) => {
                match self.categories.get(self.category_list.selected()) {
                    Some((name, _)) => {
                        let (logo, heart) = logo_or_favorite_heart(name, self.category_logo(name));
                        (logo, name.clone(), heart, false)
                    }
                    None => (None, String::new(), false, false),
                }
            }
            _ => (None, String::new(), false, false),
        }
    }

    /// A picture for a path, decoding it here or asking for it elsewhere.
    ///
    /// The one place that decides, so every list goes the same way.
    fn cover_for(&mut self, path: &std::path::Path) -> Option<slint::Image> {
        let palette = self.effective_palette();
        let ground = match self.screen {
            Screen::Screensaver => [0, 0, 0],
            Screen::Information => [
                palette.background.r,
                palette.background.g,
                palette.background.b,
            ],
            Screen::Browse if self.layout == Layout::Carousel => [
                palette.background.r,
                palette.background.g,
                palette.background.b,
            ],
            _ => [palette.surface.r, palette.surface.g, palette.surface.b],
        };
        self.covers.set_ground(ground);
        self.covers.get(path).map(to_image)
    }

    /// A browse-row image from the cache belonging to the current layout.
    /// The final flag says that Gallery left a first-time decode for a later
    /// frame because this frame's fixed budget was exhausted.
    fn row_cover_for(
        &mut self,
        path: Option<PathBuf>,
        gallery_budget: &mut usize,
    ) -> (slint::Image, bool, bool) {
        let Some(path) = path else {
            return (slint::Image::default(), false, false);
        };
        if self.layout != Layout::Gallery {
            return match self.cover_for(&path) {
                Some(image) => (image, true, false),
                None => (slint::Image::default(), false, false),
            };
        }
        match self.gallery_covers.get_budgeted(&path, gallery_budget) {
            BudgetedCover::Image(image) => (to_image(image), true, false),
            BudgetedCover::Unavailable => (slint::Image::default(), false, false),
            BudgetedCover::Deferred => (slint::Image::default(), false, true),
        }
    }

    /// Spend Gallery's first available decode on the selected entry. The
    /// row-building pass then fills neighbouring cells with what remains.
    fn prefetch_gallery_selection(&mut self, gallery_budget: &mut usize) -> bool {
        if self.layout != Layout::Gallery {
            return false;
        }
        let Some(path) = self.current_art().0 else {
            return false;
        };
        if self.gallery_covers.knows(&path) {
            return false;
        }
        matches!(
            self.gallery_covers.get_budgeted(&path, gallery_budget),
            BudgetedCover::Deferred
        )
    }

    fn load_art(&mut self) {
        if self.screen == Screen::ScraperMatches {
            self.ui
                .set_art_caption(SharedString::from(self.scraper_preview_caption.as_str()));
            self.ui.set_art_heart(false);
            self.ui.set_art_scale_x(1.0);
            if let Some(image) = self.scraper_preview_image.as_ref() {
                self.ui.set_art(to_image(image));
                self.ui.set_has_art(true);
            } else {
                self.ui.set_has_art(false);
            }
            self.art_pending = false;
            return;
        }
        // The screensaver is nothing but a picture, so it wants one whatever
        // the browse layout happens to be.
        let wanted = self.screen == Screen::CategoryImage
            || (self.show_art
                && match self.screen {
                    Screen::Screensaver => true,
                    Screen::Browse => self.layout == Layout::Details,
                    Screen::Information => true,
                    _ => false,
                });
        if !wanted {
            self.ui.set_art(slint::Image::default());
            self.ui.set_has_art(false);
            self.ui.set_art_caption(SharedString::default());
            self.ui.set_art_heart(false);
            self.ui.set_art_scale_x(1.0);
            self.art_pending = false;
            return;
        }

        let started = Instant::now();
        let (path, caption, heart, game_art) = self.current_art();
        self.ui.set_art_caption(SharedString::from(caption));
        self.ui.set_art_heart(heart);
        self.ui.set_art_scale_x(artwork_horizontal(
            self.artwork_scale,
            self.width,
            self.height,
            game_art,
        ));
        let group_preview = self.screen == Screen::Browse
            && self.browsing != Browsing::Games
            && self.layout == Layout::Details;
        match path.and_then(|path| {
            if group_preview {
                self.group_covers.get(&path).map(to_image)
            } else {
                self.cover_for(&path)
            }
        }) {
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
        let (range, selected_in_window) = if self.screen == Screen::Context {
            context_window(&self.menu, &self.menu_list, self.geometry.visible)
        } else {
            self.active_list().window()
        };
        let mut rows = Vec::with_capacity(range.len());
        let mut gallery_budget = GALLERY_DECODE_BUDGET;
        let mut gallery_pending = false;

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
                        art_scale_x: 1.0,
                        value: SharedString::new(),
                    });
                }
            }
            Screen::ScraperKeyboard => {
                for (label, _) in scraper_keyboard_keys(self.scraper_keyboard_page) {
                    rows.push(plain_row(&label, ""));
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
                            art_scale_x: 1.0,
                            value: SharedString::new(),
                        });
                    }
                    self.ui.set_saver_cell(cell);
                    self.ui.set_saver_offset(sub);
                }
            }
            Screen::Browse => {
                let with_art = self.show_art && self.layout.rows_have_art() && !self.art_pending;
                if with_art && self.layout == Layout::Gallery {
                    gallery_pending |= self.prefetch_gallery_selection(&mut gallery_budget);
                }
                match self.browsing {
                    Browsing::Categories => {
                        for index in range {
                            let name = self.categories[index].0.clone();
                            let name = &name;
                            // No number beside a group. It counted systems,
                            // so Arcade read "1" while holding a thousand
                            // games, which answers a question nobody asked
                            // with a number that means something else.
                            let logo = if with_art {
                                self.category_logo(name)
                            } else {
                                None
                            };
                            let (cover, has_cover, deferred) =
                                self.row_cover_for(logo, &mut gallery_budget);
                            gallery_pending |= deferred;
                            rows.push(Row {
                                title: SharedString::from(name.as_str()),
                                favorite: browse_row_favorite(
                                    self.layout,
                                    false,
                                    is_favorites(name),
                                ),
                                cover,
                                has_cover,
                                art_scale_x: 1.0,
                                value: SharedString::new(),
                            });
                        }
                    }
                    Browsing::Systems => {
                        for index in range {
                            let logo = if with_art {
                                self.system_logo(&self.systems[index])
                            } else {
                                None
                            };
                            let (cover, has_cover, deferred) =
                                self.row_cover_for(logo, &mut gallery_budget);
                            gallery_pending |= deferred;
                            let name = self.systems[index].name().to_string();
                            let favorite = browse_row_favorite(
                                self.layout,
                                false,
                                is_favorites(self.systems[index].category()),
                            );
                            // How many games are in there, where the card
                            // has been read for it.
                            let held = self.games_in(&self.systems[index].def.id);
                            rows.push(Row {
                                title: SharedString::from(name.as_str()),
                                favorite,
                                cover,
                                has_cover,
                                art_scale_x: 1.0,
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
                        let inside_favorites = self.in_favorites();
                        let logo = if with_art {
                            self.open_system_ref()
                                .and_then(|system| self.system_logo(system))
                        } else {
                            None
                        };
                        for index in range {
                            let game_art = with_art
                                && matches!(&self.here[index].kind, browse::Kind::Play(_))
                                && self.here[index].cover.is_some();
                            let art_scale_x = artwork_horizontal(
                                self.artwork_scale,
                                self.width,
                                self.height,
                                game_art,
                            );
                            let wanted = if with_art {
                                // Inside favourites a folder is a shelf the
                                // user made, and the panel marks it with a
                                // heart rather than a logo: no stand-in here
                                // either, so the two views agree.
                                let bare = inside_favorites && self.here[index].is_folder();
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
                            let (cover, has_cover, deferred) =
                                self.row_cover_for(wanted, &mut gallery_budget);
                            gallery_pending |= deferred;
                            let row = &self.here[index];
                            rows.push(Row {
                                // A folder is marked as one. Nothing else in
                                // the list says which rows can be entered.
                                title: SharedString::from(if row.is_folder() {
                                    format!("[ {} ]", row.name)
                                } else {
                                    row.name.clone()
                                }),
                                favorite: browse_row_favorite(
                                    self.layout,
                                    row.favorite,
                                    inside_favorites && row.is_folder(),
                                ),
                                cover,
                                has_cover,
                                art_scale_x,
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
            Screen::Menu
            | Screen::Scripts
            | Screen::FavoriteFolder
            | Screen::CategoryImage
            | Screen::GameDataSource
            | Screen::ArtworkPackLocation
            | Screen::ArtworkPackDirectory => {
                for index in range {
                    rows.push(plain_row(&self.menu[index], ""));
                }
            }
            Screen::SourceProgress => {
                let progress_rows = source_progress_rows(
                    &self.source_progress,
                    self.compact_provider_progress(),
                    self.source_progress_subject(),
                );
                for index in range {
                    if let Some((label, value)) = progress_rows.get(index) {
                        rows.push(plain_row(label, value));
                    }
                }
            }
            Screen::Context => {
                for index in range {
                    let value = self.context_value(index);
                    if self.menu[index] == CORE_VERSION {
                        // Full nightly paths use the existing title marquee on small screens.
                        rows.push(plain_row(&format!("{CORE_VERSION}: {value}"), ""));
                    } else {
                        rows.push(plain_row(&self.menu[index], &value));
                    }
                }
            }
            Screen::Options | Screen::Advanced => {
                let list = self.option_ids();
                for index in range {
                    let option = list[index];
                    rows.push(plain_row(option.label(), &self.option_value(option)));
                }
            }
            Screen::OptionsRoot => {
                for index in range {
                    let page = OptionsPage::ALL[index];
                    rows.push(plain_row(page.label(), ">"));
                }
            }
            Screen::Information => {}
            Screen::ThemeEditor => {
                if let Some(editor) = self.theme_editor.as_ref() {
                    let editor_rows = editor.rows();
                    for index in range {
                        let (title, value) = &editor_rows[index];
                        rows.push(plain_row(title, value));
                    }
                }
            }
            Screen::Scraper => {
                for index in range {
                    let row = scraper_rows(&self.scraper_scope)[index];
                    let label = if row == ScraperRow::ImageType {
                        if self.scraper_system_id().is_some() {
                            "System Image"
                        } else {
                            "Default Image"
                        }
                    } else {
                        row.label()
                    };
                    rows.push(plain_row(label, &self.scraper_value(row)));
                }
            }
            Screen::ScraperProgress => {
                let progress = self.scraper_progress_rows();
                for index in range {
                    let (title, value) = &progress[index];
                    rows.push(plain_row(title, value));
                }
            }
            Screen::ScraperMatches => {
                if self.scraper_matches.is_empty() {
                    rows.push(plain_row("No Matches", ""));
                } else {
                    for index in range {
                        let matched = &self.scraper_matches[index];
                        rows.push(plain_row(&matched.name, scraper_match_year(matched)));
                    }
                }
            }
            Screen::About | Screen::Splash => {
                // The wordmark is drawn by the interface layer; there are no
                // rows to build.
            }
            Screen::Help => {
                for index in range {
                    let line = if index == HELP_LR_ROW {
                        self.horizontal.help_line()
                    } else {
                        HELP[index]
                    };
                    rows.push(plain_row(line, ""));
                }
            }
        }

        self.rows.set_vec(rows);
        // Help fits on one screen and nothing on it can be chosen, so a
        // highlight there would be a cursor pointing at nothing. No index
        // matches -1.
        self.ui.set_selected(
            if self.screen == Screen::Help
                || (self.screen == Screen::ThemeEditor
                    && self.theme_editor.as_ref().is_some_and(|editor| {
                        matches!(editor.mode, EditorMode::Hex | EditorMode::Picker)
                    }))
            {
                -1
            } else {
                selected_in_window as i32
            },
        );
        self.update_chrome();
        self.window.request_redraw();
        self.dirty = gallery_pending;
    }

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

    fn update_compact_details(&self) {
        let details = self
            .here
            .get(self.game_list.selected())
            .filter(|row| !row.is_folder())
            .filter(|_| self.screen == Screen::Browse && self.browsing == Browsing::Games)
            .map(|row| &row.details);
        let (summary, publisher) = details.map(compact_detail_text).unwrap_or_default();
        self.ui.set_compact_summary(SharedString::from(summary));
        self.ui.set_compact_publisher(SharedString::from(publisher));
    }

    /// How much room the lines under the picture take, if any.
    ///
    /// Set here rather than with the rest of the geometry because it
    /// depends on what the cursor is on, and the cursor moves far more
    /// often than the shape of the screen does. Only over a game: a folder
    /// has nothing to say, and six empty labels beside it push the picture
    /// up the screen to make room for nothing.
    fn apply_detail_panel(&self) {
        let line = (self.geometry.small_font * 1.6).ceil().max(12.0);
        let wanted = (line * 3.0 + self.geometry.pad)
            .max(self.geometry.row_height * self.geometry.visible as f32 * 0.36);
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
        self.update_compact_details();
        self.update_operation_ui();
        let (group_title, group_subtitle) = match (self.screen, self.browsing) {
            (Screen::Browse, Browsing::Categories) => (
                self.selected_category_name()
                    .unwrap_or_default()
                    .to_string(),
                "Browse Systems".to_string(),
            ),
            (Screen::Browse, Browsing::Systems) => self
                .systems
                .get(self.system_list.selected())
                .map(|system| {
                    (
                        system.name().to_string(),
                        self.games_in(&system.def.id)
                            .map(|count| format!("{count} Games"))
                            .unwrap_or_default(),
                    )
                })
                .unwrap_or_default(),
            _ => (String::new(), String::new()),
        };
        self.ui.set_group_title(SharedString::from(group_title));
        self.ui
            .set_group_subtitle(SharedString::from(group_subtitle));
        self.ui
            .set_detail_lines(ModelRc::new(VecModel::from(self.detail_lines())));
        self.ui
            .set_clock(SharedString::from(self.status.clock.as_str()));
        self.ui.set_wifi(self.status.wifi);
        self.ui.set_bluetooth(self.status.bluetooth);
        self.ui.set_index_active(self.build.is_some());
        self.ui.set_index_fraction(
            self.build
                .as_ref()
                .map_or(0.0, |build| build.done as f32 / build.total.max(1) as f32),
        );
        self.ui.set_browse_playable(
            self.browsing == Browsing::Games
                && self
                    .here
                    .get(self.game_list.selected())
                    .is_some_and(|row| matches!(row.kind, browse::Kind::Play(_))),
        );
        self.ui
            .set_plain_scope(SharedString::from(match self.screen {
                Screen::OptionsRoot | Screen::Options | Screen::Advanced => {
                    "Global Settings".to_string()
                }
                Screen::Context => self.context_scope(),
                Screen::Scraper => self.scraper_scope_label(),
                Screen::Information => self.here_label(),
                Screen::GameDataSource | Screen::ArtworkPackLocation => {
                    self.source_system_id.clone().unwrap_or_default()
                }
                _ => String::new(),
            }));
        self.ui
            .set_heading_detail(SharedString::from(match self.screen {
                Screen::Browse if self.browsing == Browsing::Categories => {
                    "Game Browser".to_string()
                }
                Screen::Browse | Screen::OptionsRoot | Screen::Options | Screen::Advanced => {
                    format!(
                        "{}/{}",
                        (self.active_list().selected() + 1).min(self.active_list().count()),
                        self.active_list().count()
                    )
                }
                Screen::Context => {
                    let count = self.menu.iter().filter(|entry| !entry.is_empty()).count();
                    let selected = self
                        .menu
                        .iter()
                        .take(self.menu_list.selected() + 1)
                        .filter(|entry| !entry.is_empty())
                        .count();
                    format!("{selected}/{count}")
                }
                Screen::ThemeEditor => self
                    .theme_editor
                    .as_ref()
                    .map(|editor| editor.source_name().to_string())
                    .unwrap_or_default(),
                Screen::GameDataSource | Screen::ArtworkPackLocation => {
                    self.source_system_id.clone().unwrap_or_default()
                }
                Screen::CategoryImage => format!(
                    "{}/{}",
                    self.active_list().selected() + 1,
                    self.active_list().count()
                ),
                _ => String::new(),
            }));
        // Just the multiple. The chevrons were a way of showing the speed
        // without reading it, and the badge is only up for a moment now.
        let (multiple, _) = SPEED_STEPS[self.speed.min(SPEED_STEPS.len() - 1)];
        self.ui
            .set_speed_badge(SharedString::from(if multiple.fract() == 0.0 {
                format!("{multiple:.0}x")
            } else {
                format!("{multiple:.1}x")
            }));
        let now = Instant::now();
        let gallery_started = if self.screen == Screen::Browse && self.layout == Layout::Gallery {
            self.gallery_title_shown_at
        } else {
            None
        };
        let (speed_visible, gallery_title_visible) =
            transient_badges(self.speed_shown_at, gallery_started, now);
        self.ui.set_speed_visible(speed_visible);
        let gallery_title = self.current_art().1;
        self.ui.set_gallery_title(SharedString::from(gallery_title));
        self.ui.set_gallery_title_visible(gallery_title_visible);
        // Only the word travels to the interface; the arrows beside it are
        // written in the .slint file so their glyphs get embedded.
        self.ui
            .set_lr_word(SharedString::from(self.horizontal.legend_word()));

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
                    "Search This Folder".to_string(),
                    format!("{} found", self.here.len()),
                ),
            },
            Screen::Browse => {
                let name = match self.browsing {
                    Browsing::Categories => String::new(),
                    Browsing::Systems => self
                        .open_category
                        .clone()
                        .unwrap_or_else(|| "Systems".to_string()),
                    Browsing::Games => self.here_label(),
                };
                (name, speed_badge(self.speed))
            }
            Screen::Menu => ("Menu".to_string(), String::new()),
            Screen::Scripts => {
                let relative = self
                    .scripts_browser
                    .as_ref()
                    .and_then(|browser| self.scripts_directory.strip_prefix(browser.root()).ok());
                let heading = match relative.filter(|path| !path.as_os_str().is_empty()) {
                    Some(path) => format!("Scripts / {}", path.display()),
                    None => "Scripts".to_string(),
                };
                let help = match self.scripts_entries.get(self.menu_list.selected()) {
                    Some(entry) if entry.is_directory => "Open this folder to choose a script.",
                    Some(_) => {
                        "Run this script after confirmation. Degauss returns when it finishes."
                    }
                    None => "No scripts in this folder. B returns to the parent or menu.",
                };
                (heading, help.to_string())
            }
            Screen::Context => {
                self.ui.set_plain_help(self.menu_controls().into());
                let selected = self
                    .menu
                    .get(self.menu_list.selected())
                    .map(String::as_str)
                    .unwrap_or("");
                let help = if self.context_is_root() {
                    ContextPage::ALL
                        .into_iter()
                        .find(|page| page.label() == selected)
                        .map(ContextPage::help)
                        .unwrap_or("")
                } else {
                    context_help(selected)
                };
                let heading = self
                    .context_page
                    .map(|page| format!("Actions / {}", page.label()))
                    .unwrap_or_else(|| "Actions".to_string());
                (heading, help.to_string())
            }
            Screen::OptionsRoot => (
                "Options".to_string(),
                OptionsPage::ALL[self.options_root_list.selected()]
                    .help()
                    .to_string(),
            ),
            Screen::Information => ("Game Information".to_string(), String::new()),
            Screen::Scraper => {
                self.ui.set_plain_help(self.menu_controls().into());
                (
                    "ScreenScraper".to_string(),
                    self.scraper_selected_help().to_string(),
                )
            }
            Screen::ScraperMatches => (
                format!("Match {}", self.scraper_scope_label()),
                self.scraper_search_status.clone(),
            ),
            Screen::ScraperKeyboard => {
                let field = match self.scraper_keyboard_field {
                    ScraperField::Username => "Username",
                    ScraperField::Password => "Password",
                    ScraperField::SearchTerm => "Search",
                };
                let value = scraper_draft_display(
                    self.scraper_keyboard_field,
                    &self.scraper_keyboard_draft,
                );
                (
                    format!("{field}: {value}"),
                    self.scraper_keyboard_page.label().to_string(),
                )
            }
            Screen::ScraperProgress => {
                let status = if self.scraper_pending_terminal.is_some() {
                    "Refreshing lists".to_string()
                } else if let Some(terminal) = &self.scraper_terminal {
                    terminal.label().to_string()
                } else if self.scraper_cancelling {
                    "Cancelling".to_string()
                } else if !self.scraper_progress.activity.is_empty() {
                    self.scraper_progress.activity.clone()
                } else {
                    self.scraper_progress.phase.label().to_string()
                };
                ("ScreenScraper".to_string(), status)
            }
            Screen::CategoryImage => (
                format!(
                    "Image for {}",
                    self.category_image_target
                        .as_ref()
                        .map(ImageTarget::label)
                        .unwrap_or("Selection")
                ),
                "A Select, B Back, Left/Right Page".to_string(),
            ),
            Screen::GameDataSource => (
                if self.source_choice_is_shared() {
                    "Game Data Source: Neo Geo + MVS".to_string()
                } else {
                    "Game Data Source".to_string()
                },
                match self.menu_list.selected() {
                    0 => "Use gamelist.xml when present; otherwise use an installed SD/USB pack.",
                    1 => "Always use gamelist.xml, even when an artwork pack is installed.",
                    _ => "Choose a pack for images and metadata. Scraping is disabled.",
                }
                .to_string(),
            ),
            Screen::ArtworkPackLocation => (
                if self.source_choice_is_shared() {
                    "Artwork Pack: Neo Geo + MVS".to_string()
                } else {
                    "Artwork Pack Location".to_string()
                },
                "Choose a detected pack or browse to another directory.".to_string(),
            ),
            Screen::ArtworkPackDirectory => (
                self.source_directory
                    .as_deref()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Artwork Pack Directory".to_string()),
                String::new(),
            ),
            Screen::SourceProgress => (
                if self.provider_job_purpose == ProviderJobPurpose::LocationDiscovery {
                    "Checking Artwork Pack Locations".to_string()
                } else if self.provider_pending_open.is_some() {
                    "Reading Artwork Pack Database".to_string()
                } else {
                    match self.source_operation {
                        Some(SourceOperation::Recover(_)) => {
                            "Refreshing Artwork Pack Cache".to_string()
                        }
                        _ => "Changing Game Data Source".to_string(),
                    }
                },
                if self.source_cancelling || self.provider_cancelling {
                    "Stopping safely".to_string()
                } else {
                    "B Cancel".to_string()
                },
            ),
            Screen::Options | Screen::Advanced => {
                self.ui.set_plain_help(self.menu_controls().into());
                (
                    if self.screen == Screen::Advanced {
                        "Options / Developer".to_string()
                    } else {
                        format!("Options / {}", self.options_page.label())
                    },
                    self.option_ids()
                        .get(self.active_list().selected())
                        .map(|o| o.help().to_string())
                        .unwrap_or_default(),
                )
            }
            Screen::ThemeEditor => {
                let (mode, selected, custom_source) = self
                    .theme_editor
                    .as_ref()
                    .map(|editor| (editor.mode, editor.selected, editor.source_is_custom()))
                    .unwrap_or((EditorMode::Browse, 0, false));
                let status = theme_editor_help(mode, selected, custom_source);
                ("Theme editor".to_string(), status.to_string())
            }
            Screen::Help => ("Help".to_string(), String::new()),
            Screen::About => ("About".to_string(), String::new()),
            Screen::Splash => (String::new(), String::new()),
        };

        self.ui.set_heading(SharedString::from(heading));
        self.ui.set_status(SharedString::from(status));
        if self.screen == Screen::Find && self.find_mode == FindMode::Search {
            self.ui.set_find_query(self.filter.clone().into());
        }
        self.ui.set_show_stats(self.show_stats);
        self.ui.set_stats(SharedString::from(self.stats_line()));
        // A plain message says how to leave, the way the questions already
        // carry their keys in their own text. Questions keep their "A yes,
        // B no" and build progress replaces itself every frame, so neither
        // takes the hint.
        let overlay = self.message.as_deref().unwrap_or_default();
        if self.ui.get_overlay().as_str() != overlay {
            self.ui.set_overlay_offset(0.0);
        }
        self.ui
            .set_overlay_dismissible(self.pending.is_none() && self.build.is_none());
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

    #[allow(dead_code)]
    /// The effective drawing path, including any runtime default or override.
    pub fn present_mode(&self) -> PresentMode {
        PresentMode::parse(self.present_label).unwrap_or(PresentMode::Direct)
    }

    /// Resolve startup presentation without persisting an implicit device default.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn initialize_presentation(
        &mut self,
        default: PresentMode,
        explicit: Option<PresentMode>,
    ) -> PresentMode {
        let mode = initial_present_mode(self.settings.present.as_deref(), default, explicit);
        self.present_label = mode.label();
        mode
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn run(
        &mut self,
        surface: &mut dyn Surface,
        input: &mut InputReader,
        presenter: &mut Presenter,
        mut owner_alive: impl FnMut() -> Result<bool>,
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
            if !owner_alive()? {
                self.save_settings();
                return Ok(Outcome::LauncherReplaced);
            }
            let now = Instant::now();

            slint::platform::update_timers_and_animations();
            self.poll_artwork_sources();
            self.poll_source_cache();
            self.poll_provider_job();
            self.start_provider_job_if_ready();
            self.poll_scraper();
            self.poll_scraper_search();
            self.poll_scraper_preview();
            self.poll_information();

            // A held left or right scrolls only where it moves the cursor:
            // while browsing in the Direction setting. Everywhere else,
            // and in every other setting, one press stays one step. Decided
            // before the presses so the first press of a hold is retained,
            // and again before the repeats so a screen opened under a held
            // stick stops the repeat instead of taking one more step there.
            repeater.set_horizontal_repeats(self.horizontal_scrolls());
            repeater.set_favorite_hold(self.favorite_change().is_some());
            repeater.set_random_hold(self.random_shortcut_enabled());
            for edge in input.poll() {
                let action = match edge {
                    KeyEdge::Down(action) => repeater.press(action, now),
                    KeyEdge::Up(action) => repeater.release(action, now),
                };
                if let Some(action) = action {
                    if let Some(outcome) = self.handle(action) {
                        if !matches!(outcome, Outcome::Script(_)) {
                            self.save_settings();
                        }
                        return Ok(outcome);
                    }
                }
            }
            repeater.set_horizontal_repeats(self.horizontal_scrolls());
            repeater.set_favorite_hold(self.favorite_change().is_some());
            repeater.set_random_hold(self.random_shortcut_enabled());
            for action in repeater.tick(now) {
                if let Some(outcome) = self.handle(action) {
                    if !matches!(outcome, Outcome::Script(_)) {
                        self.save_settings();
                    }
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
            if self
                .speed_shown_at
                .is_some_and(|at| now.duration_since(at) >= Duration::from_millis(SPEED_BADGE_MS))
            {
                self.speed_shown_at = None;
                self.dirty = true;
            }
            if self
                .gallery_title_shown_at
                .is_some_and(|at| now.duration_since(at) >= Duration::from_millis(GALLERY_TITLE_MS))
            {
                self.gallery_title_shown_at = None;
                self.dirty = true;
            }

            // Left alone for long enough, show pictures instead.
            let idle = self.screensaver_after();
            if idle > 0
                && self.source_resolution.is_none()
                && self.build.is_none()
                && self.index_terminal.is_none()
                && self.scraper_refresh_job.is_none()
                && self.information.is_none()
                && !matches!(
                    self.screen,
                    Screen::Screensaver
                        | Screen::Splash
                        | Screen::ScraperProgress
                        | Screen::ScraperMatches
                        | Screen::SourceProgress
                )
                && !(self.screen == Screen::ScraperKeyboard
                    && self.scraper_keyboard_field == ScraperField::SearchTerm)
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

            // A theme, screen or geometry change asks for complete frames
            // through the presenter, which is only reachable from here.
            // Refresh the Rust-backed models first, then force the draw.
            let complete_repaint = take_complete_repaint(&mut self.pending_complete_repaints);
            if complete_repaint {
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
            if complete_repaint {
                presenter.force_repaint(&self.window);
            }
            self.last_build = frame_start.elapsed();

            let drew = if let Some(work) = presenter.draw(&self.window, surface)? {
                surface.present()?;
                self.index_frame_presented();
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
                if !repeater.anything_held() && self.opening.is_none() && self.refreshing.is_none()
                {
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
            // The same deferment for reading one system again. A press in
            // the frame between can already have dismissed the message;
            // the read still happens, only its explanation went early.
            if let Some(id) = self.refreshing.take() {
                let scraper_refresh = self.scraper_cache_refresh_active;
                if scraper_refresh {
                    self.start_scraper_cache_refresh(id);
                } else {
                    let error = self.refresh_system(&id);
                    if self.browsing == Browsing::Games
                        && self.open_system.as_deref() == Some(id.as_str())
                    {
                        self.relist_here();
                    }
                    if let Some(error) = error {
                        self.message = Some(error);
                    }
                }
                self.dirty = true;
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
        self.finish_background_work_for_headless();
        self.load_art();
        self.startup.first_frame_ms = self.started.elapsed().as_millis();
        let minimum_frames = 2usize;
        let maximum_frames = if self.layout == Layout::Gallery {
            self.geometry.visible.div_ceil(GALLERY_DECODE_BUDGET) + minimum_frames
        } else {
            minimum_frames
        };
        for frame in 0..maximum_frames {
            let complete_repaint = take_complete_repaint(&mut self.pending_complete_repaints);
            if complete_repaint {
                self.dirty = true;
            }
            if self.dirty {
                self.refresh();
            }
            let final_frame = frame + 1 >= minimum_frames && !self.dirty;
            // Filling a Gallery still can take longer than its half-second
            // title. A headless render has no earlier frame to inspect, so
            // restart the title only on the final, fully populated frame.
            // The interactive run loop keeps the ordinary timer unchanged.
            if final_frame && self.screen == Screen::Browse && self.layout == Layout::Gallery {
                self.gallery_title_shown_at = Some(Instant::now());
                self.update_chrome();
            }
            let started = Instant::now();
            // The framebuffer loop advances Slint before every draw. A still
            // render must do the same or initial layout and animation state
            // depend on how long discovery happened to take in this process.
            slint::platform::update_timers_and_animations();
            if complete_repaint {
                presenter.force_repaint(&self.window);
            }
            if let Some(work) = presenter.draw(&self.window, surface)? {
                self.last_work = work;
            }
            surface.present()?;
            self.timer.record(started.elapsed());
            self.update_chrome();
            self.window.request_redraw();
            if final_frame {
                break;
            }
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
        self.covers = self.fresh_cover_cache();
        self.group_covers = self.fresh_group_cover_cache();
        self.gallery_covers = self.fresh_gallery_cover_cache();
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

        if let Some(text) =
            report_failure_caches(&[&self.covers, &self.gallery_covers, &self.group_covers])
        {
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
            thumbnails: self.gallery_covers.stats,
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
        let mut saved = crate::state::State::record(
            &system,
            self.open_category.as_deref().unwrap_or_default(),
            &trail,
            self.game_list.selected(),
            &self.left_at,
            &self.category_system,
        );
        saved.selected_row = self.here.get(self.game_list.selected()).map(row_key);
        saved
    }

    /// Put the user back where they were before the game.
    ///
    /// Every step is allowed to fail without complaint. A card whose folders
    /// have moved since should land wherever the walk got to, which is where
    /// going back would have led anyway, rather than refusing to start.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn restore_position(&mut self, saved: &crate::state::State) {
        if saved.system.is_empty() {
            self.resolve_view();
            self.apply_geometry();
            return;
        }
        // Each group's own memory first, so backing out of the restored
        // system recalls the other groups exactly as before the exit.
        self.category_system = saved.category_system.clone();
        if !saved.category.is_empty() {
            self.open_category = Some(saved.category.clone());
            self.rebuild_system_list();
            self.browsing = Browsing::Systems;
            self.resolve_view();
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
            // The saved system is gone. Return to a coherent top level rather
            // than keeping a category filter under a Categories browse state.
            self.open_category = None;
            self.browsing = Browsing::Categories;
            self.rebuild_system_list();
            self.resolve_view();
            self.apply_geometry();
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
            self.resolve_view();
            self.apply_geometry();
            return;
        }

        // The first place is the one opening the system already reached.
        // `places` stops at anything the card no longer has, so a renamed
        // folder lands on its parent instead of an error screen.
        let places = saved.places();
        let walked_everything = places.len() == saved.trail.len();
        for place in places.into_iter().skip(1) {
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
        if walked_everything {
            self.game_list.select(reselect(
                &self.here,
                saved.selected_row.as_deref(),
                saved.selected,
            ));
        } else {
            // The walk stopped short of the folder the cursor position
            // belongs to; that number means a row in a folder that was
            // not reached. Re-list where the walk DID land, whose own
            // remembered row is the honest answer.
            self.show_here();
        }
        self.touch_selection();
        self.resolve_view();
        self.apply_geometry();
    }

    pub fn open_system_by_index(&mut self, index: usize) {
        self.finish_background_work_for_headless();
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
            self.finish_background_work_for_headless();
        }
    }

    fn finish_background_work_for_headless(&mut self) {
        loop {
            self.poll_artwork_sources();
            if self.source_resolution.is_some() {
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            if self.build.is_some() {
                // Headless callers explicitly finish work before rendering
                // their only image; no interactive frame is being awaited.
                self.index_frame_presented();
                self.build_one_system();
                std::thread::sleep(Duration::from_millis(1));
                continue;
            }
            self.poll_source_cache();
            self.poll_provider_job();
            self.start_provider_job_if_ready();
            self.poll_information();
            if self.build.is_none()
                && self.information.is_none()
                && self.source_job.is_none()
                && self.provider_job.is_none()
                && self.provider_requests.is_empty()
            {
                break;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Open a screen directly, for the headless render path.
    pub fn set_screen(&mut self, screen: Screen) {
        if screen == Screen::Scripts {
            self.open_scripts();
            return;
        }
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
        if screen == Screen::Information {
            self.open_context();
            self.open_information();
            self.finish_background_work_for_headless();
            return;
        }
        if screen == Screen::Find {
            // Same reason: the grid is laid out by the door, not by the
            // screen it sets.
            self.open_find(FindMode::Jump);
            return;
        }
        if screen == Screen::ThemeEditor {
            self.open_theme_editor();
            return;
        }
        if screen == Screen::CategoryImage {
            // The choices and their preview paths are assembled by the
            // normal Context action; setting the enum alone would draw an
            // empty list and could not verify the real picker.
            self.open_category_image_picker();
            return;
        }
        if screen == Screen::GameDataSource {
            self.open_game_data_source();
            return;
        }
        if screen == Screen::ArtworkPackLocation {
            self.open_game_data_source();
            if self.screen == Screen::GameDataSource {
                self.finish_background_work_for_headless();
                self.open_artwork_pack_locations();
                self.finish_background_work_for_headless();
            }
            return;
        }
        if screen == Screen::ArtworkPackDirectory {
            self.open_game_data_source();
            if self.screen == Screen::GameDataSource {
                self.open_artwork_pack_directory(self.default_pack_browser_root(), true);
            }
            return;
        }
        if screen == Screen::SourceProgress {
            let selected_id = self.context_system_id().map(str::to_string);
            let current = selected_id
                .as_deref()
                .and_then(|id| {
                    self.all_systems
                        .iter()
                        .find(|system| system.def.id == id)
                        .map(|system| system.name().to_string())
                })
                .unwrap_or_default();
            let systems = selected_id
                .as_deref()
                .and_then(crate::artwork_pack::source_group)
                .map(|group| {
                    self.all_systems
                        .iter()
                        .filter(|system| {
                            crate::artwork_pack::source_group(&system.def.id) == Some(group)
                        })
                        .count()
                })
                .unwrap_or(0);
            self.source_progress = crate::source_cache::Progress {
                current,
                system: usize::from(systems > 0),
                systems,
                ..crate::source_cache::Progress::default()
            };
            self.show_source_progress();
            return;
        }
        self.screen = screen;
        if screen == Screen::Browse {
            self.resolve_view();
        }
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
        self.layout_override = Some(layout);
        self.resolve_view();
        self.apply_geometry();
    }

    pub fn select(&mut self, index: usize) {
        if self.screen == Screen::ThemeEditor {
            if let Some(editor) = self.theme_editor.as_mut() {
                editor.selected = index.min(EDITOR_ROWS - 1);
                self.sync_theme_editor();
            }
            return;
        }
        self.active_list_mut().go_first();
        self.active_list_mut().move_items(index as isize);
        self.touch_selection();
    }

    /// Populate the manual-match screen without contacting ScreenScraper.
    /// This exists only in the test binary so visual regression captures can
    /// exercise states that normally require an asynchronous live response.
    #[cfg(test)]
    pub(crate) fn set_scraper_match_visual_fixture(
        &mut self,
        scope_title: &str,
        matches: Vec<crate::scraper::Match>,
        selected: usize,
        status: &str,
        preview: Option<crate::covers::RgbImage>,
        caption: &str,
    ) {
        self.scraper_scope = crate::scraper::Scope::Game {
            system_id: "NES".to_string(),
            launch: browse::Launch::File(PathBuf::from("/visual-fixture/game.nes")),
            title: scope_title.to_string(),
        };
        self.scraper_search_term = scope_title.to_string();
        self.scraper_search_job = None;
        self.scraper_preview_job = None;
        self.scraper_matches = matches;
        self.scraper_match_list =
            ListState::new(self.scraper_matches.len().max(1), self.geometry.visible);
        self.scraper_match_list
            .select(selected.min(self.scraper_matches.len().saturating_sub(1)));
        self.scraper_preview_match_id = self.current_scraper_match().map(|item| item.id.clone());
        self.scraper_preview_image = preview;
        self.scraper_preview_caption = caption.to_string();
        self.scraper_search_status = status.to_string();
        self.scraper_terminal = Some(ScraperTerminal::Finished);
        self.scraper_pending_terminal = None;
        self.scraper_open_matches_after_refresh = false;
        self.pending = None;
        self.message = None;
        self.screen = Screen::ScraperMatches;
        self.apply_geometry();
        self.art_pending = true;
        self.dirty = true;
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn covers(&self) -> &CoverCache {
        &self.covers
    }

    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    pub fn gallery_covers(&self) -> &CoverCache {
        &self.gallery_covers
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
        art_scale_x: 1.0,
        value: SharedString::from(value),
    }
}

fn on_off(value: bool) -> String {
    if value { "On" } else { "Off" }.to_string()
}

/// Apply display correction only to real game artwork. System and category
/// logos, folder stand-ins and screensaver pictures retain their existing
/// framebuffer geometry.
fn artwork_horizontal(scale: ArtworkScale, width: u32, height: u32, is_game_art: bool) -> f32 {
    if is_game_art {
        scale.horizontal(width, height)
    } else {
        1.0
    }
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

fn message_consumes_input(has_message: bool, build_running: bool) -> bool {
    has_message && !build_running
}

#[cfg(target_os = "linux")]
pub fn report_art_failures(covers: &CoverCache, thumbnails: &CoverCache) -> Option<String> {
    report_failure_caches(&[covers, thumbnails])
}

/// Group artwork, large artwork and Gallery thumbnails report one
/// deduplicated list. Returned rather than printed: on the device the console
/// is behind the picture, so the caller decides where it can be read.
fn report_failure_caches(caches: &[&CoverCache]) -> Option<String> {
    let mut failures = Vec::new();
    for cache in caches {
        for failure in cache.failures() {
            if failures.iter().any(|(path, _)| *path == failure.0) {
                continue;
            }
            failures.push(failure);
            if failures.len() == 10 {
                break;
            }
        }
        if failures.len() == 10 {
            break;
        }
    }
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
    pub thumbnails: CoverStats,
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
            "thumbnails   {} decoded, avg {} us, worst {} us, {} evictions, {} hits, scale {} us avg, {} KB held",
            self.thumbnails.decoded,
            self.thumbnails.avg_decode_us(),
            self.thumbnails.worst_decode_us,
            self.thumbnails.evictions,
            self.thumbnails.cache_hits,
            self.thumbnails
                .scale_us_total
                .checked_div(self.thumbnails.decoded)
                .unwrap_or(0),
            self.thumbnails.bytes_held / 1024
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
    LauncherReplaced,
    Script(Box<crate::scripts::Launch>),

    /// A launch carries its finished plan rather than a row index: the plan
    /// is built while the interface is still up, so everything that can go
    /// wrong is a message on screen and never an exit. Boxed because a plan
    /// carries a whole MGL and an outcome moves by value.
    Launch {
        plan: Box<crate::launch::LaunchPlan>,
        name: String,
    },
}

/// Runs within the renderer's single installed Slint platform.
#[cfg(test)]
pub(crate) fn test_library_launch_flow(window: Rc<MinimalSoftwareWindow>) {
    ui_acceptance_tests::run_ui_acceptance_flow(window.clone());
    // Atomically claim a fresh directory; a previous interrupted run may have
    // left a fixture behind, and its contents are not ours to remove.
    let root = (0_u64..)
        .find_map(|attempt| {
            let candidate = std::env::temp_dir().join(format!(
                "degauss-app-library-{}-{attempt}",
                std::process::id()
            ));
            match std::fs::create_dir(&candidate) {
                Ok(()) => Some(candidate),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => None,
                Err(error) => panic!("creating isolated fixture: {error}"),
            }
        })
        .expect("fixture directory counter exhausted");
    for folder in ["games/NES", "_Console", "_RA_Cores/Cores", "_Unstable"] {
        std::fs::create_dir_all(root.join(folder)).unwrap();
    }
    std::fs::write(root.join("_Console/NES_20260101.rbf"), b"standard").unwrap();
    std::fs::write(root.join("_RA_Cores/Cores/NES.rbf"), b"ra").unwrap();
    std::fs::write(root.join("_RA_Cores/NES.mgl"), b"<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir=\"1\">RA_NES</setname></mistergamedescription>").unwrap();
    let nightly = "_Unstable/NES_unstable_20260101_123456.rbf";
    std::fs::write(root.join(nightly), b"nightly").unwrap();
    let archive = root.join("games/NES/small.zip");
    std::fs::write(
        &archive,
        crate::zip::tests_archive(&["A/one.nes", "B/two.nes"], false),
    )
    .unwrap();
    let mut config = Config::parse("[app]", &root.join("degauss.toml")).unwrap();
    config.menu_root = root.to_string_lossy().into_owned();
    config.game_roots = vec![root.join("games").to_string_lossy().into_owned()];
    let table = crate::systems::parse_table(
        include_str!("../assets/systems.toml"),
        Path::new("systems.toml"),
    )
    .unwrap();
    let def = table.iter().find(|s| s.id == "NES").unwrap().clone();
    let found = FoundSystem {
        def: def.clone(),
        paths: vec![root.join("games/NES")],
        logo_dir: None,
        menu_folder: None,
    };
    let loaded = Loaded {
        config,
        settings: Settings::default(),
        settings_path: root.join("settings.toml"),
        systems: vec![found],
        table: vec![def],
        names: Default::default(),
        logo_dir: None,
        themes_dir: root.join("themes"),
        themes: Default::default(),
    };
    let ui = DegaussWindow::new().unwrap();
    let mut app = App::new(loaded, window, ui, StartupTimings::default(), 352, 240);
    app.finish_background_work_for_headless();
    let initial_screen = app.screen;
    app.screen = Screen::ScraperProgress;
    app.scraper_progress.phase = crate::scraper::Phase::Account;
    app.scraper_progress.total = 1;
    app.update_operation_ui();
    assert_eq!(app.ui.get_operation_progress().as_str(), "0 / 1 Games");
    assert_eq!(app.ui.get_operation_note().as_str(), "");
    assert!(
        !app.ui.get_operation_determinate(),
        "account checking remains indeterminate even with a known game total"
    );
    app.scraper_progress.phase = crate::scraper::Phase::Enumerating;
    app.scraper_progress.total = 0;
    app.update_operation_ui();
    assert_eq!(app.ui.get_operation_note().as_str(), "Total Not Yet Known");
    assert!(!app.ui.get_operation_determinate());
    app.scraper_progress = Default::default();
    app.screen = initial_screen;
    app.open_system_by_index(0);
    let rules = std::mem::take(&mut app.all_systems[0].def.launch);
    assert_eq!(
        app.core_system_id().as_deref(),
        Some("NES"),
        "an explicitly bound core remains selectable without file launch rules"
    );
    assert!(
        !crate::core_choices::available(&app.all_systems[0].to_config(), &root,)
            .unwrap()
            .is_empty()
    );
    app.all_systems[0].def.launch = rules;
    let rbf = std::mem::take(&mut app.all_systems[0].def.rbf);
    assert!(
        app.core_system_id().is_none(),
        "file rules alone do not bind a system core"
    );
    app.all_systems[0].def.rbf = rbf;
    app.enter(Place::Archive(archive.clone()));
    assert_eq!(app.here.len(), 2, "archive keeps two immediate folders");
    app.enter(Place::ArchiveDirectory {
        archive: archive.clone(),
        prefix: "A".into(),
    });
    assert_eq!(app.here.len(), 1);
    app.game_list.select(0);
    let mgl = |app: &mut App| match app
        .confirm_launch()
        .expect("UI must allow a valid ZIP member")
    {
        Outcome::Launch { plan, .. } => plan.mgl,
        _ => panic!("expected launch outcome"),
    };
    assert!(mgl(&mut app).contains("<rbf>_Console/NES</rbf>"));
    app.reopen_context_for(CORE_VERSION);
    assert!(app.menu.iter().any(|row| row == CORE_VERSION));
    app.menu_list
        .select(app.menu.iter().position(|row| row == CORE_VERSION).unwrap());
    app.handle(Action::Faster);
    assert!(
        app.message.is_none(),
        "successful cycling must not intercept the next input"
    );
    assert_eq!(
        app.settings.core_choices.get("NES").map(String::as_str),
        Some("standard")
    );
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().core_choices,
        app.settings.core_choices
    );
    app.handle(Action::Faster);
    assert_eq!(
        app.settings.core_choices.get("NES").map(String::as_str),
        Some("ra"),
        "consecutive inputs must cycle without dismissing a modal"
    );
    assert!(app.message.is_none());
    assert_eq!(
        Settings::load(&app.settings_path).unwrap().core_choices,
        app.settings.core_choices
    );
    assert!(mgl(&mut app).contains("same_dir=\"1\""));
    std::fs::remove_file(root.join("_Console/NES_20260101.rbf")).unwrap();
    assert!(
        mgl(&mut app).contains("_RA_Cores/Cores/NES"),
        "RA works without standard"
    );
    app.settings
        .core_choices
        .insert("NES".into(), nightly.into());
    assert!(
        mgl(&mut app).contains(nightly.trim_end_matches(".rbf")),
        "explicit nightly works without standard"
    );
    std::fs::remove_file(root.join(nightly)).unwrap();
    assert!(
        app.confirm_launch().is_none(),
        "missing explicit core must stay in UI"
    );
    assert!(app
        .message
        .as_ref()
        .is_some_and(|message| !message.is_empty()));
    app.settings.core_choices.insert("NES".into(), "ra".into());
    std::fs::write(
        &archive,
        crate::zip::tests_archive(&["A/First.nes", "A/Target.nes", "B/Target.nes"], false),
    )
    .unwrap();
    assert!(app.refresh_system("NES").is_none());
    app.show_here();
    app.filter = "TARGET".into();
    app.apply_filter();
    assert_eq!(app.here.len(), 1);
    assert_eq!(
        app.game_list.selected(),
        0,
        "search has its own row indexes"
    );
    assert!(mgl(&mut app).contains("small.zip/A/Target.nes"));
    let state_path = root.join("return-state.toml");
    app.position().save(&state_path).unwrap();
    let saved = crate::state::State::load(&state_path);
    app.restore_position(&saved);
    assert!(
        app.filter.is_empty(),
        "resume preserves existing search-reset behavior"
    );
    assert_eq!(
        app.game_list.selected(),
        1,
        "the launched member moved in the full list"
    );
    assert_eq!(
        row_key(&app.here[app.game_list.selected()]),
        format!("f:{}", archive.join("A/Target.nes").display()),
        "return must select the exact member, not another folder's same basename"
    );
    let mut old_saved = saved.clone();
    old_saved.selected_row = None;
    app.restore_position(&old_saved);
    assert_eq!(
        app.game_list.selected(),
        0,
        "old state files retain numeric selection"
    );
    std::fs::write(&archive, b"broken").unwrap();
    assert!(
        app.confirm_launch().is_none(),
        "changed archive must be revalidated at confirmation"
    );
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(test)]
mod tests {
    #[test]
    fn compact_details_keep_full_source_and_omit_empty_separators() {
        let details = crate::browse::Details {
            released: "1990-12-16T00:00:00".into(),
            players: "1".into(),
            publisher: "Example".into(),
            ..Default::default()
        };
        assert_eq!(
            super::compact_detail_text(&details),
            ("1990 · 1 Player".into(), "Example".into())
        );
        assert_eq!(details.released, "1990-12-16T00:00:00");
        let missing = crate::browse::Details::default();
        assert_eq!(
            super::compact_detail_text(&missing),
            (String::new(), String::new())
        );
        let players_only = crate::browse::Details {
            players: "1-2".into(),
            ..Default::default()
        };
        assert_eq!(super::compact_detail_text(&players_only).0, "1-2 Players");
        let ranged_players = browse::Details {
            players: "1–2".into(),
            ..Default::default()
        };
        assert_eq!(super::compact_detail_text(&ranged_players).0, "1–2 Players");
    }
    use super::*;

    fn picker_temp(tag: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("degauss-picker-{tag}-{}", std::process::id()));
        std::fs::remove_dir_all(&path).ok();
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn presentation_startup_preserves_explicit_and_saved_choices() {
        for default in [PresentMode::Direct, PresentMode::Staged] {
            assert_eq!(initial_present_mode(None, default, None), default);
            assert_eq!(
                initial_present_mode(Some("invalid"), default, None),
                default
            );
            for saved in [PresentMode::Direct, PresentMode::Staged] {
                assert_eq!(
                    initial_present_mode(Some(saved.label()), default, None),
                    saved
                );
                for explicit in [PresentMode::Direct, PresentMode::Staged] {
                    assert_eq!(
                        initial_present_mode(Some(saved.label()), default, Some(explicit)),
                        explicit
                    );
                    assert_eq!(
                        initial_present_mode(None, default, Some(explicit)),
                        explicit
                    );
                    assert_eq!(
                        initial_present_mode(Some("invalid"), default, Some(explicit)),
                        explicit
                    );
                }
            }
        }
    }

    #[test]
    fn pack_location_labels_put_present_mapped_folders_before_long_paths() {
        let root = picker_temp("location-label");
        std::fs::create_dir_all(root.join("FDS/Artwork")).unwrap();
        std::fs::create_dir_all(root.join("NES/Artwork")).unwrap();

        let label = pack_location_label("FDS", &root);

        assert!(label.starts_with("[FDS, NES] "));
        assert!(label.ends_with(&format!("-{}", std::process::id())));
        assert!(label.contains("..."));
        assert!(label.chars().count() <= 46);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pack_location_labels_preserve_the_identifying_path_tail() {
        assert_eq!(
            shortened_path_tail("/media/fat/docs", 24),
            "/media/fat/docs"
        );
        let shortened =
            shortened_path_tail("/media/usb0/collections/external/game-artwork/docs", 28);
        assert!(shortened.chars().count() <= 28);
        assert!(shortened.starts_with("..."));
        assert!(shortened.ends_with("game-artwork/docs"));
    }

    #[test]
    fn artwork_pack_health_messages_fit_the_small_overlay_and_hide_diagnostics() {
        use crate::artwork_pack::ProviderHealth;

        assert_eq!(
            artwork_pack_health_message("SuperGrafx", ProviderHealth::Ready),
            None
        );
        for health in [
            ProviderHealth::Degraded,
            ProviderHealth::Unavailable,
            ProviderHealth::Invalid,
        ] {
            let message = artwork_pack_health_message("SuperGrafx", health).unwrap();
            assert_eq!(message.lines().count(), 2);
            assert!(!message.contains("/media/"));
            assert!(!message.contains("manifest.tsv"));
        }
    }

    #[test]
    fn source_failure_messages_distinguish_storage_data_and_internal_failures() {
        let io = crate::error::DegaussError::io(
            "reading Artwork Pack",
            "/media/usb0/docs",
            std::io::Error::from(std::io::ErrorKind::NotFound),
        );
        let malformed = crate::error::DegaussError::malformed(
            "Artwork Pack",
            "/media/usb0/docs/SuperGrafx/Artwork/manifest.tsv",
            "private diagnostic detail",
        );
        let unsupported =
            crate::error::DegaussError::unsupported("Artwork Pack", "private diagnostic detail");

        let io_message = source_change_failure_message(&io);
        assert!(io_message.contains("selected storage"));
        assert!(!io_message.contains("/media/"));
        let malformed_message = source_change_failure_message(&malformed);
        assert!(malformed_message.contains("Repair the Artwork Pack"));
        assert!(!malformed_message.contains("manifest.tsv"));
        let descriptor = crate::error::DegaussError::malformed(
            "game descriptor",
            "/games/Arcade/Example.mra",
            "private XML diagnostic",
        );
        let descriptor_message = source_change_failure_message(&descriptor);
        assert!(descriptor_message.contains("Repair the game descriptor"));
        assert!(descriptor_message.contains("degauss.log"));
        assert!(!descriptor_message.contains("Artwork Pack"));
        assert!(!descriptor_message.contains("/games/"));
        assert!(!descriptor_message.contains("private XML diagnostic"));
        let unsupported_message = source_change_failure_message(&unsupported);
        assert!(unsupported_message.contains("degauss.log"));
        assert!(!unsupported_message.contains("private diagnostic detail"));
    }

    #[test]
    fn source_chooser_keeps_the_active_source_visible_after_highlight_moves() {
        assert_eq!(
            source_choice_rows(crate::artwork_source::Mode::Gamelist),
            vec!["Automatic", "Gamelist (Current)", "Artwork Pack"]
        );
        assert_eq!(
            source_choice_rows(crate::artwork_source::Mode::ArtworkPack),
            vec!["Automatic", "Gamelist", "Artwork Pack (Current)"]
        );
        assert_eq!(
            source_choice_rows(crate::artwork_source::Mode::Automatic),
            vec!["Automatic (Current)", "Gamelist", "Artwork Pack"]
        );
    }

    #[test]
    fn source_progress_labels_and_values_come_from_the_same_row_definition() {
        let progress = crate::source_cache::Progress {
            current: "SuperGrafx".to_string(),
            system: 2,
            systems: 3,
            folders: 4,
            games: 5,
            hashed_files: 6,
            hashed_bytes: 1_572_864,
        };
        let compact = source_progress_rows(&progress, true, "Location");
        assert_eq!(
            compact,
            [
                ("Location".to_string(), "SuperGrafx".to_string()),
                ("Progress".to_string(), "2 / 3".to_string()),
            ]
        );

        let complete = source_progress_rows(&progress, false, "System");
        assert_eq!(
            complete,
            [
                ("System".to_string(), "SuperGrafx".to_string()),
                ("Progress".to_string(), "2 / 3".to_string()),
                ("Folders".to_string(), "4".to_string()),
                ("Games".to_string(), "5".to_string()),
                ("CRC scan".to_string(), "6 files, 1.5 MiB".to_string()),
            ]
        );
    }

    #[test]
    fn directory_picker_stays_inside_the_configured_mount_root() {
        let root = picker_temp("allowed-root");
        let mount = root.join("mount");
        let games = mount.join("games");
        let docs = mount.join("docs");
        let outside = root.join("outside");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&docs).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        let game_roots = vec![games.to_string_lossy().into_owned()];

        assert!(directory_allowed_for(&docs, &game_roots));
        assert!(!directory_allowed_for(&outside, &game_roots));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn pack_browser_starts_above_standard_mounts_and_at_a_configured_mount_fallback() {
        let root = picker_temp("browser-root");
        let media = root.join("media");
        std::fs::create_dir_all(media.join("fat/docs")).unwrap();
        std::fs::create_dir_all(media.join("usb0/docs")).unwrap();
        assert_eq!(pack_browser_root_at(&media, &[]), media);

        let absent_media = root.join("absent-media");
        let configured_mount = root.join("configured");
        let games = configured_mount.join("games");
        std::fs::create_dir_all(&games).unwrap();
        assert_eq!(
            pack_browser_root_at(&absent_media, &[games.to_string_lossy().into_owned()]),
            configured_mount
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn directory_picker_hides_symlink_cycles_duplicates_and_external_targets() {
        use std::os::unix::fs::symlink;

        let root = picker_temp("symlink-cycle");
        let mount = root.join("mount");
        let games = mount.join("games");
        let docs = mount.join("docs");
        let child = docs.join("real-child");
        let outside = root.join("outside");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        symlink(&docs, docs.join("loop")).unwrap();
        symlink(&child, docs.join("alias-child")).unwrap();
        symlink(&outside, docs.join("outside-link")).unwrap();
        let canonical_docs = std::fs::canonicalize(&docs).unwrap();
        let game_roots = vec![games.to_string_lossy().into_owned()];

        let children = artwork_directory_children(&docs, &game_roots, &[canonical_docs]).unwrap();

        assert_eq!(
            children.len(),
            1,
            "aliases of one directory are deduplicated"
        );
        assert_eq!(children[0].1, std::fs::canonicalize(child).unwrap());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn source_choice_persists_through_pack_gamelist_and_restart() {
        let root = picker_temp("source-persistence");
        let settings_path = root.join("settings.toml");
        let docs = root.join("docs");
        let initial = Settings::default();

        let (pack, label, warning) = persist_source_choice(
            &initial,
            &settings_path,
            "SuperGrafx",
            &crate::source_cache::Target::ArtworkPack {
                docs_root: docs.clone(),
            },
        )
        .unwrap();
        assert_eq!(label, SOURCE_ARTWORK_PACK);
        assert!(warning.is_none());
        assert_eq!(
            Settings::load(&settings_path)
                .unwrap()
                .artwork_pack_roots
                .get("SuperGrafx"),
            Some(&docs.to_string_lossy().into_owned())
        );

        let (gamelist, label, warning) = persist_source_choice(
            &pack,
            &settings_path,
            "SuperGrafx",
            &crate::source_cache::Target::Gamelist,
        )
        .unwrap();
        assert_eq!(label, SOURCE_GAMELIST);
        assert!(warning.is_none());
        assert!(gamelist.artwork_pack_roots.is_empty());
        assert!(Settings::load(&settings_path)
            .unwrap()
            .gamelist_sources
            .contains("SuperGrafx"));
        assert!(
            Settings::load(&settings_path)
                .unwrap()
                .artwork_pack_roots
                .is_empty(),
            "a restart must return to Gamelist"
        );
        let (automatic, label, _) = persist_source_mode(
            &gamelist,
            &settings_path,
            "SuperGrafx",
            &crate::source_cache::Target::Gamelist,
            true,
        )
        .unwrap();
        assert_eq!(label, SOURCE_AUTOMATIC);
        assert!(automatic.artwork_pack_roots.is_empty());
        assert!(Settings::load(&settings_path)
            .unwrap()
            .gamelist_sources
            .is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_source_switch_replaces_the_shared_index_summary_with_the_staged_cache() {
        let root = picker_temp("source-index-summary");
        let games = root.join("games");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(games.join("One.rom"), b"one").unwrap();
        std::fs::write(games.join("Two.rom"), b"two").unwrap();
        let system = found_system_with_extensions("Counted", vec![games], &["rom"]);
        let library = Library::open_source_neutral(
            &system.to_config(),
            crate::browse::DisplayNames::default(),
        )
        .unwrap();
        let staged = crate::cache::StagedSystemCache {
            id: "Counted".to_string(),
            cache: crate::cache::build_system(&library),
            fingerprints: Default::default(),
            fingerprints_complete: true,
        };
        let mut index = crate::cache::Index::new();
        index.systems.insert(
            "Counted".to_string(),
            crate::cache::Summary {
                games: 0,
                folders: 0,
            },
        );

        update_index_summaries(&mut index, &[system], &[staged]);

        assert_eq!(index.systems["Counted"].games, 2);
        assert!(index.systems["Counted"].folders > 0);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_settings_save_leaves_the_old_source_active_after_cache_install() {
        let root = picker_temp("source-save-failure");
        let games = root.join("games");
        let cache_dir = root.join("cache");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::write(games.join("Game.rom"), b"rom").unwrap();
        let config = SystemConfig {
            preserve_rbf_stem: false,
            name: "Test".to_string(),
            path: games.to_string_lossy().into_owned(),
            extensions: vec!["rom".to_string()],
            rbf: "_Console/Test".to_string(),
            launch: Vec::new(),
            setname: None,
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        };
        let library =
            Library::open_source_neutral(&config, crate::browse::DisplayNames::default()).unwrap();
        let cache = crate::cache::build_system(&library);
        crate::cache::save_system(&cache_dir, "SuperGrafx", &cache).unwrap();
        let gamelist_before =
            std::fs::read(crate::cache::system_path(&cache_dir, "SuperGrafx")).unwrap();
        let cancelled = std::sync::atomic::AtomicBool::new(false);
        let prepared = crate::cache::stage_transactional(
            &cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            vec![crate::cache::StagedSystemCache {
                id: "SuperGrafx".to_string(),
                cache,
                fingerprints: Default::default(),
                fingerprints_complete: true,
            }],
            &cancelled,
        )
        .unwrap()
        .unwrap();
        prepared.install().unwrap();

        let blocked_settings_path = root.join("settings-is-a-directory");
        std::fs::create_dir(&blocked_settings_path).unwrap();
        let current = Settings::default();
        let result = persist_source_choice(
            &current,
            &blocked_settings_path,
            "SuperGrafx",
            &crate::source_cache::Target::ArtworkPack {
                docs_root: root.join("docs"),
            },
        );

        assert!(result.is_err());
        assert!(current.artwork_pack_roots.is_empty());
        assert_eq!(
            std::fs::read(crate::cache::system_path(&cache_dir, "SuperGrafx")).unwrap(),
            gamelist_before,
            "the established Gamelist cache remains the active source"
        );
        assert!(crate::cache::load_artwork_pack_data(&cache_dir, "SuperGrafx").is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn saved_text_choice_stays_authoritative_and_bad_names_use_smooth() {
        assert_eq!(resolved_font(Some("pixel 2"), "smooth"), Font::Pixel2);
        assert_eq!(
            resolved_font(Some("smooth 2"), "pixel"),
            Font::Smooth2,
            "a user override must win after a theme supplied its default"
        );
        assert_eq!(resolved_font(None, "pixel"), Font::Pixel);
        assert_eq!(resolved_font(Some("not a font"), "also bad"), Font::Smooth);
    }

    #[test]
    fn themes_resolve_from_the_system_font_without_inheriting_each_other() {
        assert_eq!(
            effective_theme_font(None, Font::Pixel2, false),
            Font::Pixel2
        );
        assert_eq!(
            effective_theme_font(Some(Font::Smooth2), Font::Pixel2, false),
            Font::Smooth2
        );
        assert_eq!(
            effective_theme_font(None, Font::Pixel, false),
            Font::Pixel,
            "a fontless theme returns to the system font, not the prior theme"
        );
        assert_eq!(
            effective_theme_font(Some(Font::Smooth2), Font::Pixel, true),
            Font::Pixel,
            "an explicit Text change overrides the current theme and survives restart"
        );

        assert_eq!(
            restored_theme_fonts(Some(Font::Smooth2), Some("smooth 2"), "smooth", None),
            (Font::Smooth, Font::Smooth2, false),
            "a legacy automatic setting write must not become the system font"
        );
        assert_eq!(
            restored_theme_fonts(Some(Font::Smooth2), Some("pixel"), "smooth", None),
            (Font::Pixel, Font::Pixel, true),
            "a different legacy value proves that Text was changed manually"
        );
        assert_eq!(
            restored_theme_fonts(Some(Font::Smooth2), Some("smooth 2"), "pixel", Some(false)),
            (Font::Smooth2, Font::Smooth2, false),
            "the explicit new marker makes the saved system font authoritative"
        );
        assert_eq!(
            restored_theme_fonts(None, Some("pixel 2"), "smooth", None),
            (Font::Pixel2, Font::Pixel2, false),
            "released fontless themes preserve their existing Text setting"
        );
    }

    #[test]
    fn the_config_accepts_exactly_the_left_right_modes_that_exist() {
        // The config validates left_right against its own word list, so
        // that list and the interface's modes must never drift apart: a
        // mode the config refused could not be written down, and a word
        // the interface ignores would pass validation and mean nothing.
        assert_eq!(
            crate::config::LEFT_RIGHT_VALUES.len(),
            Horizontal::ALL.len()
        );
        for mode in Horizontal::ALL {
            assert!(
                crate::config::LEFT_RIGHT_VALUES.contains(&mode.label()),
                "{} is not in the config's list",
                mode.label()
            );
            assert!(Horizontal::parse(mode.label()).is_some());
        }
    }

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
    fn option_actions_are_reachable_only_through_a() {
        let actions = [
            (
                OptionId::ResetCustomViews,
                OptionOperation::ConfirmResetCustomViews,
            ),
            (OptionId::ResetHidden, OptionOperation::ConfirmResetHidden),
            (OptionId::RebuildCache, OptionOperation::RebuildCache),
            (OptionId::ScrapeAll, OptionOperation::OpenScraperAll),
            (OptionId::Advanced, OptionOperation::OpenAdvanced),
        ];
        for (option, activated) in actions {
            assert_eq!(
                option_operation(option, OptionInput::Previous),
                OptionOperation::None
            );
            assert_eq!(
                option_operation(option, OptionInput::Next),
                OptionOperation::None
            );
            assert_eq!(option_operation(option, OptionInput::Activate), activated);
        }
        for input in [
            OptionInput::Previous,
            OptionInput::Next,
            OptionInput::Activate,
        ] {
            assert_eq!(
                option_operation(OptionId::Spacer, input),
                OptionOperation::None
            );
        }
        assert_eq!(
            option_operation(OptionId::Theme, OptionInput::Previous),
            OptionOperation::Adjust(-1)
        );
        assert_eq!(
            option_operation(OptionId::Theme, OptionInput::Next),
            OptionOperation::Adjust(1)
        );
        assert_eq!(
            option_operation(OptionId::Theme, OptionInput::Activate),
            OptionOperation::OpenThemeEditor
        );
    }

    #[test]
    fn every_value_uses_left_for_previous_and_right_or_a_for_next() {
        let actions = [
            OptionId::ResetCustomViews,
            OptionId::ResetHidden,
            OptionId::RebuildCache,
            OptionId::ScrapeAll,
            OptionId::Advanced,
            OptionId::Theme,
            OptionId::Spacer,
        ];
        for option in OPTIONS.iter().chain(ADVANCED.iter()).copied() {
            if actions.contains(&option) {
                continue;
            }
            assert_eq!(
                option_operation(option, OptionInput::Previous),
                OptionOperation::Adjust(-1),
                "{option:?} left"
            );
            assert_eq!(
                option_operation(option, OptionInput::Next),
                OptionOperation::Adjust(1),
                "{option:?} right"
            );
            assert_eq!(
                option_operation(option, OptionInput::Activate),
                OptionOperation::Adjust(1),
                "{option:?} A"
            );
        }
    }

    #[test]
    fn ordered_view_values_have_opposite_directions_and_wrap() {
        assert_eq!(Layout::Details.prev(), Layout::Gallery);
        assert_eq!(Layout::Details.next(), Layout::Tiled);
        for layout in Layout::ALL {
            assert_eq!(layout.prev().next(), layout);
            assert_eq!(layout.next().prev(), layout);
        }
    }

    #[test]
    fn unhide_data_changes_only_after_the_confirmation_operation() {
        let mut settings = Settings {
            hidden: vec!["PSX".into()],
            hidden_paths: vec!["d:/media/fat/games/SNES/Hacks".into()],
            ..Default::default()
        };
        let before = settings.clone();

        assert_eq!(
            option_operation(OptionId::ResetHidden, OptionInput::Previous),
            OptionOperation::None
        );
        assert_eq!(settings, before, "left cannot change hidden data");
        assert_eq!(
            option_operation(OptionId::ResetHidden, OptionInput::Activate),
            OptionOperation::ConfirmResetHidden
        );
        assert_eq!(settings, before, "opening the question changes nothing");

        assert_eq!(clear_hidden(&mut settings), 2);
        assert!(settings.hidden.is_empty());
        assert!(settings.hidden_paths.is_empty());
        let path =
            std::env::temp_dir().join(format!("degauss-reset-hidden-{}.toml", std::process::id()));
        settings.save(&path).expect("confirmed reset saves");
        let reloaded = Settings::load(&path).expect("saved reset reloads");
        assert!(reloaded.hidden.is_empty());
        assert!(reloaded.hidden_paths.is_empty());
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn build_progress_never_consumes_controls_under_the_message() {
        assert!(message_consumes_input(true, false));
        assert!(
            !message_consumes_input(true, true),
            "index progress repaints every frame while all screens remain interactive"
        );
        assert!(!message_consumes_input(false, false));
        assert!(!message_consumes_input(false, true));
    }

    #[test]
    fn the_menu_offers_hiding_only_where_it_makes_sense() {
        let systems = menu_entries(Browsing::Systems, Some("Commodore 64"), true);
        assert!(
            systems.iter().any(|e| e == "Hide Commodore 64"),
            "hiding belongs on the systems list"
        );
        let games = menu_entries(Browsing::Games, Some("Commodore 64"), true);
        assert!(
            !games.iter().any(|e| e.starts_with("Hide ")),
            "there is nothing to hide while looking at games"
        );
        for menu in [&systems, &games] {
            assert!(menu.iter().any(|e| e == "Options"));
            assert!(menu.iter().any(|e| e.starts_with("Exit")));
            assert!(!menu.iter().any(|e| e == "Scrape All Systems"));
        }
    }

    #[test]
    fn scraper_entry_points_are_scoped_and_never_implied() {
        let systems = context_entries(
            Browsing::Systems,
            false,
            None,
            None,
            false,
            ContextActions {
                scrape_scope: true,
                ..ContextActions::default()
            },
        );
        assert!(systems.iter().any(|entry| entry == SCRAPE_SYSTEM));
        assert!(!systems.iter().any(|entry| entry == SCRAPE_FOLDER));
        assert!(!systems.iter().any(|entry| entry == SCRAPE_GAME));

        let game = context_entries(
            Browsing::Games,
            false,
            Some(false),
            None,
            false,
            ContextActions {
                scrape_scope: true,
                scrape_game: true,
                ..ContextActions::default()
            },
        );
        assert!(game.iter().any(|entry| entry == SCRAPE_FOLDER));
        assert!(game.iter().any(|entry| entry == SCRAPE_GAME));

        let blocked = context_entries(
            Browsing::Games,
            false,
            Some(true),
            None,
            false,
            ContextActions::default(),
        );
        assert!(!blocked.iter().any(|entry| entry.starts_with("Scrape ")));
    }

    #[test]
    fn scraper_keyboard_covers_case_symbols_and_masks_passwords() {
        let lower = scraper_keyboard_keys(ScraperKeyboardPage::Lower);
        let upper = scraper_keyboard_keys(ScraperKeyboardPage::Upper);
        let symbols = scraper_keyboard_keys(ScraperKeyboardPage::Symbols);
        assert!(lower.iter().any(|(_, character)| *character == 'a'));
        assert!(upper.iter().any(|(_, character)| *character == 'A'));
        assert!(symbols.iter().any(|(_, character)| *character == '!'));
        for page in [&lower, &upper, &symbols] {
            assert!(page
                .iter()
                .any(|(label, character)| { label == "SP" && *character == ' ' }));
        }
        assert_eq!(
            scraper_draft_display(ScraperField::Username, "player"),
            "player"
        );
        assert_eq!(
            scraper_draft_display(ScraperField::Password, "private-value"),
            "*************"
        );
        assert!(
            !scraper_draft_display(ScraperField::Password, "private-value")
                .contains("private-value")
        );
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
    fn theme_editor_help_contains_every_mode_action_in_title_case() {
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 0, false),
            "<> Change   A Change   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 1, false),
            "A Edit   B Back   X Swap"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 11, false),
            "<> Change   A Edit   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 12, false),
            "<> Change   A Change   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 13, false),
            "<> Change   A Change   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 14, false),
            "A Save As   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 15, false),
            "A Cancel   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Hex, 0, false),
            "A Use B Back X RGB Y Reset"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Picker, 0, false),
            "A Apply   B Cancel   X Hex   Y Reset"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Swap, 0, false),
            "A Swap   B Cancel"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Name, 0, false),
            "A Type B Back X Del Y Clear"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Discard, 0, false),
            "A Choose   B Keep Editing"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 15, true),
            "A Delete   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Browse, 16, true),
            "A Cancel   B Back"
        );
        assert_eq!(
            theme_editor_help(EditorMode::Delete, 0, true),
            "A Choose   B Keep Theme"
        );
    }

    #[test]
    fn only_the_long_theme_control_list_scrolls() {
        assert_eq!(theme_editor_visible_items(EditorMode::Browse, 17, 16), 16);
        assert_eq!(theme_editor_visible_items(EditorMode::Name, 42, 16), 42);
        assert_eq!(theme_editor_visible_items(EditorMode::Delete, 2, 16), 2);
    }

    #[test]
    fn layout_names_round_trip() {
        for layout in Layout::ALL {
            assert_eq!(Layout::parse(layout.label()), Some(layout));
        }
        assert_eq!(Layout::parse("preview"), Some(Layout::Details));
        assert_eq!(Layout::parse("covers"), Some(Layout::Tiled));
        assert_eq!(Layout::parse("nonsense"), None);
    }

    #[test]
    fn custom_view_identity_covers_every_level_without_cross_system_collisions() {
        let mut views = CustomViews::default();
        let categories = ViewPlace::Categories;
        let systems = ViewPlace::Systems("Console: handhelds / imports".into());
        let same_place = "d:/media/fat/games/SMS";
        let master_system = ViewPlace::Games {
            system: "SMS".into(),
            place: same_place.into(),
        };
        let game_gear = ViewPlace::Games {
            system: "GameGear".into(),
            place: same_place.into(),
        };

        categories.set(&mut views, "list".into());
        systems.set(&mut views, "tiled".into());
        master_system.set(&mut views, "details".into());
        game_gear.set(&mut views, "carousel".into());

        assert_eq!(categories.get(&views), Some("list"));
        assert_eq!(systems.get(&views), Some("tiled"));
        assert_eq!(master_system.get(&views), Some("details"));
        assert_eq!(game_gear.get(&views), Some("carousel"));
        assert_eq!(views.len(), 4);

        assert!(master_system.remove(&mut views));
        assert_eq!(master_system.get(&views), None);
        assert_eq!(game_gear.get(&views), Some("carousel"));
        assert!(!views.games.contains_key("SMS"), "empty maps are removed");
    }

    #[test]
    fn view_resolution_is_temporary_then_custom_then_global() {
        assert_eq!(
            resolved_layout(Some(Layout::Carousel), Some("list"), Layout::Details),
            Layout::Carousel
        );
        assert_eq!(
            resolved_layout(None, Some("covers"), Layout::Details),
            Layout::Tiled,
            "released aliases remain valid as custom views"
        );
        assert_eq!(
            resolved_layout(None, Some("not-a-view"), Layout::List),
            Layout::List,
            "an unknown custom value uses the global view"
        );
        assert_eq!(resolved_layout(None, None, Layout::List), Layout::List);
    }

    fn found_system(id: &str, paths: &[&str]) -> FoundSystem {
        found_system_with_extensions(id, paths.iter().map(PathBuf::from).collect(), &["rom"])
    }

    fn found_system_with_extensions(
        id: &str,
        paths: Vec<PathBuf>,
        extensions: &[&str],
    ) -> FoundSystem {
        found_system_with_core(id, paths, extensions, "_Console/Test", None)
    }

    fn found_system_with_core(
        id: &str,
        paths: Vec<PathBuf>,
        extensions: &[&str],
        rbf: &str,
        setname: Option<&str>,
    ) -> FoundSystem {
        let extensions = extensions
            .iter()
            .map(|extension| format!("{extension:?}"))
            .collect::<Vec<_>>()
            .join(", ");
        let setname = setname
            .map(|setname| format!("setname = {setname:?}\n"))
            .unwrap_or_default();
        let text = format!(
            "[[systems]]\nname = {id:?}\nid = {id:?}\nfolders = [\"unused\"]\nrbf = {rbf:?}\nextensions = [{extensions}]\n{setname}"
        );
        let mut defs = crate::systems::parse_table(&text, Path::new("test-systems.toml"))
            .expect("test system parses");
        FoundSystem {
            def: defs.remove(0),
            paths,
            logo_dir: None,
            menu_folder: None,
        }
    }

    #[test]
    fn custom_favorites_casing_never_becomes_a_game_owner_or_scraper_target() {
        let root = picker_temp("favorite-category-casing");
        std::fs::create_dir_all(&root).unwrap();
        let game = root.join("MyShelf/Game.mgl");
        std::fs::create_dir_all(game.parent().unwrap()).unwrap();
        std::fs::write(&game, b"game").unwrap();
        for category in ["Favorites", "favorites", "FAVORITES", "FaVoRiTeS"] {
            let console = found_system_with_extensions("NES", vec![root.join("MyShelf")], &["mgl"]);
            let table = format!("[[systems]]\nname = \"Shelf\"\nid = \"CustomShelf\"\ncategory = {category:?}\nrbf = \"\"\nfolders = [\"MyShelf\"]\nextensions = [\"rbf\", \"mra\", \"mgl\"]\n");
            let table =
                crate::systems::parse_table(&table, Path::new("custom-systems.toml")).unwrap();
            let table = crate::systems::prepare_table(table, &root).unwrap();
            let mut discovered = crate::systems::discover_checked(
                &table,
                std::slice::from_ref(&root),
                None,
                &crate::systems::CoreIndex::read(&root),
            )
            .unwrap();
            let shelf = discovered.remove(0);
            assert_eq!(
                shelf.category(),
                category,
                "custom category casing survives discovery"
            );
            let systems = vec![console, shelf];
            assert_eq!(
                owner_of_path(&systems, &game).as_deref(),
                Some("NES"),
                "a Favorites shelf must not make the actual owner ambiguous: {category}"
            );
            assert!(
                scraper_scope_only_artwork_pack(
                    &crate::scraper::Scope::All,
                    &systems,
                    &std::collections::BTreeMap::from([("NES".into(), "/art".into())])
                ),
                "a Favorites shelf must not become a scrape candidate: {category}"
            );
            assert!(is_favorites(systems[1].category()));
        }
        assert!(!is_favorites("NES"));
        assert!(!is_favorites("FavoritesExtra"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn favorite_owner_requires_a_live_target_and_never_guesses_an_ambiguous_system() {
        let root = picker_temp("favorite-owner");
        let shared = root.join("shared");
        let nested = shared.join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        let bin = nested.join("Game.bin");
        let rom = shared.join("Game.rom");
        std::fs::write(&bin, b"bin").unwrap();
        std::fs::write(&rom, b"rom").unwrap();
        let systems = vec![
            found_system_with_extensions("Outer", vec![shared.clone()], &["rom"]),
            found_system_with_extensions("NestedRom", vec![nested.clone()], &["rom"]),
            found_system_with_extensions("NestedBin", vec![nested.clone()], &["bin"]),
        ];

        assert_eq!(owner_of_path(&systems, &bin).as_deref(), Some("NestedBin"));
        assert_eq!(owner_of_path(&systems, &rom).as_deref(), Some("Outer"));
        assert_eq!(owner_of_path(&systems, &shared.join("Missing.rom")), None);

        let ambiguous = vec![
            found_system_with_extensions("One", vec![shared.clone()], &["rom"]),
            found_system_with_extensions("Two", vec![shared.clone()], &["rom"]),
        ];
        assert_eq!(owner_of_path(&ambiguous, &rom), None);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn favorite_owner_uses_shared_core_setname_without_guessing_conflicts() {
        let root = picker_temp("favorite-owner-setname");
        let shared = root.join("GAMEBOY");
        std::fs::create_dir_all(&shared).unwrap();
        let game = shared.join("Shared.rom");
        let mgl = shared.join("Shared.mgl");
        std::fs::write(&game, b"game").unwrap();
        std::fs::write(&mgl, b"mgl").unwrap();
        let gameboy = found_system_with_core(
            "Gameboy",
            vec![shared.clone()],
            &["rom", "mgl"],
            "_Console/Gameboy",
            None,
        );
        let color = found_system_with_core(
            "GameboyColor",
            vec![shared],
            &["rom", "mgl"],
            "_Console/Gameboy",
            Some("GBC"),
        );
        let systems = vec![gameboy, color];
        let reference = |setname: Option<&str>| crate::favorites::FavoriteReference {
            cache_target: mgl.clone(),
            owner_target: game.clone(),
            rbf: Some("_Console/Gameboy".to_string()),
            setname: setname.map(str::to_string),
            mgl: true,
        };

        assert_eq!(
            owner_of_favorite(&systems, &reference(Some("GBC"))).as_deref(),
            Some("GameboyColor")
        );
        assert_eq!(
            owner_of_favorite(&systems, &reference(None)).as_deref(),
            Some("Gameboy"),
            "an omitted setname selects the shared core's default system"
        );
        assert_eq!(
            owner_of_favorite(&systems, &reference(Some("Conflicting"))),
            None,
            "unknown evidence must not choose between different source groups"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn neogeo_shared_group_has_one_deterministic_favorite_owner() {
        let root = picker_temp("neogeo-owner");
        let games = root.join("games");
        std::fs::create_dir_all(&games).unwrap();
        let game = games.join("Metal Slug.neo");
        std::fs::write(&game, b"neo").unwrap();
        let systems = vec![
            found_system_with_extensions("NeoGeoMVS", vec![games.clone()], &["neo"]),
            found_system_with_extensions("NeoGeo", vec![games], &["neo"]),
        ];

        assert_eq!(owner_of_path(&systems, &game).as_deref(), Some("NeoGeo"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_existing_favorite_follows_source_changes_without_cross_source_leakage() {
        let root = picker_temp("favorite-source-switch");
        let games = root.join("games");
        let cache_dir = root.join("cache");
        let docs = root.join("docs");
        let art = docs.join("SuperGrafx/Artwork");
        let favourites = root.join("_@Favorites");
        std::fs::create_dir_all(games.join("media")).unwrap();
        std::fs::create_dir_all(&art).unwrap();
        std::fs::create_dir_all(&favourites).unwrap();
        let game = games.join("Disk Name.sgx");
        let gamelist_cover = games.join("media/gamelist.jpg");
        let pack_cover = art.join("Disk Name.jpg");
        std::fs::write(&game, b"rom").unwrap();
        std::fs::write(&gamelist_cover, b"gamelist image").unwrap();
        std::fs::write(&pack_cover, b"pack image").unwrap();
        std::fs::write(
            games.join("gamelist.xml"),
            r#"<gameList><game><path>./Disk Name.sgx</path><name>Gamelist Name</name><image>./media/gamelist.jpg</image><genre>Gamelist Genre</genre><desc>Gamelist Description</desc><publisher>Gamelist Publisher</publisher><developer>Gamelist Developer</developer><releasedate>19990101T000000</releasedate><players>4</players><lang>en</lang></game></gameList>"#,
        )
        .unwrap();
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nDisk Name\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(
            art.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nDisk Name\t\t\tDisk Name\n",
        )
        .unwrap();
        let write_pack_name = |name: &str| {
            std::fs::write(
                art.join("gameinfo.tsv"),
                format!(
                    "#key\tname\tyear\tgenre\tdeveloper\tplayers\nDisk Name\t{name}\t1991\tPack Genre\tPack Developer\t1-2\n"
                ),
            )
            .unwrap();
        };
        write_pack_name("Pack Name");
        std::fs::write(
            art.join("synopsis_en.tsv"),
            "#key\tsynopsis\nDisk Name\tPack Description\n",
        )
        .unwrap();

        let config = SystemConfig {
            preserve_rbf_stem: false,
            name: "SuperGrafx".to_string(),
            path: games.to_string_lossy().into_owned(),
            extensions: vec!["sgx".to_string()],
            rbf: "_Console/TurboGrafx16".to_string(),
            launch: Vec::new(),
            setname: Some("SuperGrafx".to_string()),
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        };
        let gamelist_library =
            Library::open_with_names(&config, browse::DisplayNames::default()).unwrap();
        crate::cache::save_system(
            &cache_dir,
            "SuperGrafx",
            &crate::cache::build_system(&gamelist_library),
        )
        .unwrap();
        let pack_library =
            Library::open_source_neutral(&config, browse::DisplayNames::default()).unwrap();
        crate::cache::install_transactional(
            &cache_dir,
            crate::cache::CacheKind::ArtworkPack,
            &[crate::cache::StagedSystemCache {
                id: "SuperGrafx".to_string(),
                cache: crate::cache::build_system(&pack_library),
                fingerprints: Default::default(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();

        let favorite = favourites.join("Disk Name.mgl");
        std::fs::write(
            &favorite,
            format!(
                "<mistergamedescription><file path=\"{}\" /></mistergamedescription>",
                game.display()
            ),
        )
        .unwrap();
        let new_favorite_row = || browse::Row {
            name: "Favourite File".to_string(),
            sort_key: "favourite file".to_string(),
            kind: browse::Kind::Play(browse::Launch::File(favorite.clone())),
            cover: None,
            genre: None,
            favorite: true,
            below: None,
            details: browse::Details::default(),
        };
        let systems = vec![found_system_with_extensions(
            "SuperGrafx",
            vec![games.clone()],
            &["sgx"],
        )];
        let load_provider = |id: &str, location: &Path, cache: &crate::cache::SystemCache| {
            let mut provider = crate::artwork_pack::Provider::load(id, location, Some("en"));
            provider
                .prepare_for_cache(
                    cache,
                    &crate::cache::ContentFingerprints::new(),
                    &std::sync::atomic::AtomicBool::new(false),
                )
                .unwrap()
                .expect("test provider preparation completes");
            Some(provider)
        };

        let mut rows = vec![new_favorite_row()];
        enrich_favorite_rows(
            &mut rows,
            &systems,
            &Default::default(),
            &cache_dir,
            load_provider,
        );
        assert_eq!(rows[0].name, "Gamelist Name");
        assert_eq!(rows[0].cover.as_deref(), Some(gamelist_cover.as_path()));
        assert_eq!(rows[0].details.publisher, "Gamelist Publisher");

        let pack_roots = [(
            "SuperGrafx".to_string(),
            docs.to_string_lossy().into_owned(),
        )]
        .into();
        let mut rows = vec![new_favorite_row()];
        enrich_favorite_rows(&mut rows, &systems, &pack_roots, &cache_dir, load_provider);
        assert_eq!(rows[0].name, "Pack Name");
        assert_eq!(rows[0].cover.as_deref(), Some(pack_cover.as_path()));
        assert_eq!(rows[0].details.desc, "Pack Description");
        assert_eq!(rows[0].details.publisher, "");

        write_pack_name("Updated Pack Name");
        let mut rows = vec![new_favorite_row()];
        enrich_favorite_rows(&mut rows, &systems, &pack_roots, &cache_dir, load_provider);
        assert_eq!(rows[0].name, "Updated Pack Name");

        let mut rows = vec![new_favorite_row()];
        enrich_favorite_rows(
            &mut rows,
            &systems,
            &Default::default(),
            &cache_dir,
            load_provider,
        );
        assert_eq!(rows[0].name, "Gamelist Name");
        assert_eq!(rows[0].cover.as_deref(), Some(gamelist_cover.as_path()));

        std::fs::remove_file(&game).unwrap();
        let mut rows = vec![new_favorite_row()];
        enrich_favorite_rows(&mut rows, &systems, &pack_roots, &cache_dir, load_provider);
        assert_eq!(rows[0].name, "Favourite File");
        assert_eq!(rows[0].cover, None);
        assert_eq!(rows[0].details, browse::Details::default());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn an_amigavision_virtual_favorite_key_keeps_its_owner() {
        let root = picker_temp("amigavision-owner");
        let install = root.join("AmigaVision");
        std::fs::create_dir_all(&install).unwrap();
        let systems = vec![found_system_with_extensions(
            "Amiga",
            vec![install.clone()],
            &["adf"],
        )];
        let key = crate::favorites::amiga_key(&install, "Lotus II");

        assert_eq!(owner_of_path(&systems, &key).as_deref(), Some("Amiga"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn released_folder_views_migrate_only_to_one_proven_owner() {
        let nes_key = "d:/media/fat/games/NES";
        let archive_key = "a:/media/usb0/games/SNES/Action.zip";
        let listing_key = "l:/media/fat/games/NES/install|/media/fat/games/NES/listings/a.txt";
        let favorite_key = "d:/media/fat/_@Favorites/Arcade";
        let mut settings = Settings {
            folder_views: [
                (nes_key.into(), "list".into()),
                (archive_key.into(), "carousel".into()),
                (listing_key.into(), "future-layout".into()),
                (favorite_key.into(), "details".into()),
                ("d:/missing".into(), "tiled".into()),
            ]
            .into(),
            ..Default::default()
        };
        settings
            .custom_views
            .games
            .insert("NES".into(), [(nes_key.into(), "tiled".into())].into());
        let systems = [
            found_system("NES", &["/media/fat/games/NES"]),
            found_system("SNES", &["/media/usb0/games/SNES"]),
            found_system("Favorites", &["/media/fat/_@Favorites"]),
        ];

        migrate_legacy_views(&mut settings, &systems);

        assert!(settings.folder_views.is_empty());
        assert_eq!(settings.custom_views.games["NES"][nes_key], "tiled");
        assert_eq!(
            settings.custom_views.games["NES"][listing_key], "future-layout",
            "unknown values are preserved verbatim"
        );
        assert_eq!(settings.custom_views.games["SNES"][archive_key], "carousel");
        assert_eq!(
            settings.custom_views.games["Favorites"][favorite_key],
            "details"
        );
        assert!(settings
            .custom_views
            .games
            .values()
            .all(|places| !places.contains_key("d:/missing")));
    }

    #[test]
    fn ambiguous_legacy_paths_and_roots_are_not_guessed() {
        let shared = "d:/media/fat/games/Shared";
        let mut ambiguous_path = Settings::default();
        ambiguous_path
            .folder_views
            .insert(shared.into(), "list".into());
        migrate_legacy_views(
            &mut ambiguous_path,
            &[
                found_system("One", &["/media/fat/games/Shared"]),
                found_system("Two", &["/media/fat/games/Shared"]),
            ],
        );
        assert!(ambiguous_path.custom_views.is_empty());

        let mut unique_root = Settings::default();
        unique_root
            .folder_views
            .insert("r:".into(), "carousel".into());
        migrate_legacy_views(
            &mut unique_root,
            &[found_system("Multi", &["/one", "/two"])],
        );
        assert_eq!(unique_root.custom_views.games["Multi"]["r:"], "carousel");

        let mut ambiguous_root = Settings::default();
        ambiguous_root
            .folder_views
            .insert("r:".into(), "list".into());
        migrate_legacy_views(
            &mut ambiguous_root,
            &[
                found_system("MultiOne", &["/one", "/two"]),
                found_system("MultiTwo", &["/three", "/four"]),
            ],
        );
        assert!(ambiguous_root.custom_views.is_empty());
    }

    #[test]
    fn cycling_layouts_visits_every_one_and_returns() {
        // Every view must be reachable from the X menu, which only ever
        // steps forward: one that is not in the cycle cannot be chosen.
        let mut seen = Vec::new();
        let mut layout = Layout::Details;
        for _ in 0..Layout::ALL.len() {
            seen.push(layout);
            layout = layout.next();
        }
        assert_eq!(layout, Layout::Details, "cycling must return to the start");
        for one in Layout::ALL {
            assert!(seen.contains(&one), "{one:?} is not in the cycle");
        }
        // And each label must survive a round trip through settings.toml.
        for one in Layout::ALL {
            assert_eq!(Layout::parse(one.label()), Some(one));
        }
    }

    #[test]
    fn old_layout_indexes_are_unchanged_and_new_ones_are_appended() {
        assert_eq!(Layout::Details.index(), 0);
        assert_eq!(Layout::Tiled.index(), 1);
        assert_eq!(Layout::List.index(), 2);
        assert_eq!(Layout::Carousel.index(), 3);
        assert_eq!(Layout::MultiList.index(), 4);
        assert_eq!(Layout::Gallery.index(), 5);
    }

    #[test]
    fn gallery_is_denser_than_tiled_at_every_supported_test_geometry() {
        let config = Config::parse("[app]\n", std::path::Path::new("test")).expect("defaults");
        for (width, height) in [(352, 240), (640, 480), (1280, 720)] {
            let tiled =
                Geometry::compute(Layout::Tiled, false, false, false, width, height, &config);
            let gallery =
                Geometry::compute(Layout::Gallery, false, false, false, width, height, &config);
            assert!(
                gallery.visible > tiled.visible,
                "{width}x{height}: Gallery {} must exceed Tiled {}",
                gallery.visible,
                tiled.visible
            );
        }
    }

    #[test]
    fn multi_list_is_always_two_complete_columns() {
        let config = Config::parse("[app]\n", std::path::Path::new("test")).expect("defaults");
        for (width, height) in [(352, 240), (640, 480), (1280, 720)] {
            let geometry = Geometry::compute(
                Layout::MultiList,
                false,
                false,
                false,
                width,
                height,
                &config,
            );
            assert_eq!(geometry.columns, 2);
            assert_eq!(geometry.stride, 2);
            assert_eq!(geometry.visible % 2, 0);
        }
    }

    #[test]
    fn gallery_title_expires_and_a_new_selection_restarts_it() {
        let first = Instant::now();
        let duration = Duration::from_millis(GALLERY_TITLE_MS);
        assert!(temporary_visible(Some(first), first, duration));
        assert!(!temporary_visible(Some(first), first + duration, duration));
        let restarted = first + duration;
        assert!(temporary_visible(
            Some(restarted),
            restarted + Duration::from_millis(1),
            duration
        ));
    }

    #[test]
    fn the_latest_transient_badge_owns_the_shared_position() {
        let first = Instant::now();
        let second = first + Duration::from_millis(10);
        let now = second + Duration::from_millis(10);

        assert_eq!(transient_badges(Some(first), None, now), (true, false));
        assert_eq!(transient_badges(None, Some(first), now), (false, true));
        assert_eq!(
            transient_badges(Some(first), Some(first), now),
            (true, false),
            "simultaneous badges must never occupy the shared position together"
        );
        assert_eq!(
            transient_badges(Some(first), Some(second), now),
            (false, true),
            "a newer selection title must replace the older speed badge"
        );
        assert_eq!(
            transient_badges(Some(second), Some(first), now),
            (true, false),
            "a newer speed badge must replace the older selection title"
        );

        let after_first_expires = first + Duration::from_millis(SPEED_BADGE_MS + 1);
        assert_eq!(
            transient_badges(Some(first), Some(second), after_first_expires),
            (false, true),
            "an expired badge must not hide the other badge while it remains current"
        );
    }

    #[test]
    fn new_views_keep_the_favourites_container_heart_without_changing_old_views() {
        for layout in [
            Layout::Details,
            Layout::Tiled,
            Layout::List,
            Layout::Carousel,
        ] {
            assert!(!browse_row_favorite(layout, false, true));
            assert!(browse_row_favorite(layout, true, false));
        }
        for layout in [Layout::MultiList, Layout::Gallery] {
            assert!(browse_row_favorite(layout, false, true));
            assert!(browse_row_favorite(layout, true, false));
        }
    }

    #[test]
    fn large_and_thumbnail_failures_are_reported_once_per_path() {
        let root =
            std::env::temp_dir().join(format!("degauss-gallery-failures-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let shared = root.join("shared.png");
        let thumbnail_only = root.join("thumbnail.png");
        let mut covers = CoverCache::new(64, 4, [0, 0, 0]);
        let mut thumbnails = CoverCache::new(32, 4, [0, 0, 0]);
        assert!(covers.get(&shared).is_none());
        assert!(thumbnails.get(&shared).is_none());
        assert!(thumbnails.get(&thumbnail_only).is_none());

        let report = report_failure_caches(&[&covers, &thumbnails]).expect("two failed paths");
        let shared_text = shared.display().to_string();
        assert_eq!(
            report
                .lines()
                .filter(|line| line.contains(&shared_text))
                .count(),
            1,
            "the same failed path from both caches must be one report line"
        );
        assert!(report.contains(&thumbnail_only.display().to_string()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_artwork_scale_is_reachable_and_round_trips() {
        // The option cycles only through ALL and persists the setting token.
        // Missing either side would make a choice unreachable or forget it
        // at the next start.
        for (at, scale) in ArtworkScale::ALL.iter().copied().enumerate() {
            assert_eq!(scale.index(), at);
            assert_eq!(ArtworkScale::parse(scale.setting()), Some(scale));
        }
        assert_eq!(ArtworkScale::parse("nonsense"), None);
        assert_eq!(
            ArtworkScale::default(),
            ArtworkScale::Framebuffer,
            "an absent setting must retain the original geometry"
        );
    }

    #[test]
    fn artwork_scale_preserves_source_aspect_on_the_selected_display() {
        // A 400x200 framebuffer has 2:1 pixel geometry. On a 4:3 display its
        // pixels are narrower, so 4:3 source art needs a 1.5x raw width; on a
        // 16:9 display it needs 1.125x. Both must appear physically 4:3.
        let source_aspect = 4.0 / 3.0;
        for (scale, display_aspect) in [
            (ArtworkScale::FourThree, 4.0 / 3.0),
            (ArtworkScale::SixteenNine, 16.0 / 9.0),
        ] {
            let raw_aspect = source_aspect * scale.horizontal(400, 200);
            let physical_aspect = raw_aspect * display_aspect / 2.0;
            assert!((physical_aspect - source_aspect).abs() < 0.0001);
        }
        assert!((ArtworkScale::FourThree.horizontal(400, 200) - 1.5).abs() < 0.0001);
        assert!((ArtworkScale::SixteenNine.horizontal(400, 200) - 1.125).abs() < 0.0001);
    }

    #[test]
    fn artwork_scale_does_not_change_logos_or_screensaver_geometry() {
        // Callers classify only browse game covers as game artwork. Every
        // logo, folder stand-in and screensaver picture passes false and
        // keeps the same one-to-one framebuffer geometry in every mode.
        for scale in ArtworkScale::ALL {
            assert_eq!(artwork_horizontal(scale, 400, 200, false), 1.0);
        }
        assert_eq!(
            artwork_horizontal(ArtworkScale::FourThree, 400, 200, true),
            1.5
        );
    }

    #[test]
    fn every_left_right_mode_is_reachable_and_round_trips() {
        // The option only ever steps through ALL, so a mode missing from
        // it could never be chosen, and a label that does not parse back
        // would silently reset the setting on the next start.
        for (at, mode) in Horizontal::ALL.iter().copied().enumerate() {
            assert_eq!(mode.index(), at);
            assert_eq!(Horizontal::parse(mode.label()), Some(mode));
        }
        assert_eq!(Horizontal::parse("nonsense"), None);
        assert_eq!(
            Horizontal::default(),
            Horizontal::Speed,
            "nothing may change for anyone who does not go looking"
        );
    }

    #[test]
    fn the_stick_line_of_the_help_fits_every_mode() {
        for mode in Horizontal::ALL {
            let line = mode.help_line();
            assert!(
                line.starts_with("Stick left/right:"),
                "the line must still say which control it explains: {line:?}"
            );
            // The narrowest screen this runs on is 352 pixels wide.
            assert!(line.len() <= 46, "too long to fit: {line:?}");
        }
        assert_eq!(
            HELP[HELP_LR_ROW], "",
            "the row is filled in at draw time, not written twice"
        );
    }

    #[test]
    fn a_letter_step_lands_on_the_start_of_the_next_run() {
        let letters = ['a', 'a', 'b', 'b', 'c'];
        assert_eq!(letter_target(&letters, 0, 1), 2);
        assert_eq!(letter_target(&letters, 1, 1), 2, "from inside a run too");
        assert_eq!(letter_target(&letters, 2, 1), 4);
    }

    #[test]
    fn a_letter_step_back_lands_on_the_start_of_the_previous_run() {
        let letters = ['a', 'a', 'b', 'b', 'c'];
        assert_eq!(letter_target(&letters, 4, -1), 2);
        assert_eq!(
            letter_target(&letters, 3, -1),
            0,
            "back from inside a run skips over its own start"
        );
        assert_eq!(letter_target(&letters, 2, -1), 0);
    }

    #[test]
    fn a_letter_step_wraps_at_either_end_of_the_list() {
        // The stick has no way to say "start again"; standing at an end
        // and pressing on is unambiguous, like a move from the list edge.
        let letters = ['a', 'b', 'b', 'c', 'c'];
        assert_eq!(letter_target(&letters, 3, 1), 0);
        assert_eq!(letter_target(&letters, 4, 1), 0);
        assert_eq!(letter_target(&letters, 0, -1), 3);
    }

    #[test]
    fn a_letter_step_follows_the_list_even_where_letters_climb_twice() {
        // Folders sort before files, so first letters climb through the
        // folders and again through the files. The step walks the list as
        // it stands on screen rather than a merged alphabet.
        let letters = ['a', 'c', 'a', 'b'];
        assert_eq!(letter_target(&letters, 1, 1), 2);
        assert_eq!(letter_target(&letters, 2, -1), 1);
    }

    #[test]
    fn a_list_of_one_letter_steps_to_its_start_rather_than_spinning() {
        let letters = ['m', 'm', 'm'];
        assert_eq!(letter_target(&letters, 2, 1), 0);
        assert_eq!(letter_target(&letters, 0, -1), 0);
        assert_eq!(letter_target(&['x'], 0, 1), 0);
    }

    #[test]
    fn a_grid_steps_one_cover_until_the_sideways_keys_are_free() {
        // In every mode but Direction, left and right are spent on the
        // setting, so a whole-row step would leave the covers beside the
        // selected one unreachable. Direction frees them, and the grid
        // can be driven the way a grid reads.
        assert_eq!(vertical_step(true, false, 5), 1);
        assert_eq!(vertical_step(true, true, 5), 5);
        // A plain list moves a row at a time whatever the setting says.
        assert_eq!(vertical_step(false, false, 1), 1);
        assert_eq!(vertical_step(false, true, 1), 1);
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
        let games = context_entries(
            Browsing::Games,
            false,
            None,
            None,
            false,
            ContextActions::default(),
        );
        assert!(games.iter().any(|entry| entry == JUMP));
        assert!(games.iter().any(|entry| entry == SEARCH));
        // Nothing to clear until something has been typed.
        assert!(!games.iter().any(|entry| entry == CLEAR_SEARCH));
        assert!(context_entries(
            Browsing::Games,
            true,
            None,
            None,
            false,
            ContextActions::default(),
        )
        .iter()
        .any(|entry| entry == CLEAR_SEARCH));
        // Three groups need neither.
        let groups = context_entries(
            Browsing::Categories,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        );
        assert!(!groups.iter().any(|entry| entry == JUMP));
    }

    #[test]
    fn only_context_menu_separators_use_compact_height() {
        assert!(compact_separators(Screen::Context));
        for screen in [
            Screen::Splash,
            Screen::Browse,
            Screen::Menu,
            Screen::Options,
            Screen::ThemeEditor,
            Screen::Advanced,
            Screen::Help,
            Screen::About,
            Screen::Screensaver,
            Screen::Find,
            Screen::FavoriteFolder,
            Screen::Scraper,
            Screen::ScraperKeyboard,
            Screen::ScraperProgress,
            Screen::CategoryImage,
            Screen::ScraperMatches,
        ] {
            assert!(!compact_separators(screen), "{screen:?}");
        }
    }

    #[test]
    fn geometry_changes_queue_two_complete_frames() {
        let mut pending_complete_repaints = 0;
        let mut dirty = false;
        invalidate_geometry(&mut pending_complete_repaints, &mut dirty);
        assert!(dirty, "the first complete frame must be scheduled");
        assert_eq!(pending_complete_repaints, COMPLETE_REPAINT_FRAMES);
        assert!(take_complete_repaint(&mut pending_complete_repaints));
        assert!(take_complete_repaint(&mut pending_complete_repaints));
        assert!(!take_complete_repaint(&mut pending_complete_repaints));
    }

    #[test]
    fn context_window_counts_blank_separators_at_their_drawn_height() {
        let menu: Vec<String> = [
            "Random game",
            "Random favourite",
            "",
            "Add to favourites",
            "",
            "Jump to letter",
            "Search this folder",
            "",
            "Rebuild this system list",
            "",
            "Change view",
            "Use global view",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        let mut state = ListState::new(menu.len(), 10);
        state.select(6);
        let (range, selected) = context_window(&menu, &state, 10);
        assert_eq!(range, 0..menu.len());
        assert_eq!(range.start + selected, 6);
    }

    #[test]
    fn rebuilding_one_system_is_offered_only_inside_a_system() {
        // The entry names no system because the one meant is the one
        // being browsed. On the levels above there is no such system,
        // so offering it there would be a question with no answer.
        let games = context_entries(
            Browsing::Games,
            false,
            None,
            None,
            false,
            ContextActions::default(),
        );
        assert!(games.iter().any(|entry| entry == REBUILD_SYSTEM));
        let systems = context_entries(
            Browsing::Systems,
            false,
            None,
            None,
            false,
            ContextActions::default(),
        );
        assert!(!systems.iter().any(|entry| entry == REBUILD_SYSTEM));
        let groups = context_entries(
            Browsing::Categories,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        );
        assert!(!groups.iter().any(|entry| entry == REBUILD_SYSTEM));
    }

    #[test]
    fn default_core_reset_is_available_from_saved_state_without_enumerating_files() {
        for override_exists in [false, true] {
            let entries = context_entries(
                Browsing::Systems,
                false,
                None,
                None,
                false,
                ContextActions {
                    core_version: true,
                    core_version_override: override_exists,
                    ..ContextActions::default()
                },
            );
            assert!(entries.iter().any(|entry| entry == CORE_VERSION));
            assert_eq!(
                entries
                    .iter()
                    .any(|entry| entry == USE_DEFAULT_CORE_VERSION),
                override_exists
            );
        }
    }

    #[test]
    fn use_global_view_is_offered_only_where_an_override_exists() {
        let inherited = context_entries(
            Browsing::Categories,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        );
        assert!(inherited.iter().any(|entry| entry == CHANGE_VIEW));
        assert!(!inherited.iter().any(|entry| entry == USE_GLOBAL_VIEW));

        let custom = context_entries(
            Browsing::Categories,
            false,
            None,
            None,
            true,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        );
        assert!(custom.iter().any(|entry| entry == CHANGE_VIEW));
        assert!(custom.iter().any(|entry| entry == USE_GLOBAL_VIEW));
    }

    #[test]
    fn image_actions_exist_for_categories_and_systems_but_not_game_rows() {
        let ordinary = context_entries(
            Browsing::Categories,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        );
        assert!(ordinary.iter().any(|entry| entry == CHANGE_CATEGORY_IMAGE));
        assert!(!ordinary.iter().any(|entry| entry == CLEAR_CATEGORY_IMAGE));

        let overridden = context_entries(
            Browsing::Categories,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(true),
                ..ContextActions::default()
            },
        );
        assert!(overridden
            .iter()
            .any(|entry| entry == CHANGE_CATEGORY_IMAGE));
        assert!(overridden.iter().any(|entry| entry == CLEAR_CATEGORY_IMAGE));

        let systems = context_entries(
            Browsing::Systems,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(false),
                ..ContextActions::default()
            },
        );
        assert!(systems.iter().any(|entry| entry == CHANGE_CATEGORY_IMAGE));
        assert!(!systems.iter().any(|entry| entry == CLEAR_CATEGORY_IMAGE));

        let overridden_system = context_entries(
            Browsing::Systems,
            false,
            None,
            None,
            false,
            ContextActions {
                image_override: Some(true),
                ..ContextActions::default()
            },
        );
        assert!(overridden_system
            .iter()
            .any(|entry| entry == CHANGE_CATEGORY_IMAGE));
        assert!(overridden_system
            .iter()
            .any(|entry| entry == CLEAR_CATEGORY_IMAGE));

        let games = context_entries(
            Browsing::Games,
            false,
            None,
            None,
            false,
            ContextActions::default(),
        );
        assert!(!games.iter().any(|entry| entry == CHANGE_CATEGORY_IMAGE));
        assert!(!games.iter().any(|entry| entry == CLEAR_CATEGORY_IMAGE));
    }

    #[test]
    fn category_image_preview_tracks_the_highlighted_choice() {
        let choices = vec![
            crate::category_images::Choice {
                label: "Arcade.png".into(),
                path: PathBuf::from("/logos/Arcade.png"),
            },
            crate::category_images::Choice {
                label: "C64.png".into(),
                path: PathBuf::from("/logos/C64.png"),
            },
        ];
        assert_eq!(
            category_image_preview(&choices, 0),
            (
                Some(PathBuf::from("/logos/Arcade.png")),
                "Arcade.png".into(),
                false,
                false,
            )
        );
        assert_eq!(
            category_image_preview(&choices, 1).0,
            Some(PathBuf::from("/logos/C64.png")),
            "moving the highlight must change the preview path"
        );
        assert_eq!(
            category_image_preview(&choices, choices.len()),
            (None, String::new(), false, false),
            "a stale selection must clear the preview instead of showing old art"
        );
    }

    #[test]
    fn favourites_uses_its_image_and_keeps_the_heart_as_the_fallback() {
        let chosen = PathBuf::from("Favorites.png");
        assert_eq!(
            logo_or_favorite_heart(FAVORITES_ID, Some(chosen.clone())),
            (Some(chosen.clone()), false)
        );
        for category in [FAVORITES_ID, "favorites", "FAVORITES", "FaVoRiTeS"] {
            assert_eq!(logo_or_favorite_heart(category, None), (None, true));
            assert_eq!(
                logo_or_favorite_heart(category, Some(chosen.clone())),
                (Some(chosen.clone()), false)
            );
        }
        assert_eq!(logo_or_favorite_heart("Console", None), (None, false));
    }

    #[test]
    fn managed_system_image_wins_and_clear_restores_the_named_logo() {
        static SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let sequence = SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let logo_dir = std::env::temp_dir().join(format!(
            "degauss-app-system-image-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&logo_dir).unwrap();
        let original = logo_dir.join("Amiga.png");
        let source = logo_dir.join("Arcade.png");
        std::fs::write(&original, include_bytes!("../assets/logos/Arduboy.png")).unwrap();
        std::fs::write(&source, include_bytes!("../assets/logos/Arcade.png")).unwrap();
        let mut system = found_system("Amiga", &["/games/Amiga"]);
        system.logo_dir = Some(logo_dir.clone());

        assert_eq!(
            effective_system_logo(Some(&logo_dir), &system),
            Some(original.clone())
        );
        let managed = crate::category_images::install_system(&logo_dir, "Amiga", &source)
            .expect("valid picker image is installed");
        assert_eq!(
            effective_system_logo(Some(&logo_dir), &system),
            Some(managed),
            "the UI resolver must prefer a picker-managed image"
        );
        crate::category_images::clear_system(&logo_dir, "Amiga").unwrap();
        assert_eq!(
            effective_system_logo(Some(&logo_dir), &system),
            Some(original),
            "clearing must reveal the legacy system-ID image"
        );
        std::fs::remove_dir_all(logo_dir).unwrap();
    }

    #[test]
    fn a_jump_lands_on_the_letter_itself_wherever_it_sits() {
        // Folders first, then files, each sorted on its own: the first
        // letters climb twice, which is what broke this.
        let list = [
            ('_', true, false),
            ('a', true, false),
            ('c', true, false),
            ('s', true, false),
            ('a', false, false),
            ('b', false, false),
            ('m', false, false),
            ('z', false, false),
        ];
        // A letter the files have and the folders do not.
        assert_eq!(jump_target(&list, 'b', false), Some(5));
        assert_eq!(jump_target(&list, 'm', false), Some(6));
        assert_eq!(jump_target(&list, 'z', false), Some(7));
        // A letter both have: the first one in the list, which is the
        // folder, because that is what is on screen first.
        assert_eq!(jump_target(&list, 'a', false), Some(1));
    }

    #[test]
    fn a_jump_to_a_missing_letter_lands_after_it_among_the_files() {
        let list = [
            ('c', true, false),
            ('s', true, false),
            ('a', false, false),
            ('m', false, false),
        ];
        // No D anywhere. The next file is M, not the S folder above it.
        assert_eq!(jump_target(&list, 'd', false), Some(3));
        // Past everything.
        assert_eq!(jump_target(&list, 'z', false), None);
    }

    #[test]
    fn a_jump_works_where_every_entry_is_a_folder() {
        // Neo Geo keeps its games as folders, so there is no file run to
        // fall back to and the folders have to answer.
        let list = [('a', true, false), ('k', true, false), ('m', true, false)];
        assert_eq!(jump_target(&list, 'k', false), Some(1));
        assert_eq!(jump_target(&list, 'l', false), Some(2));
        assert_eq!(jump_target(&list, 'z', false), None);
    }

    #[test]
    fn a_jump_skips_favourites_when_they_lead() {
        let list = [
            ('a', false, true),
            ('m', false, true),
            ('a', false, false),
            ('m', false, false),
            ('z', false, false),
        ];
        assert_eq!(jump_target(&list, 'a', true), Some(2));
        assert_eq!(jump_target(&list, 'm', true), Some(3));
        assert_eq!(jump_target(&list, 'b', true), Some(3));
    }

    #[test]
    fn a_jump_keeps_favourites_eligible_on_the_favourites_shelf() {
        let list = [('a', false, true), ('m', false, true)];
        assert_eq!(jump_target(&list, 'a', false), Some(0));
        assert_eq!(jump_target(&list, 'm', false), Some(1));
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
    fn scraper_target_names_are_shortened_without_becoming_generic() {
        assert_eq!(shortened_label("Sonic", 24), "Sonic");
        assert_eq!(
            shortened_label("Sonic\n  The Hedgehog", 24),
            "Sonic The Hedgehog"
        );
        let long = shortened_label("The Great Giana Sisters Deluxe Edition", 24);
        assert_eq!(long, "The Great Giana Siste...");
        assert_eq!(long.chars().count(), 24);
    }

    #[test]
    fn manual_scraper_resolution_is_exclusive_to_unresolved_single_game_runs() {
        let game = crate::scraper::Scope::Game {
            system_id: "NES".into(),
            launch: browse::Launch::File(PathBuf::from("/games/NES/Game.nes")),
            title: "Game".into(),
        };
        let system = crate::scraper::Scope::System {
            system_id: "NES".into(),
            place: browse::Place::Dir(PathBuf::from("/games/NES")),
            display_name: "NES".into(),
        };
        let unresolved = crate::scraper::Progress {
            not_found: 1,
            ..Default::default()
        };
        let ambiguous = crate::scraper::Progress {
            ambiguous: 1,
            ..Default::default()
        };
        let resolved = crate::scraper::Progress {
            updated: 1,
            ..Default::default()
        };

        assert!(needs_manual_scraper_match(&game, true, &unresolved));
        assert!(needs_manual_scraper_match(&game, true, &ambiguous));
        assert!(!needs_manual_scraper_match(&game, true, &resolved));
        assert!(!needs_manual_scraper_match(&game, false, &unresolved));
        assert!(
            !needs_manual_scraper_match(&system, true, &unresolved),
            "multi-game scopes count and skip without entering the chooser"
        );
        assert!(!needs_manual_scraper_match(
            &crate::scraper::Scope::All,
            true,
            &ambiguous
        ));
    }

    #[test]
    fn manual_scraper_preview_events_cannot_replace_a_newer_selection() {
        let selected = crate::scraper::Match {
            id: "new".into(),
            name: "New selection".into(),
            names: vec!["New selection".into()],
            metadata: crate::scraper::Metadata::default(),
            media: None,
            rom_crc32: None,
            rom_md5: None,
            rom_sha1: None,
        };

        assert!(scraper_preview_is_current(
            Some("new"),
            Some(&selected),
            "new"
        ));
        assert!(!scraper_preview_is_current(
            Some("old"),
            Some(&selected),
            "old"
        ));
        assert!(!scraper_preview_is_current(
            Some("new"),
            Some(&selected),
            "old"
        ));
        assert!(!scraper_preview_is_current(Some("new"), None, "new"));
    }

    #[test]
    fn manual_scraper_search_errors_are_concise_and_actionable() {
        use crate::scraper::{Error, ErrorKind};

        let cases = [
            (ErrorKind::Configuration, "Search could not start"),
            (ErrorKind::Local, "Search could not start"),
            (
                ErrorKind::Authentication,
                "ScreenScraper rejected the login",
            ),
            (ErrorKind::Unavailable, "ScreenScraper is unavailable"),
            (ErrorKind::Server, "ScreenScraper is unavailable"),
            (ErrorKind::RateLimited, "ScreenScraper rate limit reached"),
            (ErrorKind::DailyQuota, "Daily request allowance exhausted"),
            (ErrorKind::FailedQuota, "Failed-search allowance exhausted"),
            (ErrorKind::NotFound, "No Matches"),
            (
                ErrorKind::MalformedResponse,
                "ScreenScraper response was unreadable",
            ),
            (ErrorKind::Transport, "Network connection failed"),
            (ErrorKind::Timeout, "ScreenScraper timed out"),
            (ErrorKind::Cancelled, "Search Cancelled"),
        ];

        for (kind, expected) in cases {
            let error = Error::new(kind, "private diagnostic detail");
            assert_eq!(scraper_search_error(&error), expected);
            assert!(!scraper_search_error(&error).contains("private diagnostic detail"));
        }
    }

    #[test]
    fn scraper_run_errors_keep_diagnostics_out_of_the_interface() {
        use crate::scraper::{Error, ErrorKind};

        let cases = [
            (ErrorKind::Configuration, "Scraper setup needs attention"),
            (
                ErrorKind::Authentication,
                "ScreenScraper rejected the login",
            ),
            (ErrorKind::Unavailable, "ScreenScraper is unavailable"),
            (ErrorKind::Server, "ScreenScraper is unavailable"),
            (ErrorKind::RateLimited, "ScreenScraper rate limit reached"),
            (ErrorKind::DailyQuota, "Daily request allowance exhausted"),
            (ErrorKind::FailedQuota, "Failed-search allowance exhausted"),
            (ErrorKind::NotFound, "No matching game was found"),
            (
                ErrorKind::MalformedResponse,
                "ScreenScraper response was unreadable",
            ),
            (ErrorKind::Transport, "Network connection failed"),
            (ErrorKind::Timeout, "ScreenScraper timed out"),
            (
                ErrorKind::Local,
                "A local game file could not be read or updated",
            ),
            (ErrorKind::Cancelled, "Scrape cancelled"),
        ];

        for (kind, expected) in cases {
            let error = Error::new(kind, "private diagnostic detail");
            assert_eq!(error.user_message(), expected);
            assert!(!error.user_message().contains("private diagnostic detail"));
        }
    }

    #[test]
    fn favouriting_is_offered_over_a_game_and_not_over_a_folder() {
        // A folder is not a favourite, and offering to make one of it
        // would write a file pointing at nothing.
        let over_folder = context_entries(
            Browsing::Games,
            false,
            None,
            None,
            false,
            ContextActions::default(),
        );
        assert!(!over_folder.iter().any(|e| e == ADD_FAVORITE));
        assert!(!over_folder.iter().any(|e| e == REMOVE_FAVORITE));

        let over_game = context_entries(
            Browsing::Games,
            false,
            Some(false),
            None,
            false,
            ContextActions::default(),
        );
        assert!(over_game.iter().any(|e| e == ADD_FAVORITE));
        assert!(!over_game.iter().any(|e| e == REMOVE_FAVORITE));

        // Already one: the only thing left to do is stop.
        let over_favourite = context_entries(
            Browsing::Games,
            false,
            Some(true),
            None,
            false,
            ContextActions::default(),
        );
        assert!(over_favourite.iter().any(|e| e == REMOVE_FAVORITE));
        assert!(!over_favourite.iter().any(|e| e == ADD_FAVORITE));
    }

    #[test]
    fn the_held_x_shortcut_is_opt_in_and_only_changes_games_outside_favourites() {
        // Existing installations have no setting, so the shortcut is absent.
        assert_eq!(
            favorite_change(false, Screen::Browse, Browsing::Games, false, true, false),
            None
        );
        // A normal game adds through the existing folder chooser; one already
        // held removes through the existing contextual-menu operation.
        assert_eq!(
            favorite_change(true, Screen::Browse, Browsing::Games, false, true, false),
            Some(FavoriteChange::Add)
        );
        assert_eq!(
            favorite_change(true, Screen::Browse, Browsing::Games, false, true, true),
            Some(FavoriteChange::Remove)
        );
        // The master shelf owns favourite files rather than source games;
        // only its ordinary contextual menu is allowed to change them.
        assert_eq!(
            favorite_change(true, Screen::Browse, Browsing::Games, true, true, true),
            None
        );
        assert_eq!(
            favorite_change(true, Screen::Browse, Browsing::Games, false, false, false),
            None,
            "folders are not games"
        );
        assert_eq!(
            favorite_change(true, Screen::Context, Browsing::Games, false, true, false),
            None,
            "the shortcut exists only while browsing"
        );
    }

    #[test]
    fn contextual_actions_have_no_blank_rows_in_grouped_pages() {
        let entries = context_entries(
            Browsing::Games,
            true,
            Some(false),
            Some(false),
            false,
            ContextActions::default(),
        );
        assert!(
            entries.iter().all(|e| !e.is_empty()),
            "pages use compact selectable rows"
        );
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
        // then hiding, then reading again, then how it looks.
        let seen: Vec<&str> = entries.iter().map(String::as_str).collect();
        let at = |what: &str| seen.iter().position(|e| *e == what).expect(what);
        assert!(at(RANDOM) < at(ADD_FAVORITE));
        assert!(at(ADD_FAVORITE) < at(JUMP));
        assert!(at(JUMP) < at(HIDE_THIS));
        assert!(at(HIDE_THIS) < at(REBUILD_SYSTEM));
        assert!(at(REBUILD_SYSTEM) < at(CHANGE_VIEW));
        // Both ways of picking something at random sit together.
        assert_eq!(at(RANDOM) + 1, at(RANDOM_FAVORITE));
    }

    #[test]
    fn operation_footer_measurement_does_not_reserve_a_hidden_browse_bar() {
        let config = Config::parse("[app]\n", Path::new("test")).unwrap();
        for (width, height) in [(352, 240), (640, 480), (1280, 720)] {
            let hidden =
                Geometry::compute(Layout::List, true, false, false, width, height, &config);
            let shown = Geometry::compute(Layout::List, true, false, true, width, height, &config);
            assert!(
                hidden.bar > 0.0,
                "operation controls need a measured footer even when Browse hides its bar"
            );
            assert_eq!(hidden.bar, shown.bar);
            let safe_height = (height as f32
                - (height as f32 * config.app.overscan_y as f32 / 100.0).round() * 2.0)
                .max(48.0);
            assert_eq!(
                hidden.row_height,
                (safe_height / 10.0).floor().max(9.0),
                "hidden browse bars must continue to use the full body height"
            );
        }
    }

    #[test]
    fn a_tube_is_measured_exactly_as_it_was_and_a_big_screen_is_not() {
        // The ceilings on text size were set for 240 lines. Raising them
        // so a 720 line screen does not look like a 240 line one with more
        // space around it must not move a single pixel on the tube, which
        // is the only screen any of this was tuned on.
        // The shipped defaults, parsed the way the program parses them.
        let config = Config::parse("[app]\n", std::path::Path::new("test")).expect("defaults");
        for layout in Layout::ALL {
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
