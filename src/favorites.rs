//! Favourites, exactly as MiSTer's own script keeps them.
//!
//! Read from `favorites.sh` on the card rather than invented. It writes two
//! things into `_@Favorites`, in folders the user names:
//!
//! - a core file (`.mra`, `.rbf`, `.mgl`) becomes a **symbolic link** to
//!   the original, under the same name;
//! - anything else becomes an **`.mgl`** naming the core and the file, with
//!   an absolute path and `delay="1"`.
//!
//! Doing the same thing means a favourite made here is a favourite in the
//! stock menu and in every script that reads that folder, and one made
//! anywhere else shows up here. Nothing about it belongs to Degauss.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::error::{DegaussError, Result};

/// The folder MiSTer's script uses, at the top of the card.
pub const FAVORITES_DIR: &str = "_@Favorites";

/// What is favourited, and which file says so.
#[derive(Debug, Default, Clone)]
pub struct Favorites {
    /// The game a favourite points at, and the favourite pointing at it.
    by_target: HashMap<PathBuf, PathBuf>,
}

impl Favorites {
    /// Read the folder. A card without one has no favourites, which is not
    /// an error.
    pub fn read(root: &Path) -> Favorites {
        let mut found = Favorites::default();
        found.walk(root, 0);
        found
    }

    fn walk(&mut self, dir: &Path, depth: usize) {
        if depth > 4 {
            return;
        }
        let Ok(listing) = std::fs::read_dir(dir) else {
            return;
        };
        for item in listing.flatten() {
            let path = item.path();
            // Asked of the card rather than taken from the directory
            // listing: on some filesystems the type in a listing calls a
            // symbolic link a regular file, and every arcade favourite is
            // a link.
            let Ok(meta) = std::fs::symlink_metadata(&path) else {
                continue;
            };
            // A link is a favourited core file, and what it points at is
            // the answer. Read before the extension is looked at, because
            // the link is named after the file it points to.
            if meta.file_type().is_symlink() {
                if let Ok(target) = std::fs::read_link(&path) {
                    self.by_target.insert(target, path);
                }
                continue;
            }
            if meta.is_dir() {
                self.walk(&path, depth + 1);
                continue;
            }
            if path
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("mgl"))
            {
                // A title rather than a path: AmigaVision keeps its
                // library inside one image, so a favourite for one names
                // the title and there is no file to point at.
                if let Some((install, title)) = crate::launch::amiga_marker(&path) {
                    self.by_target.insert(amiga_key(&install, &title), path);
                    continue;
                }
                if let Some(target) = target_of_mgl(&path) {
                    self.by_target.insert(target, path);
                }
            }
        }
    }

    pub fn holds(&self, target: &Path) -> bool {
        self.by_target.contains_key(target)
    }

    /// The favourite pointing at a game, so it can be taken away again.
    pub fn file_for(&self, target: &Path) -> Option<&Path> {
        self.by_target.get(target).map(PathBuf::as_path)
    }

    pub fn len(&self) -> usize {
        self.by_target.len()
    }
}

/// What a favourite points at, under the name that thing is known by.
///
/// A link points at its target directly. An `.mgl` says so in its text,
/// unless it carries a title instead of a path, which is what an
/// AmigaVision favourite does: that one is answered under the same made-up
/// name the title itself is looked up by, so both kinds compare equal.
pub fn target_of(path: &Path) -> Option<PathBuf> {
    if let Ok(target) = std::fs::read_link(path) {
        return Some(target);
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mgl"))
    {
        if let Some((install, title)) = crate::launch::amiga_marker(path) {
            return Some(amiga_key(&install, &title));
        }
        return target_of_mgl(path);
    }
    None
}

/// A name for an AmigaVision title that can be looked up like a path.
///
/// Not a real path and never opened: it only has to be something no file
/// on the card could also be called, so one map answers for both kinds of
/// favourite.
pub fn amiga_key(install: &Path, title: &str) -> PathBuf {
    install.join(".degauss-amigavision").join(title)
}

/// The game an `.mgl` points at.
///
/// Absolute where MiSTer's script wrote it, and relative where something
/// else did, so both are resolved rather than one being assumed.
/// The inverse of the escaping the writer applies, so a name holding an
/// ampersand or a quote matches the file it came from.
fn unescape_attr(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&amp;", "&")
}

fn target_of_mgl(path: &Path) -> Option<PathBuf> {
    let text = std::fs::read_to_string(path).ok()?;
    // The LAST path, not the first. A game that needs a companion disc has
    // the companion written ahead of it, so the first path names the disc
    // and only the last one names the game the favourite is for.
    let at = text.rfind("path=\"")? + "path=\"".len();
    let end = text[at..].find('"')? + at;
    let raw = unescape_attr(&text[at..end]);
    let raw = raw.as_str();
    if raw.starts_with('/') {
        return Some(PathBuf::from(raw));
    }
    // Written relative, in the form Main resolves: as many steps up as it
    // takes and then a path from the root, which is how ours are written
    // because they live in /tmp. What is left after the steps up is
    // therefore a path from the root, not from here.
    if raw.starts_with("../") {
        return Some(PathBuf::from("/").join(raw.trim_start_matches("../")));
    }
    Some(path.parent()?.join(raw))
}

/// The folders inside the favourites folder, as somebody would read them.
pub fn folders(root: &Path) -> Vec<String> {
    let Ok(listing) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = listing
        .flatten()
        .filter(|item| item.file_type().is_ok_and(|kind| kind.is_dir()))
        .map(|item| item.file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with('.'))
        .collect();
    names.sort_by_key(|name| name.to_lowercase());
    names
}

/// Characters MiSTer's own script refuses in a favourite's name, so a name
/// made here cannot be one it would not have made.
pub const BAD_CHARS: [char; 9] = ['\\', '/', ':', '*', '?', '"', '<', '>', '|'];

pub fn name_is_usable(name: &str) -> bool {
    let name = name.trim();
    // "." and ".." carry no forbidden character but are not names: joined to
    // the favourites root they resolve to the root itself or above it.
    !name.is_empty() && name != "." && name != ".." && !name.chars().any(|c| BAD_CHARS.contains(&c))
}

/// The name a favourite is filed under: the name the browser showed for the
/// game, so the favourites folder lists it the way its owner has seen it. A
/// shown name the card cannot hold (empty, ".", "..", any of `BAD_CHARS`)
/// falls back to the file's own stem rather than being sanitised, which is
/// the name MiSTer's own script files a game under. The stem is not
/// validated: it names a file that already exists, and in the rare case a
/// foreign mount let it hold a character the card refuses, writing the
/// favourite fails loudly, exactly as the stock script would.
pub fn favorite_name(display: &str, path: &Path) -> String {
    if name_is_usable(display) {
        display.trim().to_string()
    } else {
        path.file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Make a folder inside the favourites folder.
pub fn make_folder(root: &Path, name: &str) -> Result<PathBuf> {
    if !name_is_usable(name) {
        return Err(DegaussError::unsupported(
            "favourites",
            format!("{name:?} is not a usable folder name"),
        ));
    }
    let path = root.join(name);
    std::fs::create_dir_all(&path)
        .map_err(|e| DegaussError::io("making a favourites folder", &path, e))?;
    Ok(path)
}

/// Write a favourite for a game: an `.mgl` named after it.
pub fn add_game(folder: &Path, name: &str, mgl: &str) -> Result<PathBuf> {
    std::fs::create_dir_all(folder)
        .map_err(|e| DegaussError::io("making a favourites folder", folder, e))?;
    let path = folder.join(format!("{name}.mgl"));
    if path.exists() {
        return Err(DegaussError::unsupported(
            "favourites",
            format!("{} already has a favourite called {name}", folder.display()),
        ));
    }
    std::fs::write(&path, mgl).map_err(|e| DegaussError::io("writing a favourite", &path, e))?;
    Ok(path)
}

/// Favourite a core file by linking to it, which is what the script does:
/// an `.mra` describes its own core and set, so a copy would go stale the
/// day the original is updated.
#[cfg(unix)]
pub fn add_core(folder: &Path, name: &str, source: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(folder)
        .map_err(|e| DegaussError::io("making a favourites folder", folder, e))?;
    let path = folder.join(name);
    if path.exists() {
        return Err(DegaussError::unsupported(
            "favourites",
            format!("{} already has a favourite called {name}", folder.display()),
        ));
    }
    std::os::unix::fs::symlink(source, &path)
        .map_err(|e| DegaussError::io("linking a favourite", &path, e))?;
    Ok(path)
}

pub fn remove(path: &Path) -> Result<()> {
    std::fs::remove_file(path).map_err(|e| DegaussError::io("removing a favourite", path, e))
}

#[cfg(test)]
mod tests {
    /// A folder name has to be a name. "." and ".." pass every character
    /// test and then resolve to the favourites root or above it.
    #[test]
    fn a_folder_cannot_be_named_out_of_the_favourites_root() {
        assert!(!name_is_usable("."));
        assert!(!name_is_usable(".."));
        assert!(!name_is_usable("  ..  "));
        assert!(!name_is_usable(""));
        assert!(!name_is_usable("a/b"));
        assert!(name_is_usable("Arcade"));
        assert!(name_is_usable("Shoot 'em ups"));
    }

    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("degauss-fav-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A game that needs a companion disc has the disc written into the MGL
    /// ahead of it. Reading the first path names the disc, so the game the
    /// favourite is actually for shows no heart and cannot be un-favourited
    /// while standing on it.
    #[test]
    fn a_favourite_with_a_companion_names_the_game_not_the_companion() {
        let dir = std::env::temp_dir().join(format!("fav-companion-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mgl = dir.join("7th Guest.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription>\n\t<rbf>_Computer/ao486</rbf>\n\t\
             <file delay=\"1\" type=\"s\" index=\"2\" path=\"../../media/fat/games/AO486/7th Guest-1.chd\"/>\n\t\
             <file delay=\"1\" type=\"s\" index=\"0\" path=\"../../media/fat/games/AO486/7th Guest.vhd\"/>\n\
             </mistergamedescription>\n",
        )
        .expect("fixture written");
        let found = target_of_mgl(&mgl);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            found,
            Some(PathBuf::from("/media/fat/games/AO486/7th Guest.vhd")),
            "the favourite must name the game, not its companion disc"
        );
    }

    /// The writer escapes an ampersand, so the reader has to put it back or
    /// the path never matches the file it came from.
    #[test]
    fn an_escaped_name_reads_back_as_the_real_path() {
        let dir = std::env::temp_dir().join(format!("fav-escape-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let mgl = dir.join("rock.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription>\n\t<rbf>_Computer/C64</rbf>\n\t\
             <file delay=\"1\" type=\"f\" index=\"1\" path=\"../../media/fat/games/C64/Rock &amp; Roll.crt\"/>\n\
             </mistergamedescription>\n",
        )
        .expect("fixture written");
        let found = target_of_mgl(&mgl);
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            found,
            Some(PathBuf::from("/media/fat/games/C64/Rock & Roll.crt"))
        );
    }

    #[test]
    fn a_favourite_written_by_the_stock_script_is_understood() {
        // Byte for byte what favorites.sh writes, absolute path and all.
        let root = temp("stock");
        let folder = root.join("_Commodore 64");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(
            folder.join("Boulder Dash.mgl"),
            "<mistergamedescription>\n\t<rbf>_Computer/C64</rbf>\n\t\
             <file delay=\"1\" type=\"f\" index=\"1\" \
             path=\"/media/fat/games/C64/Boulder Dash (J1).crt\"/>\n\
             </mistergamedescription>",
        )
        .unwrap();

        let found = Favorites::read(&root);
        assert_eq!(found.len(), 1);
        assert!(found.holds(Path::new("/media/fat/games/C64/Boulder Dash (J1).crt")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_favourite_written_with_a_relative_path_is_understood_too() {
        // Not what the script writes, but what ours would if it wrote one
        // the way it writes a launch.
        let root = temp("relative");
        std::fs::write(
            root.join("thing.mgl"),
            "<mistergamedescription>\n\t<rbf>_Computer/C64</rbf>\n\t\
             <file delay=\"1\" type=\"f\" index=\"1\" \
             path=\"../../../../../media/fat/games/C64/x.crt\"/>\n\
             </mistergamedescription>",
        )
        .unwrap();
        let found = Favorites::read(&root);
        assert!(found.holds(Path::new("/media/fat/games/C64/x.crt")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_name_the_stock_script_would_refuse_is_refused_here() {
        assert!(name_is_usable("Shoot em ups"));
        assert!(!name_is_usable("Shoot/em/ups"));
        assert!(!name_is_usable(""));
        assert!(!name_is_usable("   "));
    }

    #[test]
    fn the_shown_name_names_the_favourite_when_the_card_can_hold_it() {
        // The favourite must be listed under the name the owner has seen,
        // not under the file's stem.
        assert_eq!(
            favorite_name("Blazing Star", Path::new("/x/Blazing Star (blazstar).neo")),
            "Blazing Star"
        );
    }

    #[test]
    fn a_name_the_card_cannot_hold_falls_back_to_the_files_own() {
        // MiSTer's script refuses these characters, so a name made here
        // must be one it would have made: the file's own stem is.
        let path = Path::new("/x/mslug.neo");
        assert_eq!(
            favorite_name("Metal Slug: Super Vehicle-001", path),
            "mslug"
        );
        assert_eq!(favorite_name("A/B", path), "mslug");
        assert_eq!(favorite_name("", path), "mslug");
        assert_eq!(favorite_name("  ", path), "mslug");
    }

    #[test]
    fn a_shown_name_is_trimmed_before_it_names_a_file() {
        // A stray space around a gamelist name would otherwise become part
        // of the filename on the card.
        assert_eq!(
            favorite_name("  Blazing Star  ", Path::new("/x/blazstar.neo")),
            "Blazing Star"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_favourited_core_file_is_a_link_and_is_followed() {
        // Arcade favourites are links to the .mra, which is what the stock
        // script writes. The type in a directory listing calls them plain
        // files on exFAT, so the link has to be asked for directly.
        let root = temp("links");
        let games = temp("links-games");
        let mra = games.join("Alien vs. Predator (Europe 940520).mra");
        std::fs::write(&mra, b"x").unwrap();
        std::os::unix::fs::symlink(&mra, root.join("Alien vs. Predator (Europe 940520).mra"))
            .unwrap();

        let found = Favorites::read(&root);
        assert!(found.holds(&mra), "the link points at the game");
        std::fs::remove_dir_all(&root).ok();
        std::fs::remove_dir_all(&games).ok();
    }
}
