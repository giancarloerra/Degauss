//! Browsing the disk as it is.
//!
//! The disk is the database, and a folder is a folder. MiSTer's own menu
//! shows the directory you are standing in: subfolders first, then the files
//! a core can load, and an archive opens like a directory because the loader
//! reaches inside one. Degauss shows the same thing, so the structure people
//! build with organise scripts and favourites folders is the structure they
//! browse.
//!
//! Nothing is flattened or indexed. Opening a folder is one `read_dir`,
//! which is why entering a system is instant however large the library is,
//! and why a set of folders holding the same games under different
//! groupings (what every "organised" collection looks like) cannot produce
//! duplicates: only one directory is ever on screen.
//!
//! Artwork is the EmulationStation overlay, unchanged: an optional
//! `gamelist.xml` beside the games. Each art directory it points at is read
//! ONCE into a set of names and answered from memory, because a scraped card
//! can keep tens of thousands of pictures in one FAT directory, where every
//! individual lookup rescans the whole directory.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::config::SystemConfig;
use crate::error::{DegaussError, Result};
use crate::gamelist::Gamelist;

/// Files every MiSTer card carries that are not games.
///
/// A core's boot rom and its blank disk images sit in the same folder as the
/// games and carry the same extensions, so an extension list alone lists
/// them. The X68000 folder is the clearest case: three of its five entries
/// are `boot.rom`, `boot3.vhd` and a blank disk. The stock menu excludes
/// names for the same reason.
const NOT_GAMES: [&str; 4] = ["boot.rom", "boot.vhd", "blank.vhd", "boot0.rom"];

/// Hard ceiling on how far anything walks by itself. Not a preference: a
/// symlink loop must not be able to walk the whole card forever.
pub const MAX_DEPTH: usize = 12;

/// Somewhere that can be listed.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Place {
    /// A real directory.
    Dir(PathBuf),
    /// An archive, opened like a directory: MiSTer loads `archive.zip/rom`
    /// directly, so there is no reason to hide what is inside one.
    Archive(PathBuf),
    /// A plain-text listing naming titles held inside an AmigaVision disk
    /// image. The titles are real games that are not files.
    Listing { install: PathBuf, file: PathBuf },
    /// The several folders one system's games are spread across.
    ///
    /// Systems really do keep games in more than one place. DOS is the
    /// clearest: `games/AO486` holds the disk images and boot roms, while
    /// the shortcuts that actually launch the games, and the metadata for
    /// all of them, live in `_DOS Games`. Opening only the first folder
    /// hides the entire library.
    Roots,
    /// An internal ZIP directory. Appended to preserve postcard enum tags.
    ArchiveDirectory { archive: PathBuf, prefix: String },
}

impl Place {
    pub fn path(&self) -> &Path {
        match self {
            Place::Dir(path) | Place::Archive(path) => path,
            Place::Listing { file, .. } => file,
            Place::Roots => Path::new(""),
            Place::ArchiveDirectory { archive, .. } => archive,
        }
    }

    /// A name for this place that can be written down and looked up again.
    ///
    /// Not just the path: a listing inside a disk image and the image
    /// itself are different places, and the several roots of one system are
    /// a place with no path at all.
    pub fn key(&self) -> String {
        match self {
            Place::Dir(path) => format!("d:{}", path.display()),
            Place::Archive(path) => format!("a:{}", path.display()),
            Place::Listing { install, file } => {
                format!("l:{}|{}", install.display(), file.display())
            }
            Place::Roots => "r:".to_string(),
            Place::ArchiveDirectory { archive, prefix } => {
                format!("a:{}/{}", archive.display(), prefix)
            }
        }
    }
}

/// How a game is started.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// A path handed to the loader, possibly pointing inside an archive.
    File(PathBuf),
    /// A title written into an AmigaVision boot file before the core starts.
    AmigaVision { install: PathBuf, title: String },
}

/// What one row does when it is chosen.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub enum Kind {
    Enter(Place),
    Play(Launch),
}

/// One line of a listing.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, PartialEq, Eq)]
pub struct Row {
    /// What the user reads: the gamelist name when there is one, else the
    /// name on disk exactly as it is, with no invented cleanup.
    pub name: String,
    pub sort_key: String,
    pub kind: Kind,
    pub cover: Option<PathBuf>,
    pub genre: Option<String>,
    pub favorite: bool,
    /// How many playable things are below this row, when that has been
    /// worked out and written down. [`None`] means nobody has counted:
    /// read straight off the card, the answer costs a walk of everything
    /// underneath and is not paid for on the way past.
    pub below: Option<usize>,
    /// What the gamelist says about it beyond its name, in the order it is
    /// drawn. Empty strings rather than absent ones: the panel keeps the
    /// same lines in the same places whatever a card happens to know, so
    /// the eye does not have to find them again for every game.
    pub details: Details,
}

/// The lines under the picture, in the order they are shown.
#[derive(serde::Serialize, serde::Deserialize, Debug, Clone, Default, PartialEq, Eq)]
pub struct Details {
    pub desc: String,
    pub publisher: String,
    pub developer: String,
    pub released: String,
    pub players: String,
    pub lang: String,
}

impl Details {
    /// The label each line carries, whether or not there is anything after
    /// it.
    pub const LABELS: [&'static str; 6] = ["Desc:", "Pub:", "Dev:", "Date:", "Pl:", "Lang:"];

    pub fn values(&self) -> [&str; 6] {
        [
            &self.desc,
            &self.publisher,
            &self.developer,
            &self.released,
            &self.players,
            &self.lang,
        ]
    }
}

impl Row {
    pub fn is_folder(&self) -> bool {
        matches!(self.kind, Kind::Enter(_))
    }
}

/// What a listing cost and covered, for the audit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ListStats {
    pub folders: usize,
    pub games: usize,
    pub with_art: usize,
    pub empty_folders_hidden: usize,
}

/// The names present in each artwork directory, read once.
#[derive(Debug, Default)]
struct ArtIndex {
    names: HashSet<String>,
    directories: HashSet<String>,
    files: usize,
    structural_only: bool,
}

impl ArtIndex {
    fn read(dirs: &[PathBuf]) -> Self {
        let mut index = ArtIndex::default();
        for dir in dirs {
            let Ok(listing) = std::fs::read_dir(dir) else {
                // A directory the metadata points at but the card does not
                // hold contributes nothing, and every entry naming it is
                // then counted as art that is missing.
                continue;
            };
            index.directories.insert(key(dir));
            for item in listing.flatten() {
                index.files += 1;
                index.names.insert(key(&dir.join(item.file_name())));
            }
        }
        index
    }

    fn structural(dirs: &[PathBuf]) -> Self {
        ArtIndex {
            directories: dirs.iter().map(|dir| key(dir)).collect(),
            structural_only: true,
            ..ArtIndex::default()
        }
    }

    fn contains(&self, path: &Path) -> bool {
        self.names.contains(&key(path))
    }

    /// True when this directory is one the metadata points art at, so
    /// browsing leaves it alone: it is not a folder of games, and on a real
    /// card it holds tens of thousands of pictures.
    fn is_art_directory(&self, path: &Path) -> bool {
        self.directories.contains(&key(path))
    }

    /// Whether a source-neutral structural exclusion sits anywhere below this
    /// folder. The caller still proves that no launchable content shares the
    /// subtree before hiding it, so a custom media parent can also hold games
    /// without those games disappearing.
    fn has_structural_art_below(&self, path: &Path) -> bool {
        if !self.structural_only {
            return false;
        }
        let path = PathBuf::from(key(path));
        self.directories
            .iter()
            .any(|directory| Path::new(directory).starts_with(&path))
    }
}

fn key(path: &Path) -> String {
    path.to_string_lossy().to_ascii_lowercase()
}

/// Where browsing a system begins, without opening it.
///
/// Reading a system costs seconds; knowing where it starts costs nothing
/// and is decided entirely by how many folders it was declared with. Kept
/// apart so a system whose listing has already been written down can be
/// opened without being read again.
pub fn start_for(config: &SystemConfig) -> Place {
    if config.extra_paths.is_empty() {
        Place::Dir(PathBuf::from(&config.path))
    } else {
        Place::Roots
    }
}

/// The display names MiSTer's own menu applies, read from `names.txt` at
/// the top of the card.
///
/// It renames the self-describing files: cores, `.mra` arcade definitions
/// and `.mgl` shortcuts. A card that has one expects to see those names, so
/// a browser that ignores it shows a different library than the stock menu
/// does for the same files.
///
/// One deliberate difference from Main: it looks the key up with a plain
/// substring search over the whole file, so asking for `Atom:` can be
/// answered by the `AcornAtom:` line that happens to appear first. Keying by
/// line is the same answer for every well-formed entry and not wrong for the
/// rest.
#[derive(Debug, Default, Clone)]
pub struct DisplayNames {
    by_stem: std::collections::BTreeMap<String, String>,
}

impl DisplayNames {
    /// Read the file if the card has one. Its absence is normal.
    pub fn read(path: &Path) -> Self {
        let mut names = DisplayNames::default();
        let Ok(bytes) = std::fs::read(path) else {
            return names;
        };
        for line in decode_listing(&bytes).lines() {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let key = key.trim();
            let value = value.trim();
            if key.is_empty() || value.is_empty() {
                continue;
            }
            names
                .by_stem
                .insert(key.to_ascii_lowercase(), value.to_string());
        }
        names
    }

    fn get(&self, stem: &str) -> Option<&str> {
        self.by_stem
            .get(&stem.to_ascii_lowercase())
            .map(String::as_str)
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.by_stem.len()
    }
}

struct Root {
    path: PathBuf,
    gamelist: Option<Gamelist>,
    art: ArtIndex,
}

/// Which presentation source is allowed to influence filesystem rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LibraryDataMode {
    Gamelist,
    SourceNeutral,
}

/// One system's folders, ready to browse.
pub struct Library {
    config: SystemConfig,
    roots: Vec<Root>,
    names: DisplayNames,
    /// A malformed gamelist does not prevent source-neutral Pack browsing,
    /// but it must remain visible to the diagnostic paths rather than being
    /// reduced to a log line that `--audit` cannot report.
    structural_problems: Vec<(PathBuf, String)>,
    archive_cache: std::cell::RefCell<crate::zip::ArchiveCache>,
    /// The ROM-set catalogues of a Neo Geo system, whose ZIPs and folders
    /// can be single games. [`None`] for every other system, which then
    /// pays nothing for the question.
    neogeo: Option<crate::neogeo::Catalogues>,
    /// What reading this system's metadata cost, so the answer to "why did
    /// that take a moment" is measured rather than guessed.
    pub cost: OpenCost,
}

/// What opening a system cost, in milliseconds and files.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCost {
    pub gamelist_ms: u128,
    /// Reading the ROM-set catalogues of a Neo Geo system; zero for
    /// every other system, which reads none.
    pub catalogue_ms: u128,
    pub art_ms: u128,
    pub art_files: usize,
}

/// Whether a listed entry is a directory, following a symlink to find out.
///
/// `DirEntry::file_type` deliberately does not follow links, so a folder
/// reached through one reads as an ordinary file and its games disappear.
/// Collections built out of linked trees are common enough on a card that
/// the extra call is worth making, and it is only made for links.
fn entry_is_dir_checked(item: &std::fs::DirEntry) -> Result<bool> {
    let kind = item
        .file_type()
        .map_err(|error| DegaussError::io("reading entry type", item.path(), error))?;
    if kind.is_symlink() {
        // Dangling links have no launchable target; preserve that established
        // case, but permission/device failures must not hide a subtree.
        match std::fs::metadata(item.path()) {
            Ok(metadata) => Ok(metadata.is_dir()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(DegaussError::io("reading link target", item.path(), error)),
        }
    } else {
        Ok(kind.is_dir())
    }
}

fn entry_is_dir(item: &std::fs::DirEntry) -> bool {
    match item.file_type() {
        Ok(kind) if kind.is_symlink() => item.path().is_dir(),
        Ok(kind) => kind.is_dir(),
        Err(_) => false,
    }
}

impl Library {
    /// Read the metadata overlay for every folder this system declares.
    ///
    /// A gamelist that cannot be parsed is an error rather than a silent
    /// absence: the difference between "this system has no metadata" and
    /// "its metadata is broken" is the difference between a card that is
    /// fine and one that needs attention, and only one of them is worth
    /// telling somebody about.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn open(config: &SystemConfig) -> Result<Self> {
        Self::open_with_names(config, DisplayNames::default())
    }

    /// As [`Library::open`], with the card's own display names applied.
    pub fn open_with_names(config: &SystemConfig, names: DisplayNames) -> Result<Self> {
        Self::open_in_mode(config, names, LibraryDataMode::Gamelist)
    }

    /// Open a library without binding any gamelist presentation. A bounded
    /// structural scan still identifies media directories so switching data
    /// source cannot expose a scraped artwork tree as folders of games.
    pub fn open_source_neutral(config: &SystemConfig, names: DisplayNames) -> Result<Self> {
        Self::open_in_mode(config, names, LibraryDataMode::SourceNeutral)
    }

    fn open_in_mode(
        config: &SystemConfig,
        names: DisplayNames,
        mode: LibraryDataMode,
    ) -> Result<Self> {
        let mut roots = Vec::new();
        let mut cost = OpenCost::default();
        let mut structural_problems = Vec::new();
        for path in std::iter::once(config.path.clone()).chain(config.extra_paths.iter().cloned()) {
            let path = PathBuf::from(path);
            let gamelist_path = path.join("gamelist.xml");
            let started = std::time::Instant::now();
            let (gamelist, art_dirs) = match mode {
                LibraryDataMode::Gamelist if gamelist_path.is_file() => {
                    let list = Gamelist::load(&gamelist_path, &path)?;
                    let dirs = list.art_directories();
                    (Some(list), dirs)
                }
                LibraryDataMode::SourceNeutral if gamelist_path.is_file() => {
                    let dirs = match Gamelist::structural_art_directories(&gamelist_path, &path) {
                        Ok(dirs) => dirs,
                        Err(error) => {
                            crate::note(&format!(
                                "pack scan    {}: {error}",
                                gamelist_path.display()
                            ));
                            structural_problems.push((gamelist_path.clone(), error.to_string()));
                            fallback_media_directories(&path, config)
                        }
                    };
                    (None, dirs)
                }
                _ => (None, Vec::new()),
            };
            cost.gamelist_ms += started.elapsed().as_millis();
            let started = std::time::Instant::now();
            let art = if mode == LibraryDataMode::Gamelist {
                ArtIndex::read(&art_dirs)
            } else {
                ArtIndex::structural(&art_dirs)
            };
            cost.art_ms += started.elapsed().as_millis();
            cost.art_files += art.files;
            roots.push(Root {
                path,
                gamelist,
                art,
            });
        }
        let started = std::time::Instant::now();
        let neogeo = crate::neogeo::is_romset_system(config)
            .then(|| crate::neogeo::Catalogues::open(roots.iter().map(|root| root.path.as_path())));
        cost.catalogue_ms = started.elapsed().as_millis();
        Ok(Library {
            config: config.clone(),
            roots,
            names,
            structural_problems,
            archive_cache: std::cell::RefCell::new(crate::zip::ArchiveCache::default()),
            neogeo,
            cost,
        })
    }

    /// Where browsing this system starts.
    ///
    /// One folder opens straight into it; several are shown as folders,
    /// because a system whose games live in two places has two places and
    /// pretending otherwise loses one of them.
    pub fn start(&self) -> Place {
        start_for(&self.config)
    }

    /// The rows of a place, folders first and then games, each in the order
    /// a person reads them.
    pub fn list(&self, place: &Place, show_empty: bool) -> Result<(Vec<Row>, ListStats)> {
        let (mut rows, stats) = match place {
            Place::Roots => self.list_roots(show_empty)?,
            Place::Dir(dir) => self.list_dir(dir, show_empty)?,
            Place::Archive(archive) => self.list_archive(archive, "", show_empty)?,
            Place::ArchiveDirectory { archive, prefix } => {
                self.list_archive(archive, prefix, show_empty)?
            }
            Place::Listing { install, file } => self.list_listing(install, file)?,
        };
        // Folders first, exactly as the stock menu orders them, then by
        // name without regard to case: "Zaxxon" must not sort before "apple".
        rows.sort_by(|a, b| {
            b.is_folder()
                .cmp(&a.is_folder())
                .then_with(|| a.sort_key.cmp(&b.sort_key))
        });
        Ok((rows, stats))
    }

    /// The system's folders, one row each, named as they are on the card.
    fn list_roots(&self, show_empty: bool) -> Result<(Vec<Row>, ListStats)> {
        let mut rows = Vec::new();
        let mut stats = ListStats::default();
        for (index, root) in self.roots.iter().enumerate() {
            if !show_empty && self.shows_nothing(&root.path, Some(index), 0)? {
                stats.empty_folders_hidden += 1;
                continue;
            }
            let name = root
                .path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| root.path.to_string_lossy().into_owned());
            stats.folders += 1;
            rows.push(folder_row(name, Place::Dir(root.path.clone())));
        }
        Ok((rows, stats))
    }

    fn list_dir(&self, dir: &Path, show_empty: bool) -> Result<(Vec<Row>, ListStats)> {
        let listing =
            std::fs::read_dir(dir).map_err(|e| DegaussError::io("reading folder", dir, e))?;
        let root = self.root_for(dir);
        // A Neo Geo folder answers to a ROM-set catalogue, under which a
        // ZIP or a folder can be one game rather than something to enter.
        let neogeo = self
            .neogeo
            .as_ref()
            .map(|catalogues| (catalogues, catalogues.for_dir(dir)));
        let install = amiga_install_of(dir);
        // An AmigaVision install keeps its whole library inside one disk
        // image, and the only trace of it on disk is a folder of text files
        // naming the titles. Left where they are, four thousand games sit
        // two folders down behind something called "listings", which is
        // where the user could not find them. They are brought up to the
        // folder that holds the disk image, and the raw folder is left out
        // so nothing appears twice.
        let listings_here = holds_disk_image(dir).then(|| dir.join("listings"));

        let mut rows = Vec::new();
        let mut stats = ListStats::default();

        let entries = listing
            .collect::<std::io::Result<Vec<_>>>()
            .map_err(|error| DegaussError::io("reading folder entry", dir, error))?;
        for item in entries {
            let name = item.file_name().to_string_lossy().into_owned();
            // Dotfiles are not content, and a card that has met a Mac is
            // full of `._` companions that are not games either.
            if name.starts_with('.') {
                continue;
            }
            let path = item.path();

            if entry_is_dir_checked(&item)? {
                // Windows leaves this on every card it touches, and the
                // stock menu does not show it either.
                if name == "System Volume Information" {
                    continue;
                }
                // Surfaced above as the titles they name.
                if listings_here.as_deref() == Some(path.as_path()) {
                    continue;
                }
                if self.is_art_directory(&path, root)
                    || self.is_structural_art_subtree(&path, root)?
                    || self.is_skipped(&name)
                {
                    continue;
                }
                // An unzipped Neo Geo ROM set is a folder of raw components
                // that Main loads as one game. It is listed as that game
                // and never entered: its contents are the loader's.
                if let Some((catalogues, catalogue)) = &neogeo {
                    match catalogues.classify(catalogue, &path, &name, true) {
                        crate::neogeo::Recognition::Game(title) => {
                            self.push_set_row(&mut rows, &mut stats, &path, title, root);
                            continue;
                        }
                        crate::neogeo::Recognition::Hidden => continue,
                        crate::neogeo::Recognition::Unrecognised => {}
                    }
                }
                // A folder with nothing to reach inside it is a dead end,
                // and a card accumulates them.
                if !show_empty && self.shows_nothing(&path, root, 0)? {
                    stats.empty_folders_hidden += 1;
                    continue;
                }
                stats.folders += 1;
                rows.push(folder_row(name, Place::Dir(path)));
                continue;
            }

            let extension = extension_of(&path);
            if let (Some(install), "txt") = (install.as_ref(), extension.as_str()) {
                stats.folders += 1;
                rows.push(folder_row(
                    name,
                    Place::Listing {
                        install: install.clone(),
                        file: path,
                    },
                ));
                continue;
            }

            // An archive opens as a folder unless the core takes it whole.
            if extension == "zip" && !self.config.accepts(&path) {
                // A zipped Neo Geo ROM set is one game under the name the
                // catalogue gives it. Main hands the whole ZIP to the
                // loader, so it is never opened here, not even to look.
                if let Some((catalogues, catalogue)) = &neogeo {
                    match catalogues.classify(catalogue, &path, &name, false) {
                        crate::neogeo::Recognition::Game(title) => {
                            self.push_set_row(&mut rows, &mut stats, &path, title, root);
                            continue;
                        }
                        crate::neogeo::Recognition::Hidden => continue,
                        crate::neogeo::Recognition::Unrecognised => {}
                    }
                }
                stats.folders += 1;
                // Shown without the extension, the way the stock menu shows
                // an archive it can reach into.
                let shown = name.trim_end_matches(".zip").trim_end_matches(".ZIP");
                rows.push(folder_row(shown.to_string(), Place::Archive(path)));
                continue;
            }

            if !self.config.accepts(&path) || is_not_a_game(&name) {
                continue;
            }
            stats.games += 1;
            let row = self.game_row(&path, root);
            if row.cover.is_some() {
                stats.with_art += 1;
            }
            rows.push(row);
        }

        if let Some(listings) = listings_here {
            for (name, file) in amiga_listings(&listings) {
                stats.folders += 1;
                rows.push(folder_row(
                    name,
                    Place::Listing {
                        install: dir.to_path_buf(),
                        file,
                    },
                ));
            }
        }

        Ok((rows, stats))
    }

    /// The launchable files inside an archive. Only names are read; nothing
    /// is unpacked, because unpacking is the loader's job at launch time.
    fn list_archive(
        &self,
        archive: &Path,
        prefix: &str,
        show_empty: bool,
    ) -> Result<(Vec<Row>, ListStats)> {
        let contents = self.archive_cache.borrow_mut().read(archive)?;
        let entries = &contents.entries;
        let supported: Vec<_> = entries
            .iter()
            .filter(|entry| self.config.accepts(Path::new(&entry.name)))
            .collect();
        let legacy_metadata = supported.len() == 1;
        let root = self.root_for(archive);
        let mut rows = Vec::new();
        let mut stats = ListStats::default();
        let mut folders = HashSet::new();
        let directory = if prefix.is_empty() {
            String::new()
        } else {
            format!("{prefix}/")
        };
        if !prefix.is_empty()
            && !entries
                .iter()
                .any(|entry| entry.name.starts_with(&directory))
            && !contents
                .directories
                .iter()
                .any(|entry| entry == prefix || entry.starts_with(&directory))
        {
            return Err(DegaussError::malformed(
                "zip archive",
                archive,
                format!("virtual directory {prefix:?} is missing or was renamed"),
            ));
        }
        if show_empty {
            for entry in
                contents
                    .directories
                    .iter()
                    .map(String::as_str)
                    .chain(entries.iter().filter_map(|entry| {
                        entry.name.rsplit_once('/').map(|(directory, _)| directory)
                    }))
            {
                let Some(relative) = entry.strip_prefix(&directory) else {
                    continue;
                };
                let folder = relative.split('/').next().unwrap_or_default();
                if !folder.is_empty() && folders.insert(folder.to_string()) {
                    stats.folders += 1;
                    rows.push(folder_row(
                        folder.to_string(),
                        Place::ArchiveDirectory {
                            archive: archive.to_path_buf(),
                            prefix: format!("{directory}{folder}"),
                        },
                    ));
                }
            }
        }
        for entry in supported {
            let Some(relative) = entry.name.strip_prefix(&directory) else {
                continue;
            };
            if let Some((folder, _)) = relative.split_once('/') {
                if folders.insert(folder.to_string()) {
                    stats.folders += 1;
                    rows.push(folder_row(
                        folder.to_string(),
                        Place::ArchiveDirectory {
                            archive: archive.to_path_buf(),
                            prefix: format!("{directory}{folder}"),
                        },
                    ));
                }
                continue;
            }
            stats.games += 1;
            let row =
                self.game_row_with_metadata(&archive.join(&entry.name), root, legacy_metadata);
            if row.cover.is_some() {
                stats.with_art += 1;
            }
            rows.push(row);
        }
        Ok((rows, stats))
    }

    /// The titles named by an AmigaVision listing.
    ///
    /// These files are ISO-8859-1 rather than UTF-8, because Amiga titles
    /// are full of European accents. Read as UTF-8 they fail outright and a
    /// library of thousands then looks empty; Latin-1 maps each byte to the
    /// code point of the same value, which is exact rather than a guess.
    fn list_listing(&self, install: &Path, file: &Path) -> Result<(Vec<Row>, ListStats)> {
        let bytes =
            std::fs::read(file).map_err(|e| DegaussError::io("reading listing", file, e))?;
        let text = decode_listing(&bytes);
        let root = self.root_for(file);
        let mut rows = Vec::new();
        let mut stats = ListStats::default();

        for line in text.lines() {
            let title = line.trim();
            if title.is_empty() {
                continue;
            }
            stats.games += 1;
            let mut row = Row {
                name: title.to_string(),
                sort_key: title.to_lowercase(),
                kind: Kind::Play(Launch::AmigaVision {
                    install: install.to_path_buf(),
                    title: title.to_string(),
                }),
                cover: None,
                genre: None,
                favorite: false,
                below: None,
                details: Details::default(),
            };
            self.bind(&mut row, title, root);
            if row.cover.is_some() {
                stats.with_art += 1;
            }
            rows.push(row);
        }
        Ok((rows, stats))
    }

    fn game_row(&self, path: &Path, root: Option<usize>) -> Row {
        self.game_row_with_metadata(path, root, true)
    }

    /// A recognised Neo Geo ROM set, zipped or not, as the game row the
    /// catalogue titles it, counted with the games.
    fn push_set_row(
        &self,
        rows: &mut Vec<Row>,
        stats: &mut ListStats,
        path: &Path,
        title: String,
        root: Option<usize>,
    ) {
        stats.games += 1;
        let row = self.row_for(path, title, root, true);
        if row.cover.is_some() {
            stats.with_art += 1;
        }
        rows.push(row);
    }

    fn game_row_with_metadata(&self, path: &Path, root: Option<usize>, legacy: bool) -> Row {
        self.row_for(path, self.display_name(path), root, legacy)
    }

    /// A playable row under a name already decided, with whatever the
    /// metadata overlay adds to it.
    fn row_for(&self, path: &Path, name: String, root: Option<usize>, legacy: bool) -> Row {
        let mut row = Row {
            sort_key: name.to_lowercase(),
            name,
            kind: Kind::Play(Launch::File(path.to_path_buf())),
            cover: None,
            genre: None,
            favorite: false,
            below: None,
            details: Details::default(),
        };
        let rel = root
            .and_then(|index| path.strip_prefix(&self.roots[index].path).ok())
            .unwrap_or(path)
            .to_string_lossy()
            .into_owned();
        self.bind_with_metadata(&mut row, &rel, root, legacy);
        row
    }

    /// What the stock menu would print for this file.
    ///
    /// Two rules, both taken from Main rather than invented. A file that
    /// describes itself (a core, an `.mra` board, an `.mgl` shortcut) loses
    /// its extension and may be renamed by `names.txt`. Anything else keeps
    /// its extension unless the system launches exactly one, because a
    /// library holding a game as both a disk and a cartridge would otherwise
    /// show two rows with identical names and no way to tell them apart.
    fn display_name(&self, path: &Path) -> String {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.to_string_lossy().into_owned());
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_else(|| file_name.clone());

        match extension_of(path).as_str() {
            "rbf" if self.config.preserve_rbf_stem => stem,
            "rbf" => {
                // A core carries the date it was built: NeoGeo_20260603.rbf.
                // Main cuts it at "_20" when what follows is long enough to
                // be a date, and so does this, or every core in the list
                // reads as a name with a number stuck on the end.
                let name = match stem.find("_20") {
                    Some(at) if stem.len() - at - 3 >= 6 => stem[..at].to_string(),
                    _ => stem,
                };
                self.names.get(&name).map(str::to_string).unwrap_or(name)
            }
            "mra" | "mgl" => self.names.get(&stem).map(str::to_string).unwrap_or(stem),
            _ if self.config.extensions.len() == 1 => stem,
            _ => file_name,
        }
    }

    /// Attach whatever the metadata overlay knows about a row.
    fn bind(&self, row: &mut Row, key: &str, root: Option<usize>) {
        self.bind_with_metadata(row, key, root, true);
    }

    fn bind_with_metadata(&self, row: &mut Row, key: &str, root: Option<usize>, legacy: bool) {
        let Some(meta) = root
            .and_then(|index| self.roots[index].gamelist.as_ref())
            .and_then(|list| {
                if legacy {
                    list.lookup(key)
                } else {
                    list.lookup_exact(key)
                }
            })
            .map(|(meta, _)| meta.clone())
        else {
            return;
        };
        if let Some(name) = meta.name {
            row.sort_key = name.to_lowercase();
            row.name = name;
        }
        row.genre = meta.genre;
        row.favorite = meta.favorite;
        row.details = Details {
            desc: meta.desc.unwrap_or_default(),
            publisher: meta.publisher.unwrap_or_default(),
            developer: meta.developer.unwrap_or_default(),
            released: meta.released.unwrap_or_default(),
            players: meta.players.unwrap_or_default(),
            lang: meta.lang.unwrap_or_default(),
        };
        if let Some(image) = meta.image {
            // Answered from the directory index, never by asking the
            // filesystem about one file at a time.
            let present = root.is_some_and(|index| self.roots[index].art.contains(&image));
            if present {
                row.cover = Some(image);
            }
        }
    }

    /// Every cover reachable from the top of this library, up to `want`,
    /// looking in at most `folders` places, with the title it belongs to.
    ///
    /// Breadth-first on purpose. Walking one random branch down and giving
    /// up at the first dead end finds nothing on the many systems whose
    /// games sit one folder in and whose top level holds only artwork.
    pub fn covers(&self, folders: usize, want: usize) -> Vec<(PathBuf, String)> {
        self.covers_projected(folders, want, &mut |_| {})
    }

    pub fn covers_projected(
        &self,
        folders: usize,
        want: usize,
        project: &mut dyn FnMut(&mut [Row]),
    ) -> Vec<(PathBuf, String)> {
        let mut found = Vec::new();
        let mut queue = std::collections::VecDeque::from([self.start()]);
        let mut looked = 0;

        while let Some(place) = queue.pop_front() {
            if looked >= folders || found.len() >= want {
                break;
            }
            looked += 1;
            let Ok((mut rows, _)) = self.list(&place, false) else {
                continue;
            };
            project(&mut rows);
            for row in &rows {
                if found.len() >= want {
                    break;
                }
                if let Some(picture) = row.cover.clone() {
                    if !found.iter().any(|(held, _)| *held == picture) {
                        found.push((picture, row.name.clone()));
                    }
                }
            }
            for row in rows {
                if let Kind::Enter(inner) = row.kind {
                    queue.push_back(inner);
                }
            }
        }
        found
    }

    /// The declared folder this path sits under, which decides whose
    /// gamelist and whose artwork answer for it.
    fn root_for(&self, path: &Path) -> Option<usize> {
        self.roots
            .iter()
            .enumerate()
            .filter(|(_, root)| path.starts_with(&root.path))
            // The deepest match wins, or a root nested inside another root
            // would be answered by the wrong gamelist.
            .max_by_key(|(_, root)| root.path.as_os_str().len())
            .map(|(index, _)| index)
    }

    /// True when there is nothing anywhere under this folder worth
    /// reaching: no game at any depth, and no folder that leads to one.
    ///
    /// A folder holding only empty folders is as much a dead end as an
    /// empty one, and so is a folder holding only those. The question is
    /// not "is this empty" but "does walking in here ever arrive
    /// anywhere", so the answer has to look all the way down.
    ///
    /// It costs less than it sounds. The search stops at the first thing it
    /// would show, so a folder of games answers on its first entry; only a
    /// tree that really is empty is walked to the bottom, and an empty tree
    /// is cheap to walk.
    fn shows_nothing(&self, dir: &Path, root: Option<usize>, depth: usize) -> Result<bool> {
        if depth > MAX_DEPTH {
            // Too deep to keep asking. Say it shows something, so the worst
            // a symlink loop can do is leave one folder visible.
            return Ok(false);
        }
        // Asked once for the folder, not once per file in it: it is a
        // property of the folder, and answering it rescans the parent.
        let amiga_install = amiga_install_of(dir).is_some();
        let neogeo = self
            .neogeo
            .as_ref()
            .map(|catalogues| (catalogues, catalogues.for_dir(dir)));
        let listing = std::fs::read_dir(dir)
            .map_err(|error| DegaussError::io("reading folder", dir, error))?;
        for item in listing {
            let item =
                item.map_err(|error| DegaussError::io("reading folder entry", dir, error))?;
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "System Volume Information" {
                continue;
            }
            let path = item.path();
            let is_dir = entry_is_dir_checked(&item)?;
            // A folder left out of the listing leads nowhere whatever it holds.
            let excluded = is_dir && (self.is_art_directory(&path, root) || self.is_skipped(&name));
            // What a file is, asked once: the set check and the content
            // check below both want it.
            let extension = if is_dir {
                String::new()
            } else {
                extension_of(&path)
            };
            let accepted = !is_dir && self.config.accepts(&path);
            // A Neo Geo ROM set is a game whether zipped or not, so a folder
            // of nothing but sets is worth walking into; one the catalogue
            // hides would never be shown and is not.
            if let Some((catalogues, catalogue)) = &neogeo {
                let candidate = if is_dir {
                    !excluded
                } else {
                    extension == "zip" && !accepted
                };
                if candidate {
                    match catalogues.classify(catalogue, &path, &name, is_dir) {
                        crate::neogeo::Recognition::Game(_) => return Ok(false),
                        crate::neogeo::Recognition::Hidden => continue,
                        crate::neogeo::Recognition::Unrecognised => {}
                    }
                }
            }
            if is_dir {
                if excluded || self.shows_nothing(&path, root, depth + 1)? {
                    continue;
                }
                return Ok(false);
            }
            // A file only counts if it is one this system can open, an
            // archive that opens like a folder, or a listing naming titles
            // held inside a disk image.
            if extension == "zip" || (accepted && !is_not_a_game(&name)) {
                return Ok(false);
            }
            if extension == "txt" && amiga_install {
                return Ok(false);
            }
        }
        Ok(true)
    }

    fn is_art_directory(&self, path: &Path, root: Option<usize>) -> bool {
        root.is_some_and(|index| self.roots[index].art.is_art_directory(path))
    }

    fn is_structural_art_subtree(&self, path: &Path, root: Option<usize>) -> Result<bool> {
        if root.is_some_and(|index| self.roots[index].art.has_structural_art_below(path)) {
            self.shows_nothing(path, root, 0)
        } else {
            Ok(false)
        }
    }

    fn is_skipped(&self, name: &str) -> bool {
        self.config
            .skip_folders
            .iter()
            .any(|skip| skip.eq_ignore_ascii_case(name))
    }
}

const MEDIA_CLASSIFIER_ENTRY_LIMIT: usize = 100_000;

/// Conservative source-independent fallback for a malformed gamelist. It
/// suppresses only non-root subtrees that contain image media and no file the
/// system could launch. Hitting a bound is treated as possible game content,
/// so uncertainty leaves a folder visible rather than hiding games.
fn fallback_media_directories(root: &Path, config: &SystemConfig) -> Vec<PathBuf> {
    struct Scan<'a> {
        config: &'a SystemConfig,
        seen: HashSet<PathBuf>,
        entries: usize,
        excluded: HashSet<PathBuf>,
    }

    fn walk(scan: &mut Scan<'_>, root: &Path, dir: &Path, depth: usize) -> (bool, bool) {
        if depth > MAX_DEPTH || scan.entries >= MEDIA_CLASSIFIER_ENTRY_LIMIT {
            return (false, true);
        }
        let identity = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        if !scan.seen.insert(identity) {
            return (false, true);
        }
        let Ok(listing) = std::fs::read_dir(dir) else {
            return (false, true);
        };
        let mut media = false;
        let mut launchable = false;
        for item in listing {
            if scan.entries >= MEDIA_CLASSIFIER_ENTRY_LIMIT {
                launchable = true;
                break;
            }
            scan.entries += 1;
            let Ok(item) = item else {
                launchable = true;
                continue;
            };
            let path = item.path();
            if entry_is_dir(&item) {
                let (below_media, below_launchable) = walk(scan, root, &path, depth + 1);
                media |= below_media;
                launchable |= below_launchable;
                continue;
            }
            let extension = extension_of(&path);
            media |= matches!(extension.as_str(), "jpg" | "jpeg" | "png");
            launchable |= extension == "zip" || scan.config.accepts(&path);
        }
        if dir != root && media && !launchable {
            scan.excluded.insert(dir.to_path_buf());
        }
        (media, launchable)
    }

    let mut scan = Scan {
        config,
        seen: HashSet::new(),
        entries: 0,
        excluded: HashSet::new(),
    };
    let _ = walk(&mut scan, root, root, 0);
    let mut excluded: Vec<PathBuf> = scan.excluded.into_iter().collect();
    excluded.sort();
    excluded
}

fn folder_row(name: String, place: Place) -> Row {
    Row {
        sort_key: name.to_lowercase(),
        name,
        kind: Kind::Enter(place),
        cover: None,
        genre: None,
        favorite: false,
        below: None,
        details: Details::default(),
    }
}

/// True for the boot roms and blank images a core needs and nobody plays.
///
/// Matched on the whole name rather than a prefix: `boot1.rom` is the
/// ao486 BIOS, but a game legitimately called `Bootleg.rom` is not.
fn is_not_a_game(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    if NOT_GAMES.iter().any(|known| *known == lower) {
        return true;
    }
    // boot0.rom, boot1.rom, boot3.vhd: the same file numbered, which is how
    // the DOS, 3DO, CD-i and X68000 cores ship theirs.
    let numbered = lower.strip_prefix("boot").and_then(|rest| {
        let (digits, ext) = rest.split_at(rest.find('.').unwrap_or(rest.len()));
        (!digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())).then_some(ext)
    });
    matches!(numbered, Some(".rom") | Some(".vhd"))
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|ext| ext.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The listings an AmigaVision install carries, as a readable name and the
/// file that holds the titles. `games.txt` reads as "Games".
fn amiga_listings(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(String, PathBuf)> = listing
        .flatten()
        .filter(|item| extension_of(&item.path()) == "txt")
        .map(|item| {
            let path = item.path();
            let stem = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut chars = stem.chars();
            let name = match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => stem,
            };
            (name, path)
        })
        .collect();
    found.sort_by_key(|(name, _)| name.to_lowercase());
    found
}

/// The AmigaVision install a folder belongs to, when it is the `listings`
/// folder of one. Its text files name titles held inside the disk image.
fn amiga_install_of(dir: &Path) -> Option<PathBuf> {
    if !dir
        .file_name()
        .is_some_and(|name| name.eq_ignore_ascii_case("listings"))
    {
        return None;
    }
    dir.parent()
        .filter(|parent| holds_disk_image(parent))
        .map(Path::to_path_buf)
}

fn holds_disk_image(dir: &Path) -> bool {
    let Ok(listing) = std::fs::read_dir(dir) else {
        return false;
    };
    listing
        .flatten()
        .any(|item| extension_of(&item.path()) == "hdf")
}

fn decode_listing(bytes: &[u8]) -> String {
    match std::str::from_utf8(bytes) {
        Ok(text) => text.to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// A tally of everything one system holds, gathered by walking it the same
/// way a person would.
///
/// The point of this is not statistics. It answers, for every system on the
/// card at once, the only two questions worth asking: are the games there,
/// and is the artwork bound to them. Checking that by opening each system by
/// hand is what it replaces.
#[derive(Debug, Default)]
pub struct Audit {
    pub games: usize,
    pub folders: usize,
    pub with_art: usize,
    pub places_read: usize,
    pub deepest: usize,
    /// Places that could not be read, which is the interesting failure: a
    /// missing folder is normal, a folder that errors is not.
    pub unreadable: Vec<(PathBuf, String)>,
    /// Set when the walk stopped early, so a total is never reported as
    /// complete when it is not.
    pub stopped_at_limit: bool,
    pub first_game: Option<Row>,
}

/// How many places one audit will open. A card holds folders of symlinks
/// pointing at other folders of symlinks, and the walk must end.
const AUDIT_LIMIT: usize = 20_000;

impl Library {
    /// Walk everything this system holds and count it.
    pub fn audit(&self, show_empty: bool) -> Audit {
        self.audit_projected(show_empty, &mut |_| {})
    }

    pub fn audit_projected(&self, show_empty: bool, project: &mut dyn FnMut(&mut [Row])) -> Audit {
        let mut audit = Audit {
            unreadable: self.structural_problems.clone(),
            ..Audit::default()
        };
        let mut stack: Vec<(Place, usize)> = self
            .roots
            .iter()
            .map(|root| (Place::Dir(root.path.clone()), 0))
            .collect();
        let mut seen: HashSet<(String, Option<String>)> = HashSet::new();

        while let Some((place, depth)) = stack.pop() {
            if audit.places_read >= AUDIT_LIMIT {
                audit.stopped_at_limit = true;
                break;
            }
            if depth > MAX_DEPTH {
                continue;
            }
            // The same folder reached twice is counted once. Organised
            // collections are built out of links back into the same tree.
            let prefix = match &place {
                Place::ArchiveDirectory { prefix, .. } => Some(prefix.clone()),
                _ => None,
            };
            if !seen.insert((key(place.path()), prefix)) {
                continue;
            }
            audit.places_read += 1;
            audit.deepest = audit.deepest.max(depth);

            match self.list(&place, show_empty) {
                Ok((mut rows, _)) => {
                    project(&mut rows);
                    // Projection can replace a game's display name and sort
                    // key. CLI report, audit and dry-run must therefore walk
                    // the same effective order as the interactive browser.
                    rows.sort_by(|left, right| {
                        right
                            .is_folder()
                            .cmp(&left.is_folder())
                            .then_with(|| left.sort_key.cmp(&right.sort_key))
                    });
                    for row in rows {
                        match row.kind {
                            Kind::Enter(inner) => {
                                audit.folders += 1;
                                stack.push((inner, depth + 1));
                            }
                            Kind::Play(_) => {
                                audit.games += 1;
                                if row.cover.is_some() {
                                    audit.with_art += 1;
                                }
                                if audit.first_game.is_none() {
                                    audit.first_game = Some(row);
                                }
                            }
                        }
                    }
                }
                Err(e) => audit
                    .unreadable
                    .push((place.path().to_path_buf(), e.to_string())),
            }
        }
        // A ROM-set catalogue that could not be read is the same kind of
        // failure as a folder that could not be: its sets are then listed
        // as plain archives and folders, and the owner should know why.
        audit.unreadable.extend(self.catalogue_problems());
        audit
    }

    /// Every Neo Geo ROM-set catalogue met so far that could not be read,
    /// with the reason. The sets it would have named were listed as plain
    /// archives and folders instead, which is why an index built from this
    /// library reports them: on the screen a broken catalogue would
    /// otherwise look like a folder of archives. Empty for every other
    /// system.
    pub fn catalogue_problems(&self) -> Vec<(PathBuf, String)> {
        self.neogeo
            .as_ref()
            .map(crate::neogeo::Catalogues::problems)
            .unwrap_or_default()
    }

    /// The catalogue problems met since this was last asked, for a
    /// listing read straight from the card with no index to report them:
    /// each is said once, with the folder whose sets it degraded.
    pub fn unannounced_catalogue_problems(&self) -> Vec<(PathBuf, String)> {
        self.neogeo
            .as_ref()
            .map(crate::neogeo::Catalogues::unannounced_problems)
            .unwrap_or_default()
    }

    /// The system's name as its definition gives it.
    pub fn system_name(&self) -> &str {
        &self.config.name
    }

    /// Whether each declared folder carries a metadata overlay, for the
    /// audit: a system with no artwork and no gamelist is explained, one
    /// with a gamelist and no artwork is a problem.
    pub fn gamelists(&self) -> Vec<(PathBuf, bool)> {
        self.roots
            .iter()
            .map(|root| {
                let path = root.path.clone();
                let present = path.join("gamelist.xml").is_file();
                (path, present)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SystemConfig;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "degauss-browse-{tag}-{}-{:p}",
            std::process::id(),
            &tag
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    fn system(path: &Path) -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Commodore 64".to_string(),
            path: path.to_string_lossy().into_owned(),
            extensions: vec!["d64".to_string(), "prg".to_string()],
            rbf: "_Computer/C64".to_string(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
            extra_paths: Vec::new(),
        }
    }

    fn names_of(rows: &[Row]) -> Vec<String> {
        rows.iter().map(|row| row.name.clone()).collect()
    }

    #[test]
    fn a_file_that_describes_itself_loses_its_extension_and_can_be_renamed() {
        // How the stock menu prints an arcade board or a shortcut, including
        // the rename the card's own names.txt asks for.
        let dir = temp("names");
        std::fs::write(dir.join("AcornAtom.mra"), b"x").unwrap();
        std::fs::write(dir.join("Superman.mra"), b"x").unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["mra".to_string(), "mgl".to_string()];

        let names_file = dir.join("names.txt");
        std::fs::write(&names_file, "AcornAtom:          Atom\n").unwrap();
        let names = DisplayNames::read(&names_file);
        assert_eq!(names.len(), 1);

        let library = Library::open_with_names(&config, names).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Atom", "Superman"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_core_is_named_without_the_date_it_was_built() {
        let dir = temp("core-names");
        std::fs::write(dir.join("Arduboy_20250824.rbf"), b"core").unwrap();
        std::fs::write(dir.join("NeoGeoPocket-Color.rbf"), b"core").unwrap();
        // Not a date: too short to be one, so the name keeps it.
        std::fs::write(dir.join("Thing_2049.rbf"), b"core").unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["rbf".to_string()];

        let library = Library::open(&config).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(
            names_of(&rows),
            vec!["Arduboy", "NeoGeoPocket-Color", "Thing_2049"]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_key_is_matched_whole_and_not_as_a_substring_of_another() {
        // Main looks this up by searching the file for "Atom:", which the
        // "AcornAtom:" line answers first. That is a bug, not a rule.
        let dir = temp("names-substring");
        let file = dir.join("names.txt");
        std::fs::write(&file, "AcornAtom: Atom\nAtom: Acorn Atom 2\n").unwrap();
        let names = DisplayNames::read(&file);
        assert_eq!(names.get("Atom"), Some("Acorn Atom 2"));
        assert_eq!(names.get("AcornAtom"), Some("Atom"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_extension_stays_when_a_system_launches_more_than_one() {
        // The same game held as a disk and as a cartridge must not appear
        // twice under one name with no way to tell which is which.
        let dir = temp("two-extensions");
        std::fs::write(dir.join("Uridium.d64"), b"x").unwrap();
        std::fs::write(dir.join("Uridium.prg"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Uridium.d64", "Uridium.prg"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_extension_goes_when_a_system_launches_exactly_one() {
        let dir = temp("one-extension");
        std::fs::write(dir.join("Uridium.d64"), b"x").unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["d64".to_string()];

        let library = Library::open(&config).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Uridium"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_folder_windows_leaves_behind_is_not_shown() {
        let dir = temp("svi");
        std::fs::create_dir_all(dir.join("System Volume Information")).unwrap();
        std::fs::write(dir.join("System Volume Information/x"), b"x").unwrap();
        std::fs::write(dir.join("Real.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Real.d64"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_shows_its_subfolders_and_its_games_folders_first() {
        // The point of the whole model: what is on the card is what is on
        // screen, in the order the stock menu shows it.
        let dir = temp("order");
        std::fs::create_dir_all(dir.join("Demos")).unwrap();
        std::fs::write(dir.join("Demos/keep.d64"), b"x").unwrap();
        std::fs::write(dir.join("zeta.d64"), b"x").unwrap();
        std::fs::write(dir.join("Alpha.PRG"), b"x").unwrap();
        std::fs::write(dir.join("notes.txt"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();

        assert_eq!(names_of(&rows), vec!["Demos", "Alpha.PRG", "zeta.d64"]);
        assert!(rows[0].is_folder());
        assert_eq!(stats.folders, 1);
        assert_eq!(stats.games, 2);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn entering_a_subfolder_lists_that_subfolder_and_nothing_else() {
        // Flattening is what produced duplicates from organised folders,
        // where the same game is filed under several groupings.
        let dir = temp("enter");
        std::fs::create_dir_all(dir.join("By Genre/Shooters")).unwrap();
        std::fs::write(dir.join("By Genre/Shooters/uridium.d64"), b"x").unwrap();
        std::fs::write(dir.join("uridium.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (top, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&top), vec!["By Genre", "uridium.d64"]);

        let Kind::Enter(place) = &top[0].kind else {
            panic!("a folder must be enterable");
        };
        let (inner, _) = library.list(place, false).unwrap();
        assert_eq!(names_of(&inner), vec!["Shooters"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_empty_folder_is_hidden_unless_asked_for() {
        let dir = temp("empty");
        std::fs::create_dir_all(dir.join("Nothing Here")).unwrap();
        std::fs::create_dir_all(dir.join("Has Something")).unwrap();
        std::fs::write(dir.join("Has Something/game.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (hidden, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&hidden), vec!["Has Something"]);
        assert_eq!(stats.empty_folders_hidden, 1);

        let (shown, _) = library.list(&library.start(), true).unwrap();
        assert_eq!(names_of(&shown), vec!["Has Something", "Nothing Here"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_whose_only_content_is_artwork_is_a_dead_end_and_is_hidden() {
        // Every scraped card has one: `media` holds the screenshot folder
        // and nothing else, so it is a real folder on disk that opens onto
        // an empty list.
        let dir = temp("art-only");
        std::fs::create_dir_all(dir.join("media/screenshot")).unwrap();
        // The empty folder a scraper leaves behind when it was told to
        // fetch box art and never found any. Real cards have these.
        std::fs::create_dir_all(dir.join("media/boxart2d")).unwrap();
        std::fs::write(dir.join("media/screenshot/bd.png"), b"x").unwrap();
        std::fs::write(dir.join("bd.d64"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>./bd.d64</path>
               <image>./media/screenshot/bd.png</image></game></gameList>"#,
        )
        .unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["bd.d64"]);

        // Asked for, it is there: nothing is being hidden permanently.
        let (all, _) = library.list(&library.start(), true).unwrap();
        assert!(all.iter().any(|row| row.name == "media"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_that_only_leads_to_empty_folders_is_hidden_too() {
        // The rule is "would opening this show an empty list", not "is this
        // literally empty": a folder whose only content is an empty folder
        // is the same dead end one step further away.
        let dir = temp("nested-empty");
        std::fs::create_dir_all(dir.join("Outer/Inner")).unwrap();
        std::fs::create_dir_all(dir.join("Real/Inner")).unwrap();
        std::fs::write(dir.join("Real/Inner/game.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Real"]);

        let (all, _) = library.list(&library.start(), true).unwrap();
        assert_eq!(names_of(&all), vec!["Outer", "Real"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_chain_of_folders_leading_nowhere_is_hidden_however_long_it_is() {
        // One empty folder inside another inside another is still nothing
        // to walk into, and a card collects these.
        let dir = temp("deep-empty");
        std::fs::create_dir_all(dir.join("Nothing/Under/Here/At/All")).unwrap();
        std::fs::create_dir_all(dir.join("Something/Deep/Down")).unwrap();
        std::fs::write(dir.join("Something/Deep/Down/game.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Something"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_holding_only_files_no_core_can_open_is_hidden() {
        // A folder of notes and box scans is not a dead end because it is
        // empty; it is a dead end because nothing in it can be launched.
        let dir = temp("unlaunchable");
        std::fs::create_dir_all(dir.join("Docs")).unwrap();
        std::fs::write(dir.join("Docs/manual.txt"), b"x").unwrap();
        std::fs::write(dir.join("Docs/scan.png"), b"x").unwrap();
        std::fs::write(dir.join("real.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["real.d64"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_two_levels_from_its_games_is_still_shown() {
        // The check is shallow on purpose, and this is the case it must not
        // get wrong: deciding a folder is a dead end when the games are
        // simply further down would hide a whole library.
        let dir = temp("deep-games");
        std::fs::create_dir_all(dir.join("A/B/C")).unwrap();
        std::fs::write(dir.join("A/B/C/game.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["A"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_gamelist_supplies_the_name_and_the_picture() {
        let dir = temp("meta");
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("bd.d64"), b"x").unwrap();
        std::fs::write(dir.join("media/bd.png"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>./bd.d64</path><name>Boulder Dash</name>
               <image>./media/bd.png</image></game></gameList>"#,
        )
        .unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        let game = rows.iter().find(|row| !row.is_folder()).expect("a game");
        assert_eq!(game.name, "Boulder Dash");
        assert_eq!(game.cover, Some(dir.join("media/bd.png")));
        assert_eq!(stats.with_art, 1);

        // The art directory is not a folder of games and must not be shown.
        assert!(!rows.iter().any(|row| row.name == "media"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn source_neutral_mode_uses_only_gamelist_structure() {
        let dir = temp("source-neutral-structure");
        std::fs::create_dir_all(dir.join("custom/captures")).unwrap();
        std::fs::write(dir.join("Disk Name.d64"), b"x").unwrap();
        std::fs::write(dir.join("custom/captures/red.png"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>./Disk Name.d64</path><name>Gamelist Name</name>
               <image>./custom/captures/red.png</image><genre>Gamelist Genre</genre>
               </game></gameList>"#,
        )
        .unwrap();

        let library = Library::open_source_neutral(&system(&dir), DisplayNames::default()).unwrap();
        for show_empty in [false, true] {
            let (rows, _) = library.list(&library.start(), show_empty).unwrap();
            assert_eq!(names_of(&rows), vec!["Disk Name.d64"]);
            let game = rows.first().expect("source-neutral game");
            assert_eq!(game.cover, None);
            assert_eq!(game.genre, None);
            assert_eq!(game.details, Details::default());
        }
        assert_eq!(library.gamelists(), vec![(dir.clone(), true)]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_gamelist_falls_back_conservatively_and_is_audited() {
        let dir = temp("source-neutral-malformed");
        std::fs::create_dir_all(dir.join("media/screenshots")).unwrap();
        std::fs::write(dir.join("media/screenshots/red.png"), b"x").unwrap();
        std::fs::write(dir.join("Real.d64"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            "<gameList><game><image>./media/screenshots/red.png</gameList>",
        )
        .unwrap();

        let library = Library::open_source_neutral(&system(&dir), DisplayNames::default()).unwrap();
        for show_empty in [false, true] {
            let (rows, _) = library.list(&library.start(), show_empty).unwrap();
            assert_eq!(names_of(&rows), vec!["Real.d64"]);
        }
        let audit = library.audit(false);
        assert_eq!(audit.games, 1);
        assert!(audit
            .unreadable
            .iter()
            .any(|(path, _)| path == &dir.join("gamelist.xml")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn malformed_gamelist_fallback_never_hides_a_media_folder_with_games() {
        let dir = temp("source-neutral-media-games");
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/cover.png"), b"x").unwrap();
        std::fs::write(dir.join("media/Playable.d64"), b"x").unwrap();
        std::fs::write(dir.join("gamelist.xml"), "<gameList><broken>").unwrap();

        let library = Library::open_source_neutral(&system(&dir), DisplayNames::default()).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["media"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn art_named_by_the_gamelist_but_absent_from_the_card_is_not_claimed() {
        let dir = temp("missing-art");
        std::fs::write(dir.join("bd.d64"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>./bd.d64</path>
               <image>./media/gone.png</image></game></gameList>"#,
        )
        .unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(rows[0].cover, None);
        assert_eq!(stats.with_art, 0);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_archive_opens_like_a_folder_and_its_contents_are_launchable() {
        let dir = temp("zip");
        let archive = dir.join("Neo Geo.zip");
        std::fs::write(&archive, crate::zip::tests_fixture()).unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["neo".to_string()];

        let library = Library::open(&config).unwrap();
        let (top, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&top), vec!["Neo Geo"]);

        let Kind::Enter(place) = &top[0].kind else {
            panic!("an archive must open like a folder");
        };
        let (inside, stats) = library.list(place, false).unwrap();
        assert_eq!(names_of(&inside), vec!["sub", "Metal Slug"]);
        assert_eq!(stats.games, 1);
        let Kind::Enter(nested) = &inside[0].kind else {
            panic!("implied directory");
        };
        let (nested, _) = library.list(nested, false).unwrap();
        assert_eq!(names_of(&nested), ["Another Game"]);
        // readme.txt is in the archive but no core loads it.
        assert!(!inside.iter().any(|row| row.name.contains("readme")));

        let Kind::Play(Launch::File(path)) = &inside[1].kind else {
            panic!("must be launchable");
        };
        assert_eq!(path, &archive.join("Metal Slug.neo"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn audit_visits_nested_archive_directories_with_exact_member_casing() {
        let dir = temp("audit-nested-zip");
        // ASCII case aliases are rejected by Main-compatible archive validation.
        // These UTF-8 prefixes remain distinct and must not be lowercased by audit.
        std::fs::write(
            dir.join("library.zip"),
            crate::zip::tests_archive(&["Ä/One.d64", "ä/Two.d64"], false),
        )
        .unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let audit = library.audit(false);
        assert_eq!(audit.games, 2);
        assert!(audit.unreadable.is_empty());
        assert!(audit.first_game.is_some());
        assert_eq!(
            audit.places_read, 4,
            "root, archive and both member directories"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn an_archive_the_core_takes_whole_stays_one_game() {
        let dir = temp("whole-zip");
        std::fs::write(
            dir.join("Super Mario World.zip"),
            crate::zip::tests_fixture(),
        )
        .unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["sfc".to_string(), "zip".to_string()];

        let library = Library::open(&config).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(stats.games, 1);
        assert!(!rows[0].is_folder());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn amigavision_titles_are_found_beside_the_disk_image_not_two_folders_down() {
        // Four thousand games live inside one .hdf. The only trace of them
        // on disk is a folder of text files, and buried there is where the
        // user could not find them.
        let dir = temp("amiga");
        std::fs::write(dir.join("AmigaVision.hdf"), b"disk image").unwrap();
        std::fs::create_dir_all(dir.join("listings")).unwrap();
        std::fs::write(dir.join("listings/games.txt"), "Turrican II\nAgony\n").unwrap();
        std::fs::write(dir.join("listings/demos.txt"), "State of the Art\n").unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["hdf".to_string()];

        let library = Library::open(&config).unwrap();
        let (top, _) = library.list(&library.start(), false).unwrap();
        let shown = names_of(&top);
        assert!(shown.contains(&"Games".to_string()), "got: {shown:?}");
        assert!(shown.contains(&"Demos".to_string()), "got: {shown:?}");
        assert!(
            !shown.contains(&"listings".to_string()),
            "the raw folder would show the same games twice"
        );

        let entry = top.iter().find(|row| row.name == "Games").unwrap();
        let Kind::Enter(place) = &entry.kind else {
            panic!("a listing opens like a folder")
        };
        let (titles, stats) = library.list(place, false).unwrap();
        assert_eq!(names_of(&titles), vec!["Agony", "Turrican II"]);
        assert_eq!(stats.games, 2);
        assert!(matches!(
            &titles[0].kind,
            Kind::Play(Launch::AmigaVision { title, .. }) if title == "Agony"
        ));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_listing_that_is_not_utf8_still_names_its_games() {
        // The real files are ISO-8859-1. Read as UTF-8 they fail outright,
        // and a library of thousands then looks empty.
        let dir = temp("latin1");
        std::fs::write(dir.join("AmigaVision.hdf"), b"disk image").unwrap();
        std::fs::create_dir_all(dir.join("listings")).unwrap();
        let mut line = "B".as_bytes().to_vec();
        line.extend_from_slice(&[0xe9]); // é in Latin-1, invalid UTF-8
        line.extend_from_slice(b"zier\n");
        std::fs::write(dir.join("listings/games.txt"), &line).unwrap();
        let mut config = system(&dir);
        config.extensions = vec!["hdf".to_string()];

        let library = Library::open(&config).unwrap();
        let place = Place::Listing {
            install: dir.clone(),
            file: dir.join("listings/games.txt"),
        };
        let (titles, _) = library.list(&place, false).unwrap();
        assert_eq!(names_of(&titles), vec!["Bézier"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn every_folder_a_system_declares_is_reachable() {
        // The bug this guards cost a whole library: DOS keeps its disk
        // images in games/AO486 and the shortcuts that launch them, with all
        // the metadata, in _DOS Games. Opening only the first folder showed
        // boot roms and nothing else.
        let first = temp("roots-a");
        let second = temp("roots-b");
        std::fs::write(first.join("boot.rom"), b"x").unwrap();
        std::fs::write(first.join("one.d64"), b"x").unwrap();
        std::fs::write(second.join("two.d64"), b"x").unwrap();

        let mut config = system(&first);
        config.extra_paths = vec![second.to_string_lossy().into_owned()];
        let library = Library::open(&config).unwrap();

        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(rows.len(), 2, "both folders, not just the first");
        assert!(rows.iter().all(|row| row.is_folder()));

        // And each one opens onto its own contents.
        let mut seen = Vec::new();
        for row in &rows {
            let Kind::Enter(place) = &row.kind else {
                panic!("a folder must be enterable")
            };
            let (inner, _) = library.list(place, false).unwrap();
            seen.extend(names_of(&inner));
        }
        seen.sort();
        assert_eq!(seen, vec!["one.d64", "two.d64"]);
        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&second).ok();
    }

    #[test]
    fn a_system_with_one_folder_opens_straight_into_it() {
        let dir = temp("one-root");
        std::fs::write(dir.join("game.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(
            names_of(&rows),
            vec!["game.d64"],
            "no pointless extra level"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_second_declared_folder_keeps_its_own_gamelist() {
        let first = temp("root-a");
        let second = temp("root-b");
        std::fs::write(first.join("game.d64"), b"x").unwrap();
        std::fs::write(
            first.join("gamelist.xml"),
            r#"<gameList><game><path>./game.d64</path><name>First</name></game></gameList>"#,
        )
        .unwrap();
        std::fs::write(second.join("game.d64"), b"x").unwrap();
        std::fs::write(
            second.join("gamelist.xml"),
            r#"<gameList><game><path>./game.d64</path><name>Second</name></game></gameList>"#,
        )
        .unwrap();

        let mut config = system(&first);
        config.extra_paths = vec![second.to_string_lossy().into_owned()];
        let library = Library::open(&config).unwrap();

        let (a, _) = library.list(&Place::Dir(first.clone()), false).unwrap();
        let (b, _) = library.list(&Place::Dir(second.clone()), false).unwrap();
        assert_eq!(a[0].name, "First");
        assert_eq!(b[0].name, "Second");
        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&second).ok();
    }

    #[test]
    fn a_folder_that_cannot_be_read_says_so_rather_than_looking_empty() {
        let library = Library::open(&system(Path::new("/definitely/not/here"))).unwrap();
        let err = library
            .list(&library.start(), false)
            .expect_err("must fail");
        assert!(err.to_string().contains("not/here"), "got: {err}");
    }

    #[test]
    fn a_cores_boot_rom_and_blank_disks_are_not_games() {
        // Every X68000 card looks like this: five entries, three of which
        // are the core's own furniture.
        let dir = temp("boot-files");
        for name in [
            "boot.rom",
            "boot0.rom",
            "boot1.rom",
            "boot3.vhd",
            "blank.vhd",
            "BLANK_disk.d88",
        ] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        std::fs::write(dir.join("Bootleg.rom"), b"x").unwrap();
        std::fs::write(dir.join("Real Game.d88"), b"x").unwrap();

        let mut config = system(&dir);
        config.extensions = vec!["d88".to_string(), "rom".to_string(), "vhd".to_string()];

        let library = Library::open(&config).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(
            names_of(&rows),
            vec!["BLANK_disk.d88", "Bootleg.rom", "Real Game.d88"],
            "a game whose name merely starts with boot is still a game"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn dotfiles_are_not_content() {
        let dir = temp("dotfiles");
        std::fs::write(dir.join("._Ghost.d64"), b"x").unwrap();
        std::fs::write(dir.join("Real.d64"), b"x").unwrap();
        std::fs::create_dir_all(dir.join(".Trashes")).unwrap();
        std::fs::write(dir.join(".Trashes/x.d64"), b"x").unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Real.d64"]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn covers_are_found_when_the_games_sit_below_the_top() {
        let dir = temp("covers-below");
        std::fs::create_dir_all(dir.join("USA/media")).unwrap();
        std::fs::write(dir.join("USA/bd.d64"), b"x").unwrap();
        std::fs::write(dir.join("USA/media/bd.png"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>./USA/bd.d64</path>
               <image>./USA/media/bd.png</image></game></gameList>"#,
        )
        .unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        // Nothing at the top level carries a picture, so anything that
        // stops there reports none.
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert!(rows.iter().all(|row| row.cover.is_none()));

        assert_eq!(
            library.covers(8, 4),
            vec![(dir.join("USA/media/bd.png"), "bd.d64".to_string())]
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_system_holding_only_artwork_reports_no_covers() {
        // Plenty of cards carry a gamelist and a media folder for a system
        // whose games were never copied over. Looking must come back empty
        // rather than appearing to have not looked properly.
        // Its own directory: tests run at the same time, and the other
        // art-only test builds a different tree in the same place.
        let dir = temp("art-only-covers");
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/bd.png"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>./bd.d64</path>
               <image>./media/bd.png</image></game></gameList>"#,
        )
        .unwrap();

        let library = Library::open(&system(&dir)).unwrap();
        assert!(library.covers(8, 4).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn looking_for_covers_stops_at_the_folder_limit() {
        let dir = temp("covers-limit");
        for n in 0..6 {
            std::fs::create_dir_all(dir.join(format!("d{n}"))).unwrap();
            std::fs::write(dir.join(format!("d{n}/g.d64")), b"x").unwrap();
        }
        let library = Library::open(&system(&dir)).unwrap();
        // One listing: the top level only, which holds folders and no
        // pictures. It must return rather than descend.
        assert!(library.covers(1, 4).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn projected_audit_uses_the_effective_display_order() {
        let dir = temp("projected-audit-order");
        std::fs::write(dir.join("Alpha.d64"), b"a").unwrap();
        std::fs::write(dir.join("Zulu.d64"), b"z").unwrap();
        let library = Library::open_source_neutral(&system(&dir), DisplayNames::default()).unwrap();

        let audit = library.audit_projected(false, &mut |rows| {
            let row = rows.iter_mut().find(|row| row.name == "Zulu.d64").unwrap();
            row.name = "Aardvark from provider".to_string();
            row.sort_key = row.name.to_lowercase();
        });

        assert_eq!(
            audit.first_game.unwrap().name,
            "Aardvark from provider",
            "CLI report and dry-run must choose the first game shown by the effective source"
        );
        std::fs::remove_dir_all(&dir).ok();
    }
    #[test]
    fn multigame_zip_metadata_is_exact_and_same_basenames_remain_distinct() {
        let dir = temp("zip-metadata");
        let archive = dir.join("library.zip");
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&["one/Game.d64", "two/Game.d64", "empty/"], true),
        )
        .unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList>
            <game><path>library.zip</path><name>Archive title</name></game>
            <game><path>Game.d64</path><name>Wrong sibling</name></game>
            <game><path>./library.zip/one/Game.d64</path><name>Correct one</name></game>
            </gameList>"#,
        )
        .unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library
            .list(&Place::Archive(archive.clone()), true)
            .unwrap();
        assert_eq!(names_of(&rows), ["empty", "one", "two"]);
        let (hidden_empty, _) = library
            .list(&Place::Archive(archive.clone()), false)
            .unwrap();
        assert_eq!(names_of(&hidden_empty), ["one", "two"]);
        for (prefix, expected) in [("one", "Correct one"), ("two", "Game.d64")] {
            let place = Place::ArchiveDirectory {
                archive: archive.clone(),
                prefix: prefix.into(),
            };
            let (rows, _) = library.list(&place, false).unwrap();
            assert_eq!(names_of(&rows), [expected]);
            assert_eq!(
                rows[0].kind,
                Kind::Play(Launch::File(archive.join(format!("{prefix}/Game.d64"))))
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn one_supported_zip_member_preserves_legacy_archive_metadata() {
        let dir = temp("zip-single-metadata");
        let archive = dir.join("Only.zip");
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&["folder/Game.d64", "readme.txt"], false),
        )
        .unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList><game><path>Only.zip</path><name>Legacy name</name></game></gameList>"#,
        )
        .unwrap();
        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library
            .list(
                &Place::ArchiveDirectory {
                    archive,
                    prefix: "folder".into(),
                },
                false,
            )
            .unwrap();
        assert_eq!(names_of(&rows), ["Legacy name"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn existing_place_serialization_tags_do_not_move() {
        for (place, tag) in [
            (Place::Dir("x".into()), 0),
            (Place::Archive("x.zip".into()), 1),
            (
                Place::Listing {
                    install: "i".into(),
                    file: "f".into(),
                },
                2,
            ),
            (Place::Roots, 3),
        ] {
            let bytes = postcard::to_allocvec(&place).unwrap();
            assert_eq!(bytes[0], tag);
            assert_eq!(postcard::from_bytes::<Place>(&bytes).unwrap(), place);
        }
        let place = Place::ArchiveDirectory {
            archive: "x.zip".into(),
            prefix: "folder".into(),
        };
        assert_eq!(postcard::to_allocvec(&place).unwrap()[0], 4);
    }

    #[test]
    fn unstable_core_names_keep_build_suffixes_while_stable_names_stay_short() {
        let dir = temp("nightly-names");
        let mut config = system(&dir);
        config.preserve_rbf_stem = true;
        let library = Library::open(&config).unwrap();
        assert_eq!(
            library.display_name(Path::new("Core_20260907_build123.rbf")),
            "Core_20260907_build123"
        );
        config.preserve_rbf_stem = false;
        let library = Library::open(&config).unwrap();
        assert_eq!(
            library.display_name(Path::new("Core_20260907_build123.rbf")),
            "Core"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn long_multiline_description_with_double_escaped_quotes_survives_browse_and_cache() {
        let dir = temp("description-regression");
        std::fs::write(dir.join("Race.d64"), b"synthetic test content").unwrap();
        let opening = "A visiting pilot arrives at an island circuit and joins a friendly race across beaches and hills. The competitors unlock new routes, explore hidden tracks and collect useful items along the way.";
        let xml = format!("<gameList><game><path>./Race.d64</path><name>Island Race</name><genre>Racing</genre><desc>{opening}\n\nThe second paragraph describes the &amp;quot;Token Challenge&amp;quot;.\n\nAnother paragraph describes multiplayer races.</desc><favorite>true</favorite></game></gameList>");
        std::fs::write(dir.join("gamelist.xml"), xml).unwrap();
        let expected = format!(
            "{}...",
            opening.chars().take(160).collect::<String>().trim_end()
        );
        let library = Library::open(&system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(rows[0].details.desc, expected);
        assert_eq!(rows[0].genre.as_deref(), Some("Racing"));
        assert!(rows[0].favorite);
        let cache = crate::cache::build_system_controlled(
            &library,
            &std::sync::atomic::AtomicBool::new(false),
            &mut |_, _| {},
        )
        .unwrap()
        .unwrap();
        let encoded = postcard::to_allocvec(&cache).unwrap();
        let decoded: crate::cache::SystemCache = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(
            decoded.get(&library.start()).unwrap().rows[0].details.desc,
            expected
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// The shipped Neo Geo definition, pointed at a temporary folder.
    fn neogeo_system(path: &Path) -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Neo Geo".to_string(),
            path: path.to_string_lossy().into_owned(),
            extensions: vec!["neo".to_string(), "mgl".to_string()],
            rbf: "_Console/NeoGeo".to_string(),
            launch: vec![crate::config::LaunchRule {
                extensions: vec!["neo".to_string(), "mgl".to_string()],
                kind: "f".to_string(),
                index: 1,
                delay: 1,
                reset_delay: None,
                reset_hold: None,
                companion_extensions: Vec::new(),
                companion_index: None,
            }],
            skip_folders: Vec::new(),
            setname: None,
            extra_paths: Vec::new(),
        }
    }

    const ROMSETS: &str = r#"<romsets>
        <romset name="mslug" altname="Metal Slug"/>
        <romset name="kof98,kof98n" altname="The King of Fighters '98"/>
        <romset name="secret" altname="Never Shown" hide="1"/>
        <romset name="untitled"/>
    </romsets>"#;

    fn play_target(row: &Row) -> &Path {
        match &row.kind {
            Kind::Play(Launch::File(path)) => path,
            other => panic!("{} must be a playable file, got {other:?}", row.name),
        }
    }

    #[test]
    fn a_zipped_neo_geo_set_is_one_game_named_by_romsets_xml_and_is_never_opened() {
        // The bytes are not an archive at all. Listing it as a game under
        // the catalogue's title is only possible if the ZIP was never
        // opened, which is the point: Main takes the whole file, and a
        // library of hundreds of sets must not be read to be listed.
        let dir = temp("neogeo-zip");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::write(dir.join("MSLUG.zip"), b"not an archive").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Metal Slug"]);
        assert_eq!((stats.games, stats.folders), (1, 0));
        assert_eq!(play_target(&rows[0]), dir.join("MSLUG.zip"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unzipped_set_directory_is_one_game_and_its_rom_files_are_never_entered() {
        let dir = temp("neogeo-dir");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::create_dir_all(dir.join("kof98n")).unwrap();
        std::fs::write(dir.join("kof98n/prom"), b"p").unwrap();
        std::fs::write(dir.join("kof98n/crom0"), b"c").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        // A later alias of the entry, so Main prints the set name after
        // the title to tell it from the first.
        assert_eq!(names_of(&rows), vec!["The King of Fighters '98 (kof98n)"]);
        assert_eq!((stats.games, stats.folders), (1, 0));
        assert_eq!(play_target(&rows[0]), dir.join("kof98n"));
        // A folder holding only sets is a folder of games, not a dead end.
        assert!(!library.shows_nothing(&dir, Some(0), 0).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn organisational_folders_stay_navigable_and_sets_inside_them_are_games() {
        // Sets are filed under genre folders on real cards. The folder is
        // still a folder; what it holds are games, answered by the same
        // catalogue at the top of the system.
        let dir = temp("neogeo-organised");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::create_dir_all(dir.join("Run and Gun/mslug")).unwrap();
        std::fs::write(dir.join("Run and Gun/mslug/prom"), b"p").unwrap();
        std::fs::write(dir.join("Run and Gun/kof98.zip"), b"zip").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (top, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&top), vec!["Run and Gun"]);
        let Kind::Enter(Place::Dir(genre)) = &top[0].kind else {
            panic!("an organisational folder is entered, not played");
        };
        let (inside, stats) = library.list(&Place::Dir(genre.clone()), false).unwrap();
        assert_eq!(
            names_of(&inside),
            vec!["Metal Slug", "The King of Fighters '98"]
        );
        assert_eq!((stats.games, stats.folders), (2, 0));
        assert!(inside.iter().all(|row| !row.is_folder()));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_directory_carrying_its_own_romset_xml_is_a_game_titled_by_it() {
        let dir = temp("neogeo-romset-xml");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        // Not in the catalogue at all: the file alone makes it a game.
        std::fs::create_dir_all(dir.join("homebrew")).unwrap();
        std::fs::write(
            dir.join("homebrew/romset.xml"),
            r#"<romset name="homebrew" altname="Home Brew"/>"#,
        )
        .unwrap();
        std::fs::write(dir.join("homebrew/prom"), b"p").unwrap();
        // Hidden by the catalogue, but the folder's own file outranks it,
        // as it does in Main.
        std::fs::create_dir_all(dir.join("secret")).unwrap();
        std::fs::write(dir.join("secret/romset.xml"), r#"<romset name="secret"/>"#).unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Home Brew", "secret"]);
        assert_eq!((stats.games, stats.folders), (2, 0));
        assert_eq!(play_target(&rows[0]), dir.join("homebrew"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn standalone_neo_and_mgl_rows_keep_their_names_beside_rom_sets() {
        // What the release before this one showed for a .neo (its file
        // name, since the system lists two extensions) and for an .mgl
        // (its stem) must not move: existing favourites, saved positions
        // and gamelists are keyed by them.
        let dir = temp("neogeo-standalone");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::write(dir.join("Blazing Star.neo"), b"neo").unwrap();
        std::fs::write(dir.join("Shortcut.mgl"), b"<mistergamedescription/>").unwrap();
        std::fs::write(dir.join("mslug.zip"), b"zip").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(
            names_of(&rows),
            vec!["Blazing Star.neo", "Metal Slug", "Shortcut"]
        );
        assert_eq!(stats.games, 3);
        assert_eq!(play_target(&rows[0]), dir.join("Blazing Star.neo"));
        assert_eq!(play_target(&rows[2]), dir.join("Shortcut.mgl"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn unrelated_zips_and_directories_are_not_neo_geo_games() {
        let dir = temp("neogeo-unrelated");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        // An archive the catalogue does not know still opens like a folder.
        std::fs::write(dir.join("Neo Geo.zip"), crate::zip::tests_fixture()).unwrap();
        // A folder the catalogue does not know is a folder.
        std::fs::create_dir_all(dir.join("misc")).unwrap();
        std::fs::write(dir.join("misc/Other.neo"), b"neo").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["misc", "Neo Geo"]);
        assert_eq!((stats.games, stats.folders), (0, 2));
        assert_eq!(rows[0].kind, Kind::Enter(Place::Dir(dir.join("misc"))));
        assert_eq!(
            rows[1].kind,
            Kind::Enter(Place::Archive(dir.join("Neo Geo.zip")))
        );

        // A catalogue beside another system's games means nothing to it:
        // the ZIP is an archive to enter and the folder is a folder.
        std::fs::write(dir.join("mslug.zip"), crate::zip::tests_fixture()).unwrap();
        std::fs::create_dir_all(dir.join("kof98")).unwrap();
        std::fs::write(dir.join("kof98/game.d64"), b"x").unwrap();
        let c64 = Library::open(&system(&dir)).unwrap();
        let (rows, stats) = c64.list(&c64.start(), true).unwrap();
        assert_eq!(names_of(&rows), vec!["kof98", "misc", "mslug", "Neo Geo"]);
        assert_eq!(stats.games, 0);
        assert!(rows.iter().all(Row::is_folder));

        // Neo Geo CD runs the same core on discs and sees the same thing.
        let mut cd = neogeo_system(&dir);
        cd.extensions = vec!["cue".to_string(), "chd".to_string(), "mgl".to_string()];
        let cd = Library::open(&cd).unwrap();
        let (rows, _) = cd.list(&cd.start(), true).unwrap();
        assert!(
            rows.iter().all(Row::is_folder),
            "got: {:?}",
            names_of(&rows)
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_romsets_xml_is_reported_and_keeps_every_standalone_game() {
        // A broken catalogue must not take the .neo files down with it.
        // The sets fall back to what they are without one, an archive and
        // a folder, and the audit names the file so it can be fixed.
        let dir = temp("neogeo-broken-catalogue");
        std::fs::write(dir.join("romsets.xml"), "<romsets><romset name=\"mslug\"").unwrap();
        std::fs::write(dir.join("Blazing Star.neo"), b"neo").unwrap();
        std::fs::write(dir.join("mslug.zip"), crate::zip::tests_fixture()).unwrap();
        std::fs::create_dir_all(dir.join("kof98")).unwrap();
        std::fs::write(dir.join("kof98/Inside.neo"), b"neo").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["kof98", "mslug", "Blazing Star.neo"]);
        assert_eq!((stats.games, stats.folders), (1, 2));
        assert_eq!(
            rows[1].kind,
            Kind::Enter(Place::Archive(dir.join("mslug.zip")))
        );
        let audit = library.audit(false);
        assert_eq!(
            audit.games, 4,
            "the .neo files inside the fallbacks still count"
        );
        let problem = audit
            .unreadable
            .iter()
            .find(|(path, _)| path == &dir.join("romsets.xml"))
            .expect("the broken catalogue is reported");
        assert!(problem.1.contains("romsets.xml"), "got: {}", problem.1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_malformed_romset_xml_is_reported_and_the_catalogue_still_names_the_set() {
        let dir = temp("neogeo-broken-romset");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::create_dir_all(dir.join("mslug")).unwrap();
        std::fs::write(dir.join("mslug/romset.xml"), "<romset name=").unwrap();
        std::fs::write(dir.join("mslug/prom"), b"p").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&rows), vec!["Metal Slug"]);
        assert!(!rows[0].is_folder());
        let audit = library.audit(false);
        assert_eq!(audit.games, 1);
        assert!(
            audit
                .unreadable
                .iter()
                .any(|(path, reason)| path == &dir.join("mslug/romset.xml")
                    && reason.contains("romset.xml")),
            "got: {:?}",
            audit.unreadable
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_sub_folder_romsets_xml_replaces_the_root_catalogue_for_that_folder() {
        // Main reads the catalogue of the folder it is scanning when there
        // is one, and the system's otherwise. The core's own release notes
        // ship a sub-folder catalogue for a second collection this way.
        let dir = temp("neogeo-subcatalogue");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::create_dir_all(dir.join("gog")).unwrap();
        std::fs::write(
            dir.join("gog/romsets.xml"),
            r#"<romsets><romset name="mslug" altname="Metal Slug (GOG)"/></romsets>"#,
        )
        .unwrap();
        std::fs::write(dir.join("gog/mslug.zip"), b"zip").unwrap();
        std::fs::write(dir.join("gog/kof98.zip"), b"zip").unwrap();
        std::fs::write(dir.join("mslug.zip"), b"zip").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (top, _) = library.list(&library.start(), false).unwrap();
        assert_eq!(names_of(&top), vec!["gog", "Metal Slug"]);
        let (inside, stats) = library.list(&Place::Dir(dir.join("gog")), false).unwrap();
        // kof98 is only in the root catalogue, which does not apply here,
        // so it is the archive it would be anywhere else.
        assert_eq!(names_of(&inside), vec!["kof98", "Metal Slug (GOG)"]);
        assert_eq!((stats.games, stats.folders), (1, 1));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn hidden_sets_are_not_listed_and_a_folder_holding_only_hidden_sets_is_empty() {
        let dir = temp("neogeo-hidden");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::write(dir.join("secret.zip"), b"zip").unwrap();
        std::fs::create_dir_all(dir.join("Drafts/SECRET")).unwrap();
        std::fs::write(dir.join("Drafts/SECRET/prom"), b"p").unwrap();
        std::fs::write(dir.join("untitled.zip"), b"zip").unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        // The hidden ZIP is gone, the folder of nothing but a hidden set is
        // a dead end, and the entry with no title keeps its own name.
        assert_eq!(names_of(&rows), vec!["untitled"]);
        assert_eq!(stats.empty_folders_hidden, 1);
        // Asked for, the empty folder is there; the hidden set never is.
        let (all, _) = library.list(&library.start(), true).unwrap();
        assert_eq!(names_of(&all), vec!["Drafts", "untitled"]);
        let (drafts, _) = library.list(&Place::Dir(dir.join("Drafts")), true).unwrap();
        assert!(drafts.is_empty(), "got: {:?}", names_of(&drafts));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn gamelist_records_bind_to_zip_and_directory_sets() {
        // A gamelist names the file on the card, and a set is the ZIP or
        // the folder itself: its record binds like any other game's, name
        // and picture, and the gamelist name wins as it does everywhere.
        let dir = temp("neogeo-gamelist");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("mslug.zip"), b"zip").unwrap();
        std::fs::create_dir_all(dir.join("kof98")).unwrap();
        std::fs::write(dir.join("kof98/prom"), b"p").unwrap();
        std::fs::write(dir.join("media/mslug.png"), b"x").unwrap();
        std::fs::write(dir.join("media/kof98.png"), b"x").unwrap();
        std::fs::write(
            dir.join("gamelist.xml"),
            r#"<gameList>
            <game><path>./mslug.zip</path><name>Metal Slug (scraped)</name>
              <image>./media/mslug.png</image></game>
            <game><path>./kof98</path><genre>Fighting</genre>
              <image>./media/kof98.png</image></game>
            </gameList>"#,
        )
        .unwrap();

        let library = Library::open(&neogeo_system(&dir)).unwrap();
        let (rows, stats) = library.list(&library.start(), false).unwrap();
        assert_eq!(
            names_of(&rows),
            vec!["Metal Slug (scraped)", "The King of Fighters '98"]
        );
        assert_eq!(stats.with_art, 2);
        assert_eq!(rows[0].cover, Some(dir.join("media/mslug.png")));
        assert_eq!(rows[1].cover, Some(dir.join("media/kof98.png")));
        assert_eq!(rows[1].genre.as_deref(), Some("Fighting"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn neo_geo_and_neo_geo_mvs_list_the_same_shared_library_identically() {
        // Both shipped rows point at one folder. The catalogue is read the
        // same way for each, so neither can show a set the other calls a
        // folder, which is what would put two different lists in one cache
        // directory.
        let dir = temp("neogeo-shared");
        std::fs::write(dir.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::write(dir.join("mslug.zip"), b"zip").unwrap();
        std::fs::create_dir_all(dir.join("kof98")).unwrap();
        std::fs::write(dir.join("kof98/prom"), b"p").unwrap();
        std::fs::write(dir.join("Blazing Star.neo"), b"neo").unwrap();
        let table = crate::systems::parse_table(
            include_str!("../assets/systems.toml"),
            Path::new("systems.toml"),
        )
        .unwrap();
        let mut listings = Vec::new();
        for id in ["NeoGeo", "NeoGeoMVS"] {
            let def = table.iter().find(|system| system.id == id).unwrap().clone();
            let config = crate::systems::FoundSystem {
                def,
                paths: vec![dir.clone()],
                logo_dir: None,
                menu_folder: None,
            }
            .to_config();
            let library = Library::open(&config).unwrap();
            let (rows, stats) = library.list(&library.start(), false).unwrap();
            assert_eq!(
                names_of(&rows),
                vec!["Blazing Star.neo", "Metal Slug", "The King of Fighters '98"],
                "{id}"
            );
            assert_eq!((stats.games, stats.folders), (3, 0), "{id}");
            listings.push(rows);
        }
        assert_eq!(listings[0], listings[1]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_folder_without_its_own_catalogue_answers_to_the_first_declared_folders() {
        // A library split over the card and a USB stick or a network share
        // is declared as more than one folder. Main reads the romsets.xml
        // of the folder it is scanning when there is one, and otherwise
        // the one at the top of the core's home folder, which is a single
        // folder: the first its search order finds, and the first declared
        // here. So a second folder with its own catalogue answers to it,
        // and a second folder or sub-folder without one answers to the
        // first folder's, not to an empty list and not to its parent's.
        let first = temp("neogeo-root-a");
        let second = temp("neogeo-root-b");
        let third = temp("neogeo-root-c");
        std::fs::write(first.join("romsets.xml"), ROMSETS).unwrap();
        std::fs::write(
            second.join("romsets.xml"),
            r#"<romsets><romset name="lastblad" altname="The Last Blade"/></romsets>"#,
        )
        .unwrap();
        std::fs::write(first.join("mslug.zip"), b"zip").unwrap();
        std::fs::write(first.join("lastblad.zip"), crate::zip::tests_fixture()).unwrap();
        std::fs::write(second.join("lastblad.zip"), b"zip").unwrap();
        std::fs::write(second.join("mslug.zip"), crate::zip::tests_fixture()).unwrap();
        std::fs::create_dir_all(second.join("Fighting/lastblad")).unwrap();
        std::fs::write(second.join("Fighting/lastblad/Other.neo"), b"neo").unwrap();
        std::fs::create_dir_all(second.join("Fighting/kof98")).unwrap();
        std::fs::write(second.join("Fighting/kof98/prom"), b"p").unwrap();
        std::fs::write(third.join("mslug.zip"), b"zip").unwrap();
        std::fs::write(third.join("lastblad.zip"), crate::zip::tests_fixture()).unwrap();

        let mut config = neogeo_system(&first);
        config.extra_paths = vec![
            second.to_string_lossy().into_owned(),
            third.to_string_lossy().into_owned(),
        ];
        let library = Library::open(&config).unwrap();
        let (a, stats) = library.list(&Place::Dir(first.clone()), false).unwrap();
        assert_eq!(names_of(&a), vec!["lastblad", "Metal Slug"]);
        assert_eq!((stats.games, stats.folders), (1, 1));
        // The second folder carries its own catalogue and answers to it
        // alone: the first folder's entries do not reach into it.
        let (b, stats) = library.list(&Place::Dir(second.clone()), false).unwrap();
        assert_eq!(names_of(&b), vec!["Fighting", "mslug", "The Last Blade"]);
        assert_eq!((stats.games, stats.folders), (1, 2));
        // A sub-folder of the second folder has no catalogue of its own,
        // so it answers to the first declared folder's, as Main's home
        // folder lookup does, and not to the folder above it: lastblad is
        // an ordinary folder here, holding a .neo to stay listed.
        let (fighting, stats) = library
            .list(&Place::Dir(second.join("Fighting")), false)
            .unwrap();
        assert_eq!(
            names_of(&fighting),
            vec!["lastblad", "The King of Fighters '98"]
        );
        assert_eq!((stats.games, stats.folders), (1, 1));
        // A declared folder without a catalogue answers to the first
        // declared folder's too, rather than listing its sets as archives.
        let (c, stats) = library.list(&Place::Dir(third.clone()), false).unwrap();
        assert_eq!(names_of(&c), vec!["lastblad", "Metal Slug"]);
        assert_eq!((stats.games, stats.folders), (1, 1));
        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&second).ok();
        std::fs::remove_dir_all(&third).ok();
    }
}
