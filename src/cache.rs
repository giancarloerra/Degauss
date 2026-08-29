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

use serde::{Deserialize, Serialize};

use crate::browse::{Kind, Library, Place, Row, MAX_DEPTH};
use crate::error::{DegaussError, Result};

/// Bumped when the shape of what is written changes, so an old file is
/// ignored rather than misread.
const FORMAT: u32 = 1;

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
    let mut cache = SystemCache {
        format: FORMAT,
        folders: BTreeMap::new(),
    };
    let mut seen = Vec::new();
    walk(library, &library.start(), 0, &mut cache, &mut seen);
    cache
}

fn walk(
    library: &Library,
    place: &Place,
    depth: usize,
    cache: &mut SystemCache,
    seen: &mut Vec<String>,
) -> usize {
    let key = place.key();
    if depth > MAX_DEPTH || seen.contains(&key) {
        // Too deep, or round in a circle: a card with a symlink loop must
        // not take the machine with it.
        return 0;
    }
    seen.push(key.clone());

    let Ok((mut rows, _)) = library.list(place, true) else {
        return 0;
    };

    let mut games = 0;
    for row in &mut rows {
        match &row.kind {
            Kind::Play(_) => games += 1,
            Kind::Enter(inner) => {
                let below = walk(library, &inner.clone(), depth + 1, cache, seen);
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
    games
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

/// Throw away everything written down, so the next build starts from the
/// card rather than from itself.
pub fn clear(dir: &Path) {
    let _ = std::fs::remove_dir_all(dir);
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
}
