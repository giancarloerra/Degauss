//! Optional core update browser backed only by this MiSTer's Downloader setup.
//!
//! The module name and the persisted `show_misterzine` setting are retained
//! internally so settings written by 0.8.0 keep working. No MiSTerZine data is
//! read. The effective local Downloader configuration decides which remote
//! cores may appear, while every installed core remains visible even when no
//! configured database owns it.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};

use crate::browse::{Details, Kind, Launch, Row};
use crate::error::{DegaussError, Result};
use crate::systems::{CoreCatalogue, CoreEntry, CoreVariant};

const DEFAULT_DATABASE_ID: &str = "distribution_mister";
const DEFAULT_DATABASE_URL: &str =
    "https://raw.githubusercontent.com/MiSTer-devel/Distribution_MiSTer/main/db.json.zip";
const CACHE_FORMAT: u32 = 1;
const MAX_CONFIG_BYTES: u64 = 2 * 1024 * 1024;
const MAX_DATABASE_BYTES: u64 = 16 * 1024 * 1024;
const MAX_DATABASE_JSON_BYTES: u64 = 64 * 1024 * 1024;
const MAX_DATABASES: usize = 256;
const MAX_REMOTE_CORES: usize = 30_000;
const MAX_LOCAL_CORES: usize = 10_000;
const MAX_PATH_TEXT: usize = 1_024;
const MAX_SHORT_TEXT: usize = 512;
const CURL_POLL: Duration = Duration::from_millis(25);
const CA_BUNDLES: [&str; 3] = [
    "/etc/ssl/certs/cacert.pem",
    "/etc/ssl/cert.pem",
    "/etc/ssl/certs/ca-certificates.crt",
];
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalState {
    AvailableThroughDownloader,
    LocalOnly,
    VersionUnknown,
    Current,
    UpdateAvailable,
    NewerThanConfigured,
    KeptLocal,
}

impl LocalState {
    pub fn label(self) -> &'static str {
        match self {
            Self::AvailableThroughDownloader => "Available through Downloader",
            Self::LocalOnly => "Installed · No configured source",
            Self::VersionUnknown => "Installed · Version unknown",
            Self::Current => "Installed · Current",
            Self::UpdateAvailable => "Installed · Update available",
            Self::NewerThanConfigured => "Installed · Newer than configured",
            Self::KeptLocal => "Installed · Downloader keeps local file",
        }
    }

    fn order(self) -> u8 {
        match self {
            Self::UpdateAvailable => 0,
            Self::AvailableThroughDownloader => 1,
            Self::VersionUnknown => 2,
            Self::KeptLocal => 3,
            Self::NewerThanConfigured => 4,
            Self::Current => 5,
            Self::LocalOnly => 6,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    key: String,
    title: String,
    base: String,
    source: String,
    source_label: String,
    local_build: String,
    available_build: String,
    remote_path: String,
    state: LocalState,
    installed: bool,
    core_identity: Option<String>,
    game_count: Option<usize>,
    game: Option<Row>,
    pub launch_path: Option<PathBuf>,
    pub cover: Option<PathBuf>,
    pub logo_id: Option<String>,
}

impl Item {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn installed(&self) -> bool {
        self.installed
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    fn source(&self) -> &str {
        &self.source
    }

    pub fn source_label(&self) -> String {
        self.source_label.clone()
    }

    pub fn state_label(&self) -> &'static str {
        self.state.label()
    }

    pub fn display_title(&self) -> String {
        self.title.clone()
    }

    pub fn core_identity(&self) -> Option<&str> {
        self.core_identity.as_deref()
    }

    pub fn set_game_match(&mut self, count: usize, game: Option<Row>) {
        self.game_count = Some(count);
        self.cover = game.as_ref().and_then(|row| row.cover.clone());
        self.game = game;
    }

    pub fn game_count(&self) -> Option<usize> {
        self.game_count
    }

    pub fn has_game_match(&self) -> bool {
        self.game.is_some()
    }

    pub fn information_with_games(&self, matches: Option<&GameMatches>) -> String {
        let mut text = self.title.clone();
        for (label, value) in [
            ("Local status", self.state.label().to_string()),
            ("Type", self.base.clone()),
            ("Source", self.source_label()),
            ("Installed build", self.local_build.clone()),
            ("Configured build", self.available_build.clone()),
            ("Downloader path", self.remote_path.clone()),
        ] {
            if !value.trim().is_empty() {
                text.push_str(&format!("\n\n{label}: {value}"));
            }
        }
        let games = matches
            .map(|matches| matches.games.as_slice())
            .unwrap_or_default();
        let game = matches
            .and_then(|matches| matches.single.as_ref())
            .or(self.game.as_ref());
        if games.len() > 1 {
            text.push_str(&format!("\n\nGames using this core: {}", games.len()));
            text.push_str("\n\nGame titles");
            for game in games {
                text.push('\n');
                text.push_str(&game.name);
            }
        } else if let Some(game) = game {
            text.push_str(&format!("\n\nMatched game: {}", game.name));
            for (label, value) in [
                ("Genre", game.genre.as_deref().unwrap_or("")),
                ("Publisher", game.details.publisher.as_str()),
                ("Developer", game.details.developer.as_str()),
                ("Released", game.details.released.as_str()),
                ("Players", game.details.players.as_str()),
                ("Language", game.details.lang.as_str()),
            ] {
                if !value.trim().is_empty() {
                    text.push_str(&format!("\n\n{label}: {value}"));
                }
            }
            if !game.details.desc.trim().is_empty() {
                text.push_str("\n\nDescription\n");
                text.push_str(&game.details.desc);
            }
        } else if let Some(count) = self.game_count.filter(|count| *count > 0) {
            text.push_str(&format!("\n\nGames using this core: {count}"));
        }
        text
    }

    pub fn cover(&self) -> Option<&Path> {
        self.cover.as_deref()
    }

    pub fn logo_id(&self) -> Option<&str> {
        self.logo_id.as_deref()
    }

    pub fn row(&self, cover: Option<PathBuf>) -> Row {
        let mut context = vec![self.base.clone()];
        if !self.source_label.is_empty() {
            context.push(self.source_label.clone());
        }
        Row {
            name: self.display_title(),
            sort_key: self.title.to_ascii_lowercase(),
            kind: Kind::Play(Launch::File(self.launch_path.clone().unwrap_or_default())),
            cover,
            genre: Some(context.join(" · ")),
            favorite: false,
            below: None,
            details: Details {
                desc: self.state.label().to_string(),
                publisher: String::new(),
                developer: String::new(),
                released: self.available_build.clone(),
                players: String::new(),
                lang: String::new(),
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture(title: &str, state: LocalState, launch_path: Option<PathBuf>) -> Self {
        Self::fixture_with_key(
            title,
            title,
            "Console",
            "distribution_mister",
            state,
            launch_path,
        )
    }

    #[cfg(test)]
    pub(crate) fn fixture_with(
        title: &str,
        base: &str,
        source: &str,
        state: LocalState,
        launch_path: Option<PathBuf>,
    ) -> Self {
        Self::fixture_with_key(title, title, base, source, state, launch_path)
    }

    #[cfg(test)]
    pub(crate) fn fixture_with_key(
        key: &str,
        title: &str,
        base: &str,
        source: &str,
        state: LocalState,
        launch_path: Option<PathBuf>,
    ) -> Self {
        Self {
            key: key.to_string(),
            title: title.to_string(),
            base: base.to_string(),
            source: source.to_string(),
            source_label: readable_source(source),
            local_build: "2026-09-01".into(),
            available_build: "2026-09-16".into(),
            remote_path: format!("_{base}/{title}_20260916.rbf"),
            state,
            installed: launch_path.is_some(),
            core_identity: None,
            game_count: None,
            game: None,
            launch_path,
            cover: None,
            logo_id: None,
        }
    }
}

fn readable_source(source: &str) -> String {
    match source {
        "" | "local" => "Local file".to_string(),
        "distribution_mister" => "MiSTer Distribution".to_string(),
        "jtcores" | "jtbindb" => "Jotego".to_string(),
        "coin-opcollection/distribution-misterfpga" | "coinop" => "Coin-Op".to_string(),
        "theypsilon_unofficial_distribution" => "theypsilon".to_string(),
        other => other
            .split(['_', '-'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars
                    .next()
                    .map(|first| {
                        first
                            .to_uppercase()
                            .chain(chars.flat_map(char::to_lowercase))
                            .collect::<String>()
                    })
                    .unwrap_or_default()
            })
            .collect::<Vec<_>>()
            .join(" "),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterField {
    Type,
    Source,
}

impl FilterField {
    pub const ALL: [Self; 2] = [Self::Type, Self::Source];

    pub fn index(self) -> usize {
        match self {
            Self::Type => 0,
            Self::Source => 1,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Type => "Type",
            Self::Source => "Source",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterChoice {
    All,
    Value { label: String, key: String },
}

impl FilterChoice {
    pub fn label(&self) -> &str {
        match self {
            Self::All => "All",
            Self::Value { label, .. } => label,
        }
    }

    fn key(&self) -> Option<&str> {
        match self {
            Self::All => None,
            Self::Value { key, .. } => Some(key),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filters {
    selected: [Option<String>; 2],
}

impl Filters {
    pub fn is_active(&self) -> bool {
        self.selected.iter().any(Option::is_some)
    }

    pub fn clear(&mut self) {
        self.selected = Default::default();
    }

    pub fn label(&self, field: FilterField, items: &[Item]) -> String {
        let Some(key) = self.selected[field.index()].as_deref() else {
            return "All".to_string();
        };
        choices(items, field)
            .into_iter()
            .find(|choice| choice.key() == Some(key))
            .map(|choice| choice.label().to_string())
            .unwrap_or_else(|| key.to_string())
    }

    pub fn choose(&mut self, field: FilterField, choice: &FilterChoice) {
        self.selected[field.index()] = choice.key().map(str::to_string);
    }

    pub fn choice_is_selected(&self, field: FilterField, choice: &FilterChoice) -> bool {
        self.selected[field.index()].as_deref() == choice.key()
    }

    pub fn matches(&self, item: &Item) -> bool {
        FilterField::ALL.into_iter().all(|field| {
            let Some(wanted) = self.selected[field.index()].as_deref() else {
                return true;
            };
            let actual = match field {
                FilterField::Type => item.base.as_str(),
                FilterField::Source => item.source(),
            };
            actual.eq_ignore_ascii_case(wanted)
        })
    }
}

pub fn choices(items: &[Item], field: FilterField) -> Vec<FilterChoice> {
    let mut known = BTreeMap::<String, String>::new();
    for item in items {
        let (key, label) = match field {
            FilterField::Type => (
                item.base.trim().to_ascii_lowercase(),
                item.base.trim().to_string(),
            ),
            FilterField::Source => (
                item.source().trim().to_ascii_lowercase(),
                item.source_label(),
            ),
        };
        if !key.is_empty() {
            known.entry(key).or_insert(label);
        }
    }
    let mut values = vec![FilterChoice::All];
    values.extend(
        known
            .into_iter()
            .map(|(key, label)| FilterChoice::Value { label, key }),
    );
    values
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub updated: String,
    pub items: Vec<Item>,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub cache_dir: PathBuf,
    pub menu_root: PathBuf,
    pub cores: CoreCatalogue,
    pub force_refresh: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Downloading,
    CheckingAvailability,
    Matching,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Reading Downloader Settings",
            Self::Downloading => "Reading Core Databases",
            Self::CheckingAvailability => "Reading Archived Listings",
            Self::Matching => "Comparing Installed Cores",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Progress {
    pub phase: Phase,
    pub completed: usize,
    pub total: usize,
    pub cancelling: bool,
}

impl Default for Progress {
    fn default() -> Self {
        Self {
            phase: Phase::Checking,
            completed: 0,
            total: 0,
            cancelling: false,
        }
    }
}

#[derive(Debug)]
pub enum Event {
    Progress(Progress),
    Snapshot(Snapshot),
    Ready {
        snapshot: Snapshot,
        notice: Option<String>,
    },
    Cancelled,
    Failed {
        message: String,
    },
}

pub struct Job {
    events: Receiver<Event>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl Job {
    pub fn try_recv(&mut self) -> Option<Event> {
        match self.events.try_recv() {
            Ok(event) => {
                if matches!(
                    event,
                    Event::Ready { .. } | Event::Cancelled | Event::Failed { .. }
                ) {
                    if let Some(handle) = self.handle.take() {
                        let _ = handle.join();
                    }
                }
                Some(event)
            }
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                if let Some(handle) = self.handle.take() {
                    let _ = handle.join();
                }
                None
            }
        }
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn pending_fixture() -> Self {
        let (_sender, events) = mpsc::sync_channel(1);
        Self {
            events,
            cancelled: Arc::new(AtomicBool::new(false)),
            handle: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn cancelled_for_test(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[derive(Debug, Clone)]
pub enum GameSource {
    Gamelist,
    ArtworkPack {
        docs_root: PathBuf,
        language: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub enum GameTarget {
    System { id: String },
    Arcade { core_identity: String },
}

impl GameTarget {
    fn system_id(&self) -> &str {
        match self {
            Self::System { id } => id,
            Self::Arcade { .. } => "Arcade",
        }
    }
}

#[derive(Debug, Clone)]
pub struct GameRequest {
    pub key: String,
    pub cache_dir: PathBuf,
    pub source: GameSource,
    pub target: GameTarget,
}

#[derive(Debug, Clone)]
pub struct GamePreview {
    pub name: String,
    sort_key: String,
    pub cover: Option<PathBuf>,
}

#[derive(Debug, Default)]
pub struct GameMatches {
    pub games: Vec<GamePreview>,
    pub single: Option<Row>,
}

impl GameMatches {
    #[cfg(test)]
    pub fn from_rows(rows: Vec<Row>) -> Self {
        let mut matches = Self::default();
        for row in rows {
            matches.push(row);
        }
        matches.sort();
        matches
    }

    fn push(&mut self, row: Row) {
        if self.games.is_empty() {
            self.single = Some(row.clone());
        } else {
            self.single = None;
        }
        self.games.push(GamePreview {
            name: row.name,
            sort_key: row.sort_key,
            cover: row.cover,
        });
    }

    fn sort(&mut self) {
        self.games.sort_by(|left, right| {
            left.sort_key
                .to_ascii_lowercase()
                .cmp(&right.sort_key.to_ascii_lowercase())
                .then_with(|| {
                    left.name
                        .to_ascii_lowercase()
                        .cmp(&right.name.to_ascii_lowercase())
                })
        });
    }
}

#[derive(Debug)]
pub enum GameEvent {
    Ready { key: String, matches: GameMatches },
    Cancelled,
    Failed { key: String, error: DegaussError },
}

pub struct GameJob {
    key: String,
    event: Receiver<GameEvent>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl GameJob {
    pub fn try_recv(&mut self) -> Option<GameEvent> {
        let event = match self.event.try_recv() {
            Ok(event) => event,
            Err(TryRecvError::Empty) => return None,
            Err(TryRecvError::Disconnected) if self.handle.is_some() => GameEvent::Failed {
                key: self.key.clone(),
                error: DegaussError::unsupported(
                    "core update games",
                    "game-list worker stopped without a result",
                ),
            },
            Err(TryRecvError::Disconnected) => return None,
        };
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
        Some(event)
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

impl Drop for GameJob {
    fn drop(&mut self) {
        self.cancel();
    }
}

pub fn start_game_job(request: GameRequest) -> Result<GameJob> {
    let (sender, event) = mpsc::channel();
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let key = request.key.clone();
    let job_key = key.clone();
    let handle = std::thread::Builder::new()
        .name("degauss-core-games".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                read_core_games(&request, &worker_cancelled)
            }))
            .unwrap_or_else(|_| {
                Err(DegaussError::unsupported(
                    "core update games",
                    "game-list worker panicked",
                ))
            });
            let message = if worker_cancelled.load(Ordering::Relaxed) {
                GameEvent::Cancelled
            } else {
                match result {
                    Ok(matches) => GameEvent::Ready { key, matches },
                    Err(error) => GameEvent::Failed { key, error },
                }
            };
            let _ = sender.send(message);
        })
        .map_err(|error| {
            DegaussError::unsupported(
                "core update games",
                format!("starting game-list worker: {error}"),
            )
        })?;
    Ok(GameJob {
        key: job_key,
        event,
        cancelled,
        handle: Some(handle),
    })
}

fn read_core_games(request: &GameRequest, cancelled: &AtomicBool) -> Result<GameMatches> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(GameMatches::default());
    }
    let system_id = request.target.system_id();
    let cache = match &request.source {
        GameSource::Gamelist => crate::cache::load_system_checked(&request.cache_dir, system_id)?,
        GameSource::ArtworkPack { .. } => {
            crate::cache::load_artwork_pack_data_checked(&request.cache_dir, system_id)?
                .map(|data| data.cache)
        }
    }
    .ok_or_else(|| {
        DegaussError::unsupported(
            "core update games",
            format!("the {system_id} library cache is unavailable"),
        )
    })?;

    let wanted = match &request.target {
        GameTarget::System { .. } => None,
        GameTarget::Arcade { core_identity } => {
            let Some(map) =
                crate::cache::ensure_core_game_map(&request.cache_dir, &cache, cancelled)?
            else {
                return Ok(GameMatches::default());
            };
            Some(
                map.paths(core_identity)
                    .iter()
                    .cloned()
                    .map(Launch::File)
                    .collect::<HashSet<_>>(),
            )
        }
    };
    let provider = prepared_game_provider(request, system_id)?;
    let mut seen = HashSet::<Launch>::new();
    let mut matches = GameMatches::default();
    for row in cache.folders.values().flat_map(|folder| folder.rows.iter()) {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(GameMatches::default());
        }
        let Kind::Play(launch) = &row.kind else {
            continue;
        };
        if wanted
            .as_ref()
            .is_some_and(|wanted| !wanted.contains(launch))
            || !seen.insert(launch.clone())
        {
            continue;
        }
        let mut row = row.clone();
        if let Some(provider) = provider.as_ref() {
            provider.apply_prepared(std::slice::from_mut(&mut row));
        }
        matches.push(row);
    }
    matches.sort();
    Ok(matches)
}

fn prepared_game_provider(
    request: &GameRequest,
    system_id: &str,
) -> Result<Option<crate::artwork_pack::Provider>> {
    let GameSource::ArtworkPack {
        docs_root,
        language,
    } = &request.source
    else {
        return Ok(None);
    };
    let state =
        crate::cache::load_pack_source_state(&request.cache_dir, system_id)?.ok_or_else(|| {
            DegaussError::unsupported(
                "core update games",
                format!("the {system_id} Artwork Pack state is unavailable"),
            )
        })?;
    let accepted = state
        .accepted
        .as_ref()
        .filter(|accepted| Path::new(&accepted.docs_root) == docs_root)
        .ok_or_else(|| {
            DegaussError::unsupported(
                "core update games",
                format!("the {system_id} Artwork Pack selection is not prepared"),
            )
        })?;
    let kept = state.declined.as_ref().filter(|declined| {
        Path::new(&declined.docs_root) == docs_root && declined.signature.is_some()
    });
    let current = crate::artwork_pack::normalized_language(language.as_deref());
    let prepared_language = if kept.is_some_and(|kept| kept.language == current) {
        current
    } else {
        accepted.language.clone()
    };
    let prepared = crate::cache::load_pack_prepared_map(&request.cache_dir, system_id)?
        .ok_or_else(|| {
            DegaussError::unsupported(
                "core update games",
                format!("the {system_id} Artwork Pack mapping is unavailable"),
            )
        })?;
    Ok(Some(crate::artwork_pack::Provider::from_prepared_state(
        system_id,
        docs_root,
        prepared_language.as_deref(),
        accepted.health,
        accepted.diagnostics.clone(),
        accepted.signature.clone(),
        prepared,
    )))
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct Cache {
    format: u32,
    checked: u64,
    global_filter: String,
    global_filter_defined: bool,
    configuration: Vec<ConfiguredDatabase>,
    cores: Vec<RemoteCore>,
    local_hashes: Vec<LocalHash>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct ConfiguredDatabase {
    id: String,
    url: String,
    description: String,
    filter: Option<String>,
}

impl ConfiguredDatabase {
    fn label(&self) -> String {
        if self.description.trim().is_empty() {
            readable_source(&self.id)
        } else {
            self.description.trim().to_string()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DownloaderConfig {
    global_filter: String,
    global_filter_defined: bool,
    databases: Vec<ConfiguredDatabase>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct RemoteCore {
    database: String,
    source_label: String,
    path: String,
    identity: String,
    title: String,
    category: String,
    hash: String,
    size: u64,
    overwrite: bool,
    build: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct LocalHash {
    path: String,
    size: u64,
    modified: u64,
    hash: String,
}

#[derive(Debug, Clone)]
struct LocalCore {
    relative: String,
    identity: String,
    title: String,
    category: String,
    build: String,
    file_path: PathBuf,
    launch_path: Option<PathBuf>,
    logo_id: Option<String>,
}

#[derive(Debug, Default)]
struct IniDocument {
    defaults: BTreeMap<String, String>,
    sections: Vec<IniSection>,
    duplicate_sections: Vec<String>,
}

#[derive(Debug)]
struct IniSection {
    name: String,
    values: BTreeMap<String, String>,
}

#[derive(Deserialize, Debug)]
struct Manifest {
    #[serde(default)]
    v: u32,
    db_id: String,
    timestamp: u64,
    files: BTreeMap<String, FileDescription>,
    #[serde(default)]
    tag_dictionary: BTreeMap<String, i64>,
    #[serde(default)]
    default_options: DefaultOptions,
    #[serde(default)]
    archives: BTreeMap<String, ArchiveDescription>,
}

#[derive(Deserialize, Debug, Default)]
struct DefaultOptions {
    #[serde(default)]
    filter: Option<String>,
}

#[derive(Deserialize, Debug, Clone, Default)]
struct FileDescription {
    #[serde(default)]
    hash: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    tags: Vec<serde_json::Value>,
    #[serde(default = "default_true")]
    overwrite: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Debug, Default)]
struct ArchiveDescription {
    #[serde(default)]
    extract: String,
    #[serde(default)]
    target_folder: String,
    #[serde(default)]
    summary_inline: Option<ArchiveSummary>,
    #[serde(default)]
    internal_summary: Option<ArchiveSummary>,
    #[serde(default)]
    summary_file: Option<SummaryFile>,
}

#[derive(Deserialize, Debug, Default)]
struct ArchiveSummary {
    #[serde(default)]
    files: BTreeMap<String, FileDescription>,
}

#[derive(Deserialize, Debug, Clone)]
struct SummaryFile {
    hash: String,
    size: u64,
    url: String,
}

#[derive(Debug)]
struct FetchError {
    user: String,
    diagnostic: String,
    cancelled: bool,
}

fn config_error(path: &Path, detail: impl Into<String>) -> DegaussError {
    DegaussError::malformed("Downloader configuration", path, detail.into())
}

fn read_limited_text(path: &Path) -> Result<String> {
    let mut bytes = Vec::new();
    match OpenOptions::new().read(true).open(path) {
        Ok(file) => file
            .take(MAX_CONFIG_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| DegaussError::io("reading Downloader configuration", path, error))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => {
            return Err(DegaussError::io(
                "opening Downloader configuration",
                path,
                error,
            ));
        }
    };
    if bytes.len() as u64 > MAX_CONFIG_BYTES {
        return Err(config_error(path, "file is larger than 2 MiB"));
    }
    String::from_utf8(bytes)
        .map_err(|error| config_error(path, format!("file is not UTF-8: {error}")))
}

fn strip_inline_comment(value: &str) -> &str {
    let bytes = value.as_bytes();
    let mut quote = None;
    for (index, byte) in bytes.iter().copied().enumerate() {
        match byte {
            b'\'' | b'"' if quote == Some(byte) => quote = None,
            b'\'' | b'"' if quote.is_none() => quote = Some(byte),
            b'#' | b';'
                if quote.is_none()
                    && (index == 0 || bytes[index.saturating_sub(1)].is_ascii_whitespace()) =>
            {
                return &value[..index];
            }
            _ => {}
        }
    }
    value
}

fn parse_ini(path: &Path, text: &str) -> Result<IniDocument> {
    let mut document = IniDocument::default();
    let mut current: Option<usize> = None;
    let mut last_key: Option<String> = None;
    let mut seen_sections = BTreeSet::new();
    let mut skipping = false;
    let mut option_indent = None;

    for (line_index, raw) in text.lines().enumerate() {
        let line_number = line_index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        let indent = raw.len().saturating_sub(raw.trim_start().len());
        if trimmed.starts_with('[')
            && trimmed.ends_with(']')
            && option_indent.is_none_or(|current| indent <= current)
        {
            let name = trimmed[1..trimmed.len() - 1].trim();
            if name.is_empty() {
                return Err(config_error(
                    path,
                    format!("empty section at line {line_number}"),
                ));
            }
            if name != "DEFAULT" && !seen_sections.insert(name.to_string()) {
                document.duplicate_sections.push(name.to_string());
                current = None;
                last_key = None;
                skipping = true;
                option_indent = None;
                continue;
            }
            skipping = false;
            last_key = None;
            option_indent = None;
            if name == "DEFAULT" {
                current = None;
            } else {
                document.sections.push(IniSection {
                    name: name.to_string(),
                    values: BTreeMap::new(),
                });
                current = Some(document.sections.len() - 1);
            }
            continue;
        }
        if skipping {
            continue;
        }
        if raw.chars().next().is_some_and(char::is_whitespace)
            && option_indent.is_some_and(|current| indent > current)
        {
            if let Some(key) = last_key.as_deref() {
                let values = current
                    .map(|index| &mut document.sections[index].values)
                    .unwrap_or(&mut document.defaults);
                if let Some(value) = values.get_mut(key) {
                    value.push(' ');
                    value.push_str(strip_inline_comment(trimmed).trim());
                    continue;
                }
            }
        }
        let separator = match (trimmed.find('='), trimmed.find(':')) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (left, right) => left.or(right),
        }
        .ok_or_else(|| config_error(path, format!("invalid line {line_number}")))?;
        let key = trimmed[..separator].trim().to_ascii_lowercase();
        if key.is_empty() {
            return Err(config_error(
                path,
                format!("empty option at line {line_number}"),
            ));
        }
        let value = strip_inline_comment(&trimmed[separator + 1..])
            .trim_matches(|character: char| matches!(character, ' ' | '\t' | '\'' | '"'))
            .to_string();
        let values = current
            .map(|index| &mut document.sections[index].values)
            .unwrap_or(&mut document.defaults);
        if values.insert(key.clone(), value).is_some() {
            return Err(config_error(
                path,
                format!("option {key} is defined more than once at line {line_number}"),
            ));
        }
        last_key = Some(key);
        option_indent = Some(indent);
    }
    Ok(document)
}

fn ini_value<'a>(
    section: &'a IniSection,
    defaults: &'a BTreeMap<String, String>,
    key: &str,
) -> Option<&'a str> {
    section
        .values
        .get(key)
        .or_else(|| defaults.get(key))
        .map(String::as_str)
}

fn configured_database(
    path: &Path,
    section: &IniSection,
    defaults: &BTreeMap<String, String>,
) -> Result<ConfiguredDatabase> {
    let id = section.name.to_ascii_lowercase();
    let url = ini_value(section, defaults, "db_url")
        .or_else(|| (id == DEFAULT_DATABASE_ID).then_some(DEFAULT_DATABASE_URL))
        .ok_or_else(|| config_error(path, format!("[{}] has no db_url", section.name)))?
        .trim()
        .to_string();
    if !safe_web_url(&url) {
        return Err(config_error(
            path,
            format!("[{}] has an unsupported db_url", section.name),
        ));
    }
    Ok(ConfiguredDatabase {
        id,
        url,
        description: ini_value(section, defaults, "description")
            .unwrap_or("")
            .trim()
            .to_string(),
        filter: ini_value(section, defaults, "filter")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_ascii_lowercase),
    })
}

fn safe_web_url(value: &str) -> bool {
    if value.len() > MAX_PATH_TEXT
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        return false;
    }
    let Some(remainder) = value
        .strip_prefix("https://")
        .or_else(|| value.strip_prefix("http://"))
    else {
        return false;
    };
    let authority = remainder
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit_once('@')
        .map(|(_, host)| host)
        .unwrap_or(remainder.split(['/', '?', '#']).next().unwrap_or_default());
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed
            .split_once(']')
            .map(|(host, _)| host)
            .unwrap_or("")
    } else {
        authority.split(':').next().unwrap_or_default()
    };
    !host.is_empty()
}

fn drop_in_paths(menu_root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    let folder = menu_root.join("downloader");
    match std::fs::read_dir(&folder) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry.map_err(|error| {
                    DegaussError::io("reading Downloader drop-ins", &folder, error)
                })?;
                let name = entry.file_name();
                let name = name.to_string_lossy();
                if !name.starts_with('.')
                    && name.ends_with(".ini")
                    && entry
                        .file_type()
                        .map_err(|error| {
                            DegaussError::io("checking Downloader drop-in", entry.path(), error)
                        })?
                        .is_file()
                {
                    paths.push(entry.path());
                }
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(DegaussError::io(
                "reading Downloader drop-ins",
                &folder,
                error,
            ))
        }
    }
    paths.sort();

    let mut star = Vec::new();
    for entry in std::fs::read_dir(menu_root)
        .map_err(|error| DegaussError::io("reading MiSTer root", menu_root, error))?
    {
        let entry = entry
            .map_err(|error| DegaussError::io("reading MiSTer root entry", menu_root, error))?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("downloader_")
            && name.ends_with(".ini")
            && !name.starts_with('.')
            && entry
                .file_type()
                .map_err(|error| {
                    DegaussError::io("checking Downloader drop-in", entry.path(), error)
                })?
                .is_file()
        {
            star.push(entry.path());
        }
    }
    star.sort();
    paths.extend(star);
    Ok(paths)
}

fn read_downloader_config(menu_root: &Path) -> Result<DownloaderConfig> {
    let base_path = menu_root.join("downloader.ini");
    let base = parse_ini(&base_path, &read_limited_text(&base_path)?)?;
    if base
        .duplicate_sections
        .iter()
        .any(|section| section.eq_ignore_ascii_case("mister"))
    {
        return Err(config_error(
            &base_path,
            "the [MiSTer] section is defined more than once",
        ));
    }
    let global = base
        .sections
        .iter()
        .find(|section| section.name.eq_ignore_ascii_case("mister"));
    let global_filter_defined = global.is_some_and(|section| {
        section.values.contains_key("filter") || base.defaults.contains_key("filter")
    });
    let global_filter = global
        .and_then(|section| ini_value(section, &base.defaults, "filter"))
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();

    let mut databases = Vec::new();
    let mut seen = BTreeSet::new();
    let base_database_count = base
        .sections
        .iter()
        .filter(|section| !section.name.eq_ignore_ascii_case("mister"))
        .count();
    for section in base
        .sections
        .iter()
        .filter(|section| !section.name.eq_ignore_ascii_case("mister"))
    {
        let database = configured_database(&base_path, section, &base.defaults)?;
        if !seen.insert(database.id.clone()) {
            return Err(config_error(
                &base_path,
                format!("database [{}] is configured more than once", section.name),
            ));
        }
        databases.push(database);
    }

    for path in drop_in_paths(menu_root)? {
        let document = parse_ini(&path, &read_limited_text(&path)?)?;
        for section in &document.sections {
            if section.name.eq_ignore_ascii_case("mister") {
                return Err(config_error(
                    &path,
                    "drop-in files cannot contain a [MiSTer] section",
                ));
            }
            if section.name.eq_ignore_ascii_case(DEFAULT_DATABASE_ID) {
                return Err(config_error(
                    &path,
                    "the main distribution database is allowed only in downloader.ini",
                ));
            }
            let database = configured_database(&path, section, &document.defaults)?;
            if seen.insert(database.id.clone()) {
                databases.push(database);
            }
        }
    }

    if base_database_count == 0 && !seen.contains(DEFAULT_DATABASE_ID) {
        let url = base
            .defaults
            .get("db_url")
            .map(String::as_str)
            .unwrap_or(DEFAULT_DATABASE_URL)
            .trim()
            .to_string();
        if !safe_web_url(&url) {
            return Err(config_error(
                &base_path,
                "[DEFAULT] has an unsupported db_url",
            ));
        }
        databases.push(ConfiguredDatabase {
            id: DEFAULT_DATABASE_ID.to_string(),
            url,
            description: base
                .defaults
                .get("description")
                .cloned()
                .unwrap_or_default(),
            filter: None,
        });
    }
    if let Some(position) = databases
        .iter()
        .position(|database| database.id == DEFAULT_DATABASE_ID)
    {
        let default = databases.remove(position);
        databases.insert(0, default);
    }
    if databases.len() > MAX_DATABASES {
        return Err(DegaussError::unsupported(
            "Downloader configuration",
            format!("{} databases exceed the supported limit", databases.len()),
        ));
    }
    Ok(DownloaderConfig {
        global_filter,
        global_filter_defined,
        databases,
    })
}

#[derive(Debug)]
struct FilterRule {
    positive: Vec<FilterTag>,
    negative: Vec<FilterTag>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum FilterTag {
    Text(String),
    Index(i64),
}

fn normalize_filter_term(value: &str) -> String {
    value
        .chars()
        .filter(|character| !matches!(character, '-' | '_'))
        .flat_map(char::to_lowercase)
        .collect()
}

fn valid_filter_part(value: &str) -> bool {
    let value = value.strip_prefix('!').unwrap_or(value);
    !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
}

fn filter_rule(
    value: &str,
    dictionary: &BTreeMap<String, i64>,
) -> std::result::Result<Option<FilterRule>, String> {
    let value = value.trim().to_ascii_lowercase();
    if value.is_empty() || value == "all" {
        return Ok(None);
    }
    if value == "!all" {
        return Ok(Some(FilterRule {
            positive: vec![FilterTag::Text("__never__".into())],
            negative: Vec::new(),
        }));
    }
    let mut positive = Vec::new();
    let mut negative = Vec::new();
    let mut positive_all = false;
    for part in value.split_whitespace() {
        if !valid_filter_part(part) {
            return Err(format!("invalid filter term {part:?}"));
        }
        let excluded = part.starts_with('!');
        let term = normalize_filter_term(part.trim_start_matches('!'));
        if term == "none" {
            return Err("invalid filter term \"none\"".into());
        }
        if term == "all" {
            if excluded {
                return Err("invalid filter term \"!all\"".into());
            }
            positive_all = true;
            continue;
        }
        let tag = dictionary
            .get(&term)
            .copied()
            .map(FilterTag::Index)
            .unwrap_or_else(|| FilterTag::Text(term));
        if excluded {
            negative.push(tag);
        } else {
            positive.push(tag);
        }
    }
    let essential = dictionary
        .get("essential")
        .copied()
        .map(FilterTag::Index)
        .unwrap_or_else(|| FilterTag::Text("essential".into()));
    if !positive.is_empty() && !positive.contains(&essential) && !negative.contains(&essential) {
        positive.push(essential);
    }
    if positive_all {
        positive.clear();
    }
    Ok(Some(FilterRule { positive, negative }))
}

fn description_tags(description: &FileDescription) -> Vec<FilterTag> {
    description
        .tags
        .iter()
        .filter_map(|value| match value {
            serde_json::Value::String(text) => Some(FilterTag::Text(normalize_filter_term(text))),
            serde_json::Value::Number(number) => number.as_i64().map(FilterTag::Index),
            _ => None,
        })
        .collect()
}

fn selected_by_filter(description: &FileDescription, rule: Option<&FilterRule>) -> bool {
    let Some(rule) = rule else {
        return true;
    };
    let tags = description_tags(description);
    let included = rule.positive.is_empty() || rule.positive.iter().any(|tag| tags.contains(tag));
    included && !rule.negative.iter().any(|tag| tags.contains(tag))
}

fn effective_filter(
    config: &DownloaderConfig,
    database: &ConfiguredDatabase,
    manifest: &Manifest,
) -> String {
    let mut value = config.global_filter.clone();
    if let Some(default) = manifest.default_options.filter.as_deref() {
        if !config.global_filter_defined || default.to_ascii_lowercase().contains("[mister]") {
            value = default.to_ascii_lowercase();
        }
    }
    if let Some(database_filter) = database.filter.as_deref() {
        value = database_filter.to_ascii_lowercase();
    }
    value
        .replace("[mister]", &config.global_filter)
        .trim()
        .to_string()
}

fn safe_core_path(value: &str) -> bool {
    if value.is_empty()
        || value.len() > MAX_PATH_TEXT
        || value.starts_with('/')
        || value.contains('\\')
        || value.chars().any(char::is_control)
    {
        return false;
    }
    let path = Path::new(value);
    path.extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
        && path
            .components()
            .next()
            .and_then(|component| match component {
                Component::Normal(value) => value.to_str(),
                _ => None,
            })
            .is_some_and(|folder| folder.starts_with('_'))
}

fn validate_hash(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn undated_stem(stem: &str) -> &str {
    match stem.rsplit_once('_') {
        Some((before, after))
            if after.len() >= 8 && after.as_bytes()[..8].iter().all(u8::is_ascii_digit) =>
        {
            before
        }
        _ => stem,
    }
}

fn build_date(stem: &str) -> Option<String> {
    let (_, suffix) = stem.rsplit_once('_')?;
    let date = suffix.get(..8)?;
    date.bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..8]))
}

fn title_from_stem(stem: &str) -> String {
    undated_stem(stem)
        .trim_start_matches("RA_")
        .replace(['_', '-'], " ")
}

fn category_from_path(path: &str) -> String {
    let folder = path.split('/').next().unwrap_or_default();
    match folder.trim_start_matches('_') {
        "RA_Cores" => "RetroAchievements".into(),
        "LLAPI" => "LLAPI".into(),
        other => other.replace('_', " "),
    }
}

fn remote_core(
    database: &ConfiguredDatabase,
    path: &str,
    description: &FileDescription,
) -> std::result::Result<Option<RemoteCore>, String> {
    if !safe_core_path(path) {
        return Ok(None);
    }
    if !validate_hash(&description.hash) {
        return Err(format!("core {path:?} has no valid MD5 hash"));
    }
    let stem = Path::new(path)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| format!("core {path:?} has no usable name"))?;
    let identity = crate::systems::core_name(stem.trim_start_matches("RA_"));
    if identity.is_empty() {
        return Err(format!("core {path:?} has no usable identity"));
    }
    Ok(Some(RemoteCore {
        database: database.id.clone(),
        source_label: database.label(),
        path: path.replace('\\', "/"),
        identity,
        title: title_from_stem(stem),
        category: category_from_path(path),
        hash: description.hash.to_ascii_lowercase(),
        size: description.size,
        overwrite: description.overwrite,
        build: build_date(stem).unwrap_or_default(),
    }))
}

fn archive_can_hold_menu_core(archive: &ArchiveDescription) -> bool {
    if archive.summary_inline.is_some() || archive.internal_summary.is_some() {
        return true;
    }
    if archive.summary_file.is_none() {
        return false;
    }
    let target = archive.target_folder.trim_start_matches("./");
    archive.extract.eq_ignore_ascii_case("selective")
        || target.is_empty()
        || target.starts_with('_')
}

fn collect_remote_files(
    database: &ConfiguredDatabase,
    files: &BTreeMap<String, FileDescription>,
    rule: Option<&FilterRule>,
    cores: &mut Vec<RemoteCore>,
) -> std::result::Result<(), String> {
    for (path, description) in files {
        if !selected_by_filter(description, rule) {
            continue;
        }
        if let Some(core) = remote_core(database, path, description)? {
            cores.push(core);
            if cores.len() > MAX_REMOTE_CORES {
                return Err(format!("more than {MAX_REMOTE_CORES} core entries"));
            }
        }
    }
    Ok(())
}

fn decode_json_payload(
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> std::result::Result<Vec<u8>, FetchError> {
    if bytes.starts_with(b"PK") {
        return extract_single_json(bytes, cancelled);
    }
    if bytes.len() as u64 > MAX_DATABASE_JSON_BYTES {
        return Err(FetchError {
            user: "A Downloader database is too large for Degauss to read safely.".into(),
            diagnostic: format!("JSON payload exceeded {MAX_DATABASE_JSON_BYTES} bytes"),
            cancelled: false,
        });
    }
    Ok(bytes.to_vec())
}

fn fetch_database<F>(
    config: &DownloaderConfig,
    database: &ConfiguredDatabase,
    cancelled: &AtomicBool,
    events: &SyncSender<Event>,
    fetch: &mut F,
    failed_sources: &mut Vec<String>,
) -> std::result::Result<Vec<RemoteCore>, FetchError>
where
    F: FnMut(&str, u64, Duration, &AtomicBool) -> std::result::Result<Vec<u8>, FetchError>,
{
    let payload = fetch(
        &database.url,
        MAX_DATABASE_BYTES,
        Duration::from_secs(20),
        cancelled,
    )?;
    let json = decode_json_payload(&payload, cancelled)?;
    let manifest: Manifest = serde_json::from_slice(&json).map_err(|error| FetchError {
        user: "A configured Downloader database returned data Degauss could not read.".into(),
        diagnostic: format!("{}: manifest JSON: {error}", database.id),
        cancelled: false,
    })?;
    if manifest.v > 1 {
        return Err(FetchError {
            user: "A configured database requires a newer Degauss version.".into(),
            diagnostic: format!(
                "{}: unsupported database version {}",
                database.id, manifest.v
            ),
            cancelled: false,
        });
    }
    if !manifest.db_id.eq_ignore_ascii_case(&database.id) {
        return Err(FetchError {
            user: "A configured Downloader database does not match its configuration.".into(),
            diagnostic: format!(
                "section {} returned database id {}",
                database.id, manifest.db_id
            ),
            cancelled: false,
        });
    }
    let filter = effective_filter(config, database, &manifest);
    let rule = filter_rule(&filter, &manifest.tag_dictionary).map_err(|detail| FetchError {
        user: "A configured Downloader filter could not be applied.".into(),
        diagnostic: format!("{}: {detail}", database.id),
        cancelled: false,
    })?;
    let mut cores = Vec::new();
    collect_remote_files(database, &manifest.files, rule.as_ref(), &mut cores).map_err(
        |detail| FetchError {
            user: "A configured Downloader database returned invalid core data.".into(),
            diagnostic: format!("{}: {detail}", database.id),
            cancelled: false,
        },
    )?;
    let archives: Vec<_> = manifest
        .archives
        .iter()
        .filter(|(_, archive)| archive_can_hold_menu_core(archive))
        .collect();
    for (index, (name, archive)) in archives.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(FetchError {
                user: String::new(),
                diagnostic: format!("{}: archive summary cancelled", database.id),
                cancelled: true,
            });
        }
        send_progress(
            events,
            Phase::CheckingAvailability,
            index,
            archives.len(),
            false,
        );
        // Each archive summary is independent. Keep the other configured
        // sources when one server times out or returns unusable data.
        let loaded = (|| {
            let mut archive_cores = Vec::new();
            let summary = if let Some(summary_file) = archive.summary_file.as_ref() {
                if !safe_web_url(&summary_file.url)
                    || !validate_hash(&summary_file.hash)
                    || summary_file.size > MAX_DATABASE_BYTES
                {
                    return Err(FetchError {
                        user: "A configured Downloader archive has an invalid summary.".into(),
                        diagnostic: "invalid archive summary descriptor".into(),
                        cancelled: false,
                    });
                }
                let payload = fetch(
                    &summary_file.url,
                    MAX_DATABASE_BYTES,
                    Duration::from_secs(20),
                    cancelled,
                )?;
                if payload.len() as u64 != summary_file.size
                    || md5_hex(&payload) != summary_file.hash.to_ascii_lowercase()
                {
                    return Err(FetchError {
                        user: "A Downloader archive summary failed its integrity check.".into(),
                        diagnostic: "archive summary size or MD5 mismatch".into(),
                        cancelled: false,
                    });
                }
                let json = decode_json_payload(&payload, cancelled)?;
                Some(
                    serde_json::from_slice::<ArchiveSummary>(&json).map_err(|error| {
                        FetchError {
                            user:
                                "A Downloader archive summary returned data Degauss could not read."
                                    .into(),
                            diagnostic: format!("summary JSON: {error}"),
                            cancelled: false,
                        }
                    })?,
                )
            } else {
                None
            };
            if let Some(summary) = summary
                .as_ref()
                .or(archive.summary_inline.as_ref())
                .or(archive.internal_summary.as_ref())
            {
                collect_remote_files(database, &summary.files, rule.as_ref(), &mut archive_cores)
                    .map_err(|detail| FetchError {
                    user: "A configured Downloader archive returned invalid core data.".into(),
                    diagnostic: detail,
                    cancelled: false,
                })?;
            }
            Ok::<_, FetchError>(archive_cores)
        })();
        match loaded {
            Ok(archive_cores) => cores.extend(archive_cores),
            Err(error) if error.cancelled => return Err(error),
            Err(error) => {
                crate::note(&format!(
                    "core updates  {} archive {} failed: {}",
                    database.id, name, error.diagnostic
                ));
                failed_sources.push(format!("{} / {}: {}", database.label(), name, error.user));
            }
        }
    }
    let _ = manifest.timestamp;
    Ok(cores)
}

fn fetch_url(
    url: &str,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> std::result::Result<Vec<u8>, FetchError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(FetchError {
            user: String::new(),
            diagnostic: "cancelled before request".into(),
            cancelled: true,
        });
    }
    if !safe_web_url(url) {
        return Err(FetchError {
            user: "A configured Downloader database uses an unsupported address.".into(),
            diagnostic: format!("unsupported URL: {url:?}"),
            cancelled: false,
        });
    }
    let response = ResponseFile::create().map_err(|diagnostic| FetchError {
        user: "Core Updates could not be opened. Try again.".into(),
        diagnostic,
        cancelled: false,
    })?;
    let config = curl_config(url, response.path(), limit, timeout);
    let mut child = Command::new("curl")
        .args(["-q", "--config", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| FetchError {
            user: if error.kind() == std::io::ErrorKind::NotFound {
                "MiSTer cannot check core updates because curl is missing.".into()
            } else {
                "Core Updates could not be opened. Try again.".into()
            },
            diagnostic: format!("curl could not start: {error}"),
            cancelled: false,
        })?;
    let write = child
        .stdin
        .take()
        .ok_or_else(|| FetchError {
            user: "Core Updates could not be opened. Try again.".into(),
            diagnostic: "curl stdin was unavailable".into(),
            cancelled: false,
        })
        .and_then(|mut stdin| {
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| FetchError {
                    user: "Core Updates could not be opened. Try again.".into(),
                    diagnostic: format!("curl config write failed: {error}"),
                    cancelled: false,
                })
        });
    if let Err(error) = write {
        stop_child(&mut child);
        return Err(error);
    }
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            stop_child(&mut child);
            return Err(FetchError {
                user: String::new(),
                diagnostic: "request cancelled".into(),
                cancelled: true,
            });
        }
        if started.elapsed() > timeout + Duration::from_secs(2) {
            stop_child(&mut child);
            return Err(FetchError {
                user: "A Downloader database did not respond in time.".into(),
                diagnostic: format!(
                    "request watchdog expired after {}s",
                    started.elapsed().as_secs()
                ),
                cancelled: false,
            });
        }
        if response.len().is_some_and(|size| size > limit) {
            stop_child(&mut child);
            return Err(FetchError {
                user: "A Downloader database is too large for Degauss to read safely.".into(),
                diagnostic: format!("response exceeded {limit} bytes"),
                cancelled: false,
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(CURL_POLL),
            Err(error) => {
                stop_child(&mut child);
                return Err(FetchError {
                    user: "Core Updates could not be opened. Try again.".into(),
                    diagnostic: format!("curl process monitoring failed: {error}"),
                    cancelled: false,
                });
            }
        }
    };
    if !status.success() {
        let code = status.code();
        let user = match code {
            Some(5..=7) => {
                "Could not reach a configured Downloader database. Check the network connection."
            }
            Some(28) => "A Downloader database did not respond in time.",
            Some(35 | 51 | 60) => {
                "A secure database connection failed. Check MiSTer's date and network."
            }
            Some(63) => "A Downloader database is too large for Degauss to read safely.",
            Some(22) => "A configured Downloader database is unavailable.",
            _ => "Core Updates could not be opened. Try again.",
        };
        return Err(FetchError {
            user: user.into(),
            diagnostic: format!("curl exit {code:?}"),
            cancelled: false,
        });
    }
    response.read(limit).map_err(|diagnostic| FetchError {
        user: "A Downloader database returned data Degauss could not read.".into(),
        diagnostic,
        cancelled: false,
    })
}

fn extract_single_json(
    bytes: &[u8],
    cancelled: &AtomicBool,
) -> std::result::Result<Vec<u8>, FetchError> {
    let archive = ResponseFile::create_with_extension("zip").map_err(|diagnostic| FetchError {
        user: "A Downloader database returned data Degauss could not read.".into(),
        diagnostic,
        cancelled: false,
    })?;
    archive.write(bytes).map_err(|diagnostic| FetchError {
        user: "A Downloader database returned data Degauss could not read.".into(),
        diagnostic,
        cancelled: false,
    })?;
    let entries = crate::zip::entries(archive.path()).map_err(|error| FetchError {
        user: "A Downloader database returned data Degauss could not read.".into(),
        diagnostic: error.to_string(),
        cancelled: false,
    })?;
    let [entry] = entries.as_slice() else {
        return Err(FetchError {
            user: "A Downloader database returned an invalid archive.".into(),
            diagnostic: format!("expected one JSON member, found {}", entries.len()),
            cancelled: false,
        });
    };
    if !entry.name.to_ascii_lowercase().ends_with(".json") || entry.size > MAX_DATABASE_JSON_BYTES {
        return Err(FetchError {
            user: "A Downloader database returned an invalid archive.".into(),
            diagnostic: format!("invalid archive member {:?}", entry.name),
            cancelled: false,
        });
    }
    let extracted = ResponseFile::create().map_err(|diagnostic| FetchError {
        user: "A Downloader database returned data Degauss could not read.".into(),
        diagnostic,
        cancelled: false,
    })?;
    let output = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(extracted.path())
        .map_err(|error| FetchError {
            user: "A Downloader database returned data Degauss could not read.".into(),
            diagnostic: format!("extraction output failed: {error}"),
            cancelled: false,
        })?;
    let mut child = Command::new("unzip")
        .args(["-p"])
        .arg(archive.path())
        .arg(literal_unzip_member_pattern(&entry.name))
        .stdin(Stdio::null())
        .stdout(Stdio::from(output))
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| FetchError {
            user: if error.kind() == std::io::ErrorKind::NotFound {
                "MiSTer cannot read Downloader databases because unzip is missing.".into()
            } else {
                "A Downloader database returned data Degauss could not read.".into()
            },
            diagnostic: format!("unzip could not start: {error}"),
            cancelled: false,
        })?;
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            stop_child(&mut child);
            return Err(FetchError {
                user: String::new(),
                diagnostic: "database extraction cancelled".into(),
                cancelled: true,
            });
        }
        if started.elapsed() > Duration::from_secs(15) {
            stop_child(&mut child);
            return Err(FetchError {
                user: "A Downloader database took too long to unpack.".into(),
                diagnostic: "unzip timed out".into(),
                cancelled: false,
            });
        }
        if extracted
            .len()
            .is_some_and(|size| size > MAX_DATABASE_JSON_BYTES)
        {
            stop_child(&mut child);
            return Err(FetchError {
                user: "A Downloader database is too large for Degauss to read safely.".into(),
                diagnostic: "extracted JSON exceeded limit".into(),
                cancelled: false,
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(CURL_POLL),
            Err(error) => {
                stop_child(&mut child);
                return Err(FetchError {
                    user: "A Downloader database returned data Degauss could not read.".into(),
                    diagnostic: format!("unzip monitoring failed: {error}"),
                    cancelled: false,
                });
            }
        }
    };
    if !status.success() {
        return Err(FetchError {
            user: "A Downloader database returned data Degauss could not read.".into(),
            diagnostic: format!("unzip exit {:?}", status.code()),
            cancelled: false,
        });
    }
    let body = extracted
        .read(MAX_DATABASE_JSON_BYTES)
        .map_err(|diagnostic| FetchError {
            user: "A Downloader database returned data Degauss could not read.".into(),
            diagnostic,
            cancelled: false,
        })?;
    if body.len() as u64 != entry.size {
        return Err(FetchError {
            user: "A Downloader database returned data Degauss could not read.".into(),
            diagnostic: format!("extracted {} bytes, expected {}", body.len(), entry.size),
            cancelled: false,
        });
    }
    Ok(body)
}

fn literal_unzip_member_pattern(name: &str) -> String {
    let mut pattern = String::with_capacity(name.len());
    for (position, character) in name.chars().enumerate() {
        match (position, character) {
            (0, '-') => pattern.push_str("[-]"),
            (_, '*') => pattern.push_str("[*]"),
            (_, '?') => pattern.push_str("[?]"),
            (_, '[') => pattern.push_str("[[]"),
            (_, _) => pattern.push(character),
        }
    }
    pattern
}

fn curl_config(url: &str, output: &Path, limit: u64, timeout: Duration) -> String {
    let ca = CA_BUNDLES
        .iter()
        .map(Path::new)
        .find(|path| OpenOptions::new().read(true).open(path).is_ok())
        .map(|path| format!("cacert = \"{}\"\n", curl_quote(&path.to_string_lossy())))
        .unwrap_or_default();
    format!(
        "silent\nshow-error\nfail\nlocation\nmax-redirs = \"5\"\ncompressed\nproto = \"=http,=https\"\nproto-redir = \"=http,=https\"\n{ca}connect-timeout = \"3\"\nmax-time = \"{}\"\nmax-filesize = \"{limit}\"\nuser-agent = \"Degauss/{}\"\noutput = \"{}\"\nurl = \"{}\"\n",
        timeout.as_secs(),
        env!("CARGO_PKG_VERSION"),
        curl_quote(&output.to_string_lossy()),
        curl_quote(url),
    )
}

fn curl_quote(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(character),
        }
    }
    quoted
}

fn stop_child(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn md5_hex(bytes: &[u8]) -> String {
    lower_hex(&Md5::digest(bytes))
}

fn lower_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02x}");
    }
    output
}

struct ResponseFile {
    path: PathBuf,
}

impl ResponseFile {
    fn create() -> std::result::Result<Self, String> {
        Self::create_with_extension("part")
    }

    fn create_with_extension(extension: &str) -> std::result::Result<Self, String> {
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                ".degauss-core-updates-{}-{sequence}.{extension}",
                std::process::id(),
            ));
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            match options.open(&path) {
                Ok(file) => {
                    drop(file);
                    return Ok(Self { path });
                }
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(format!("temporary response file failed: {error}")),
            }
        }
        Err("temporary response file names were exhausted".into())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn len(&self) -> Option<u64> {
        std::fs::metadata(&self.path)
            .ok()
            .map(|metadata| metadata.len())
    }

    fn read(&self, limit: u64) -> std::result::Result<Vec<u8>, String> {
        let mut bytes = Vec::new();
        OpenOptions::new()
            .read(true)
            .open(&self.path)
            .map_err(|error| format!("response file could not be opened: {error}"))?
            .take(limit.saturating_add(1))
            .read_to_end(&mut bytes)
            .map_err(|error| format!("response file could not be read: {error}"))?;
        if bytes.len() as u64 > limit {
            return Err(format!("response exceeded {limit} bytes"));
        }
        Ok(bytes)
    }

    fn write(&self, bytes: &[u8]) -> std::result::Result<(), String> {
        std::fs::write(&self.path, bytes)
            .map_err(|error| format!("response file could not be written: {error}"))
    }
}

impl Drop for ResponseFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

fn catalogue_identity(entry: &CoreEntry) -> String {
    match entry.variant {
        CoreVariant::RetroAchievements => crate::core_variants::ra_launcher_identity(&entry.path)
            .unwrap_or_else(|_| crate::systems::core_name(&entry.name)),
        _ => entry
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(crate::systems::core_name)
            .unwrap_or_else(|| crate::systems::core_name(&entry.name)),
    }
}

fn scan_local_cores(request: &Request) -> Result<Vec<LocalCore>> {
    let root_metadata = std::fs::metadata(&request.menu_root).map_err(|error| {
        DegaussError::io(
            "checking the configured MiSTer menu",
            &request.menu_root,
            error,
        )
    })?;
    if !root_metadata.is_dir() {
        return Err(DegaussError::unsupported(
            "installed core inventory",
            format!("{} is not a folder", request.menu_root.display()),
        ));
    }
    let mut by_identity = BTreeMap::<String, Vec<&CoreEntry>>::new();
    for entry in &request.cores.entries {
        by_identity
            .entry(catalogue_identity(entry))
            .or_default()
            .push(entry);
    }
    let mut cores = Vec::new();
    for entry in &request.cores.entries {
        if matches!(entry.variant, CoreVariant::RetroAchievements) {
            continue;
        }
        let metadata = match std::fs::metadata(&entry.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => {
                return Err(DegaussError::io(
                    "checking an installed core",
                    &entry.path,
                    error,
                ));
            }
        };
        if !metadata.is_file()
            || !entry
                .path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
        {
            continue;
        }
        let Some(stem) = entry.path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        cores.push(LocalCore {
            relative: local_relative_path(&request.menu_root, &entry.path)?,
            identity: catalogue_identity(entry),
            title: entry.name.clone(),
            category: entry.category.clone(),
            build: build_date(stem).unwrap_or_default(),
            file_path: entry.path.clone(),
            launch_path: Some(entry.path.clone()),
            logo_id: entry.logo_id.clone(),
        });
    }
    scan_support_core_folder(
        &request.menu_root,
        &request.menu_root.join("_RA_Cores/Cores"),
        "RA_Cores",
        &by_identity,
        &mut cores,
    )?;
    scan_support_core_folder(
        &request.menu_root,
        &request.menu_root.join("_Arcade/cores"),
        "Arcade",
        &by_identity,
        &mut cores,
    )?;
    scan_unlisted_local_cores(&request.menu_root, &by_identity, &mut cores)?;
    if cores.len() > MAX_LOCAL_CORES {
        return Err(DegaussError::unsupported(
            "installed core inventory",
            format!("{} core files exceed the supported limit", cores.len()),
        ));
    }
    cores.sort_by(|left, right| left.relative.cmp(&right.relative));
    Ok(cores)
}

/// The Cores browser deliberately keeps one preferred Standard launcher for
/// each core identity. Core Updates instead reports every installed file, so
/// retain alternate Standard builds that live beside that preferred launcher.
fn scan_unlisted_local_cores(
    menu_root: &Path,
    by_identity: &BTreeMap<String, Vec<&CoreEntry>>,
    cores: &mut Vec<LocalCore>,
) -> Result<()> {
    let mut seen = cores
        .iter()
        .map(|core| core.relative.to_ascii_lowercase())
        .collect::<BTreeSet<_>>();
    let entries = std::fs::read_dir(menu_root)
        .map_err(|error| DegaussError::io("reading installed cores", menu_root, error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| DegaussError::io("reading an installed core", menu_root, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with('_') {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::metadata(&path)
            .map_err(|error| DegaussError::io("checking an installed core", &path, error))?;
        if !metadata.is_dir() {
            continue;
        }
        let label = name.trim_start_matches('_');
        if label.eq_ignore_ascii_case("Arcade")
            || label.eq_ignore_ascii_case("RA_Cores")
            || crate::systems::is_favorites(label.trim_start_matches('@'))
        {
            continue;
        }
        scan_unlisted_local_core_folder(menu_root, &path, label, 0, by_identity, &mut seen, cores)?;
    }
    Ok(())
}

fn scan_unlisted_local_core_folder(
    menu_root: &Path,
    folder: &Path,
    menu_folder: &str,
    depth: usize,
    by_identity: &BTreeMap<String, Vec<&CoreEntry>>,
    seen: &mut BTreeSet<String>,
    cores: &mut Vec<LocalCore>,
) -> Result<()> {
    if depth > 1 {
        return Ok(());
    }
    let entries = std::fs::read_dir(folder)
        .map_err(|error| DegaussError::io("reading installed cores", folder, error))?;
    for entry in entries {
        let entry =
            entry.map_err(|error| DegaussError::io("reading an installed core", folder, error))?;
        let path = entry.path();
        let metadata = std::fs::metadata(&path)
            .map_err(|error| DegaussError::io("checking an installed core", &path, error))?;
        if metadata.is_dir() {
            scan_unlisted_local_core_folder(
                menu_root,
                &path,
                menu_folder,
                depth + 1,
                by_identity,
                seen,
                cores,
            )?;
            continue;
        }
        if !metadata.is_file()
            || !path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let identity = crate::systems::core_name(stem);
        if identity.is_empty() {
            continue;
        }
        let relative = local_relative_path(menu_root, &path)?;
        if !seen.insert(relative.to_ascii_lowercase()) {
            continue;
        }
        let logo_id = by_identity
            .get(&identity)
            .and_then(|entries| entries.iter().find_map(|entry| entry.logo_id.clone()));
        cores.push(LocalCore {
            relative,
            identity,
            title: title_from_stem(stem),
            category: menu_folder.replace('_', " "),
            build: build_date(stem).unwrap_or_default(),
            file_path: path.clone(),
            launch_path: Some(path),
            logo_id,
        });
    }
    Ok(())
}

fn local_relative_path(menu_root: &Path, path: &Path) -> Result<String> {
    path.strip_prefix(menu_root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            DegaussError::unsupported(
                "installed core inventory",
                format!("{} is outside the MiSTer root", path.display()),
            )
        })
}

fn scan_support_core_folder(
    menu_root: &Path,
    folder: &Path,
    menu_folder: &str,
    by_identity: &BTreeMap<String, Vec<&CoreEntry>>,
    cores: &mut Vec<LocalCore>,
) -> Result<()> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(DegaussError::io("reading installed cores", folder, error)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| DegaussError::io("reading an installed core", folder, error))?;
        let path = entry.path();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
            || !path.is_file()
        {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        let identity = crate::systems::core_name(stem.trim_start_matches("RA_"));
        if identity.is_empty() {
            continue;
        }
        let candidates = by_identity.get(&identity);
        let matched = candidates.and_then(|entries| {
            entries.iter().copied().find(|entry| {
                matches!(entry.variant, CoreVariant::RetroAchievements)
                    && menu_folder.eq_ignore_ascii_case("RA_Cores")
            })
        });
        let ra_launcher = menu_folder
            .eq_ignore_ascii_case("RA_Cores")
            .then(|| {
                candidates.and_then(|entries| {
                    entries
                        .iter()
                        .find(|entry| matches!(entry.variant, CoreVariant::RetroAchievements))
                        .map(|entry| entry.path.clone())
                })
            })
            .flatten();
        let launch_path = if menu_folder.eq_ignore_ascii_case("Arcade") {
            None
        } else {
            ra_launcher.or_else(|| matched.map(|entry| entry.path.clone()))
        };
        cores.push(LocalCore {
            relative: local_relative_path(menu_root, &path)?,
            identity,
            title: matched
                .map(|entry| entry.name.clone())
                .unwrap_or_else(|| title_from_stem(stem)),
            category: matched
                .map(|entry| entry.category.clone())
                .unwrap_or_else(|| match menu_folder {
                    "RA_Cores" => "RetroAchievements".into(),
                    other => other.replace('_', " "),
                }),
            build: build_date(stem).unwrap_or_default(),
            file_path: path,
            launch_path,
            logo_id: matched.and_then(|entry| entry.logo_id.clone()),
        });
    }
    Ok(())
}

fn path_match_rank(remote_path: &str, local_path: &str) -> u8 {
    let remote_folder = remote_path.split('/').next().unwrap_or_default();
    let local_folder = local_path.split('/').next().unwrap_or_default();
    if local_path == remote_path {
        0
    } else if local_folder == remote_folder {
        1
    } else {
        2
    }
}

fn assign_local_cores(remotes: &[RemoteCore], locals: &[LocalCore]) -> Vec<Option<usize>> {
    let mut assignments = vec![None; remotes.len()];
    let mut matched = vec![false; locals.len()];
    let local_paths = locals
        .iter()
        .map(|local| local.relative.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut locals_by_identity = BTreeMap::<&str, Vec<usize>>::new();
    for (index, local) in locals.iter().enumerate() {
        locals_by_identity
            .entry(local.identity.as_str())
            .or_default()
            .push(index);
    }
    let remote_paths = remotes
        .iter()
        .map(|remote| remote.path.to_ascii_lowercase())
        .collect::<Vec<_>>();
    for rank in 0..=2 {
        for (remote_index, remote) in remotes.iter().enumerate() {
            if assignments[remote_index].is_some() {
                continue;
            }
            let candidate =
                locals_by_identity
                    .get(remote.identity.as_str())
                    .and_then(|candidates| {
                        candidates.iter().copied().find(|&local_index| {
                            !matched[local_index]
                                && path_match_rank(
                                    &remote_paths[remote_index],
                                    &local_paths[local_index],
                                ) == rank
                        })
                    });
            if let Some(local_index) = candidate {
                assignments[remote_index] = Some(local_index);
                matched[local_index] = true;
            }
        }
    }
    assignments
}

fn metadata_stamp(path: &Path) -> std::io::Result<(u64, u64)> {
    let metadata = std::fs::metadata(path)?;
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    Ok((metadata.len(), modified))
}

fn hash_local_file(
    path: &Path,
    cache: &mut Cache,
    cancelled: &AtomicBool,
) -> std::result::Result<Option<String>, DegaussError> {
    let (size, modified) = metadata_stamp(path)
        .map_err(|error| DegaussError::io("checking installed core", path, error))?;
    let key = path.to_string_lossy();
    if let Some(record) = cache
        .local_hashes
        .iter()
        .find(|record| record.path == key && record.size == size && record.modified == modified)
    {
        return Ok(Some(record.hash.clone()));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|error| DegaussError::io("opening installed core", path, error))?;
    let mut digest = Md5::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| DegaussError::io("reading installed core", path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    let hash = lower_hex(&digest.finalize());
    cache.local_hashes.retain(|record| record.path != key);
    cache.local_hashes.push(LocalHash {
        path: key.into_owned(),
        size,
        modified,
        hash: hash.clone(),
    });
    if cache.local_hashes.len() > MAX_LOCAL_CORES {
        cache
            .local_hashes
            .sort_by(|left, right| left.path.cmp(&right.path));
        cache.local_hashes.truncate(MAX_LOCAL_CORES);
    }
    Ok(Some(hash))
}

fn compare_core(
    remote: &RemoteCore,
    local: &LocalCore,
    cache: &mut Cache,
    cancelled: &AtomicBool,
) -> Option<LocalState> {
    if !remote.build.is_empty() && !local.build.is_empty() {
        return Some(if remote.build.as_str() > local.build.as_str() {
            LocalState::UpdateAvailable
        } else if remote.build.as_str() < local.build.as_str() {
            LocalState::NewerThanConfigured
        } else {
            LocalState::Current
        });
    }
    match hash_local_file(&local.file_path, cache, cancelled) {
        Ok(Some(hash)) if hash == remote.hash => Some(LocalState::Current),
        Ok(Some(_)) if remote.overwrite => Some(LocalState::UpdateAvailable),
        Ok(Some(_)) => Some(LocalState::KeptLocal),
        Ok(None) => None,
        Err(error) => {
            crate::note(&format!("core updates  version check failed: {error}"));
            Some(LocalState::VersionUnknown)
        }
    }
}

fn match_cache(
    cache: &mut Cache,
    request: &Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
) -> Result<Option<Snapshot>> {
    let locals = scan_local_cores(request)?;
    let total = cache.cores.len().saturating_add(locals.len());
    let assignments = assign_local_cores(&cache.cores, &locals);
    let mut matched = vec![false; locals.len()];
    let mut items = Vec::with_capacity(total);
    for (index, remote) in cache.cores.clone().into_iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let item = if let Some(local_index) = assignments[index] {
            let local = &locals[local_index];
            let Some(state) = compare_core(&remote, local, cache, cancelled) else {
                return Ok(None);
            };
            matched[local_index] = true;
            Item {
                key: format!("{}:{}", remote.database, remote.path.to_ascii_lowercase()),
                title: local.title.clone(),
                base: local.category.clone(),
                source: remote.database.clone(),
                source_label: remote.source_label.clone(),
                local_build: local.build.clone(),
                available_build: remote.build.clone(),
                remote_path: remote.path.clone(),
                state,
                installed: true,
                core_identity: Some(local.identity.clone()),
                game_count: None,
                game: None,
                launch_path: local.launch_path.clone(),
                cover: None,
                logo_id: local.logo_id.clone(),
            }
        } else {
            Item {
                key: format!("{}:{}", remote.database, remote.path.to_ascii_lowercase()),
                title: remote.title.clone(),
                base: remote.category.clone(),
                source: remote.database.clone(),
                source_label: remote.source_label.clone(),
                local_build: String::new(),
                available_build: remote.build.clone(),
                remote_path: remote.path.clone(),
                state: LocalState::AvailableThroughDownloader,
                installed: false,
                core_identity: Some(remote.identity.clone()),
                game_count: None,
                game: None,
                launch_path: None,
                cover: None,
                logo_id: None,
            }
        };
        items.push(item);
        if index % 16 == 0 || index + 1 == cache.cores.len() {
            send_progress(events, Phase::Matching, index + 1, total, false);
        }
    }
    for (index, local) in locals.into_iter().enumerate() {
        if matched[index] {
            continue;
        }
        items.push(Item {
            key: format!("local:{}", local.relative.to_ascii_lowercase()),
            title: local.title,
            base: local.category,
            source: "local".into(),
            source_label: "Local file".into(),
            local_build: local.build,
            available_build: String::new(),
            remote_path: String::new(),
            state: LocalState::LocalOnly,
            installed: true,
            core_identity: Some(local.identity),
            game_count: None,
            game: None,
            launch_path: local.launch_path,
            cover: None,
            logo_id: local.logo_id,
        });
    }
    items.sort_by(|left, right| {
        left.state
            .order()
            .cmp(&right.state.order())
            .then_with(|| {
                left.base
                    .to_ascii_lowercase()
                    .cmp(&right.base.to_ascii_lowercase())
            })
            .then_with(|| {
                left.title
                    .to_ascii_lowercase()
                    .cmp(&right.title.to_ascii_lowercase())
            })
            .then_with(|| left.source.cmp(&right.source))
    });
    Ok(Some(Snapshot {
        updated: cache.checked.to_string(),
        items,
    }))
}

pub fn cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("core-updates.bin")
}

fn load_cache(cache_dir: &Path) -> Result<Option<Cache>> {
    let path = cache_path(cache_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(DegaussError::io("reading Core Updates cache", &path, error)),
    };
    let cache: Cache = postcard::from_bytes(&bytes)
        .map_err(|error| DegaussError::malformed("Core Updates cache", &path, error.to_string()))?;
    validate_cache(&cache)
        .map_err(|detail| DegaussError::malformed("Core Updates cache", &path, detail))?;
    Ok(Some(cache))
}

fn validate_cache(cache: &Cache) -> std::result::Result<(), String> {
    if cache.format != CACHE_FORMAT {
        return Err(format!("unsupported format {}", cache.format));
    }
    if cache.global_filter.len() > MAX_CONFIG_BYTES as usize
        || cache.configuration.len() > MAX_DATABASES
        || cache.cores.len() > MAX_REMOTE_CORES
    {
        return Err("saved data exceeds supported limits".into());
    }
    let mut databases = BTreeSet::new();
    for database in &cache.configuration {
        if database.id.is_empty()
            || database.id.len() > MAX_SHORT_TEXT
            || !safe_web_url(&database.url)
            || !databases.insert(database.id.as_str())
        {
            return Err("saved database configuration is invalid".into());
        }
    }
    let configured: BTreeSet<&str> = cache
        .configuration
        .iter()
        .map(|database| database.id.as_str())
        .collect();
    let mut paths = BTreeSet::new();
    for core in &cache.cores {
        if !configured.contains(core.database.as_str())
            || !safe_core_path(&core.path)
            || !validate_hash(&core.hash)
            || core.identity.is_empty()
            || core.identity.len() > MAX_SHORT_TEXT
            || !paths.insert((core.database.as_str(), core.path.to_ascii_lowercase()))
        {
            return Err("saved core entry is invalid".into());
        }
    }
    if cache.local_hashes.len() > MAX_LOCAL_CORES
        || cache
            .local_hashes
            .iter()
            .any(|record| record.path.len() > MAX_PATH_TEXT || !validate_hash(&record.hash))
    {
        return Err("saved local hash entry is invalid".into());
    }
    Ok(())
}

fn save_cache(cache_dir: &Path, cache: &Cache, cancelled: &AtomicBool) -> Result<()> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(());
    }
    validate_cache(cache)
        .map_err(|detail| DegaussError::unsupported("Core Updates cache", detail))?;
    let bytes = postcard::to_stdvec(cache)
        .map_err(|error| DegaussError::unsupported("Core Updates cache", error.to_string()))?;
    crate::cache::write(&cache_path(cache_dir), &bytes)
}

fn configuration_matches(cache: &Cache, config: &DownloaderConfig) -> bool {
    cache.global_filter == config.global_filter
        && cache.global_filter_defined == config.global_filter_defined
        && cache.configuration == config.databases
}

fn send_progress(
    events: &SyncSender<Event>,
    phase: Phase,
    completed: usize,
    total: usize,
    cancelling: bool,
) {
    let _ = events.try_send(Event::Progress(Progress {
        phase,
        completed,
        total,
        cancelling,
    }));
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn finish_with_saved_or_error(
    saved: &Option<Snapshot>,
    message: String,
    events: &SyncSender<Event>,
) {
    if let Some(snapshot) = saved {
        let _ = events.send(Event::Ready {
            snapshot: snapshot.clone(),
            notice: Some(format!(
                "Core Updates could not be refreshed. Showing saved results.\n\n{message}"
            )),
        });
    } else {
        let _ = events.send(Event::Failed { message });
    }
}

fn unavailable_sources_notice(failed_sources: &[String], partial: bool) -> String {
    let ending = if partial {
        "Other results are shown. Refresh to retry."
    } else {
        "Refresh to retry."
    };
    format!(
        "Core Updates skipped these sources:\n\n{}\n\n{ending}",
        failed_sources.join("\n"),
    )
}

fn run_with_fetch<F>(
    request: Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
    mut fetch: F,
) where
    F: FnMut(&str, u64, Duration, &AtomicBool) -> std::result::Result<Vec<u8>, FetchError>,
{
    send_progress(events, Phase::Checking, 0, 0, false);
    let config = match read_downloader_config(&request.menu_root) {
        Ok(config) => config,
        Err(error) => {
            crate::note(&format!("core updates  configuration failed: {error}"));
            let _ = events.send(Event::Failed {
                message:
                    "Downloader settings could not be read. Check downloader.ini and try again."
                        .into(),
            });
            return;
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled);
        return;
    }
    let cached = match load_cache(&request.cache_dir) {
        Ok(cache) => cache,
        Err(error) => {
            crate::note(&format!("core updates  saved data rejected: {error}"));
            None
        }
    };
    let mut saved_snapshot = None;
    if let Some(mut cache) = cached.filter(|cache| configuration_matches(cache, &config)) {
        match match_cache(&mut cache, &request, events, cancelled) {
            Ok(Some(snapshot)) if !request.force_refresh => {
                if let Err(error) = save_cache(&request.cache_dir, &cache, cancelled) {
                    crate::note(&format!(
                        "core updates  local hash cache save failed: {error}"
                    ));
                }
                let _ = events.send(Event::Ready {
                    snapshot,
                    notice: None,
                });
                return;
            }
            Ok(Some(snapshot)) => {
                saved_snapshot = Some(snapshot.clone());
                let _ = events.send(Event::Snapshot(snapshot));
            }
            Ok(None) => {
                let _ = events.send(Event::Cancelled);
                return;
            }
            Err(error) => {
                crate::note(&format!("core updates  local comparison failed: {error}"));
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled);
        return;
    }

    let mut remote = Vec::new();
    let mut seen_paths = BTreeSet::new();
    let mut failed_sources = Vec::new();
    let mut databases_read = 0;
    for (index, database) in config.databases.iter().enumerate() {
        send_progress(
            events,
            Phase::Downloading,
            index,
            config.databases.len(),
            false,
        );
        let database_cores = match fetch_database(
            &config,
            database,
            cancelled,
            events,
            &mut fetch,
            &mut failed_sources,
        ) {
            Ok(cores) => {
                databases_read += 1;
                cores
            }
            Err(error) if error.cancelled => {
                let _ = events.send(Event::Cancelled);
                return;
            }
            Err(error) => {
                crate::note(&format!(
                    "core updates  {} failed: {}",
                    database.id, error.diagnostic
                ));
                failed_sources.push(format!("{}: {}", database.label(), error.user));
                continue;
            }
        };
        for core in database_cores {
            let path = core.path.to_ascii_lowercase();
            // Downloader gives the first configured database ownership of an
            // exact target path. Different dated or alternative paths remain
            // distinct, just as they do for Downloader itself.
            if seen_paths.insert(path) {
                remote.push(core);
            }
        }
    }
    send_progress(
        events,
        Phase::Downloading,
        config.databases.len(),
        config.databases.len(),
        false,
    );
    if databases_read == 0 && !failed_sources.is_empty() {
        finish_with_saved_or_error(
            &saved_snapshot,
            unavailable_sources_notice(&failed_sources, false),
            events,
        );
        return;
    }
    let old_hashes = load_cache(&request.cache_dir)
        .ok()
        .flatten()
        .map(|cache| cache.local_hashes)
        .unwrap_or_default();
    let mut cache = Cache {
        format: CACHE_FORMAT,
        checked: now(),
        global_filter: config.global_filter,
        global_filter_defined: config.global_filter_defined,
        configuration: config.databases,
        cores: remote,
        local_hashes: old_hashes,
    };
    let snapshot = match match_cache(&mut cache, &request, events, cancelled) {
        Ok(Some(snapshot)) => snapshot,
        Ok(None) => {
            let _ = events.send(Event::Cancelled);
            return;
        }
        Err(error) => {
            crate::note(&format!("core updates  local comparison failed: {error}"));
            finish_with_saved_or_error(
                &saved_snapshot,
                "Installed cores could not be read. Check the card and try again.".into(),
                events,
            );
            return;
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled);
        return;
    }
    if !failed_sources.is_empty() {
        // A partial read is useful now but must not become the complete
        // cached catalogue for later opens.
        let _ = events.send(Event::Ready {
            snapshot,
            notice: Some(unavailable_sources_notice(&failed_sources, true)),
        });
        return;
    }
    if let Err(error) = save_cache(&request.cache_dir, &cache, cancelled) {
        crate::note(&format!("core updates  cache save failed: {error}"));
        let _ = events.send(Event::Ready {
            snapshot,
            notice: Some("Core Updates were read but could not be saved.".into()),
        });
        return;
    }
    let _ = events.send(Event::Ready {
        snapshot,
        notice: None,
    });
}

fn run(request: Request, events: &SyncSender<Event>, cancelled: &AtomicBool) {
    run_with_fetch(request, events, cancelled, fetch_url);
}

pub fn start(request: Request) -> Result<Job> {
    let (sender, events) = mpsc::sync_channel(16);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-core-updates".into())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(request, &sender, &worker_cancelled)
            }));
            if outcome.is_err() {
                crate::note("core updates  worker stopped unexpectedly");
                let _ = sender.send(Event::Failed {
                    message: "Core Updates could not be opened. Try again.".into(),
                });
            }
        })
        .map_err(|error| {
            DegaussError::unsupported(
                "Core Updates",
                format!("could not start the background reader: {error}"),
            )
        })?;
    Ok(Job {
        events,
        cancelled,
        handle: Some(handle),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};
    use std::sync::atomic::AtomicUsize;

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "degauss-core-updates-{name}-{}-{sequence}",
                std::process::id()
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }

        fn write(&self, relative: &str, bytes: impl AsRef<[u8]>) -> PathBuf {
            let path = self.0.join(relative);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).unwrap();
            }
            std::fs::write(&path, bytes).unwrap();
            path
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn database(id: &str, url: &str) -> ConfiguredDatabase {
        ConfiguredDatabase {
            id: id.into(),
            url: url.into(),
            description: String::new(),
            filter: None,
        }
    }

    fn manifest(id: &str, files: &[(&str, &[u8], &[Value], bool)]) -> Vec<u8> {
        let mut descriptions = Map::new();
        for (path, contents, tags, overwrite) in files {
            descriptions.insert(
                (*path).into(),
                json!({
                    "hash": md5_hex(contents),
                    "size": contents.len(),
                    "tags": tags,
                    "overwrite": overwrite,
                }),
            );
        }
        serde_json::to_vec(&json!({
            "v": 1,
            "db_id": id,
            "timestamp": 1_800_000_000_u64,
            "files": descriptions,
            "tag_dictionary": {},
            "default_options": {"filter": "all"},
            "archives": {},
        }))
        .unwrap()
    }

    fn request(root: &TestRoot, entries: Vec<CoreEntry>, force_refresh: bool) -> Request {
        Request {
            cache_dir: root.path().join("cache"),
            menu_root: root.path().to_path_buf(),
            cores: CoreCatalogue {
                format: CoreCatalogue::FORMAT,
                entries,
            },
            force_refresh,
        }
    }

    fn core(path: PathBuf, name: &str, category: &str) -> CoreEntry {
        CoreEntry {
            name: name.into(),
            category: category.into(),
            variant: CoreVariant::Standard,
            path,
            logo_id: None,
        }
    }

    fn run_events<F>(request: Request, mut fetch: F) -> Vec<Event>
    where
        F: FnMut(&str, u64, Duration, &AtomicBool) -> std::result::Result<Vec<u8>, FetchError>,
    {
        let (sender, receiver) = mpsc::sync_channel(256);
        run_with_fetch(
            request,
            &sender,
            &AtomicBool::new(false),
            |url, limit, timeout, cancelled| fetch(url, limit, timeout, cancelled),
        );
        drop(sender);
        receiver.into_iter().collect()
    }

    fn ready(events: Vec<Event>) -> (Snapshot, Option<String>) {
        events
            .into_iter()
            .find_map(|event| match event {
                Event::Ready { snapshot, notice } => Some((snapshot, notice)),
                _ => None,
            })
            .expect("worker should finish with saved results")
    }

    #[test]
    fn game_match_uses_unique_game_artwork_and_keeps_multi_game_cores_generic() {
        let mut item = Item::fixture(
            "Fixture Core",
            LocalState::Current,
            Some(PathBuf::from("/_Console/Fixture.rbf")),
        );
        let cover = PathBuf::from("/media/fat/docs/Fixture/Artwork/game.jpg");
        let game = Row {
            name: "Fixture Game".into(),
            sort_key: String::new(),
            kind: Kind::Play(Launch::File(PathBuf::from(
                "/media/fat/games/Fixture/game.rom",
            ))),
            cover: Some(cover.clone()),
            genre: Some("Action".into()),
            favorite: false,
            below: None,
            details: Details {
                desc: "A uniquely matched game.".into(),
                publisher: "Fixture Publisher".into(),
                developer: String::new(),
                released: "2026".into(),
                players: "1".into(),
                lang: String::new(),
            },
        };

        item.set_game_match(1, Some(game.clone()));
        assert_eq!(item.cover(), Some(cover.as_path()));
        let information = item.information_with_games(None);
        assert!(information.contains("Matched game: Fixture Game"));
        assert!(information.contains("Genre: Action"));
        assert!(information.contains("Publisher: Fixture Publisher"));
        assert!(information.contains("Description\nA uniquely matched game."));

        item.set_game_match(2, None);
        assert!(item.cover().is_none());
        let second = Row {
            name: "Second Game".into(),
            sort_key: "second".into(),
            kind: Kind::Play(Launch::File(PathBuf::from(
                "/media/fat/games/Fixture/second.rom",
            ))),
            cover: None,
            genre: None,
            favorite: false,
            below: None,
            details: Details::default(),
        };
        let matches = GameMatches::from_rows(vec![
            Row {
                name: "Fixture Game".into(),
                ..game
            },
            second,
        ]);
        let information = item.information_with_games(Some(&matches));
        assert!(information.contains("Games using this core: 2"));
        assert!(information.contains("Game titles\nFixture Game\nSecond Game"));
        assert!(!information.contains("Matched game:"));
    }

    #[test]
    fn selected_system_game_worker_reads_titles_and_artwork_only_from_its_cache() {
        let root = TestRoot::new("selected-games");
        let games_dir = root.path().join("games/Fixture");
        std::fs::create_dir_all(games_dir.join("images")).unwrap();
        root.write("games/Fixture/Alpha.rom", b"alpha");
        root.write("games/Fixture/Beta.rom", b"beta");
        root.write("games/Fixture/images/alpha.jpg", b"picture");
        root.write("games/Fixture/images/beta.jpg", b"picture");
        root.write(
            "games/Fixture/gamelist.xml",
            br#"<gameList>
<game><path>Alpha.rom</path><name>Alpha Title</name><image>images/alpha.jpg</image></game>
<game><path>Beta.rom</path><name>Beta Title</name><image>images/beta.jpg</image></game>
</gameList>"#,
        );
        let config = crate::config::SystemConfig {
            name: "Fixture".into(),
            path: games_dir.to_string_lossy().into_owned(),
            extensions: vec!["rom".into()],
            rbf: "_Console/Fixture".into(),
            launch: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
            preserve_rbf_stem: false,
        };
        let library = crate::browse::Library::open(&config).unwrap();
        let cache = crate::cache::build_system(&library);
        let cache_dir = root.path().join("cache");
        crate::cache::save_system(&cache_dir, "Fixture", &cache).unwrap();
        let request = GameRequest {
            key: "fixture".into(),
            cache_dir,
            source: GameSource::Gamelist,
            target: GameTarget::System {
                id: "Fixture".into(),
            },
        };
        let matches = read_core_games(&request, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            matches
                .games
                .iter()
                .map(|game| game.name.as_str())
                .collect::<Vec<_>>(),
            ["Alpha Title", "Beta Title"]
        );
        assert!(matches.games.iter().all(|game| game.cover.is_some()));
        assert!(
            matches.single.is_none(),
            "multi-game details are not retained"
        );
    }

    #[test]
    fn selected_game_worker_honours_cancellation_before_reading() {
        let request = GameRequest {
            key: "fixture".into(),
            cache_dir: PathBuf::from("/cache-that-must-not-be-read"),
            source: GameSource::Gamelist,
            target: GameTarget::System {
                id: "Fixture".into(),
            },
        };
        assert!(read_core_games(&request, &AtomicBool::new(true))
            .unwrap()
            .games
            .is_empty());
    }

    #[test]
    fn downloader_configuration_matches_base_and_drop_in_precedence() {
        let root = TestRoot::new("config");
        root.write(
            "downloader.ini",
            br#"
[MiSTer]
filter = console

[distribution_mister]
description = Distribution

[custom]
db_url = https://example.test/custom.json
filter = [mister] arcade

[custom]
db_url = https://example.test/ignored.json
"#,
        );
        root.write(
            "downloader/01-extra.ini",
            br#"
[custom]
db_url = https://example.test/also-ignored.json

[drop_one]
db_url: https://example.test/one.json
"#,
        );
        root.write(
            "downloader_z.ini",
            br#"
[drop_two]
db_url = https://example.test/two.json
"#,
        );
        root.write(
            "downloader/ignored.INI",
            b"[not_configured]\ndb_url = https://example.test/not-configured.json\n",
        );
        root.write(
            "Downloader_ignored.ini",
            b"[also_not_configured]\ndb_url = https://example.test/also-not-configured.json\n",
        );

        let config = read_downloader_config(root.path()).unwrap();
        assert_eq!(config.global_filter, "console");
        assert!(config.global_filter_defined);
        assert_eq!(
            config
                .databases
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["distribution_mister", "custom", "drop_one", "drop_two"]
        );
        assert_eq!(config.databases[0].url, DEFAULT_DATABASE_URL);
        assert_eq!(
            config.databases[1].filter.as_deref(),
            Some("[mister] arcade")
        );
        assert_eq!(config.databases[2].url, "https://example.test/one.json");
    }

    #[test]
    fn downloader_configuration_adds_default_when_only_drop_ins_define_databases() {
        let root = TestRoot::new("implicit-default");
        root.write("downloader.ini", b"[MiSTer]\nfilter = all\n");
        root.write(
            "downloader/custom.ini",
            b"[custom]\ndb_url = https://example.test/custom.json\n",
        );

        let config = read_downloader_config(root.path()).unwrap();
        assert_eq!(
            config
                .databases
                .iter()
                .map(|entry| entry.id.as_str())
                .collect::<Vec<_>>(),
            ["distribution_mister", "custom"]
        );
    }

    #[test]
    fn downloader_configuration_rejects_global_drop_ins_and_duplicate_global_section() {
        let root = TestRoot::new("invalid-config");
        root.write("downloader.ini", b"[MiSTer]\n[MiSTer]\n");
        assert!(read_downloader_config(root.path()).is_err());

        root.write("downloader.ini", b"[MiSTer]\n");
        root.write("downloader/global.ini", b"[MiSTer]\nfilter = all\n");
        assert!(read_downloader_config(root.path()).is_err());
    }

    #[test]
    fn ini_parser_treats_deeper_indentation_as_a_continuation() {
        let parsed = parse_ini(
            Path::new("test.ini"),
            "[custom]\n  db_url = https://example.test/\n    path.json\n  description: Example\n",
        )
        .unwrap();
        assert_eq!(
            parsed.sections[0].values.get("db_url").map(String::as_str),
            Some("https://example.test/ path.json")
        );
        assert_eq!(
            parsed.sections[0]
                .values
                .get("description")
                .map(String::as_str),
            Some("Example")
        );
    }

    #[test]
    fn web_urls_follow_downloader_scheme_rules() {
        assert!(safe_web_url("https://example.test/db.json.zip"));
        assert!(safe_web_url("http://example.test/db.json"));
        assert!(safe_web_url("https://localhost/db.json"));
        assert!(safe_web_url("http://127.0.0.1/db.json"));
        assert!(safe_web_url("http://10.0.0.2/db.json"));
        assert!(safe_web_url("http://[::1]/db.json"));
        assert!(!safe_web_url("file:///media/fat/db.json"));
        assert!(!safe_web_url("https://example.test/db.json\noutput=x"));
        assert!(!safe_web_url("https://example.test/db name.json"));
    }

    #[test]
    fn downloader_filters_include_essential_and_exclude_negative_tags() {
        let dictionary = BTreeMap::from([
            ("essential".into(), 1),
            ("console".into(), 2),
            ("beta".into(), 3),
        ]);
        let rule = filter_rule("console !beta", &dictionary).unwrap().unwrap();
        let tagged = |tags: Vec<Value>| FileDescription {
            hash: "0".repeat(32),
            size: 1,
            tags,
            overwrite: true,
        };
        assert!(selected_by_filter(&tagged(vec![json!(1)]), Some(&rule)));
        assert!(selected_by_filter(&tagged(vec![json!(2)]), Some(&rule)));
        assert!(!selected_by_filter(
            &tagged(vec![json!(2), json!(3)]),
            Some(&rule)
        ));
        assert!(!selected_by_filter(&tagged(vec![json!(4)]), Some(&rule)));
        assert!(filter_rule("all", &dictionary).unwrap().is_none());
        let none = filter_rule("!all", &dictionary).unwrap().unwrap();
        assert!(!selected_by_filter(&tagged(vec![json!(1)]), Some(&none)));
        assert!(filter_rule("none", &dictionary).is_err());
    }

    #[test]
    fn exact_local_path_wins_before_a_looser_identity_match() {
        let remotes = vec![
            RemoteCore {
                database: "first".into(),
                source_label: "First".into(),
                path: "_Other/Foo.rbf".into(),
                identity: "foo".into(),
                title: "Foo".into(),
                category: "Other".into(),
                hash: "0".repeat(32),
                size: 1,
                overwrite: true,
                build: String::new(),
            },
            RemoteCore {
                database: "second".into(),
                source_label: "Second".into(),
                path: "_Console/Foo.rbf".into(),
                identity: "foo".into(),
                title: "Foo".into(),
                category: "Console".into(),
                hash: "0".repeat(32),
                size: 1,
                overwrite: true,
                build: String::new(),
            },
        ];
        let locals = vec![LocalCore {
            relative: "_Console/Foo.rbf".into(),
            identity: "foo".into(),
            title: "Foo".into(),
            category: "Console".into(),
            build: String::new(),
            file_path: PathBuf::from("_Console/Foo.rbf"),
            launch_path: None,
            logo_id: None,
        }];
        assert_eq!(assign_local_cores(&remotes, &locals), vec![None, Some(0)]);
    }

    #[test]
    fn alternate_standard_builds_are_kept_and_matched_by_exact_path() {
        const URL: &str = "https://example.test/distribution.json";
        let root = TestRoot::new("alternate-standard-builds");
        root.write(
            "downloader.ini",
            format!("[MiSTer]\nfilter = all\n\n[distribution_mister]\ndb_url = {URL}\n"),
        );
        let standard = root.write("_Console/console_extra/N64_94MHz_20260131.rbf", b"standard");
        let alternate = root.write(
            "_Console/console_extra/N64_94MHz+_20260809.rbf",
            b"alternate",
        );
        root.write("_@Favorites/cores/Hidden_20260809.rbf", b"hidden");
        let database = manifest(
            "distribution_mister",
            &[
                (
                    "_Console/console_extra/N64_94MHz_20260131.rbf",
                    b"standard",
                    &[],
                    true,
                ),
                (
                    "_Console/console_extra/N64_94MHz+_20260809.rbf",
                    b"alternate",
                    &[],
                    true,
                ),
            ],
        );
        let (snapshot, notice) = ready(run_events(
            request(
                &root,
                vec![core(standard.clone(), "N64 94MHz", "Console")],
                false,
            ),
            |url, _, _, _| {
                assert_eq!(url, URL);
                Ok(database.clone())
            },
        ));
        assert!(notice.is_none());
        let find = |title: &str| {
            snapshot
                .items
                .iter()
                .find(|item| item.title() == title)
                .unwrap()
        };
        assert_eq!(find("N64 94MHz").state, LocalState::Current);
        assert_eq!(find("N64 94MHz").launch_path, Some(standard));
        assert_eq!(find("N64 94MHz+").state, LocalState::Current);
        assert_eq!(find("N64 94MHz+").launch_path, Some(alternate));
        assert!(snapshot.items.iter().all(|item| item.title() != "Hidden"));
    }

    #[test]
    fn configured_databases_produce_installed_available_and_local_only_rows() {
        const DISTRIBUTION_URL: &str = "https://example.test/distribution.json";
        const JT_URL: &str = "https://example.test/jt.json";
        const CUSTOM_URL: &str = "https://example.test/custom.json";
        let root = TestRoot::new("end-to-end");
        root.write(
            "downloader.ini",
            format!(
                "[MiSTer]\nfilter = all\n\n[distribution_mister]\ndb_url = {DISTRIBUTION_URL}\ndescription = Distribution\n\n[jtcores]\ndb_url = {JT_URL}\n\n[custom_db]\ndb_url = {CUSTOM_URL}\ndescription = Custom Source\n"
            ),
        );
        let official = root.write("_Console/Official_20260901.rbf", b"official-old");
        let jt = root.write("_Console/JTCore.rbf", b"jt-current");
        let manual = root.write("_Other/ManualCore_20260801.rbf", b"manual");
        root.write("_Arcade/cores/ArcadeCore_20260901.rbf", b"arcade-current");
        root.write("_RA_Cores/Cores/RAExample.rbf", b"ra-current");
        let ra_launcher = root.write(
            "_RA_Cores/RAExample.mgl",
            b"<mistergamedescription><rbf>_RA_Cores/Cores/RAExample</rbf><setname>RA_RAExample</setname></mistergamedescription>",
        );
        let entries = vec![
            core(official, "Official", "Console"),
            core(jt, "JT Core", "Console"),
            core(manual, "Manual Core", "Other"),
            CoreEntry {
                name: "RA Example".into(),
                category: "Console".into(),
                variant: CoreVariant::RetroAchievements,
                path: ra_launcher.clone(),
                logo_id: None,
            },
        ];
        let distribution = manifest(
            "distribution_mister",
            &[
                ("_Console/Official_20260902.rbf", b"official-new", &[], true),
                ("_Computer/NewCore_20260902.rbf", b"new", &[], true),
                ("_RA_Cores/Cores/RAExample.rbf", b"ra-current", &[], true),
            ],
        );
        let jt_manifest = manifest(
            "jtcores",
            &[("_Console/JTCore.rbf", b"jt-current", &[], true)],
        );
        let custom = manifest(
            "custom_db",
            &[
                (
                    "_Arcade/cores/ArcadeCore_20260901.rbf",
                    b"arcade-current",
                    &[],
                    true,
                ),
                ("_Utility/CustomNew_20260901.rbf", b"custom", &[], true),
                ("_Computer/NewCore_20260902.rbf", b"duplicate", &[], true),
            ],
        );

        let (snapshot, notice) = ready(run_events(
            request(&root, entries, false),
            |url, _, _, _| {
                Ok(match url {
                    DISTRIBUTION_URL => distribution.clone(),
                    JT_URL => jt_manifest.clone(),
                    CUSTOM_URL => custom.clone(),
                    other => panic!("unexpected database {other}"),
                })
            },
        ));
        assert!(notice.is_none());
        let find = |title: &str, source: &str| {
            snapshot
                .items
                .iter()
                .find(|item| item.title() == title && item.source() == source)
                .unwrap()
        };
        assert_eq!(
            find("Official", "distribution_mister").state,
            LocalState::UpdateAvailable
        );
        assert_eq!(find("JT Core", "jtcores").state, LocalState::Current);
        assert_eq!(
            find("RA Example", "distribution_mister").launch_path,
            Some(ra_launcher)
        );
        assert_eq!(find("ArcadeCore", "custom_db").state, LocalState::Current);
        assert!(find("ArcadeCore", "custom_db").launch_path.is_none());
        assert_eq!(
            find("NewCore", "distribution_mister").state,
            LocalState::AvailableThroughDownloader
        );
        assert_eq!(
            find("CustomNew", "custom_db").state,
            LocalState::AvailableThroughDownloader
        );
        assert_eq!(find("Manual Core", "local").state, LocalState::LocalOnly);
        assert_eq!(
            snapshot
                .items
                .iter()
                .filter(|item| item.title() == "NewCore")
                .count(),
            1,
            "the first configured database owns an exact target path"
        );
    }

    #[test]
    fn cache_avoids_network_and_survives_a_failed_explicit_refresh() {
        const URL: &str = "https://example.test/distribution.json";
        let root = TestRoot::new("cache");
        root.write(
            "downloader.ini",
            format!("[distribution_mister]\ndb_url = {URL}\n"),
        );
        let installed = root.write("_Console/Installed_20260901.rbf", b"old");
        let request = request(&root, vec![core(installed, "Installed", "Console")], false);
        let body = manifest(
            "distribution_mister",
            &[("_Console/Installed_20260902.rbf", b"new", &[], true)],
        );
        let calls = AtomicUsize::new(0);
        let (first, notice) = ready(run_events(request.clone(), |url, _, _, _| {
            assert_eq!(url, URL);
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(body.clone())
        }));
        assert!(notice.is_none());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert_eq!(first.items.len(), 1);

        let (cached, notice) = ready(run_events(request.clone(), |_, _, _, _| {
            panic!("a normal cached open must not use the network")
        }));
        assert!(notice.is_none());
        assert_eq!(cached.items.len(), 1);

        let mut refresh = request.clone();
        refresh.force_refresh = true;
        let (saved, notice) = ready(run_events(refresh, |_, _, _, _| {
            Err(FetchError {
                user: "network unavailable".into(),
                diagnostic: "offline".into(),
                cancelled: false,
            })
        }));
        assert_eq!(saved.items.len(), 1);
        let notice = notice.unwrap();
        assert!(notice.contains("Showing saved results"));
        assert!(notice.contains("MiSTer Distribution: network unavailable"));

        root.write(
            "downloader.ini",
            format!("[distribution_mister]\ndb_url = {URL}\ndescription = Changed\n"),
        );
        let refetches = AtomicUsize::new(0);
        let (_, notice) = ready(run_events(request, |_, _, _, _| {
            refetches.fetch_add(1, Ordering::Relaxed);
            Ok(body.clone())
        }));
        assert!(notice.is_none());
        assert_eq!(refetches.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn one_timed_out_database_does_not_hide_other_core_updates_or_poison_cache() {
        const FIRST: &str = "https://example.test/first.json";
        const SLOW: &str = "https://example.test/slow.json";
        const LAST: &str = "https://example.test/last.json";
        let root = TestRoot::new("partial-database");
        root.write(
            "downloader.ini",
            format!(
                "[first]\ndb_url = {FIRST}\n[slow]\ndb_url = {SLOW}\n[last]\ndb_url = {LAST}\n"
            ),
        );
        let request = request(&root, Vec::new(), false);
        let first = manifest(
            "first",
            &[("_Console/First_20260901.rbf", b"one", &[], true)],
        );
        let last = manifest("last", &[("_Console/Last_20260901.rbf", b"two", &[], true)]);
        let (snapshot, notice) = ready(run_events(request.clone(), |url, _, _, _| match url {
            FIRST => Ok(first.clone()),
            LAST => Ok(last.clone()),
            SLOW => Err(FetchError {
                user: "A Downloader database did not respond in time.".into(),
                diagnostic: "curl exit Some(28)".into(),
                cancelled: false,
            }),
            other => panic!("unexpected URL {other}"),
        }));
        assert_eq!(snapshot.items.len(), 2);
        let notice = notice.unwrap();
        assert!(notice.contains("Slow: A Downloader database did not respond in time"));
        assert!(
            !cache_path(&request.cache_dir).exists(),
            "partial results must be retried"
        );

        let calls = AtomicUsize::new(0);
        let (recovered, notice) = ready(run_events(request, |url, _, _, _| {
            calls.fetch_add(1, Ordering::Relaxed);
            Ok(match url {
                FIRST => first.clone(),
                LAST => last.clone(),
                SLOW => manifest(
                    "slow",
                    &[("_Console/Slow_20260901.rbf", b"three", &[], true)],
                ),
                other => panic!("unexpected URL {other}"),
            })
        }));
        assert_eq!(calls.load(Ordering::Relaxed), 3);
        assert_eq!(recovered.items.len(), 3);
        assert!(notice.is_none());
    }

    #[test]
    fn no_reachable_source_without_saved_results_reports_the_failed_names() {
        let root = TestRoot::new("all-failed");
        root.write(
            "downloader.ini",
            "[custom]\ndb_url = https://example.test/core.json\n",
        );
        let events = run_events(request(&root, Vec::new(), false), |_, _, _, _| {
            Err(FetchError {
                user: "A Downloader database did not respond in time.".into(),
                diagnostic: "curl exit Some(28)".into(),
                cancelled: false,
            })
        });
        let message = events.into_iter().find_map(|event| match event {
            Event::Failed { message } => Some(message),
            _ => None,
        });
        let message = message.expect("no source and no saved result must be a failure");
        assert!(message.contains("Custom: A Downloader database did not respond in time"));
        assert!(!message.contains("Other results are shown"));
    }

    #[test]
    fn timed_out_archive_summary_skips_only_that_archive() {
        const DATABASE_URL: &str = "https://example.test/db.json";
        const SLOW_SUMMARY: &str = "https://example.test/slow-summary.json";
        let body = serde_json::to_vec(&json!({
            "v": 1,
            "db_id": "custom",
            "timestamp": 1_800_000_000_u64,
            "files": {
                "_Console/Direct_20260901.rbf": {
                    "hash": md5_hex(b"direct"), "size": 6, "tags": []
                }
            },
            "tag_dictionary": {},
            "default_options": {"filter": "all"},
            "archives": {
                "slow_bundle": {
                    "extract": "selective", "target_folder": "",
                    "summary_file": {
                        "hash": md5_hex(b"summary"), "size": 7, "url": SLOW_SUMMARY
                    }
                },
                "working_bundle": {
                    "extract": "selective", "target_folder": "",
                    "summary_inline": {"files": {
                        "_Console/Working_20260901.rbf": {
                            "hash": md5_hex(b"working"), "size": 7, "tags": []
                        }
                    }}
                }
            }
        }))
        .unwrap();
        let configured = database("custom", DATABASE_URL);
        let config = DownloaderConfig {
            global_filter: String::new(),
            global_filter_defined: false,
            databases: vec![configured.clone()],
        };
        let (sender, _receiver) = mpsc::sync_channel(16);
        let mut failures = Vec::new();
        let mut fetch = |url: &str, _: u64, _: Duration, _: &AtomicBool| match url {
            DATABASE_URL => Ok(body.clone()),
            SLOW_SUMMARY => Err(FetchError {
                user: "A Downloader database did not respond in time.".into(),
                diagnostic: "curl exit Some(28)".into(),
                cancelled: false,
            }),
            other => panic!("unexpected URL {other}"),
        };
        let cores = fetch_database(
            &config,
            &configured,
            &AtomicBool::new(false),
            &sender,
            &mut fetch,
            &mut failures,
        )
        .unwrap();
        assert_eq!(
            cores
                .iter()
                .map(|core| core.title.as_str())
                .collect::<Vec<_>>(),
            ["Direct", "Working"]
        );
        assert_eq!(failures.len(), 1);
        assert!(failures[0].contains("Custom / slow_bundle"));
        assert!(failures[0].contains("did not respond in time"));
    }

    #[test]
    fn external_archive_summary_takes_priority_over_inline_data() {
        const DATABASE_URL: &str = "https://example.test/db.json";
        const SUMMARY_URL: &str = "https://example.test/summary.json";
        let summary = serde_json::to_vec(&json!({
            "v": 1,
            "files": {
                "_Console/External_20260901.rbf": {
                    "hash": md5_hex(b"external"),
                    "size": 8,
                    "tags": []
                }
            },
            "folders": {}
        }))
        .unwrap();
        let database_body = serde_json::to_vec(&json!({
            "v": 1,
            "db_id": "custom",
            "timestamp": 1_800_000_000_u64,
            "files": {},
            "tag_dictionary": {},
            "default_options": {"filter": "all"},
            "archives": {
                "bundle": {
                    "extract": "selective",
                    "target_folder": "",
                    "summary_inline": {
                        "files": {
                            "_Console/Inline_20260901.rbf": {
                                "hash": md5_hex(b"inline"),
                                "size": 6,
                                "tags": []
                            }
                        }
                    },
                    "summary_file": {
                        "hash": md5_hex(&summary),
                        "size": summary.len(),
                        "url": SUMMARY_URL
                    }
                }
            }
        }))
        .unwrap();
        let configured = database("custom", DATABASE_URL);
        let config = DownloaderConfig {
            global_filter: String::new(),
            global_filter_defined: false,
            databases: vec![configured.clone()],
        };
        let (sender, _receiver) = mpsc::sync_channel(16);
        let mut fetch = |url: &str, _: u64, _: Duration, _: &AtomicBool| {
            Ok(match url {
                DATABASE_URL => database_body.clone(),
                SUMMARY_URL => summary.clone(),
                other => panic!("unexpected URL {other}"),
            })
        };
        let mut failed_sources = Vec::new();
        let cores = fetch_database(
            &config,
            &configured,
            &AtomicBool::new(false),
            &sender,
            &mut fetch,
            &mut failed_sources,
        )
        .unwrap();
        assert_eq!(cores.len(), 1);
        assert_eq!(cores[0].title, "External");
        assert!(failed_sources.is_empty());
    }

    #[test]
    fn cancellation_finishes_without_fetching() {
        let root = TestRoot::new("cancel");
        let (sender, receiver) = mpsc::sync_channel(16);
        let cancelled = AtomicBool::new(true);
        run_with_fetch(
            request(&root, Vec::new(), true),
            &sender,
            &cancelled,
            |_, _, _, _| panic!("cancelled work must not fetch"),
        );
        drop(sender);
        assert!(receiver
            .into_iter()
            .any(|event| matches!(event, Event::Cancelled)));
    }

    #[test]
    #[ignore = "requires current public Downloader databases"]
    fn live_configured_databases_use_the_production_fetch_path() {
        let root = TestRoot::new("live");
        root.write(
            "downloader.ini",
            br#"
[distribution_mister]
db_url = https://raw.githubusercontent.com/MiSTer-devel/Distribution_MiSTer/main/db.json.zip

[jtcores]
db_url = https://raw.githubusercontent.com/jotego/jtcores_mister/main/jtbindb.json.zip

[Coin-OpCollection/Distribution-MiSTerFPGA]
db_url = https://raw.githubusercontent.com/Coin-OpCollection/Distribution-MiSTerFPGA/db/db.json.zip
"#,
        );
        root.write(
            "downloader/extra-cores.ini",
            br#"

[meathax/meatcores]
db_url = https://raw.githubusercontent.com/meathax/meatcores/db/db.json.zip

[rmonic79/rmcores]
db_url = https://raw.githubusercontent.com/rmonic79/rmcores/db/db.json.zip
"#,
        );
        root.write(
            "downloader_optional-cores.ini",
            br#"

[TheJesusFish/Slop-Core]
db_url = https://raw.githubusercontent.com/TheJesusFish/Slop-Core/db/db.json.zip

[theypsilon_unofficial_distribution]
db_url = https://raw.githubusercontent.com/theypsilon/Unofficial_Distribution_MiSTer/main/unofficialdb.json.zip
"#,
        );
        let (snapshot, notice) = ready(run_events(request(&root, Vec::new(), true), fetch_url));
        assert!(notice.is_none());
        assert!(!snapshot.items.is_empty());
        let sources = snapshot
            .items
            .iter()
            .map(Item::source)
            .collect::<BTreeSet<_>>();
        assert_eq!(
            sources,
            BTreeSet::from([
                "coin-opcollection/distribution-misterfpga",
                "distribution_mister",
                "jtcores",
                "meathax/meatcores",
                "rmonic79/rmcores",
                "thejesusfish/slop-core",
                "theypsilon_unofficial_distribution",
            ])
        );
        assert!(snapshot.items.iter().all(|item| {
            item.state == LocalState::AvailableThroughDownloader && !item.installed()
        }));
    }
}
