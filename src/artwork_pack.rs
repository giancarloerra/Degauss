//! Read-only support for the MiSTer Artwork Pack database format.
//!
//! Pack keys are deliberately transient. A provider snapshot resolves the
//! current local TSV files and JPEG directory, then projects presentation on
//! source-neutral browse rows without writing anything into the Pack.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs::File;
use std::hash::Hash;
use std::io::{BufRead, BufReader, Read};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::browse::{Details, Kind, Launch, Row};
use crate::error::{DegaussError, Result};

const MAX_TSV_BYTES: u64 = 32 * 1024 * 1024;
const MAX_TOTAL_TSV_BYTES: u64 = 64 * 1024 * 1024;
const MAX_ROWS: usize = 250_000;
const MAX_LINE_BYTES: usize = 64 * 1024;
const MAX_VALUE_BYTES: usize = 1_024;
const MAX_JPEGS: usize = 100_000;
// XML metadata stays bounded; embedded ROM bytes do not belong to that budget.
const MAX_IDENTITY_XML_BYTES: usize = 1024 * 1024;
const MAX_MGL_REDIRECTS: usize = 8;
const MAX_HASH_BYTES: u64 = 64 * 1024 * 1024;
const HASH_BUFFER_BYTES: usize = 128 * 1024;

type ImageIndex = (HashMap<String, PathBuf>, Option<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameIdentity {
    pub name: String,
    pub setname: Option<String>,
    pub crc32: Option<u32>,
    pub size: Option<u64>,
    extension: Option<String>,
    hash_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MatchMethod {
    ExactKey,
    IndexName,
    TrailingSetname,
    Crc32AndSize,
    UniqueBareTitle,
}

impl MatchMethod {
    pub fn label(self) -> &'static str {
        match self {
            Self::ExactKey => "exact key",
            Self::IndexName => "index name",
            Self::TrailingSetname => "setname",
            Self::Crc32AndSize => "CRC32 and size",
            Self::UniqueBareTitle => "unique title",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolutionDiagnostic {
    pub pack_folder: String,
    pub key: String,
    pub method: MatchMethod,
    pub synopsis_language: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PackPresentation {
    pub name: Option<String>,
    pub cover: Option<PathBuf>,
    pub genre: Option<String>,
    pub details: Details,
    pub diagnostic: Option<ResolutionDiagnostic>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderHealth {
    Ready,
    Degraded,
    Unavailable,
    Invalid,
}

impl ProviderHealth {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Degraded => "Degraded",
            Self::Unavailable => "Unavailable",
            Self::Invalid => "Invalid",
        }
    }

    pub fn usable(self) -> bool {
        matches!(self, Self::Ready | Self::Degraded)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GameInfo {
    name: Option<String>,
    year: Option<String>,
    genre: Option<String>,
    developer: Option<String>,
    players: Option<String>,
}

#[derive(Debug, Clone)]
struct Lookup<K> {
    values: HashMap<K, String>,
    ambiguous: HashSet<K>,
}

impl<K: Eq + Hash + Clone> Default for Lookup<K> {
    fn default() -> Self {
        Self {
            values: HashMap::new(),
            ambiguous: HashSet::new(),
        }
    }
}

impl<K: Eq + Hash + Clone> Lookup<K> {
    /// Returns true only when this insertion newly made the lookup ambiguous.
    fn insert(&mut self, key: K, value: String) -> bool {
        if self.ambiguous.contains(&key) {
            return false;
        }
        match self.values.get(&key) {
            Some(existing) if existing != &value => {
                self.values.remove(&key);
                self.ambiguous.insert(key);
                true
            }
            Some(_) => false,
            None => {
                self.values.insert(key, value);
                false
            }
        }
    }

    fn get(&self, key: &K) -> Option<&str> {
        (!self.ambiguous.contains(key))
            .then(|| self.values.get(key).map(String::as_str))
            .flatten()
    }
}

#[derive(Debug, Clone)]
struct PackDirectory {
    label: String,
    images: HashMap<String, PathBuf>,
    display_keys: HashMap<String, String>,
    index_names: Lookup<String>,
    index_crc: Lookup<(u32, u64)>,
    bare_titles: Lookup<String>,
    gameinfo: HashMap<String, GameInfo>,
    synopsis: HashMap<String, (String, String)>,
}

impl PackDirectory {
    fn has_key(&self, key: &str) -> bool {
        let key = fold(key);
        self.images.contains_key(&key)
            || self.gameinfo.contains_key(&key)
            || self.synopsis.contains_key(&key)
    }

    fn resolve(&self, identity: &GameIdentity) -> Option<PackPresentation> {
        let mut candidates: Vec<(String, MatchMethod)> = Vec::new();
        let exact = fold(&identity.name);
        if self.has_key(&identity.name) {
            candidates.push((exact.clone(), MatchMethod::ExactKey));
        }
        if let Some(key) = self.index_names.get(&exact) {
            candidates.push((fold(key), MatchMethod::IndexName));
        }

        let setname = identity
            .setname
            .as_deref()
            .or_else(|| trailing_parenthesized(&identity.name));
        if let Some(setname) = setname.filter(|setname| self.has_key(setname)) {
            candidates.push((fold(setname), MatchMethod::TrailingSetname));
        }
        if let (Some(crc), Some(size)) = (identity.crc32, identity.size) {
            if let Some(key) = self.index_crc.get(&(crc, size)) {
                candidates.push((fold(key), MatchMethod::Crc32AndSize));
            }
        }
        let bare = fold(&bare_title(&identity.name));
        if let Some(key) = self.bare_titles.get(&bare) {
            candidates.push((fold(key), MatchMethod::UniqueBareTitle));
        }

        let mut seen = HashSet::new();
        let mut metadata_only = None;
        for (key, method) in candidates {
            if !seen.insert(key.clone()) {
                continue;
            }
            let has_image = self.images.contains_key(&key);
            let has_metadata = self.gameinfo.contains_key(&key) || self.synopsis.contains_key(&key);
            if has_image {
                return Some(self.presentation(&key, method));
            }
            if has_metadata && metadata_only.is_none() {
                metadata_only = Some(self.presentation(&key, method));
            }
        }
        metadata_only
    }

    fn presentation(&self, key: &str, method: MatchMethod) -> PackPresentation {
        let info = self.gameinfo.get(key);
        let synopsis = self.synopsis.get(key);
        let display_key = self
            .display_keys
            .get(key)
            .cloned()
            .unwrap_or_else(|| key.to_string());
        PackPresentation {
            name: info.and_then(|info| info.name.clone()),
            cover: self.images.get(key).cloned(),
            genre: info.and_then(|info| info.genre.clone()),
            details: Details {
                desc: synopsis
                    .map(|(_, text)| first_line(text))
                    .unwrap_or_default(),
                publisher: String::new(),
                developer: info
                    .and_then(|info| info.developer.clone())
                    .unwrap_or_default(),
                released: info.and_then(|info| info.year.clone()).unwrap_or_default(),
                players: info
                    .and_then(|info| info.players.clone())
                    .unwrap_or_default(),
                lang: String::new(),
            },
            diagnostic: Some(ResolutionDiagnostic {
                pack_folder: self.label.clone(),
                key: display_key,
                method,
                synopsis_language: synopsis.map(|(language, _)| language.clone()),
            }),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Provider {
    pub system_id: String,
    pub docs_root: PathBuf,
    pub health: ProviderHealth,
    pub diagnostics: Vec<String>,
    directories: Arc<Vec<PackDirectory>>,
    snapshot: Option<SourceFingerprint>,
    synopsis_language: Option<String>,
    /// Effective Pack presentation keyed by stable launch identity. It is
    /// prepared by a background worker so browse projection performs no game
    /// descriptor, archive, ROM, or Pack filesystem I/O.
    prepared: Option<Arc<HashMap<String, PackPresentation>>>,
    archive_cache: Arc<std::sync::Mutex<crate::zip::ArchiveCache>>,
}

impl Provider {
    fn identity_for_launch(
        &self,
        launch: &Launch,
        cancelled: &AtomicBool,
    ) -> Result<Option<GameIdentity>> {
        let mut archives = self.archive_cache.lock().map_err(|_| {
            DegaussError::unsupported("archive lookup", "archive cache lock was poisoned")
        })?;
        identity_for_launch_controlled(launch, cancelled, &mut archives)
    }

    pub fn load(system_id: &str, docs_root: &Path, language: Option<&str>) -> Self {
        let cancelled = AtomicBool::new(false);
        Self::load_controlled(system_id, docs_root, language, &cancelled)
            .expect("a non-cancellable provider load cannot be cancelled")
    }

    /// Parse one immutable provider snapshot with cooperative cancellation
    /// between Pack directory entries and table rows.
    pub fn load_controlled(
        system_id: &str,
        docs_root: &Path,
        language: Option<&str>,
        cancelled: &AtomicBool,
    ) -> Option<Self> {
        Self::load_observed_controlled(system_id, docs_root, language, cancelled, |_, _| {})
    }

    #[cfg(test)]
    fn load_observed(
        system_id: &str,
        docs_root: &Path,
        language: Option<&str>,
        after_read: impl FnMut(usize, &Path),
    ) -> Self {
        let cancelled = AtomicBool::new(false);
        Self::load_observed_controlled(system_id, docs_root, language, &cancelled, after_read)
            .expect("a non-cancellable provider load cannot be cancelled")
    }

    fn load_observed_controlled(
        system_id: &str,
        docs_root: &Path,
        language: Option<&str>,
        cancelled: &AtomicBool,
        mut after_read: impl FnMut(usize, &Path),
    ) -> Option<Self> {
        if cancelled.load(Ordering::Relaxed) {
            return None;
        }
        let Some(mapping) = mapping(system_id) else {
            return Some(Self {
                system_id: system_id.to_string(),
                docs_root: docs_root.to_path_buf(),
                health: ProviderHealth::Invalid,
                diagnostics: vec![format!("{system_id} has no Artwork Pack mapping")],
                directories: Arc::new(Vec::new()),
                snapshot: None,
                synopsis_language: normalized_language(language),
                prepared: None,
                archive_cache: Arc::new(std::sync::Mutex::new(crate::zip::ArchiveCache::default())),
            });
        };

        for attempt in 0..2 {
            let before = match source_fingerprint_controlled(mapping, docs_root, cancelled) {
                Ok(Some(before)) => before,
                Ok(None) => return None,
                Err(error) => {
                    let mut provider = Self::load_once_controlled(
                        system_id, docs_root, language, mapping, cancelled,
                    )?;
                    provider
                        .diagnostics
                        .push(format!("source fingerprint could not be read: {error}"));
                    if provider.health == ProviderHealth::Ready {
                        provider.health = ProviderHealth::Degraded;
                    }
                    return Some(provider);
                }
            };
            let mut provider =
                Self::load_once_controlled(system_id, docs_root, language, mapping, cancelled)?;
            after_read(attempt, docs_root);
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let after = match source_fingerprint_controlled(mapping, docs_root, cancelled) {
                Ok(Some(after)) => after,
                Ok(None) => return None,
                Err(error) => {
                    provider.snapshot = None;
                    provider
                        .diagnostics
                        .push(format!("source fingerprint could not be re-read: {error}"));
                    if provider.health == ProviderHealth::Ready {
                        provider.health = ProviderHealth::Degraded;
                    }
                    return Some(provider);
                }
            };
            if before == after {
                provider.snapshot = Some(after);
                return Some(provider);
            }
            if attempt == 1 {
                return Some(Self {
                    system_id: system_id.to_string(),
                    docs_root: docs_root.to_path_buf(),
                    health: ProviderHealth::Invalid,
                    diagnostics: vec![format!(
                        "{} changed repeatedly while it was being read; wait for the Artwork Pack update to finish",
                        docs_root.display()
                    )],
                    directories: Arc::new(Vec::new()),
                    snapshot: None,
                    synopsis_language: normalized_language(language),
                    prepared: None,
            archive_cache: Arc::new(std::sync::Mutex::new(crate::zip::ArchiveCache::default())),
                });
            }
        }
        unreachable!()
    }

    fn load_once_controlled(
        system_id: &str,
        docs_root: &Path,
        language: Option<&str>,
        mapping: Mapping,
        cancelled: &AtomicBool,
    ) -> Option<Self> {
        let mut directories = Vec::new();
        let mut diagnostics = Vec::new();
        let mut present = 0usize;
        let mut primary_usable = false;
        for (index, folder) in mapping.folders.iter().enumerate() {
            if cancelled.load(Ordering::Relaxed) {
                return None;
            }
            let artwork = docs_root.join(folder).join("Artwork");
            if !artwork.exists() {
                continue;
            }
            present += 1;
            match load_stable_directory(folder, &artwork, language, cancelled) {
                Ok(Some((directory, warnings))) => {
                    primary_usable |= index == 0;
                    diagnostics.extend(warnings);
                    directories.push(directory);
                }
                Ok(None) => return None,
                Err(error) => diagnostics.push(format!("{folder}: {error}")),
            }
        }

        let health = if directories.is_empty() {
            if present == 0 {
                diagnostics.push(format!(
                    "No mapped Artwork directory was found under {}",
                    docs_root.display()
                ));
                ProviderHealth::Unavailable
            } else {
                ProviderHealth::Invalid
            }
        } else if !primary_usable || !diagnostics.is_empty() {
            ProviderHealth::Degraded
        } else {
            ProviderHealth::Ready
        };

        Some(Self {
            system_id: system_id.to_string(),
            docs_root: docs_root.to_path_buf(),
            health,
            diagnostics,
            directories: Arc::new(directories),
            snapshot: None,
            synopsis_language: normalized_language(language),
            prepared: None,
            archive_cache: Arc::new(std::sync::Mutex::new(crate::zip::ArchiveCache::default())),
        })
    }

    /// Cheap change check for an already parsed provider. It examines the
    /// Artwork directory and table identities, not JPEG bodies. Re-entering a
    /// Pack system separately clears decoded images, so a style replacement
    /// at an unchanged path still reloads the visible pixels.
    pub fn still_current(&self, docs_root: &Path, language: Option<&str>) -> bool {
        if !self.configuration_matches(docs_root, language) {
            return false;
        }
        let Some(mapping) = mapping(&self.system_id) else {
            return false;
        };
        let Some(snapshot) = self.snapshot.as_ref() else {
            return false;
        };
        snapshot.still_current(mapping, docs_root)
    }

    /// Whether a worker result still belongs to the selected root and language.
    /// Unlike `still_current`, this deliberately accepts an Unavailable or
    /// Invalid snapshot whose source could not be fingerprinted: that health is
    /// the result the UI must show instead of scheduling the same failed read
    /// forever.
    pub fn configuration_matches(&self, docs_root: &Path, language: Option<&str>) -> bool {
        self.docs_root == docs_root && self.synopsis_language == normalized_language(language)
    }

    /// A reused parsed provider must not carry presentation prepared against
    /// an older source-neutral cache across a new worker validation pass.
    pub fn clear_prepared(&mut self) {
        self.prepared = None;
        self.archive_cache = Arc::new(std::sync::Mutex::new(crate::zip::ArchiveCache::default()));
    }

    #[cfg(test)]
    pub(crate) fn prepared_available(&self) -> bool {
        self.prepared.is_some()
    }

    /// Whether this snapshot still owns the parsed lookup catalogue needed to
    /// prepare rows. UI-held providers deliberately discard it after worker
    /// preparation so selecting many Pack systems cannot retain every TSV
    /// index in MiSTer's limited RAM.
    pub(crate) fn catalogue_available(&self) -> bool {
        !self.health.usable() || !self.directories.is_empty()
    }

    /// Retain only the provider health, source fingerprint and already
    /// prepared row presentation needed by the UI. A later worker validation
    /// reparses an unchanged usable catalogue before preparing a different or
    /// rebuilt cache.
    pub(crate) fn discard_catalogue(&mut self) {
        self.directories = Arc::new(Vec::new());
        self.archive_cache = Arc::new(std::sync::Mutex::new(crate::zip::ArchiveCache::default()));
    }

    #[cfg(test)]
    pub fn presentation_for_launch(&self, launch: &Launch) -> Result<Option<PackPresentation>> {
        self.presentation_for_launch_with_fingerprints(launch, &BTreeMap::new())
    }

    pub fn presentation_for_launch_with_fingerprints(
        &self,
        launch: &Launch,
        fingerprints: &crate::cache::ContentFingerprints,
    ) -> Result<Option<PackPresentation>> {
        self.presentation_for_launch_controlled(launch, fingerprints, &AtomicBool::new(false))
    }

    fn presentation_for_launch_controlled(
        &self,
        launch: &Launch,
        fingerprints: &crate::cache::ContentFingerprints,
        cancelled: &AtomicBool,
    ) -> Result<Option<PackPresentation>> {
        if !self.health.usable() {
            return Ok(None);
        }
        let Some(mut identity) = self.identity_for_launch(launch, cancelled)? else {
            return Ok(None);
        };
        let cheap = self.resolve(&identity);
        if cheap
            .as_ref()
            .is_some_and(|presentation| presentation.cover.is_some())
        {
            return Ok(cheap);
        }
        if identity.crc32.is_none() {
            if let Some(path) = identity.hash_path.as_deref() {
                if let Some(fingerprint) = fingerprints.get(&fingerprint_key(path)) {
                    // The provider worker validated this complete map against
                    // the live files before the snapshot reached the UI. A
                    // per-row stat here would move card I/O back into input
                    // and rendering whenever a list is opened or filtered.
                    identity.crc32 = Some(fingerprint.crc32);
                    identity.size = Some(fingerprint.size);
                }
            }
        }
        let with_crc = self.resolve(&identity);
        Ok(match with_crc {
            Some(presentation) if presentation.cover.is_some() => Some(presentation),
            Some(presentation) if cheap.is_none() => Some(presentation),
            _ => cheap,
        })
    }

    /// Resolve every playable row while running on a background worker. The
    /// resulting in-memory map deliberately contains no source-neutral rows
    /// and is never serialized into Degauss caches.
    pub fn prepare_for_cache(
        &mut self,
        cache: &crate::cache::SystemCache,
        fingerprints: &crate::cache::ContentFingerprints,
        cancelled: &AtomicBool,
    ) -> Result<Option<usize>> {
        let mut prepared = HashMap::new();
        let mut inspected = HashSet::new();
        for folder in cache.folders.values() {
            for row in &folder.rows {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let Kind::Play(launch) = &row.kind else {
                    continue;
                };
                let key = launch_cache_key(launch);
                if !inspected.insert(key.clone()) {
                    continue;
                }
                let presentation =
                    self.presentation_for_launch_controlled(launch, fingerprints, cancelled)?;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                if let Some(presentation) = presentation {
                    prepared.insert(key, presentation);
                }
            }
        }
        let matched = prepared.len();
        self.prepared = Some(Arc::new(prepared));
        Ok(Some(matched))
    }

    /// Apply a snapshot already prepared by a worker. Absence means the
    /// worker has not validated this cache, so the UI leaves source-neutral
    /// rows untouched instead of falling back to synchronous filesystem I/O.
    pub fn apply_prepared(&self, rows: &mut [Row]) -> usize {
        if !self.health.usable() {
            return 0;
        }
        let Some(prepared) = self.prepared.as_ref() else {
            return 0;
        };
        let mut matched = 0;
        for row in rows {
            let Kind::Play(launch) = &row.kind else {
                continue;
            };
            let Some(presentation) = prepared.get(&launch_cache_key(launch)) else {
                continue;
            };
            apply_presentation(row, presentation.clone());
            matched += 1;
        }
        matched
    }

    pub fn apply_with_fingerprints(
        &self,
        rows: &mut [Row],
        fingerprints: &crate::cache::ContentFingerprints,
    ) -> Result<usize> {
        if !self.health.usable() {
            return Ok(0);
        }
        let mut matched = 0;
        for row in rows {
            let Kind::Play(launch) = &row.kind else {
                continue;
            };
            let presentation =
                match self.presentation_for_launch_with_fingerprints(launch, fingerprints) {
                    Ok(Some(presentation)) => presentation,
                    Ok(None) => continue,
                    Err(error) => {
                        crate::note(&format!("pack match   {error}"));
                        continue;
                    }
                };
            apply_presentation(row, presentation);
            matched += 1;
        }
        Ok(matched)
    }

    /// Calculate the one source-neutral fingerprint needed after every
    /// cheaper Pack match has failed. This is called only by a background
    /// cache worker, never by the render or input thread.
    pub fn fingerprint_for_launch(
        &self,
        launch: &Launch,
        cancelled: &AtomicBool,
        on_bytes: &mut dyn FnMut(u64),
    ) -> Result<Option<(String, crate::cache::ContentFingerprint)>> {
        if !self.health.usable() || cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(identity) = self.identity_for_launch(launch, cancelled)? else {
            return Ok(None);
        };
        if self
            .resolve(&identity)
            .is_some_and(|presentation| presentation.cover.is_some())
        {
            return Ok(None);
        }
        let Some(path) = identity.hash_path.as_deref() else {
            return Ok(None);
        };
        let Some((crc32, metadata)) = crc32_if_eligible(path, cancelled, on_bytes)? else {
            return Ok(None);
        };
        let modified = metadata
            .modified()
            .map_err(|error| DegaussError::io("reading game modification time", path, error))?;
        let duration = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|error| {
                DegaussError::unsupported(
                    "Artwork Pack fingerprint",
                    format!(
                        "{} has a modification time before 1970: {error}",
                        path.display()
                    ),
                )
            })?;
        Ok(Some((
            fingerprint_key(path),
            crate::cache::ContentFingerprint {
                size: metadata.len(),
                modified_seconds: duration.as_secs(),
                modified_nanos: duration.subsec_nanos(),
                crc32,
            },
        )))
    }

    pub fn fingerprints_for_cache(
        &self,
        cache: &crate::cache::SystemCache,
        cancelled: &AtomicBool,
        on_progress: &mut dyn FnMut(usize, u64),
    ) -> Result<Option<crate::cache::ContentFingerprints>> {
        let mut fingerprints = crate::cache::ContentFingerprints::new();
        let mut inspected = HashSet::new();
        let mut files = 0usize;
        let mut bytes = 0u64;
        for folder in cache.folders.values() {
            for row in &folder.rows {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let Kind::Play(launch) = &row.kind else {
                    continue;
                };
                if !inspected.insert(launch_cache_key(launch)) {
                    continue;
                }
                let fingerprint = self.fingerprint_for_launch(launch, cancelled, &mut |read| {
                    bytes = bytes.saturating_add(read);
                    on_progress(files, bytes);
                })?;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                if let Some((key, fingerprint)) = fingerprint {
                    fingerprints.insert(key, fingerprint);
                    files += 1;
                    on_progress(files, bytes);
                }
            }
        }
        Ok(Some(fingerprints))
    }

    /// Validate persisted loose-file CRCs for this exact provider policy.
    /// This may stat and parse game files and therefore belongs on the
    /// provider worker, never in browse projection.
    pub fn cached_fingerprints_are_current(
        &self,
        cache: &crate::cache::SystemCache,
        fingerprints: &crate::cache::ContentFingerprints,
        complete: bool,
        cancelled: &AtomicBool,
    ) -> Result<Option<bool>> {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if !self.health.usable() {
            return Ok(Some(true));
        }
        if !complete {
            return Ok(Some(false));
        }

        let mut inspected = HashSet::new();
        for folder in cache.folders.values() {
            for row in &folder.rows {
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let Kind::Play(launch) = &row.kind else {
                    continue;
                };
                if !inspected.insert(launch_cache_key(launch)) {
                    continue;
                }
                let identity = self.identity_for_launch(launch, cancelled)?;
                if cancelled.load(Ordering::Relaxed) {
                    return Ok(None);
                }
                let Some(identity) = identity else {
                    continue;
                };
                if identity.crc32.is_some()
                    || self
                        .resolve(&identity)
                        .is_some_and(|presentation| presentation.cover.is_some())
                {
                    continue;
                }
                let Some(path) = identity.hash_path.as_deref() else {
                    continue;
                };
                let extension = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .unwrap_or("")
                    .to_ascii_lowercase();
                let metadata = match std::fs::metadata(path) {
                    Ok(metadata) => metadata,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                        return Ok(Some(false));
                    }
                    Err(error) => {
                        return Err(DegaussError::io(
                            "validating cached Artwork Pack identity",
                            path,
                            error,
                        ));
                    }
                };
                if !metadata.is_file()
                    || hashing_excluded(&extension)
                    || metadata.len() > MAX_HASH_BYTES
                {
                    continue;
                }
                let Some(fingerprint) = fingerprints.get(&fingerprint_key(path)) else {
                    return Ok(Some(false));
                };
                if !fingerprint.matches_metadata(&metadata) {
                    return Ok(Some(false));
                }
            }
        }
        Ok(Some(true))
    }

    fn resolve(&self, identity: &GameIdentity) -> Option<PackPresentation> {
        let mut metadata_only = None;
        let mut directories: Vec<&PackDirectory> = self.directories.iter().collect();
        if self.system_id == "SNES" {
            if identity.extension.as_deref() == Some("bs") {
                directories.sort_by_key(|directory| directory.label != "Satellaview");
            } else {
                directories.retain(|directory| directory.label != "Satellaview");
            }
        }
        for directory in directories {
            if let Some(presentation) = directory.resolve(identity) {
                if presentation.cover.is_some() {
                    return Some(presentation);
                }
                if metadata_only.is_none() {
                    metadata_only = Some(presentation);
                }
            }
        }
        metadata_only
    }

    pub fn status_line(&self) -> String {
        match self.diagnostics.first() {
            Some(problem) => format!("{}: {problem}", self.health.label()),
            None => self.health.label().to_string(),
        }
    }
}

fn apply_presentation(row: &mut Row, presentation: PackPresentation) {
    if let Some(name) = presentation.name {
        row.name = name;
        row.sort_key = row.name.to_lowercase();
    }
    row.cover = presentation.cover;
    row.genre = presentation.genre;
    row.details = presentation.details;
}

fn launch_cache_key(launch: &Launch) -> String {
    match launch {
        Launch::File(path) => format!("file:{}", path.display()),
        Launch::AmigaVision { install, title } => {
            format!("amigavision:{}\0{title}", install.display())
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Mapping {
    group: &'static str,
    folders: &'static [&'static str],
}

fn mapping(system_id: &str) -> Option<Mapping> {
    let mapping = match system_id {
        "3DO" => Mapping {
            group: "3DO",
            folders: &["3DO"],
        },
        "AmigaCD32" => Mapping {
            group: "AmigaCD32",
            folders: &["AmigaCD32"],
        },
        "Arcade" => Mapping {
            group: "Arcade",
            folders: &["Arcade"],
        },
        "Atari2600" => Mapping {
            group: "Atari2600",
            folders: &["Atari2600"],
        },
        "Atari5200" => Mapping {
            group: "Atari5200",
            folders: &["ATARI5200"],
        },
        "Atari7800" => Mapping {
            group: "Atari7800",
            folders: &["ATARI7800"],
        },
        "AtariLynx" => Mapping {
            group: "AtariLynx",
            folders: &["AtariLynx"],
        },
        "CDI" => Mapping {
            group: "CDI",
            folders: &["CD-i"],
        },
        "ColecoVision" => Mapping {
            group: "ColecoVision",
            folders: &["Coleco"],
        },
        "FDS" => Mapping {
            group: "FDS",
            folders: &["FDS", "NES"],
        },
        "Gameboy" | "Gameboy2P" | "SuperGameboy" => Mapping {
            group: system_id_group(system_id),
            folders: &["GAMEBOY", "GBC"],
        },
        "GameboyColor" => Mapping {
            group: "GameboyColor",
            folders: &["GBC", "GAMEBOY"],
        },
        "GBA" | "GBA2P" => Mapping {
            group: system_id_group(system_id),
            folders: &["GBA"],
        },
        "GameGear" | "GameGear2P" => Mapping {
            group: system_id_group(system_id),
            folders: &["GameGear"],
        },
        "Genesis" => Mapping {
            group: "Genesis",
            folders: &["Genesis"],
        },
        "Intellivision" => Mapping {
            group: "Intellivision",
            folders: &["Intellivision"],
        },
        "Jaguar" => Mapping {
            group: "Jaguar",
            folders: &["Jaguar"],
        },
        "MegaCD" => Mapping {
            group: "MegaCD",
            folders: &["MegaCD"],
        },
        "Nintendo64" => Mapping {
            group: "Nintendo64",
            folders: &["N64"],
        },
        "NeoGeo" | "NeoGeoMVS" => Mapping {
            group: "NeoGeo",
            folders: &["NEOGEO"],
        },
        "NES" => Mapping {
            group: "NES",
            folders: &["NES"],
        },
        "NeoGeoCD" => Mapping {
            group: "NeoGeoCD",
            folders: &["NeoGeo-CD"],
        },
        "NeoGeoPocket" => Mapping {
            group: "NeoGeoPocket",
            folders: &["NeoGeoPocket"],
        },
        "NeoGeoPocketColor" => Mapping {
            group: "NeoGeoPocketColor",
            folders: &["NeoGeoPocket-Color"],
        },
        "Odyssey2" => Mapping {
            group: "Odyssey2",
            folders: &["ODYSSEY2"],
        },
        "PSX" => Mapping {
            group: "PSX",
            folders: &["PSX"],
        },
        "Sega32X" => Mapping {
            group: "Sega32X",
            folders: &["S32X"],
        },
        "SG1000" => Mapping {
            group: "SG1000",
            folders: &["SG-1000"],
        },
        "MasterSystem" => Mapping {
            group: "MasterSystem",
            folders: &["SMS"],
        },
        "SNES" => Mapping {
            group: "SNES",
            folders: &["SNES", "Satellaview"],
        },
        "Saturn" => Mapping {
            group: "Saturn",
            folders: &["Saturn"],
        },
        "SuperGrafx" => Mapping {
            group: "SuperGrafx",
            folders: &["SuperGrafx"],
        },
        "TurboGrafx16" => Mapping {
            group: "TurboGrafx16",
            folders: &["TGFX16"],
        },
        "TurboGrafx16CD" => Mapping {
            group: "TurboGrafx16CD",
            folders: &["TGFX16-CD"],
        },
        "Vectrex" => Mapping {
            group: "Vectrex",
            folders: &["VECTREX"],
        },
        "VirtualBoy" => Mapping {
            group: "VirtualBoy",
            folders: &["VirtualBoy"],
        },
        "WonderSwan" => Mapping {
            group: "WonderSwan",
            folders: &["WonderSwan"],
        },
        "WonderSwanColor" => Mapping {
            group: "WonderSwanColor",
            folders: &["WonderSwanColor"],
        },
        _ => return None,
    };
    Some(mapping)
}

fn system_id_group(system_id: &str) -> &'static str {
    match system_id {
        "Gameboy2P" => "Gameboy2P",
        "SuperGameboy" => "SuperGameboy",
        "GBA2P" => "GBA2P",
        "GameGear2P" => "GameGear2P",
        _ => match system_id {
            "Gameboy" => "Gameboy",
            "GBA" => "GBA",
            "GameGear" => "GameGear",
            _ => "",
        },
    }
}

pub fn supports(system_id: &str) -> bool {
    mapping(system_id).is_some()
}

pub fn source_group(system_id: &str) -> Option<&'static str> {
    mapping(system_id).map(|mapping| mapping.group)
}

pub fn expected_folders(system_id: &str) -> &'static [&'static str] {
    mapping(system_id)
        .map(|mapping| mapping.folders)
        .unwrap_or(&[])
}

pub fn selected_root<'a>(roots: &'a BTreeMap<String, String>, system_id: &str) -> Option<&'a Path> {
    roots
        .get(source_group(system_id)?)
        .map(String::as_str)
        .map(Path::new)
}

/// Find bounded structural candidates. The UI validates each candidate with a
/// full provider read on a worker before it can be offered for selection.
pub fn discover_candidate_roots(system_id: &str, game_roots: &[String]) -> Vec<PathBuf> {
    let mut bases = vec![
        PathBuf::from("/media/fat"),
        PathBuf::from("/media/network"),
        PathBuf::from("/media/fat/cifs"),
    ];
    for index in 0..=7 {
        bases.insert(index + 1, PathBuf::from(format!("/media/usb{index}")));
    }
    for root in game_roots {
        let path = PathBuf::from(root);
        if let Some(parent) = path.parent() {
            bases.push(parent.to_path_buf());
        }
    }

    let mut found = Vec::new();
    let mut seen = HashSet::new();
    for base in bases {
        let docs = if base.file_name().is_some_and(|name| name == "docs") {
            base.clone()
        } else {
            base.join("docs")
        };
        if !expected_folders(system_id)
            .iter()
            .any(|folder| docs.join(folder).join("Artwork").is_dir())
        {
            continue;
        }
        let canonical = std::fs::canonicalize(&docs).unwrap_or(docs);
        let allowed_base = std::fs::canonicalize(&base).unwrap_or(base);
        if !canonical.starts_with(&allowed_base) {
            continue;
        }
        if seen.insert(canonical.clone()) {
            found.push(canonical);
        }
    }
    found
}

pub fn normalize_docs_root(system_id: &str, chosen: &Path) -> Option<PathBuf> {
    let canonical = std::fs::canonicalize(chosen).ok()?;
    for ancestor in canonical.ancestors() {
        if ancestor.file_name().is_some_and(|name| name == "docs")
            && expected_folders(system_id)
                .iter()
                .any(|folder| ancestor.join(folder).join("Artwork").is_dir())
        {
            return Some(ancestor.to_path_buf());
        }
    }
    let direct = canonical.join("docs");
    expected_folders(system_id)
        .iter()
        .any(|folder| direct.join(folder).join("Artwork").is_dir())
        .then_some(direct)
}

fn load_stable_directory(
    label: &str,
    artwork: &Path,
    language: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Option<(PackDirectory, Vec<String>)>> {
    for attempt in 0..2 {
        let Some(before) = fingerprint_controlled(artwork, cancelled)? else {
            return Ok(None);
        };
        let loaded = load_directory(label, artwork, language, cancelled);
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(after) = fingerprint_controlled(artwork, cancelled)? else {
            return Ok(None);
        };
        if before == after {
            return loaded;
        }
        if attempt == 1 {
            return Err(DegaussError::unsupported(
                "Artwork Pack",
                format!("{} changed while it was being read", artwork.display()),
            ));
        }
    }
    unreachable!()
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    directory_modified: u128,
    /// The manifest content identity closes the FAT/exFAT same-size,
    /// same-timestamp replacement case used by an artwork-style switch.
    /// Supplemental tables retain the cheaper metadata identity.
    tables: Vec<(String, u64, u128, Option<u32>)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SourceFingerprint(Vec<(String, Option<Fingerprint>)>);

impl SourceFingerprint {
    /// Recheck an immutable snapshot without enumerating the image directory.
    /// The directory mtime detects entry replacement/addition/removal, and the
    /// already-known table paths detect in-place table edits. Provider loading
    /// itself still performs the full bounded directory read on its worker.
    fn still_current(&self, mapping: Mapping, docs_root: &Path) -> bool {
        self.0.len() == mapping.folders.len()
            && self.0.iter().zip(mapping.folders).all(
                |((recorded_folder, recorded), current_folder)| {
                    recorded_folder == current_folder
                        && match recorded {
                            Some(recorded) => recorded
                                .still_current(&docs_root.join(current_folder).join("Artwork")),
                            None => !docs_root.join(current_folder).join("Artwork").exists(),
                        }
                },
            )
    }
}

impl Fingerprint {
    fn still_current(&self, artwork: &Path) -> bool {
        let Ok(directory) = std::fs::metadata(artwork) else {
            return false;
        };
        if !directory.is_dir() || modified_nanos(&directory) != self.directory_modified {
            return false;
        }
        self.tables
            .iter()
            .all(|(name, size, modified, content_crc32)| {
                let path = artwork.join(name);
                std::fs::metadata(&path).is_ok_and(|metadata| {
                    metadata.is_file()
                        && metadata.len() == *size
                        && modified_nanos(&metadata) == *modified
                        && content_crc32.is_none_or(|expected| file_crc32(&path) == Some(expected))
                })
            })
    }
}

fn source_fingerprint_controlled(
    mapping: Mapping,
    docs_root: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<SourceFingerprint>> {
    let mut directories = Vec::new();
    for folder in mapping.folders {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let artwork = docs_root.join(folder).join("Artwork");
        let fingerprint = if artwork.exists() {
            let Some(fingerprint) = fingerprint_controlled(&artwork, cancelled)? else {
                return Ok(None);
            };
            Some(fingerprint)
        } else {
            None
        };
        directories.push(((*folder).to_string(), fingerprint));
    }
    Ok(Some(SourceFingerprint(directories)))
}

fn fingerprint_controlled(artwork: &Path, cancelled: &AtomicBool) -> Result<Option<Fingerprint>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let directory_modified = std::fs::metadata(artwork)
        .map_err(|error| DegaussError::io("reading Artwork Pack directory", artwork, error))?
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let listing = std::fs::read_dir(artwork)
        .map_err(|error| DegaussError::io("reading Artwork Pack directory", artwork, error))?;
    let mut entries = Vec::new();
    for item in listing {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let item =
            item.map_err(|error| DegaussError::io("reading Artwork Pack entry", artwork, error))?;
        let path = item.path();
        let name = item.file_name().to_string_lossy().into_owned();
        if !recognized_table_file(&name) {
            continue;
        }
        let metadata = item
            .metadata()
            .map_err(|error| DegaussError::io("reading Artwork Pack entry", &path, error))?;
        if metadata.len() > MAX_TSV_BYTES {
            return Err(DegaussError::unsupported(
                "Artwork Pack table",
                format!(
                    "{} is {} bytes, past the {MAX_TSV_BYTES} byte limit",
                    path.display(),
                    metadata.len()
                ),
            ));
        }
        let modified = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let content_crc32 = if name.eq_ignore_ascii_case("manifest.tsv") {
            let Some(crc32) = file_crc32_controlled(&path, cancelled)? else {
                return Ok(None);
            };
            Some(crc32)
        } else {
            None
        };
        entries.push((name, metadata.len(), modified, content_crc32));
    }
    entries.sort();
    Ok(Some(Fingerprint {
        directory_modified,
        tables: entries,
    }))
}

fn file_crc32(path: &Path) -> Option<u32> {
    file_crc32_controlled(path, &AtomicBool::new(false))
        .ok()
        .flatten()
}

fn file_crc32_controlled(path: &Path, cancelled: &AtomicBool) -> Result<Option<u32>> {
    let mut file = File::open(path)
        .map_err(|error| DegaussError::io("opening Artwork Pack manifest", path, error))?;
    let mut hasher = crc32fast::Hasher::new();
    let mut buffer = [0_u8; HASH_BUFFER_BYTES];
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let read = file
            .read(&mut buffer)
            .map_err(|error| DegaussError::io("reading Artwork Pack manifest", path, error))?;
        if read == 0 {
            return Ok(Some(hasher.finalize()));
        }
        hasher.update(&buffer[..read]);
    }
}

fn modified_nanos(metadata: &std::fs::Metadata) -> u128 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn recognized_table_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "index.tsv" | "gameinfo.tsv" | "manifest.tsv"
    ) || (lower.starts_with("synopsis_") && lower.ends_with(".tsv"))
}

fn normalized_language(language: Option<&str>) -> Option<String> {
    language
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .map(str::to_ascii_lowercase)
}

fn load_directory(
    label: &str,
    artwork: &Path,
    language: Option<&str>,
    cancelled: &AtomicBool,
) -> Result<Option<(PackDirectory, Vec<String>)>> {
    let mut warnings = Vec::new();
    let mut table_budget = TableBudget::default();
    let Some((all_images, collision)) = image_index(artwork, cancelled)? else {
        return Ok(None);
    };
    if let Some(name) = collision {
        return Err(DegaussError::malformed(
            "Artwork Pack",
            artwork,
            format!("case-insensitive JPEG collision for {name}"),
        ));
    }

    let manifest_path = artwork.join("manifest.tsv");
    let Some(manifest_rows) = read_tsv_controlled(
        &manifest_path,
        &["#key", "style", "ss_system_id"],
        3,
        &mut table_budget,
        cancelled,
    )?
    else {
        return Ok(None);
    };
    let mut manifest_rows_by_key: HashMap<String, Vec<String>> = HashMap::new();
    for row in manifest_rows {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let key = &row[0];
        validate_key(key, &manifest_path)?;
        let folded = fold(key);
        match manifest_rows_by_key.get(&folded) {
            Some(existing) if existing != &row => {
                return Err(DegaussError::malformed(
                    "Artwork Pack manifest",
                    &manifest_path,
                    format!("conflicting duplicate key {key}"),
                ));
            }
            _ => {
                manifest_rows_by_key.insert(folded, row);
            }
        }
    }

    let mut images = HashMap::new();
    let mut display_keys = HashMap::new();
    let mut missing_images = 0usize;
    for (folded, row) in &manifest_rows_by_key {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let key = &row[0];
        display_keys.insert(folded.clone(), key.clone());
        match all_images.get(folded) {
            Some(path) => {
                images.insert(folded.clone(), path.clone());
            }
            None => missing_images += 1,
        }
    }
    if images.is_empty() {
        return Err(DegaussError::malformed(
            "Artwork Pack manifest",
            &manifest_path,
            "no manifest key has a matching JPEG",
        ));
    }
    if missing_images > 0 {
        warnings.push(format!(
            "{label}: {missing_images} manifest images are missing"
        ));
    }

    let mut index_names = Lookup::default();
    let mut index_crc = Lookup::default();
    let mut bare_titles = Lookup::default();
    let index_path = artwork.join("index.tsv");
    if index_path.is_file() {
        match read_tsv_controlled(
            &index_path,
            &["#name", "crc", "size", "key"],
            4,
            &mut table_budget,
            cancelled,
        )
        .and_then(|rows| match rows {
            Some(rows) => parse_index_rows_controlled(rows, &index_path, cancelled),
            None => Ok(None),
        }) {
            Ok(Some((names, crc, bare, index_warnings))) => {
                index_names = names;
                index_crc = crc;
                bare_titles = bare;
                warnings.extend(
                    index_warnings
                        .into_iter()
                        .map(|warning| format!("{label}: {warning}")),
                );
            }
            Ok(None) => return Ok(None),
            Err(error) => warnings.push(format!("{label}: index.tsv ignored: {error}")),
        }
    } else {
        warnings.push(format!("{label}: index.tsv is missing; exact matches only"));
    }

    let mut gameinfo = HashMap::new();
    let gameinfo_path = artwork.join("gameinfo.tsv");
    if gameinfo_path.is_file() {
        match read_tsv_controlled(
            &gameinfo_path,
            &["#key", "name", "year", "genre", "developer", "players"],
            6,
            &mut table_budget,
            cancelled,
        ) {
            Ok(Some(rows)) => {
                let mut ambiguous = HashSet::new();
                for row in rows {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if let Err(error) = validate_key(&row[0], &gameinfo_path) {
                        gameinfo.clear();
                        warnings.push(format!("{label}: gameinfo.tsv ignored: {error}"));
                        ambiguous.clear();
                        break;
                    }
                    if let Err(error) = validate_value(&row[1], "name", &gameinfo_path) {
                        gameinfo.clear();
                        warnings.push(format!("{label}: gameinfo.tsv ignored: {error}"));
                        ambiguous.clear();
                        break;
                    }
                    let key = fold(&row[0]);
                    let value = GameInfo {
                        name: present(&row[1]),
                        year: present(&row[2]),
                        genre: present(&row[3]),
                        developer: present(&row[4]),
                        players: present(&row[5]),
                    };
                    if ambiguous.contains(&key) {
                        continue;
                    }
                    match gameinfo.get(&key) {
                        Some(existing) if existing != &value => {
                            gameinfo.remove(&key);
                            ambiguous.insert(key.clone());
                            warnings.push(format!(
                                "{label}: conflicting gameinfo rows for {} were ignored",
                                row[0]
                            ));
                        }
                        Some(_) => {}
                        None => {
                            display_keys
                                .entry(key.clone())
                                .or_insert_with(|| row[0].clone());
                            gameinfo.insert(key, value);
                        }
                    }
                }
            }
            Ok(None) => return Ok(None),
            Err(error) => warnings.push(format!("{label}: gameinfo.tsv ignored: {error}")),
        }
    } else {
        warnings.push(format!("{label}: gameinfo.tsv is missing"));
    }

    let Some(synopsis) = load_synopses(
        artwork,
        language,
        &mut warnings,
        &mut table_budget,
        cancelled,
    )?
    else {
        return Ok(None);
    };
    for key in synopsis.keys() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        display_keys
            .entry(key.clone())
            .or_insert_with(|| key.clone());
    }
    let mut catalog_bare_conflicts = 0usize;
    for (folded, display) in &display_keys {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let bare = fold(&bare_title(display));
        if !bare.is_empty() {
            catalog_bare_conflicts += usize::from(bare_titles.insert(bare, folded.clone()));
        }
    }
    if catalog_bare_conflicts > 0 {
        warnings.push(format!(
            "{label}: {catalog_bare_conflicts} ambiguous catalogue bare-title lookup{} ignored",
            if catalog_bare_conflicts == 1 { "" } else { "s" }
        ));
    }

    Ok(Some((
        PackDirectory {
            label: label.to_string(),
            images,
            display_keys,
            index_names,
            index_crc,
            bare_titles,
            gameinfo,
            synopsis,
        },
        warnings,
    )))
}

type ParsedIndex = (
    Lookup<String>,
    Lookup<(u32, u64)>,
    Lookup<String>,
    Vec<String>,
);

#[cfg(test)]
fn parse_index_rows(rows: Vec<Vec<String>>, path: &Path) -> Result<ParsedIndex> {
    parse_index_rows_controlled(rows, path, &AtomicBool::new(false))?.ok_or_else(|| {
        DegaussError::unsupported(
            "Artwork Pack index",
            "index parsing was cancelled unexpectedly",
        )
    })
}

fn parse_index_rows_controlled(
    rows: Vec<Vec<String>>,
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<ParsedIndex>> {
    let mut names = Lookup::default();
    let mut crc_and_size = Lookup::default();
    let mut bare_titles = Lookup::default();
    let mut name_conflicts = 0usize;
    let mut crc_conflicts = 0usize;
    let mut bare_conflicts = 0usize;
    for row in rows {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        validate_value(&row[0], "name", path)?;
        validate_key(&row[3], path)?;
        let name = fold(&row[0]);
        let key = fold(&row[3]);
        name_conflicts += usize::from(names.insert(name, key.clone()));
        bare_conflicts += usize::from(bare_titles.insert(fold(&bare_title(&row[0])), key.clone()));
        match (row[1].is_empty(), row[2].is_empty()) {
            (true, true) => {}
            (false, false) => {
                let crc = u32::from_str_radix(&row[1], 16).map_err(|error| {
                    DegaussError::malformed(
                        "Artwork Pack index",
                        path,
                        format!("invalid CRC {}: {error}", row[1]),
                    )
                })?;
                let size = row[2].parse::<u64>().map_err(|error| {
                    DegaussError::malformed(
                        "Artwork Pack index",
                        path,
                        format!("invalid size {}: {error}", row[2]),
                    )
                })?;
                crc_conflicts += usize::from(crc_and_size.insert((crc, size), key));
            }
            _ => {
                return Err(DegaussError::malformed(
                    "Artwork Pack index",
                    path,
                    "CRC and size must both be present or both be empty",
                ));
            }
        }
    }
    let mut warnings = Vec::new();
    if name_conflicts > 0 {
        warnings.push(format!(
            "{name_conflicts} conflicting index name lookup{} ignored",
            if name_conflicts == 1 { "" } else { "s" }
        ));
    }
    if crc_conflicts > 0 {
        warnings.push(format!(
            "{crc_conflicts} conflicting CRC-and-size lookup{} ignored",
            if crc_conflicts == 1 { "" } else { "s" }
        ));
    }
    if bare_conflicts > 0 {
        warnings.push(format!(
            "{bare_conflicts} ambiguous bare-title lookup{} ignored",
            if bare_conflicts == 1 { "" } else { "s" }
        ));
    }
    Ok(Some((names, crc_and_size, bare_titles, warnings)))
}

fn image_index(artwork: &Path, cancelled: &AtomicBool) -> Result<Option<ImageIndex>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let contained_root = std::fs::canonicalize(artwork)
        .map_err(|error| DegaussError::io("resolving Artwork Pack directory", artwork, error))?;
    let listing = std::fs::read_dir(artwork)
        .map_err(|error| DegaussError::io("reading Artwork Pack images", artwork, error))?;
    let mut images = HashMap::new();
    let mut count = 0usize;
    for item in listing {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let item =
            item.map_err(|error| DegaussError::io("reading Artwork Pack image", artwork, error))?;
        let path = item.path();
        if !path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("jpg"))
        {
            continue;
        }
        let file_type = item
            .file_type()
            .map_err(|error| DegaussError::io("reading Artwork Pack image", &path, error))?;
        if file_type.is_symlink() {
            let resolved = std::fs::canonicalize(&path)
                .map_err(|error| DegaussError::io("resolving Artwork Pack image", &path, error))?;
            if !resolved.starts_with(&contained_root) {
                return Err(DegaussError::malformed(
                    "Artwork Pack image",
                    &path,
                    format!(
                        "resolved outside the selected Artwork directory to {}",
                        resolved.display()
                    ),
                ));
            }
            let metadata = std::fs::metadata(&resolved)
                .map_err(|error| DegaussError::io("reading Artwork Pack image", &path, error))?;
            if !metadata.is_file() {
                continue;
            }
        } else if !file_type.is_file() {
            continue;
        }
        count += 1;
        validate_jpeg_count(count, artwork)?;
        let Some(stem) = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .map(str::to_string)
        else {
            return Err(DegaussError::malformed(
                "Artwork Pack image",
                &path,
                "filename is not UTF-8",
            ));
        };
        validate_key(&stem, &path)?;
        if let Some(key) = insert_image_entry(&mut images, &stem, path) {
            return Ok(Some((images, Some(key))));
        }
    }
    Ok(Some((images, None)))
}

fn insert_image_entry(
    images: &mut HashMap<String, PathBuf>,
    stem: &str,
    path: PathBuf,
) -> Option<String> {
    let key = fold(stem);
    images.insert(key.clone(), path).map(|_| key)
}

fn validate_jpeg_count(count: usize, artwork: &Path) -> Result<()> {
    if count > MAX_JPEGS {
        return Err(DegaussError::unsupported(
            "Artwork Pack",
            format!("{} has more than {MAX_JPEGS} JPEGs", artwork.display()),
        ));
    }
    Ok(())
}

fn load_synopses(
    artwork: &Path,
    preferred: Option<&str>,
    warnings: &mut Vec<String>,
    table_budget: &mut TableBudget,
    cancelled: &AtomicBool,
) -> Result<Option<HashMap<String, (String, String)>>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let listing = std::fs::read_dir(artwork)
        .map_err(|error| DegaussError::io("reading Artwork Pack synopsis files", artwork, error))?;
    let mut paths = BTreeMap::new();
    for item in listing {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let item = match item {
            Ok(item) => item,
            Err(error) => {
                warnings.push(format!(
                    "{}: one directory entry could not be read: {error}",
                    artwork.display()
                ));
                continue;
            }
        };
        let name = item.file_name().to_string_lossy().into_owned();
        let lower = name.to_ascii_lowercase();
        let Some(language) = lower
            .strip_prefix("synopsis_")
            .and_then(|rest| rest.strip_suffix(".tsv"))
        else {
            continue;
        };
        if !language.is_empty()
            && language
                .chars()
                .all(|character| character.is_ascii_alphanumeric() || character == '-')
        {
            paths.insert(language.to_string(), item.path());
        }
    }
    let mut order = Vec::new();
    if let Some(language) = preferred
        .map(str::trim)
        .filter(|language| !language.is_empty())
        .map(str::to_ascii_lowercase)
    {
        order.push(language);
    }
    if !order.iter().any(|language| language == "en") {
        order.push("en".to_string());
    }
    for language in paths.keys() {
        if !order.contains(language) {
            order.push(language.clone());
        }
    }

    let mut synopsis = HashMap::new();
    for language in order {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let Some(path) = paths.get(&language) else {
            continue;
        };
        match read_tsv_controlled(path, &["#key", "synopsis"], 2, table_budget, cancelled) {
            Ok(Some(rows)) => {
                let mut values = HashMap::new();
                let mut ambiguous = HashSet::new();
                let mut malformed = None;
                for row in rows {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    if let Err(error) = validate_key(&row[0], path) {
                        malformed = Some(error);
                        break;
                    }
                    let key = fold(&row[0]);
                    if ambiguous.contains(&key) {
                        continue;
                    }
                    match values.get(&key) {
                        Some(existing) if existing != &row[1] => {
                            values.remove(&key);
                            ambiguous.insert(key);
                        }
                        Some(_) => {}
                        None => {
                            values.insert(key, row[1].clone());
                        }
                    }
                }
                if let Some(error) = malformed {
                    warnings.push(format!(
                        "{} ignored: {error}",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ));
                    continue;
                }
                if !ambiguous.is_empty() {
                    warnings.push(format!(
                        "{}: {} conflicting synopsis row{} ignored",
                        path.file_name().unwrap_or_default().to_string_lossy(),
                        ambiguous.len(),
                        if ambiguous.len() == 1 { "" } else { "s" }
                    ));
                }
                for (key, text) in values {
                    if cancelled.load(Ordering::Relaxed) {
                        return Ok(None);
                    }
                    synopsis
                        .entry(key)
                        .or_insert_with(|| (language.clone(), text));
                }
            }
            Ok(None) => return Ok(None),
            Err(error) => warnings.push(format!(
                "{} ignored: {error}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )),
        }
    }
    Ok(Some(synopsis))
}

#[derive(Default)]
struct TableBudget {
    bytes: u64,
}

#[cfg(test)]
fn read_tsv(
    path: &Path,
    header: &[&str],
    columns: usize,
    budget: &mut TableBudget,
) -> Result<Vec<Vec<String>>> {
    let cancelled = AtomicBool::new(false);
    read_tsv_controlled(path, header, columns, budget, &cancelled)?.ok_or_else(|| {
        DegaussError::unsupported(
            "Artwork Pack table",
            "table read was cancelled unexpectedly",
        )
    })
}

fn read_tsv_controlled(
    path: &Path,
    header: &[&str],
    columns: usize,
    budget: &mut TableBudget,
    cancelled: &AtomicBool,
) -> Result<Option<Vec<Vec<String>>>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let size = std::fs::metadata(path)
        .map_err(|error| DegaussError::io("reading Artwork Pack table", path, error))?
        .len();
    if size > MAX_TSV_BYTES {
        return Err(DegaussError::unsupported(
            "Artwork Pack table",
            format!(
                "{} is {size} bytes, past the {MAX_TSV_BYTES} byte limit",
                path.display()
            ),
        ));
    }
    let total = budget.bytes.checked_add(size).ok_or_else(|| {
        DegaussError::unsupported(
            "Artwork Pack tables",
            format!("{} makes the table-size total overflow", path.display()),
        )
    })?;
    if total > MAX_TOTAL_TSV_BYTES {
        return Err(DegaussError::unsupported(
            "Artwork Pack tables",
            format!(
                "{} makes the directory exceed the {MAX_TOTAL_TSV_BYTES} byte table limit",
                path.display()
            ),
        ));
    }
    budget.bytes = total;
    let file = File::open(path)
        .map_err(|error| DegaussError::io("opening Artwork Pack table", path, error))?;
    let mut reader = BufReader::new(file);
    let mut bytes = Vec::new();
    let mut rows = Vec::new();
    let mut line = 0usize;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        bytes.clear();
        let read = reader
            .read_until(b'\n', &mut bytes)
            .map_err(|error| DegaussError::io("reading Artwork Pack table", path, error))?;
        if read == 0 {
            break;
        }
        line += 1;
        if bytes.len() > MAX_LINE_BYTES {
            return Err(DegaussError::malformed(
                "Artwork Pack table",
                path,
                format!("line {line} is longer than {MAX_LINE_BYTES} bytes"),
            ));
        }
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
        }
        if bytes.last() == Some(&b'\r') {
            return Err(DegaussError::malformed(
                "Artwork Pack table",
                path,
                format!("line {line} uses CRLF rather than LF"),
            ));
        }
        let text = std::str::from_utf8(&bytes).map_err(|error| {
            DegaussError::malformed(
                "Artwork Pack table",
                path,
                format!("line {line} is not UTF-8: {error}"),
            )
        })?;
        let fields: Vec<String> = text.split('\t').map(str::to_string).collect();
        if fields.len() != columns {
            return Err(DegaussError::malformed(
                "Artwork Pack table",
                path,
                format!(
                    "line {line} has {} columns, expected {columns}",
                    fields.len()
                ),
            ));
        }
        if line == 1 {
            if fields.iter().map(String::as_str).ne(header.iter().copied()) {
                return Err(DegaussError::malformed(
                    "Artwork Pack table",
                    path,
                    format!("header is {:?}, expected {header:?}", fields),
                ));
            }
            continue;
        }
        for (column, field) in fields.iter().enumerate() {
            validate_field(field, &format!("column {}", column + 1), path)?;
        }
        if rows.len() >= MAX_ROWS {
            return Err(DegaussError::unsupported(
                "Artwork Pack table",
                format!("{} has more than {MAX_ROWS} rows", path.display()),
            ));
        }
        rows.push(fields);
    }
    if line == 0 {
        return Err(DegaussError::malformed(
            "Artwork Pack table",
            path,
            "file is empty",
        ));
    }
    Ok(Some(rows))
}

fn validate_key(key: &str, origin: &Path) -> Result<()> {
    validate_value(key, "key", origin)?;
    let path = Path::new(key);
    if path.is_absolute()
        || path.components().count() != 1
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || key.contains(['/', '\\', '\0'])
        || matches!(key, "." | "..")
    {
        return Err(DegaussError::malformed(
            "Artwork Pack key",
            origin,
            format!("unsafe key {key:?}"),
        ));
    }
    Ok(())
}

fn validate_value(value: &str, field: &str, origin: &Path) -> Result<()> {
    if value.len() > MAX_VALUE_BYTES {
        return Err(DegaussError::malformed(
            "Artwork Pack table",
            origin,
            format!("{field} is longer than {MAX_VALUE_BYTES} bytes"),
        ));
    }
    validate_field(value, field, origin)
}

fn validate_field(value: &str, field: &str, origin: &Path) -> Result<()> {
    if value.contains('\0') {
        return Err(DegaussError::malformed(
            "Artwork Pack table",
            origin,
            format!("{field} contains NUL"),
        ));
    }
    Ok(())
}

fn present(value: &str) -> Option<String> {
    (!value.is_empty()).then(|| value.to_string())
}

fn fold(value: &str) -> String {
    value.to_lowercase()
}

fn bare_title(value: &str) -> String {
    let mut output = String::new();
    let mut depth = 0usize;
    for character in value.chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            _ if depth == 0 => output.push(character),
            _ => {}
        }
    }
    output.trim().to_string()
}

fn trailing_parenthesized(value: &str) -> Option<&str> {
    let trimmed = value.trim_end();
    let before = trimmed.strip_suffix(')')?;
    let start = before.rfind('(')?;
    let inside = before.get(start + 1..)?.trim();
    (!inside.is_empty()).then_some(inside)
}

fn first_line(value: &str) -> String {
    const ROOM: usize = 160;
    let line = value
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.chars().count() > ROOM {
        let kept: String = line.chars().take(ROOM).collect();
        format!("{}...", kept.trim_end())
    } else {
        line.to_string()
    }
}

fn identity_for_launch_controlled(
    launch: &Launch,
    cancelled: &AtomicBool,
    archives: &mut crate::zip::ArchiveCache,
) -> Result<Option<GameIdentity>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    match launch {
        Launch::AmigaVision { title, .. } => Ok(Some(GameIdentity {
            name: title.clone(),
            setname: None,
            crc32: None,
            size: None,
            extension: None,
            hash_path: None,
        })),
        Launch::File(path) => {
            identity_for_path_redirected(path, cancelled, 0, &mut HashSet::new(), archives)
        }
    }
}

#[cfg(test)]
fn identity_for_path(path: &Path) -> Result<Option<GameIdentity>> {
    identity_for_path_controlled(path, &AtomicBool::new(false))
}

#[cfg(test)]
fn identity_for_path_controlled(
    path: &Path,
    cancelled: &AtomicBool,
) -> Result<Option<GameIdentity>> {
    identity_for_path_redirected(
        path,
        cancelled,
        0,
        &mut HashSet::new(),
        &mut crate::zip::ArchiveCache::default(),
    )
}

fn identity_for_path_redirected(
    path: &Path,
    cancelled: &AtomicBool,
    redirects: usize,
    visited_mgls: &mut HashSet<PathBuf>,
    archives: &mut crate::zip::ArchiveCache,
) -> Result<Option<GameIdentity>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    if let Some((archive, member)) = archive_member(path) {
        let Some(entries) = archives.read_controlled(&archive, cancelled)? else {
            return Ok(None);
        };
        let Some(entry) = entries
            .entries
            .iter()
            .find(|entry| Path::new(&entry.name) == member)
        else {
            return Ok(None);
        };
        return Ok(Some(GameIdentity {
            name: Path::new(&entry.name)
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or(&entry.name)
                .to_string(),
            setname: None,
            crc32: Some(entry.crc32),
            size: Some(entry.size),
            extension: Path::new(&entry.name)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase),
            hash_path: None,
        }));
    }

    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension == "mra" {
        let Some(setname) = xml_text(path, "setname", cancelled)? else {
            return Ok(None);
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        return Ok(Some(GameIdentity {
            name: setname.clone(),
            setname: Some(setname),
            crc32: None,
            size: None,
            extension: Some(extension),
            hash_path: None,
        }));
    }
    if extension == "mgl" {
        if redirects >= MAX_MGL_REDIRECTS {
            return Err(DegaussError::malformed(
                "Artwork Pack identity MGL",
                path,
                format!("redirect chain exceeds {MAX_MGL_REDIRECTS} files"),
            ));
        }
        let identity = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if !visited_mgls.insert(identity) {
            return Err(DegaussError::malformed(
                "Artwork Pack identity MGL",
                path,
                "redirect chain contains a cycle",
            ));
        }
        let Some(target) = crate::favorites::mgl_target(path)? else {
            return Ok(None);
        };
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        return identity_for_path_redirected(
            &target,
            cancelled,
            redirects + 1,
            visited_mgls,
            archives,
        );
    }
    let name = path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .or_else(|| path.file_name().and_then(|name| name.to_str()))
        .ok_or_else(|| {
            DegaussError::unsupported(
                "Artwork Pack identity",
                format!("{} is not valid UTF-8", path.display()),
            )
        })?
        .to_string();
    Ok(Some(GameIdentity {
        name,
        setname: None,
        crc32: None,
        size: None,
        extension: (!extension.is_empty()).then_some(extension),
        hash_path: Some(path.to_path_buf()),
    }))
}

fn archive_member(path: &Path) -> Option<(PathBuf, PathBuf)> {
    let components: Vec<Component<'_>> = path.components().collect();
    let index = components.iter().position(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .to_ascii_lowercase()
            .ends_with(".zip")
    })?;
    if index + 1 >= components.len() {
        return None;
    }
    let archive: PathBuf = components[..=index].iter().collect();
    let member: PathBuf = components[index + 1..].iter().collect();
    Some((archive, member))
}

fn crc32_if_eligible(
    path: &Path,
    cancelled: &AtomicBool,
    on_bytes: &mut dyn FnMut(u64),
) -> Result<Option<(u32, std::fs::Metadata)>> {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    for attempt in 0..2 {
        let before = std::fs::metadata(path)
            .map_err(|error| DegaussError::io("reading game before hashing", path, error))?;
        if !before.is_file() || hashing_excluded(&extension) || before.len() > MAX_HASH_BYTES {
            return Ok(None);
        }
        let mut file = File::open(path)
            .map_err(|error| DegaussError::io("hashing game for Artwork Pack", path, error))?;
        let mut hasher = crc32fast::Hasher::new();
        let mut buffer = vec![0; HASH_BUFFER_BYTES];
        loop {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            let read = file
                .read(&mut buffer)
                .map_err(|error| DegaussError::io("hashing game for Artwork Pack", path, error))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
            on_bytes(read as u64);
        }
        let after = std::fs::metadata(path)
            .map_err(|error| DegaussError::io("reading game after hashing", path, error))?;
        if same_file_snapshot(&before, &after) {
            return Ok(Some((hasher.finalize(), after)));
        }
        if attempt == 1 {
            return Err(DegaussError::unsupported(
                "Artwork Pack fingerprint",
                format!("{} changed while its CRC was calculated", path.display()),
            ));
        }
    }
    unreachable!()
}

fn hashing_excluded(extension: &str) -> bool {
    matches!(extension, "chd" | "iso" | "bin" | "vhd" | "hdf" | "cso")
}

fn same_file_snapshot(before: &std::fs::Metadata, after: &std::fs::Metadata) -> bool {
    before.len() == after.len()
        && modified_parts(before).is_some()
        && modified_parts(before) == modified_parts(after)
}

fn modified_parts(metadata: &std::fs::Metadata) -> Option<(u64, u32)> {
    let duration = metadata
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    Some((duration.as_secs(), duration.subsec_nanos()))
}

fn fingerprint_key(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

/// Bound retained XML metadata without rejecting large embedded ROM payloads.
struct IdentityXmlInput<'a, R> {
    inner: R,
    cancelled: &'a AtomicBool,
    metadata_bytes: usize,
}

impl<R: BufRead> Read for IdentityXmlInput<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        let available = self.fill_buf()?;
        let count = output.len().min(available.len());
        output[..count].copy_from_slice(&available[..count]);
        self.consume(count);
        Ok(count)
    }
}

impl<R: BufRead> BufRead for IdentityXmlInput<'_, R> {
    fn fill_buf(&mut self) -> std::io::Result<&[u8]> {
        if self.cancelled.load(Ordering::Relaxed) {
            return Err(std::io::Error::other("game descriptor reading cancelled"));
        }
        let available = self.inner.fill_buf()?;
        let remaining = MAX_IDENTITY_XML_BYTES.saturating_sub(self.metadata_bytes);
        if remaining == 0 && !available.is_empty() {
            return Err(std::io::Error::other(format!(
                "XML metadata exceeds {MAX_IDENTITY_XML_BYTES} bytes"
            )));
        }
        Ok(&available[..available.len().min(remaining)])
    }

    fn consume(&mut self, count: usize) {
        self.metadata_bytes += count;
        self.inner.consume(count);
    }
}

fn xml_text(path: &Path, wanted: &str, cancelled: &AtomicBool) -> Result<Option<String>> {
    let file = File::open(path)
        .map_err(|error| DegaussError::io("opening game descriptor", path, error))?;
    xml_text_from_reader(BufReader::new(file), path, wanted, cancelled)
}

fn xml_text_from_reader<R: BufRead>(
    input: R,
    path: &Path,
    wanted: &str,
    cancelled: &AtomicBool,
) -> Result<Option<String>> {
    let mut reader = Reader::from_reader(IdentityXmlInput {
        inner: input,
        cancelled,
        metadata_bytes: 0,
    });
    let mut buffer = Vec::new();
    let mut active = false;
    let mut value = String::new();
    let mut embedded_payload = false;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if embedded_payload {
            // Native MRA parts, patches and cheats carry hexadecimal data. Consume
            // those bytes through quick-xml's stream API, retaining neither a
            // text event nor a copy. Leave markup and other content to the XML
            // parser, including its existing malformed-document diagnostics.
            loop {
                let available = match reader.stream().fill_buf() {
                    Ok(bytes) => bytes
                        .iter()
                        .take_while(|byte| {
                            byte.is_ascii_hexdigit() || byte.is_ascii_whitespace() || **byte == b','
                        })
                        .count(),
                    Err(_) if cancelled.load(Ordering::Relaxed) => return Ok(None),
                    Err(error) => {
                        return Err(DegaussError::io("reading game descriptor", path, error))
                    }
                };
                if available == 0 {
                    break;
                }
                reader.stream().consume(available);
                reader.get_mut().metadata_bytes -= available;
            }
            embedded_payload = false;
        }
        match reader.read_event_into(&mut buffer) {
            Err(_) if cancelled.load(Ordering::Relaxed) => return Ok(None),
            Err(error) => {
                return Err(DegaussError::malformed(
                    "game descriptor",
                    path,
                    format!("at position {}: {error}", reader.buffer_position()),
                ));
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(event)) => {
                active = event.name().as_ref().eq_ignore_ascii_case(wanted);
                embedded_payload = !active
                    && ["part", "patch", "cheat"]
                        .iter()
                        .any(|tag| event.name().as_ref().eq_ignore_ascii_case(tag));
                if active {
                    value.clear();
                }
            }
            Ok(Event::Text(text)) if active => value.push_str(&text.xml10_content()),
            Ok(Event::CData(text)) if active => {
                value.push_str(&text.into_inner());
            }
            Ok(Event::End(event))
                if active && event.name().as_ref().eq_ignore_ascii_case(wanted) =>
            {
                return Ok(Some(value.trim().to_string()));
            }
            _ => {}
        }
        buffer.clear();
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp(tag: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-artwork-pack-{tag}-{}-{:p}",
            std::process::id(),
            &tag
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn pack(tag: &str) -> (PathBuf, PathBuf) {
        let root = temp(tag).join("docs");
        let art = root.join("SuperGrafx/Artwork");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nChosen Game (USA)\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(art.join("Chosen Game (USA).jpg"), b"jpeg").unwrap();
        (root, art)
    }

    fn identity(name: &str) -> GameIdentity {
        GameIdentity {
            name: name.to_string(),
            setname: None,
            crc32: None,
            size: None,
            extension: None,
            hash_path: None,
        }
    }

    fn ready_tables(art: &Path, index_rows: &str, gameinfo_rows: &str) {
        std::fs::write(
            art.join("index.tsv"),
            format!("#name\tcrc\tsize\tkey\n{index_rows}"),
        )
        .unwrap();
        std::fs::write(
            art.join("gameinfo.tsv"),
            format!("#key\tname\tyear\tgenre\tdeveloper\tplayers\n{gameinfo_rows}"),
        )
        .unwrap();
    }

    fn ready_directory(docs: &Path, folder: &str, key: &str, alias: &str, title: &str) {
        let art = docs.join(folder).join("Artwork");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(
            art.join("manifest.tsv"),
            format!("#key\tstyle\tss_system_id\n{key}\ttest-style\t1\n"),
        )
        .unwrap();
        std::fs::write(art.join(format!("{key}.jpg")), b"jpeg").unwrap();
        ready_tables(
            &art,
            &format!("{alias}\t\t\t{key}\n"),
            &format!("{key}\t{title}\t1990\tTest\tStudio\t1\n"),
        );
    }

    #[test]
    fn exact_and_index_matches_project_pack_only_fields() {
        let (root, art) = pack("exact-index");
        std::fs::write(
            art.join("index.tsv"),
            "#name\tcrc\tsize\tkey\nDisk Name (Europe)\t1234abcd\t4\tChosen Game (USA)\n",
        )
        .unwrap();
        std::fs::write(
            art.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nChosen Game (USA)\tPack Name\t1991\tAction\tStudio\t1-2\n",
        )
        .unwrap();
        std::fs::write(
            art.join("synopsis_en.tsv"),
            "#key\tsynopsis\nChosen Game (USA)\tPack description\n",
        )
        .unwrap();
        let provider = Provider::load("SuperGrafx", &root, Some("it"));
        assert_eq!(
            provider.health,
            ProviderHealth::Ready,
            "{:?}",
            provider.diagnostics
        );
        let presentation = provider
            .resolve(&GameIdentity {
                name: "Disk Name (Europe)".into(),
                setname: None,
                crc32: None,
                size: None,
                extension: None,
                hash_path: None,
            })
            .unwrap();
        assert_eq!(presentation.name.as_deref(), Some("Pack Name"));
        assert_eq!(presentation.genre.as_deref(), Some("Action"));
        assert_eq!(presentation.details.publisher, "");
        assert_eq!(presentation.details.lang, "");
        assert_eq!(presentation.details.desc, "Pack description");
        assert_eq!(
            presentation.diagnostic.as_ref().unwrap().method,
            MatchMethod::IndexName
        );
    }

    #[test]
    fn gamelist_and_pack_presentations_are_strictly_exclusive_in_both_directions() {
        let base = temp("source-exclusivity");
        let games = base.join("games");
        let docs = base.join("docs");
        let art = docs.join("SuperGrafx/Artwork");
        let red = games.join("media/red.jpg");
        let blue = art.join("Disk Name.jpg");
        std::fs::create_dir_all(red.parent().unwrap()).unwrap();
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(games.join("Disk Name.sgx"), b"rom").unwrap();
        std::fs::write(&red, b"red").unwrap();
        std::fs::write(&blue, b"blue").unwrap();
        std::fs::write(art.join("Other.jpg"), b"other").unwrap();
        let gamelist = r#"<gameList><game><path>./Disk Name.sgx</path><name>Gamelist Name</name><image>./media/red.jpg</image><genre>Gamelist Genre</genre><desc>Gamelist Description</desc><publisher>Gamelist Publisher</publisher><developer>Gamelist Developer</developer><releasedate>20010102T000000</releasedate><players>4</players><lang>en</lang></game></gameList>"#;
        std::fs::write(games.join("gamelist.xml"), gamelist).unwrap();
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nDisk Name\tbox-2D\t105\nOther\tbox-2D\t105\n",
        )
        .unwrap();
        ready_tables(
            &art,
            "Disk Name\t\t\tDisk Name\n",
            "Disk Name\tPack Name\t1991\tPack Genre\tPack Developer\t1-2\n",
        );
        std::fs::write(
            art.join("synopsis_en.tsv"),
            "#key\tsynopsis\nDisk Name\tPack Description\n",
        )
        .unwrap();
        let config = crate::config::SystemConfig {
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
        let first_game = |library: &crate::browse::Library| {
            library
                .list(&library.start(), false)
                .unwrap()
                .0
                .into_iter()
                .find(|row| matches!(row.kind, Kind::Play(_)))
                .unwrap()
        };

        let gamelist_library = crate::browse::Library::open_with_names(
            &config,
            crate::browse::DisplayNames::default(),
        )
        .unwrap();
        let gamelist_row = first_game(&gamelist_library);
        assert_eq!(gamelist_row.name, "Gamelist Name");
        assert_eq!(gamelist_row.cover.as_deref(), Some(red.as_path()));
        assert_eq!(gamelist_row.genre.as_deref(), Some("Gamelist Genre"));
        assert_eq!(gamelist_row.details.desc, "Gamelist Description");
        assert_eq!(gamelist_row.details.publisher, "Gamelist Publisher");
        assert_eq!(gamelist_row.details.developer, "Gamelist Developer");
        assert_eq!(gamelist_row.details.released, "2001-01-02");
        assert_eq!(gamelist_row.details.players, "4");
        assert_eq!(gamelist_row.details.lang, "en");

        let neutral_library = crate::browse::Library::open_source_neutral(
            &config,
            crate::browse::DisplayNames::default(),
        )
        .unwrap();
        let mut pack_rows = vec![first_game(&neutral_library)];
        let provider = Provider::load("SuperGrafx", &docs, Some("en"));
        provider
            .apply_with_fingerprints(&mut pack_rows, &crate::cache::ContentFingerprints::new())
            .unwrap();
        let pack_row = &pack_rows[0];
        assert_eq!(pack_row.name, "Pack Name");
        assert_eq!(pack_row.cover.as_deref(), Some(blue.as_path()));
        assert_eq!(pack_row.genre.as_deref(), Some("Pack Genre"));
        assert_eq!(pack_row.details.desc, "Pack Description");
        assert_eq!(pack_row.details.publisher, "");
        assert_eq!(pack_row.details.developer, "Pack Developer");
        assert_eq!(pack_row.details.released, "1991");
        assert_eq!(pack_row.details.players, "1-2");
        assert_eq!(pack_row.details.lang, "");

        ready_tables(
            &art,
            "Disk Name\t\t\tDisk Name\n",
            "Disk Name\t\t1991\tPack Genre\tPack Developer\t1-2\n",
        );
        let provider = Provider::load("SuperGrafx", &docs, Some("en"));
        let mut rows = vec![first_game(&neutral_library)];
        provider
            .apply_with_fingerprints(&mut rows, &crate::cache::ContentFingerprints::new())
            .unwrap();
        assert_eq!(rows[0].name, "Disk Name");
        assert_ne!(rows[0].name, "Gamelist Name");

        std::fs::remove_file(&blue).unwrap();
        let provider = Provider::load("SuperGrafx", &docs, Some("en"));
        assert_eq!(provider.health, ProviderHealth::Degraded);
        let mut rows = vec![first_game(&neutral_library)];
        provider
            .apply_with_fingerprints(&mut rows, &crate::cache::ContentFingerprints::new())
            .unwrap();
        assert_eq!(rows[0].cover, None);
        assert_ne!(rows[0].cover, gamelist_row.cover);

        std::fs::write(games.join("gamelist.xml"), "<gameList><broken>").unwrap();
        let malformed_neutral = crate::browse::Library::open_source_neutral(
            &config,
            crate::browse::DisplayNames::default(),
        )
        .unwrap();
        let mut rows = vec![first_game(&malformed_neutral)];
        provider
            .apply_with_fingerprints(&mut rows, &crate::cache::ContentFingerprints::new())
            .unwrap();
        assert_ne!(rows[0].name, "Gamelist Name");
        assert_eq!(rows[0].cover, None);

        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\n../unsafe\tbox-2D\t105\n",
        )
        .unwrap();
        let invalid = Provider::load("SuperGrafx", &docs, Some("en"));
        assert_eq!(invalid.health, ProviderHealth::Invalid);
        let mut rows = vec![first_game(&malformed_neutral)];
        assert_eq!(
            invalid
                .apply_with_fingerprints(&mut rows, &crate::cache::ContentFingerprints::new())
                .unwrap(),
            0
        );
        assert_eq!(rows[0].name, "Disk Name");
        assert_eq!(rows[0].cover, None);
        assert_eq!(rows[0].genre, None);
        assert_eq!(rows[0].details, Details::default());

        std::fs::write(games.join("gamelist.xml"), gamelist).unwrap();
        let restored = crate::browse::Library::open_with_names(
            &config,
            crate::browse::DisplayNames::default(),
        )
        .unwrap();
        let restored = first_game(&restored);
        assert_eq!(restored.name, "Gamelist Name");
        assert_eq!(restored.cover.as_deref(), Some(red.as_path()));
        assert_eq!(restored.genre.as_deref(), Some("Gamelist Genre"));
        assert_eq!(restored.details, gamelist_row.details);
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn missing_index_is_degraded_but_exact_key_still_works() {
        let (root, _) = pack("degraded");
        let provider = Provider::load("SuperGrafx", &root, Some("en"));
        assert_eq!(provider.health, ProviderHealth::Degraded);
        assert!(provider
            .resolve(&GameIdentity {
                name: "Chosen Game (USA)".into(),
                setname: None,
                crc32: None,
                size: None,
                extension: None,
                hash_path: None,
            })
            .unwrap()
            .cover
            .is_some());
    }

    #[test]
    fn unsafe_manifest_key_invalidates_the_pack() {
        let (root, art) = pack("unsafe");
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\n../escape\tbox-2D\t105\n",
        )
        .unwrap();
        let provider = Provider::load("SuperGrafx", &root, None);
        assert_eq!(provider.health, ProviderHealth::Invalid);
        assert!(!provider.health.usable());
    }

    #[test]
    fn shared_source_group_is_only_neogeo_pair() {
        assert_eq!(source_group("NeoGeo"), Some("NeoGeo"));
        assert_eq!(source_group("NeoGeoMVS"), Some("NeoGeo"));
        assert_ne!(source_group("Gameboy"), source_group("Gameboy2P"));
        assert!(!supports("C64"));
    }

    #[test]
    fn candidate_discovery_finds_and_normalizes_a_configured_mount_pack() {
        let mount = temp("discover-configured-mount");
        let games = mount.join("games");
        let artwork = mount.join("docs/SuperGrafx/Artwork");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&artwork).unwrap();
        let roots = discover_candidate_roots("SuperGrafx", &[games.to_string_lossy().into_owned()]);
        let docs = std::fs::canonicalize(mount.join("docs")).unwrap();
        assert!(roots.contains(&docs));
        assert_eq!(normalize_docs_root("SuperGrafx", &artwork), Some(docs));
        std::fs::remove_dir_all(mount).ok();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_rejects_a_docs_symlink_outside_its_mount() {
        use std::os::unix::fs::symlink;

        let root = temp("discover-symlink-escape");
        let mount = root.join("mount");
        let outside = root.join("outside/docs");
        let games = mount.join("games");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(outside.join("SuperGrafx/Artwork")).unwrap();
        symlink(&outside, mount.join("docs")).unwrap();

        assert!(
            discover_candidate_roots("SuperGrafx", &[games.to_string_lossy().into_owned()])
                .is_empty()
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn archive_identity_uses_central_directory_crc_and_size() {
        let archive = temp("zip").join("games.zip");
        std::fs::write(&archive, crate::zip::tests_fixture()).unwrap();
        let identity = identity_for_path(&archive.join("Metal Slug.neo"))
            .unwrap()
            .unwrap();
        assert_eq!(identity.name, "Metal Slug");
        assert_eq!(identity.crc32, Some(0xb5348fd2));
        assert_eq!(identity.size, Some(300));
    }

    #[test]
    fn synopsis_falls_back_per_key_in_deterministic_order() {
        let (root, art) = pack("synopsis-fallback");
        std::fs::write(
            art.join("synopsis_it.tsv"),
            "#key\tsynopsis\nOther\tItalian only for another key\n",
        )
        .unwrap();
        std::fs::write(
            art.join("synopsis_en.tsv"),
            "#key\tsynopsis\nChosen Game (USA)\tEnglish fallback\n",
        )
        .unwrap();
        let provider = Provider::load("SuperGrafx", &root, Some("it"));
        let presentation = provider
            .resolve(&GameIdentity {
                name: "Chosen Game (USA)".into(),
                setname: None,
                crc32: None,
                size: None,
                extension: None,
                hash_path: None,
            })
            .unwrap();
        assert_eq!(presentation.details.desc, "English fallback");
        assert_eq!(
            presentation
                .diagnostic
                .unwrap()
                .synopsis_language
                .as_deref(),
            Some("en")
        );
    }

    #[test]
    fn synopsis_uses_preferred_english_then_alphabetical_installed_language_per_key() {
        let (root, art) = pack("synopsis-complete-order");
        ready_tables(&art, "", "");
        std::fs::write(
            art.join("synopsis_it.tsv"),
            "#key\tsynopsis\nOther\tItalian for another game\n",
        )
        .unwrap();
        std::fs::write(
            art.join("synopsis_en.tsv"),
            "#key\tsynopsis\nOther\tEnglish for another game\n",
        )
        .unwrap();
        std::fs::write(
            art.join("synopsis_fr.tsv"),
            "#key\tsynopsis\nChosen Game (USA)\tFrench fallback\n",
        )
        .unwrap();
        std::fs::write(
            art.join("synopsis_de.tsv"),
            "#key\tsynopsis\nChosen Game (USA)\tGerman alphabetical fallback\n",
        )
        .unwrap();

        let provider = Provider::load("SuperGrafx", &root, Some("it"));
        let presentation = provider.resolve(&identity("Chosen Game (USA)")).unwrap();
        assert_eq!(presentation.details.desc, "German alphabetical fallback");
        assert_eq!(
            presentation
                .diagnostic
                .unwrap()
                .synopsis_language
                .as_deref(),
            Some("de")
        );
        assert_eq!(
            provider.resolve(&identity("No Such Key")),
            None,
            "absence after every installed language is a normal lookup miss"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn synopsis_text_is_not_rejected_by_the_short_key_and_name_limit() {
        let (root, art) = pack("long-synopsis");
        ready_tables(&art, "", "");
        let synopsis = "Long description ".repeat(100);
        assert!(synopsis.len() > MAX_VALUE_BYTES);
        std::fs::write(
            art.join("synopsis_en.tsv"),
            format!("#key\tsynopsis\nChosen Game (USA)\t{synopsis}\n"),
        )
        .unwrap();

        let provider = Provider::load("SuperGrafx", &root, Some("en"));
        assert_eq!(provider.health, ProviderHealth::Ready);
        let presentation = provider.resolve(&identity("Chosen Game (USA)")).unwrap();
        assert!(presentation.details.desc.starts_with("Long description"));
        assert!(presentation.details.desc.ends_with("..."));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn resolver_uses_every_upstream_match_step_in_order() {
        let (root, art) = pack("match-ladder");
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nExact Key\tbox-2D\t105\nIndexed Key\tbox-2D\t105\nSetKey\tbox-2D\t105\nCRC Key\tbox-2D\t105\nBare Game (USA)\tbox-2D\t105\n",
        )
        .unwrap();
        for key in [
            "Exact Key",
            "Indexed Key",
            "SetKey",
            "CRC Key",
            "Bare Game (USA)",
        ] {
            std::fs::write(art.join(format!("{key}.jpg")), b"jpeg").unwrap();
        }
        ready_tables(
            &art,
            "Alias Name\t\t\tIndexed Key\nCRC Source\t89abcdef\t123\tCRC Key\nBare Game (USA)\t\t\tBare Game (USA)\n",
            "",
        );
        let provider = Provider::load("SuperGrafx", &root, None);
        assert_eq!(
            provider.health,
            ProviderHealth::Ready,
            "{:?}",
            provider.diagnostics
        );

        let cases = [
            (identity("Exact Key"), MatchMethod::ExactKey),
            (identity("alias name"), MatchMethod::IndexName),
            (
                identity("Unrelated display title (SetKey)"),
                MatchMethod::TrailingSetname,
            ),
            (
                GameIdentity {
                    crc32: Some(0x89ab_cdef),
                    size: Some(123),
                    ..identity("Unmatched dump")
                },
                MatchMethod::Crc32AndSize,
            ),
            (identity("Bare Game (Europe)"), MatchMethod::UniqueBareTitle),
        ];
        for (identity, expected) in cases {
            assert_eq!(
                provider
                    .resolve(&identity)
                    .and_then(|presentation| presentation.diagnostic)
                    .map(|diagnostic| diagnostic.method),
                Some(expected)
            );
        }
        assert!(provider
            .resolve(&GameIdentity {
                crc32: Some(0x89ab_cdef),
                size: Some(124),
                ..identity("Unmatched dump")
            })
            .is_none());
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn ambiguous_name_crc_and_bare_title_are_never_guessed() {
        let (root, art) = pack("ambiguous-lookups");
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nFirst Key\tbox-2D\t105\nSecond Key\tbox-2D\t105\n",
        )
        .unwrap();
        for key in ["First Key", "Second Key"] {
            std::fs::write(art.join(format!("{key}.jpg")), b"jpeg").unwrap();
        }
        ready_tables(
            &art,
            "Same Alias\t11111111\t4\tFirst Key\nSame Alias\t11111111\t4\tSecond Key\nShared (USA)\t\t\tFirst Key\nShared (Europe)\t\t\tSecond Key\n",
            "",
        );
        let provider = Provider::load("SuperGrafx", &root, None);
        assert_eq!(provider.health, ProviderHealth::Degraded);
        assert!(provider.resolve(&identity("Same Alias")).is_none());
        assert!(provider.resolve(&identity("Shared (Japan)")).is_none());
        assert!(provider
            .resolve(&GameIdentity {
                crc32: Some(0x1111_1111),
                size: Some(4),
                ..identity("No name match")
            })
            .is_none());
        assert!(provider
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("conflicting index name")));
        assert!(provider
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("conflicting CRC-and-size")));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn presentation_never_mixes_fields_from_different_keys() {
        let (root, art) = pack("exclusive-fields");
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nImage Only\tbox-2D\t105\n",
        )
        .unwrap();
        std::fs::write(art.join("Image Only.jpg"), b"jpeg").unwrap();
        ready_tables(
            &art,
            "Metadata Alias\t\t\tImage Only\n",
            "Metadata Alias\tMetadata Name\t1999\tPuzzle\tMetadata Studio\t2\nMetadata Only\tOnly Metadata Name\t2000\tStrategy\tOther Studio\t1\n",
        );
        let provider = Provider::load("SuperGrafx", &root, None);
        let presentation = provider.resolve(&identity("Metadata Alias")).unwrap();
        assert!(presentation.cover.is_some());
        assert_eq!(presentation.name, None);
        assert_eq!(presentation.genre, None);
        assert_eq!(presentation.details, Details::default());

        let metadata = provider.resolve(&identity("Metadata Only")).unwrap();
        assert_eq!(metadata.cover, None);
        assert_eq!(metadata.name.as_deref(), Some("Only Metadata Name"));
        assert_eq!(metadata.genre.as_deref(), Some("Strategy"));
        assert_eq!(metadata.details.publisher, "");
        assert_eq!(metadata.details.lang, "");
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn conflicting_gameinfo_rows_remove_only_that_keys_metadata() {
        let (root, art) = pack("gameinfo-conflict");
        ready_tables(
            &art,
            "",
            "Chosen Game (USA)\tFirst Name\t1990\tAction\tFirst Studio\t1\nChosen Game (USA)\tSecond Name\t1991\tPuzzle\tSecond Studio\t2\n",
        );

        let provider = Provider::load("SuperGrafx", &root, None);
        assert_eq!(provider.health, ProviderHealth::Degraded);
        let presentation = provider.resolve(&identity("Chosen Game (USA)")).unwrap();
        assert!(presentation.cover.is_some());
        assert_eq!(presentation.name, None);
        assert_eq!(presentation.genre, None);
        assert_eq!(presentation.details, Details::default());
        assert!(provider
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("conflicting gameinfo rows")));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn optional_ordered_sibling_health_matches_the_contract() {
        let base = temp("ordered-health");
        let docs = base.join("docs");
        let primary = docs.join("GAMEBOY/Artwork");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::write(
            primary.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nPrimary\tbox-2D\t9\n",
        )
        .unwrap();
        std::fs::write(primary.join("Primary.jpg"), b"jpeg").unwrap();
        ready_tables(&primary, "", "");
        assert_eq!(
            Provider::load("Gameboy", &docs, None).health,
            ProviderHealth::Ready,
            "an absent optional sibling is normal"
        );

        let secondary = docs.join("GBC/Artwork");
        std::fs::create_dir_all(&secondary).unwrap();
        std::fs::write(secondary.join("manifest.tsv"), "bad header\n").unwrap();
        assert_eq!(
            Provider::load("Gameboy", &docs, None).health,
            ProviderHealth::Degraded,
            "a present malformed sibling degrades the usable primary"
        );

        std::fs::remove_dir_all(&primary).unwrap();
        std::fs::write(
            secondary.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nSecondary\tbox-2D\t10\n",
        )
        .unwrap();
        std::fs::write(secondary.join("Secondary.jpg"), b"jpeg").unwrap();
        ready_tables(&secondary, "", "");
        assert_eq!(
            Provider::load("Gameboy", &docs, None).health,
            ProviderHealth::Degraded,
            "a valid secondary cannot make a missing primary Ready"
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn provider_snapshot_reuse_stops_when_a_table_or_image_listing_changes() {
        let (root, art) = pack("snapshot-current");
        ready_tables(&art, "", "");
        let provider = Provider::load("SuperGrafx", &root, Some("EN"));
        assert!(provider.still_current(&root, Some("en")));
        assert!(!provider.still_current(&root, Some("it")));

        std::fs::write(
            art.join("gameinfo.tsv"),
            "#key\tname\tyear\tgenre\tdeveloper\tplayers\nChosen Game (USA)\tChanged title\t1991\tAction\tStudio\t1\n",
        )
        .unwrap();
        assert!(!provider.still_current(&root, Some("en")));

        let mut provider = Provider::load("SuperGrafx", &root, Some("en"));
        assert!(provider.still_current(&root, Some("en")));
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nChosen Game (USA)\ta-replacement-style\t105\n",
        )
        .unwrap();
        let current_directory_modified = modified_nanos(&std::fs::metadata(&art).unwrap());
        let current_manifest_modified =
            modified_nanos(&std::fs::metadata(art.join("manifest.tsv")).unwrap());
        let fingerprint = provider
            .snapshot
            .as_mut()
            .and_then(|snapshot| snapshot.0[0].1.as_mut())
            .unwrap();
        fingerprint.directory_modified = current_directory_modified;
        fingerprint
            .tables
            .iter_mut()
            .find(|(name, _, _, _)| name.eq_ignore_ascii_case("manifest.tsv"))
            .unwrap()
            .2 = current_manifest_modified;
        assert!(
            !provider.still_current(&root, Some("en")),
            "manifest identity must detect a style replacement even when FAT/exFAT metadata does not"
        );

        let provider = Provider::load("SuperGrafx", &root, Some("en"));
        assert!(provider.still_current(&root, Some("en")));
        std::fs::write(art.join("Another.jpg"), b"jpeg").unwrap();
        assert!(!provider.still_current(&root, Some("en")));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn provider_load_cancellation_is_a_distinct_terminal_not_invalid_health() {
        let (root, art) = pack("provider-cancelled");
        ready_tables(&art, "", "");
        let cancelled = AtomicBool::new(false);
        let loaded =
            Provider::load_observed_controlled("SuperGrafx", &root, None, &cancelled, |_, _| {
                cancelled.store(true, Ordering::Relaxed)
            });
        assert!(loaded.is_none());
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[cfg(unix)]
    #[test]
    fn current_snapshot_recheck_does_not_enumerate_thousands_of_image_names() {
        use std::os::unix::fs::PermissionsExt;

        let (root, art) = pack("snapshot-known-paths");
        ready_tables(&art, "", "");
        let provider = Provider::load("SuperGrafx", &root, Some("en"));

        let original_mode = std::fs::metadata(&art).unwrap().permissions().mode();
        std::fs::set_permissions(&art, std::fs::Permissions::from_mode(0o111)).unwrap();
        assert!(
            provider.still_current(&root, Some("en")),
            "rechecking a snapshot should stat its known directory and tables, not read every image name"
        );
        std::fs::set_permissions(&art, std::fs::Permissions::from_mode(original_mode)).unwrap();
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn a_pack_that_changes_during_both_reads_is_invalid_not_partly_loaded() {
        let (root, art) = pack("mid-install");
        ready_tables(&art, "", "");
        let provider = Provider::load_observed("SuperGrafx", &root, None, |attempt, docs_root| {
            let manifest = docs_root.join("SuperGrafx/Artwork/manifest.tsv");
            std::fs::write(
                manifest,
                format!(
                    "#key\tstyle\tss_system_id\nChosen Game (USA)\tchanging-style-{}\t105\n",
                    "x".repeat(attempt + 1)
                ),
            )
            .unwrap();
        });

        assert_eq!(provider.health, ProviderHealth::Invalid);
        assert!(provider.directories.is_empty());
        assert!(provider
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.contains("changed repeatedly")));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn mra_identity_uses_the_parsed_setname_not_the_display_filename() {
        let dir = temp("mra-setname");
        let mra = dir.join("Pretty Arcade Title.mra");
        std::fs::write(
            &mra,
            "<misterromdescription><setname>realparent</setname></misterromdescription>",
        )
        .unwrap();
        let identity = identity_for_path(&mra).unwrap().unwrap();
        assert_eq!(identity.name, "realparent");
        assert_eq!(identity.setname.as_deref(), Some("realparent"));
        std::fs::remove_dir_all(dir).ok();
    }

    fn embedded_mra(size: usize, setname_first: bool) -> Vec<u8> {
        let identity = "<setname>embedded</setname>";
        let mut bytes = b"<misterromdescription>".to_vec();
        if setname_first {
            bytes.extend_from_slice(identity.as_bytes());
        }
        bytes.extend_from_slice(b"<rom index=\"0\"><part>");
        bytes.resize(bytes.len() + size, b'A');
        bytes.extend_from_slice(b"</part></rom>");
        if !setname_first {
            bytes.extend_from_slice(identity.as_bytes());
        }
        bytes.extend_from_slice(b"</misterromdescription>");
        bytes
    }

    #[test]
    fn large_embedded_mra_matches_before_or_after_rom_data_and_through_mgl() {
        let dir = temp("large-embedded-mra");
        let mra = dir.join("Game.mra");
        for first in [true, false] {
            for payload in [
                MAX_IDENTITY_XML_BYTES - 1,
                MAX_IDENTITY_XML_BYTES + 1,
                3 * MAX_IDENTITY_XML_BYTES,
            ] {
                std::fs::write(&mra, embedded_mra(payload, first)).unwrap();
                let identity = identity_for_path(&mra).unwrap().unwrap();
                assert_eq!(identity.name, "embedded");
                assert_eq!(identity.setname.as_deref(), Some("embedded"));
            }
        }
        let mgl = dir.join("Favorite.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><file path=\"Game.mra\"/></mistergamedescription>",
        )
        .unwrap();
        assert_eq!(identity_for_path(&mgl).unwrap().unwrap().name, "embedded");
        // The background preparation path must also complete, even when there
        // is no matching image and it considers a fingerprint for this MRA.
        let docs = dir.join("docs");
        ready_directory(&docs, "Arcade", "other", "Other", "Other");
        let provider = Provider::load("Arcade", &docs, None);
        assert!(provider.health.usable(), "{:?}", provider.diagnostics);
        assert!(provider
            .fingerprint_for_launch(&Launch::File(mra), &AtomicBool::new(false), &mut |_| {})
            .unwrap()
            .is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn mra_native_payload_tags_accept_large_comma_separated_hex() {
        let cancelled = AtomicBool::new(false);
        for tag in ["part", "patch", "cheat"] {
            let document = format!(
                "<misterromdescription><{tag}>{}</{tag}><setname>kept</setname></misterromdescription>",
                "AA, BB\n".repeat(MAX_IDENTITY_XML_BYTES / 4)
            );
            // Small chunks force tag, payload, and comma boundaries through
            // the real buffered reader rather than a single borrowed slice.
            let reader = BufReader::with_capacity(13, std::io::Cursor::new(document));
            assert_eq!(
                xml_text_from_reader(reader, Path::new("payload.mra"), "setname", &cancelled)
                    .unwrap()
                    .as_deref(),
                Some("kept"),
                "native {tag} payload must not consume the XML metadata budget"
            );
        }
    }

    #[test]
    fn mra_identity_keeps_metadata_limits_and_existing_xml_errors() {
        let path = Path::new("descriptor.mra");
        let cancelled = AtomicBool::new(false);
        let oversized = format!(
            "<misterromdescription><setname>{}</setname></misterromdescription>",
            "x".repeat(MAX_IDENTITY_XML_BYTES + 1)
        );
        let error =
            xml_text_from_reader(std::io::Cursor::new(oversized), path, "setname", &cancelled)
                .unwrap_err();
        assert!(
            error.to_string().contains("XML metadata exceeds"),
            "{error}"
        );
        let malformed = b"<misterromdescription><rom></wrong><setname>bad</setname>";
        assert!(
            xml_text_from_reader(std::io::Cursor::new(malformed), path, "setname", &cancelled)
                .is_err()
        );
        assert_eq!(
            xml_text_from_reader(
                std::io::Cursor::new(b"<misterromdescription/>"),
                path,
                "setname",
                &cancelled
            )
            .unwrap(),
            None
        );
        // Matching never validated bytes after the first complete setname.
        assert_eq!(
            xml_text_from_reader(
                std::io::Cursor::new(b"<root><setname>  kept  </setname><broken"),
                path,
                "setname",
                &cancelled
            )
            .unwrap()
            .as_deref(),
            Some("kept")
        );
    }

    #[test]
    fn embedded_mra_stream_checks_cancellation_and_propagates_read_errors() {
        struct ControlledInput<'a> {
            bytes: std::io::Cursor<Vec<u8>>,
            cancelled: &'a AtomicBool,
            fail: bool,
        }
        impl Read for ControlledInput<'_> {
            fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
                if self.bytes.position() >= 4096 {
                    if self.fail {
                        return Err(std::io::Error::other("injected descriptor read failure"));
                    }
                    self.cancelled.store(true, Ordering::Relaxed);
                }
                self.bytes.read(output)
            }
        }
        for fail in [false, true] {
            let cancelled = AtomicBool::new(false);
            let input = ControlledInput {
                bytes: std::io::Cursor::new(embedded_mra(3 * MAX_IDENTITY_XML_BYTES, false)),
                cancelled: &cancelled,
                fail,
            };
            let result = xml_text_from_reader(
                BufReader::with_capacity(128, input),
                Path::new("embedded.mra"),
                "setname",
                &cancelled,
            );
            if fail {
                assert!(result
                    .unwrap_err()
                    .to_string()
                    .contains("injected descriptor read failure"));
            } else {
                assert_eq!(result.unwrap(), None);
                assert!(cancelled.load(Ordering::Relaxed));
            }
        }
    }

    #[test]
    fn loose_crc_is_cancellable_and_resolves_only_with_matching_size() {
        let (root, art) = pack("loose-crc");
        let rom = root.parent().unwrap().join("Different Name.pce");
        let bytes = vec![0x5a; HASH_BUFFER_BYTES * 2];
        std::fs::write(&rom, &bytes).unwrap();
        let crc = crc32fast::hash(&bytes);
        ready_tables(
            &art,
            &format!(
                "Indexed source\t{crc:08x}\t{}\tChosen Game (USA)\n",
                bytes.len()
            ),
            "",
        );
        let provider = Provider::load("SuperGrafx", &root, None);
        let launch = Launch::File(rom.clone());
        assert!(provider.presentation_for_launch(&launch).unwrap().is_none());

        let cancelled = AtomicBool::new(false);
        let fingerprint = provider
            .fingerprint_for_launch(&launch, &cancelled, &mut |_| {})
            .unwrap()
            .expect("eligible loose ROM fingerprint");
        let fingerprints = crate::cache::ContentFingerprints::from([fingerprint]);
        let cache = crate::cache::SystemCache {
            format: 0,
            folders: std::collections::BTreeMap::from([(
                "root".to_string(),
                crate::cache::Folder {
                    mtime: 0,
                    rows: vec![Row {
                        name: "Different Name".to_string(),
                        sort_key: "different name".to_string(),
                        kind: Kind::Play(launch.clone()),
                        cover: None,
                        genre: None,
                        favorite: false,
                        below: None,
                        details: Details::default(),
                    }],
                    games: 1,
                },
            )]),
        };
        assert_eq!(
            provider
                .cached_fingerprints_are_current(&cache, &fingerprints, true, &cancelled)
                .unwrap(),
            Some(true)
        );
        assert_eq!(
            provider
                .presentation_for_launch_with_fingerprints(&launch, &fingerprints)
                .unwrap()
                .and_then(|presentation| presentation.diagnostic)
                .map(|diagnostic| diagnostic.method),
            Some(MatchMethod::Crc32AndSize)
        );

        std::fs::write(&rom, [bytes.as_slice(), b"changed"].concat()).unwrap();
        assert_eq!(
            provider
                .cached_fingerprints_are_current(&cache, &fingerprints, true, &cancelled)
                .unwrap(),
            Some(false),
            "the worker must reject a persisted CRC after the source file changes"
        );

        let cancelled = AtomicBool::new(false);
        let result = provider
            .fingerprint_for_launch(&launch, &cancelled, &mut |_| {
                cancelled.store(true, Ordering::Relaxed);
            })
            .unwrap();
        assert_eq!(result, None);
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn completed_provider_clones_release_operation_archive_listings() {
        let root = temp("provider-archive-lifecycle");
        let archive = root.join("empty.zip");
        let mut bytes = vec![0; 22];
        bytes[..4].copy_from_slice(b"PK\x05\x06");
        std::fs::write(&archive, bytes).unwrap();
        let mut provider = Provider::load("NES", &root, None);
        let contents = provider
            .archive_cache
            .lock()
            .unwrap()
            .read(&archive)
            .unwrap();
        let listing = Arc::downgrade(&contents);
        drop(contents);
        let mut worker = provider.clone();

        provider.discard_catalogue();
        assert!(
            listing.upgrade().is_some(),
            "an active worker keeps its own listing"
        );
        assert!(!Arc::ptr_eq(&provider.archive_cache, &worker.archive_cache));
        worker.clear_prepared();
        assert!(
            listing.upgrade().is_none(),
            "completed clones release the previous listing"
        );

        let contents = worker.archive_cache.lock().unwrap().read(&archive).unwrap();
        let listing = Arc::downgrade(&contents);
        drop(contents);
        worker.discard_catalogue();
        assert!(
            listing.upgrade().is_none(),
            "a UI snapshot retains no archive listing"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prepared_browse_projection_does_not_reopen_game_descriptors() {
        let (root, art) = pack("prepared-descriptor");
        ready_tables(
            &art,
            "",
            "Chosen Game (USA)\tPack Name\t1991\tAction\tStudio\t1\n",
        );
        let mra = root.parent().unwrap().join("Different File.mra");
        std::fs::write(
            &mra,
            "<misterromdescription><setname>Chosen Game (USA)</setname></misterromdescription>",
        )
        .unwrap();
        let original = Row {
            name: "Different File".to_string(),
            sort_key: "different file".to_string(),
            kind: Kind::Play(Launch::File(mra.clone())),
            cover: None,
            genre: None,
            favorite: false,
            below: None,
            details: Details::default(),
        };
        let cache = crate::cache::SystemCache {
            format: 0,
            folders: std::collections::BTreeMap::from([(
                "root".to_string(),
                crate::cache::Folder {
                    mtime: 0,
                    rows: vec![original.clone()],
                    games: 1,
                },
            )]),
        };
        let mut provider = Provider::load("SuperGrafx", &root, None);
        provider
            .prepare_for_cache(
                &cache,
                &crate::cache::ContentFingerprints::new(),
                &AtomicBool::new(false),
            )
            .unwrap()
            .expect("provider preparation completes");
        assert!(provider.catalogue_available());
        provider.discard_catalogue();
        assert!(
            !provider.catalogue_available(),
            "the UI snapshot must not retain the Pack lookup catalogue"
        );

        std::fs::remove_file(&mra).unwrap();
        let mut rows = vec![original];
        assert_eq!(provider.apply_prepared(&mut rows), 1);
        assert_eq!(rows[0].name, "Pack Name");
        assert_eq!(
            rows[0].cover.as_deref(),
            Some(art.join("Chosen Game (USA).jpg").as_path())
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn bounded_tsv_parser_rejects_each_malformed_shape() {
        let dir = temp("bounded-tsv");
        let cases: &[(&str, &[u8], &str)] = &[
            ("header.tsv", b"key\nvalue\n", "header"),
            ("columns.tsv", b"#key\nvalue\textra\n", "columns"),
            ("utf8.tsv", b"#key\n\xff\n", "UTF-8"),
            ("crlf.tsv", b"#key\r\nvalue\r\n", "CRLF"),
        ];
        for (name, bytes, expected) in cases {
            let path = dir.join(name);
            std::fs::write(&path, bytes).unwrap();
            let error = read_tsv(&path, &["#key"], 1, &mut TableBudget::default())
                .expect_err("malformed table must fail");
            assert!(error.to_string().contains(expected), "{error}");
        }

        let value = dir.join("value.tsv");
        std::fs::write(
            &value,
            format!("#key\n{}\n", "x".repeat(MAX_VALUE_BYTES + 1)),
        )
        .unwrap();
        let rows = read_tsv(&value, &["#key"], 1, &mut TableBudget::default()).unwrap();
        assert!(validate_value(&rows[0][0], "key", &value)
            .unwrap_err()
            .to_string()
            .contains("longer than"));

        let line = dir.join("line.tsv");
        let mut bytes = b"#key\n".to_vec();
        bytes.resize(bytes.len() + MAX_LINE_BYTES + 1, b'x');
        bytes.push(b'\n');
        std::fs::write(&line, bytes).unwrap();
        assert!(read_tsv(&line, &["#key"], 1, &mut TableBudget::default())
            .unwrap_err()
            .to_string()
            .contains("line 2 is longer"));

        let oversized = dir.join("oversized.tsv");
        let file = File::create(&oversized).unwrap();
        file.set_len(MAX_TSV_BYTES + 1).unwrap();
        assert!(
            read_tsv(&oversized, &["#key"], 1, &mut TableBudget::default())
                .unwrap_err()
                .to_string()
                .contains("past the")
        );

        let total = dir.join("total.tsv");
        std::fs::write(&total, "#key\n").unwrap();
        let mut budget = TableBudget {
            bytes: MAX_TOTAL_TSV_BYTES,
        };
        assert!(read_tsv(&total, &["#key"], 1, &mut budget)
            .unwrap_err()
            .to_string()
            .contains("table limit"));

        let rows_limit = dir.join("rows.tsv");
        let mut rows = String::with_capacity((MAX_ROWS + 1) * 2 + 5);
        rows.push_str("#key\n");
        for _ in 0..=MAX_ROWS {
            rows.push_str("x\n");
        }
        std::fs::write(&rows_limit, rows).unwrap();
        assert!(
            read_tsv(&rows_limit, &["#key"], 1, &mut TableBudget::default())
                .unwrap_err()
                .to_string()
                .contains("more than 250000 rows")
        );

        assert!(validate_jpeg_count(MAX_JPEGS, &dir).is_ok());
        assert!(validate_jpeg_count(MAX_JPEGS + 1, &dir)
            .unwrap_err()
            .to_string()
            .contains("more than 100000 JPEGs"));
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn production_scale_tables_and_image_listing_load_as_one_bounded_snapshot() {
        let root = temp("production-scale").join("docs");
        let art = root.join("SuperGrafx/Artwork");
        std::fs::create_dir_all(&art).unwrap();

        let mut manifest = std::io::BufWriter::new(File::create(art.join("manifest.tsv")).unwrap());
        writeln!(manifest, "#key\tstyle\tss_system_id").unwrap();
        for number in 0..5_000 {
            let key = format!("Key {number:05}");
            writeln!(manifest, "{key}\tbox-2D\t105").unwrap();
            std::fs::write(art.join(format!("{key}.jpg")), b"jpeg").unwrap();
        }
        manifest.flush().unwrap();

        let mut index = std::io::BufWriter::new(File::create(art.join("index.tsv")).unwrap());
        writeln!(index, "#name\tcrc\tsize\tkey").unwrap();
        for number in 0..25_000 {
            writeln!(index, "Alias {number:05}\t\t\tKey {:05}", number % 5_000).unwrap();
        }
        index.flush().unwrap();

        let mut gameinfo = std::io::BufWriter::new(File::create(art.join("gameinfo.tsv")).unwrap());
        writeln!(gameinfo, "#key\tname\tyear\tgenre\tdeveloper\tplayers").unwrap();
        for number in 0..10_000 {
            writeln!(
                gameinfo,
                "Key {number:05}\tTitle {number:05}\t1990\tTest\tStudio\t1"
            )
            .unwrap();
        }
        gameinfo.flush().unwrap();

        for language in ["de", "en", "es", "fr", "it", "pt"] {
            let mut synopsis = std::io::BufWriter::new(
                File::create(art.join(format!("synopsis_{language}.tsv"))).unwrap(),
            );
            writeln!(synopsis, "#key\tsynopsis").unwrap();
            for number in 0..10_000 {
                writeln!(
                    synopsis,
                    "Key {number:05}\t{language} synopsis for {number:05}"
                )
                .unwrap();
            }
            synopsis.flush().unwrap();
        }

        let provider = Provider::load("SuperGrafx", &root, Some("en"));
        assert_eq!(
            provider.health,
            ProviderHealth::Ready,
            "{:?}",
            provider.diagnostics
        );
        let image_match = provider.resolve(&identity("Alias 24999")).unwrap();
        assert_eq!(image_match.name.as_deref(), Some("Title 04999"));
        assert_eq!(
            image_match.cover.as_deref(),
            Some(art.join("Key 04999.jpg").as_path())
        );
        assert_eq!(image_match.details.desc, "en synopsis for 04999");
        let metadata_only = provider.resolve(&identity("Key 09999")).unwrap();
        assert_eq!(metadata_only.name.as_deref(), Some("Title 09999"));
        assert!(metadata_only.cover.is_none());
        assert_eq!(metadata_only.details.desc, "en synopsis for 09999");

        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn index_rejects_bad_crc_and_half_present_fingerprints() {
        let path = Path::new("index.tsv");
        let bad_crc = vec![vec![
            "Game".into(),
            "not-hex".into(),
            "4".into(),
            "Key".into(),
        ]];
        assert!(parse_index_rows(bad_crc, path)
            .unwrap_err()
            .to_string()
            .contains("invalid CRC"));
        let incomplete = vec![vec![
            "Game".into(),
            "12345678".into(),
            String::new(),
            "Key".into(),
        ]];
        assert!(parse_index_rows(incomplete, path)
            .unwrap_err()
            .to_string()
            .contains("both be present"));
    }

    #[test]
    fn duplicate_rows_are_coalesced_or_made_unresolvable() {
        let path = Path::new("index.tsv");
        let exact = vec![
            vec!["Alias".into(), String::new(), String::new(), "Key".into()],
            vec!["Alias".into(), String::new(), String::new(), "Key".into()],
        ];
        let (names, _, _, warnings) = parse_index_rows(exact, path).unwrap();
        assert_eq!(names.get(&"alias".to_string()), Some("key"));
        assert!(warnings.is_empty());

        let (root, art) = pack("manifest-duplicates");
        ready_tables(&art, "", "");
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nChosen Game (USA)\tbox-2D\t105\nChosen Game (USA)\tbox-2D\t105\n",
        )
        .unwrap();
        assert_eq!(
            Provider::load("SuperGrafx", &root, None).health,
            ProviderHealth::Ready
        );
        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nChosen Game (USA)\tbox-2D\t105\nChosen Game (USA)\tbox-3D\t105\n",
        )
        .unwrap();
        assert_eq!(
            Provider::load("SuperGrafx", &root, None).health,
            ProviderHealth::Invalid
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn minimum_install_and_missing_image_health_are_explicit() {
        let base = temp("health-minimum");
        let docs = base.join("docs");
        assert_eq!(
            Provider::load("SuperGrafx", &docs, None).health,
            ProviderHealth::Unavailable
        );

        let art = docs.join("SuperGrafx/Artwork");
        std::fs::create_dir_all(&art).unwrap();
        std::fs::write(art.join("Orphan.jpg"), b"jpeg").unwrap();
        assert_eq!(
            Provider::load("SuperGrafx", &docs, None).health,
            ProviderHealth::Invalid,
            "a lone JPEG is not a selectable Pack"
        );
        std::fs::remove_file(art.join("Orphan.jpg")).unwrap();
        std::fs::write(art.join("index.tsv"), "#name\tcrc\tsize\tkey\n").unwrap();
        assert_eq!(
            Provider::load("SuperGrafx", &docs, None).health,
            ProviderHealth::Invalid,
            "an orphan supplemental table is not a selectable Pack"
        );

        std::fs::write(
            art.join("manifest.tsv"),
            "#key\tstyle\tss_system_id\nPresent\tstyle\t105\nMissing\tstyle\t105\n",
        )
        .unwrap();
        std::fs::write(art.join("Present.jpg"), b"jpeg").unwrap();
        ready_tables(
            &art,
            "",
            "Present\tPresent\t1990\tTest\tStudio\t1\nMissing\tMissing\t1991\tTest\tStudio\t1\n",
        );
        let provider = Provider::load("SuperGrafx", &docs, None);
        assert_eq!(provider.health, ProviderHealth::Degraded);
        assert!(provider
            .diagnostics
            .iter()
            .any(|problem| problem.contains("manifest images are missing")));
        let missing = provider.resolve(&identity("Missing")).unwrap();
        assert_eq!(missing.cover, None);
        assert_eq!(missing.name.as_deref(), Some("Missing"));
        std::fs::remove_dir_all(base).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_jpeg_symlink_cannot_escape_the_selected_artwork_directory() {
        let (root, art) = pack("image-escape");
        ready_tables(&art, "", "");
        let image = art.join("Chosen Game (USA).jpg");
        std::fs::remove_file(&image).unwrap();
        let outside = root.parent().unwrap().join("outside.jpg");
        std::fs::write(&outside, b"jpeg").unwrap();
        std::os::unix::fs::symlink(&outside, &image).unwrap();
        let provider = Provider::load("SuperGrafx", &root, None);
        assert_eq!(provider.health, ProviderHealth::Invalid);
        assert!(provider
            .diagnostics
            .iter()
            .any(|problem| problem.contains("outside the selected Artwork directory")));
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn case_colliding_jpeg_names_are_ambiguous_on_every_host_filesystem() {
        let mut images = HashMap::new();
        assert_eq!(
            insert_image_entry(&mut images, "Game", PathBuf::from("Game.jpg")),
            None
        );
        assert_eq!(
            insert_image_entry(&mut images, "game", PathBuf::from("game.JPG")),
            Some("game".to_string())
        );
    }

    #[test]
    fn malformed_or_conflicting_synopsis_falls_back_without_cross_key_data() {
        let (root, art) = pack("synopsis-damage");
        ready_tables(
            &art,
            "",
            "Chosen Game (USA)\tPack title\t1990\tTest\tStudio\t1\n",
        );
        std::fs::write(
            art.join("synopsis_it.tsv"),
            "#key\tsynopsis\nChosen Game (USA)\tUno\nChosen Game (USA)\tDue\n",
        )
        .unwrap();
        std::fs::write(
            art.join("synopsis_en.tsv"),
            "#key\tsynopsis\nChosen Game (USA)\tEnglish fallback\n",
        )
        .unwrap();
        let provider = Provider::load("SuperGrafx", &root, Some("it"));
        assert_eq!(provider.health, ProviderHealth::Degraded);
        let presentation = provider.resolve(&identity("Chosen Game (USA)")).unwrap();
        assert_eq!(presentation.details.desc, "English fallback");
        assert_eq!(
            presentation
                .diagnostic
                .unwrap()
                .synopsis_language
                .as_deref(),
            Some("en")
        );

        std::fs::write(art.join("synopsis_it.tsv"), "wrong\theader\n").unwrap();
        let provider = Provider::load("SuperGrafx", &root, Some("it"));
        assert_eq!(provider.health, ProviderHealth::Degraded);
        assert_eq!(
            provider
                .resolve(&identity("Chosen Game (USA)"))
                .unwrap()
                .details
                .desc,
            "English fallback"
        );
        std::fs::remove_dir_all(root.parent().unwrap()).ok();
    }

    #[test]
    fn ordered_catalogues_choose_only_the_documented_first_match() {
        let base = temp("catalogue-order");
        let docs = base.join("docs");
        ready_directory(&docs, "GAMEBOY", "GB Key", "Shared Alias", "Game Boy");
        ready_directory(&docs, "GBC", "GBC Key", "Shared Alias", "Game Boy Color");
        assert_eq!(
            Provider::load("Gameboy", &docs, None)
                .resolve(&identity("Shared Alias"))
                .unwrap()
                .name
                .as_deref(),
            Some("Game Boy")
        );
        assert_eq!(
            Provider::load("GameboyColor", &docs, None)
                .resolve(&identity("Shared Alias"))
                .unwrap()
                .name
                .as_deref(),
            Some("Game Boy Color")
        );

        ready_directory(&docs, "FDS", "FDS Key", "Disk Alias", "Famicom Disk");
        ready_directory(&docs, "NES", "NES Key", "Disk Alias", "NES");
        assert_eq!(
            Provider::load("FDS", &docs, None)
                .resolve(&identity("Disk Alias"))
                .unwrap()
                .name
                .as_deref(),
            Some("Famicom Disk")
        );

        ready_directory(&docs, "SNES", "SNES Key", "Broadcast Alias", "SNES");
        ready_directory(
            &docs,
            "Satellaview",
            "BS Key",
            "Broadcast Alias",
            "Satellaview",
        );
        let snes = Provider::load("SNES", &docs, None);
        assert_eq!(
            snes.resolve(&GameIdentity {
                extension: Some("sfc".into()),
                ..identity("Broadcast Alias")
            })
            .unwrap()
            .name
            .as_deref(),
            Some("SNES")
        );
        assert_eq!(
            snes.resolve(&GameIdentity {
                extension: Some("bs".into()),
                ..identity("Broadcast Alias")
            })
            .unwrap()
            .name
            .as_deref(),
            Some("Satellaview")
        );
        std::fs::remove_dir_all(base).ok();
    }

    #[test]
    fn mgl_identity_uses_the_final_entity_aware_game_path_and_never_guesses() {
        let dir = temp("mgl-identity");
        let game = dir.join("Rock & Roll.pce");
        std::fs::write(&game, b"rom").unwrap();
        let mgl = dir.join("Favourite.mgl");
        std::fs::write(
            &mgl,
            format!(
                "<mistergamedescription><file path=\"{}\"/><file path=\"{}\"/></mistergamedescription>",
                dir.join("Companion.chd").display(),
                game.display().to_string().replace('&', "&amp;")
            ),
        )
        .unwrap();
        let parsed = identity_for_path(&mgl).unwrap().unwrap();
        assert_eq!(parsed.name, "Rock & Roll");
        assert_eq!(parsed.hash_path.as_deref(), Some(game.as_path()));

        std::fs::write(
            &mgl,
            "<mistergamedescription><file path=\"broken></mistergamedescription>",
        )
        .unwrap();
        assert!(identity_for_path(&mgl).is_err());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn mgl_identity_rejects_cycles_and_overlong_redirect_chains() {
        let dir = temp("mgl-identity-bounds");
        let cycle = dir.join("Cycle.mgl");
        std::fs::write(
            &cycle,
            format!(
                "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
                cycle.display()
            ),
        )
        .unwrap();
        assert!(identity_for_path(&cycle)
            .unwrap_err()
            .to_string()
            .contains("redirect chain contains a cycle"));

        let game = dir.join("Game.rom");
        std::fs::write(&game, b"rom").unwrap();
        let chain: Vec<PathBuf> = (0..=MAX_MGL_REDIRECTS)
            .map(|index| dir.join(format!("Redirect-{index}.mgl")))
            .collect();
        for (index, mgl) in chain.iter().enumerate() {
            let target = chain.get(index + 1).unwrap_or(&game);
            std::fs::write(
                mgl,
                format!(
                    "<mistergamedescription><file path=\"{}\"/></mistergamedescription>",
                    target.display()
                ),
            )
            .unwrap();
        }
        assert!(identity_for_path(&chain[0])
            .unwrap_err()
            .to_string()
            .contains("redirect chain exceeds 8 files"));
        std::fs::remove_dir_all(dir).ok();
    }
}
