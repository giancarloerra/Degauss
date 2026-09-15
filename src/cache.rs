//! A written-down copy of what the card holds.
//!
//! Reading a system means listing every folder under it and parsing its
//! `gamelist.xml`. On this hardware that is seconds for a large system, and
//! a walk of every system to find the ones holding nothing takes far longer
//! than a cold start should. None of it changes between one run and the
//! next unless the card does, and the card changes when somebody changes
//! it, not while a frontend is running.
//!
//! So it is written down once and read back afterwards. Deliberately NOT
//! rebuilt on its own: a frontend that decides for itself when to spend
//! twenty seconds is a frontend that stops for twenty seconds at a moment
//! of its own choosing. Rebuilding is a menu entry.
//!
//! One file per system, plus a small index. Startup reads only the index,
//! which is what makes it cheap; a system's own file is read when that
//! system is opened.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use crate::browse::{Kind, Library, Place, Row, MAX_DEPTH};
use crate::error::{DegaussError, Result};

/// Bumped when the shape of what is written changes, so an old file is
/// ignored rather than misread.
const FORMAT: u32 = 1;
const ARTWORK_PACK_FORMAT: u32 = 3;

/// What is known about every system, small enough to read at startup.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Index {
    pub format: u32,
    /// Keyed by the system's id from the table.
    pub systems: BTreeMap<String, Summary>,
}

/// What one system holds.
#[derive(Serialize, Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub games: usize,
    pub folders: usize,
}

/// Every folder of one system, as it was when this was written.
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct SystemCache {
    pub format: u32,
    /// Keyed by [`Place::key`].
    pub folders: BTreeMap<String, Folder>,
}

/// Independent envelope for source-neutral rows. Keeping it under its own
/// path and version prevents a Pack selection from changing or invalidating
/// the released gamelist cache format.
#[derive(Serialize, Deserialize, Debug, Clone)]
struct ArtworkPackCache {
    format: u32,
    cache: SystemCache,
    fingerprints: ContentFingerprints,
    /// True only when the cancellable Pack worker inspected every row in
    /// this source-neutral cache. Ordinary legacy rebuilds can preserve valid
    /// fingerprints, but cannot claim that newly added rows were covered.
    fingerprints_complete: bool,
}

pub type ContentFingerprints = BTreeMap<String, ContentFingerprint>;

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentFingerprint {
    pub size: u64,
    pub modified_seconds: u64,
    pub modified_nanos: u32,
    pub crc32: u32,
}

impl ContentFingerprint {
    /// A persisted CRC is valid only for the exact file snapshot that was
    /// hashed. The provider worker obtains this metadata while validating a
    /// complete cache before the snapshot reaches browse projection.
    pub fn matches_metadata(self, metadata: &std::fs::Metadata) -> bool {
        let Some(modified) = metadata
            .modified()
            .ok()
            .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        else {
            return false;
        };
        metadata.is_file()
            && metadata.len() == self.size
            && modified.as_secs() == self.modified_seconds
            && modified.subsec_nanos() == self.modified_nanos
    }
}

#[derive(Debug, Clone)]
pub struct ArtworkPackData {
    pub cache: SystemCache,
    pub fingerprints: ContentFingerprints,
    pub fingerprints_complete: bool,
}

#[derive(Debug, Clone)]
pub struct StagedSystemCache {
    pub id: String,
    pub cache: SystemCache,
    pub fingerprints: ContentFingerprints,
    pub fingerprints_complete: bool,
}

/// One folder's listing.
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Folder {
    /// When the folder itself last changed, seconds since the epoch.
    ///
    /// Kept so a rebuild can skip what has not moved. Not consulted while
    /// browsing: see the note at the top of this file.
    pub mtime: i64,
    pub rows: Vec<Row>,
    /// Playable things below this folder, at any depth.
    pub games: usize,
}

impl Default for Index {
    fn default() -> Self {
        Self::new()
    }
}

impl Index {
    pub fn new() -> Index {
        Index {
            format: FORMAT,
            systems: BTreeMap::new(),
        }
    }
}

impl SystemCache {
    pub fn get(&self, place: &Place) -> Option<&Folder> {
        self.folders.get(&place.key())
    }

    /// What this system holds, taken from the folder it starts in rather
    /// than by adding every folder up: the counts already run bottom-up, so
    /// the top one is the total.
    pub fn summary(&self, start: &Place) -> Summary {
        Summary {
            games: self.get(start).map(|folder| folder.games).unwrap_or(0),
            folders: self.folders.len(),
        }
    }
}

/// Where the cache lives: beside the settings, on the card, so it survives
/// a power cycle. That is the whole point of it.
pub fn dir_for(settings_path: &Path) -> PathBuf {
    settings_path
        .parent()
        .unwrap_or(Path::new("."))
        .join("cache")
}

pub fn index_path(dir: &Path) -> PathBuf {
    dir.join("index.bin")
}

/// A system's own file. The id comes from the shipped systems table and is
/// plain, but it decides a filename, so anything surprising is replaced
/// rather than trusted.
pub fn system_path(dir: &Path, id: &str) -> PathBuf {
    let safe: String = id
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    dir.join(format!("{safe}.bin"))
}

pub fn artwork_pack_system_path(dir: &Path, id: &str) -> PathBuf {
    system_path(&dir.join("artwork-pack"), id)
}

pub fn load_index(dir: &Path) -> Option<Index> {
    let bytes = std::fs::read(index_path(dir)).ok()?;
    let index: Index = postcard::from_bytes(&bytes).ok()?;
    (index.format == FORMAT).then_some(index)
}

pub fn load_system(dir: &Path, id: &str) -> Option<SystemCache> {
    let bytes = std::fs::read(system_path(dir, id)).ok()?;
    let cache: SystemCache = postcard::from_bytes(&bytes).ok()?;
    (cache.format == FORMAT).then_some(cache)
}

pub fn load_artwork_pack_system(dir: &Path, id: &str) -> Option<SystemCache> {
    load_artwork_pack_data(dir, id).map(|data| data.cache)
}

pub fn load_artwork_pack_data(dir: &Path, id: &str) -> Option<ArtworkPackData> {
    load_artwork_pack_data_checked(dir, id).ok().flatten()
}

/// The same, telling a cache that is not there from one that cannot be
/// read: where the answer decides whether a complete result exists to be
/// kept, a file that cannot be read at that moment is not a missing one.
/// A file that does not decode is no cache, as it is for every cache.
pub fn load_artwork_pack_data_checked(dir: &Path, id: &str) -> Result<Option<ArtworkPackData>> {
    Ok(read_artwork_pack_cache(dir, id)?.map(|(_, data)| data))
}

/// The same, with the cache's marker: CRC32 of the file's bytes as read,
/// the revision a prepared mapping was made against. A replaced file,
/// whoever wrote it, is a different marker, and a mapping prepared
/// against the old one says so. Hashed only here, for the readers that
/// compare or write the marker down; every other reader decodes alone.
pub fn load_artwork_pack_data_marked(
    dir: &Path,
    id: &str,
) -> Result<Option<(ArtworkPackData, u32)>> {
    Ok(read_artwork_pack_cache(dir, id)?.map(|(bytes, data)| (data, crc32fast::hash(&bytes))))
}

fn read_artwork_pack_cache(dir: &Path, id: &str) -> Result<Option<(Vec<u8>, ArtworkPackData)>> {
    let path = artwork_pack_system_path(dir, id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading the Artwork Pack cache",
                &path,
                error,
            ))
        }
    };
    let Ok(cache) = postcard::from_bytes::<ArtworkPackCache>(&bytes) else {
        return Ok(None);
    };
    if cache.format != ARTWORK_PACK_FORMAT || cache.cache.format != FORMAT {
        return Ok(None);
    }
    let data = ArtworkPackData {
        cache: cache.cache,
        fingerprints: cache.fingerprints,
        fingerprints_complete: cache.fingerprints_complete,
    };
    Ok(Some((bytes, data)))
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| DegaussError::io("making the cache folder", parent, e))?;
    }
    // Written beside and moved into place: a half-written cache that is
    // read back as if it were whole is worse than none at all.
    let temp = path.with_extension("part");
    std::fs::write(&temp, bytes).map_err(|e| DegaussError::io("writing the cache", &temp, e))?;
    if std::fs::rename(&temp, path).is_ok() {
        return Ok(());
    }
    // The card is exFAT and has been seen to refuse the move even with
    // both names in the same folder. Writing straight to the name is the
    // fallback: less safe against an interrupted write, but a file cut
    // short fails to decode and is read as no cache rather than as a
    // wrong one, which is the same outcome as not having written it.
    let outcome =
        std::fs::write(path, bytes).map_err(|e| DegaussError::io("writing the cache", path, e));
    let _ = std::fs::remove_file(&temp);
    outcome
}

pub fn save_index(dir: &Path, index: &Index) -> Result<()> {
    let bytes = postcard::to_stdvec(index)
        .map_err(|e| DegaussError::unsupported("cache", format!("writing the index: {e}")))?;
    write(&index_path(dir), &bytes)
}

#[cfg(test)]
pub fn save_system(dir: &Path, id: &str, cache: &SystemCache) -> Result<()> {
    let bytes = postcard::to_stdvec(cache)
        .map_err(|e| DegaussError::unsupported("cache", format!("writing {id}: {e}")))?;
    write(&system_path(dir, id), &bytes)
}

/// Read one whole system off the card and write down what is there.
///
/// Every folder is listed unfiltered, so what is written is the card as it
/// is and the settings decide what is shown afterwards. The count of
/// playable things under each folder is worked out on the way back up, so
/// the answer to "is there anything in here" costs a lookup rather than a
/// walk.
pub fn build_system_checked(library: &Library) -> Result<SystemCache> {
    build_system_controlled(library, &AtomicBool::new(false), &mut |_, _| {})?
        .ok_or_else(|| DegaussError::unsupported("cache build", "cancelled"))
}

#[cfg(test)]
pub fn build_system(library: &Library) -> SystemCache {
    build_system_checked(library).expect("fixture cache must build")
}

/// Build with cooperative cancellation and progress between filesystem
/// places. The callback receives completed folders and discovered games.
pub fn build_system_controlled(
    library: &Library,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(usize, usize),
) -> Result<Option<SystemCache>> {
    build_system_observed(library, cancelled, &mut |_, folders, games, starting| {
        if !starting {
            progress(folders, games);
        }
    })
}

pub fn build_system_observed(
    library: &Library,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(&Place, usize, usize, bool),
) -> Result<Option<SystemCache>> {
    let mut cache = SystemCache {
        format: FORMAT,
        folders: BTreeMap::new(),
    };
    let mut seen = Vec::new();
    let mut folders = 0usize;
    let mut games = 0usize;
    let completed = walk_controlled(
        library,
        &library.start(),
        0,
        &mut cache,
        &mut seen,
        cancelled,
        &mut folders,
        &mut games,
        progress,
    )?;
    Ok(completed.map(|_| cache))
}

#[allow(clippy::too_many_arguments)]
fn walk_controlled(
    library: &Library,
    place: &Place,
    depth: usize,
    cache: &mut SystemCache,
    seen: &mut Vec<String>,
    cancelled: &AtomicBool,
    folders_done: &mut usize,
    games_done: &mut usize,
    progress: &mut impl FnMut(&Place, usize, usize, bool),
) -> Result<Option<usize>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let key = place.key();
    if depth > MAX_DEPTH {
        return Err(DegaussError::unsupported(
            "cache traversal",
            format!(
                "{} exceeds the maximum folder depth of {MAX_DEPTH}",
                place.path().display()
            ),
        ));
    }
    if seen.contains(&key) {
        return Ok(Some(0));
    }
    seen.push(key.clone());
    progress(place, *folders_done, *games_done, true);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let (mut rows, _) = library.list(place, true)?;

    let mut games = 0;
    for row in &mut rows {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match &row.kind {
            Kind::Play(_) => {
                games += 1;
                *games_done += 1;
            }
            Kind::Enter(inner) => {
                let Some(below) = walk_controlled(
                    library,
                    &inner.clone(),
                    depth + 1,
                    cache,
                    seen,
                    cancelled,
                    folders_done,
                    games_done,
                    progress,
                )?
                else {
                    return Ok(None);
                };
                row.below = Some(below);
                games += below;
            }
        }
    }
    cache.folders.insert(
        key,
        Folder {
            mtime: mtime_of(place.path()),
            rows,
            games,
        },
    );
    *folders_done += 1;
    progress(place, *folders_done, *games_done, false);
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    Ok(Some(games))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheKind {
    Gamelist,
    ArtworkPack,
}

fn encode_for(kind: CacheKind, staged: &StagedSystemCache) -> Result<Vec<u8>> {
    let id = &staged.id;
    match kind {
        CacheKind::Gamelist => postcard::to_stdvec(&staged.cache)
            .map_err(|error| DegaussError::unsupported("cache", format!("writing {id}: {error}"))),
        CacheKind::ArtworkPack => postcard::to_stdvec(&ArtworkPackCache {
            format: ARTWORK_PACK_FORMAT,
            cache: staged.cache.clone(),
            fingerprints: staged.fingerprints.clone(),
            fingerprints_complete: staged.fingerprints_complete,
        })
        .map_err(|error| {
            DegaussError::unsupported("artwork pack cache", format!("writing {id}: {error}"))
        }),
    }
}

fn path_for_kind(dir: &Path, kind: CacheKind, id: &str) -> PathBuf {
    match kind {
        CacheKind::Gamelist => system_path(dir, id),
        CacheKind::ArtworkPack => artwork_pack_system_path(dir, id),
    }
}

fn validate_encoded(kind: CacheKind, bytes: &[u8]) -> bool {
    match kind {
        CacheKind::Gamelist => {
            postcard::from_bytes::<SystemCache>(bytes).is_ok_and(|cache| cache.format == FORMAT)
        }
        CacheKind::ArtworkPack => postcard::from_bytes::<ArtworkPackCache>(bytes)
            .is_ok_and(|cache| cache.format == ARTWORK_PACK_FORMAT && cache.cache.format == FORMAT),
    }
}

#[derive(Debug)]
struct StagedCacheFile {
    final_path: PathBuf,
    new_path: PathBuf,
    backup_path: PathBuf,
    had_old: bool,
    backup_moved: bool,
    installed: bool,
}

/// Complete, synced and decoded cache files that have not changed any live
/// cache yet. Dropping the value cleans only this transaction's staging files.
#[derive(Debug)]
pub struct PreparedCacheGroup {
    caches: Vec<StagedSystemCache>,
    /// CRC32 of each staged cache's bytes, in the order of `caches`: the
    /// marker the installed file will carry, known before it is installed.
    markers: Vec<u32>,
    files: Vec<StagedCacheFile>,
    finished: bool,
}

impl Drop for PreparedCacheGroup {
    fn drop(&mut self) {
        if !self.finished {
            let _ = restore_staged(&mut self.files);
            self.finished = true;
        }
    }
}

impl PreparedCacheGroup {
    /// Completed rows available for computing the matching index before any
    /// live file is replaced.
    pub fn caches(&self) -> &[StagedSystemCache] {
        &self.caches
    }

    /// The marker each staged cache will carry once installed, in the order
    /// of `caches`.
    pub fn markers(&self) -> &[u32] {
        &self.markers
    }

    /// Stage the matching index in the same rollback transaction as its rows.
    pub fn with_index(mut self, dir: &Path, index: &Index) -> Result<Self> {
        let bytes = postcard::to_stdvec(index)
            .map_err(|e| DegaussError::unsupported("cache index", e.to_string()))?;
        self.stage_extra_file(index_path(dir), &bytes, "index", |written| {
            postcard::from_bytes::<Index>(written).is_ok_and(|decoded| decoded.format == FORMAT)
        })?;
        Ok(self)
    }

    /// Stage one system's prepared Pack state, both files, in the same
    /// rollback transaction as its rows: the rows, the signature they were
    /// prepared against and the prepared presentation are one result, and
    /// none of them is installed without the others.
    pub fn with_pack_state(
        mut self,
        dir: &Path,
        id: &str,
        source: &[u8],
        prepared: &[u8],
    ) -> Result<Self> {
        self.stage_extra_file(
            artwork_pack_prepared_path(dir, id),
            prepared,
            "prepared",
            |written| matches!(decode_pack_prepared(written), Ok(Some(_))),
        )?;
        self.stage_extra_file(
            artwork_pack_source_path(dir, id),
            source,
            "state",
            |written| matches!(decode_pack_source(written), Ok(Some(_))),
        )?;
        Ok(self)
    }

    /// Write one more file into the transaction: created beside its final
    /// path, synced, read back and decoded before it counts. Tracked from
    /// the moment it exists, so a failure after that cleans it up with the
    /// rest.
    fn stage_extra_file(
        &mut self,
        final_path: PathBuf,
        bytes: &[u8],
        what: &str,
        valid: impl FnOnce(&[u8]) -> bool,
    ) -> Result<()> {
        if self
            .files
            .iter()
            .any(|entry| entry.final_path == final_path)
        {
            return Err(DegaussError::unsupported(
                "cache transaction",
                format!("{} already staged", final_path.display()),
            ));
        }
        // Inspect the previous file before creating one, so a failed
        // metadata check cannot leave an untracked staging file behind.
        let had_old = path_exists(&final_path)?;
        let (new_path, backup_path) = loop {
            let serial = NEXT_CACHE_TRANSACTION.fetch_add(1, Ordering::Relaxed);
            let tag = format!("degauss-{what}-{}-{serial}", std::process::id());
            let new_path = final_path.with_extension(format!("{tag}.new"));
            let backup_path = final_path.with_extension(format!("{tag}.bak"));
            if !path_exists(&new_path)? && !path_exists(&backup_path)? {
                break (new_path, backup_path);
            }
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_path)
            .map_err(|e| DegaussError::io("creating staged cache", &new_path, e))?;
        self.files.push(StagedCacheFile {
            had_old,
            final_path,
            new_path: new_path.clone(),
            backup_path,
            backup_moved: false,
            installed: false,
        });
        use std::io::Write as _;
        file.write_all(bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| DegaussError::io("writing staged cache", &new_path, e))?;
        let written = std::fs::read(&new_path)
            .map_err(|e| DegaussError::io("validating staged cache", &new_path, e))?;
        if !valid(&written) {
            return Err(DegaussError::malformed(
                "staged cache",
                &new_path,
                "the written file could not be decoded",
            ));
        }
        Ok(())
    }

    /// Replace every member of the group as one transaction. If any member
    /// fails, every earlier member is restored before the error is returned.
    /// This handles observed I/O failures, not process termination or power
    /// loss between renames: no filesystem provides one atomic rename for
    /// this group, and no restart-recovery journal is written here.
    pub fn install(mut self) -> Result<(Vec<StagedSystemCache>, Vec<String>)> {
        let install = (|| -> Result<()> {
            for entry in &mut self.files {
                if entry.had_old {
                    std::fs::rename(&entry.final_path, &entry.backup_path).map_err(|error| {
                        DegaussError::io("backing up the previous cache", &entry.final_path, error)
                    })?;
                    entry.backup_moved = true;
                } else if path_exists(&entry.final_path)? {
                    return Err(DegaussError::unsupported(
                        "cache transaction",
                        format!(
                            "{} appeared after the replacement was staged",
                            entry.final_path.display()
                        ),
                    ));
                }
                if let Err(error) = std::fs::rename(&entry.new_path, &entry.final_path) {
                    return Err(DegaussError::io(
                        "installing the staged cache",
                        &entry.final_path,
                        error,
                    ));
                }
                entry.installed = true;
            }
            Ok(())
        })();

        if let Err(error) = install {
            let restoration_problem = restore_staged(&mut self.files);
            self.finished = true;
            return match restoration_problem {
                Some(problem) => Err(DegaussError::unsupported(
                    "cache transaction",
                    format!("{error}; {problem}"),
                )),
                None => Err(error),
            };
        }

        let mut warnings = Vec::new();
        for entry in &self.files {
            if entry.had_old {
                if let Err(error) = std::fs::remove_file(&entry.backup_path) {
                    warnings.push(format!(
                        "could not remove old cache backup {}: {error}",
                        entry.backup_path.display()
                    ));
                }
            }
        }
        self.finished = true;
        Ok((std::mem::take(&mut self.caches), warnings))
    }
}

fn path_exists(path: &Path) -> Result<bool> {
    match std::fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(DegaussError::io(
            "checking cache transaction path",
            path,
            error,
        )),
    }
}

fn restore_staged(files: &mut [StagedCacheFile]) -> Option<String> {
    let mut restoration_problem = None;
    for entry in files.iter_mut().rev() {
        if entry.installed {
            if let Err(problem) = std::fs::remove_file(&entry.final_path) {
                restoration_problem = Some(format!(
                    "removing newly installed {} failed: {problem}",
                    entry.final_path.display()
                ));
                continue;
            }
            entry.installed = false;
        }
        if entry.backup_moved {
            if let Err(problem) = std::fs::rename(&entry.backup_path, &entry.final_path) {
                restoration_problem = Some(format!(
                    "restoring {} failed: {problem}",
                    entry.final_path.display()
                ));
            } else {
                entry.backup_moved = false;
            }
        }
        let _ = std::fs::remove_file(&entry.new_path);
    }
    restoration_problem
}

static NEXT_CACHE_TRANSACTION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
const CACHE_WRITE_CHUNK: usize = 128 * 1024;

/// Write, flush, sync and decode every target file without mutating any live
/// cache. Intended for the source worker so the UI thread only performs the
/// short final rename transaction.
pub fn stage_transactional(
    dir: &Path,
    kind: CacheKind,
    caches: Vec<StagedSystemCache>,
    cancelled: &AtomicBool,
) -> Result<Option<PreparedCacheGroup>> {
    stage_transactional_with_tag_source(dir, kind, caches, cancelled, || {
        let serial = NEXT_CACHE_TRANSACTION.fetch_add(1, Ordering::Relaxed);
        format!("degauss-{}-{serial}", std::process::id())
    })
}

fn stage_transactional_with_tag_source(
    dir: &Path,
    kind: CacheKind,
    caches: Vec<StagedSystemCache>,
    cancelled: &AtomicBool,
    mut next_tag: impl FnMut() -> String,
) -> Result<Option<PreparedCacheGroup>> {
    let tag = loop {
        let tag = next_tag();
        if transaction_tag_is_available(dir, kind, &caches, &tag)? {
            break tag;
        }
    };
    stage_transactional_with_tag(dir, kind, caches, cancelled, &tag)
}

fn transaction_tag_is_available(
    dir: &Path,
    kind: CacheKind,
    caches: &[StagedSystemCache],
    tag: &str,
) -> Result<bool> {
    for (position, cache) in caches.iter().enumerate() {
        let final_path = path_for_kind(dir, kind, &cache.id);
        for path in [
            final_path.with_extension(format!("{tag}-{position}.new")),
            final_path.with_extension(format!("{tag}-{position}.bak")),
        ] {
            match std::fs::symlink_metadata(&path) {
                Ok(_) => return Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(DegaussError::io(
                        "checking a cache transaction path",
                        &path,
                        error,
                    ));
                }
            }
        }
    }
    Ok(true)
}

fn stage_transactional_with_tag(
    dir: &Path,
    kind: CacheKind,
    caches: Vec<StagedSystemCache>,
    cancelled: &AtomicBool,
    tag: &str,
) -> Result<Option<PreparedCacheGroup>> {
    if caches.is_empty() {
        return Err(DegaussError::unsupported(
            "cache transaction",
            "no cache files were supplied",
        ));
    }
    let mut prepared = PreparedCacheGroup {
        caches,
        markers: Vec::new(),
        files: Vec::new(),
        finished: false,
    };
    for (position, cache) in prepared.caches.iter().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let id = &cache.id;
        let final_path = path_for_kind(dir, kind, id);
        let parent = final_path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)
            .map_err(|error| DegaussError::io("making the cache folder", parent, error))?;
        let new_path = final_path.with_extension(format!("{tag}-{position}.new"));
        let backup_path = final_path.with_extension(format!("{tag}-{position}.bak"));
        let bytes = encode_for(kind, cache)?;
        prepared.markers.push(crc32fast::hash(&bytes));
        let had_old = path_exists(&final_path)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_path)
            .map_err(|error| DegaussError::io("creating staged cache", &new_path, error))?;
        prepared.files.push(StagedCacheFile {
            had_old,
            final_path,
            new_path: new_path.clone(),
            backup_path,
            installed: false,
            backup_moved: false,
        });
        use std::io::Write as _;
        for chunk in bytes.chunks(CACHE_WRITE_CHUNK) {
            if cancelled.load(Ordering::Relaxed) {
                return Ok(None);
            }
            file.write_all(chunk)
                .map_err(|error| DegaussError::io("writing staged cache", &new_path, error))?;
        }
        file.sync_all()
            .map_err(|error| DegaussError::io("syncing staged cache", &new_path, error))?;
        drop(file);
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let written = std::fs::read(&new_path)
            .map_err(|error| DegaussError::io("validating staged cache", &new_path, error))?;
        if !validate_encoded(kind, &written) {
            return Err(DegaussError::malformed(
                "staged cache",
                &new_path,
                "the written cache could not be decoded",
            ));
        }
    }
    Ok(Some(prepared))
}

/// Convenience for callers that do not need the worker/UI staging boundary.
#[cfg(test)]
pub fn install_transactional(
    dir: &Path,
    kind: CacheKind,
    caches: &[StagedSystemCache],
) -> Result<Vec<String>> {
    let cancelled = AtomicBool::new(false);
    let prepared = stage_transactional(dir, kind, caches.to_vec(), &cancelled)?
        .ok_or_else(|| DegaussError::unsupported("cache transaction", "staging was cancelled"))?;
    prepared.install().map(|(_, warnings)| warnings)
}

/// Commit a complete system scan and its summary together. Written as
/// the ordinary system file, or as the Pack cache's rows without a
/// preparation: no fingerprints, and not complete, for a system whose
/// Pack is prepared only when its user says so.
pub fn save_system_with_index(
    dir: &Path,
    kind: CacheKind,
    id: &str,
    cache: &SystemCache,
    index: &Index,
) -> Result<Vec<String>> {
    let staged = StagedSystemCache {
        id: id.to_string(),
        cache: cache.clone(),
        fingerprints: ContentFingerprints::new(),
        fingerprints_complete: false,
    };
    stage_transactional(dir, kind, vec![staged], &AtomicBool::new(false))?
        .ok_or_else(|| DegaussError::unsupported("cache transaction", "cancelled"))?
        .with_index(dir, index)?
        .install()
        .map(|(_, warnings)| warnings)
}

/// Bumped when the shape of a state file changes. The source state's
/// version doubles as the matching policy's: a mapping prepared under an
/// older policy reads as no state, and is prepared again.
const PACK_SOURCE_FORMAT: u32 = 1;
const PACK_PREPARED_FORMAT: u32 = 1;

/// A Pack the user let Degauss prepare for one system, and what it was
/// prepared against. Written with the prepared rows, in the same
/// transaction as the source-neutral cache, so the next entry can tell
/// from stats alone whether the rows still stand.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct AcceptedSource {
    pub docs_root: String,
    /// The synopsis language the rows were prepared for, normalised.
    pub language: Option<String>,
    /// The source as it was read, or nothing when it could not be
    /// fingerprinted whole; nothing reads as changed.
    pub signature: Option<crate::artwork_pack::SourceFingerprint>,
    /// The marker of the `<id>.bin` the rows were prepared from.
    pub cache_marker: u32,
    pub health: crate::artwork_pack::ProviderHealth,
    pub diagnostics: Vec<String>,
    /// Rows the preparation left without Pack data because their own
    /// descriptor could not be read. A count only: the paths are in the
    /// log of the run that prepared them.
    pub skipped_entries: u32,
}

/// A Pack state the user chose not to prepare: Not Now for a Pack never
/// prepared, Keep Current for a change to a prepared one. Remembered so
/// the same unchanged state is not asked about at every entry.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct DeclinedSource {
    pub docs_root: String,
    pub language: Option<String>,
    pub signature: Option<crate::artwork_pack::SourceFingerprint>,
    /// The `<id>.bin` marker the decline was made against, when there was
    /// a prepared cache to keep.
    pub cache_marker: Option<u32>,
}

/// What is known about one system's Automatic or explicit Pack decision.
#[derive(Serialize, Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct PackSourceState {
    pub accepted: Option<AcceptedSource>,
    pub declined: Option<DeclinedSource>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PackSourceFile {
    format: u32,
    state: PackSourceState,
}

/// The prepared presentation of every matched row, keyed by how the row is
/// started, exactly as the worker's map is held in memory.
pub type PackPreparedMap =
    std::collections::HashMap<crate::browse::Launch, crate::artwork_pack::PackPresentation>;

#[derive(Serialize, Deserialize, Debug, Clone)]
struct PackPreparedFile {
    format: u32,
    prepared: PackPreparedMap,
}

/// `<id>.source.bin` beside `<id>.bin`: small, read at startup for every
/// Automatic member and at every entry.
pub fn artwork_pack_source_path(dir: &Path, id: &str) -> PathBuf {
    artwork_pack_system_path(dir, id).with_extension("source.bin")
}

/// `<id>.prepared.bin` beside `<id>.bin`: read only when the system is
/// entered on the unchanged path, or a favourite or the screensaver needs
/// its rows.
pub fn artwork_pack_prepared_path(dir: &Path, id: &str) -> PathBuf {
    artwork_pack_system_path(dir, id).with_extension("prepared.bin")
}

pub fn encode_pack_source(state: &PackSourceState) -> Result<Vec<u8>> {
    postcard::to_stdvec(&PackSourceFile {
        format: PACK_SOURCE_FORMAT,
        state: state.clone(),
    })
    .map_err(|error| DegaussError::unsupported("artwork pack state", error.to_string()))
}

/// The same shape as [`PackPreparedFile`], borrowed: the map handed in
/// is encoded as it is, not copied into an owned file first.
#[derive(Serialize)]
struct PackPreparedFileRef<'a> {
    format: u32,
    prepared: &'a PackPreparedMap,
}

pub fn encode_pack_prepared(prepared: &PackPreparedMap) -> Result<Vec<u8>> {
    postcard::to_stdvec(&PackPreparedFileRef {
        format: PACK_PREPARED_FORMAT,
        prepared,
    })
    .map_err(|error| DegaussError::unsupported("artwork pack state", error.to_string()))
}

/// The version written at the front of a state file, read on its own
/// before the body: a file written under another version is told apart
/// from one of this version that is cut short or damaged.
fn written_format(bytes: &[u8]) -> std::result::Result<u32, postcard::Error> {
    postcard::take_from_bytes::<u32>(bytes).map(|(format, _)| format)
}

fn decode_pack_source(
    bytes: &[u8],
) -> std::result::Result<Option<PackSourceState>, postcard::Error> {
    if written_format(bytes)? != PACK_SOURCE_FORMAT {
        return Ok(None);
    }
    let file: PackSourceFile = postcard::from_bytes(bytes)?;
    Ok(Some(file.state))
}

fn decode_pack_prepared(
    bytes: &[u8],
) -> std::result::Result<Option<PackPreparedMap>, postcard::Error> {
    if written_format(bytes)? != PACK_PREPARED_FORMAT {
        return Ok(None);
    }
    let file: PackPreparedFile = postcard::from_bytes(bytes)?;
    Ok(Some(file.prepared))
}

/// Read a state file. A missing file is the ordinary case and says
/// nothing. One that is there but cannot be read, or that carries this
/// version and does not decode (cut short, or damaged), is an error for
/// the caller to show: what was decided is unknown, and a question or a
/// preparation in its place would write over the decision. One written
/// under another version of the state or the matching policy is by
/// design read as no state and prepared again; the log says so.
fn load_state_file<T>(
    path: &Path,
    decode: impl FnOnce(&[u8]) -> std::result::Result<Option<T>, postcard::Error>,
) -> Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(DegaussError::io(
                "reading the Artwork Pack state",
                path,
                error,
            ))
        }
    };
    let decoded = decode(&bytes)
        .map_err(|error| DegaussError::malformed("Artwork Pack state", path, error.to_string()))?;
    if decoded.is_none() {
        crate::note(&format!(
            "artwork pack state {}: written under another version, read as no state, prepared again on consent",
            path.display()
        ));
    }
    Ok(decoded)
}

pub fn load_pack_source_state(dir: &Path, id: &str) -> Result<Option<PackSourceState>> {
    load_state_file(&artwork_pack_source_path(dir, id), decode_pack_source)
}

pub fn load_pack_prepared_map(dir: &Path, id: &str) -> Result<Option<PackPreparedMap>> {
    load_state_file(&artwork_pack_prepared_path(dir, id), decode_pack_prepared)
}

/// The decision file alone: a decline, or a refreshed signature, changes
/// nothing about the prepared rows.
pub fn save_pack_source_state(dir: &Path, id: &str, state: &PackSourceState) -> Result<()> {
    write(
        &artwork_pack_source_path(dir, id),
        &encode_pack_source(state)?,
    )
}

/// Both state files, the rows first: a crash between the two leaves rows
/// without a decision, which is read as no state, never a decision
/// without rows.
pub fn save_pack_state(
    dir: &Path,
    id: &str,
    state: &PackSourceState,
    prepared: &PackPreparedMap,
) -> Result<()> {
    write(
        &artwork_pack_prepared_path(dir, id),
        &encode_pack_prepared(prepared)?,
    )?;
    save_pack_source_state(dir, id, state)
}

/// When a folder itself last changed, seconds since the epoch, or 0.
///
/// A directory's own mtime moves when an entry is added or removed from it,
/// and not when something changes inside a subdirectory. That is
/// enough to skip a folder that has not moved, and not enough to notice one
/// that changed three folders down, which is why a rebuild walks rather
/// than trusting it.
pub fn mtime_of(path: &Path) -> i64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    let Ok(time) = meta.modified() else {
        return 0;
    };
    time.duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_default_index_round_trips_as_the_existing_cache_format() {
        let index = super::Index::default();
        assert_eq!(index.format, super::FORMAT);
        let bytes = postcard::to_stdvec(&index).unwrap();
        let restored: super::Index = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(restored.format, super::FORMAT);
        assert!(restored.systems.is_empty());
    }

    use super::*;
    use crate::config::SystemConfig;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("degauss-cache-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn system(dir: &Path) -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Test".into(),
            path: dir.to_string_lossy().into_owned(),
            extensions: vec!["d64".into()],
            rbf: "_Computer/Test".into(),
            launch: Vec::new(),
            setname: None,
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    #[test]
    fn the_count_under_a_folder_adds_up_what_is_below_it() {
        // Two games at the top and three more two folders down. The answer
        // to "is there anything in here" has to survive the depth, because
        // that is what decides whether the folder is shown at all.
        let dir = temp("counts");
        std::fs::create_dir_all(dir.join("USA/Good")).unwrap();
        std::fs::write(dir.join("one.d64"), b"x").unwrap();
        std::fs::write(dir.join("two.d64"), b"x").unwrap();
        for n in 0..3 {
            std::fs::write(dir.join(format!("USA/Good/g{n}.d64")), b"x").unwrap();
        }
        std::fs::create_dir_all(dir.join("Empty/AlsoEmpty")).unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let cache = build_system(&library);
        let start = library.start();

        assert_eq!(
            cache.summary(&start).games,
            5,
            "two at the top, three below"
        );

        let top = cache.get(&start).expect("the top folder");
        let usa = top.rows.iter().find(|row| row.name == "USA").expect("USA");
        assert_eq!(usa.below, Some(3));
        let empty = top
            .rows
            .iter()
            .find(|row| row.name == "Empty")
            .expect("Empty");
        assert_eq!(empty.below, Some(0), "nothing below it at any depth");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn games_inside_a_folder_full_of_pictures_are_still_counted() {
        // The DOS core keeps its games in a folder called media, which is
        // the name every other system gives its artwork. Nothing may skip a
        // folder because of what it is called.
        let dir = temp("games-in-media");
        std::fs::create_dir_all(dir.join("media/stunts")).unwrap();
        std::fs::write(dir.join("media/stunts/stunts.d64"), b"x").unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let cache = build_system(&library);
        assert_eq!(cache.summary(&library.start()).games, 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn what_is_written_down_reads_back_the_same() {
        let dir = temp("roundtrip");
        std::fs::write(dir.join("one.d64"), b"x").unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let built = build_system(&library);

        let store = temp("roundtrip-store");
        save_system(&store, "Test", &built).unwrap();
        let read = load_system(&store, "Test").expect("reads back");
        assert_eq!(read.folders.len(), built.folders.len());
        assert_eq!(
            read.summary(&library.start()).games,
            built.summary(&library.start()).games
        );

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn refreshing_one_system_leaves_the_rest_of_the_index_alone() {
        // Favouriting rewrites one system's summary in the index that is
        // already there. The refresh must never be built from Index::new(),
        // which would wipe every other system: the untouched summaries have
        // to survive the trip through the file identical.
        let store = temp("index-refresh");
        let mut index = Index::new();
        index.systems.insert(
            "A".into(),
            Summary {
                games: 3,
                folders: 1,
            },
        );
        index.systems.insert(
            "Favorites".into(),
            Summary {
                games: 0,
                folders: 1,
            },
        );
        save_index(&store, &index).unwrap();

        let mut index = load_index(&store).expect("reads back");
        index.systems.insert(
            "Favorites".into(),
            Summary {
                games: 1,
                folders: 1,
            },
        );
        save_index(&store, &index).unwrap();

        let read = load_index(&store).expect("reads back again");
        assert_eq!(
            read.systems.get("A"),
            Some(&Summary {
                games: 3,
                folders: 1
            }),
            "the system that was not touched"
        );
        assert_eq!(
            read.systems.get("Favorites"),
            Some(&Summary {
                games: 1,
                folders: 1
            }),
            "the system that was"
        );
        std::fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn a_card_that_was_never_indexed_offers_no_index_to_fold_into() {
        // A single-system refresh folds its summary into the index that
        // is already there, and refresh_system asks this exact question
        // first. With no index on disk the answer must stay None on every
        // asking: an index conjured here would reach the disk holding one
        // system, and startup trusts an index that exists, so every other
        // system would come up missing after a restart.
        let store = temp("index-none");
        assert!(load_index(&store).is_none(), "nothing to fold into");
        assert!(load_index(&store).is_none(), "and asking created nothing");
        std::fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn a_rebuilt_system_cache_counts_a_file_written_after_the_first_build() {
        // Adding a favourite is the one moment Degauss knows a folder on
        // the card changed, and the answer to that is building this one
        // system's cache again. The rebuilt cache has to see the new file,
        // or the shelf keeps showing yesterday's favourites.
        let dir = temp("rebuild-sees-more");
        std::fs::write(dir.join("one.d64"), b"x").unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let before = build_system(&library).summary(&library.start()).games;

        std::fs::write(dir.join("two.d64"), b"x").unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let after = build_system(&library).summary(&library.start()).games;

        assert_eq!(after, before + 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_file_written_by_another_version_is_ignored_rather_than_misread() {
        let store = temp("wrong-format");
        let stale = SystemCache {
            format: FORMAT + 1,
            folders: BTreeMap::new(),
        };
        save_system(&store, "Test", &stale).unwrap();
        assert!(load_system(&store, "Test").is_none());
        std::fs::remove_dir_all(&store).ok();
    }

    #[test]
    fn artwork_pack_cache_is_separate_and_keeps_only_source_neutral_data() {
        let games = temp("pack-separate-games");
        let rom = games.join("Disk Game.d64");
        std::fs::write(&rom, b"rom").unwrap();
        let library = Library::open_source_neutral(&system(&games), Default::default()).unwrap();
        let built = build_system(&library);
        let store = temp("pack-separate-store");
        save_system(&store, "Test", &built).unwrap();
        let legacy_before = std::fs::read(system_path(&store, "Test")).unwrap();
        let fingerprints = ContentFingerprints::from([(
            rom.to_string_lossy().into_owned(),
            ContentFingerprint {
                size: 3,
                modified_seconds: 1,
                modified_nanos: 2,
                crc32: 0x1234_5678,
            },
        )]);
        install_transactional(
            &store,
            CacheKind::ArtworkPack,
            &[StagedSystemCache {
                id: "Test".into(),
                cache: built.clone(),
                fingerprints: fingerprints.clone(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();

        assert_eq!(
            std::fs::read(system_path(&store, "Test")).unwrap(),
            legacy_before,
            "writing the Pack cache must not rewrite the released cache"
        );
        let read = load_artwork_pack_data(&store, "Test").expect("Pack cache reads");
        assert_eq!(read.fingerprints, fingerprints);
        assert!(read.fingerprints_complete);
        let bytes = std::fs::read(artwork_pack_system_path(&store, "Test")).unwrap();
        for forbidden in ["Transient Pack Key", "Pack synopsis", "/docs/Test/Artwork"] {
            assert!(
                !bytes
                    .windows(forbidden.len())
                    .any(|window| window == forbidden.as_bytes()),
                "Pack presentation must not be serialized: {forbidden}"
            );
        }
        std::fs::remove_dir_all(games).ok();
        std::fs::remove_dir_all(store).ok();
    }

    fn staged(id: &str) -> StagedSystemCache {
        StagedSystemCache {
            id: id.to_string(),
            cache: SystemCache {
                format: FORMAT,
                folders: BTreeMap::new(),
            },
            fingerprints: ContentFingerprints::new(),
            fingerprints_complete: true,
        }
    }

    #[test]
    fn failed_neogeo_mvs_install_restores_both_shared_group_caches() {
        let store = temp("neogeo-group-rollback");
        let first = artwork_pack_system_path(&store, "NeoGeo");
        let second = artwork_pack_system_path(&store, "NeoGeoMVS");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        std::fs::write(&first, b"old-first").unwrap();
        std::fs::write(&second, b"old-second").unwrap();

        let cancelled = AtomicBool::new(false);
        let prepared = stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("NeoGeo"), staged("NeoGeoMVS")],
            &cancelled,
            "rollback",
        )
        .unwrap()
        .unwrap();
        let first_new = prepared.files[0].new_path.clone();
        let first_backup = prepared.files[0].backup_path.clone();
        let second_new = prepared.files[1].new_path.clone();
        let blocked_backup = prepared.files[1].backup_path.clone();
        std::fs::create_dir(&blocked_backup).unwrap();

        let error = prepared
            .install()
            .expect_err("the occupied second backup path must fail the transaction");
        assert!(error.to_string().contains("backing up the previous cache"));
        assert_eq!(std::fs::read(&first).unwrap(), b"old-first");
        assert_eq!(std::fs::read(&second).unwrap(), b"old-second");
        assert!(!first_new.exists());
        assert!(!first_backup.exists());
        assert!(!second_new.exists());
        assert!(blocked_backup.is_dir());
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn failed_staging_cleans_earlier_new_files_without_touching_the_blocker() {
        let store = temp("transaction-stage-cleanup");
        let first = artwork_pack_system_path(&store, "First");
        let second = artwork_pack_system_path(&store, "Second");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        let first_new = first.with_extension("stage-cleanup-0.new");
        let second_new = second.with_extension("stage-cleanup-1.new");
        std::fs::write(&second_new, b"pre-existing blocker").unwrap();

        let cancelled = AtomicBool::new(false);
        stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("First"), staged("Second")],
            &cancelled,
            "stage-cleanup",
        )
        .expect_err("the occupied second staging path must fail");
        assert!(!first_new.exists());
        assert_eq!(std::fs::read(&second_new).unwrap(), b"pre-existing blocker");
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn a_reused_process_tag_skips_stale_new_and_backup_paths() {
        let store = temp("transaction-tag-reuse");
        let first = artwork_pack_system_path(&store, "First");
        let second = artwork_pack_system_path(&store, "Second");
        std::fs::create_dir_all(first.parent().unwrap()).unwrap();
        let stale_new = first.with_extension("reused-0.new");
        let stale_backup = second.with_extension("reused-1.bak");
        std::fs::write(&stale_new, b"interrupted replacement").unwrap();
        std::fs::write(&stale_backup, b"previous live cache").unwrap();
        let fresh_first = first.with_extension("fresh-0.new");
        let fresh_second = second.with_extension("fresh-1.new");
        let mut tags = ["reused", "fresh"].into_iter();

        let cancelled = AtomicBool::new(false);
        let prepared = stage_transactional_with_tag_source(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("First"), staged("Second")],
            &cancelled,
            || tags.next().expect("the fresh tag must be available").into(),
        )
        .unwrap()
        .unwrap();

        assert_eq!(
            std::fs::read(&stale_new).unwrap(),
            b"interrupted replacement"
        );
        assert_eq!(
            std::fs::read(&stale_backup).unwrap(),
            b"previous live cache"
        );
        assert!(fresh_first.exists());
        assert!(fresh_second.exists());
        drop(prepared);
        assert!(!fresh_first.exists());
        assert!(!fresh_second.exists());
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn staging_does_not_change_the_live_cache_before_install() {
        let store = temp("transaction-not-live-before-install");
        let final_path = artwork_pack_system_path(&store, "Test");
        std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
        std::fs::write(&final_path, b"old-complete-cache").unwrap();
        let cancelled = AtomicBool::new(false);

        let prepared = stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("Test")],
            &cancelled,
            "not-live",
        )
        .unwrap()
        .unwrap();

        assert_eq!(std::fs::read(&final_path).unwrap(), b"old-complete-cache");
        assert!(prepared.files[0].new_path.is_file());
        let (_, warnings) = prepared.install().unwrap();
        assert!(warnings.is_empty());
        assert!(load_artwork_pack_data(&store, "Test").is_some());
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn dropping_a_prepared_group_removes_only_its_staging_files() {
        let store = temp("transaction-drop-cleanup");
        let final_path = artwork_pack_system_path(&store, "Test");
        std::fs::create_dir_all(final_path.parent().unwrap()).unwrap();
        std::fs::write(&final_path, b"old-complete-cache").unwrap();
        let cancelled = AtomicBool::new(false);
        let prepared = stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("Test")],
            &cancelled,
            "drop-cleanup",
        )
        .unwrap()
        .unwrap();
        let new_path = prepared.files[0].new_path.clone();
        assert!(new_path.is_file());

        drop(prepared);

        assert!(!new_path.exists());
        assert_eq!(std::fs::read(&final_path).unwrap(), b"old-complete-cache");
        std::fs::remove_dir_all(store).ok();
    }

    #[test]
    fn cancellation_during_a_directory_walk_returns_no_partial_cache() {
        let games = temp("cancel-mid-walk");
        for folder in ["A", "B", "C"] {
            std::fs::create_dir_all(games.join(folder)).unwrap();
            std::fs::write(games.join(folder).join("Game.d64"), b"rom").unwrap();
        }
        let library = Library::open_source_neutral(&system(&games), Default::default()).unwrap();
        let cancelled = AtomicBool::new(false);

        let built = build_system_controlled(&library, &cancelled, &mut |folders, _| {
            if folders >= 1 {
                cancelled.store(true, Ordering::Relaxed);
            }
        })
        .unwrap();

        assert!(built.is_none());
        std::fs::remove_dir_all(games).ok();
    }

    #[test]
    fn folder_observation_precedes_reading_and_preserves_cache_bytes() {
        let games = temp("observed-walk");
        let child = games.join("Child");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::write(child.join("Game.d64"), b"rom").unwrap();
        let library = Library::open_source_neutral(&system(&games), Default::default()).unwrap();
        let original = build_system_checked(&library).unwrap();
        let mut events = Vec::new();
        let observed = build_system_observed(
            &library,
            &AtomicBool::new(false),
            &mut |place, folders, found, starting| {
                events.push((place.clone(), folders, found, starting));
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            postcard::to_stdvec(&original).unwrap(),
            postcard::to_stdvec(&observed).unwrap()
        );
        assert_eq!(events[0], (library.start(), 0, 0, true));
        let entered = events
            .iter()
            .position(|event| event.0 == Place::Dir(child.clone()) && event.3)
            .unwrap();
        let finished = events
            .iter()
            .position(|event| event.0 == Place::Dir(child.clone()) && !event.3)
            .unwrap();
        assert!(entered < finished);
        assert_eq!(events.last().unwrap().2, 1);
        std::fs::remove_dir_all(games).unwrap();
    }

    #[test]
    fn cancellation_from_pre_read_observer_prevents_the_read() {
        let games = temp("cancel-before-read");
        let library = Library::open_source_neutral(&system(&games), Default::default()).unwrap();
        std::fs::remove_dir(&games).unwrap();
        std::fs::write(&games, b"not a directory").unwrap();
        let cancelled = AtomicBool::new(false);
        let result = build_system_observed(&library, &cancelled, &mut |_, _, _, starting| {
            assert!(starting);
            cancelled.store(true, Ordering::Relaxed);
        });
        assert!(
            result.unwrap().is_none(),
            "cancellation must run before the failing read_dir call"
        );
        std::fs::remove_file(games).unwrap();
    }

    #[test]
    fn an_unreadable_nested_branch_fails_instead_of_becoming_an_empty_cache() {
        let games = temp("nested-read-failure");
        let not_a_directory = games.join("not-a-directory");
        std::fs::write(&not_a_directory, b"file").unwrap();
        let library = Library::open_source_neutral(&system(&games), Default::default()).unwrap();
        let cancelled = AtomicBool::new(false);
        let mut cache = SystemCache {
            format: FORMAT,
            folders: BTreeMap::new(),
        };
        let mut seen = Vec::new();
        let mut folders = 0;
        let mut games_done = 0;

        let error = walk_controlled(
            &library,
            &Place::Dir(not_a_directory),
            1,
            &mut cache,
            &mut seen,
            &cancelled,
            &mut folders,
            &mut games_done,
            &mut |_, _, _, _| {},
        )
        .unwrap_err();

        assert!(error.to_string().contains("reading folder"));
        assert!(cache.folders.is_empty());
        std::fs::remove_dir_all(games).ok();
    }

    #[test]
    fn successful_rebuild_removes_deleted_files_and_zip_members_without_touching_other_systems() {
        let games = temp("rebuild-removal-games");
        let store = temp("rebuild-removal-store");
        std::fs::write(games.join("Keep.d64"), b"rom").unwrap();
        std::fs::write(games.join("Remove.d64"), b"rom").unwrap();
        std::fs::write(
            games.join("Set.zip"),
            crate::zip::tests_archive(&["One.d64", "Two.d64"], false),
        )
        .unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let initial = build_system_checked(&library).unwrap();
        let mut index = Index::new();
        index
            .systems
            .insert("Test".into(), initial.summary(&library.start()));
        index.systems.insert(
            "Untouched".into(),
            Summary {
                games: 7,
                folders: 2,
            },
        );
        save_system_with_index(&store, CacheKind::Gamelist, "Test", &initial, &index).unwrap();
        std::fs::write(system_path(&store, "Untouched"), b"other-cache-bytes").unwrap();
        let independent = artwork_pack_system_path(&store, "Test");
        std::fs::create_dir_all(independent.parent().unwrap()).unwrap();
        std::fs::write(&independent, b"independent-pack").unwrap();
        assert_eq!(index.systems["Test"].games, 4);

        std::fs::remove_file(games.join("Remove.d64")).unwrap();
        std::fs::write(
            games.join("Set.zip"),
            crate::zip::tests_archive(&["One.d64"], false),
        )
        .unwrap();
        let rebuilt = build_system_checked(&Library::open(&system(&games)).unwrap()).unwrap();
        index
            .systems
            .insert("Test".into(), rebuilt.summary(&library.start()));
        assert!(
            save_system_with_index(&store, CacheKind::Gamelist, "Test", &rebuilt, &index)
                .unwrap()
                .is_empty()
        );
        let reloaded = load_system(&store, "Test").unwrap();
        assert_eq!(reloaded.summary(&library.start()).games, 2);
        assert_eq!(load_index(&store).unwrap().systems["Test"].games, 2);
        assert_eq!(load_index(&store).unwrap().systems["Untouched"].games, 7);
        assert_eq!(
            std::fs::read(system_path(&store, "Untouched")).unwrap(),
            b"other-cache-bytes"
        );
        assert_eq!(std::fs::read(independent).unwrap(), b"independent-pack");
        std::fs::remove_dir_all(games).unwrap();
        std::fs::remove_dir_all(store).unwrap();
    }

    #[test]
    fn malformed_nested_archive_cannot_replace_a_previous_cache_or_index() {
        let games = temp("failed-rebuild-games");
        let store = temp("failed-rebuild-store");
        std::fs::create_dir_all(games.join("Nested")).unwrap();
        let archive = games.join("Nested/Set.zip");
        std::fs::write(&archive, crate::zip::tests_archive(&["Game.d64"], false)).unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let old = build_system_checked(&library).unwrap();
        let mut index = Index::new();
        index
            .systems
            .insert("Test".into(), old.summary(&library.start()));
        save_system_with_index(&store, CacheKind::Gamelist, "Test", &old, &index).unwrap();
        let old_cache_bytes = std::fs::read(system_path(&store, "Test")).unwrap();
        let old_index_bytes = std::fs::read(index_path(&store)).unwrap();
        std::fs::write(&archive, b"broken archive").unwrap();
        let error = build_system_checked(&Library::open(&system(&games)).unwrap()).unwrap_err();
        assert!(error.to_string().contains("Set.zip"));
        assert_eq!(
            std::fs::read(system_path(&store, "Test")).unwrap(),
            old_cache_bytes
        );
        assert_eq!(std::fs::read(index_path(&store)).unwrap(), old_index_bytes);
        assert_eq!(load_index(&store).unwrap().systems["Test"].games, 1);
        std::fs::remove_dir_all(games).unwrap();
        std::fs::remove_dir_all(store).unwrap();
    }

    #[test]
    fn failed_index_install_rolls_back_the_new_rows_and_preserves_the_old_pair() {
        let store = temp("index-install-rollback");
        let old = staged("Test");
        let old_index = Index::new();
        save_system_with_index(&store, CacheKind::Gamelist, "Test", &old.cache, &old_index)
            .unwrap();
        let old_cache_bytes = std::fs::read(system_path(&store, "Test")).unwrap();
        let old_index_bytes = std::fs::read(index_path(&store)).unwrap();
        let mut next = Index::new();
        next.systems.insert(
            "Test".into(),
            Summary {
                games: 8,
                folders: 1,
            },
        );
        let mut changed = staged("Test");
        changed.cache.folders.insert(
            "changed".into(),
            Folder {
                mtime: 0,
                rows: Vec::new(),
                games: 8,
            },
        );
        let prepared = stage_transactional(
            &store,
            CacheKind::Gamelist,
            vec![changed],
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap()
        .with_index(&store, &next)
        .unwrap();
        assert_eq!(prepared.caches().len(), 1);
        // Fail the real rename after the rows have already been installed.
        let index_staging = prepared.files.last().unwrap().new_path.clone();
        std::fs::remove_file(index_staging).unwrap();
        let error = prepared.install().unwrap_err();
        assert!(error.to_string().contains("installing the staged cache"));
        assert_eq!(
            std::fs::read(system_path(&store, "Test")).unwrap(),
            old_cache_bytes
        );
        assert_eq!(std::fs::read(index_path(&store)).unwrap(), old_index_bytes);
        assert_eq!(
            std::fs::read_dir(&store).unwrap().count(),
            2,
            "only original live files remain"
        );
        std::fs::remove_dir_all(store).unwrap();
    }

    #[test]
    fn dropping_a_complete_staged_pair_preserves_live_files() {
        let store = temp("cancel-staged-pair");
        let cache = staged("Test");
        let index = Index::new();
        save_system_with_index(&store, CacheKind::Gamelist, "Test", &cache.cache, &index).unwrap();
        let old_cache_bytes = std::fs::read(system_path(&store, "Test")).unwrap();
        let old_index_bytes = std::fs::read(index_path(&store)).unwrap();
        let prepared = stage_transactional(
            &store,
            CacheKind::Gamelist,
            vec![cache],
            &AtomicBool::new(false),
        )
        .unwrap()
        .unwrap()
        .with_index(&store, &index)
        .unwrap();
        drop(prepared);
        assert_eq!(
            std::fs::read(system_path(&store, "Test")).unwrap(),
            old_cache_bytes
        );
        assert_eq!(std::fs::read(index_path(&store)).unwrap(), old_index_bytes);
        assert_eq!(std::fs::read_dir(&store).unwrap().count(), 2);
        std::fs::remove_dir_all(store).unwrap();
    }

    #[test]
    fn cancellation_at_the_final_progress_callback_still_discards_the_cache() {
        let games = temp("cancel-final-progress");
        std::fs::write(games.join("Game.d64"), b"rom").unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let cancelled = AtomicBool::new(false);
        let cache = build_system_controlled(&library, &cancelled, &mut |_, _| {
            cancelled.store(true, Ordering::Relaxed)
        })
        .unwrap();
        assert!(cache.is_none());
        std::fs::remove_dir_all(games).unwrap();
    }

    #[test]
    fn excessive_depth_is_an_error_instead_of_an_empty_subtree() {
        let games = temp("excessive-depth");
        let library = Library::open(&system(&games)).unwrap();
        let mut cache = SystemCache {
            format: FORMAT,
            folders: BTreeMap::new(),
        };
        let error = walk_controlled(
            &library,
            &library.start(),
            MAX_DEPTH + 1,
            &mut cache,
            &mut Vec::new(),
            &AtomicBool::new(false),
            &mut 0,
            &mut 0,
            &mut |_, _, _, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("maximum folder depth"));
        assert!(cache.folders.is_empty());
        std::fs::remove_dir_all(games).unwrap();
    }

    fn accepted_state(marker: u32) -> PackSourceState {
        PackSourceState {
            accepted: Some(AcceptedSource {
                docs_root: "/docs".into(),
                language: Some("en".into()),
                signature: None,
                cache_marker: marker,
                health: crate::artwork_pack::ProviderHealth::Ready,
                diagnostics: Vec::new(),
                skipped_entries: 2,
            }),
            declined: None,
        }
    }

    /// The rows, the decision and the prepared presentation of one system
    /// are one result: staged together, installed together, and when the
    /// last of them cannot be installed every earlier one goes back to
    /// what it was. The marker the decision records is the marker the
    /// installed rows read back with, or the next entry would find its
    /// own preparation "changed".
    #[test]
    fn pack_state_is_installed_with_the_rows_and_rolls_back_with_them() {
        let store = temp("pack-state-transaction");
        let rows_path = artwork_pack_system_path(&store, "Test");
        let source_path = artwork_pack_source_path(&store, "Test");
        let prepared_path = artwork_pack_prepared_path(&store, "Test");
        std::fs::create_dir_all(rows_path.parent().unwrap()).unwrap();
        std::fs::write(&rows_path, b"old-rows").unwrap();
        std::fs::write(&source_path, b"old-decision").unwrap();
        std::fs::write(&prepared_path, b"old-prepared").unwrap();
        let cancelled = AtomicBool::new(false);
        let prepared = stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("Test")],
            &cancelled,
            "with-state",
        )
        .unwrap()
        .unwrap();
        let marker = prepared.markers()[0];
        let prepared = prepared
            .with_pack_state(
                &store,
                "Test",
                &encode_pack_source(&accepted_state(marker)).unwrap(),
                &encode_pack_prepared(&PackPreparedMap::new()).unwrap(),
            )
            .unwrap();
        assert_eq!(prepared.files.len(), 3);
        assert_eq!(std::fs::read(&rows_path).unwrap(), b"old-rows");
        assert_eq!(std::fs::read(&source_path).unwrap(), b"old-decision");
        assert_eq!(std::fs::read(&prepared_path).unwrap(), b"old-prepared");
        assert!(
            prepared.with_pack_state(&store, "Test", b"", b"").is_err(),
            "one state per system per transaction"
        );

        // Block the last file's backup so the install fails after the rows
        // and the prepared presentation have already gone in.
        let prepared = stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("Test")],
            &cancelled,
            "with-state-blocked",
        )
        .unwrap()
        .unwrap()
        .with_pack_state(
            &store,
            "Test",
            &encode_pack_source(&accepted_state(marker)).unwrap(),
            &encode_pack_prepared(&PackPreparedMap::new()).unwrap(),
        )
        .unwrap();
        let blocked = prepared.files[2].backup_path.clone();
        assert_eq!(prepared.files[2].final_path, source_path);
        std::fs::create_dir(&blocked).unwrap();
        let error = prepared.install().unwrap_err();
        assert!(
            error.to_string().contains("backing up the previous cache"),
            "{error}"
        );
        assert_eq!(std::fs::read(&rows_path).unwrap(), b"old-rows");
        assert_eq!(std::fs::read(&source_path).unwrap(), b"old-decision");
        assert_eq!(std::fs::read(&prepared_path).unwrap(), b"old-prepared");
        std::fs::remove_dir(&blocked).unwrap();

        let prepared = stage_transactional_with_tag(
            &store,
            CacheKind::ArtworkPack,
            vec![staged("Test")],
            &cancelled,
            "with-state-installed",
        )
        .unwrap()
        .unwrap();
        let marker = prepared.markers()[0];
        let (_, warnings) = prepared
            .with_pack_state(
                &store,
                "Test",
                &encode_pack_source(&accepted_state(marker)).unwrap(),
                &encode_pack_prepared(&PackPreparedMap::new()).unwrap(),
            )
            .unwrap()
            .install()
            .unwrap();
        assert!(warnings.is_empty());
        let (_, installed) = load_artwork_pack_data_marked(&store, "Test")
            .unwrap()
            .unwrap();
        assert_eq!(
            installed, marker,
            "the marker written down is the marker the rows read back with"
        );
        assert_eq!(crc32fast::hash(&std::fs::read(&rows_path).unwrap()), marker);
        assert_eq!(
            load_pack_source_state(&store, "Test").unwrap().unwrap(),
            accepted_state(marker)
        );
        assert!(load_pack_prepared_map(&store, "Test")
            .unwrap()
            .unwrap()
            .is_empty());
        assert_eq!(
            std::fs::read_dir(rows_path.parent().unwrap())
                .unwrap()
                .count(),
            3,
            "no staging or backup file is left behind"
        );
        std::fs::remove_dir_all(store).ok();
    }

    /// A state file that is not there says nothing, as the other cache
    /// files do. One that does not decode was written under another
    /// version and reads as no state: the consequence is a question or a
    /// preparation, which is visible, rather than rows drawn from a
    /// decision nobody made. One that is there but cannot be read is an
    /// error and not no state: reading it as no state would let a
    /// question or a preparation write over a decision that still exists.
    #[test]
    fn a_stale_state_file_reads_as_no_state_and_an_unreadable_one_is_an_error() {
        let store = temp("pack-state-loading");
        assert!(load_pack_source_state(&store, "Test").unwrap().is_none());
        assert!(load_pack_prepared_map(&store, "Test").unwrap().is_none());
        save_pack_state(&store, "Test", &accepted_state(9), &PackPreparedMap::new()).unwrap();
        assert_eq!(
            load_pack_source_state(&store, "Test").unwrap().unwrap(),
            accepted_state(9)
        );
        // Cut short, as a write that did not finish leaves it: the
        // version at the front is this one, so the decision is not read
        // as never taken; the error names the file.
        let source_path = artwork_pack_source_path(&store, "Test");
        let written = std::fs::read(&source_path).unwrap();
        std::fs::write(&source_path, &written[..written.len() / 2]).unwrap();
        let error = load_pack_source_state(&store, "Test").unwrap_err();
        assert!(
            error
                .to_string()
                .contains("Artwork Pack state is malformed at"),
            "{error}"
        );
        std::fs::write(&source_path, b"").unwrap();
        assert!(
            load_pack_source_state(&store, "Test").is_err(),
            "an empty file has no version to read: not no state"
        );
        let stale = postcard::to_stdvec(&PackSourceFile {
            format: PACK_SOURCE_FORMAT + 1,
            state: accepted_state(9),
        })
        .unwrap();
        std::fs::write(&source_path, stale).unwrap();
        assert!(
            load_pack_source_state(&store, "Test").unwrap().is_none(),
            "a state written under another policy is prepared again, not misread"
        );
        std::fs::remove_file(&source_path).unwrap();
        std::fs::create_dir(&source_path).unwrap();
        let error = load_pack_source_state(&store, "Test").unwrap_err();
        assert!(
            error.to_string().contains("reading the Artwork Pack state"),
            "{error}"
        );
        assert!(
            load_pack_prepared_map(&store, "Test").unwrap().is_some(),
            "the other file is read on its own"
        );
        std::fs::remove_dir_all(store).ok();
    }
}
