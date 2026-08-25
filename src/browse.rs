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
}

impl Place {
    pub fn path(&self) -> &Path {
        match self {
            Place::Dir(path) | Place::Archive(path) => path,
            Place::Listing { file, .. } => file,
            Place::Roots => Path::new(""),
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

    fn contains(&self, path: &Path) -> bool {
        self.names.contains(&key(path))
    }

    /// True when this directory is one the metadata points art at, so
    /// browsing leaves it alone: it is not a folder of games, and on a real
    /// card it holds tens of thousands of pictures.
    fn is_art_directory(&self, path: &Path) -> bool {
        self.directories.contains(&key(path))
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

/// One system's folders, ready to browse.
pub struct Library {
    config: SystemConfig,
    roots: Vec<Root>,
    names: DisplayNames,
    /// What reading this system's metadata cost, so the answer to "why did
    /// that take a moment" is measured rather than guessed.
    pub cost: OpenCost,
}

/// What opening a system cost, in milliseconds and files.
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenCost {
    pub gamelist_ms: u128,
    pub art_ms: u128,
    pub art_files: usize,
}

/// Whether a listed entry is a directory, following a symlink to find out.
///
/// `DirEntry::file_type` deliberately does not follow links, so a folder
/// reached through one reads as an ordinary file and its games disappear.
/// Collections built out of linked trees are common enough on a card that
/// the extra call is worth making, and it is only made for links.
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
        let mut roots = Vec::new();
        let mut cost = OpenCost::default();
        for path in std::iter::once(config.path.clone()).chain(config.extra_paths.iter().cloned()) {
            let path = PathBuf::from(path);
            let gamelist_path = path.join("gamelist.xml");
            let started = std::time::Instant::now();
            let gamelist = if gamelist_path.is_file() {
                Some(Gamelist::load(&gamelist_path, &path)?)
            } else {
                None
            };
            cost.gamelist_ms += started.elapsed().as_millis();
            let art_dirs = gamelist
                .as_ref()
                .map(|list| list.art_directories())
                .unwrap_or_default();
            let started = std::time::Instant::now();
            let art = ArtIndex::read(&art_dirs);
            cost.art_ms += started.elapsed().as_millis();
            cost.art_files += art.files;
            roots.push(Root {
                path,
                gamelist,
                art,
            });
        }
        Ok(Library {
            config: config.clone(),
            roots,
            names,
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
            Place::Roots => self.list_roots(show_empty),
            Place::Dir(dir) => self.list_dir(dir, show_empty)?,
            Place::Archive(archive) => self.list_archive(archive)?,
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
    fn list_roots(&self, show_empty: bool) -> (Vec<Row>, ListStats) {
        let mut rows = Vec::new();
        let mut stats = ListStats::default();
        for (index, root) in self.roots.iter().enumerate() {
            if !show_empty && self.shows_nothing(&root.path, Some(index), 0) {
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
        (rows, stats)
    }

    fn list_dir(&self, dir: &Path, show_empty: bool) -> Result<(Vec<Row>, ListStats)> {
        let listing =
            std::fs::read_dir(dir).map_err(|e| DegaussError::io("reading folder", dir, e))?;
        let root = self.root_for(dir);
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

        // An entry the filesystem refuses is a game that silently is not
        // there. Dropping it is still the only answer, because one bad entry
        // must not cost the whole folder, but it is counted and said out
        // loud: a folder that is quietly short is the harder fault to find.
        let mut unreadable = 0usize;
        let entries: Vec<std::fs::DirEntry> = listing
            .filter_map(|item| match item {
                Ok(entry) => Some(entry),
                Err(_) => {
                    unreadable += 1;
                    None
                }
            })
            .collect();
        if unreadable > 0 {
            crate::note(&format!(
                "folder       {}: {unreadable} entries could not be read and are missing",
                dir.display()
            ));
        }
        for item in entries {
            let name = item.file_name().to_string_lossy().into_owned();
            // Dotfiles are not content, and a card that has met a Mac is
            // full of `._` companions that are not games either.
            if name.starts_with('.') {
                continue;
            }
            // Unreadable entries are skipped rather than guessed at.
            if item.file_type().is_err() {
                continue;
            }
            let path = item.path();

            if entry_is_dir(&item) {
                // Windows leaves this on every card it touches, and the
                // stock menu does not show it either.
                if name == "System Volume Information" {
                    continue;
                }
                // Surfaced above as the titles they name.
                if listings_here.as_deref() == Some(path.as_path()) {
                    continue;
                }
                if self.is_art_directory(&path, root) || self.is_skipped(&name) {
                    continue;
                }
                // A folder with nothing to reach inside it is a dead end,
                // and a card accumulates them.
                if !show_empty && self.shows_nothing(&path, root, 0) {
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
    fn list_archive(&self, archive: &Path) -> Result<(Vec<Row>, ListStats)> {
        let names = crate::zip::list(archive)?;
        let root = self.root_for(archive);
        let mut rows = Vec::new();
        let mut stats = ListStats::default();

        for name in names {
            let inner = archive.join(&name);
            if !self.config.accepts(&inner) {
                continue;
            }
            stats.games += 1;
            let row = self.game_row(&inner, root);
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
        let name = self.display_name(path);
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
        self.bind(&mut row, &rel, root);
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
        let Some(meta) = root
            .and_then(|index| self.roots[index].gamelist.as_ref())
            .and_then(|list| list.lookup(key))
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
        let mut found = Vec::new();
        let mut queue = std::collections::VecDeque::from([self.start()]);
        let mut looked = 0;

        while let Some(place) = queue.pop_front() {
            if looked >= folders || found.len() >= want {
                break;
            }
            looked += 1;
            let Ok((rows, _)) = self.list(&place, false) else {
                continue;
            };
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
    fn shows_nothing(&self, dir: &Path, root: Option<usize>, depth: usize) -> bool {
        if depth > MAX_DEPTH {
            // Too deep to keep asking. Say it shows something, so the worst
            // a symlink loop can do is leave one folder visible.
            return false;
        }
        // Asked once for the folder, not once per file in it: it is a
        // property of the folder, and answering it rescans the parent.
        let amiga_install = amiga_install_of(dir).is_some();
        let Ok(listing) = std::fs::read_dir(dir) else {
            // A folder that cannot be read is not known to be empty, and
            // hiding it would hide the problem with it.
            return false;
        };
        for item in listing.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "System Volume Information" {
                continue;
            }
            let path = item.path();
            if entry_is_dir(&item) {
                if self.is_art_directory(&path, root)
                    || self.is_skipped(&name)
                    || self.shows_nothing(&path, root, depth + 1)
                {
                    continue;
                }
                return false;
            }
            // A file only counts if it is one this system can open, an
            // archive that opens like a folder, or a listing naming titles
            // held inside a disk image.
            let extension = extension_of(&path);
            if extension == "zip" || (self.config.accepts(&path) && !is_not_a_game(&name)) {
                return false;
            }
            if extension == "txt" && amiga_install {
                return false;
            }
        }
        true
    }

    fn is_art_directory(&self, path: &Path, root: Option<usize>) -> bool {
        root.is_some_and(|index| self.roots[index].art.is_art_directory(path))
    }

    fn is_skipped(&self, name: &str) -> bool {
        self.config
            .skip_folders
            .iter()
            .any(|skip| skip.eq_ignore_ascii_case(name))
    }
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
        let mut audit = Audit::default();
        let mut stack: Vec<(Place, usize)> = self
            .roots
            .iter()
            .map(|root| (Place::Dir(root.path.clone()), 0))
            .collect();
        let mut seen: HashSet<String> = HashSet::new();

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
            if !seen.insert(key(place.path())) {
                continue;
            }
            audit.places_read += 1;
            audit.deepest = audit.deepest.max(depth);

            match self.list(&place, show_empty) {
                Ok((rows, _)) => {
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
        audit
    }

    /// Whether each declared folder carries a metadata overlay, for the
    /// audit: a system with no artwork and no gamelist is explained, one
    /// with a gamelist and no artwork is a problem.
    pub fn gamelists(&self) -> Vec<(PathBuf, bool)> {
        self.roots
            .iter()
            .map(|root| (root.path.clone(), root.gamelist.is_some()))
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
        assert_eq!(names_of(&inside), vec!["Another Game", "Metal Slug"]);
        assert_eq!(stats.games, 2);
        // readme.txt is in the archive but no core loads it.
        assert!(!inside.iter().any(|row| row.name.contains("readme")));

        let Kind::Play(Launch::File(path)) = &inside[1].kind else {
            panic!("must be launchable");
        };
        assert_eq!(path, &archive.join("Metal Slug.neo"));
        std::fs::remove_dir_all(&dir).ok();
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
}
