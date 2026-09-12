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
    let bytes = std::fs::read(artwork_pack_system_path(dir, id)).ok()?;
    let cache: ArtworkPackCache = postcard::from_bytes(&bytes).ok()?;
    (cache.format == ARTWORK_PACK_FORMAT && cache.cache.format == FORMAT).then_some(
        ArtworkPackData {
            cache: cache.cache,
            fingerprints: cache.fingerprints,
            fingerprints_complete: cache.fingerprints_complete,
        },
    )
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
///
/// An archive that cannot be read, or a member of one that Main cannot use,
/// is left out and written to `warnings` with its reason: the rest of the
/// system is still written down. A folder that cannot be read is still an
/// error, because nothing says how much of the system sat under it.
pub fn build_system_checked(library: &Library, warnings: &mut Vec<String>) -> Result<SystemCache> {
    build_system_controlled(library, &AtomicBool::new(false), warnings, &mut |_, _| {})?
        .ok_or_else(|| DegaussError::unsupported("cache build", "cancelled"))
}

#[cfg(test)]
pub fn build_system(library: &Library) -> SystemCache {
    build_system_checked(library, &mut Vec::new()).expect("fixture cache must build")
}

/// Build with cooperative cancellation and progress between filesystem
/// places. The callback receives completed folders and discovered games.
pub fn build_system_controlled(
    library: &Library,
    cancelled: &AtomicBool,
    warnings: &mut Vec<String>,
    progress: &mut impl FnMut(usize, usize),
) -> Result<Option<SystemCache>> {
    build_system_observed(
        library,
        cancelled,
        warnings,
        &mut |_, folders, games, starting| {
            if !starting {
                progress(folders, games);
            }
        },
    )
}

pub fn build_system_observed(
    library: &Library,
    cancelled: &AtomicBool,
    warnings: &mut Vec<String>,
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
        warnings,
        &mut folders,
        &mut games,
        progress,
    )?;
    Ok(completed.map(|_| cache))
}

/// The reason an archive is skipped whole, when the error is the archive
/// reader's own: its directory could not be read or does not hold together.
/// The archive is named once by the caller; its own error would name it
/// again, and the summary has to fit a screen.
fn archive_skip_reason(error: &DegaussError, place: &Place) -> Option<String> {
    let Place::Archive(archive) = place else {
        return None;
    };
    match error {
        DegaussError::Malformed { what, path, detail } if path == archive => {
            Some(format!("{what} is malformed: {detail}"))
        }
        DegaussError::Io { what, path, source } if path == archive => {
            Some(format!("{what} failed: {source}"))
        }
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk_controlled(
    library: &Library,
    place: &Place,
    depth: usize,
    cache: &mut SystemCache,
    seen: &mut Vec<String>,
    cancelled: &AtomicBool,
    warnings: &mut Vec<String>,
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
    let (mut rows, _, skipped) = library.list_reporting(place, true)?;
    if !skipped.is_empty() {
        // The reader wrote every member to the log with its reason when it
        // read the archive. The summary on screen gets one line per reason,
        // with a count, so it stays readable for an archive with hundreds
        // of them.
        let mut reasons = BTreeMap::new();
        for skipped in skipped {
            *reasons.entry(skipped.reason).or_insert(0usize) += 1;
        }
        for (reason, count) in reasons {
            warnings.push(format!(
                "{}: {count} member{} skipped: {reason}",
                place.path().display(),
                if count == 1 { "" } else { "s" }
            ));
        }
    }

    let mut games = 0;
    let mut dropped = Vec::new();
    for (index, row) in rows.iter_mut().enumerate() {
        if cancelled.load(Ordering::Relaxed) {
            return Ok(None);
        }
        match &row.kind {
            Kind::Play(_) => {
                games += 1;
                *games_done += 1;
            }
            Kind::Enter(inner) => {
                if let Place::ArchiveDirectory { archive, prefix } = inner {
                    if depth + 1 > MAX_DEPTH {
                        // A folder inside an archive that would sit past
                        // the depth the walk goes is that folder's
                        // condition, not the archive's and not the
                        // system's: its row goes and nothing under it is
                        // written down, the warning names the archive and
                        // the folder, and the archive's other members stay.
                        // A folder on the card that deep is the system's
                        // failure it has always been.
                        let line = format!(
                            "{}/{prefix}: skipped: {}",
                            archive.display(),
                            crate::browse::member_depth_reason()
                        );
                        crate::note(&format!("zip          {line}"));
                        warnings.push(line);
                        dropped.push(index);
                        continue;
                    }
                }
                let before = (seen.len(), *folders_done, *games_done, warnings.len());
                let below = match walk_controlled(
                    library,
                    &inner.clone(),
                    depth + 1,
                    cache,
                    seen,
                    cancelled,
                    warnings,
                    folders_done,
                    games_done,
                    progress,
                ) {
                    Ok(Some(below)) => below,
                    Ok(None) => return Ok(None),
                    Err(error) => {
                        // An archive whose directory does not hold together
                        // is left out whole: every folder its subtree already
                        // wrote is taken back (its keys were pushed to `seen`
                        // before being written), the progress counts it
                        // raised and the member lines its listing added to
                        // the summary go back with it, its row goes, and the
                        // rest of the system carries on. Nothing of it is
                        // ever published half done. Any other failure under
                        // it is the system's as before.
                        let Some(reason) = archive_skip_reason(&error, inner) else {
                            return Err(error);
                        };
                        // The log gets the whole error here, whatever later
                        // becomes of the summary this scan is building.
                        crate::note(&format!(
                            "zip          {}: skipped: {error}",
                            inner.path().display()
                        ));
                        for key in seen.drain(before.0..) {
                            cache.folders.remove(&key);
                        }
                        *folders_done = before.1;
                        *games_done = before.2;
                        warnings.truncate(before.3);
                        warnings.push(format!("{}: skipped: {reason}", inner.path().display()));
                        dropped.push(index);
                        continue;
                    }
                };
                row.below = Some(below);
                games += below;
            }
        }
    }
    if !dropped.is_empty() {
        let mut index = 0;
        rows.retain(|_| {
            let keep = !dropped.contains(&index);
            index += 1;
            keep
        });
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

    /// Stage the matching index in the same rollback transaction as its rows.
    pub fn with_index(mut self, dir: &Path, index: &Index) -> Result<Self> {
        let final_path = index_path(dir);
        if self
            .files
            .iter()
            .any(|entry| entry.final_path == final_path)
        {
            return Err(DegaussError::unsupported(
                "cache transaction",
                "index already staged",
            ));
        }
        // Inspect the previous index before creating a file, so a failed
        // metadata check cannot leave an untracked staging file behind.
        let had_old = path_exists(&final_path)?;
        let (new_path, backup_path) = loop {
            let serial = NEXT_CACHE_TRANSACTION.fetch_add(1, Ordering::Relaxed);
            let tag = format!("degauss-index-{}-{serial}", std::process::id());
            let new_path = final_path.with_extension(format!("{tag}.new"));
            let backup_path = final_path.with_extension(format!("{tag}.bak"));
            if !path_exists(&new_path)? && !path_exists(&backup_path)? {
                break (new_path, backup_path);
            }
        };
        let bytes = postcard::to_stdvec(index)
            .map_err(|e| DegaussError::unsupported("cache index", e.to_string()))?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_path)
            .map_err(|e| DegaussError::io("creating staged index", &new_path, e))?;
        self.files.push(StagedCacheFile {
            had_old,
            final_path,
            new_path: new_path.clone(),
            backup_path,
            backup_moved: false,
            installed: false,
        });
        use std::io::Write as _;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| DegaussError::io("writing staged index", &new_path, e))?;
        let written = std::fs::read(&new_path)
            .map_err(|e| DegaussError::io("validating staged index", &new_path, e))?;
        let decoded: Index = postcard::from_bytes(&written)
            .map_err(|e| DegaussError::malformed("staged index", &new_path, e.to_string()))?;
        if decoded.format != FORMAT {
            return Err(DegaussError::malformed(
                "staged index",
                &new_path,
                "incompatible format",
            ));
        }
        Ok(self)
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

/// Commit a complete system scan and its summary together.
pub fn save_system_with_index(
    dir: &Path,
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
    stage_transactional(
        dir,
        CacheKind::Gamelist,
        vec![staged],
        &AtomicBool::new(false),
    )?
    .ok_or_else(|| DegaussError::unsupported("cache transaction", "cancelled"))?
    .with_index(dir, index)?
    .install()
    .map(|(_, warnings)| warnings)
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

        let built =
            build_system_controlled(&library, &cancelled, &mut Vec::new(), &mut |folders, _| {
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
        let original = build_system_checked(&library, &mut Vec::new()).unwrap();
        let mut events = Vec::new();
        let observed = build_system_observed(
            &library,
            &AtomicBool::new(false),
            &mut Vec::new(),
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
        let result = build_system_observed(
            &library,
            &cancelled,
            &mut Vec::new(),
            &mut |_, _, _, starting| {
                assert!(starting);
                cancelled.store(true, Ordering::Relaxed);
            },
        );
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
            &mut Vec::new(),
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
        let initial = build_system_checked(&library, &mut Vec::new()).unwrap();
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
        save_system_with_index(&store, "Test", &initial, &index).unwrap();
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
        let mut warnings = Vec::new();
        let rebuilt =
            build_system_checked(&Library::open(&system(&games)).unwrap(), &mut warnings).unwrap();
        assert!(warnings.is_empty(), "{warnings:?}");
        index
            .systems
            .insert("Test".into(), rebuilt.summary(&library.start()));
        assert!(save_system_with_index(&store, "Test", &rebuilt, &index)
            .unwrap()
            .is_empty());
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

    /// An archive that stops holding together is left out with a warning
    /// and the system is written down without it, but not before: the
    /// previous complete cache and index stay until the new scan is
    /// published as one transaction, and a system whose only content was
    /// that archive completes with nothing, warned about, rather than
    /// looking like a healthy empty system.
    #[test]
    fn malformed_nested_archive_is_skipped_with_a_warning_until_published() {
        let games = temp("failed-rebuild-games");
        let store = temp("failed-rebuild-store");
        std::fs::create_dir_all(games.join("Nested")).unwrap();
        let archive = games.join("Nested/Set.zip");
        std::fs::write(&archive, crate::zip::tests_archive(&["Game.d64"], false)).unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let old = build_system_checked(&library, &mut Vec::new()).unwrap();
        let mut index = Index::new();
        index
            .systems
            .insert("Test".into(), old.summary(&library.start()));
        save_system_with_index(&store, "Test", &old, &index).unwrap();
        let old_cache_bytes = std::fs::read(system_path(&store, "Test")).unwrap();
        let old_index_bytes = std::fs::read(index_path(&store)).unwrap();
        std::fs::write(&archive, b"broken archive").unwrap();
        let mut warnings = Vec::new();
        let rebuilt =
            build_system_checked(&Library::open(&system(&games)).unwrap(), &mut warnings).unwrap();
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(
            warnings[0].starts_with(&format!("{}: skipped: ", archive.display())),
            "{warnings:?}"
        );
        assert!(warnings[0].contains("end-of-directory"), "{warnings:?}");
        assert_eq!(rebuilt.summary(&library.start()).games, 0);
        assert!(
            !rebuilt
                .folders
                .contains_key(&Place::Archive(archive.clone()).key()),
            "a rejected archive must not be written down as a folder"
        );
        let nested = rebuilt
            .get(&Place::Dir(games.join("Nested")))
            .expect("the folder holding the archive is still written down");
        assert!(nested.rows.is_empty(), "{:?}", nested.rows);
        assert_eq!(nested.games, 0);
        assert_eq!(
            std::fs::read(system_path(&store, "Test")).unwrap(),
            old_cache_bytes,
            "scanning publishes nothing by itself"
        );
        assert_eq!(std::fs::read(index_path(&store)).unwrap(), old_index_bytes);
        assert_eq!(load_index(&store).unwrap().systems["Test"].games, 1);
        index
            .systems
            .insert("Test".into(), rebuilt.summary(&library.start()));
        save_system_with_index(&store, "Test", &rebuilt, &index).unwrap();
        assert_eq!(load_index(&store).unwrap().systems["Test"].games, 0);
        assert_eq!(
            load_system(&store, "Test")
                .unwrap()
                .summary(&library.start())
                .games,
            0
        );
        std::fs::remove_dir_all(games).unwrap();
        std::fs::remove_dir_all(store).unwrap();
    }

    /// A structurally corrupt archive costs the system only that archive,
    /// whichever way its directory fails to hold together: the healthy file
    /// beside it and the healthy archive beside it are written down, nothing
    /// of the rejected one is (not a folder key, not a row, not a count),
    /// and one warning names it. The mutations are the ones the reader's
    /// own tests refuse; this is where refusing them has to reach the
    /// system as a skipped archive rather than a failed build.
    #[test]
    fn a_corrupt_archive_is_skipped_and_its_siblings_are_written_down() {
        let healthy = crate::zip::tests_archive(&["One.d64", "Two.d64"], false);
        let classic = crate::zip::tests_archive(&["Game.d64", "Plain.d64"], false);
        let zip64 = crate::zip::tests_archive(&["Game.d64"], true);
        let central = |bytes: &[u8]| {
            bytes
                .windows(4)
                .position(|b| b == [0x50, 0x4b, 0x01, 0x02])
                .unwrap()
        };
        let end64 = zip64
            .windows(4)
            .position(|b| b == [0x50, 0x4b, 6, 6])
            .unwrap();
        let eocd = classic.len() - 22;
        let mutations: [(&str, &[u8], &[(usize, u8)], &str); 9] = [
            (
                "truncated",
                &healthy,
                &[],
                "no valid end-of-directory record",
            ),
            (
                "garbage",
                b"broken archive",
                &[],
                "no valid end-of-directory record",
            ),
            (
                "signature",
                &classic,
                &[(central(&classic), 0)],
                "invalid central-directory signature",
            ),
            (
                "count",
                &classic,
                &[(eocd + 8, 9), (eocd + 10, 9)],
                "central-directory size, count or offset is inconsistent",
            ),
            (
                "offset",
                &classic,
                &[(eocd + 16, 1)],
                "central-directory size, count or offset is inconsistent",
            ),
            (
                "zip64-record",
                &zip64,
                &[(end64 + 4, 43)],
                "invalid ZIP64 end record",
            ),
            (
                "multi-disk",
                &classic,
                &[(eocd + 4, 1)],
                "multi-disk archive",
            ),
            (
                "masked-header",
                &classic,
                &[(central(&classic) + 9, 32)],
                "masked local header",
            ),
            (
                "sentinel-disk",
                &classic,
                &[
                    (central(&classic) + 34, 0xff),
                    (central(&classic) + 35, 0xff),
                ],
                "member starts on another disk",
            ),
        ];
        for (tag, original, changes, reason) in mutations {
            let games = temp(&format!("corrupt-archive-{tag}"));
            std::fs::write(games.join("Healthy.d64"), b"rom").unwrap();
            std::fs::write(games.join("Good.zip"), &healthy).unwrap();
            let broken = games.join("Broken.zip");
            let mut bytes = original.to_vec();
            if changes.is_empty() {
                bytes.truncate(bytes.len() / 2);
            }
            for (offset, value) in changes {
                bytes[*offset] = *value;
            }
            std::fs::write(&broken, &bytes).unwrap();
            let library = Library::open(&system(&games)).unwrap();
            let mut warnings = Vec::new();
            let cache = build_system_checked(&library, &mut warnings)
                .unwrap_or_else(|error| panic!("{tag}: the system failed: {error}"));
            assert_eq!(cache.summary(&library.start()).games, 3, "{tag}");
            let top = cache.get(&library.start()).unwrap();
            assert_eq!(
                top.rows
                    .iter()
                    .map(|row| row.name.as_str())
                    .collect::<Vec<_>>(),
                ["Good", "Healthy"],
                "{tag}: the rejected archive has no row"
            );
            assert_eq!(top.rows[0].below, Some(2), "{tag}");
            assert!(
                !cache
                    .folders
                    .contains_key(&Place::Archive(broken.clone()).key()),
                "{tag}"
            );
            assert!(
                cache
                    .folders
                    .contains_key(&Place::Archive(games.join("Good.zip")).key()),
                "{tag}"
            );
            assert_eq!(warnings.len(), 1, "{tag}: {warnings:?}");
            assert!(
                warnings[0].starts_with(&format!("{}: skipped: ", broken.display()))
                    && warnings[0].contains(reason),
                "{tag}: {warnings:?}"
            );
            std::fs::remove_dir_all(games).unwrap();
        }
    }

    /// The summary on screen is one line per archive and reason; the log
    /// is where the complete diagnostic lives for the run, so a skipped
    /// member has to be there with its name and reason, and a skipped
    /// archive with its error. Only what the log gains during the build
    /// counts, searched by this fixture's own paths, because the log is
    /// shared by every test that writes one and kept across runs. The
    /// launch check of a retained member reads the archive again and must
    /// not repeat the member block: a start resolves every favourite in an
    /// archive through that read.
    #[test]
    fn every_skip_is_in_the_log_with_the_member_or_the_archive_and_its_reason() {
        let games = temp("skips-in-the-log");
        let broken = games.join("Broken.zip");
        std::fs::write(&broken, b"broken archive").unwrap();
        let held = games.join("Held.zip");
        std::fs::write(
            &held,
            crate::zip::tests_archive(&["Game.d64", "inner.zip"], false),
        )
        .unwrap();
        // The log may not exist yet on a fresh machine; `note` creates it.
        let log_since = |from: usize| {
            let log = std::fs::read_to_string(crate::LOG_PATH).unwrap_or_default();
            log[from.min(log.len())..].to_string()
        };
        let before = log_since(0).len();
        let library = Library::open(&system(&games)).unwrap();
        let mut warnings = Vec::new();
        let cache = build_system_checked(&library, &mut warnings).unwrap();
        assert_eq!(cache.summary(&library.start()).games, 1);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        let log = log_since(before);
        let member = format!(
            "zip          {}: member inner.zip: nested archive member is unsupported by MiSTer Main",
            held.display()
        );
        assert!(
            log.lines().any(|line| line == member),
            "no line {member:?} in the log"
        );
        let skipped = format!("zip          {}: skipped: ", broken.display());
        let error = log
            .lines()
            .find_map(|line| line.strip_prefix(&skipped))
            .unwrap_or_else(|| panic!("no line starting {skipped:?} in the log"));
        assert!(
            error.contains(&broken.display().to_string()) && error.contains("malformed"),
            "the archive line carries the complete error: {error:?}"
        );
        let before = log_since(0).len();
        crate::zip::validate_member(&held.join("Game.d64")).unwrap();
        let held_line = format!("zip          {}: ", held.display());
        assert!(
            !log_since(before)
                .lines()
                .any(|line| line.starts_with(&held_line)),
            "the launch check wrote the archive's members again"
        );
        std::fs::remove_dir_all(games).unwrap();
    }

    /// An archive that fails after part of it was walked (here, one taken
    /// away between two of its folders) is taken back whole, and the
    /// progress figures go back with it: the dashboard must not end on more
    /// games than the cache it announces holds. The member line its
    /// listing put in the summary goes back too: an archive nothing is
    /// published from is reported once, as skipped, not also as holding a
    /// skipped member.
    #[test]
    fn a_skipped_archive_takes_its_progress_counts_back_with_it() {
        let games = temp("skipped-archive-progress");
        std::fs::write(games.join("Plain.d64"), b"rom").unwrap();
        let archive = games.join("Gone.zip");
        let plain = |name: &'static [u8]| crate::zip::TestEntry {
            name,
            flags: 0,
            method: 0,
            extra: &[],
        };
        std::fs::write(
            &archive,
            crate::zip::tests_archive_entries(
                &[
                    plain(b"a/one.d64"),
                    plain(b"b/two.d64"),
                    crate::zip::TestEntry {
                        name: b"locked.d64",
                        flags: 1,
                        method: 0,
                        extra: &[],
                    },
                ],
                false,
            ),
        )
        .unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let mut warnings = Vec::new();
        let mut last = None;
        let cache = build_system_observed(
            &library,
            &AtomicBool::new(false),
            &mut warnings,
            &mut |place, folders, found, starting| {
                if starting
                    && matches!(place, Place::ArchiveDirectory { prefix, .. } if prefix == "b")
                {
                    assert_eq!(found, 1, "a/one.d64 was counted before b was entered");
                    std::fs::remove_file(&archive).unwrap();
                }
                last = Some((folders, found));
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(cache.summary(&library.start()).games, 1);
        assert_eq!(last, Some((cache.folders.len(), 1)));
        assert_eq!(
            warnings.len(),
            1,
            "the line for locked.d64 must go back with the archive: {warnings:?}"
        );
        assert!(
            warnings[0].starts_with(&format!("{}: skipped: ", archive.display())),
            "{warnings:?}"
        );
        std::fs::remove_dir_all(games).unwrap();
    }

    /// Main splits its target at the first `.zip`, so an archive inside an
    /// archive can never be launched: that member is left out, the
    /// supported member beside it is written down and counted, and the
    /// warning says which archive and why.
    #[test]
    fn an_inner_archive_member_is_skipped_and_the_outer_members_are_kept() {
        let games = temp("inner-archive-member");
        let outer = games.join("Outer.zip");
        std::fs::write(
            &outer,
            crate::zip::tests_archive(&["Game.d64", "inner.zip", "Other.d64"], false),
        )
        .unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let mut warnings = Vec::new();
        let cache = build_system_checked(&library, &mut warnings).unwrap();
        assert_eq!(cache.summary(&library.start()).games, 2);
        let inside = cache.get(&Place::Archive(outer.clone())).unwrap();
        assert_eq!(
            inside
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["Game", "Other"]
        );
        assert!(inside.rows.iter().all(|row| !row.is_folder()));
        assert_eq!(cache.get(&library.start()).unwrap().rows[0].below, Some(2));
        assert_eq!(
            warnings,
            [format!(
                "{}: 1 member skipped: nested archive member is unsupported by MiSTer Main",
                outer.display()
            )]
        );
        std::fs::remove_dir_all(games).unwrap();
    }

    /// A folder inside an archive that sits past the depth the walk goes
    /// is left out with everything under it, with a warning naming the
    /// archive and the folder, and the archive's other members are still
    /// published beside the games outside it: a condition confined to some
    /// members must not take the archive, let alone the system, down with
    /// it. A folder on the card that deep stays the system's failure it has
    /// always been.
    #[test]
    fn a_folder_past_the_depth_limit_inside_an_archive_is_skipped_with_what_is_under_it() {
        let games = temp("archive-depth-overflow");
        let deep = format!("{}game.d64", "d/".repeat(MAX_DEPTH + 1));
        let archive = games.join("Deep.zip");
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&[deep.as_str(), "shallow.d64"], false),
        )
        .unwrap();
        std::fs::write(games.join("Beside.d64"), b"x").unwrap();
        let library = Library::open(&system(&games)).unwrap();
        let mut warnings = Vec::new();
        let cache = build_system_checked(&library, &mut warnings).unwrap();
        assert_eq!(cache.summary(&library.start()).games, 2);
        let root = cache.get(&library.start()).unwrap();
        assert_eq!(
            root.rows
                .iter()
                .map(|row| (row.name.as_str(), row.below))
                .collect::<Vec<_>>(),
            [("Deep", Some(1)), ("Beside", None)],
            "the archive is published with its shallow member"
        );
        let inside = cache.get(&Place::Archive(archive.clone())).unwrap();
        assert_eq!(
            inside
                .rows
                .iter()
                .map(|row| row.name.as_str())
                .collect::<Vec<_>>(),
            ["d", "shallow"]
        );
        // The archive sits one level below the system, so the folder that
        // would be one level past the limit is the one with MAX_DEPTH
        // segments: the level above it is written down, empty, and nothing
        // from there on is.
        let prefix = |segments: usize| vec!["d"; segments].join("/");
        let folder = |segments: usize| {
            cache.get(&Place::ArchiveDirectory {
                archive: archive.clone(),
                prefix: prefix(segments),
            })
        };
        let last = folder(MAX_DEPTH - 1).expect("the folder above the limit");
        assert!(last.rows.is_empty(), "{:?}", last.rows);
        assert_eq!(last.games, 0);
        assert!(folder(MAX_DEPTH).is_none());
        assert!(folder(MAX_DEPTH + 1).is_none());
        assert_eq!(
            warnings,
            [format!(
                "{}/{}: skipped: {}",
                archive.display(),
                prefix(MAX_DEPTH),
                crate::browse::member_depth_reason()
            )]
        );

        let folders = temp("folder-depth-overflow");
        let mut deep = folders.clone();
        for _ in 0..=MAX_DEPTH {
            deep.push("d");
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("game.d64"), b"x").unwrap();
        let library = Library::open(&system(&folders)).unwrap();
        let mut warnings = Vec::new();
        let error = build_system_checked(&library, &mut warnings).unwrap_err();
        assert!(
            error.to_string().contains("maximum folder depth"),
            "{error}"
        );
        assert!(warnings.is_empty(), "{warnings:?}");
        std::fs::remove_dir_all(games).unwrap();
        std::fs::remove_dir_all(folders).unwrap();
    }

    #[test]
    fn failed_index_install_rolls_back_the_new_rows_and_preserves_the_old_pair() {
        let store = temp("index-install-rollback");
        let old = staged("Test");
        let old_index = Index::new();
        save_system_with_index(&store, "Test", &old.cache, &old_index).unwrap();
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
        save_system_with_index(&store, "Test", &cache.cache, &index).unwrap();
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
        let cache = build_system_controlled(&library, &cancelled, &mut Vec::new(), &mut |_, _| {
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
            &mut Vec::new(),
            &mut 0,
            &mut 0,
            &mut |_, _, _, _| {},
        )
        .unwrap_err();
        assert!(error.to_string().contains("maximum folder depth"));
        assert!(cache.folders.is_empty());
        std::fs::remove_dir_all(games).unwrap();
    }
}
