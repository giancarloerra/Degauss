//! Where the paths inside an MGL point, read the way MiSTer Main reads them.
//!
//! Main uses an absolute `<file path>` as written and joins every other
//! one, `./x` and `../x` included, to the core's home directory: it never
//! looks beside the MGL. The home directory is `games/<setname>` when the
//! descriptor carries a `<setname>` without `same_dir="1"`, and the core's
//! own `games/<core>` folder otherwise, whichever storage root holds it
//! first in Main's order (USB sticks, network, CIFS, card). Degauss stands
//! its recognised systems in for those folders: a system whose core (and
//! set name) the descriptor names answers with the folders it was found
//! in, and a name the systems table does not carry is looked for over the
//! configured game roots.
//!
//! This is the one place Degauss interprets those paths. Everything that
//! reads an MGL rather than handing it to Main unchanged (favourites, the
//! Artwork Pack match, reports) asks [`Homes::resolve`]; a direct launch of
//! an existing MGL does not come through here.
//!
//! The answer names a descriptor class, and the class decides the identity:
//!
//! - [`Class::Game`]: the core is a system Degauss knows, or the descriptor
//!   names no core at all. The LAST `<file>` is the game (a companion disc
//!   is written ahead of it), and nothing is stat'd for a single home.
//! - [`Class::CoreSet`]: the descriptor names a core outside the systems
//!   table, an arcade core with its ROM set spelled out as several files
//!   being the usual case. The descriptor itself is the identity (its file
//!   stem, plus its `<setname>`), every component has to exist under the
//!   home, and none of them is ever hashed.
//!
//! A component present in more than one of a system's folders is reported
//! as [`Component::Ambiguous`], never picked by folder order.

use std::path::{Path, PathBuf};

use crate::config::SystemConfig;
use crate::error::{DegaussError, Result};
use crate::favorites::{descriptor_reference, normalize_path, DescriptorReference};
use crate::systems::FoundSystem;

/// The game roots in Main's search order and the recognised systems, which
/// together say where a descriptor's non-absolute components live.
#[derive(Debug)]
pub struct Homes {
    roots: Vec<PathBuf>,
    /// Every system with a core, favourites excluded: the core and set
    /// name the descriptor has to match, and the folders that answer.
    cores: Vec<SystemConfig>,
}

/// What one `<file path>` came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Component {
    /// Written absolute: used as written, never checked.
    Absolute(PathBuf),
    /// The `../..` run then `media/...` spelling Degauss and the launch
    /// scripts write for an absolute card path; Main gets the same file by
    /// climbing out of any home directory.
    RootRelative(PathBuf),
    /// Under exactly one home directory.
    Home(PathBuf),
    /// Under no home directory that exists; `tried` is where it was
    /// looked for, empty when no home could be named at all.
    Missing { raw: String, tried: Vec<PathBuf> },
    /// Under several of the system's folders at once, each holding a file
    /// of that name. Main would load one of them by its own folder order,
    /// and this program cannot tell which.
    Ambiguous { raw: String, existing: Vec<PathBuf> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    Game,
    CoreSet,
}

/// Why a descriptor's paths could not be placed: the error's own text,
/// and whether files were found at all. A favourite is kept either way;
/// the kind decides whether it may still be handed to Main, which loads
/// what it finds by its own folder order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Diagnostic {
    /// No file where a component was looked for, or no home to look in.
    Unplaced(String),
    /// Files of one component's name in several of the system's folders.
    Ambiguous(String),
}

impl std::fmt::Display for Diagnostic {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Diagnostic::Unplaced(text) | Diagnostic::Ambiguous(text) => f.write_str(text),
        }
    }
}

/// One descriptor, resolved.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub descriptor: PathBuf,
    pub rbf: Option<String>,
    pub setname: Option<String>,
    pub class: Class,
    /// One per `<file>`, in document order.
    pub components: Vec<Component>,
    /// The name Main would look for under `games`, for saying where a
    /// missing component was expected.
    home_name: Option<String>,
}

/// No roots and no systems: only for tests whose descriptors carry
/// absolute or root-relative paths. Every production caller builds one
/// from the configured game roots and the systems found on the card.
#[cfg(test)]
impl Default for Homes {
    fn default() -> Self {
        Homes {
            roots: Vec::new(),
            cores: Vec::new(),
        }
    }
}

impl Homes {
    pub fn new(roots: &[String], systems: &[FoundSystem]) -> Homes {
        Homes {
            roots: roots.iter().map(PathBuf::from).collect(),
            cores: systems
                .iter()
                .filter(|system| {
                    !crate::systems::is_favorites(system.category())
                        && !system.def.rbf.trim().is_empty()
                })
                .map(FoundSystem::to_config)
                .collect(),
        }
    }

    /// Read a descriptor and resolve every path in it. Read errors and
    /// malformed XML are the same errors the favourites reader raises.
    pub fn resolve(&self, descriptor: &Path) -> Result<Resolved> {
        let reference = descriptor_reference(descriptor, "favourite MGL")?;
        Ok(self.resolve_reference(descriptor, &reference))
    }

    pub fn resolve_reference(
        &self,
        descriptor: &Path,
        reference: &DescriptorReference,
    ) -> Resolved {
        let (recognised, home_name, homes) = self.home_dirs(
            reference.rbf.as_deref(),
            reference.setname.as_deref(),
            reference.same_dir,
        );
        let class = if recognised || reference.rbf.is_none() {
            Class::Game
        } else {
            Class::CoreSet
        };
        let components = reference
            .files
            .iter()
            .map(
                |raw| match component(raw, &homes, class == Class::CoreSet) {
                    // No home exists: say where Main would have looked, one
                    // `games/<name>` per root, so the log names the folders.
                    Component::Missing { raw, tried } if tried.is_empty() => Component::Missing {
                        tried: home_name
                            .as_deref()
                            .and_then(folder_name)
                            .map(|name| {
                                self.roots
                                    .iter()
                                    .map(|root| root.join(name).join(&raw))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        raw,
                    },
                    component => component,
                },
            )
            .collect();
        Resolved {
            descriptor: descriptor.to_path_buf(),
            rbf: reference.rbf.clone(),
            setname: reference.setname.clone(),
            class,
            components,
            home_name,
        }
    }

    /// Main's `HomeDir()` for a descriptor: whether the core is one of the
    /// recognised systems, the folder name Main would look for, and the
    /// directories that stand for it here.
    ///
    /// A `<setname>` without `same_dir="1"` is the name Main looks for, so
    /// the recognised systems carrying that set name answer with their
    /// folders; failing that, the name itself is looked for over the roots.
    /// Otherwise the core's own name is the folder: the recognised systems
    /// of that core with no set name of their own, or the core's stem over
    /// the roots, `minimig` being `Amiga` as Main spells it.
    fn home_dirs(
        &self,
        rbf: Option<&str>,
        setname: Option<&str>,
        same_dir: bool,
    ) -> (bool, Option<String>, Vec<PathBuf>) {
        let Some(rbf) = rbf else {
            return (false, None, Vec::new());
        };
        let stem = Path::new(rbf)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let matching: Vec<&SystemConfig> = self
            .cores
            .iter()
            .filter(|system| {
                let configured = Path::new(&system.rbf)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("");
                crate::core_variants::same_core_identity(configured, stem)
                    || crate::core_choices::is_unstable_reference(system, rbf)
            })
            .collect();
        let recognised = !matching.is_empty();
        let mut homes = Vec::new();
        let mut add = |folders: &[String]| {
            for folder in folders.iter().filter(|folder| !folder.is_empty()) {
                let folder = PathBuf::from(folder);
                if !homes.contains(&folder) {
                    homes.push(folder);
                }
            }
        };
        let name = match setname.filter(|_| !same_dir) {
            Some(set) => {
                for system in matching.iter().filter(|system| {
                    system
                        .setname
                        .as_deref()
                        .is_some_and(|configured| configured.eq_ignore_ascii_case(set))
                }) {
                    add(&folders_of(system));
                }
                set.to_string()
            }
            None => {
                for system in matching
                    .iter()
                    .filter(|system| system.setname.as_deref().is_none_or(str::is_empty))
                {
                    add(&folders_of(system));
                }
                if stem.eq_ignore_ascii_case("minimig") {
                    "Amiga".to_string()
                } else {
                    stem.to_string()
                }
            }
        };
        if homes.is_empty() {
            if let Some(name) = folder_name(&name) {
                homes.extend(crate::systems::existing_folder(name, &self.roots));
            }
        }
        (recognised, (!name.is_empty()).then_some(name), homes)
    }
}

/// The name as one folder under `games`, or nothing when it is not one:
/// empty, or carrying a `/`, a `.` or a `..` step. Main joins a
/// `<setname>` to `games` as written, so such a value would lead out of
/// the game roots; here it is looked for nowhere, and the descriptor
/// reports its component as missing with the name it gave.
fn folder_name(name: &str) -> Option<&str> {
    let mut steps = Path::new(name).components();
    match (steps.next(), steps.next()) {
        (Some(std::path::Component::Normal(_)), None) => Some(name),
        _ => None,
    }
}

/// One `<file path>` against the home directories that apply.
///
/// With one home and `verify` off nothing is stat'd: the path is as good
/// as an absolute one and a missing file surfaces where an absolute one's
/// would. With several homes, or `verify` on, each candidate is checked
/// and exactly one existing file is the answer. No home at all is
/// `Missing` with nothing tried.
pub fn component(raw: &str, homes: &[PathBuf], verify: bool) -> Component {
    if raw.starts_with('/') {
        return Component::Absolute(normalize_path(PathBuf::from(raw)));
    }
    if let Some(rest) = root_relative(raw) {
        return Component::RootRelative(normalize_path(Path::new("/").join(rest)));
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for home in homes {
        let candidate = normalize_path(home.join(raw));
        if !candidates.contains(&candidate) {
            candidates.push(candidate);
        }
    }
    if candidates.is_empty() {
        return Component::Missing {
            raw: raw.to_string(),
            tried: Vec::new(),
        };
    }
    if candidates.len() == 1 && !verify {
        return Component::Home(candidates.remove(0));
    }
    let mut existing: Vec<PathBuf> = candidates
        .iter()
        .filter(|candidate| exists(candidate))
        .cloned()
        .collect();
    match existing.len() {
        0 => Component::Missing {
            raw: raw.to_string(),
            tried: candidates,
        },
        1 => Component::Home(existing.remove(0)),
        _ => Component::Ambiguous {
            raw: raw.to_string(),
            existing,
        },
    }
}

impl Resolved {
    /// The game a [`Class::Game`] descriptor is for: its last `<file>`, or
    /// nothing when it lists no file. The errors are the ones of
    /// [`Resolved::verify`].
    pub fn game_target(&self) -> Result<Option<PathBuf>> {
        let Some(last) = self.components.last() else {
            return Ok(None);
        };
        if let Some(error) = self.problem(last) {
            return Err(error);
        }
        Ok(target_of(last))
    }

    /// The name a [`Class::CoreSet`] descriptor is known by: its own.
    pub fn identity_name(&self) -> String {
        self.descriptor
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// Every component resolved to one file, or the first that did not:
    ///
    /// - missing: `Io { what: "MGL component", NotFound }` at the one path
    ///   tried, or at the descriptor when several or none were;
    /// - ambiguous: `Unsupported { what: "MGL component" }` naming every
    ///   file found;
    /// - not absolute in a descriptor with no `<rbf>`:
    ///   `Malformed { what: "MGL descriptor" }`.
    pub fn verify(&self) -> Result<()> {
        match self
            .components
            .iter()
            .find_map(|component| self.problem(component))
        {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The first problem, as one line for the log and Game Information.
    pub fn diagnostic(&self) -> Option<Diagnostic> {
        self.components.iter().find_map(|component| {
            let text = self.problem(component)?.to_string();
            Some(if matches!(component, Component::Ambiguous { .. }) {
                Diagnostic::Ambiguous(text)
            } else {
                Diagnostic::Unplaced(text)
            })
        })
    }

    fn problem(&self, component: &Component) -> Option<DegaussError> {
        match component {
            Component::Absolute(_) | Component::RootRelative(_) | Component::Home(_) => None,
            Component::Missing { raw, .. } if self.rbf.is_none() => Some(DegaussError::malformed(
                "MGL descriptor",
                &self.descriptor,
                format!("component {raw:?} is not absolute and the descriptor names no core"),
            )),
            Component::Missing { raw, tried } => Some(match tried.as_slice() {
                [only] => DegaussError::io(
                    "MGL component",
                    only,
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("{raw} is not under {}", parent_of(only).display()),
                    ),
                ),
                [] => DegaussError::io(
                    "MGL component",
                    &self.descriptor,
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!(
                            "{raw} has no games folder to be under: none for {}",
                            self.home_name.as_deref().unwrap_or("the descriptor's core")
                        ),
                    ),
                ),
                several => DegaussError::io(
                    "MGL component",
                    &self.descriptor,
                    std::io::Error::new(
                        std::io::ErrorKind::NotFound,
                        format!("{raw} is not under any of: {}", listed(several)),
                    ),
                ),
            }),
            Component::Ambiguous { raw, existing } => {
                Some(ambiguous_error(&self.descriptor, raw, existing))
            }
        }
    }
}

/// The error for a component found in several folders, shared with the
/// launch relocation so both say the same thing.
pub fn ambiguous_error(descriptor: &Path, raw: &str, existing: &[PathBuf]) -> DegaussError {
    DegaussError::unsupported(
        "MGL component",
        format!(
            "{raw} in {} exists in each of {}; MiSTer would load one of them and Degauss cannot tell which",
            descriptor.display(),
            listed(existing)
        ),
    )
}

/// The one file a resolved component stands for, if it stands for one.
pub fn target_of(component: &Component) -> Option<PathBuf> {
    match component {
        Component::Absolute(path) | Component::RootRelative(path) | Component::Home(path) => {
            Some(path.clone())
        }
        Component::Missing { .. } | Component::Ambiguous { .. } => None,
    }
}

/// True for a path Main joins to the home directory: not absolute and not
/// the root-relative `media` spelling. Bare names, `./x` and `../x` alike.
pub fn needs_home(raw: &str) -> bool {
    !raw.starts_with('/') && root_relative(raw).is_none()
}

/// The `media/...` remainder of the root-relative spelling: a run of `../`
/// followed by `media` alone or `media/...`. An ordinary `../roms/x` is not
/// that and stays home-relative, as Main reads it.
fn root_relative(raw: &str) -> Option<&str> {
    let mut rest = raw;
    let mut climbed = false;
    while let Some(after) = rest.strip_prefix("../") {
        rest = after;
        climbed = true;
    }
    (climbed && (rest == "media" || rest.starts_with("media/"))).then_some(rest)
}

fn folders_of(system: &SystemConfig) -> Vec<String> {
    std::iter::once(system.path.clone())
        .chain(system.extra_paths.iter().cloned())
        .collect()
}

/// A file, or a member inside a ZIP: the existence test every favourite
/// and launch path has always applied.
fn exists(path: &Path) -> bool {
    path.is_file() || crate::zip::validate_member(path).is_ok()
}

fn parent_of(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

fn listed(paths: &[PathBuf]) -> String {
    paths
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("degauss-mgl-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn system(id: &str, rbf: &str, setname: Option<&str>, paths: &[PathBuf]) -> FoundSystem {
        let setname_line = setname
            .map(|set| format!("setname = \"{set}\"\n"))
            .unwrap_or_default();
        let def = crate::systems::parse_table(
            &format!(
                "[[systems]]\nname = \"{id}\"\nid = \"{id}\"\nfolders = [\"{id}\"]\nrbf = \"{rbf}\"\n{setname_line}extensions = [\"bin\", \"nes\", \"fds\", \"mgl\"]\n"
            ),
            Path::new("mgl fixture"),
        )
        .unwrap()
        .remove(0);
        FoundSystem {
            def,
            paths: paths.to_vec(),
            logo_dir: None,
            menu_folder: None,
        }
    }

    fn favorites_system(paths: &[PathBuf]) -> FoundSystem {
        let mut found = system("Favorites", "", None, paths);
        found.def.category = Some("Favorites".into());
        found
    }

    /// A synthetic copy of the public arcade layout: the descriptor under
    /// `_Arcade`, its three bare components installed under the set's own
    /// games folder, and a decoy of the same name beside the descriptor.
    fn arcade_fixture(root: &Path) -> (PathBuf, PathBuf) {
        let arcade = root.join("_Arcade");
        let home = root.join("games/Battletoads");
        std::fs::create_dir_all(&arcade).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        for name in ["btc0-p0.bin", "btc0-p1.bin", "btc0-s.bin"] {
            std::fs::write(home.join(name), b"payload").unwrap();
        }
        std::fs::write(arcade.join("btc0-s.bin"), b"decoy beside the descriptor").unwrap();
        let mgl = arcade.join("Battletoads.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription>\n\t<rbf>_Arcade/cores/Battletoads</rbf>\n\t<setname>Battletoads</setname>\n\t\
             <file delay=\"1\" type=\"f\" index=\"0\" path=\"btc0-p0.bin\"/>\n\t\
             <file delay=\"1\" type=\"f\" index=\"1\" path=\"btc0-p1.bin\"/>\n\t\
             <file delay=\"1\" type=\"f\" index=\"2\" path=\"btc0-s.bin\"/>\n\
             </mistergamedescription>\n",
        )
        .unwrap();
        (mgl, home)
    }

    fn arcade_homes(root: &Path) -> Homes {
        Homes::new(
            &[
                root.join("games").to_string_lossy().into_owned(),
                root.to_string_lossy().into_owned(),
            ],
            &[
                system("Arcade", "", None, &[root.join("_Arcade")]),
                system("NES", "_Console/NES", None, &[root.join("games/NES")]),
                favorites_system(&[root.join("_@Favorites")]),
            ],
        )
    }

    /// The defect this module exists for: a descriptor's bare components
    /// used to be looked for beside the descriptor, so an arcade set that
    /// Main loads correctly was reported missing and never matched.
    #[test]
    fn arcade_core_descriptor_components_resolve_through_the_set_home_dir_not_beside_the_mgl() {
        let root = temp("arcade-home");
        let (mgl, home) = arcade_fixture(&root);
        let resolved = arcade_homes(&root).resolve(&mgl).unwrap();
        assert_eq!(resolved.class, Class::CoreSet);
        assert_eq!(
            resolved.components,
            vec![
                Component::Home(home.join("btc0-p0.bin")),
                Component::Home(home.join("btc0-p1.bin")),
                Component::Home(home.join("btc0-s.bin")),
            ],
            "every component is under games/<setname>, never beside the descriptor"
        );
        assert_eq!(resolved.identity_name(), "Battletoads");
        assert_eq!(resolved.setname.as_deref(), Some("Battletoads"));
        resolved.verify().unwrap();
        assert_eq!(resolved.diagnostic(), None);
        std::fs::remove_dir_all(root).ok();
    }

    /// An ordinary console descriptor keeps naming its game: the last
    /// file, found under the core's own folder, not the companion disc.
    #[test]
    fn console_mgl_bare_game_resolves_through_its_core_home_and_keeps_the_game_identity() {
        let root = temp("console-home");
        let home = root.join("games/NES");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("Game.nes"), b"rom").unwrap();
        let mgl = root.join("_@Favorites/Game.mgl");
        std::fs::create_dir_all(mgl.parent().unwrap()).unwrap();
        std::fs::write(
            &mgl,
            "<mistergamedescription><rbf>_Console/NES</rbf>\
             <file delay=\"1\" type=\"f\" index=\"2\" path=\"Companion.bin\"/>\
             <file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.nes\"/>\
             </mistergamedescription>",
        )
        .unwrap();
        let resolved = arcade_homes(&root).resolve(&mgl).unwrap();
        assert_eq!(resolved.class, Class::Game);
        assert_eq!(
            resolved.game_target().unwrap(),
            Some(home.join("Game.nes")),
            "the game is the last file, under the core's games folder"
        );
        assert_eq!(
            resolved.components[0],
            Component::Home(home.join("Companion.bin")),
            "a single home is not stat'd for a game descriptor: the companion need not exist yet"
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn absolute_components_are_preserved_exactly() {
        let root = temp("absolute");
        let existing = root.join("games/NES/Rock & Roll.nes");
        std::fs::create_dir_all(existing.parent().unwrap()).unwrap();
        std::fs::write(&existing, b"rom").unwrap();
        let mgl = root.join("Absolute.mgl");
        std::fs::write(
            &mgl,
            format!(
                "<mistergamedescription><rbf>_Console/NES</rbf>\
                 <file delay=\"1\" type=\"f\" index=\"1\" path=\"/nowhere/on/this/card/Missing.nes\"/>\
                 <file delay=\"1\" type=\"f\" index=\"1\" path=\"{}\"/>\
                 </mistergamedescription>",
                existing.display().to_string().replace('&', "&amp;")
            ),
        )
        .unwrap();
        // No roots and no systems: nothing to look anything up in, and an
        // absolute path needs nothing looked up.
        let resolved = Homes::default().resolve(&mgl).unwrap();
        assert_eq!(
            resolved.components,
            vec![
                Component::Absolute(PathBuf::from("/nowhere/on/this/card/Missing.nes")),
                Component::Absolute(existing.clone()),
            ],
            "absolute paths are taken as written, existing or not"
        );
        assert_eq!(resolved.game_target().unwrap(), Some(existing));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn the_root_relative_favourite_spelling_still_names_the_card_path() {
        let root = temp("root-relative");
        let mgl = root.join("Relative.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><rbf>_Computer/C64</rbf>\
             <file delay=\"1\" type=\"f\" index=\"1\" path=\"../../../../../media/fat/games/C64/x.crt\"/>\
             </mistergamedescription>",
        )
        .unwrap();
        let resolved = Homes::default().resolve(&mgl).unwrap();
        assert_eq!(
            resolved.components,
            vec![Component::RootRelative(PathBuf::from(
                "/media/fat/games/C64/x.crt"
            ))]
        );
        assert!(!needs_home("../../media/fat/x.crt"));
        assert!(
            needs_home("../roms/x.crt"),
            "an ordinary parent path is home-relative"
        );
        assert!(needs_home("./x.crt"));
        assert!(needs_home("x.crt"));
        assert!(!needs_home("/media/fat/x.crt"));
        std::fs::remove_dir_all(root).ok();
    }

    /// Main appends `./x` and `../x` to the home directory literally, so a
    /// descriptor kept in a favourites folder does not point beside itself.
    #[test]
    fn dot_and_parent_forms_follow_the_home_dir_not_the_descriptor_folder() {
        let root = temp("dot-forms");
        let nes = root.join("games/NES");
        let fds = root.join("games/FDS");
        std::fs::create_dir_all(&nes).unwrap();
        std::fs::create_dir_all(&fds).unwrap();
        let folder = root.join("_@Favorites/Folder");
        std::fs::create_dir_all(&folder).unwrap();
        // Decoys where the old reading looked.
        std::fs::write(folder.join("Game.nes"), b"decoy").unwrap();
        std::fs::create_dir_all(root.join("_@Favorites/FDS")).unwrap();
        std::fs::write(root.join("_@Favorites/FDS/Game.fds"), b"decoy").unwrap();
        let same = folder.join("Same.mgl");
        std::fs::write(
            &same,
            "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"./Game.nes\"/></mistergamedescription>",
        )
        .unwrap();
        let parent = folder.join("Parent.mgl");
        std::fs::write(
            &parent,
            "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"../FDS/Game.fds\"/></mistergamedescription>",
        )
        .unwrap();
        let homes = arcade_homes(&root);
        assert_eq!(
            homes.resolve(&same).unwrap().game_target().unwrap(),
            Some(nes.join("Game.nes"))
        );
        assert_eq!(
            homes.resolve(&parent).unwrap().game_target().unwrap(),
            Some(fds.join("Game.fds"))
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// A system's games are routinely spread across alias folders. The same
    /// name in two of them is two different files, and picking the first
    /// by folder order would silently play another game.
    #[test]
    fn two_alias_folders_holding_the_same_component_are_reported_not_chosen() {
        let root = temp("alias-ambiguity");
        let megadrive = root.join("games/MegaDrive");
        let genesis = root.join("games/Genesis");
        std::fs::create_dir_all(&megadrive).unwrap();
        std::fs::create_dir_all(&genesis).unwrap();
        std::fs::write(megadrive.join("Game.bin"), b"one build").unwrap();
        std::fs::write(genesis.join("Game.bin"), b"another build").unwrap();
        let mgl = root.join("Game.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><rbf>_Console/MegaDrive</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.bin\"/></mistergamedescription>",
        )
        .unwrap();
        let homes = Homes::new(
            &[root.join("games").to_string_lossy().into_owned()],
            &[system(
                "Genesis",
                "_Console/MegaDrive",
                None,
                &[megadrive.clone(), genesis.clone()],
            )],
        );
        let resolved = homes.resolve(&mgl).unwrap();
        assert_eq!(
            resolved.components,
            vec![Component::Ambiguous {
                raw: "Game.bin".into(),
                existing: vec![megadrive.join("Game.bin"), genesis.join("Game.bin")],
            }]
        );
        let error = resolved.game_target().unwrap_err();
        assert!(
            matches!(
                &error,
                DegaussError::Unsupported {
                    what: "MGL component",
                    ..
                }
            ),
            "{error}"
        );
        let text = error.to_string();
        assert!(text.contains(&megadrive.join("Game.bin").display().to_string()));
        assert!(text.contains(&genesis.join("Game.bin").display().to_string()));
        assert_eq!(resolved.diagnostic(), Some(Diagnostic::Ambiguous(text)));

        std::fs::remove_file(genesis.join("Game.bin")).unwrap();
        let resolved = homes.resolve(&mgl).unwrap();
        assert_eq!(
            resolved.components,
            vec![Component::Home(megadrive.join("Game.bin"))],
            "one remaining file is not ambiguous"
        );
        std::fs::remove_file(megadrive.join("Game.bin")).unwrap();
        let resolved = homes.resolve(&mgl).unwrap();
        assert_eq!(
            resolved.components,
            vec![Component::Missing {
                raw: "Game.bin".into(),
                tried: vec![megadrive.join("Game.bin"), genesis.join("Game.bin")],
            }]
        );
        let error = resolved.game_target().unwrap_err();
        assert!(
            matches!(&error, DegaussError::Io { what: "MGL component", source, .. }
                if source.kind() == std::io::ErrorKind::NotFound),
            "{error}"
        );
        assert_eq!(
            resolved.diagnostic(),
            Some(Diagnostic::Unplaced(error.to_string())),
            "nothing found is not the same as too much found"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// The RA launcher writes `<setname same_dir="1">RA_NES</setname>`:
    /// Main keeps the core's own folder for it, so `games/RA_NES` is never
    /// looked for and the core's folder is.
    #[test]
    fn same_dir_launchers_use_the_core_folder_not_a_ra_prefixed_one() {
        let root = temp("same-dir");
        let nes = root.join("games/NES");
        std::fs::create_dir_all(&nes).unwrap();
        std::fs::create_dir_all(root.join("games/RA_NES")).unwrap();
        std::fs::write(root.join("games/RA_NES/Game.nes"), b"decoy").unwrap();
        std::fs::write(nes.join("Game.nes"), b"rom").unwrap();
        let mgl = root.join("Game.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir=\"1\">RA_NES</setname><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.nes\"/></mistergamedescription>",
        )
        .unwrap();
        let homes = arcade_homes(&root);
        let (recognised, name, found) =
            homes.home_dirs(Some("_RA_Cores/Cores/NES"), Some("RA_NES"), true);
        assert!(recognised, "an RA core is the same system");
        assert_eq!(name.as_deref(), Some("NES"));
        assert_eq!(found, vec![nes.clone()]);
        let resolved = homes.resolve(&mgl).unwrap();
        assert_eq!(resolved.class, Class::Game);
        assert_eq!(resolved.game_target().unwrap(), Some(nes.join("Game.nes")));
        std::fs::remove_dir_all(root).ok();
    }

    /// A `<setname>` without `same_dir` is the folder Main looks in: the
    /// recognised system carrying that set name answers, and a set name no
    /// system carries is looked for over the roots as `games/<setname>`.
    #[test]
    fn setname_without_same_dir_selects_the_setname_system_or_its_games_folder() {
        let root = temp("setname");
        let nes = root.join("games/NES");
        let fds = root.join("games/FDS");
        let other = root.join("games/Other");
        for folder in [&nes, &fds, &other] {
            std::fs::create_dir_all(folder).unwrap();
        }
        std::fs::write(fds.join("Game.fds"), b"disk").unwrap();
        std::fs::write(other.join("Game.fds"), b"disk elsewhere").unwrap();
        let homes = Homes::new(
            &[root.join("games").to_string_lossy().into_owned()],
            &[
                system("NES", "_Console/NES", None, std::slice::from_ref(&nes)),
                system(
                    "FDS",
                    "_Console/NES",
                    Some("FDS"),
                    std::slice::from_ref(&fds),
                ),
            ],
        );
        let write = |setname: &str| {
            let mgl = root.join(format!("{setname}.mgl"));
            std::fs::write(
                &mgl,
                format!("<mistergamedescription><rbf>_Console/NES</rbf><setname>{setname}</setname><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.fds\"/></mistergamedescription>"),
            )
            .unwrap();
            mgl
        };
        let known = homes.resolve(&write("FDS")).unwrap();
        assert_eq!(known.game_target().unwrap(), Some(fds.join("Game.fds")));
        let unknown = homes.resolve(&write("Other")).unwrap();
        assert_eq!(
            homes
                .home_dirs(Some("_Console/NES"), Some("Other"), false)
                .2,
            vec![other.clone()],
            "a set name outside the table is games/<setname> over the roots, as Main has it"
        );
        assert_eq!(unknown.class, Class::Game);
        assert_eq!(unknown.game_target().unwrap(), Some(other.join("Game.fds")));
        std::fs::remove_dir_all(root).ok();
    }

    /// Main joins a `<setname>` to `games` as written, so a value with a
    /// path step in it names a folder outside the game roots. Nothing is
    /// looked for there: a component of such a descriptor is missing, and
    /// no path outside the roots is named as tried.
    #[test]
    fn a_setname_with_path_steps_is_looked_for_nowhere() {
        let root = temp("setname-steps");
        let games = root.join("games");
        let outside = root.join("outside");
        std::fs::create_dir_all(&games).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::write(outside.join("Game.fds"), b"disk outside the roots").unwrap();
        let homes = Homes::new(&[games.to_string_lossy().into_owned()], &[]);
        for setname in ["../outside", "./outside", "..", "sub/outside"] {
            let mgl = root.join("Escape.mgl");
            std::fs::write(
                &mgl,
                format!("<mistergamedescription><rbf>_Arcade/cores/Escape</rbf><setname>{setname}</setname><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.fds\"/></mistergamedescription>"),
            )
            .unwrap();
            let resolved = homes.resolve(&mgl).unwrap();
            assert_eq!(
                resolved.components,
                vec![Component::Missing {
                    raw: "Game.fds".into(),
                    tried: Vec::new(),
                }],
                "{setname}: the file outside the roots is never found, and no path there is listed"
            );
            let error = resolved.verify().unwrap_err();
            assert!(
                matches!(&error, DegaussError::Io { what: "MGL component", path, .. } if path == &mgl),
                "{setname}: {error}"
            );
            assert!(error.to_string().contains(setname), "{setname}: {error}");
            assert!(
                error.to_string().contains("Game.fds"),
                "{setname}: the report names the file the descriptor asked for: {error}"
            );
        }
        assert_eq!(
            folder_name("Rock & Roll"),
            Some("Rock & Roll"),
            "an ordinary set name is one folder"
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// Main prints "No rbf found!" for such a descriptor; here the
    /// component cannot be placed and the descriptor is the thing at fault.
    #[test]
    fn a_descriptor_without_rbf_cannot_resolve_a_bare_component() {
        let root = temp("no-rbf");
        let mgl = root.join("NoCore.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.nes\"/></mistergamedescription>",
        )
        .unwrap();
        let resolved = Homes::default().resolve(&mgl).unwrap();
        assert_eq!(resolved.class, Class::Game);
        let error = resolved.game_target().unwrap_err();
        assert!(
            matches!(&error, DegaussError::Malformed { what: "MGL descriptor", path, .. } if path == &mgl),
            "{error}"
        );
        assert!(error.to_string().contains("names no core"));
        std::fs::remove_dir_all(root).ok();
    }

    /// A core no system in the table names is a set of its own, and a
    /// missing part of that set is that entry's failure, reported at the
    /// path Main would have loaded.
    #[test]
    fn a_core_set_with_a_missing_component_names_the_expected_path() {
        let root = temp("core-set-missing");
        let (mgl, home) = arcade_fixture(&root);
        std::fs::remove_file(home.join("btc0-s.bin")).unwrap();
        let resolved = arcade_homes(&root).resolve(&mgl).unwrap();
        assert_eq!(resolved.class, Class::CoreSet);
        let error = resolved.verify().unwrap_err();
        assert!(
            matches!(&error, DegaussError::Io { what: "MGL component", path, source }
                if path == &home.join("btc0-s.bin") && source.kind() == std::io::ErrorKind::NotFound),
            "{error}"
        );
        assert!(
            !error.to_string().contains("_Arcade/btc0-s.bin"),
            "the decoy beside the descriptor is never where the component is looked for"
        );
        // No games folder at all: the roots are named, so the log says
        // where Main would have looked.
        std::fs::remove_dir_all(&home).unwrap();
        let resolved = arcade_homes(&root).resolve(&mgl).unwrap();
        let error = resolved.verify().unwrap_err();
        assert!(
            matches!(&error, DegaussError::Io { what: "MGL component", path, .. } if path == &mgl),
            "{error}"
        );
        assert!(error.to_string().contains("not under any of"));
        assert!(error.to_string().contains(
            &root
                .join("games/Battletoads/btc0-p0.bin")
                .display()
                .to_string()
        ));
        std::fs::remove_dir_all(root).ok();
    }

    /// The roots are searched in the configured order, USB before network
    /// before CIFS before the card, for a set name and for a core name
    /// alike; a recognised system answers with the folders it was found in.
    #[test]
    fn home_dirs_follow_the_configured_root_order() {
        let root = temp("root-order");
        let usb = root.join("usb0/games");
        let network = root.join("network/games");
        let cifs = root.join("fat/cifs/games");
        let card = root.join("fat/games");
        let legacy = root.join("fat");
        for folder in [
            network.join("Battletoads"),
            usb.join("NES"),
            card.join("NES"),
            cifs.join("Elsewhere"),
            legacy.join("_Arcade"),
        ] {
            std::fs::create_dir_all(folder).unwrap();
        }
        let roots: Vec<String> = [&usb, &network, &cifs, &card, &legacy]
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        let homes = Homes::new(
            &roots,
            &[system("NES", "_Console/NES", None, &[usb.join("NES")])],
        );
        let (recognised, name, found) = homes.home_dirs(
            Some("_Arcade/cores/Battletoads"),
            Some("Battletoads"),
            false,
        );
        assert!(!recognised);
        assert_eq!(name.as_deref(), Some("Battletoads"));
        assert_eq!(found, vec![network.join("Battletoads")]);
        let (recognised, _, found) = homes.home_dirs(Some("_Console/NES"), None, false);
        assert!(recognised);
        assert_eq!(
            found,
            vec![usb.join("NES")],
            "the recognised system was found under the first root and answers with that folder"
        );
        let (_, name, found) = homes.home_dirs(Some("_Other/Elsewhere"), None, false);
        assert_eq!(name.as_deref(), Some("Elsewhere"));
        assert_eq!(found, vec![cifs.join("Elsewhere")]);
        let (_, name, found) = homes.home_dirs(Some("_Computer/minimig"), None, false);
        assert_eq!(
            name.as_deref(),
            Some("Amiga"),
            "Main spells the Minimig folder Amiga"
        );
        assert!(found.is_empty());
        assert_eq!(
            homes.home_dirs(None, None, false),
            (false, None, Vec::new())
        );
        std::fs::remove_dir_all(root).ok();
    }
}
