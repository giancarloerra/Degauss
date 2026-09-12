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
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use crate::error::{DegaussError, Result};

/// The folder MiSTer's script uses, at the top of the card.
pub const FAVORITES_DIR: &str = "_@Favorites";
const MAX_MGL_BYTES: u64 = 1024 * 1024;
const MAX_MGL_VALUE_BYTES: usize = 4096;

/// What a Favorite points at, plus optional descriptor evidence that can
/// distinguish systems sharing one folder and file extension. `cache_target`
/// preserves the exact target used by MiSTer's Favorites representation;
/// `owner_target` may be the game named inside a linked MGL.
///
/// A descriptor whose paths cannot be placed (a component missing from
/// its home directory, or found in two of its system's folders) is still
/// a favourite: it is keyed under its own file, so it lists and can be
/// removed, and `diagnostic` says what is wrong with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FavoriteReference {
    pub cache_target: PathBuf,
    pub owner_target: PathBuf,
    pub rbf: Option<String>,
    pub setname: Option<String>,
    pub mgl: bool,
    pub diagnostic: Option<crate::mgl::Diagnostic>,
}

/// An MGL or MRA as written: its file paths in document order, exactly as
/// the attribute has them, and the core and set it names. Where the paths
/// point is the resolver's business (`crate::mgl`), not the parser's.
#[derive(Debug, Default)]
pub(crate) struct DescriptorReference {
    pub files: Vec<String>,
    pub rbf: Option<String>,
    pub setname: Option<String>,
    /// `<setname same_dir="1">`: Main keeps the core's own games folder
    /// instead of one named after the set.
    pub same_dir: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DescriptorField {
    Rbf,
    Setname,
}

/// What is favourited, and which file says so.
#[derive(Debug, Default, Clone)]
pub struct Favorites {
    /// The game a favourite points at, and the favourite pointing at it.
    by_target: HashMap<PathBuf, PathBuf>,
}

impl Favorites {
    /// Read the folder. A card without one has no favourites, which is not
    /// an error.
    #[cfg(test)]
    pub fn read(root: &Path) -> Favorites {
        Favorites::read_with(root, &crate::mgl::Homes::default())
    }

    /// Read the folder, placing each descriptor's paths the way MiSTer
    /// Main would, through `homes`.
    pub fn read_with(root: &Path, homes: &crate::mgl::Homes) -> Favorites {
        let mut found = Favorites::default();
        found.walk(root, 0, homes);
        found
    }

    fn walk(&mut self, dir: &Path, depth: usize, homes: &crate::mgl::Homes) {
        if depth > 6 {
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
                    self.by_target
                        .insert(resolve_link_target(&path, target), path);
                }
                continue;
            }
            if meta.is_dir() {
                self.walk(&path, depth + 1, homes);
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
                if let Some(reference) = reference_of(&path, homes) {
                    // Said once per read, so the log names the favourite
                    // that cannot be placed and what was found instead.
                    if let Some(diagnostic) = &reference.diagnostic {
                        crate::note(&format!("mgl          {}: {diagnostic}", path.display()));
                    }
                    self.by_target.insert(reference.cache_target, path);
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
#[cfg(test)]
pub fn target_of(path: &Path) -> Option<PathBuf> {
    reference_of(path, &crate::mgl::Homes::default()).map(|reference| reference.cache_target)
}

/// Resolve the Favorite target while retaining core/set evidence for callers
/// that need to identify the owning system. This reads only the same symlink,
/// MGL or MRA that represents the Favorite and never changes it.
///
/// The paths inside an MGL are placed as MiSTer Main places them, through
/// `homes`: a game descriptor is known by its game, and a descriptor for a
/// core outside the systems table (an arcade set) by the descriptor itself.
/// One whose paths cannot be placed is keyed under its own file and carries
/// the reason in `diagnostic` rather than being dropped or guessed at.
pub fn reference_of(path: &Path, homes: &crate::mgl::Homes) -> Option<FavoriteReference> {
    if let Ok(target) = std::fs::read_link(path) {
        let cache_target = resolve_link_target(path, target);
        let extension = cache_target
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        let (owner_target, rbf, setname, diagnostic) = match extension.as_str() {
            "mgl" => match homes.resolve(&cache_target) {
                Ok(resolved) => {
                    let (owner_target, diagnostic) = placed(&resolved, &cache_target);
                    (owner_target, resolved.rbf, resolved.setname, diagnostic)
                }
                Err(_) => (cache_target.clone(), None, None, None),
            },
            "mra" => match descriptor_reference(&cache_target, "favourite MRA") {
                Ok(descriptor) => (
                    cache_target.clone(),
                    descriptor.rbf,
                    descriptor.setname,
                    None,
                ),
                Err(_) => (cache_target.clone(), None, None, None),
            },
            _ => (cache_target.clone(), None, None, None),
        };
        return Some(FavoriteReference {
            cache_target,
            owner_target,
            rbf,
            setname,
            mgl: extension == "mgl",
            diagnostic,
        });
    }
    if path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("mgl"))
    {
        if let Some((install, title)) = crate::launch::amiga_marker(path) {
            let target = amiga_key(&install, &title);
            return Some(FavoriteReference {
                cache_target: target.clone(),
                owner_target: target,
                rbf: None,
                setname: None,
                mgl: true,
                diagnostic: None,
            });
        }
        let resolved = homes.resolve(path).ok()?;
        if resolved.class == crate::mgl::Class::Game && resolved.components.is_empty() {
            return None;
        }
        let (target, diagnostic) = placed(&resolved, path);
        return Some(FavoriteReference {
            cache_target: target.clone(),
            owner_target: target,
            rbf: resolved.rbf,
            setname: resolved.setname,
            mgl: true,
            diagnostic,
        });
    }
    None
}

/// What a resolved descriptor stands for: its game for a game descriptor,
/// the descriptor itself for a core set, and the descriptor itself with
/// the reason when the game cannot be placed.
fn placed(
    resolved: &crate::mgl::Resolved,
    descriptor: &Path,
) -> (PathBuf, Option<crate::mgl::Diagnostic>) {
    match resolved.class {
        crate::mgl::Class::CoreSet => (descriptor.to_path_buf(), resolved.diagnostic()),
        crate::mgl::Class::Game => match resolved.game_target() {
            Ok(Some(target)) => (target, None),
            Ok(None) => (descriptor.to_path_buf(), None),
            Err(_) => (descriptor.to_path_buf(), resolved.diagnostic()),
        },
    }
}

/// Whether any file path is one Main would join to the core's home
/// directory (bare, `./x` or `../x`): those are placed through the owning
/// system's folders before launch, the way metadata places them.
pub fn has_home_relative_paths(path: &Path) -> Result<bool> {
    let text = read_mgl_text(path, "favourite MGL")?;
    let mut reader = Reader::from_str(&text);
    loop {
        match reader
            .read_event()
            .map_err(|e| DegaussError::malformed("favourite MGL", path, e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref().eq_ignore_ascii_case("file") => {
                for a in e.attributes() {
                    let a = a.map_err(|e| {
                        DegaussError::malformed("favourite MGL", path, e.to_string())
                    })?;
                    if a.key.as_ref().eq_ignore_ascii_case("path") {
                        let raw = a.normalized_value(XmlVersion::Implicit1_0).map_err(|e| {
                            DegaussError::malformed("favourite MGL", path, e.to_string())
                        })?;
                        if crate::mgl::needs_home(&raw) {
                            return Ok(true);
                        }
                    }
                }
            }
            Event::Eof => return Ok(false),
            _ => {}
        }
    }
}

/// Relocating an MGL must not change the meaning of its relative file paths.
/// Replace only path attribute values, preserving other actions and attributes.
///
/// A path Main would join to the home directory is placed in the owning
/// system's folders, as MiSTer would place it in the core's own; one found
/// in none of them, or in two, is an error rather than a guess.
pub fn relocate_mgl(
    text: &str,
    original: &Path,
    system: &crate::config::SystemConfig,
) -> Result<String> {
    let linked = std::fs::read_link(original)
        .ok()
        .map(|target| resolve_link_target(original, target));
    let original = linked.as_deref().unwrap_or(original);
    let folders: Vec<PathBuf> = std::iter::once(&system.path)
        .chain(system.extra_paths.iter())
        .filter(|folder| !folder.is_empty())
        .map(PathBuf::from)
        .collect();
    let mut reader = Reader::from_str(text);
    let mut patches = Vec::new();
    loop {
        let start = reader.buffer_position() as usize;
        match reader
            .read_event()
            .map_err(|e| DegaussError::malformed("favourite MGL", original, e.to_string()))?
        {
            Event::Start(e) | Event::Empty(e) if e.name().as_ref().eq_ignore_ascii_case("file") => {
                for a in e.attributes() {
                    let a = a.map_err(|e| {
                        DegaussError::malformed("favourite MGL", original, e.to_string())
                    })?;
                    if !a.key.as_ref().eq_ignore_ascii_case("path") {
                        continue;
                    }
                    let raw = a.normalized_value(XmlVersion::Implicit1_0).map_err(|e| {
                        DegaussError::malformed("favourite MGL", original, e.to_string())
                    })?;
                    if raw.starts_with('/') {
                        continue;
                    }
                    let target = match crate::mgl::component(&raw, &folders, true) {
                        crate::mgl::Component::Absolute(target)
                        | crate::mgl::Component::RootRelative(target)
                        | crate::mgl::Component::Home(target) => target,
                        crate::mgl::Component::Missing { .. } => {
                            return Err(DegaussError::unsupported(
                                "favourite file",
                                format!("cannot resolve {raw:?} in {} game folders", system.name),
                            ));
                        }
                        crate::mgl::Component::Ambiguous { existing, .. } => {
                            return Err(crate::mgl::ambiguous_error(original, &raw, &existing));
                        }
                    };
                    // The raw attribute value is a borrowed slice of the input.
                    let bytes = a.value.as_ref();
                    let offset = bytes.as_ptr() as usize - text.as_ptr() as usize;
                    if offset < start || offset + bytes.len() > reader.buffer_position() as usize {
                        return Err(DegaussError::unsupported(
                            "favourite MGL",
                            "cannot locate path attribute",
                        ));
                    }
                    let escaped = quick_xml::escape::escape(target.to_str().ok_or_else(|| {
                        DegaussError::unsupported("favourite file", "non UTF-8 path")
                    })?)
                    .into_owned();
                    patches.push((offset..offset + bytes.len(), escaped));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    let mut result = text.to_owned();
    for (range, replacement) in patches.into_iter().rev() {
        result.replace_range(range, &replacement);
    }
    Ok(result)
}

/// A name for an AmigaVision title that can be looked up like a path.
///
/// Not a real path and never opened: it only has to be something no file
/// on the card could also be called, so one map answers for both kinds of
/// favourite.
pub fn amiga_key(install: &Path, title: &str) -> PathBuf {
    install.join(".degauss-amigavision").join(title)
}

pub fn is_amiga_key(path: &Path) -> bool {
    path.components()
        .any(|component| component.as_os_str() == ".degauss-amigavision")
}

/// Bound the bytes actually consumed, rather than relying on metadata from an
/// earlier instant. A growing file or symlink target cannot bypass this limit.
pub(crate) fn read_mgl_text(path: &Path, what: &'static str) -> Result<String> {
    let file = File::open(path).map_err(|error| DegaussError::io(what, path, error))?;
    let mut bytes = Vec::new();
    file.take(MAX_MGL_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| DegaussError::io(what, path, error))?;
    if bytes.len() as u64 > MAX_MGL_BYTES {
        return Err(DegaussError::unsupported(
            what,
            format!("{} is larger than {MAX_MGL_BYTES} bytes", path.display()),
        ));
    }
    String::from_utf8(bytes).map_err(|error| {
        DegaussError::io(
            what,
            path,
            std::io::Error::new(std::io::ErrorKind::InvalidData, error),
        )
    })
}

/// Read a descriptor as written. The inverse of the escaping the writer
/// applies, so a name holding an ampersand or a quote matches the file it
/// came from. Every `<file>` is kept in document order: a game that needs
/// a companion disc has the companion written ahead of it, so the last
/// path is the game and the earlier ones are what it is fed with.
pub(crate) fn descriptor_reference(path: &Path, what: &'static str) -> Result<DescriptorReference> {
    let text = read_mgl_text(path, what)?;
    let mut reader = Reader::from_str(&text);
    let mut files = Vec::new();
    let mut active = None;
    let mut value = String::new();
    let mut rbf = None;
    let mut rbf_ambiguous = false;
    let mut setname = None;
    let mut setname_ambiguous = false;
    let mut same_dir = false;
    loop {
        match reader.read_event() {
            Ok(Event::Eof) => break,
            Ok(Event::Start(event)) => {
                if event.name().as_ref().eq_ignore_ascii_case("file") {
                    files.extend(file_target(path, what, &event)?);
                } else if event.name().as_ref().eq_ignore_ascii_case("rbf") {
                    active = Some(DescriptorField::Rbf);
                    value.clear();
                } else if event.name().as_ref().eq_ignore_ascii_case("setname") {
                    active = Some(DescriptorField::Setname);
                    value.clear();
                    // Main reads the attribute by name regardless of case
                    // and only the exact value "1" counts.
                    for attribute in event.attributes() {
                        let attribute = attribute.map_err(|error| {
                            DegaussError::malformed(
                                what,
                                path,
                                format!("bad setname attribute: {error}"),
                            )
                        })?;
                        if attribute.key.as_ref().eq_ignore_ascii_case("same_dir")
                            && attribute.value.as_ref() == "1"
                        {
                            same_dir = true;
                        }
                    }
                }
            }
            Ok(Event::Empty(event)) if event.name().as_ref().eq_ignore_ascii_case("file") => {
                files.extend(file_target(path, what, &event)?);
            }
            Ok(Event::Text(text)) if active.is_some() => {
                value.push_str(&text.xml10_content());
                validate_descriptor_value(path, what, &value)?;
            }
            Ok(Event::CData(text)) if active.is_some() => {
                value.push_str(&text.xml10_content());
                validate_descriptor_value(path, what, &value)?;
            }
            Ok(Event::GeneralRef(entity)) if active.is_some() => {
                let name = entity.into_inner();
                if let Some(text) = quick_xml::escape::resolve_predefined_entity(&name) {
                    value.push_str(text);
                } else if let Some(character) = resolve_numeric_entity(&name) {
                    value.push(character);
                } else {
                    return Err(DegaussError::malformed(
                        what,
                        path,
                        format!("unknown entity &{name};"),
                    ));
                }
                validate_descriptor_value(path, what, &value)?;
            }
            Ok(Event::End(event)) => {
                let closed = if event.name().as_ref().eq_ignore_ascii_case("rbf") {
                    Some(DescriptorField::Rbf)
                } else if event.name().as_ref().eq_ignore_ascii_case("setname") {
                    Some(DescriptorField::Setname)
                } else {
                    None
                };
                if closed == active {
                    match active {
                        Some(DescriptorField::Rbf) => {
                            merge_descriptor_value(&mut rbf, &mut rbf_ambiguous, &value)
                        }
                        Some(DescriptorField::Setname) => {
                            merge_descriptor_value(&mut setname, &mut setname_ambiguous, &value)
                        }
                        None => {}
                    }
                    active = None;
                    value.clear();
                }
            }
            Ok(_) => {}
            Err(error) => {
                return Err(DegaussError::malformed(
                    what,
                    path,
                    format!("at position {}: {error}", reader.buffer_position()),
                ));
            }
        }
    }
    let setname = (!setname_ambiguous).then_some(setname).flatten();
    Ok(DescriptorReference {
        files,
        rbf: (!rbf_ambiguous).then_some(rbf).flatten(),
        same_dir: same_dir && setname.is_some(),
        setname,
    })
}

/// The `path` attribute of one `<file>`, as written.
fn file_target(
    path: &Path,
    what: &'static str,
    event: &quick_xml::events::BytesStart<'_>,
) -> Result<Option<String>> {
    let mut target = None;
    for attribute in event.attributes() {
        let attribute = attribute.map_err(|error| {
            DegaussError::malformed(what, path, format!("bad file attribute: {error}"))
        })?;
        if attribute.key.as_ref().eq_ignore_ascii_case("path") {
            let raw = attribute
                .normalized_value(XmlVersion::Implicit1_0)
                .map_err(|error| {
                    DegaussError::malformed(what, path, format!("bad path attribute: {error}"))
                })?;
            target = Some(raw.into_owned());
        }
    }
    Ok(target)
}

fn validate_descriptor_value(path: &Path, what: &'static str, value: &str) -> Result<()> {
    if value.len() > MAX_MGL_VALUE_BYTES {
        return Err(DegaussError::malformed(
            what,
            path,
            format!("descriptor value is longer than {MAX_MGL_VALUE_BYTES} bytes"),
        ));
    }
    Ok(())
}

fn merge_descriptor_value(slot: &mut Option<String>, ambiguous: &mut bool, value: &str) {
    let value = value.trim();
    if value.is_empty() || *ambiguous {
        return;
    }
    match slot {
        Some(existing) if !existing.eq_ignore_ascii_case(value) => {
            *slot = None;
            *ambiguous = true;
        }
        Some(_) => {}
        None => *slot = Some(value.to_string()),
    }
}

fn resolve_numeric_entity(name: &str) -> Option<char> {
    let digits = name.strip_prefix('#')?;
    let value = match digits
        .strip_prefix('x')
        .or_else(|| digits.strip_prefix('X'))
    {
        Some(hex) => u32::from_str_radix(hex, 16).ok()?,
        None => digits.parse().ok()?,
    };
    char::from_u32(value)
}

pub(crate) fn resolve_link_target(link: &Path, target: PathBuf) -> PathBuf {
    if target.is_absolute() {
        normalize_path(target)
    } else {
        normalize_path(link.parent().unwrap_or(Path::new(".")).join(target))
    }
}

pub(crate) fn normalize_path(path: PathBuf) -> PathBuf {
    use std::path::Component;

    let absolute = path.is_absolute();
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => normalized.push(Path::new("/")),
            Component::CurDir => {}
            Component::ParentDir => {
                let previous_is_parent =
                    normalized.components().next_back() == Some(Component::ParentDir);
                if previous_is_parent || (!normalized.pop() && !absolute) {
                    normalized.push("..");
                }
            }
            Component::Normal(part) => normalized.push(part),
        }
    }
    normalized
}

/// The folders inside the favourites folder, as somebody would read them.
pub fn folders(root: &Path) -> Result<Vec<String>> {
    let listing = match std::fs::read_dir(root) {
        Ok(listing) => listing,
        // A card with no favourites yet has no root. The New folder entry is
        // how the first one is created, so absence is an empty collection.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(DegaussError::io("reading favourite folders", root, e)),
    };
    let mut names = Vec::new();
    for item in listing {
        let item = item.map_err(|e| DegaussError::io("reading favourite folders", root, e))?;
        let path = item.path();
        let kind = item
            .file_type()
            .map_err(|e| DegaussError::io("reading a favourite folder", &path, e))?;
        let name = item.file_name().to_string_lossy().into_owned();
        if kind.is_dir() && !name.starts_with('.') {
            names.push(name);
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
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

    /// The systems table with one core in it, and no folder found for it:
    /// enough for a descriptor of that core to be a game descriptor rather
    /// than a set of its own, which is what an absolute or root-relative
    /// favourite of a table core is.
    fn homes_knowing(rbf: &str) -> crate::mgl::Homes {
        let def = crate::systems::parse_table(
            &format!(
                "[[systems]]\nname = \"Core\"\nid = \"Core\"\nfolders = [\"Core\"]\nrbf = \"{rbf}\"\nextensions = [\"crt\", \"rom\"]\n"
            ),
            Path::new("test"),
        )
        .unwrap()
        .remove(0);
        crate::mgl::Homes::new(
            &[],
            &[crate::systems::FoundSystem {
                def,
                paths: Vec::new(),
                logo_dir: None,
                menu_folder: None,
            }],
        )
    }

    fn ra_fixture_systems(folders: &[PathBuf]) -> Vec<crate::systems::FoundSystem> {
        let defs = crate::systems::parse_table(
            r#"
[[systems]]
name = "NES"
id = "NES"
folders = ["NES"]
rbf = "_Console/NES"
extensions = ["nes"]
[[systems]]
name = "FDS"
id = "FDS"
folders = ["NES", "FDS"]
rbf = "_Console/NES"
setname = "FDS"
extensions = ["fds"]
"#,
            Path::new("test"),
        )
        .unwrap();
        defs.into_iter()
            .map(|def| crate::systems::FoundSystem {
                def,
                paths: folders.to_vec(),
                logo_dir: None,
                menu_folder: None,
            })
            .collect()
    }

    /// An RA launcher favourite keeps the core's own folders (`same_dir`),
    /// and a bare name present in two of those folders is two different
    /// files: the favourite is kept, keyed under itself so it can still be
    /// removed, and says why it points at no game, rather than being
    /// assigned the first folder's file. With one file it resolves.
    #[test]
    fn bare_ra_favorites_in_two_alias_folders_are_reported_not_assigned_to_the_first() {
        let root = temp("context-ra");
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        std::fs::create_dir_all(&primary).unwrap();
        std::fs::create_dir_all(&secondary).unwrap();
        for folder in [&primary, &secondary] {
            std::fs::write(folder.join("Game.fds"), b"path fixture").unwrap();
        }
        let systems = ra_fixture_systems(&[primary.clone(), secondary.clone()]);
        let homes = crate::mgl::Homes::new(&[], &systems);
        let favorite = root.join("Game.mgl");
        std::fs::write(&favorite, "<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir=\"1\">RA_NES</setname><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.fds\"/></mistergamedescription>").unwrap();
        let reference = reference_of(&favorite, &homes).unwrap();
        assert_eq!(reference.owner_target, favorite);
        assert_eq!(reference.cache_target, favorite);
        let crate::mgl::Diagnostic::Ambiguous(diagnostic) = reference
            .diagnostic
            .expect("two files of that name is a diagnostic")
        else {
            panic!("two files found is ambiguity, not absence");
        };
        assert!(diagnostic.contains(&primary.join("Game.fds").display().to_string()));
        assert!(diagnostic.contains(&secondary.join("Game.fds").display().to_string()));
        let favorites = Favorites::read_with(&root, &homes);
        assert!(
            !favorites.holds(&primary.join("Game.fds")),
            "neither file is silently the favourite"
        );
        assert!(!favorites.holds(&secondary.join("Game.fds")));
        assert_eq!(favorites.file_for(&favorite), Some(favorite.as_path()));
        let logged = std::fs::read_to_string(crate::LOG_PATH).unwrap();
        assert!(
            logged.contains(&format!("mgl          {}: ", favorite.display())),
            "the read says which favourite cannot be placed"
        );

        std::fs::remove_file(secondary.join("Game.fds")).unwrap();
        let reference = reference_of(&favorite, &homes).unwrap();
        assert_eq!(reference.owner_target, primary.join("Game.fds"));
        assert_eq!(reference.diagnostic, None);
        let favorites = Favorites::read_with(&root, &homes);
        assert!(favorites.holds(&primary.join("Game.fds")));
        assert_eq!(
            favorites.file_for(&primary.join("Game.fds")),
            Some(favorite.as_path())
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn oversized_symlink_descriptors_are_bounded_and_never_assigned_an_owner() {
        let root = temp("oversized-linked-mgl");
        let descriptor = root.join("oversized.mgl");
        let file = File::create(&descriptor).unwrap();
        file.set_len(MAX_MGL_BYTES + 1).unwrap();
        let link = root.join("Favorite.mgl");
        std::os::unix::fs::symlink(&descriptor, &link).unwrap();
        let homes = crate::mgl::Homes::default();
        let error = homes.resolve(&link).unwrap_err().to_string();
        assert!(error.contains("larger than"));
        assert!(crate::launch::amiga_marker(&descriptor).is_none());
        assert!(reference_of(&descriptor, &homes).is_none());
        let system = crate::systems::parse_table(
            r#"
[[systems]]
name = "NES"
id = "NES"
folders = ["NES"]
rbf = "_Console/NES"
extensions = ["nes", "mgl"]
"#,
            Path::new("bounded descriptor fixture"),
        )
        .unwrap()
        .remove(0);
        let found = crate::systems::FoundSystem {
            def: system,
            paths: vec![root.join("games/NES")],
            logo_dir: None,
            menu_folder: None,
        };
        let launch_error = crate::launch::plan_with_choice(
            &found.to_config(),
            &link,
            &root.join("temp.mgl"),
            &root,
            false,
            None,
        )
        .unwrap_err()
        .to_string();
        assert!(launch_error.contains("larger than"));
        assert!(has_home_relative_paths(&link)
            .unwrap_err()
            .to_string()
            .contains("larger than"));
        // Replacing the target with an ordinary valid descriptor still works.
        std::fs::write(&descriptor, "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"/media/fat/games/NES/Game.nes\"/></mistergamedescription>").unwrap();
        assert_eq!(
            homes.resolve(&link).unwrap().game_target().unwrap(),
            Some(PathBuf::from("/media/fat/games/NES/Game.nes"))
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_card_with_no_favourites_root_offers_an_empty_folder_list() {
        // The folder chooser appends New folder. Treating a genuinely absent
        // root as empty is what lets the first favourite create it.
        let root = std::env::temp_dir().join(format!("degauss-fav-missing-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        assert_eq!(
            folders(&root).expect("absence is a new card"),
            Vec::<String>::new()
        );
    }

    #[test]
    fn favourite_folders_are_sorted_and_non_folders_are_not_offered() {
        let root = temp("folder-list");
        std::fs::create_dir_all(root.join("zeta")).unwrap();
        std::fs::create_dir_all(root.join("Arcade")).unwrap();
        std::fs::create_dir_all(root.join(".private")).unwrap();
        std::fs::write(root.join("loose.mgl"), "x").unwrap();

        assert_eq!(
            folders(&root).expect("folder list reads"),
            ["Arcade".to_string(), "zeta".to_string()]
        );
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_favourites_path_that_cannot_be_a_directory_is_reported() {
        // A storage/path failure must not look like an empty collection and
        // offer New folder as if nothing were wrong.
        let root = temp("folder-error");
        let file = root.join("not-a-directory");
        std::fs::write(&file, "x").unwrap();
        assert!(folders(&file).is_err());
        std::fs::remove_dir_all(&root).ok();
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
        let found = crate::mgl::Homes::default()
            .resolve(&mgl)
            .expect("valid MGL")
            .game_target()
            .expect("root-relative paths need no home");
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
        let found = crate::mgl::Homes::default()
            .resolve(&mgl)
            .expect("valid MGL")
            .game_target()
            .expect("root-relative paths need no home");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(
            found,
            Some(PathBuf::from("/media/fat/games/C64/Rock & Roll.crt"))
        );
    }

    #[test]
    fn descriptor_cdata_uses_xml_1_0_line_endings() {
        let dir = temp("cdata-line-endings");
        let mgl = dir.join("cdata.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><setname><![CDATA[Don\r\nPachi]]></setname></mistergamedescription>",
        )
        .unwrap();

        let descriptor = descriptor_reference(&mgl, "test MGL").unwrap();
        assert_eq!(descriptor.setname.as_deref(), Some("Don\nPachi"));

        std::fs::remove_dir_all(dir).unwrap();
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

        let found = Favorites::read_with(&root, &homes_knowing("_Computer/C64"));
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
        let found = Favorites::read_with(&root, &homes_knowing("_Computer/C64"));
        assert!(found.holds(Path::new("/media/fat/games/C64/x.crt")));
        std::fs::remove_dir_all(&root).ok();
    }

    /// Main joins `./x` and `../x` to the core's games folder, never to
    /// the folder the descriptor sits in. A favourite written by hand that
    /// way is therefore held under the game Main would load, and the
    /// files beside it are left alone.
    #[test]
    fn parent_relative_and_same_directory_mgl_targets_follow_the_core_home_dir() {
        let root = temp("ordinary-relative");
        let folder = root.join("_@Favorites/Folder");
        let nes = root.join("games/NES");
        let fds = root.join("games/FDS");
        for directory in [&folder, &nes, &fds, &root.join("_@Favorites/roms")] {
            std::fs::create_dir_all(directory).unwrap();
        }
        std::fs::write(folder.join("Same.nes"), b"beside the descriptor").unwrap();
        std::fs::write(
            root.join("_@Favorites/roms/Parent.fds"),
            b"beside the folder",
        )
        .unwrap();
        let parent_relative = folder.join("Parent.mgl");
        let same_directory = folder.join("Same.mgl");
        std::fs::write(
            &parent_relative,
            "<mistergamedescription><rbf>_Console/NES</rbf><file path=\"../FDS/Parent.fds\"/></mistergamedescription>",
        )
        .unwrap();
        std::fs::write(
            &same_directory,
            "<mistergamedescription><rbf>_Console/NES</rbf><file path=\"./Same.nes\"/></mistergamedescription>",
        )
        .unwrap();
        let systems = ra_fixture_systems(std::slice::from_ref(&nes));
        let homes = crate::mgl::Homes::new(&[], &systems);

        let found = Favorites::read_with(&root, &homes);
        assert!(found.holds(&fds.join("Parent.fds")));
        assert!(found.holds(&nes.join("Same.nes")));
        assert!(!found.holds(&folder.join("Same.nes")));
        assert!(!found.holds(&root.join("_@Favorites/roms/Parent.fds")));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn repeated_relative_parents_are_preserved_during_normalization() {
        // A relative favourite may need to climb above the directory that
        // holds its MGL. Collapsing two leading parents into no parent would
        // silently point that favourite at a different game.
        assert_eq!(
            normalize_path(PathBuf::from("../../roms/x.rom")),
            PathBuf::from("../../roms/x.rom")
        );
        assert_eq!(
            normalize_path(PathBuf::from("folder/../../roms/x.rom")),
            PathBuf::from("../roms/x.rom")
        );
        assert_eq!(
            normalize_path(PathBuf::from("/../../media/fat/games/x.rom")),
            PathBuf::from("/media/fat/games/x.rom")
        );
    }

    #[test]
    fn favorite_reference_keeps_mgl_core_and_set_evidence() {
        let root = temp("mgl-owner-evidence");
        let game = root.join("Colour.rom");
        let mgl = root.join("Colour.mgl");
        std::fs::write(&game, b"game").unwrap();
        std::fs::write(
            &mgl,
            format!(
                "<mistergamedescription><rbf>_Console/Gameboy</rbf><setname>GBC &amp; Color</setname><file path=\"{}\"/></mistergamedescription>",
                game.display()
            ),
        )
        .unwrap();

        let reference = reference_of(&mgl, &homes_knowing("_Console/Gameboy"))
            .expect("valid Favorite reference");

        assert_eq!(reference.cache_target, game);
        assert_eq!(reference.diagnostic, None);
        assert_eq!(reference.owner_target, reference.cache_target);
        assert_eq!(reference.rbf.as_deref(), Some("_Console/Gameboy"));
        assert_eq!(reference.setname.as_deref(), Some("GBC & Color"));
        assert!(reference.mgl);
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn favourites_are_read_through_six_nested_folders_but_not_beyond_the_bound() {
        let root = temp("depth-six");
        let mut folder = root.clone();
        for depth in 1..=7 {
            folder = folder.join(format!("d{depth}"));
            std::fs::create_dir_all(&folder).unwrap();
            let target = format!("/media/fat/games/Test/depth-{depth}.rom");
            std::fs::write(
                folder.join(format!("depth-{depth}.mgl")),
                format!("<mistergamedescription><file path=\"{target}\"/></mistergamedescription>"),
            )
            .unwrap();
        }

        let found = Favorites::read(&root);

        assert!(found.holds(Path::new("/media/fat/games/Test/depth-6.rom")));
        assert!(!found.holds(Path::new("/media/fat/games/Test/depth-7.rom")));
        std::fs::remove_dir_all(root).ok();
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

    #[cfg(unix)]
    #[test]
    fn a_relative_core_symlink_is_resolved_from_the_favourite_folder() {
        let root = temp("relative-link");
        let originals = root.join("originals");
        let folder = root.join("_Arcade");
        std::fs::create_dir_all(&originals).unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        let original = originals.join("Game.mra");
        std::fs::write(&original, b"mra").unwrap();
        let favourite = folder.join("Game.mra");
        std::os::unix::fs::symlink(Path::new("../originals/Game.mra"), &favourite).unwrap();

        let found = Favorites::read(&root);

        assert!(found.holds(&original));
        assert_eq!(target_of(&favourite), Some(original));
        std::fs::remove_dir_all(root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_mgl_keeps_its_cache_target_but_uses_its_game_for_ownership() {
        let root = temp("linked-mgl-reference");
        let originals = root.join("originals");
        let favorites = root.join("_@Favorites");
        std::fs::create_dir_all(&originals).unwrap();
        std::fs::create_dir_all(&favorites).unwrap();
        let game = originals.join("Game.rom");
        let original = originals.join("Game.mgl");
        std::fs::write(&game, b"game").unwrap();
        std::fs::write(
            &original,
            format!(
                "<mistergamedescription><rbf>_Console/Test</rbf><file path=\"{}\"/></mistergamedescription>",
                game.display()
            ),
        )
        .unwrap();
        let favorite = favorites.join("Game.mgl");
        std::os::unix::fs::symlink(Path::new("../originals/Game.mgl"), &favorite).unwrap();

        let homes = homes_knowing("_Console/Test");
        let reference = reference_of(&favorite, &homes).expect("linked MGL reference");

        assert_eq!(reference.cache_target, original);
        assert_eq!(reference.owner_target, game);
        assert_eq!(reference.rbf.as_deref(), Some("_Console/Test"));
        assert_eq!(target_of(&favorite), Some(original));
        std::fs::remove_dir_all(root).ok();
    }
}
