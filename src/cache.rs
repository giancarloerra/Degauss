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
#[derive(Serialize, Deserialize, Default, Debug, Clone)]
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
pub fn build_system(library: &Library) -> SystemCache {
    let cancelled = AtomicBool::new(false);
    match build_system_controlled(library, &cancelled, &mut |_, _| {}) {
        Ok(Some(cache)) => cache,
        Ok(None) => SystemCache {
            format: FORMAT,
            folders: BTreeMap::new(),
        },
        Err(error) => {
            crate::note(&format!("cache        build failed: {error}"));
            SystemCache {
                format: FORMAT,
                folders: BTreeMap::new(),
            }
        }
    }
}

/// Build with cooperative cancellation and progress between filesystem
/// places. The callback receives completed folders and discovered games.
pub fn build_system_controlled(
    library: &Library,
    cancelled: &AtomicBool,
    progress: &mut impl FnMut(usize, usize),
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
    progress: &mut impl FnMut(usize, usize),
) -> Result<Option<usize>> {
    if cancelled.load(Ordering::Relaxed) {
        return Ok(None);
    }
    let key = place.key();
    if depth > MAX_DEPTH || seen.contains(&key) {
        return Ok(Some(0));
    }
    seen.push(key.clone());
    let (mut rows, _) = match library.list(place, true) {
        Ok(listing) => listing,
        // Released Degauss treats an unreadable nested branch as holding no
        // games and continues indexing the rest of the system. Preserve that
        // behaviour, but let an unreadable system root fail a transactional
        // source build instead of installing an empty replacement cache.
        Err(_) if depth > 0 => return Ok(Some(0)),
        Err(error) => return Err(error),
    };

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
    progress(*folders_done, *games_done);
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
    /// Replace every member of the group as one transaction. If any member
    /// fails, every earlier member is restored before the error is returned.
    pub fn install(mut self) -> Result<(Vec<StagedSystemCache>, Vec<String>)> {
        let install = (|| -> Result<()> {
            for entry in &mut self.files {
                if entry.had_old {
                    std::fs::rename(&entry.final_path, &entry.backup_path).map_err(|error| {
                        DegaussError::io("backing up the previous cache", &entry.final_path, error)
                    })?;
                    entry.backup_moved = true;
                } else if entry.final_path.exists() {
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
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&new_path)
            .map_err(|error| DegaussError::io("creating staged cache", &new_path, error))?;
        prepared.files.push(StagedCacheFile {
            had_old: final_path.exists(),
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

/// Clear the released index and gamelist system files while retaining the
/// independent Artwork Pack cache. A forced global rebuild can then refresh
/// source-neutral rows without discarding valid CRC fingerprints before the
/// replacement for that system has been produced.
pub fn clear_for_rebuild(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if entry.file_name() == "artwork-pack" {
            continue;
        }
        if path.is_dir() {
            let _ = std::fs::remove_dir_all(path);
        } else {
            let _ = std::fs::remove_file(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SystemConfig;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("degauss-cache-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn system(dir: &Path) -> SystemConfig {
        SystemConfig {
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

    #[test]
    fn forced_rebuild_clear_retains_only_the_independent_pack_cache() {
        let store = temp("pack-preserved-on-rebuild");
        std::fs::write(index_path(&store), b"old-index").unwrap();
        std::fs::write(system_path(&store, "Test"), b"old-system").unwrap();
        std::fs::create_dir_all(store.join("other-dir")).unwrap();
        std::fs::write(store.join("other-dir/file"), b"old").unwrap();
        let cache = SystemCache {
            format: FORMAT,
            folders: BTreeMap::new(),
        };
        install_transactional(
            &store,
            CacheKind::ArtworkPack,
            &[StagedSystemCache {
                id: "Test".into(),
                cache,
                fingerprints: ContentFingerprints::new(),
                fingerprints_complete: true,
            }],
        )
        .unwrap();

        clear_for_rebuild(&store);

        assert!(!index_path(&store).exists());
        assert!(!system_path(&store, "Test").exists());
        assert!(!store.join("other-dir").exists());
        assert!(load_artwork_pack_data(&store, "Test").is_some());
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
    fn an_unreadable_nested_branch_keeps_the_released_zero_subtree_behaviour() {
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

        let below = walk_controlled(
            &library,
            &Place::Dir(not_a_directory),
            1,
            &mut cache,
            &mut seen,
            &cancelled,
            &mut folders,
            &mut games_done,
            &mut |_, _| {},
        )
        .unwrap();

        assert_eq!(below, Some(0));
        assert!(cache.folders.is_empty());
        std::fs::remove_dir_all(games).ok();
    }
}
