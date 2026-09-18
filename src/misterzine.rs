//! Optional MiSTerZine updates browser for content installed on this MiSTer.
//!
//! The official feed is kept in one independent cache. A single cancellable
//! worker reads that cache, checks the fixed HTTPS endpoint, downloads only
//! when needed, and matches releases against Degauss's existing catalogues.
//! It never scans the card and never changes a game or core index.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::browse::{Details, Kind, Launch, Row};
use crate::error::{DegaussError, Result};
use crate::systems::{CoreCatalogue, CoreVariant};

const META_URL: &str = "https://misterzine.fyi/releases/meta.json";
const DATA_URL: &str = "https://misterzine.fyi/releases/data.json";
const CACHE_FORMAT: u32 = 2;
const LEGACY_CACHE_FORMAT: u32 = 1;
const MAX_META_BYTES: u64 = 16 * 1024;
const MAX_DATA_BYTES: u64 = 2 * 1024 * 1024;
const AVAILABILITY_CACHE_FORMAT: u32 = 1;
const MAX_UPDATE_ALL_ARCHIVE_BYTES: u64 = 4 * 1024 * 1024;
const MAX_UPDATE_ALL_JSON_BYTES: u64 = 16 * 1024 * 1024;
const MAX_UPDATE_ALL_FILES: usize = 100_000;
const MAX_ROWS: usize = 20_000;
const MAX_SHORT_TEXT: usize = 256;
const MAX_PATH_TEXT: usize = 1_024;
const CURL_POLL: Duration = Duration::from_millis(25);
const CA_BUNDLES: [&str; 3] = [
    "/etc/ssl/certs/cacert.pem",
    "/etc/ssl/cert.pem",
    "/etc/ssl/certs/ca-certificates.crt",
];
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy)]
struct UpdateAllDatabase {
    id: &'static str,
    url: &'static str,
}

const UPDATE_ALL_DATABASES: [UpdateAllDatabase; 4] = [
    UpdateAllDatabase {
        id: "distribution_mister",
        url: "https://raw.githubusercontent.com/MiSTer-devel/Distribution_MiSTer/main/db.json.zip",
    },
    UpdateAllDatabase {
        id: "jtcores",
        url: "https://raw.githubusercontent.com/jotego/jtcores_mister/main/jtbindb.json.zip",
    },
    UpdateAllDatabase {
        id: "Coin-OpCollection/Distribution-MiSTerFPGA",
        url: "https://raw.githubusercontent.com/Coin-OpCollection/Distribution-MiSTerFPGA/db/db.json.zip",
    },
    UpdateAllDatabase {
        id: "theypsilon_unofficial_distribution",
        url: "https://raw.githubusercontent.com/theypsilon/Distribution_Unofficial_MiSTer/main/unofficialdb.json.zip",
    },
];

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct AvailabilityCache {
    format: u32,
    databases: Vec<AvailabilityDatabase>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct AvailabilityDatabase {
    id: String,
    timestamp: u64,
    cores: Vec<String>,
    mras: Vec<String>,
}

#[derive(Deserialize)]
struct UpdateAllManifest {
    db_id: String,
    timestamp: u64,
    files: BTreeMap<String, serde::de::IgnoredAny>,
}

impl AvailabilityCache {
    fn contains(&self, release: &Release) -> bool {
        let core = crate::systems::core_name(&release.core);
        if core.is_empty() {
            return false;
        }
        if release.base == "Arcade" {
            let Some(mra) = release.mra.as_deref().and_then(normalize_manifest_mra) else {
                return false;
            };
            self.databases.iter().any(|database| {
                database.cores.binary_search(&core).is_ok()
                    && database.mras.binary_search(&mra).is_ok()
            })
        } else {
            self.databases
                .iter()
                .any(|database| database.cores.binary_search(&core).is_ok())
        }
    }
}

fn normalize_manifest_mra(path: &str) -> Option<String> {
    if path.starts_with('/')
        || path.len() > MAX_PATH_TEXT
        || path.contains('\\')
        || path.chars().any(char::is_control)
    {
        return None;
    }
    let parsed = Path::new(path);
    if path.is_empty()
        || parsed
            .extension()
            .is_none_or(|extension| !extension.eq_ignore_ascii_case("mra"))
        || parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return None;
    }
    Some(path.to_ascii_lowercase())
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Meta {
    pub updated: String,
    pub hash: String,
    pub rows: usize,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct Release {
    #[serde(default)]
    title: String,
    #[serde(default)]
    base: String,
    #[serde(default)]
    date: String,
    #[serde(default)]
    src: Option<String>,
    #[serde(default)]
    beta: bool,
    #[serde(default)]
    deprecated: bool,
    #[serde(default)]
    manufacturer: String,
    #[serde(default)]
    core: String,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    bd: Option<String>,
    #[serde(default)]
    b: i64,
    #[serde(default)]
    k: String,
    #[serde(default)]
    mra: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    format: u32,
    pub meta: Meta,
    rows: Vec<Release>,
}

/// The released 0.8.0 candidate cache shape. Postcard encodes structs by
/// field order, so it is decoded explicitly and upgraded in memory.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyRelease {
    title: String,
    base: String,
    manufacturer: String,
    core: String,
    updated: String,
    bd: String,
    b: i64,
    k: String,
    mra: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
struct LegacyCache {
    format: u32,
    meta: Meta,
    rows: Vec<LegacyRelease>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalState {
    NotInstalled,
    MraNotInstalled,
    CoreMissing,
    AvailableThroughUpdateAll,
    VersionUnknown,
    Current,
    UpdateAvailable,
}

impl LocalState {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "Not installed",
            Self::MraNotInstalled => "MRA not installed",
            Self::CoreMissing => "MRA installed · Core missing",
            Self::AvailableThroughUpdateAll => "Available through Update All",
            Self::VersionUnknown => "Installed · Version unknown",
            Self::Current => "Installed · Current",
            Self::UpdateAvailable => "Installed · Update available",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    key: String,
    title: String,
    base: String,
    date: String,
    source: String,
    beta: bool,
    deprecated: bool,
    updated: String,
    batch: i64,
    manufacturer: String,
    core: String,
    state: LocalState,
    pub launch_path: Option<PathBuf>,
    pub cover: Option<PathBuf>,
    pub logo_id: Option<String>,
}

impl Item {
    pub fn key(&self) -> &str {
        &self.key
    }

    pub fn installed(&self) -> bool {
        self.launch_path.is_some()
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
        readable_source(&self.source)
    }

    pub fn state_label(&self) -> &'static str {
        self.state.label()
    }

    pub fn status_label(&self) -> String {
        match (self.beta, self.deprecated) {
            (true, true) => "Beta · Deprecated".to_string(),
            (true, false) => "Beta".to_string(),
            (false, true) => "Deprecated".to_string(),
            (false, false) => String::new(),
        }
    }

    pub fn display_title(&self) -> String {
        let mut title = self.title.clone();
        if self.beta {
            title.push_str(" [Beta]");
        }
        if self.deprecated {
            title.push_str(" [Deprecated]");
        }
        title
    }

    pub fn information(&self) -> String {
        let mut text = self.display_title();
        for (label, value) in [
            ("Local status", self.state.label().to_string()),
            ("Type", self.base.clone()),
            ("Source", self.source_label()),
            ("Release status", self.status_label()),
            ("Core", self.core.clone()),
            ("Manufacturer", self.manufacturer.clone()),
            ("MiSTer debut", self.date.clone()),
            ("Latest shipped update", self.updated.clone()),
        ] {
            if !value.trim().is_empty() {
                text.push_str(&format!("\n\n{label}: {value}"));
            }
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
        let source = self.source_label();
        if !source.is_empty() {
            context.push(source);
        }
        let status = self.status_label();
        if !status.is_empty() {
            context.push(status);
        }
        Row {
            name: self.display_title(),
            sort_key: self.title.to_ascii_lowercase(),
            // An unavailable release is intercepted before launch. Keeping
            // it a non-folder lets every existing layout present it like the
            // neighbouring installed releases.
            kind: Kind::Play(Launch::File(self.launch_path.clone().unwrap_or_default())),
            cover,
            genre: Some(context.join(" · ")),
            favorite: false,
            below: None,
            details: Details {
                desc: self.state.label().to_string(),
                publisher: String::new(),
                developer: String::new(),
                released: self.updated.clone(),
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
            date: "2026-01-01".to_string(),
            source: source.to_string(),
            beta: false,
            deprecated: false,
            updated: "2026-09-16".to_string(),
            batch: 1,
            manufacturer: "Fixture Publisher".to_string(),
            core: "Fixture".to_string(),
            state,
            launch_path,
            cover: None,
            logo_id: None,
        }
    }
}

fn readable_source(source: &str) -> String {
    match source {
        "" => "Unknown".to_string(),
        "distribution_mister" => "MiSTer Distribution".to_string(),
        "coinop" => "Coin-Op".to_string(),
        "jtbindb" => "Jotego".to_string(),
        "meathax" => "MeatHax".to_string(),
        "rmcores" => "RMCores".to_string(),
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
        if field == FilterField::Source || !key.is_empty() {
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArcadeSystem {
    pub id: String,
    pub artwork_pack: bool,
}

#[derive(Debug, Clone)]
pub struct Request {
    pub cache_dir: PathBuf,
    pub menu_root: PathBuf,
    pub cores: CoreCatalogue,
    pub arcade_systems: Vec<ArcadeSystem>,
    pub force_refresh: bool,
    /// The matcher and cache support Update All availability, but the current
    /// browser deliberately shows installed updates only.
    pub include_available: bool,
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
            Self::Checking => "Checking MiSTerZine Updates",
            Self::Downloading => "Downloading Releases",
            Self::CheckingAvailability => "Checking Update All Availability",
            Self::Matching => "Matching Installed Content",
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
    /// A valid saved snapshot is ready before the network check finishes.
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

pub fn start(request: Request) -> Result<Job> {
    let (sender, events) = mpsc::sync_channel(16);
    let cancelled = Arc::new(AtomicBool::new(false));
    let worker_cancelled = Arc::clone(&cancelled);
    let handle = std::thread::Builder::new()
        .name("degauss-misterzine".to_string())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                run(request, &sender, &worker_cancelled)
            }));
            if outcome.is_err() {
                crate::note("misterzine   worker stopped unexpectedly");
                let _ = sender.send(Event::Failed {
                    message: "MiSTerZine could not be opened. Try again.".to_string(),
                });
            }
        })
        .map_err(|error| {
            DegaussError::unsupported(
                "MiSTerZine",
                format!("could not start the background reader: {error}"),
            )
        })?;
    Ok(Job {
        events,
        cancelled,
        handle: Some(handle),
    })
}

fn run(request: Request, events: &SyncSender<Event>, cancelled: &AtomicBool) {
    run_with_fetches(
        request,
        events,
        cancelled,
        fetch_json,
        fetch_availability_database,
    );
}

#[cfg(test)]
fn run_with_fetch<F>(request: Request, events: &SyncSender<Event>, cancelled: &AtomicBool, fetch: F)
where
    F: FnMut(&str, u64, Duration, &AtomicBool) -> std::result::Result<Vec<u8>, FetchError>,
{
    run_with_fetches(request, events, cancelled, fetch, |_, _| {
        Err(FetchError {
            user: "Update All availability could not be checked.".to_string(),
            diagnostic: "no availability fetcher was configured".to_string(),
            cancelled: false,
        })
    });
}

fn run_with_fetches<F, G>(
    request: Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
    mut fetch: F,
    mut fetch_availability: G,
) where
    F: FnMut(&str, u64, Duration, &AtomicBool) -> std::result::Result<Vec<u8>, FetchError>,
    G: FnMut(
        UpdateAllDatabase,
        &AtomicBool,
    ) -> std::result::Result<AvailabilityDatabase, FetchError>,
{
    let cache_result = load_cache(&request.cache_dir);
    let cache_problem = cache_result.is_err();
    let mut cached = match cache_result {
        Ok(cache) => cache,
        Err(error) => {
            crate::note(&format!("misterzine   saved data rejected: {error}"));
            None
        }
    };
    let availability_result = if request.include_available {
        load_availability_cache(&request.cache_dir)
    } else {
        Ok(None)
    };
    let availability_cache_problem = availability_result.is_err();
    let cached_availability = match availability_result {
        Ok(cache) => cache,
        Err(error) => {
            crate::note(&format!(
                "misterzine   saved Update All availability rejected: {error}"
            ));
            None
        }
    };
    let mut cached_snapshot = None;
    if let Some(cache) = cached.as_ref() {
        send_progress(events, Phase::Matching, 0, cache.rows.len(), false);
        match match_cache_with_availability(
            cache,
            &request,
            cached_availability.as_ref(),
            events,
            cancelled,
        ) {
            Some(snapshot) => {
                cached_snapshot = Some(snapshot.clone());
                if events.send(Event::Snapshot(snapshot)).is_err() {
                    return;
                }
            }
            None => {
                let _ = events.send(Event::Cancelled);
                return;
            }
        }
    }
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled);
        return;
    }

    send_progress(events, Phase::Checking, 0, 0, false);
    let meta = match fetch(META_URL, MAX_META_BYTES, Duration::from_secs(6), cancelled) {
        Ok(body) => match decode_meta(&body) {
            Ok(meta) => meta,
            Err(error) => {
                finish_with_saved_or_error(
                    cached_snapshot,
                    "MiSTerZine returned data Degauss could not read.",
                    &error,
                    cache_problem,
                    events,
                );
                return;
            }
        },
        Err(error) if error.cancelled => {
            let _ = events.send(Event::Cancelled);
            return;
        }
        Err(error) => {
            finish_with_saved_or_error(
                cached_snapshot,
                &error.user,
                &error.diagnostic,
                cache_problem,
                events,
            );
            return;
        }
    };

    if !request.force_refresh
        && cached
            .as_ref()
            .is_some_and(|cache| cache.format == CACHE_FORMAT && cache.meta.hash == meta.hash)
    {
        let cache = cached.as_ref().expect("matching saved release cache");
        finish_after_release_refresh(
            cache,
            &request,
            cached_availability,
            availability_cache_problem,
            events,
            cancelled,
            &mut fetch_availability,
        );
        return;
    }

    send_progress(events, Phase::Downloading, 0, meta.rows, false);
    let body = match fetch(DATA_URL, MAX_DATA_BYTES, Duration::from_secs(20), cancelled) {
        Ok(body) => body,
        Err(error) if error.cancelled => {
            let _ = events.send(Event::Cancelled);
            return;
        }
        Err(error) => {
            finish_with_saved_or_error(
                cached_snapshot,
                &error.user,
                &error.diagnostic,
                cache_problem,
                events,
            );
            return;
        }
    };
    let cache = match decode_data(meta, &body) {
        Ok(cache) => cache,
        Err(error) => {
            finish_with_saved_or_error(
                cached_snapshot,
                "MiSTerZine returned data Degauss could not read.",
                &error,
                cache_problem,
                events,
            );
            return;
        }
    };
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled);
        return;
    }
    match save_cache(&request.cache_dir, &cache, cancelled) {
        Ok(()) => {}
        Err(SaveCacheError::Cancelled) => {
            let _ = events.send(Event::Cancelled);
            return;
        }
        Err(SaveCacheError::Failed(error)) => {
            finish_with_saved_or_error(
                cached_snapshot,
                "MiSTerZine releases could not be saved.",
                &error.to_string(),
                cache_problem,
                events,
            );
            return;
        }
    }
    cached = Some(cache);
    let cache = cached.as_ref().expect("new cache retained");
    finish_after_release_refresh(
        cache,
        &request,
        cached_availability,
        availability_cache_problem,
        events,
        cancelled,
        &mut fetch_availability,
    );
}

fn finish_after_release_refresh<G>(
    cache: &Cache,
    request: &Request,
    cached_availability: Option<AvailabilityCache>,
    availability_cache_problem: bool,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
    fetch_availability: &mut G,
) where
    G: FnMut(
        UpdateAllDatabase,
        &AtomicBool,
    ) -> std::result::Result<AvailabilityDatabase, FetchError>,
{
    let (availability, notice) = if request.include_available {
        refresh_availability(
            request,
            cached_availability,
            availability_cache_problem,
            events,
            cancelled,
            fetch_availability,
        )
    } else {
        (None, None)
    };
    if cancelled.load(Ordering::Relaxed) {
        let _ = events.send(Event::Cancelled);
        return;
    }
    send_progress(events, Phase::Matching, 0, cache.rows.len(), false);
    match match_cache_with_availability(cache, request, availability.as_ref(), events, cancelled) {
        Some(snapshot) => {
            let _ = events.send(Event::Ready { snapshot, notice });
        }
        None => {
            let _ = events.send(Event::Cancelled);
        }
    }
}

fn refresh_availability<G>(
    request: &Request,
    cached: Option<AvailabilityCache>,
    cache_problem: bool,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
    fetch: &mut G,
) -> (Option<AvailabilityCache>, Option<String>)
where
    G: FnMut(
        UpdateAllDatabase,
        &AtomicBool,
    ) -> std::result::Result<AvailabilityDatabase, FetchError>,
{
    let mut databases = Vec::with_capacity(UPDATE_ALL_DATABASES.len());
    send_progress(
        events,
        Phase::CheckingAvailability,
        0,
        UPDATE_ALL_DATABASES.len(),
        false,
    );
    for (index, expected) in UPDATE_ALL_DATABASES.iter().copied().enumerate() {
        match fetch(expected, cancelled) {
            Ok(database) => databases.push(database),
            Err(error) if error.cancelled => return (None, None),
            Err(error) => {
                crate::note(&format!(
                    "misterzine   Update All availability failed: {}",
                    error.diagnostic
                ));
                return availability_fallback(cached, cache_problem);
            }
        }
        send_progress(
            events,
            Phase::CheckingAvailability,
            index + 1,
            UPDATE_ALL_DATABASES.len(),
            false,
        );
    }
    let fresh = AvailabilityCache {
        format: AVAILABILITY_CACHE_FORMAT,
        databases,
    };
    if let Err(detail) = validate_availability_cache(&fresh) {
        crate::note(&format!(
            "misterzine   Update All availability rejected: {detail}"
        ));
        return availability_fallback(cached, cache_problem);
    }
    match save_availability_cache(&request.cache_dir, &fresh, cancelled) {
        Ok(()) => (Some(fresh), None),
        Err(SaveCacheError::Cancelled) => (None, None),
        Err(SaveCacheError::Failed(error)) => {
            crate::note(&format!(
                "misterzine   Update All availability could not be saved: {error}"
            ));
            (
                Some(fresh),
                Some("Update All availability could not be saved for offline use.".to_string()),
            )
        }
    }
}

fn availability_fallback(
    cached: Option<AvailabilityCache>,
    cache_problem: bool,
) -> (Option<AvailabilityCache>, Option<String>) {
    if let Some(cached) = cached {
        (
            Some(cached),
            Some(
                "Update All availability could not be refreshed. Showing saved availability."
                    .to_string(),
            ),
        )
    } else {
        let suffix = if cache_problem {
            " Saved availability data could not be read."
        } else {
            ""
        };
        (
            None,
            Some(format!(
                "Update All availability could not be checked. Showing installed content only.{suffix}"
            )),
        )
    }
}

fn finish_with_saved_or_error(
    cached: Option<Snapshot>,
    user: &str,
    diagnostic: &str,
    cache_problem: bool,
    events: &SyncSender<Event>,
) {
    crate::note(&format!("misterzine   update failed: {diagnostic}"));
    if let Some(snapshot) = cached {
        let date = display_date(&snapshot.updated).to_string();
        let _ = events.send(Event::Ready {
            snapshot,
            notice: Some(format!(
                "MiSTerZine could not be updated. Showing saved releases from {date}."
            )),
        });
    } else {
        let suffix = if cache_problem {
            " Saved MiSTerZine data could not be read."
        } else {
            ""
        };
        let _ = events.send(Event::Failed {
            message: format!("{}.{suffix}", user.trim_end_matches('.')),
        });
    }
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

pub fn cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("misterzine.bin")
}

pub fn load_cache(cache_dir: &Path) -> Result<Option<Cache>> {
    let path = cache_path(cache_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading MiSTerZine saved data",
                path,
                error,
            ))
        }
    };
    let cache: Cache = match postcard::from_bytes(&bytes) {
        Ok(cache) => cache,
        Err(current_error) => {
            let legacy: LegacyCache = postcard::from_bytes(&bytes).map_err(|legacy_error| {
                DegaussError::malformed(
                    "MiSTerZine saved data",
                    &path,
                    format!("current cache: {current_error}; previous cache: {legacy_error}"),
                )
            })?;
            if legacy.format != LEGACY_CACHE_FORMAT {
                return Err(DegaussError::malformed(
                    "MiSTerZine saved data",
                    &path,
                    format!("unsupported cache format {}", legacy.format),
                ));
            }
            Cache {
                format: LEGACY_CACHE_FORMAT,
                meta: legacy.meta,
                rows: legacy
                    .rows
                    .into_iter()
                    .map(|row| Release {
                        title: row.title,
                        base: row.base,
                        date: String::new(),
                        src: None,
                        beta: false,
                        deprecated: false,
                        manufacturer: row.manufacturer,
                        core: row.core,
                        updated: row.updated,
                        bd: Some(row.bd),
                        b: row.b,
                        k: row.k,
                        mra: Some(row.mra),
                    })
                    .collect(),
            }
        }
    };
    validate_cache(&cache)
        .map_err(|detail| DegaussError::malformed("MiSTerZine saved data", &path, detail))?;
    Ok(Some(cache))
}

fn availability_cache_path(cache_dir: &Path) -> PathBuf {
    cache_dir.join("misterzine-update-all.bin")
}

fn load_availability_cache(cache_dir: &Path) -> Result<Option<AvailabilityCache>> {
    let path = availability_cache_path(cache_dir);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading Update All availability",
                path,
                error,
            ));
        }
    };
    let cache: AvailabilityCache = postcard::from_bytes(&bytes).map_err(|error| {
        DegaussError::malformed("Update All availability", &path, error.to_string())
    })?;
    validate_availability_cache(&cache)
        .map_err(|detail| DegaussError::malformed("Update All availability", &path, detail))?;
    Ok(Some(cache))
}

fn validate_availability_cache(cache: &AvailabilityCache) -> std::result::Result<(), String> {
    if cache.format != AVAILABILITY_CACHE_FORMAT {
        return Err(format!(
            "unsupported availability cache format {}",
            cache.format
        ));
    }
    if cache.databases.len() != UPDATE_ALL_DATABASES.len() {
        return Err(format!(
            "expected {} Update All databases, found {}",
            UPDATE_ALL_DATABASES.len(),
            cache.databases.len()
        ));
    }
    let mut ids = BTreeSet::new();
    for database in &cache.databases {
        if !UPDATE_ALL_DATABASES
            .iter()
            .any(|expected| expected.id == database.id)
            || !ids.insert(database.id.as_str())
        {
            return Err(format!("unexpected or repeated database {}", database.id));
        }
        if database.timestamp == 0 {
            return Err(format!("database {} has no timestamp", database.id));
        }
        if database.cores.len().saturating_add(database.mras.len()) > MAX_UPDATE_ALL_FILES {
            return Err(format!("database {} exceeds the entry limit", database.id));
        }
        if !strictly_sorted(&database.cores)
            || !strictly_sorted(&database.mras)
            || database
                .cores
                .iter()
                .any(|value| value.is_empty() || value.len() > MAX_SHORT_TEXT)
            || database
                .mras
                .iter()
                .any(|value| normalize_manifest_mra(value).as_deref() != Some(value.as_str()))
        {
            return Err(format!("database {} has invalid identities", database.id));
        }
    }
    Ok(())
}

fn strictly_sorted(values: &[String]) -> bool {
    values.windows(2).all(|pair| pair[0] < pair[1])
}

fn decode_availability_database(
    expected: UpdateAllDatabase,
    body: &[u8],
) -> std::result::Result<AvailabilityDatabase, String> {
    let manifest: UpdateAllManifest =
        serde_json::from_slice(body).map_err(|error| error.to_string())?;
    if manifest.db_id != expected.id {
        return Err(format!(
            "expected database {}, received {}",
            expected.id, manifest.db_id
        ));
    }
    if manifest.timestamp == 0 {
        return Err(format!("database {} has no timestamp", expected.id));
    }
    if manifest.files.len() > MAX_UPDATE_ALL_FILES {
        return Err(format!("database {} exceeds the file limit", expected.id));
    }
    let mut cores = BTreeSet::new();
    let mut mras = BTreeSet::new();
    for path in manifest.files.keys() {
        if path.len() > MAX_PATH_TEXT || path.chars().any(char::is_control) {
            continue;
        }
        let parsed = Path::new(path);
        if parsed
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        {
            continue;
        }
        if parsed
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
        {
            if let Some(stem) = parsed.file_stem().and_then(|stem| stem.to_str()) {
                let identity = crate::systems::core_name(stem);
                if !identity.is_empty() {
                    cores.insert(identity);
                }
            }
        } else if let Some(mra) = normalize_manifest_mra(path) {
            mras.insert(mra);
        }
    }
    Ok(AvailabilityDatabase {
        id: manifest.db_id,
        timestamp: manifest.timestamp,
        cores: cores.into_iter().collect(),
        mras: mras.into_iter().collect(),
    })
}

#[derive(Debug)]
enum SaveCacheError {
    Cancelled,
    Failed(DegaussError),
}

fn save_cache(
    cache_dir: &Path,
    cache: &Cache,
    cancelled: &AtomicBool,
) -> std::result::Result<(), SaveCacheError> {
    if cache.format != CACHE_FORMAT {
        return Err(SaveCacheError::Failed(DegaussError::unsupported(
            "MiSTerZine saved data",
            format!("cannot save cache format {}", cache.format),
        )));
    }
    validate_cache(cache)
        .map_err(|detail| DegaussError::unsupported("MiSTerZine saved data", detail))
        .map_err(SaveCacheError::Failed)?;
    let path = cache_path(cache_dir);
    let bytes = postcard::to_stdvec(cache)
        .map_err(|error| DegaussError::unsupported("MiSTerZine saved data", error.to_string()))
        .map_err(SaveCacheError::Failed)?;
    if !path.exists() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SaveCacheError::Cancelled);
        }
        return crate::cache::write(&path, &bytes).map_err(SaveCacheError::Failed);
    }

    let parent = path.parent().expect("the MiSTerZine cache has a parent");
    std::fs::create_dir_all(parent)
        .map_err(|error| DegaussError::io("making the MiSTerZine cache folder", parent, error))
        .map_err(SaveCacheError::Failed)?;
    let temp = path.with_extension("part");
    std::fs::write(&temp, &bytes)
        .map_err(|error| DegaussError::io("writing MiSTerZine saved data", &temp, error))
        .map_err(SaveCacheError::Failed)?;
    if cancelled.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&temp);
        return Err(SaveCacheError::Cancelled);
    }
    match std::fs::rename(&temp, &path) {
        Ok(()) => Ok(()),
        Err(_) => replace_cache_directly(&path, &temp, &bytes),
    }
}

fn save_availability_cache(
    cache_dir: &Path,
    cache: &AvailabilityCache,
    cancelled: &AtomicBool,
) -> std::result::Result<(), SaveCacheError> {
    validate_availability_cache(cache)
        .map_err(|detail| DegaussError::unsupported("Update All availability", detail))
        .map_err(SaveCacheError::Failed)?;
    let path = availability_cache_path(cache_dir);
    let bytes = postcard::to_stdvec(cache)
        .map_err(|error| DegaussError::unsupported("Update All availability", error.to_string()))
        .map_err(SaveCacheError::Failed)?;
    if !path.exists() {
        if cancelled.load(Ordering::Relaxed) {
            return Err(SaveCacheError::Cancelled);
        }
        return crate::cache::write(&path, &bytes).map_err(SaveCacheError::Failed);
    }

    let parent = path
        .parent()
        .expect("the Update All availability cache has a parent");
    std::fs::create_dir_all(parent)
        .map_err(|error| DegaussError::io("making the availability cache folder", parent, error))
        .map_err(SaveCacheError::Failed)?;
    let temp = path.with_extension("part");
    std::fs::write(&temp, &bytes)
        .map_err(|error| DegaussError::io("writing Update All availability", &temp, error))
        .map_err(SaveCacheError::Failed)?;
    if cancelled.load(Ordering::Relaxed) {
        let _ = std::fs::remove_file(&temp);
        return Err(SaveCacheError::Cancelled);
    }
    match std::fs::rename(&temp, &path) {
        Ok(()) => Ok(()),
        Err(_) => replace_cache_directly(&path, &temp, &bytes),
    }
}

fn replace_cache_directly(
    path: &Path,
    temp: &Path,
    bytes: &[u8],
) -> std::result::Result<(), SaveCacheError> {
    let outcome = std::fs::write(path, bytes)
        .map_err(|error| DegaussError::io("replacing MiSTerZine saved data", path, error))
        .map_err(SaveCacheError::Failed);
    let _ = std::fs::remove_file(temp);
    outcome
}

fn decode_meta(body: &[u8]) -> std::result::Result<Meta, String> {
    let meta: Meta = serde_json::from_slice(body).map_err(|error| error.to_string())?;
    validate_meta(&meta)?;
    Ok(meta)
}

fn decode_data(meta: Meta, body: &[u8]) -> std::result::Result<Cache, String> {
    let digest = sha256_hex(body);
    if digest != meta.hash {
        return Err("the releases checksum did not match meta.json".to_string());
    }
    let rows: Vec<Release> = serde_json::from_slice(body).map_err(|error| error.to_string())?;
    let cache = Cache {
        format: CACHE_FORMAT,
        meta,
        rows,
    };
    validate_cache(&cache)?;
    Ok(cache)
}

fn sha256_hex(body: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(body);
    let mut output = String::with_capacity(64);
    for byte in digest {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn validate_meta(meta: &Meta) -> std::result::Result<(), String> {
    if meta.updated.is_empty() || meta.updated.len() > MAX_SHORT_TEXT {
        return Err("meta.json has no usable update date".to_string());
    }
    if meta.rows == 0 || meta.rows > MAX_ROWS {
        return Err(format!(
            "meta.json declares an invalid row count: {}",
            meta.rows
        ));
    }
    if meta.hash.len() != 64
        || !meta
            .hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("meta.json has no valid SHA-256 hash".to_string());
    }
    Ok(())
}

fn validate_cache(cache: &Cache) -> std::result::Result<(), String> {
    if !matches!(cache.format, CACHE_FORMAT | LEGACY_CACHE_FORMAT) {
        return Err(format!("unsupported cache format {}", cache.format));
    }
    validate_meta(&cache.meta)?;
    if cache.rows.len() != cache.meta.rows || cache.rows.len() > MAX_ROWS {
        return Err(format!(
            "row count {} does not match meta.json {}",
            cache.rows.len(),
            cache.meta.rows
        ));
    }
    let mut keys = BTreeSet::new();
    for (index, row) in cache.rows.iter().enumerate() {
        validate_release(row).map_err(|detail| format!("row {}: {detail}", index + 1))?;
        if !keys.insert(row.k.as_str()) {
            return Err(format!("row {}: repeated k", index + 1));
        }
    }
    Ok(())
}

fn validate_release(row: &Release) -> std::result::Result<(), String> {
    for (name, value) in [
        ("title", row.title.as_str()),
        ("base", row.base.as_str()),
        ("updated", row.updated.as_str()),
        ("k", row.k.as_str()),
    ] {
        if value.is_empty() || value.len() > MAX_SHORT_TEXT {
            return Err(format!("missing or overlong {name}"));
        }
    }
    for (name, value) in [
        ("manufacturer", row.manufacturer.as_str()),
        ("core", row.core.as_str()),
        ("date", row.date.as_str()),
        ("src", row.src.as_deref().unwrap_or("")),
        ("bd", row.bd.as_deref().unwrap_or("")),
    ] {
        if value.len() > MAX_SHORT_TEXT {
            return Err(format!("overlong {name}"));
        }
    }
    if row.base == "Arcade" {
        let mra = row.mra.as_deref().unwrap_or("");
        if mra.is_empty() || mra.len() > MAX_PATH_TEXT || !safe_relative_mra(mra) {
            return Err("missing or invalid mra path".to_string());
        }
    } else if row.core.is_empty() {
        return Err("non-Arcade release has no core identity".to_string());
    }
    Ok(())
}

fn safe_relative_mra(value: &str) -> bool {
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("mra"))
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
fn match_cache(
    cache: &Cache,
    request: &Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
) -> Option<Snapshot> {
    match_cache_with_availability(cache, request, None, events, cancelled)
}

fn match_cache_with_availability(
    cache: &Cache,
    request: &Request,
    availability: Option<&AvailabilityCache>,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
) -> Option<Snapshot> {
    let arcade = arcade_matches(request);
    let arcade_cores = arcade_core_matches(&request.menu_root);
    let cores = core_matches(&request.cores);
    let mut items = Vec::with_capacity(cache.rows.len());
    for (index, row) in cache.rows.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let item = if row.base == "Arcade" {
            let mra = row.mra.as_deref().expect("validated Arcade MRA");
            let path = request.menu_root.join(mra);
            let cover = arcade.get(mra).and_then(|(_, cover)| cover.clone());
            if !path.is_file() {
                not_installed(row)
            } else {
                let identity = crate::systems::core_name(&row.core);
                match arcade_cores.get(&identity) {
                    Some(_) => Item {
                        key: row.k.clone(),
                        title: row.title.clone(),
                        base: row.base.clone(),
                        date: row.date.clone(),
                        source: row.src.clone().unwrap_or_default(),
                        beta: row.beta,
                        deprecated: row.deprecated,
                        updated: row.updated.clone(),
                        batch: row.b,
                        manufacturer: row.manufacturer.clone(),
                        core: row.core.clone(),
                        state: LocalState::VersionUnknown,
                        launch_path: Some(path),
                        cover,
                        logo_id: None,
                    },
                    None => Item {
                        key: row.k.clone(),
                        title: row.title.clone(),
                        base: row.base.clone(),
                        date: row.date.clone(),
                        source: row.src.clone().unwrap_or_default(),
                        beta: row.beta,
                        deprecated: row.deprecated,
                        updated: row.updated.clone(),
                        batch: row.b,
                        manufacturer: row.manufacturer.clone(),
                        core: row.core.clone(),
                        state: LocalState::CoreMissing,
                        launch_path: None,
                        cover,
                        logo_id: None,
                    },
                }
            }
        } else {
            let identity = crate::systems::core_name(&row.core);
            match cores.get(&identity) {
                Some(core) => Item {
                    key: row.k.clone(),
                    title: row.title.clone(),
                    base: row.base.clone(),
                    date: row.date.clone(),
                    source: row.src.clone().unwrap_or_default(),
                    beta: row.beta,
                    deprecated: row.deprecated,
                    updated: row.updated.clone(),
                    batch: row.b,
                    manufacturer: row.manufacturer.clone(),
                    core: row.core.clone(),
                    state: core_state(core, row.bd.as_deref().unwrap_or("")),
                    launch_path: Some(core.path.clone()),
                    cover: None,
                    logo_id: core.logo_id.clone(),
                },
                None => not_installed(row),
            }
        };
        if item.installed()
            || (request.include_available
                && availability.is_some_and(|available| available.contains(row)))
        {
            let mut item = item;
            if !item.installed() {
                item.state = LocalState::AvailableThroughUpdateAll;
            }
            items.push(item);
        }
        if index % 64 == 0 || index + 1 == cache.rows.len() {
            send_progress(events, Phase::Matching, index + 1, cache.rows.len(), false);
        }
    }
    items.sort_by(|left, right| {
        right
            .updated
            .cmp(&left.updated)
            .then_with(|| right.batch.cmp(&left.batch))
            .then_with(|| {
                left.core
                    .to_ascii_lowercase()
                    .cmp(&right.core.to_ascii_lowercase())
            })
            .then_with(|| {
                left.title
                    .to_ascii_lowercase()
                    .cmp(&right.title.to_ascii_lowercase())
            })
    });
    Some(Snapshot {
        updated: cache.meta.updated.clone(),
        items,
    })
}

fn not_installed(row: &Release) -> Item {
    Item {
        key: row.k.clone(),
        title: row.title.clone(),
        base: row.base.clone(),
        date: row.date.clone(),
        source: row.src.clone().unwrap_or_default(),
        beta: row.beta,
        deprecated: row.deprecated,
        updated: row.updated.clone(),
        batch: row.b,
        manufacturer: row.manufacturer.clone(),
        core: row.core.clone(),
        state: if row.base == "Arcade" {
            LocalState::MraNotInstalled
        } else {
            LocalState::NotInstalled
        },
        launch_path: None,
        cover: None,
        logo_id: None,
    }
}

fn arcade_matches(request: &Request) -> BTreeMap<String, (PathBuf, Option<PathBuf>)> {
    let mut matches = BTreeMap::new();
    for system in &request.arcade_systems {
        let cache = if system.artwork_pack {
            crate::cache::load_artwork_pack_system(&request.cache_dir, &system.id)
        } else {
            crate::cache::load_system(&request.cache_dir, &system.id)
        }
        .or_else(|| crate::cache::load_system(&request.cache_dir, &system.id));
        let Some(cache) = cache else {
            continue;
        };
        for folder in cache.folders.values() {
            for row in &folder.rows {
                let Kind::Play(Launch::File(path)) = &row.kind else {
                    continue;
                };
                let Ok(relative) = path.strip_prefix(&request.menu_root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                matches
                    .entry(relative)
                    .or_insert_with(|| (path.clone(), row.cover.clone()));
            }
        }
    }
    matches
}

/// Arcade RBFs are runtime support files rather than direct menu launchers,
/// so the ordinary Cores catalogue deliberately excludes them. MiSTerZine
/// needs only this one shallow directory to distinguish a launchable MRA from
/// an MRA whose required core is absent.
fn arcade_core_matches(menu_root: &Path) -> BTreeMap<String, PathBuf> {
    let root = menu_root.join("_Arcade/cores");
    let listing = match std::fs::read_dir(&root) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return BTreeMap::new(),
        Err(error) => {
            crate::note(&format!(
                "misterzine   could not read Arcade cores at {}: {error}",
                root.display()
            ));
            return BTreeMap::new();
        }
    };
    let mut matches = BTreeMap::new();
    for entry in listing {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                crate::note(&format!(
                    "misterzine   could not read an Arcade core entry: {error}"
                ));
                continue;
            }
        };
        let path = entry.path();
        if !path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
            || !path.is_file()
        {
            continue;
        }
        let identity = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(crate::systems::core_name)
            .unwrap_or_default();
        if !identity.is_empty() {
            matches.entry(identity).or_insert(path);
        }
    }
    matches
}

fn core_matches(catalogue: &CoreCatalogue) -> BTreeMap<String, crate::systems::CoreEntry> {
    let mut matches = BTreeMap::new();
    for entry in &catalogue.entries {
        if !entry
            .path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
        {
            continue;
        }
        let identity = entry
            .path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(|stem| crate::systems::core_name(stem.trim_start_matches("RA_")))
            .or_else(|| entry.logo_id.as_deref().map(crate::systems::core_name))
            .unwrap_or_default();
        if !identity.is_empty() {
            // CoreCatalogue order is Standard, RA, then Unstable for one
            // identity, so the first launch target is deterministic.
            matches.entry(identity).or_insert_with(|| entry.clone());
        }
    }
    matches
}

fn core_state(core: &crate::systems::CoreEntry, remote_build: &str) -> LocalState {
    if !matches!(core.variant, CoreVariant::Standard) {
        return LocalState::VersionUnknown;
    }
    let Some(local) = core
        .path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .and_then(build_date)
    else {
        return LocalState::VersionUnknown;
    };
    if !valid_iso_date(remote_build) {
        return LocalState::VersionUnknown;
    }
    if local.as_str() < remote_build {
        LocalState::UpdateAvailable
    } else {
        LocalState::Current
    }
}

fn build_date(stem: &str) -> Option<String> {
    let (_, suffix) = stem.rsplit_once('_')?;
    let date = suffix.get(..8)?;
    date.bytes()
        .all(|byte| byte.is_ascii_digit())
        .then(|| format!("{}-{}-{}", &date[..4], &date[4..6], &date[6..8]))
}

fn valid_iso_date(value: &str) -> bool {
    value.len() == 10
        && value.as_bytes()[4] == b'-'
        && value.as_bytes()[7] == b'-'
        && value
            .bytes()
            .enumerate()
            .all(|(index, byte)| matches!(index, 4 | 7) || byte.is_ascii_digit())
}

fn display_date(updated: &str) -> &str {
    updated.get(..10).unwrap_or(updated)
}

#[derive(Debug)]
struct FetchError {
    user: String,
    diagnostic: String,
    cancelled: bool,
}

fn fetch_json(
    url: &str,
    limit: u64,
    timeout: Duration,
    cancelled: &AtomicBool,
) -> std::result::Result<Vec<u8>, FetchError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(FetchError {
            user: String::new(),
            diagnostic: "cancelled before request".to_string(),
            cancelled: true,
        });
    }
    let response = ResponseFile::create().map_err(|diagnostic| FetchError {
        user: "MiSTerZine could not be opened. Try again.".to_string(),
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
                "MiSTer cannot contact MiSTerZine because curl is missing.".to_string()
            } else {
                "MiSTerZine could not be opened. Try again.".to_string()
            },
            diagnostic: format!("curl could not start: {error}"),
            cancelled: false,
        })?;
    let write = child
        .stdin
        .take()
        .ok_or_else(|| FetchError {
            user: "MiSTerZine could not be opened. Try again.".to_string(),
            diagnostic: "curl stdin was unavailable".to_string(),
            cancelled: false,
        })
        .and_then(|mut stdin| {
            stdin
                .write_all(config.as_bytes())
                .map_err(|error| FetchError {
                    user: "MiSTerZine could not be opened. Try again.".to_string(),
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
                diagnostic: "request cancelled".to_string(),
                cancelled: true,
            });
        }
        if started.elapsed() > timeout + Duration::from_secs(2) {
            stop_child(&mut child);
            return Err(FetchError {
                user: "MiSTerZine did not respond in time. Try again.".to_string(),
                diagnostic: format!(
                    "application watchdog expired after {}s",
                    started.elapsed().as_secs()
                ),
                cancelled: false,
            });
        }
        if response.len().is_some_and(|size| size > limit) {
            stop_child(&mut child);
            return Err(FetchError {
                user: "MiSTerZine returned more data than Degauss can safely read.".to_string(),
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
                    user: "MiSTerZine could not be opened. Try again.".to_string(),
                    diagnostic: format!("curl process monitoring failed: {error}"),
                    cancelled: false,
                });
            }
        }
    };
    if !status.success() {
        let code = status.code();
        let (user, detail) = match code {
            Some(5..=7) => (
                "Could not reach MiSTerZine. Check the network connection.",
                "name resolution or connection failed",
            ),
            Some(28) => (
                "MiSTerZine did not respond in time. Try again.",
                "request timed out",
            ),
            Some(35 | 51 | 60) => (
                "Secure connection to MiSTerZine failed. Check MiSTer's date and network.",
                "TLS validation failed",
            ),
            Some(63) => (
                "MiSTerZine returned more data than Degauss can safely read.",
                "response exceeded the configured limit",
            ),
            Some(22) => (
                "MiSTerZine is unavailable. Try again later.",
                "server returned an HTTP error",
            ),
            _ => (
                "MiSTerZine could not be opened. Try again.",
                "unclassified curl failure",
            ),
        };
        return Err(FetchError {
            user: user.to_string(),
            diagnostic: format!("curl exit {:?}: {detail}", code),
            cancelled: false,
        });
    }
    response.read(limit).map_err(|diagnostic| FetchError {
        user: "MiSTerZine returned data Degauss could not read.".to_string(),
        diagnostic,
        cancelled: false,
    })
}

fn fetch_availability_database(
    expected: UpdateAllDatabase,
    cancelled: &AtomicBool,
) -> std::result::Result<AvailabilityDatabase, FetchError> {
    let archive_bytes = fetch_json(
        expected.url,
        MAX_UPDATE_ALL_ARCHIVE_BYTES,
        Duration::from_secs(12),
        cancelled,
    )
    .map_err(|mut error| {
        if !error.cancelled {
            error.user = "Update All availability could not be checked.".to_string();
        }
        error.diagnostic = format!("{}: {}", expected.id, error.diagnostic);
        error
    })?;
    let archive = ResponseFile::create_with_extension("zip").map_err(|diagnostic| FetchError {
        user: "Update All availability could not be checked.".to_string(),
        diagnostic: format!("{}: {diagnostic}", expected.id),
        cancelled: false,
    })?;
    archive
        .write(&archive_bytes)
        .map_err(|diagnostic| FetchError {
            user: "Update All availability could not be checked.".to_string(),
            diagnostic: format!("{}: {diagnostic}", expected.id),
            cancelled: false,
        })?;
    let body = extract_update_all_json(expected, &archive, cancelled)?;
    decode_availability_database(expected, &body).map_err(|diagnostic| FetchError {
        user: "Update All returned data Degauss could not read.".to_string(),
        diagnostic: format!("{}: {diagnostic}", expected.id),
        cancelled: false,
    })
}

fn extract_update_all_json(
    expected: UpdateAllDatabase,
    archive: &ResponseFile,
    cancelled: &AtomicBool,
) -> std::result::Result<Vec<u8>, FetchError> {
    if cancelled.load(Ordering::Relaxed) {
        return Err(FetchError {
            user: String::new(),
            diagnostic: format!("{}: cancelled before extraction", expected.id),
            cancelled: true,
        });
    }
    let entries = crate::zip::entries(archive.path()).map_err(|error| FetchError {
        user: "Update All returned data Degauss could not read.".to_string(),
        diagnostic: format!("{}: {error}", expected.id),
        cancelled: false,
    })?;
    let [entry] = entries.as_slice() else {
        return Err(FetchError {
            user: "Update All returned data Degauss could not read.".to_string(),
            diagnostic: format!(
                "{}: expected one manifest member, found {}",
                expected.id,
                entries.len()
            ),
            cancelled: false,
        });
    };
    if !entry.name.to_ascii_lowercase().ends_with(".json") || entry.size > MAX_UPDATE_ALL_JSON_BYTES
    {
        return Err(FetchError {
            user: "Update All returned data Degauss could not read.".to_string(),
            diagnostic: format!(
                "{}: invalid manifest member {:?} ({} bytes)",
                expected.id, entry.name, entry.size
            ),
            cancelled: false,
        });
    }

    let extracted = ResponseFile::create().map_err(|diagnostic| FetchError {
        user: "Update All returned data Degauss could not read.".to_string(),
        diagnostic: format!("{}: {diagnostic}", expected.id),
        cancelled: false,
    })?;
    let output = OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(extracted.path())
        .map_err(|error| FetchError {
            user: "Update All returned data Degauss could not read.".to_string(),
            diagnostic: format!("{}: extraction output failed: {error}", expected.id),
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
                "MiSTer cannot check Update All because unzip is missing.".to_string()
            } else {
                "Update All returned data Degauss could not read.".to_string()
            },
            diagnostic: format!("{}: unzip could not start: {error}", expected.id),
            cancelled: false,
        })?;
    let started = Instant::now();
    let status = loop {
        if cancelled.load(Ordering::Relaxed) {
            stop_child(&mut child);
            return Err(FetchError {
                user: String::new(),
                diagnostic: format!("{}: extraction cancelled", expected.id),
                cancelled: true,
            });
        }
        if started.elapsed() > Duration::from_secs(10) {
            stop_child(&mut child);
            return Err(FetchError {
                user: "Update All returned data Degauss could not read.".to_string(),
                diagnostic: format!("{}: unzip timed out", expected.id),
                cancelled: false,
            });
        }
        if extracted
            .len()
            .is_some_and(|size| size > MAX_UPDATE_ALL_JSON_BYTES)
        {
            stop_child(&mut child);
            return Err(FetchError {
                user: "Update All returned more data than Degauss can safely read.".to_string(),
                diagnostic: format!("{}: extracted manifest exceeded limit", expected.id),
                cancelled: false,
            });
        }
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => std::thread::sleep(CURL_POLL),
            Err(error) => {
                stop_child(&mut child);
                return Err(FetchError {
                    user: "Update All returned data Degauss could not read.".to_string(),
                    diagnostic: format!("{}: unzip monitoring failed: {error}", expected.id),
                    cancelled: false,
                });
            }
        }
    };
    if !status.success() {
        return Err(FetchError {
            user: "Update All returned data Degauss could not read.".to_string(),
            diagnostic: format!("{}: unzip exit {:?}", expected.id, status.code()),
            cancelled: false,
        });
    }
    let body = extracted
        .read(MAX_UPDATE_ALL_JSON_BYTES)
        .map_err(|diagnostic| FetchError {
            user: "Update All returned data Degauss could not read.".to_string(),
            diagnostic: format!("{}: {diagnostic}", expected.id),
            cancelled: false,
        })?;
    if body.len() as u64 != entry.size {
        return Err(FetchError {
            user: "Update All returned data Degauss could not read.".to_string(),
            diagnostic: format!(
                "{}: extracted {} bytes, expected {}",
                expected.id,
                body.len(),
                entry.size
            ),
            cancelled: false,
        });
    }
    Ok(body)
}

fn literal_unzip_member_pattern(name: &str) -> String {
    let mut pattern = String::with_capacity(name.len());
    for character in name.chars() {
        match character {
            '*' => pattern.push_str("[*]"),
            '?' => pattern.push_str("[?]"),
            '[' => pattern.push_str("[[]"),
            _ => pattern.push(character),
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
        "silent\nshow-error\nfail\ncompressed\nproto = \"=https\"\n{ca}connect-timeout = \"3\"\nmax-time = \"{}\"\nmax-filesize = \"{limit}\"\nuser-agent = \"Degauss/{}\"\noutput = \"{}\"\nurl = \"{}\"\n",
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
                ".degauss-misterzine-{}-{sequence}.{extension}",
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
        Err("temporary response file names were exhausted".to_string())
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn len(&self) -> Option<u64> {
        std::fs::metadata(&self.path).ok().map(|meta| meta.len())
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browse::{Details, Kind, Launch, Place, Row};
    use crate::cache::{Folder, SystemCache};
    use crate::systems::{CoreEntry, CoreVariant};

    fn temp_root(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "degauss-misterzine-{tag}-{}-{}",
            std::process::id(),
            TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ))
    }

    fn request(root: &Path) -> Request {
        Request {
            cache_dir: root.join("cache"),
            menu_root: root.join("menu"),
            cores: CoreCatalogue::default(),
            arcade_systems: Vec::new(),
            force_refresh: false,
            include_available: false,
        }
    }

    fn add_standard_core(request: &mut Request, stem: &str) -> PathBuf {
        let path = request
            .menu_root
            .join("_Console")
            .join(format!("{stem}_20260916.rbf"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"core").unwrap();
        request.cores.entries.push(CoreEntry {
            name: stem.to_string(),
            category: "Console".to_string(),
            variant: CoreVariant::Standard,
            path: path.clone(),
            logo_id: Some(stem.to_string()),
        });
        path
    }

    fn empty_availability() -> AvailabilityCache {
        AvailabilityCache {
            format: AVAILABILITY_CACHE_FORMAT,
            databases: UPDATE_ALL_DATABASES
                .iter()
                .map(|database| AvailabilityDatabase {
                    id: database.id.to_string(),
                    timestamp: 1,
                    cores: Vec::new(),
                    mras: Vec::new(),
                })
                .collect(),
        }
    }

    fn events() -> (SyncSender<Event>, Receiver<Event>) {
        mpsc::sync_channel(32)
    }

    fn release(base: &str) -> Release {
        Release {
            title: "Example".to_string(),
            base: base.to_string(),
            date: "2026-01-01".to_string(),
            src: Some("distribution_mister".to_string()),
            beta: false,
            deprecated: false,
            manufacturer: "Example Co".to_string(),
            core: if base == "Arcade" {
                "example".to_string()
            } else {
                "Example-Core".to_string()
            },
            updated: "2026-09-12".to_string(),
            bd: Some("2026-09-12".to_string()),
            b: 1,
            k: "example".to_string(),
            mra: Some(if base == "Arcade" {
                "_Arcade/Example.mra".to_string()
            } else {
                String::new()
            }),
        }
    }

    fn meta(rows: usize, body: &[u8]) -> Meta {
        Meta {
            updated: "2026-09-16T12:48Z".to_string(),
            hash: sha256_hex(body),
            rows,
        }
    }

    #[test]
    fn unknown_json_fields_are_ignored_but_required_identity_is_enforced() {
        let body = br#"[{"title":"Example","base":"Console","updated":"2026-09-12","core":"Example","k":"example","future":{"anything":true}}]"#;
        let cache = decode_data(meta(1, body), body).unwrap();
        assert_eq!(cache.rows[0].title, "Example");

        let missing =
            br#"[{"title":"Example","base":"Console","updated":"2026-09-12","core":"Example"}]"#;
        assert!(decode_data(meta(1, missing), missing)
            .unwrap_err()
            .contains("missing or overlong k"));

        let duplicate = serde_json::to_vec(&vec![release("Console"), release("Computer")]).unwrap();
        assert!(decode_data(meta(2, &duplicate), &duplicate)
            .unwrap_err()
            .contains("repeated k"));
    }

    #[test]
    fn checksum_row_count_and_unsafe_arcade_paths_are_rejected() {
        let body = serde_json::to_vec(&vec![release("Arcade")]).unwrap();
        let mut wrong_hash = meta(1, &body);
        wrong_hash.hash = "0".repeat(64);
        assert!(decode_data(wrong_hash, &body)
            .unwrap_err()
            .contains("checksum"));
        assert!(decode_data(meta(2, &body), &body)
            .unwrap_err()
            .contains("row count"));

        let mut unsafe_row = release("Arcade");
        unsafe_row.mra = Some("../Example.mra".to_string());
        let unsafe_body = serde_json::to_vec(&vec![unsafe_row]).unwrap();
        assert!(decode_data(meta(1, &unsafe_body), &unsafe_body)
            .unwrap_err()
            .contains("invalid mra"));
    }

    #[test]
    fn update_all_manifest_keeps_only_core_and_arcade_availability() {
        let expected = UPDATE_ALL_DATABASES[0];
        let body = serde_json::to_vec(&serde_json::json!({
            "db_id": expected.id,
            "timestamp": 123,
            "files": {
                "_Console/NES_20260916.rbf": {"hash": "fixture"},
                "_Arcade/cores/JTCPS1_20260916.rbf": {"hash": "fixture"},
                "_Arcade/Street Fighter II.mra": {"hash": "fixture"},
                "docs/readme.txt": {"hash": "fixture"},
                "../outside.rbf": {"hash": "fixture"}
            }
        }))
        .unwrap();

        let database = decode_availability_database(expected, &body).unwrap();
        assert_eq!(database.id, expected.id);
        assert_eq!(database.timestamp, 123);
        assert_eq!(database.cores, ["jtcps1", "nes"]);
        assert_eq!(database.mras, ["_arcade/street fighter ii.mra"]);

        let wrong = serde_json::to_vec(&serde_json::json!({
            "db_id": "another_database",
            "timestamp": 123,
            "files": {}
        }))
        .unwrap();
        assert!(decode_availability_database(expected, &wrong)
            .unwrap_err()
            .contains("expected database"));
    }

    #[test]
    fn valid_update_all_availability_cache_round_trips() {
        let root = temp_root("availability-cache");
        let cache = empty_availability();
        save_availability_cache(&root, &cache, &AtomicBool::new(false)).unwrap();
        assert_eq!(load_availability_cache(&root).unwrap(), Some(cache));

        let mut invalid = empty_availability();
        invalid.databases.pop();
        assert!(save_availability_cache(&root, &invalid, &AtomicBool::new(false)).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn core_versions_are_only_claimed_from_dated_standard_rbf_names() {
        let standard = CoreEntry {
            name: "Example".into(),
            category: "Console".into(),
            variant: CoreVariant::Standard,
            path: PathBuf::from("/_Console/Example_20260901.rbf"),
            logo_id: Some("ExampleCore".into()),
        };
        assert_eq!(
            core_state(&standard, "2026-09-12"),
            LocalState::UpdateAvailable
        );
        assert_eq!(core_state(&standard, "2026-08-01"), LocalState::Current);

        let undated = CoreEntry {
            path: PathBuf::from("/_Console/Example.rbf"),
            ..standard.clone()
        };
        assert_eq!(
            core_state(&undated, "2026-09-12"),
            LocalState::VersionUnknown
        );
        let ra = CoreEntry {
            variant: CoreVariant::RetroAchievements,
            path: PathBuf::from("/_Console/RA_Example.mgl"),
            ..standard
        };
        assert_eq!(core_state(&ra, "2026-09-12"), LocalState::VersionUnknown);
    }

    #[test]
    fn exact_arcade_path_and_matching_core_are_both_required() {
        let root = temp_root("arcade-match");
        let mut request = request(&root);
        let mra = request.menu_root.join("_Arcade/Example.mra");
        std::fs::create_dir_all(mra.parent().unwrap()).unwrap();
        std::fs::write(&mra, b"fixture").unwrap();
        let place = Place::Dir(request.menu_root.join("_Arcade"));
        let mut folders = BTreeMap::new();
        folders.insert(
            place.key(),
            Folder {
                mtime: 0,
                rows: vec![Row {
                    name: "Example".into(),
                    sort_key: "example".into(),
                    kind: Kind::Play(Launch::File(mra.clone())),
                    cover: Some(root.join("example.png")),
                    genre: None,
                    favorite: false,
                    below: None,
                    details: Details::default(),
                }],
                games: 1,
            },
        );
        crate::cache::save_system(
            &request.cache_dir,
            "Arcade",
            &SystemCache { format: 1, folders },
        )
        .unwrap();
        request.arcade_systems.push(ArcadeSystem {
            id: "Arcade".into(),
            artwork_pack: false,
        });
        let core = request.menu_root.join("_Arcade/cores/example_20260912.rbf");
        std::fs::create_dir_all(core.parent().unwrap()).unwrap();
        std::fs::write(&core, b"fixture").unwrap();
        let body = serde_json::to_vec(&vec![release("Arcade")]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let (sender, _receiver) = events();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            snapshot.items[0].launch_path.as_deref(),
            Some(mra.as_path())
        );
        assert_eq!(snapshot.items[0].state, LocalState::VersionUnknown);

        let mut other = release("Arcade");
        other.mra = Some("_Arcade/Other.mra".into());
        let body = serde_json::to_vec(&vec![other]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert!(snapshot.items.is_empty(), "a missing MRA is not shown");

        std::fs::remove_file(&core).unwrap();
        let body = serde_json::to_vec(&vec![release("Arcade")]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert!(
            snapshot.items.is_empty(),
            "an MRA without its required core is not shown"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn core_matching_uses_the_real_launcher_identity_not_its_logo_id() {
        let root = temp_root("core-match");
        let mut request = request(&root);
        let core = request.menu_root.join("_Computer/Minimig_20260901.rbf");
        std::fs::create_dir_all(core.parent().unwrap()).unwrap();
        std::fs::write(&core, b"fixture").unwrap();
        request.cores.entries.push(CoreEntry {
            name: "Amiga".into(),
            category: "Computer".into(),
            variant: CoreVariant::Standard,
            path: core.clone(),
            logo_id: Some("Amiga".into()),
        });
        let mut row = release("Computer");
        row.core = "Minimig".into();
        row.bd = Some("2026-09-12".into());
        let body = serde_json::to_vec(&vec![row]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let (sender, _receiver) = events();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            snapshot.items[0].launch_path.as_deref(),
            Some(core.as_path())
        );
        assert_eq!(snapshot.items[0].state, LocalState::UpdateAvailable);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn non_arcade_matching_uses_only_rbf_launchers() {
        let root = temp_root("rbf-only-match");
        let mut request = request(&root);
        let ra = request.menu_root.join("_RA_Cores/NES.mgl");
        std::fs::create_dir_all(ra.parent().unwrap()).unwrap();
        std::fs::write(&ra, b"<mistergamedescription/>").unwrap();
        request.cores.entries.push(CoreEntry {
            name: "NES".into(),
            category: "Console".into(),
            variant: CoreVariant::RetroAchievements,
            path: ra,
            logo_id: Some("NES".into()),
        });
        let mut row = release("Console");
        row.core = "NES".into();
        let body = serde_json::to_vec(&vec![row.clone()]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let (sender, _receiver) = events();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert!(
            snapshot.items.is_empty(),
            "a non-launchable RA descriptor does not make the core installed"
        );

        let rbf = request.menu_root.join("_Console/NES_20260916.rbf");
        std::fs::create_dir_all(rbf.parent().unwrap()).unwrap();
        std::fs::write(&rbf, b"core").unwrap();
        request.cores.entries.push(CoreEntry {
            name: "NES".into(),
            category: "Console".into(),
            variant: CoreVariant::Standard,
            path: rbf.clone(),
            logo_id: Some("NES".into()),
        });
        let body = serde_json::to_vec(&vec![row]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert_eq!(
            snapshot.items[0].launch_path.as_deref(),
            Some(rbf.as_path())
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn only_launchable_items_are_returned_for_every_source() {
        let root = temp_root("installed-only-sources");
        let mut request = request(&root);
        let installed = add_standard_core(&mut request, "Example-Core");
        let mut present = release("Console");
        present.k = "installed".into();
        present.src = Some("future_source".into());
        let mut missing_known_source = release("Console");
        missing_known_source.k = "missing-known".into();
        missing_known_source.title = "Missing Known Source".into();
        missing_known_source.src = Some("coinop".into());
        missing_known_source.core = "Missing-Known".into();
        let mut missing_future_source = release("Computer");
        missing_future_source.k = "missing-future".into();
        missing_future_source.title = "Missing Future Source".into();
        missing_future_source.src = Some("future_source".into());
        missing_future_source.manufacturer = "Future Developer".into();
        missing_future_source.core = "Missing-Future".into();
        let rows = vec![present, missing_known_source, missing_future_source];
        let body = serde_json::to_vec(&rows).unwrap();
        let cache = decode_data(meta(rows.len(), &body), &body).unwrap();
        let (sender, _receiver) = events();

        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();

        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(snapshot.items[0].title(), "Example");
        assert_eq!(
            snapshot.items[0].launch_path.as_deref(),
            Some(installed.as_path())
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn dormant_update_all_matcher_can_include_only_supported_uninstalled_entries() {
        let root = temp_root("available-uninstalled");
        let mut request = request(&root);
        request.include_available = true;
        let installed = add_standard_core(&mut request, "Outside-Core");
        let arcade_mra = request
            .menu_root
            .join("_Arcade")
            .join("Street Fighter II.mra");
        std::fs::create_dir_all(arcade_mra.parent().unwrap()).unwrap();
        std::fs::write(&arcade_mra, b"fixture").unwrap();
        let arcade_cover = root.join("street-fighter-ii.png");
        let arcade_place = Place::Dir(request.menu_root.join("_Arcade"));
        let mut arcade_folders = BTreeMap::new();
        arcade_folders.insert(
            arcade_place.key(),
            Folder {
                mtime: 0,
                rows: vec![Row {
                    name: "Street Fighter II".into(),
                    sort_key: "street fighter ii".into(),
                    kind: Kind::Play(Launch::File(arcade_mra)),
                    cover: Some(arcade_cover.clone()),
                    genre: None,
                    favorite: false,
                    below: None,
                    details: Details::default(),
                }],
                games: 1,
            },
        );
        crate::cache::save_system(
            &request.cache_dir,
            "Arcade",
            &SystemCache {
                format: 1,
                folders: arcade_folders,
            },
        )
        .unwrap();
        request.arcade_systems.push(ArcadeSystem {
            id: "Arcade".into(),
            artwork_pack: false,
        });

        let mut installed_row = release("Console");
        installed_row.k = "installed".into();
        installed_row.title = "Installed Outside Update All".into();
        installed_row.core = "Outside-Core".into();
        let mut available_core = release("Console");
        available_core.k = "available-core".into();
        available_core.title = "Available Console".into();
        available_core.core = "NES".into();
        let mut unavailable_core = release("Computer");
        unavailable_core.k = "unavailable-core".into();
        unavailable_core.title = "Unavailable Computer".into();
        unavailable_core.core = "Missing-Core".into();
        let mut available_arcade = release("Arcade");
        available_arcade.k = "available-arcade".into();
        available_arcade.title = "Available Arcade".into();
        available_arcade.core = "JTCPS1".into();
        available_arcade.mra = Some("_Arcade/Street Fighter II.mra".into());
        let mut split_arcade = release("Arcade");
        split_arcade.k = "split-arcade".into();
        split_arcade.title = "Split Arcade".into();
        split_arcade.core = "Split-Core".into();
        split_arcade.mra = Some("_Arcade/Split.mra".into());

        let rows = vec![
            installed_row,
            available_core,
            unavailable_core,
            available_arcade,
            split_arcade,
        ];
        let body = serde_json::to_vec(&rows).unwrap();
        let cache = decode_data(meta(rows.len(), &body), &body).unwrap();
        let mut availability = empty_availability();
        availability.databases[0].cores = vec!["jtcps1".into(), "nes".into()];
        availability.databases[0].mras = vec!["_arcade/street fighter ii.mra".into()];
        availability.databases[1].cores = vec!["splitcore".into()];
        availability.databases[2].mras = vec!["_arcade/split.mra".into()];
        validate_availability_cache(&availability).unwrap();
        let (sender, _receiver) = events();

        let snapshot = match_cache_with_availability(
            &cache,
            &request,
            Some(&availability),
            &sender,
            &AtomicBool::new(false),
        )
        .unwrap();

        assert_eq!(snapshot.items.len(), 3);
        let installed_item = snapshot
            .items
            .iter()
            .find(|item| item.title() == "Installed Outside Update All")
            .unwrap();
        assert_eq!(
            installed_item.launch_path.as_deref(),
            Some(installed.as_path())
        );
        for title in ["Available Console", "Available Arcade"] {
            let item = snapshot
                .items
                .iter()
                .find(|item| item.title() == title)
                .unwrap();
            assert_eq!(item.state, LocalState::AvailableThroughUpdateAll);
            assert!(item.launch_path.is_none());
        }
        assert_eq!(
            snapshot
                .items
                .iter()
                .find(|item| item.title() == "Available Arcade")
                .and_then(Item::cover),
            Some(arcade_cover.as_path())
        );
        assert!(!snapshot
            .items
            .iter()
            .any(|item| item.title() == "Unavailable Computer" || item.title() == "Split Arcade"));

        request.include_available = false;
        let installed_only = match_cache_with_availability(
            &cache,
            &request,
            Some(&availability),
            &sender,
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(installed_only.items.len(), 1);
        assert_eq!(
            installed_only.items[0].title(),
            "Installed Outside Update All"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unzip_member_pattern_matches_validated_member_names_literally() {
        assert_eq!(
            literal_unzip_member_pattern("manifest*[draft]??.json"),
            "manifest[*][[]draft][?][?].json"
        );
    }

    #[test]
    fn availability_refresh_is_cached_and_failed_refresh_reuses_it() {
        let root = temp_root("availability-refresh");
        let mut request = request(&root);
        request.include_available = true;
        let mut row = release("Console");
        row.title = "Available Console".into();
        row.core = "NES".into();
        let body = serde_json::to_vec(&vec![row]).unwrap();
        let meta = meta(1, &body);
        save_cache(
            &request.cache_dir,
            &decode_data(meta.clone(), &body).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let meta_body = serde_json::to_vec(&meta).unwrap();
        let (sender, receiver) = events();
        run_with_fetches(
            request.clone(),
            &sender,
            &AtomicBool::new(false),
            |url, _, _, _| {
                assert_eq!(url, META_URL);
                Ok(meta_body.clone())
            },
            |expected, _| {
                let mut database = AvailabilityDatabase {
                    id: expected.id.to_string(),
                    timestamp: 1,
                    cores: Vec::new(),
                    mras: Vec::new(),
                };
                if expected.id == UPDATE_ALL_DATABASES[0].id {
                    database.cores.push("nes".into());
                }
                Ok(database)
            },
        );
        let ready = receiver.try_iter().find_map(|event| match event {
            Event::Ready { snapshot, notice } => Some((snapshot, notice)),
            _ => None,
        });
        let (snapshot, notice) = ready.expect("fresh availability completes");
        assert!(notice.is_none());
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(
            snapshot.items[0].state,
            LocalState::AvailableThroughUpdateAll
        );
        assert!(load_availability_cache(&request.cache_dir)
            .unwrap()
            .is_some());

        let (sender, receiver) = events();
        run_with_fetches(
            request,
            &sender,
            &AtomicBool::new(false),
            |url, _, _, _| {
                assert_eq!(url, META_URL);
                Ok(meta_body.clone())
            },
            |expected, _| {
                Err(FetchError {
                    user: "Update All availability could not be checked.".into(),
                    diagnostic: format!("{}: synthetic failure", expected.id),
                    cancelled: false,
                })
            },
        );
        let ready = receiver.try_iter().find_map(|event| match event {
            Event::Ready { snapshot, notice } => Some((snapshot, notice)),
            _ => None,
        });
        let (snapshot, notice) = ready.expect("cached availability remains usable");
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(
            notice.as_deref(),
            Some("Update All availability could not be refreshed. Showing saved availability.")
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn unchanged_metadata_skips_data_but_explicit_refresh_downloads_it() {
        let root = temp_root("refresh-decision");
        let request = request(&root);
        let body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let meta = meta(1, &body);
        save_cache(
            &request.cache_dir,
            &decode_data(meta.clone(), &body).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();

        let meta_body = serde_json::to_vec(&meta).unwrap();
        let mut calls = Vec::new();
        let (sender, receiver) = events();
        run_with_fetch(
            request.clone(),
            &sender,
            &AtomicBool::new(false),
            |url, _, _, _| {
                calls.push(url.to_string());
                assert_eq!(url, META_URL);
                Ok(meta_body.clone())
            },
        );
        assert_eq!(calls, vec![META_URL]);
        assert!(receiver
            .try_iter()
            .any(|event| matches!(event, Event::Ready { notice: None, .. })));

        let mut forced = request;
        forced.force_refresh = true;
        let mut calls = Vec::new();
        let (sender, receiver) = events();
        run_with_fetch(forced, &sender, &AtomicBool::new(false), |url, _, _, _| {
            calls.push(url.to_string());
            if url == META_URL {
                Ok(meta_body.clone())
            } else {
                Ok(body.clone())
            }
        });
        assert_eq!(calls, vec![META_URL, DATA_URL]);
        assert!(receiver
            .try_iter()
            .any(|event| matches!(event, Event::Ready { notice: None, .. })));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_refresh_keeps_a_valid_dated_cache_and_first_use_reports_failure() {
        let root = temp_root("offline-cache");
        let mut cached_request = request(&root);
        add_standard_core(&mut cached_request, "Example-Core");
        let body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        save_cache(
            &cached_request.cache_dir,
            &decode_data(meta(1, &body), &body).unwrap(),
            &AtomicBool::new(false),
        )
        .unwrap();
        let offline = || FetchError {
            user: "Could not reach MiSTerZine. Check the network connection.".into(),
            diagnostic: "synthetic connection failure".into(),
            cancelled: false,
        };
        let (sender, receiver) = events();
        run_with_fetch(
            cached_request,
            &sender,
            &AtomicBool::new(false),
            |_, _, _, _| Err(offline()),
        );
        let ready = receiver.try_iter().find_map(|event| match event {
            Event::Ready { snapshot, notice } => Some((snapshot, notice)),
            _ => None,
        });
        let (snapshot, notice) = ready.expect("saved releases remain usable");
        assert_eq!(snapshot.items.len(), 1);
        assert_eq!(
            notice.as_deref(),
            Some("MiSTerZine could not be updated. Showing saved releases from 2026-09-16.")
        );

        let empty = temp_root("offline-empty");
        let (sender, receiver) = events();
        run_with_fetch(
            request(&empty),
            &sender,
            &AtomicBool::new(false),
            |_, _, _, _| Err(offline()),
        );
        assert!(receiver.try_iter().any(|event| matches!(
            event,
            Event::Failed { message }
                if message == "Could not reach MiSTerZine. Check the network connection."
        )));

        let broken = temp_root("offline-broken-cache");
        let broken_request = request(&broken);
        std::fs::create_dir_all(&broken_request.cache_dir).unwrap();
        std::fs::write(cache_path(&broken_request.cache_dir), b"not a cache").unwrap();
        let (sender, receiver) = events();
        run_with_fetch(
            broken_request,
            &sender,
            &AtomicBool::new(false),
            |_, _, _, _| Err(offline()),
        );
        assert!(receiver.try_iter().any(|event| matches!(
            event,
            Event::Failed { message }
                if message == "Could not reach MiSTerZine. Check the network connection. Saved MiSTerZine data could not be read."
        )));
        std::fs::remove_dir_all(root).ok();
        std::fs::remove_dir_all(empty).ok();
        std::fs::remove_dir_all(broken).ok();
    }

    #[test]
    fn an_incompatible_changed_feed_keeps_the_last_valid_cache() {
        let root = temp_root("changed-format");
        let request = request(&root);
        let original_body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let original = decode_data(meta(1, &original_body), &original_body).unwrap();
        save_cache(&request.cache_dir, &original, &AtomicBool::new(false)).unwrap();

        let incompatible = br#"[{"future_identity":"not-a-supported-launch-id"}]"#.to_vec();
        let changed_meta = meta(1, &incompatible);
        let meta_body = serde_json::to_vec(&changed_meta).unwrap();
        let (sender, receiver) = events();
        run_with_fetch(
            request.clone(),
            &sender,
            &AtomicBool::new(false),
            |url, _, _, _| {
                if url == META_URL {
                    Ok(meta_body.clone())
                } else {
                    Ok(incompatible.clone())
                }
            },
        );
        assert!(receiver.try_iter().any(|event| matches!(
            event,
            Event::Ready {
                notice: Some(message),
                ..
            } if message.contains("saved releases from 2026-09-16")
        )));
        assert_eq!(load_cache(&request.cache_dir).unwrap(), Some(original));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancellation_stops_matching_without_replacing_the_saved_cache() {
        let root = temp_root("cancel");
        let request = request(&root);
        let body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        save_cache(&request.cache_dir, &cache, &AtomicBool::new(false)).unwrap();
        let cancelled = AtomicBool::new(true);
        let (sender, _receiver) = events();
        assert!(match_cache(&cache, &request, &sender, &cancelled).is_none());
        assert_eq!(load_cache(&request.cache_dir).unwrap(), Some(cache));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancellation_after_download_does_not_replace_the_saved_cache() {
        let root = temp_root("cancel-before-save");
        let mut request = request(&root);
        request.force_refresh = true;
        let original_body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let original = decode_data(meta(1, &original_body), &original_body).unwrap();
        save_cache(&request.cache_dir, &original, &AtomicBool::new(false)).unwrap();

        let mut changed = release("Console");
        changed.title = "Changed release".into();
        let changed_body = serde_json::to_vec(&vec![changed]).unwrap();
        let changed_meta = serde_json::to_vec(&meta(1, &changed_body)).unwrap();
        let cancelled = AtomicBool::new(false);
        let (sender, receiver) = events();
        run_with_fetch(request.clone(), &sender, &cancelled, |url, _, _, _| {
            if url == META_URL {
                Ok(changed_meta.clone())
            } else {
                cancelled.store(true, Ordering::Relaxed);
                Ok(changed_body.clone())
            }
        });
        assert!(receiver
            .try_iter()
            .any(|event| matches!(event, Event::Cancelled)));
        assert_eq!(load_cache(&request.cache_dir).unwrap(), Some(original));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cancellation_before_cache_rename_preserves_the_saved_cache() {
        let root = temp_root("cancel-before-cache-rename");
        let original_body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let original = decode_data(meta(1, &original_body), &original_body).unwrap();
        save_cache(&root, &original, &AtomicBool::new(false)).unwrap();

        let mut changed = release("Console");
        changed.title = "Changed release".into();
        let changed_body = serde_json::to_vec(&vec![changed]).unwrap();
        let changed = decode_data(meta(1, &changed_body), &changed_body).unwrap();
        assert!(matches!(
            save_cache(&root, &changed, &AtomicBool::new(true)),
            Err(SaveCacheError::Cancelled)
        ));
        assert_eq!(load_cache(&root).unwrap(), Some(original));
        assert!(!cache_path(&root).with_extension("part").exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn failed_cache_replacement_preserves_the_saved_cache() {
        let root = temp_root("cache-replacement");
        let original_body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let original = decode_data(meta(1, &original_body), &original_body).unwrap();
        save_cache(&root, &original, &AtomicBool::new(false)).unwrap();

        let temp = cache_path(&root).with_extension("part");
        std::fs::create_dir_all(&temp).unwrap();
        let mut changed = release("Console");
        changed.title = "Changed release".into();
        let changed_body = serde_json::to_vec(&vec![changed]).unwrap();
        let changed = decode_data(meta(1, &changed_body), &changed_body).unwrap();
        assert!(save_cache(&root, &changed, &AtomicBool::new(false)).is_err());
        assert_eq!(load_cache(&root).unwrap(), Some(original));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn refused_cache_rename_falls_back_to_a_direct_replacement() {
        let root = temp_root("cache-direct-replacement");
        let original_body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let original = decode_data(meta(1, &original_body), &original_body).unwrap();
        save_cache(&root, &original, &AtomicBool::new(false)).unwrap();

        let mut changed = release("Console");
        changed.title = "Changed release".into();
        let changed_body = serde_json::to_vec(&vec![changed]).unwrap();
        let changed = decode_data(meta(1, &changed_body), &changed_body).unwrap();
        let bytes = postcard::to_stdvec(&changed).unwrap();
        let path = cache_path(&root);
        let temp = path.with_extension("part");
        std::fs::write(&temp, &bytes).unwrap();

        replace_cache_directly(&path, &temp, &bytes).unwrap();

        assert_eq!(load_cache(&root).unwrap(), Some(changed));
        assert!(!temp.exists());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn separate_cache_round_trips_without_touching_other_cache_files() {
        let root = temp_root("separate-cache");
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(&root).unwrap();
        let unrelated = root.join("index.bin");
        std::fs::write(&unrelated, b"unchanged").unwrap();
        let body = serde_json::to_vec(&vec![release("Console")]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        save_cache(&root, &cache, &AtomicBool::new(false)).unwrap();
        assert_eq!(load_cache(&root).unwrap(), Some(cache));
        assert_eq!(std::fs::read(unrelated).unwrap(), b"unchanged");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn feed_metadata_is_retained_and_presented_with_accurate_labels() {
        let root = temp_root("feed-metadata");
        let mut request = request(&root);
        add_standard_core(&mut request, "Example-Core");
        let body = br#"[{"title":"Example","base":"Console","date":"2025-02-03","src":"future_source","beta":true,"deprecated":true,"manufacturer":"Example Co","core":"Example-Core","updated":"2026-09-12","bd":"2026-09-12","b":1,"k":"example","mra":""}]"#;
        let cache = decode_data(meta(1, body), body).unwrap();
        let (sender, _receiver) = events();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        let item = &snapshot.items[0];
        assert_eq!(item.display_title(), "Example [Beta] [Deprecated]");
        assert_eq!(item.source_label(), "Future Source");
        assert_eq!(
            item.information(),
            "Example [Beta] [Deprecated]\n\nLocal status: Installed · Current\n\nType: Console\n\nSource: Future Source\n\nRelease status: Beta · Deprecated\n\nCore: Example-Core\n\nManufacturer: Example Co\n\nMiSTer debut: 2025-02-03\n\nLatest shipped update: 2026-09-12"
        );
        let row = item.row(None);
        assert!(row.details.publisher.is_empty());
        assert!(row.details.developer.is_empty());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn nullable_feed_fields_remain_readable() {
        let root = temp_root("nullable-feed");
        let mut request = request(&root);
        add_standard_core(&mut request, "Example-Core");
        let body = br#"[{"title":"Unknown Source","base":"Other","date":"2025-02-03","src":null,"manufacturer":"","core":"Example-Core","updated":"2026-09-12","bd":null,"b":1,"k":"example","mra":null}]"#;
        let cache = decode_data(meta(1, body), body).unwrap();
        let (sender, _receiver) = events();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert_eq!(snapshot.items[0].source_label(), "Unknown");
        assert!(snapshot.items[0].information().contains("Source: Unknown"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn type_and_source_filters_combine_in_memory() {
        let items = vec![
            Item::fixture_with(
                "One",
                "Console",
                "distribution_mister",
                LocalState::Current,
                None,
            ),
            Item::fixture_with("Two", "Arcade", "coinop", LocalState::Current, None),
            Item::fixture_with("Three", "Console", "coinop", LocalState::Current, None),
        ];
        let mut filters = Filters::default();
        let console = choices(&items, FilterField::Type)
            .into_iter()
            .find(|choice| choice.label() == "Console")
            .unwrap();
        let coinop = choices(&items, FilterField::Source)
            .into_iter()
            .find(|choice| choice.label() == "Coin-Op")
            .unwrap();
        filters.choose(FilterField::Type, &console);
        filters.choose(FilterField::Source, &coinop);
        assert_eq!(
            items
                .iter()
                .filter(|item| filters.matches(item))
                .map(Item::title)
                .collect::<Vec<_>>(),
            ["Three"]
        );
        filters.clear();
        assert!(items.iter().all(|item| filters.matches(item)));
    }

    #[test]
    fn previous_cache_format_is_read_and_refreshed_to_the_current_format() {
        let root = temp_root("legacy-cache");
        let request = request(&root);
        let row = release("Console");
        let body = serde_json::to_vec(&vec![row.clone()]).unwrap();
        let current_meta = meta(1, &body);
        let legacy = LegacyCache {
            format: LEGACY_CACHE_FORMAT,
            meta: current_meta.clone(),
            rows: vec![LegacyRelease {
                title: row.title,
                base: row.base,
                manufacturer: row.manufacturer,
                core: row.core,
                updated: row.updated,
                bd: row.bd.unwrap(),
                b: row.b,
                k: row.k,
                mra: row.mra.unwrap(),
            }],
        };
        std::fs::create_dir_all(&request.cache_dir).unwrap();
        std::fs::write(
            cache_path(&request.cache_dir),
            postcard::to_stdvec(&legacy).unwrap(),
        )
        .unwrap();
        let loaded = load_cache(&request.cache_dir).unwrap().unwrap();
        assert_eq!(loaded.format, LEGACY_CACHE_FORMAT);
        assert!(loaded.rows[0].date.is_empty());

        let meta_body = serde_json::to_vec(&current_meta).unwrap();
        let mut calls = Vec::new();
        let (sender, receiver) = events();
        run_with_fetch(
            request.clone(),
            &sender,
            &AtomicBool::new(false),
            |url, _, _, _| {
                calls.push(url.to_string());
                if url == META_URL {
                    Ok(meta_body.clone())
                } else {
                    Ok(body.clone())
                }
            },
        );
        assert_eq!(calls, [META_URL, DATA_URL]);
        assert!(receiver
            .try_iter()
            .any(|event| matches!(event, Event::Ready { notice: None, .. })));
        assert_eq!(
            load_cache(&request.cache_dir).unwrap().unwrap().format,
            CACHE_FORMAT
        );
        std::fs::remove_dir_all(root).ok();
    }
}
