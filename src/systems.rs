//! The systems table, and which of them this card actually has.
//!
//! The table says which core runs a system and which of that core's slots a
//! file goes into. It is compiled from public community data and shipped as
//! `systems.toml`, which the user is free to edit: nothing regenerates it
//! behind their back.
//!
//! Discovery is deliberately cheap. Checking that a folder exists costs one
//! lookup; counting what is inside it costs a directory walk, and on this
//! hardware a walk of a large library takes seconds. With a hundred systems
//! that would be a minute of staring at nothing before the first frame, so
//! nothing is counted until a system is opened.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config::LaunchRule;
use crate::error::{DegaussError, Result};

/// One row of the systems table.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SystemDef {
    /// Human name, e.g. "Commodore 64".
    pub name: String,
    /// Short identifier used by MiSTer tooling, e.g. "C64".
    pub id: String,
    /// Folder names this system's games live in, relative to a games root.
    pub folders: Vec<String>,
    /// Core to load, in MiSTer's own form, e.g. `_Computer/C64`.
    pub rbf: String,
    /// Every launchable extension, lowercase and without the dot.
    pub extensions: Vec<String>,
    #[serde(default)]
    pub launch: Vec<LaunchRule>,
    /// Optional artwork for the system itself. Point this at a file on the
    /// card and it will be used; otherwise a logo of the system's own name
    /// from the `logos` folder is drawn, and failing that the name itself.
    #[serde(default)]
    pub logo: Option<String>,
    /// Which group this belongs to in the browser: Arcade, Computer,
    /// Console or Other, mirroring how MiSTer's own menu is organised.
    ///
    /// Left out of the table for most systems because the core's own path
    /// already says it: a core under `_Console` is a console. An explicit
    /// value wins when the guess would be wrong.
    #[serde(default)]
    pub category: Option<String>,
    /// Folder names to skip while scanning this system.
    ///
    /// Some collections ship the same games several times over: a card's
    /// _Arcade folder typically holds a few thousand real .mra files and
    /// twenty thousand symlinks to them, sorted by genre and manufacturer.
    /// Listing all of them would be slow and would show every game a dozen
    /// times.
    #[serde(default)]
    pub skip_folders: Vec<String>,
    /// Set name for a core that presents itself as several systems, e.g.
    /// the Atari 7800 core running Atari 2600 games.
    #[serde(default)]
    setname: Option<String>,
}

impl SystemDef {
    /// The group this system belongs to.
    pub fn category(&self) -> &str {
        if let Some(explicit) = self.category.as_deref() {
            return explicit;
        }
        // MiSTer's own layout: cores live under _Console, _Computer,
        // _Arcade, _Other or _Utility, and that is exactly the grouping the
        // main menu shows.
        match self.rbf.split('/').next().unwrap_or_default() {
            "_Console" => "Console",
            "_Computer" => "Computer",
            "_Arcade" => "Arcade",
            "_Utility" => "Utility",
            _ => "Other",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SystemsFile {
    systems: Vec<SystemDef>,
}

/// A system whose folder is present on this machine.
#[derive(Debug, Clone)]
pub struct FoundSystem {
    pub def: SystemDef,
    /// Every folder of this system's that exists, absolute.
    ///
    /// Systems really do keep their games in more than one place: a system's
    /// first listed folder can exist and hold nothing while its games sit in
    /// a later one, so every folder that exists is used, not just the first.
    pub paths: Vec<PathBuf>,
    /// Where to look for a logo named after this system.
    pub logo_dir: Option<PathBuf>,
    /// The menu folder this system's core actually sits in on this card,
    /// when it could be found. The table's own guess is the fallback.
    pub menu_folder: Option<String>,
}

impl FoundSystem {
    pub fn name(&self) -> &str {
        &self.def.name
    }

    /// Which group this system appears under.
    ///
    /// Where the core actually is on this card wins over what the table
    /// guessed. The stock menu is a listing of the `_`-prefixed folders at
    /// the top of the card, so a core in `_Console` is a console however it
    /// is described elsewhere; anything else puts systems in groups the
    /// user does not see in their own menu.
    pub fn category(&self) -> &str {
        self.menu_folder
            .as_deref()
            .unwrap_or_else(|| self.def.category())
    }

    /// The folder shown to the user, and where a scan starts.
    pub fn path(&self) -> &Path {
        self.paths
            .first()
            .map(|p| p.as_path())
            .unwrap_or(Path::new(""))
    }

    /// The system as the catalog and launcher want it: a folder, the
    /// extensions that count, and how to start each of them.
    pub fn to_config(&self) -> crate::config::SystemConfig {
        crate::config::SystemConfig {
            name: self.def.name.clone(),
            path: self.path().to_string_lossy().into_owned(),
            extra_paths: self
                .paths
                .iter()
                .skip(1)
                .map(|p| p.to_string_lossy().into_owned())
                .collect(),
            extensions: self.def.extensions.clone(),
            rbf: self.def.rbf.clone(),
            launch: self.def.launch.clone(),
            skip_folders: self.def.skip_folders.clone(),
            setname: self.def.setname.clone(),
        }
    }

    /// Artwork for the system.
    ///
    /// Either the table names a file, or one sits in the `logos` folder
    /// under whatever name the system has. Anything dropped into that
    /// folder by hand works the same way.
    pub fn logo(&self) -> Option<PathBuf> {
        if let Some(named) = self.def.logo.as_deref() {
            let path = PathBuf::from(named);
            return path.is_file().then_some(path);
        }
        let dir = self.logo_dir.as_ref()?;
        for extension in ["png", "jpg"] {
            let path = dir.join(format!("{}.{extension}", self.def.id));
            if path.is_file() {
                return Some(path);
            }
        }
        None
    }
}

/// Load the table. A malformed table is an error: a frontend that silently
/// lost half its systems would be blamed on the card.
pub fn load_table(path: &Path) -> Result<Vec<SystemDef>> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| DegaussError::io("reading systems table", path, e))?;
    parse_table(&text, path)
}

pub fn parse_table(text: &str, origin: &Path) -> Result<Vec<SystemDef>> {
    let file: SystemsFile = toml::from_str(text)
        .map_err(|e| DegaussError::malformed("systems table", origin, e.to_string()))?;
    if file.systems.is_empty() {
        return Err(DegaussError::malformed(
            "systems table",
            origin,
            "no [[systems]] entries",
        ));
    }
    Ok(file.systems)
}

/// Find the systems whose folders exist under any of the given roots.
///
/// Order follows the table, so the list reads the same on every machine.
/// A system with several possible folder names contributes once, using the
/// first that exists.
pub fn discover(
    table: &[SystemDef],
    roots: &[PathBuf],
    logo_dir: Option<&Path>,
    cores: &CoreIndex,
) -> Vec<FoundSystem> {
    let mut found = Vec::new();
    for def in table {
        let paths = existing_folders(def, roots);
        if paths.is_empty() {
            continue;
        }
        found.push(FoundSystem {
            // The core the table names, or failing that a core named after
            // the system itself. A table entry can name a core this card
            // does not have: several systems run on more than one core, and
            // the table can only name one of them.
            menu_folder: cores
                .folder_of(&def.rbf)
                .or_else(|| cores.folder_of(&def.id))
                .map(str::to_string),
            def: def.clone(),
            paths,
            logo_dir: logo_dir.map(|d| d.to_path_buf()),
        });
    }
    found
}

/// Which menu folder each installed core sits in.
///
/// MiSTer's main menu is a listing of the `_`-prefixed folders at the top of
/// the card, and a core belongs to whichever one holds its `.rbf`. Reading
/// that is the only way to group systems the way the user's own menu groups
/// them: a table can only guess, and guesses go wrong in ways that read as
/// nonsense, like a handheld filed under Arcade because some other core of
/// a similar name is an arcade board.
#[derive(Debug, Default)]
pub struct CoreIndex {
    /// Lowercased core name, without its date stamp, to the menu folder
    /// holding it and how deep inside that folder it sits.
    folders: BTreeMap<String, (String, usize)>,
}

impl CoreIndex {
    /// Read every core on the card. One walk of the menu folders, which
    /// hold a few hundred files between them.
    pub fn read(root: &Path) -> Self {
        let mut index = CoreIndex::default();
        let Ok(listing) = std::fs::read_dir(root) else {
            return index;
        };
        for item in listing.flatten() {
            let name = item.file_name().to_string_lossy().into_owned();
            if !name.starts_with('_') || !item.path().is_dir() {
                continue;
            }
            // What the stock menu prints: the folder without its marker.
            let label = name.trim_start_matches('_').to_string();
            index.walk(&item.path(), &label, 0);
        }
        index
    }

    fn walk(&mut self, dir: &Path, label: &str, depth: usize) {
        // Cores sit at the top of a menu folder, or one level in, in the
        // `*_extra` folders the community ships. Nothing deeper is a core
        // the menu offers, and going further means reading every genre
        // folder of an organised arcade collection to learn nothing.
        if depth > 1 {
            return;
        }
        let Ok(listing) = std::fs::read_dir(dir) else {
            return;
        };
        for item in listing.flatten() {
            let path = item.path();
            if path.is_dir() {
                self.walk(&path, label, depth + 1);
                continue;
            }
            let name = item.file_name().to_string_lossy().into_owned();
            let Some(stem) = name
                .strip_suffix(".rbf")
                .or_else(|| name.strip_suffix(".RBF"))
            else {
                continue;
            };
            // The shallowest copy of a core wins. This is not a tie-break:
            // `_Arcade/cores` holds a copy of several console cores, because
            // that is where an `.mra` loads its core from. Those are support
            // files, never shown in the menu, and letting one of them answer
            // puts the Master System under Arcade.
            let key = core_name(stem);
            let better = match self.folders.get(&key) {
                Some((_, seen)) => depth < *seen,
                None => true,
            };
            if better {
                self.folders.insert(key, (label.to_string(), depth));
            }
        }
    }

    /// The menu folder holding the core named by an `rbf` field, which is
    /// written in MiSTer's own form, e.g. `_Console/NeoGeo`.
    pub fn folder_of(&self, rbf: &str) -> Option<&str> {
        let core = rbf.rsplit('/').next().unwrap_or(rbf);
        if core.is_empty() {
            return None;
        }
        self.folders
            .get(&core_name(core))
            .map(|(folder, _)| folder.as_str())
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn len(&self) -> usize {
        self.folders.len()
    }
}

/// True when the core an MGL naming `rbf` would load is really on the card.
///
/// `rbf` is MiSTer's own reference, e.g. `_Console/NeoGeo`, and MiSTer
/// resolves it against the top of the card: that folder, that name, with
/// the dated builds updaters install answering for the plain name. The
/// index the menu grouping keeps is not usable for this: it matches a core
/// name anywhere at the top of the card, so a support copy under
/// `_Arcade/cores` or a favourite's dangling link would answer for a core
/// whose real file is gone. This looks only where MiSTer will look, and
/// compares through the same `core_name` the walk uses, so a dated file
/// counts and a different core that merely shares a prefix does not.
pub fn core_file_exists(menu_root: &Path, rbf: &str) -> bool {
    let (folder, core) = match rbf.rsplit_once('/') {
        Some((folder, core)) => (Some(folder), core),
        None => (None, rbf),
    };
    if core.is_empty() {
        return false;
    }
    let dir = match folder {
        Some(folder) => menu_root.join(folder),
        None => menu_root.to_path_buf(),
    };
    let Ok(listing) = std::fs::read_dir(dir) else {
        return false;
    };
    let wanted = core_name(core);
    for item in listing.flatten() {
        // A directory named like a core would pass the name checks, and
        // MiSTer cannot load a directory.
        if !item.path().is_file() {
            continue;
        }
        let name = item.file_name().to_string_lossy().into_owned();
        let Some(stem) = name
            .strip_suffix(".rbf")
            .or_else(|| name.strip_suffix(".RBF"))
        else {
            continue;
        };
        if core_name(stem).eq_ignore_ascii_case(&wanted) {
            return true;
        }
    }
    false
}

/// A core's name reduced to what identifies it: no date stamp, no
/// punctuation, no case.
///
/// `NeoGeo_20260603` and `NeoGeo` are the same core, since a card holds
/// whichever build was installed last. Punctuation goes because the same
/// system is written `NeoGeoPocket-Color` as a file and `NeoGeoPocketColor`
/// as an id, and they have to meet.
fn core_name(stem: &str) -> String {
    let trimmed = match stem.rsplit_once('_') {
        // Compared as bytes, not sliced as a string. A date stamp is eight
        // ASCII digits either way, and `&after[..8]` panics when byte 8 lands
        // inside a multi-byte character, which a core named in anything but
        // ASCII would do.
        Some((before, after))
            if after.len() >= 8 && after.as_bytes()[..8].iter().all(u8::is_ascii_digit) =>
        {
            before
        }
        _ => stem,
    };
    trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// Every folder of this system's that exists, in the order the table lists
/// them, without duplicates.
///
/// All of them, not just the first: a system's games are routinely spread
/// across the folders it names, and stopping at the first one loses whatever
/// is in the rest.
fn existing_folders(def: &SystemDef, roots: &[PathBuf]) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    for folder in &def.folders {
        for root in roots {
            // An absolute folder in the table stands on its own; joining
            // replaces the root, which is what we want.
            let candidate = root.join(folder);
            if candidate.is_dir() && !found.contains(&candidate) {
                found.push(candidate);
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    /// A core file named in anything but ASCII used to take the whole
    /// frontend down before it drew a frame: the date-stamp test sliced the
    /// string at byte eight, and the card is read at startup.
    #[test]
    fn a_core_named_in_a_non_ascii_script_does_not_crash_the_index() {
        assert_eq!(core_name("Core_\u{65e5}\u{672c}\u{8a9e}\u{30c6}"), "core");
        assert_eq!(core_name("NeoGeo_20260603"), "neogeo");
        assert_eq!(core_name("NeoGeo"), "neogeo");
        // Eight bytes but not eight digits: the stamp is not stripped.
        assert_eq!(core_name("Core_abcdefgh"), "coreabcdefgh");
    }

    use super::*;

    const TABLE: &str = r#"
[[systems]]
name = "Commodore 64"
id = "C64"
folders = ["C64"]
rbf = "_Computer/C64"
extensions = ["d64", "prg"]

[[systems.launch]]
extensions = ["d64"]
type = "s"
index = 0
delay = 1

[[systems]]
name = "Genesis"
id = "Genesis"
folders = ["MegaDrive", "Genesis"]
rbf = "_Console/MegaDrive"
extensions = ["md", "bin"]
"#;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("degauss-systems-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn a_dated_core_build_answers_for_the_plain_name() {
        // Updaters install cores as NeoGeo_20260101.rbf, and MiSTer loads
        // that file for an MGL saying _Console/NeoGeo. Presence has to be
        // judged the way MiSTer resolves, or every updated card looks like
        // it has no cores at all.
        let root = temp_dir("core-dated");
        std::fs::create_dir_all(root.join("_Console")).unwrap();
        std::fs::write(root.join("_Console/NeoGeo_20260101.rbf"), b"x").unwrap();
        assert!(core_file_exists(&root, "_Console/NeoGeo"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_core_that_merely_shares_a_prefix_does_not_answer() {
        // NeoGeoPocket.rbf sitting beside a missing NeoGeo.rbf must not
        // make Neo Geo launchable: a bare prefix match would say it is.
        let root = temp_dir("core-prefix");
        std::fs::create_dir_all(root.join("_Console")).unwrap();
        std::fs::write(root.join("_Console/NeoGeoPocket.rbf"), b"x").unwrap();
        assert!(!core_file_exists(&root, "_Console/NeoGeo"));
        assert!(core_file_exists(&root, "_Console/NeoGeoPocket"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_copy_of_the_core_somewhere_else_does_not_answer() {
        // _Arcade/cores holds support copies of console cores for .mra
        // loading. MiSTer resolves an MGL's rbf against the named folder
        // only, so a copy elsewhere must not make the launch look safe.
        let root = temp_dir("core-elsewhere");
        std::fs::create_dir_all(root.join("_Arcade/cores")).unwrap();
        std::fs::write(root.join("_Arcade/cores/NES.rbf"), b"x").unwrap();
        assert!(!core_file_exists(&root, "_Console/NES"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_directory_named_like_a_core_does_not_answer() {
        // MiSTer cannot load a directory, however it is named. Only a real
        // file counts as the core being present.
        let root = temp_dir("core-dir");
        std::fs::create_dir_all(root.join("_Console/NeoGeo.rbf")).unwrap();
        assert!(!core_file_exists(&root, "_Console/NeoGeo"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_missing_folder_means_the_core_is_missing_not_a_crash() {
        let root = temp_dir("core-nofolder");
        assert!(!core_file_exists(&root, "_Console/NeoGeo"));
        assert!(!core_file_exists(&root, ""));
        assert!(!core_file_exists(&root, "_Console/"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_shipped_table_parses_and_covers_the_systems_people_have() {
        // The real file, not a fixture: a table that stopped parsing would
        // leave the browser empty.
        let path = Path::new("assets/systems.toml");
        let table = load_table(path).expect("the shipped systems table parses");
        assert!(
            table.len() > 90,
            "expected the full table, got {}",
            table.len()
        );

        let c64 = table
            .iter()
            .find(|s| s.id == "C64")
            .expect("C64 is in the table");
        assert_eq!(c64.rbf, "_Computer/C64");
        assert!(c64.extensions.iter().any(|e| e == "prg"));
        assert!(
            c64.launch.iter().any(|r| r.kind == "s" && r.index == 0),
            "the disk slot must survive generation"
        );
        // Every system must be startable: either it says which slot a file
        // goes into, or its files say it themselves (.mra, .mgl and .rbf
        // name their own core).
        const SELF_DESCRIBING: [&str; 3] = ["mra", "mgl", "rbf"];
        for system in &table {
            let self_describing = system
                .extensions
                .iter()
                .all(|e| SELF_DESCRIBING.contains(&e.as_str()));
            assert!(
                !system.launch.is_empty() || self_describing,
                "{} has no launch rule and no self-describing files, so nothing could start",
                system.name
            );
        }
    }

    #[test]
    fn only_systems_with_a_folder_on_this_card_are_offered() {
        let root = temp_dir("discover");
        std::fs::create_dir_all(root.join("C64")).unwrap();

        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let found = discover(
            &table,
            std::slice::from_ref(&root),
            None,
            &CoreIndex::default(),
        );

        assert_eq!(found.len(), 1, "Genesis has no folder here");
        assert_eq!(found[0].name(), "Commodore 64");
        assert_eq!(found[0].path(), root.join("C64"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn every_folder_a_system_names_is_used_not_just_the_first() {
        // A system's first listed folder can exist and hold nothing while its
        // games sit in a later one; stopping at the first would lose them all.
        let root = temp_dir("multi");
        std::fs::create_dir_all(root.join("MegaDrive")).unwrap();
        std::fs::create_dir_all(root.join("Genesis")).unwrap();

        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let found = discover(
            &table,
            std::slice::from_ref(&root),
            None,
            &CoreIndex::default(),
        );

        let genesis = found.iter().find(|s| s.def.id == "Genesis").expect("found");
        assert_eq!(genesis.paths.len(), 2, "both folders must be scanned");
        assert_eq!(genesis.path(), root.join("MegaDrive"), "the first is shown");
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_system_with_several_folder_names_uses_the_one_that_exists() {
        let root = temp_dir("alias");
        std::fs::create_dir_all(root.join("Genesis")).unwrap();

        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let found = discover(
            &table,
            std::slice::from_ref(&root),
            None,
            &CoreIndex::default(),
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path(), root.join("Genesis"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn several_roots_are_searched_in_order() {
        let first = temp_dir("root-a");
        let second = temp_dir("root-b");
        std::fs::create_dir_all(second.join("C64")).unwrap();

        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let found = discover(
            &table,
            &[first.clone(), second.clone()],
            None,
            &CoreIndex::default(),
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path(), second.join("C64"));
        std::fs::remove_dir_all(&first).ok();
        std::fs::remove_dir_all(&second).ok();
    }

    #[test]
    fn a_logo_is_found_by_the_system_id_without_any_configuration() {
        // Dropping C64.png into the logos folder should be all it takes.
        let dir = temp_dir("logos");
        let logos = dir.join("logos");
        std::fs::create_dir_all(&logos).unwrap();
        std::fs::write(logos.join("C64.png"), b"art").unwrap();
        std::fs::create_dir_all(dir.join("C64")).unwrap();

        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let found = discover(
            &table,
            std::slice::from_ref(&dir),
            Some(&logos),
            &CoreIndex::default(),
        );

        assert_eq!(found.len(), 1);
        assert_eq!(found[0].logo(), Some(logos.join("C64.png")));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn the_category_comes_from_the_core_path_unless_it_is_stated() {
        // MiSTer keeps cores under _Console, _Computer and so on, and that
        // is the grouping its own menu shows, so it needs no second table.
        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let c64 = &table[0];
        assert_eq!(c64.category(), "Computer", "_Computer/C64");
        let genesis = &table[1];
        assert_eq!(genesis.category(), "Console", "_Console/MegaDrive");

        let stated = SystemDef {
            category: Some("Arcade".into()),
            ..c64.clone()
        };
        assert_eq!(stated.category(), "Arcade", "an explicit value wins");
    }

    #[test]
    fn a_malformed_table_is_an_error_rather_than_an_empty_browser() {
        assert!(parse_table("systems = []", Path::new("t.toml")).is_err());
        assert!(parse_table("nonsense", Path::new("t.toml")).is_err());
    }

    #[test]
    fn a_logo_is_only_offered_when_the_file_is_really_there() {
        let table = parse_table(TABLE, Path::new("systems.toml")).unwrap();
        let system = FoundSystem {
            menu_folder: None,
            def: SystemDef {
                logo: Some("/definitely/not/here.png".into()),
                ..table[0].clone()
            },
            paths: vec![PathBuf::from("/games/C64")],
            logo_dir: None,
        };
        assert!(
            system.logo().is_none(),
            "a missing logo must fall back to the name, not draw a broken image"
        );
    }

    #[test]
    fn a_core_is_grouped_by_the_menu_folder_it_actually_sits_in() {
        // The table guessed Arcade for this handheld because a core of a
        // similar name is an arcade board. The card says otherwise, and the
        // card is what the user's own menu shows.
        let card = temp_dir("cores");
        std::fs::create_dir_all(card.join("_Console")).unwrap();
        std::fs::create_dir_all(card.join("_Computer")).unwrap();
        std::fs::write(card.join("_Console/NeoGeoPocket.rbf"), b"core").unwrap();
        std::fs::write(card.join("_Computer/ao486_20260603.rbf"), b"core").unwrap();

        let cores = CoreIndex::read(&card);
        assert_eq!(cores.folder_of("_Arcade/NeoGeoPocket"), Some("Console"));
        // A date stamp is part of every release and not part of the name.
        assert_eq!(cores.folder_of("_Computer/ao486"), Some("Computer"));
        assert_eq!(cores.folder_of("_Console/NotInstalled"), None);
        assert_eq!(cores.folder_of(""), None);
        std::fs::remove_dir_all(&card).ok();
    }

    #[test]
    fn a_core_is_found_however_its_name_is_punctuated() {
        // The card writes it `NeoGeoPocket-Color.rbf`; the table calls the
        // system `NeoGeoPocketColor`. Same core.
        let card = temp_dir("cores-punct");
        std::fs::create_dir_all(card.join("_Console")).unwrap();
        std::fs::write(card.join("_Console/NeoGeoPocket-Color.rbf"), b"core").unwrap();

        let cores = CoreIndex::read(&card);
        assert_eq!(cores.folder_of("NeoGeoPocketColor"), Some("Console"));
        std::fs::remove_dir_all(&card).ok();
    }

    #[test]
    fn a_system_whose_named_core_is_not_installed_falls_back_to_its_own_name() {
        // The table names Jotego's arcade board for the Neo Geo Pocket. A
        // card that does not have it, but does have the console core, must
        // still show the system where its menu shows it.
        let card = temp_dir("cores-fallback");
        std::fs::create_dir_all(card.join("_Console")).unwrap();
        std::fs::write(card.join("_Console/NeoGeoPocket.rbf"), b"core").unwrap();
        let root = temp_dir("cores-fallback-games");
        std::fs::create_dir_all(root.join("NGP")).unwrap();

        let table = parse_table(
            r#"
[[systems]]
name = "Neo Geo Pocket"
id = "NeoGeoPocket"
folders = ["NGP"]
rbf = "_Arcade/JTNGP"
extensions = ["ngp"]
"#,
            Path::new("test"),
        )
        .unwrap();

        let found = discover(
            &table,
            std::slice::from_ref(&root),
            None,
            &CoreIndex::read(&card),
        );
        assert_eq!(found[0].category(), "Console");
        std::fs::remove_dir_all(&card).ok();
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_support_copy_of_a_core_does_not_decide_the_group() {
        // `_Arcade/cores` holds a copy of the Master System core, because
        // that is where an .mra loads its core from. It is never shown in
        // the menu, and letting it answer files a console under Arcade.
        let card = temp_dir("cores-support");
        std::fs::create_dir_all(card.join("_Arcade/cores")).unwrap();
        std::fs::create_dir_all(card.join("_Console")).unwrap();
        std::fs::write(card.join("_Arcade/cores/SMS_20260603.rbf"), b"core").unwrap();
        std::fs::write(card.join("_Console/SMS_20260603.rbf"), b"core").unwrap();

        let cores = CoreIndex::read(&card);
        assert_eq!(cores.folder_of("_Console/SMS"), Some("Console"));
        std::fs::remove_dir_all(&card).ok();
    }

    #[test]
    fn cores_filed_one_level_into_a_menu_folder_still_count() {
        // Community core packs ship in `<menu>/<something>_extra`.
        let card = temp_dir("cores-nested");
        std::fs::create_dir_all(card.join("_Console/console_extra")).unwrap();
        std::fs::write(
            card.join("_Console/console_extra/NeoGeo_Turbo_20250814.rbf"),
            b"core",
        )
        .unwrap();

        let cores = CoreIndex::read(&card);
        assert_eq!(cores.folder_of("_Console/NeoGeo_Turbo"), Some("Console"));
        std::fs::remove_dir_all(&card).ok();
    }

    #[test]
    fn a_folder_that_is_not_a_menu_folder_is_not_read() {
        let card = temp_dir("not-menu");
        std::fs::create_dir_all(card.join("games")).unwrap();
        std::fs::write(card.join("games/Stray.rbf"), b"core").unwrap();

        assert_eq!(CoreIndex::read(&card).len(), 0);
        std::fs::remove_dir_all(&card).ok();
    }

    #[test]
    fn a_card_with_no_menu_folders_leaves_the_table_in_charge() {
        // Running on a desktop, where none of this exists.
        let cores = CoreIndex::read(Path::new("/definitely/not/a/card"));
        assert_eq!(cores.len(), 0);
        assert_eq!(cores.folder_of("_Console/NeoGeo"), None);
    }
}
