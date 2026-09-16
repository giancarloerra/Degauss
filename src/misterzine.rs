//! Optional MiSTerZine release browser.
//!
//! The official feed is kept in one independent cache. A single cancellable
//! worker reads that cache, checks the fixed HTTPS endpoint, downloads only
//! when needed, and matches releases against Degauss's existing catalogues.
//! It never scans the card and never changes a game or core index.

use std::collections::BTreeMap;
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
const CACHE_FORMAT: u32 = 1;
const MAX_META_BYTES: u64 = 16 * 1024;
const MAX_DATA_BYTES: u64 = 2 * 1024 * 1024;
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
    manufacturer: String,
    #[serde(default)]
    core: String,
    #[serde(default)]
    updated: String,
    #[serde(default)]
    bd: String,
    #[serde(default)]
    b: i64,
    #[serde(default)]
    k: String,
    #[serde(default)]
    mra: String,
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Cache {
    format: u32,
    pub meta: Meta,
    rows: Vec<Release>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LocalState {
    NotInstalled,
    VersionUnknown,
    Current,
    UpdateAvailable,
}

impl LocalState {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotInstalled => "Not installed",
            Self::VersionUnknown => "Installed · Version unknown",
            Self::Current => "Installed · Current",
            Self::UpdateAvailable => "Installed · Update available",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    title: String,
    base: String,
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
    pub fn installed(&self) -> bool {
        self.state != LocalState::NotInstalled
    }

    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn summary(&self) -> String {
        format!("{} · {} · {}", self.base, self.updated, self.state.label())
    }

    pub fn cover(&self) -> Option<&Path> {
        self.cover.as_deref()
    }

    pub fn logo_id(&self) -> Option<&str> {
        self.logo_id.as_deref()
    }

    pub fn row(&self, cover: Option<PathBuf>) -> Row {
        Row {
            name: self.title.clone(),
            sort_key: self.title.to_ascii_lowercase(),
            // An unavailable release is intercepted before launch. Keeping
            // it a non-folder lets every existing layout present it like the
            // neighbouring installed releases.
            kind: Kind::Play(Launch::File(self.launch_path.clone().unwrap_or_default())),
            cover,
            genre: Some(self.base.clone()),
            favorite: false,
            below: None,
            details: Details {
                desc: self.state.label().to_string(),
                publisher: self.manufacturer.clone(),
                developer: self.core.clone(),
                released: self.updated.clone(),
                players: String::new(),
                lang: String::new(),
            },
        }
    }

    #[cfg(test)]
    pub(crate) fn fixture(title: &str, state: LocalState, launch_path: Option<PathBuf>) -> Self {
        Self {
            title: title.to_string(),
            base: "Console".to_string(),
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Checking,
    Downloading,
    Matching,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking MiSTerZine",
            Self::Downloading => "Downloading Releases",
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
    run_with_fetch(request, events, cancelled, fetch_json);
}

fn run_with_fetch<F>(
    request: Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
    mut fetch: F,
) where
    F: FnMut(&str, u64, Duration, &AtomicBool) -> std::result::Result<Vec<u8>, FetchError>,
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
    let mut cached_snapshot = None;
    if let Some(cache) = cached.as_ref() {
        send_progress(events, Phase::Matching, 0, cache.rows.len(), false);
        match match_cache(cache, &request, events, cancelled) {
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
            .is_some_and(|cache| cache.meta.hash == meta.hash)
    {
        let snapshot = cached_snapshot.expect("a matching cache produced a snapshot");
        let _ = events.send(Event::Ready {
            snapshot,
            notice: None,
        });
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
    send_progress(events, Phase::Matching, 0, cache.rows.len(), false);
    match match_cache(cache, &request, events, cancelled) {
        Some(snapshot) => {
            let _ = events.send(Event::Ready {
                snapshot,
                notice: None,
            });
        }
        None => {
            let _ = events.send(Event::Cancelled);
        }
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
    let cache: Cache = postcard::from_bytes(&bytes).map_err(|error| {
        DegaussError::malformed("MiSTerZine saved data", &path, error.to_string())
    })?;
    validate_cache(&cache)
        .map_err(|detail| DegaussError::malformed("MiSTerZine saved data", &path, detail))?;
    Ok(Some(cache))
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
        Err(error) => {
            let _ = std::fs::remove_file(&temp);
            Err(SaveCacheError::Failed(DegaussError::io(
                "replacing MiSTerZine saved data",
                &path,
                error,
            )))
        }
    }
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
    if cache.format != CACHE_FORMAT {
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
    for (index, row) in cache.rows.iter().enumerate() {
        validate_release(row).map_err(|detail| format!("row {}: {detail}", index + 1))?;
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
        ("bd", row.bd.as_str()),
    ] {
        if value.len() > MAX_SHORT_TEXT {
            return Err(format!("overlong {name}"));
        }
    }
    if row.base == "Arcade" {
        if row.mra.is_empty() || row.mra.len() > MAX_PATH_TEXT || !safe_relative_mra(&row.mra) {
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

fn match_cache(
    cache: &Cache,
    request: &Request,
    events: &SyncSender<Event>,
    cancelled: &AtomicBool,
) -> Option<Snapshot> {
    let arcade = arcade_matches(request);
    let cores = core_matches(&request.cores);
    let mut items = Vec::with_capacity(cache.rows.len());
    for (index, row) in cache.rows.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let item = if row.base == "Arcade" {
            match arcade.get(&row.mra) {
                Some((path, cover)) => Item {
                    title: row.title.clone(),
                    base: row.base.clone(),
                    updated: row.updated.clone(),
                    batch: row.b,
                    manufacturer: row.manufacturer.clone(),
                    core: row.core.clone(),
                    state: LocalState::VersionUnknown,
                    launch_path: Some(path.clone()),
                    cover: cover.clone(),
                    logo_id: None,
                },
                None => not_installed(row),
            }
        } else {
            let identity = crate::systems::core_name(&row.core);
            match cores.get(&identity) {
                Some(core) => Item {
                    title: row.title.clone(),
                    base: row.base.clone(),
                    updated: row.updated.clone(),
                    batch: row.b,
                    manufacturer: row.manufacturer.clone(),
                    core: row.core.clone(),
                    state: core_state(core, &row.bd),
                    launch_path: Some(core.path.clone()),
                    cover: None,
                    logo_id: core.logo_id.clone(),
                },
                None => not_installed(row),
            }
        };
        items.push(item);
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
        title: row.title.clone(),
        base: row.base.clone(),
        updated: row.updated.clone(),
        batch: row.b,
        manufacturer: row.manufacturer.clone(),
        core: row.core.clone(),
        state: LocalState::NotInstalled,
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
        for _ in 0..100 {
            let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                ".degauss-misterzine-{}-{sequence}.part",
                std::process::id()
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
        }
    }

    fn events() -> (SyncSender<Event>, Receiver<Event>) {
        mpsc::sync_channel(32)
    }

    fn release(base: &str) -> Release {
        Release {
            title: "Example".to_string(),
            base: base.to_string(),
            manufacturer: "Example Co".to_string(),
            core: if base == "Arcade" {
                "example".to_string()
            } else {
                "Example-Core".to_string()
            },
            updated: "2026-09-12".to_string(),
            bd: "2026-09-12".to_string(),
            b: 1,
            k: "example".to_string(),
            mra: if base == "Arcade" {
                "_Arcade/Example.mra".to_string()
            } else {
                String::new()
            },
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
        unsafe_row.mra = "../Example.mra".to_string();
        let unsafe_body = serde_json::to_vec(&vec![unsafe_row]).unwrap();
        assert!(decode_data(meta(1, &unsafe_body), &unsafe_body)
            .unwrap_err()
            .contains("invalid mra"));
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
    fn exact_arcade_paths_match_only_indexed_local_mras() {
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
        other.mra = "_Arcade/Other.mra".into();
        let body = serde_json::to_vec(&vec![other]).unwrap();
        let cache = decode_data(meta(1, &body), &body).unwrap();
        let snapshot = match_cache(&cache, &request, &sender, &AtomicBool::new(false)).unwrap();
        assert_eq!(snapshot.items[0].state, LocalState::NotInstalled);
        assert!(snapshot.items[0].launch_path.is_none());
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
        row.bd = "2026-09-12".into();
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
        assert_eq!(snapshot.items[0].state, LocalState::NotInstalled);
        assert!(snapshot.items[0].launch_path.is_none());

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
        let cached_request = request(&root);
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
}
