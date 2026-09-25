//! Launching a game, the way MiSTer itself does it.
//!
//! There is no bespoke mechanism here. MiSTer's Main process polls a FIFO at
//! `/dev/MiSTer_cmd` and accepts `load_core <path>`; a path ending in `.mgl`
//! is parsed as a tiny XML "game description" naming the core and the file
//! to feed it. The community MGL packs all
//! launch exactly this way, which is why Degauss does too: no new
//! convention, no per-game files on disk.
//!
//! One MGL is written per launch and overwritten next time. It goes in the
//! system temp directory (tmpfs on a MiSTer) because Main re-executes itself
//! when a core loads and only needs the file to survive that moment.
//!
//! Facts encoded below, all read from MiSTer's own parser rather than
//! assumed:
//!
//! * `<file>` needs ALL of delay, type, index, path: an item missing any
//!   one of them is discarded silently, so a typo means "nothing happens"
//!   rather than an error. The builder therefore cannot construct a partial
//!   item.
//! * `type` is only `s` (mount as a drive/slot) or `f` (send as a file);
//!   anything else kills the item.
//! * `delay` is in whole seconds.
//! * `index` selects the core's own menu slot; the extension sub-index is
//!   computed by Main from the file extension and must not be encoded here.
//! * A path is passed through unchanged when it is absolute, but both
//!   production implementations prefix `../../../../..` to an absolute path,
//!   which resolves identically from any core home directory. Degauss
//!   matches the proven form.
//! * Every other path, `./x` and `../x` included, is joined to the core's
//!   home directory (`games/<setname>` or `games/<core>`), never to the
//!   folder the MGL sits in. Degauss reads such paths the same way, in
//!   `crate::mgl`, when it has to interpret an MGL rather than hand it over.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use sha1::{Digest, Sha1};

use crate::config::{LaunchRule, SystemConfig};
use crate::error::{DegaussError, Result};

/// Where Main listens for commands.
pub const CMD_FIFO: &str = "/dev/MiSTer_cmd";

/// Ask the running MiSTer Main to apply its Menu framebuffer scanline filter.
pub fn set_hdmi_scanlines(enabled: bool, fifo: &Path) -> Result<()> {
    let command = if enabled {
        b"fb_scanlines 1\n".as_slice()
    } else {
        b"fb_scanlines 0\n".as_slice()
    };
    std::fs::write(fifo, command)
        .map_err(|error| DegaussError::io("setting HDMI scanlines", fifo, error))
}

/// Shared game-launch signal read by Zaparoo Core.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub const ACTIVE_GAME_FILE: &str = "/tmp/ACTIVEGAME";

const AMIGAVISION_2026_04_GAMES_BYTES: usize = 38_497;
const AMIGAVISION_2026_04_GAMES_SHA1: [u8; 20] = [
    0x7a, 0x84, 0x4c, 0x5b, 0xca, 0x31, 0xa6, 0xf6, 0xc4, 0x9f, 0x16, 0x2a, 0xd5, 0xe3, 0xdd, 0x5f,
    0x6b, 0x62, 0x01, 0xf6,
];
const AMIGAVISION_2026_04_LAUNCHES: &[u8] =
    include_bytes!("../assets/amigavision-2026-04-launches.txt");

/// One action inside an MGL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MglItem {
    pub delay: u8,
    /// 's' to mount, 'f' to load. Constrained at construction.
    pub kind: char,
    pub index: u8,
    /// Absolute path to the game file.
    pub path: String,
    /// A reset to perform after the file, for cores that need one before
    /// the game will start.
    pub reset: Option<(u8, Option<u8>)>,
}

impl MglItem {
    pub fn new(rule: &LaunchRule, absolute_path: &str) -> Result<Self> {
        let kind = match rule.kind.to_ascii_lowercase().as_str() {
            "s" => 's',
            "f" => 'f',
            other => {
                return Err(DegaussError::unsupported(
                    "MGL type",
                    format!(
                        "{other:?} is not one of \"s\" or \"f\"; MiSTer would discard the item"
                    ),
                ))
            }
        };
        if !absolute_path.starts_with('/') {
            return Err(DegaussError::unsupported(
                "MGL path",
                format!("{absolute_path:?} is not absolute"),
            ));
        }
        Ok(MglItem {
            delay: rule.delay,
            kind,
            index: rule.index,
            path: absolute_path.to_string(),
            reset: rule.reset_delay.map(|delay| (delay, rule.reset_hold)),
        })
    }
}

/// Escape a value for an XML attribute in double quotes.
fn escape_attr(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

/// Start an AmigaVision title.
///
/// AmigaVision keeps its whole library inside one hard disk image and picks
/// a game by NAME at boot: the name goes in a file the image reads on the
/// way up, and the core is then started against that image. So the MGL
/// carries no file at all, only the set name.
///
/// The boot file is the one thing Degauss writes outside its own folder,
/// and it is what AmigaVision itself expects to be written.
pub fn plan_amiga_vision(
    system: &SystemConfig,
    install: &Path,
    title: &str,
    mgl_path: &Path,
) -> Result<LaunchPlan> {
    // AmigaVision names the title in a boot file rather than passing a
    // path, so the MGL carries only the core and the set.
    let mgl = format!(
        "<mistergamedescription>\n\t<rbf>{}</rbf>\n\t<setname>{}</setname>\n</mistergamedescription>\n",
        escape_attr(&system.rbf),
        escape_attr(system.setname.as_deref().unwrap_or("Amiga"))
    );
    let mgl_path_str = mgl_path.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "mgl path",
            format!("{} is not valid UTF-8", mgl_path.display()),
        )
    })?;

    Ok(LaunchPlan {
        mgl,
        mgl_path: mgl_path.to_path_buf(),
        command: format!("load_core {mgl_path_str}\n"),
        boot_file: Some((
            install.join("shared").join("ags_boot"),
            amiga_vision_boot_title(install, title)?,
        )),
    })
}

/// Start an AmigaVision title after applying the same family and version
/// selections used by ordinary file launches.
#[allow(clippy::too_many_arguments)]
pub fn plan_amiga_vision_with_selections(
    system: &SystemConfig,
    install: &Path,
    title: &str,
    mgl_path: &Path,
    menu_root: &Path,
    ra_first: bool,
    selected_version: Option<&str>,
    selected_family: Option<&str>,
) -> Result<LaunchPlan> {
    let family = crate::launch_cores::resolve_for_version(
        system,
        menu_root,
        selected_family,
        selected_version,
        ra_first,
    )?;
    let mut plan = plan_amiga_vision(&family, install, title, mgl_path)?;
    let core = crate::core_choices::resolve(&family, menu_root, selected_version, ra_first)?;
    plan.mgl = crate::core_variants::apply(&plan.mgl, mgl_path, &core)?;
    Ok(plan)
}

/// A second file to mount beside this one, if the rule asks for one and the
/// folder holds it.
///
/// The first match in the folder wins, which is what MiSTer's own tooling
/// does. A game with two CDs needs its own shortcut either way.
fn companion_for(rule: &LaunchRule, game: &Path) -> Result<Option<PathBuf>> {
    if rule.companion_extensions.is_empty() {
        return Ok(None);
    }
    let accepts = |path: &Path| {
        path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            rule.companion_extensions
                .iter()
                .any(|x| x.eq_ignore_ascii_case(e))
        })
    };
    if let Some((archive, member)) = crate::zip::split_member_path(game) {
        let parent = Path::new(&member).parent().unwrap_or(Path::new(""));
        let mut found: Vec<_> = crate::zip::entries(&archive)?
            .into_iter()
            .filter(|entry| entry.name != member)
            .filter(|entry| Path::new(&entry.name).parent().unwrap_or(Path::new("")) == parent)
            .filter(|entry| accepts(Path::new(&entry.name)))
            .map(|entry| archive.join(entry.name))
            .collect();
        found.sort();
        return Ok(found.into_iter().next());
    }
    let Some(dir) = game.parent() else {
        return Ok(None);
    };
    // Preserve the established ordinary-folder behavior. Archive read errors
    // must propagate because a failed listing cannot establish no companion.
    let Ok(listing) = std::fs::read_dir(dir) else {
        return Ok(None);
    };
    let mut found: Vec<PathBuf> = listing
        .flatten()
        .map(|item| item.path())
        .filter(|path| path != game)
        .filter(|path| accepts(path))
        .collect();
    found.sort();
    Ok(found.into_iter().next())
}

/// Encode text back to ISO-8859-1, the way AmigaVision wrote it.
///
/// The listings are Latin-1 and are read by mapping each byte to the code
/// point of the same value, so this is the exact inverse. A character that
/// does not fit in a byte cannot have come from such a listing, and is an
/// error rather than something to mangle quietly: a title written wrong is
/// a game that will not start, with nothing on screen to say why.
fn latin1(text: &str) -> Result<Vec<u8>> {
    text.chars()
        .map(|c| {
            u8::try_from(c as u32).map_err(|_| {
                DegaussError::unsupported(
                    "AmigaVision title",
                    format!("{c:?} in {text:?} is not ISO-8859-1"),
                )
            })
        })
        .collect()
}

fn amiga_vision_lines(bytes: &[u8]) -> impl Iterator<Item = &[u8]> {
    bytes.split(|byte| *byte == b'\n').filter_map(|line| {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        (!line.is_empty()).then_some(line)
    })
}

fn amiga_vision_boot_title(install: &Path, title: &str) -> Result<Vec<u8>> {
    let mut original = latin1(title)?;
    let games_path = install.join("listings").join("games.txt");
    let Ok(metadata) = std::fs::metadata(&games_path) else {
        original.push(b'\n');
        return Ok(original);
    };
    if metadata.len() != AMIGAVISION_2026_04_GAMES_BYTES as u64 {
        original.push(b'\n');
        return Ok(original);
    }
    let Ok(games) = std::fs::read(&games_path) else {
        original.push(b'\n');
        return Ok(original);
    };
    if Sha1::digest(&games).as_slice() != AMIGAVISION_2026_04_GAMES_SHA1 {
        original.push(b'\n');
        return Ok(original);
    }

    let Some(index) = amiga_vision_lines(&games).position(|line| line == original) else {
        original.push(b'\n');
        return Ok(original);
    };
    let Some(mapped) = amiga_vision_lines(AMIGAVISION_2026_04_LAUNCHES).nth(index) else {
        return Err(DegaussError::malformed(
            "AmigaVision launch map",
            games_path,
            format!("missing entry {} for {title:?}", index + 1),
        ));
    };
    let Some((&kind, canonical)) = mapped.split_first() else {
        return Err(DegaussError::malformed(
            "AmigaVision launch map",
            games_path,
            format!("empty entry {} for {title:?}", index + 1),
        ));
    };
    let Some(canonical) = canonical.strip_prefix(b"\t") else {
        return Err(DegaussError::malformed(
            "AmigaVision launch map",
            games_path,
            format!("invalid entry {} for {title:?}", index + 1),
        ));
    };
    if kind == b'I' {
        return Err(DegaussError::unsupported(
            "AmigaVision direct launch",
            format!(
                "{title:?} is listed by AmigaVision under known issues and must be opened from the AmigaVision menu"
            ),
        ));
    }
    if kind != b'G' || canonical.is_empty() {
        return Err(DegaussError::malformed(
            "AmigaVision launch map",
            games_path,
            format!("invalid entry {} for {title:?}", index + 1),
        ));
    }

    let mut boot_title = canonical.to_vec();
    boot_title.push(b'\n');
    Ok(boot_title)
}

/// Build the MGL document. `rbf` is MiSTer's own core reference, e.g.
/// `_Computer/C64`, without extension or datecode: Main resolves it by
/// prefix so it survives core updates.
pub fn build_mgl(rbf: &str, setname: Option<&str>, items: &[MglItem]) -> Result<String> {
    // Main resolves a relative path against the MGL's own folder, and ours
    // sits in /tmp. Five steps up reaches the root from anywhere it could
    // be written.
    build_mgl_with(rbf, setname, items, "../../../../..")
}

/// As [`build_mgl`], choosing what a path is written relative to.
///
/// A favourite is written where MiSTer's own favourites script writes one,
/// several folders down the card, and that script writes absolute paths.
/// Ours are relative because they live in `/tmp`.
pub fn build_mgl_with(
    rbf: &str,
    setname: Option<&str>,
    items: &[MglItem],
    prefix: &str,
) -> Result<String> {
    if items.is_empty() {
        return Err(DegaussError::unsupported(
            "MGL",
            "no items to launch".to_string(),
        ));
    }
    // Main keeps at most six actions and drops the rest without a word.
    if items.len() > 6 {
        return Err(DegaussError::unsupported(
            "MGL",
            format!("{} items; MiSTer keeps only the first 6", items.len()),
        ));
    }

    let mut out = String::from("<mistergamedescription>\n");
    out.push_str(&format!("\t<rbf>{}</rbf>\n", escape_attr(rbf)));
    // One core can present itself as several systems, and the set name is
    // how it is told which. The Atari 7800 core runs 2600 games, the NES
    // core runs the Famicom Disk System, the Master System core runs the
    // Game Gear. Without this the core starts, and starts as the wrong
    // machine.
    if let Some(setname) = setname.filter(|name| !name.is_empty()) {
        out.push_str(&format!("\t<setname>{}</setname>\n", escape_attr(setname)));
    }
    for item in items {
        out.push_str(&format!(
            "\t<file delay=\"{}\" type=\"{}\" index=\"{}\" path=\"{}{}\"/>\n",
            item.delay,
            item.kind,
            item.index,
            prefix,
            escape_attr(&item.path)
        ));
        // MiSTer only keeps a reset action when it carries a delay; hold is
        // optional and defaults to a brief pulse.
        if let Some((delay, hold)) = item.reset {
            match hold {
                Some(hold) => {
                    out.push_str(&format!("\t<reset delay=\"{delay}\" hold=\"{hold}\"/>\n"))
                }
                None => out.push_str(&format!("\t<reset delay=\"{delay}\"/>\n")),
            }
        }
    }
    out.push_str("</mistergamedescription>\n");
    Ok(out)
}

/// What a launch would do, without doing it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    pub mgl: String,
    pub mgl_path: std::path::PathBuf,
    pub command: String,
    /// A file that must be written before the core starts, and what goes in
    /// it. Used by AmigaVision, which chooses a game by name at boot.
    /// A file to write before the core starts, as BYTES rather than text.
    ///
    /// The encoding belongs to whoever is going to read it back, not to
    /// Rust. AmigaVision's boot file is ISO-8859-1 because its listings are,
    /// and writing UTF-8 there turns every accented title into one the
    /// Amiga side cannot match.
    pub boot_file: Option<(PathBuf, Vec<u8>)>,
}

/// Start one explicit launcher from the cached Cores catalogue.
///
/// The catalogue is populated only by the shallow menu-root discovery, but
/// the file is checked again at the final hand-over boundary: it can have
/// been removed after discovery, and an arbitrary path must never become a
/// core command merely because stale cache bytes named it.
pub fn plan_core(core: &Path, menu_root: &Path) -> Result<LaunchPlan> {
    plan_direct_launcher(core, menu_root, &["RBF", "MGL"])
}

/// Launch a locally installed item selected from Core Updates. Standard and
/// Unstable rows use RBFs, while RetroAchievements rows use their MGL
/// launchers. MRA remains accepted for settings written by the earlier
/// catalogue implementation.
pub fn plan_misterzine(item: &Path, menu_root: &Path) -> Result<LaunchPlan> {
    plan_direct_launcher(item, menu_root, &["RBF", "MGL", "MRA"])
}

fn plan_direct_launcher(core: &Path, menu_root: &Path, kinds: &[&str]) -> Result<LaunchPlan> {
    if !core.is_file() {
        return Err(DegaussError::unsupported(
            "core launch",
            format!("{} is no longer installed", core.display()),
        ));
    }
    let canonical_root = menu_root.canonicalize().map_err(|error| {
        DegaussError::io("checking the configured MiSTer menu", menu_root, error)
    })?;
    let canonical_core = core
        .canonicalize()
        .map_err(|error| DegaussError::io("checking the installed launcher", core, error))?;
    let relative = canonical_core.strip_prefix(&canonical_root).map_err(|_| {
        DegaussError::unsupported(
            "core launch",
            format!("{} is outside the configured MiSTer menu", core.display()),
        )
    })?;
    if relative.components().any(|component| {
        matches!(
            component,
            std::path::Component::ParentDir
                | std::path::Component::RootDir
                | std::path::Component::Prefix(_)
        )
    }) {
        return Err(DegaussError::unsupported(
            "core launch",
            format!("{} is outside the configured MiSTer menu", core.display()),
        ));
    }
    let supported = canonical_core
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            kinds
                .iter()
                .any(|kind| extension.eq_ignore_ascii_case(kind))
        });
    if !supported {
        return Err(DegaussError::unsupported(
            "core launch",
            format!(
                "{} is not an {} launcher",
                core.display(),
                kinds.join(" or ")
            ),
        ));
    }
    check_descriptor_core(&canonical_core, menu_root)?;
    let path = canonical_core.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "core launch",
            format!("{} is not valid UTF-8", canonical_core.display()),
        )
    })?;
    Ok(LaunchPlan {
        mgl: String::new(),
        mgl_path: PathBuf::new(),
        command: format!("load_core {path}\n"),
        boot_file: None,
    })
}

/// An MRA or MGL names its own core. Check that Main can find that core
/// before handing over the display and input devices.
fn check_descriptor_core(launcher: &Path, menu_root: &Path) -> Result<()> {
    let Some(extension) = launcher
        .extension()
        .and_then(|extension| extension.to_str())
    else {
        return Ok(());
    };
    let rbf = if extension.eq_ignore_ascii_case("mra") {
        crate::artwork_pack::xml_text(launcher, "rbf", &AtomicBool::new(false))?
    } else if extension.eq_ignore_ascii_case("mgl") {
        crate::favorites::descriptor_reference(launcher, "MGL launcher")?.rbf
    } else {
        None
    };
    if let Some(rbf) = rbf {
        let installed = if extension.eq_ignore_ascii_case("mra") {
            arcade_core_present(launcher, menu_root, &rbf)?
        } else {
            crate::core_variants::core_present(menu_root, &rbf)?
        };
        if !installed {
            return Err(DegaussError::unsupported(
                "launcher core",
                format!(
                    "{} requires {rbf}, which is not installed",
                    launcher.display()
                ),
            ));
        }
    }
    Ok(())
}

/// Main resolves an MRA's bare RBF name in its menu folder's cores directory,
/// accepting both the bare name and Arcade- prefixed versioned filenames.
fn arcade_core_present(launcher: &Path, menu_root: &Path, rbf: &str) -> Result<bool> {
    if Path::new(rbf).components().count() != 1 {
        return Ok(false);
    }
    let arcade_root = launcher
        .ancestors()
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with('_'))
        })
        .last()
        .unwrap_or(menu_root);
    let cores = arcade_root.join("cores");
    let entries = match std::fs::read_dir(&cores) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(error) => return Err(DegaussError::io("Arcade cores directory", &cores, error)),
    };
    let prefixed = format!("Arcade-{rbf}");
    for entry in entries {
        let entry = entry.map_err(|error| DegaussError::io("Arcade core entry", &cores, error))?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.to_ascii_lowercase().ends_with(".rbf") {
            continue;
        }
        let matches = [rbf, prefixed.as_str()].iter().any(|wanted| {
            name.get(..wanted.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(wanted))
                && matches!(name.as_bytes().get(wanted.len()), Some(b'.' | b'_'))
        });
        if matches && entry.path().is_file() {
            return Ok(true);
        }
    }
    Ok(false)
}

fn plan_self_describing(
    system: &SystemConfig,
    game: &Path,
    mgl_path: &Path,
    menu_root: &Path,
) -> Result<LaunchPlan> {
    check_descriptor_core(game, menu_root)?;
    plan(system, game, mgl_path)
}

/// The element a favourite carries when the thing it points at is not a
/// file.
///
/// AmigaVision keeps its library inside one disk image and chooses a title
/// by writing its name into a boot file before the core starts. An MGL has
/// no way to say "write this first", so a favourite for one cannot be an
/// ordinary MGL naming a path: there is no path.
///
/// What it can be is an ordinary MGL that starts AmigaVision, carrying the
/// title in an element Main does not know. Main's MGL parser walks tags it
/// recognises and ignores the rest, so this is a valid MGL everywhere: the
/// stock menu starts AmigaVision at its own menu, and Degauss starts it at
/// the title. Nothing else in the favourites folder needs this, and
/// nothing else uses it.
pub const DEGAUSS_TAG: &str = "degauss";

/// Build the favourite for an AmigaVision title.
pub fn favorite_mgl_amiga(system: &SystemConfig, install: &Path, title: &str) -> Result<String> {
    let install = install.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "install path",
            format!("{} is not valid UTF-8", install.display()),
        )
    })?;
    Ok(format!(
        "<mistergamedescription>\n\t<rbf>{}</rbf>\n\t<setname>{}</setname>\n\t\
         <{DEGAUSS_TAG} kind=\"amigavision\" install=\"{}\" title=\"{}\"/>\n\
         </mistergamedescription>\n",
        escape_attr(&system.rbf),
        escape_attr(system.setname.as_deref().unwrap_or("Amiga")),
        escape_attr(install),
        escape_attr(title)
    ))
}

/// Build an AmigaVision favourite with the effective family and version
/// embedded for MiSTer's native Favourites menu.
pub fn favorite_mgl_amiga_with_selections(
    system: &SystemConfig,
    install: &Path,
    title: &str,
    menu_root: &Path,
    ra_first: bool,
    selected_version: Option<&str>,
    selected_family: Option<&str>,
) -> Result<String> {
    let family = crate::launch_cores::resolve_for_version(
        system,
        menu_root,
        selected_family,
        selected_version,
        ra_first,
    )?;
    let text = favorite_mgl_amiga(&family, install, title)?;
    let core = crate::core_choices::resolve(&family, menu_root, selected_version, ra_first)?;
    crate::core_variants::apply(&text, install, &core)
}

/// The AmigaVision title an MGL carries, where it carries one.
pub fn amiga_marker(mgl: &Path) -> Option<(PathBuf, String)> {
    let text = crate::favorites::read_mgl_text(mgl, "AmigaVision MGL marker").ok()?;
    let at = text.find(&format!("<{DEGAUSS_TAG} "))?;
    let rest = &text[at..];
    let install = attribute(rest, "install")?;
    let title = attribute(rest, "title")?;
    Some((PathBuf::from(install), title))
}

/// One attribute out of an element, with the escapes it was written with
/// put back.
fn attribute(text: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let at = text.find(&needle)? + needle.len();
    let end = text[at..].find('"')? + at;
    Some(unescape_attr(&text[at..end]))
}

fn unescape_attr(value: &str) -> String {
    value
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

/// The MGL a favourite carries, in the form MiSTer's own favourites script
/// writes: an absolute path rather than one relative to `/tmp`.
///
/// [`None`] where a favourite is a link rather than a file: an `.mra`
/// already names its core and its set, so the script links to it and so do
/// we, and a copy would go stale the day the original is updated.
pub fn favorite_mgl(system: &SystemConfig, game: &Path) -> Result<Option<String>> {
    if let Some(extension) = game.extension().and_then(|e| e.to_str()) {
        if matches!(
            extension.to_ascii_lowercase().as_str(),
            "mra" | "mgl" | "rbf"
        ) {
            return Ok(None);
        }
    }
    let rule = rule_for(system, game)?;
    let absolute = game.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "game path",
            format!("{} is not valid UTF-8", game.display()),
        )
    })?;
    let mut items = Vec::new();
    if let Some(companion) = companion_for(rule, game)? {
        if let Some(path) = companion.to_str() {
            let mut extra = MglItem::new(rule, path)?;
            extra.index = rule.companion_index.unwrap_or(rule.index);
            extra.reset = None;
            items.push(extra);
        }
    }
    items.push(MglItem::new(rule, absolute)?);
    build_mgl_with(
        rule.rbf.as_deref().unwrap_or(&system.rbf),
        system.setname.as_deref(),
        &items,
        "",
    )
    .map(Some)
}

/// The rule that starts this file: the one the system declares for its
/// extension, or, in a Neo Geo ROM-set system, the `.neo` rule for any
/// file no rule covers, since Main hands whatever fills the Neo Geo file
/// slot to its ROM-set loader. In every other system a file without a
/// rule is refused rather than guessed at.
fn rule_for<'a>(system: &'a SystemConfig, game: &Path) -> Result<&'a LaunchRule> {
    system
        .rule_for(game)
        .or_else(|| crate::neogeo::romset_rule(system))
        .ok_or_else(|| {
            DegaussError::unsupported(
                "launch rule",
                format!(
                    "no [[systems.launch]] rule covers {:?} in system {}",
                    game.extension().and_then(|e| e.to_str()).unwrap_or(""),
                    system.name
                ),
            )
        })
}

/// The format rule represented by an existing Degauss-compatible favourite.
/// The final file is the selected game; any earlier files are companions.
fn favorite_rule<'a>(system: &'a SystemConfig, favorite: &Path) -> Result<Option<&'a LaunchRule>> {
    let descriptor = crate::favorites::descriptor_reference(favorite, "favourite MGL")?;
    Ok(descriptor
        .files
        .last()
        .and_then(|path| system.rule_for(Path::new(path))))
}

/// The fixed core named by a format-specific rule, after confirming MiSTer
/// can actually load it from the menu root.
fn rule_core(
    system: &SystemConfig,
    rule: &LaunchRule,
    menu_root: &Path,
) -> Result<Option<crate::core_variants::EffectiveCore>> {
    let Some(rbf) = rule.rbf.as_ref() else {
        return Ok(None);
    };
    if !crate::core_variants::core_present(menu_root, rbf)? {
        return Err(DegaussError::unsupported(
            "launch core",
            format!("{rbf} required by {} is not installed", system.name),
        ));
    }
    Ok(Some(crate::core_variants::EffectiveCore {
        rbf: rbf.clone(),
        setname_xml: crate::core_variants::standard(system).setname_xml,
    }))
}

/// The standalone AmigaVision CD32 package keeps its title launchers beside
/// MiSTer's menu, not beside the CHDs they describe. Prefer a launcher on the
/// CHD's own storage, then try the configured MiSTer menu root.
fn amiga_vision_cd32_mgl(
    system: &SystemConfig,
    game: &Path,
    menu_root: &Path,
) -> Result<Option<PathBuf>> {
    if !game
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("chd"))
        || crate::zip::split_member_path(game).is_some()
    {
        return Ok(None);
    }
    let home = std::iter::once(&system.path)
        .chain(system.extra_paths.iter())
        .map(Path::new)
        .filter(|home| {
            home.file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("AmigaCD32"))
                && game.starts_with(home)
        })
        .max_by_key(|home| home.components().count());
    let Some(home) = home else {
        return Ok(None);
    };
    let Some(stem) = game.file_stem() else {
        return Ok(None);
    };

    let relative = Path::new("_Console/_Amiga CD32 Games")
        .join(stem)
        .with_extension("mgl");
    let mut launchers = Vec::with_capacity(2);
    if let Some(storage) = home
        .parent()
        .filter(|games| {
            games
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case("games"))
        })
        .and_then(Path::parent)
    {
        launchers.push(storage.join(&relative));
    }
    let menu_launcher = menu_root.join(relative);
    if launchers.last() != Some(&menu_launcher) {
        launchers.push(menu_launcher);
    }

    for launcher in launchers {
        match std::fs::metadata(&launcher) {
            Ok(metadata) if metadata.is_file() => return Ok(Some(launcher)),
            Ok(_) => (),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => {
                return Err(DegaussError::io(
                    "checking the AmigaVision CD32 launcher",
                    launcher,
                    error,
                ));
            }
        }
    }
    Ok(None)
}

/// Whether starting this file relies on the system's own core.
///
/// A self-describing file names its own core: an `.mra` carries it, a
/// ready-made `.mgl` names it inside, and a bare `.rbf` is one. Only an MGL
/// built here writes `system.rbf` into what MiSTer is asked to load, so
/// only those launches can fail on a core the card does not have.
///
/// The exception among `.mgl` files is an AmigaVision favourite: it holds
/// a title marker rather than a playable shortcut, and `plan` rewrites it
/// into a fresh MGL naming the system's core, so it needs that core after
/// all.
pub fn needs_system_core(game: &Path) -> bool {
    let Some(extension) = game
        .extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
    else {
        return true;
    };
    match extension.as_str() {
        "mra" | "rbf" => false,
        "mgl" => amiga_marker(game).is_some(),
        _ => true,
    }
}

/// Whether a game's core is selected by Degauss rather than fixed by the
/// launch format itself. Unsupported files simply do not offer the action.
pub fn game_core_selectable(system: &SystemConfig, launch: &crate::browse::Launch) -> bool {
    match launch {
        crate::browse::Launch::AmigaVision { .. } => true,
        crate::browse::Launch::File(game) if needs_system_core(game) => {
            rule_for(system, game).is_ok_and(|rule| rule.rbf.is_none())
        }
        crate::browse::Launch::File(_) => false,
    }
}

/// Build a launch using the current preference without persisting variant data.
/// Self-describing custom launchers remain untouched. Recognized Favorites are
/// converted only into the temporary output, preserving the saved Favorite.
#[cfg(test)]
pub fn plan_with_preference(
    system: &SystemConfig,
    game: &Path,
    mgl_path: &Path,
    menu_root: &Path,
    ra_first: bool,
) -> Result<LaunchPlan> {
    plan_with_choice(system, game, mgl_path, menu_root, ra_first, None)
}

#[cfg(test)]
pub fn favorite_mgl_with_preference(
    system: &SystemConfig,
    game: &Path,
    menu_root: &Path,
    ra_first: bool,
) -> Result<Option<String>> {
    favorite_mgl_with_choice(system, game, menu_root, ra_first, None)
}

/// Plan a launch under the chosen core.
///
/// An existing `.mgl` is handed to MiSTer as it is, whatever it names: a
/// custom or arcade descriptor is Main's to read. Only a favourite for one
/// of this system's own cores is rewritten, and only when the chosen core
/// differs or the favourite carries paths Main would join to the home
/// directory: those are placed in the system's folders as Main would place
/// them under `games/<core>`, so the temporary copy names the same file.
#[cfg(test)]
pub fn plan_with_choice(
    system: &SystemConfig,
    game: &Path,
    mgl_path: &Path,
    menu_root: &Path,
    ra_first: bool,
    selected: Option<&str>,
) -> Result<LaunchPlan> {
    plan_with_selections(system, game, mgl_path, menu_root, ra_first, selected, None)
}

/// Plan a launch after selecting the compatible core family, then the core
/// version within it. The original system remains the identity used to
/// recognise existing favourites made with any declared family.
pub fn plan_with_selections(
    system: &SystemConfig,
    game: &Path,
    mgl_path: &Path,
    menu_root: &Path,
    ra_first: bool,
    selected_version: Option<&str>,
    selected_family: Option<&str>,
) -> Result<LaunchPlan> {
    if let Some(launcher) = amiga_vision_cd32_mgl(system, game, menu_root)? {
        return plan_self_describing(system, &launcher, mgl_path, menu_root);
    }
    if let Some((install, title)) = amiga_marker(game) {
        return plan_amiga_vision_with_selections(
            system,
            &install,
            &title,
            mgl_path,
            menu_root,
            ra_first,
            selected_version,
            selected_family,
        );
    }
    if game
        .extension()
        .is_some_and(|s| s.eq_ignore_ascii_case("mgl"))
        && amiga_marker(game).is_none()
    {
        if !crate::core_variants::recognized_favorite(game, system)? {
            return plan_self_describing(system, game, mgl_path, menu_root);
        }
        let referenced = crate::favorites::descriptor_reference(game, "favourite MGL")?
            .files
            .last()
            .map(PathBuf::from)
            .filter(|path| path.is_absolute());
        if let Some(launcher) = referenced
            .as_deref()
            .map(|path| amiga_vision_cd32_mgl(system, path, menu_root))
            .transpose()?
            .flatten()
        {
            return plan_self_describing(system, &launcher, mgl_path, menu_root);
        }
        let fixed = match favorite_rule(system, game)? {
            Some(rule) => rule_core(system, rule, menu_root)?,
            None => None,
        };
        let core = match fixed {
            Some(core) => core,
            None => {
                let family = crate::launch_cores::resolve_for_version(
                    system,
                    menu_root,
                    selected_family,
                    selected_version,
                    ra_first,
                )?;
                crate::core_choices::resolve(&family, menu_root, selected_version, ra_first)?
            }
        };
        if !crate::core_variants::needs_conversion(game, &core)?
            && !crate::favorites::has_home_relative_paths(game)?
        {
            return plan_self_describing(system, game, mgl_path, menu_root);
        }
        let text = crate::core_variants::convert_favorite(game, &core)?;
        let mgl = crate::favorites::relocate_mgl(&text, game, system)?;
        return Ok(LaunchPlan {
            mgl,
            mgl_path: mgl_path.to_path_buf(),
            command: format!("load_core {}\n", mgl_path.display()),
            boot_file: None,
        });
    }
    if !needs_system_core(game) {
        return plan_self_describing(system, game, mgl_path, menu_root);
    }
    let rule = rule_for(system, game)?;
    if rule.rbf.is_some() {
        rule_core(system, rule, menu_root)?;
        return plan(system, game, mgl_path);
    }
    let family = crate::launch_cores::resolve_for_version(
        system,
        menu_root,
        selected_family,
        selected_version,
        ra_first,
    )?;
    let mut result = plan(&family, game, mgl_path)?;
    let core = crate::core_choices::resolve(&family, menu_root, selected_version, ra_first)?;
    result.mgl = crate::core_variants::apply(&result.mgl, mgl_path, &core)?;
    Ok(result)
}

#[cfg(test)]
pub fn favorite_mgl_with_choice(
    system: &SystemConfig,
    game: &Path,
    menu_root: &Path,
    ra_first: bool,
    selected: Option<&str>,
) -> Result<Option<String>> {
    favorite_mgl_with_selections(system, game, menu_root, ra_first, selected, None)
}

pub fn favorite_mgl_with_selections(
    system: &SystemConfig,
    game: &Path,
    menu_root: &Path,
    ra_first: bool,
    selected_version: Option<&str>,
    selected_family: Option<&str>,
) -> Result<Option<String>> {
    let Some(text) = favorite_mgl(system, game)? else {
        return Ok(None);
    };
    if rule_for(system, game)?.rbf.is_some() {
        return Ok(Some(text));
    }
    let family = crate::launch_cores::resolve_for_version(
        system,
        menu_root,
        selected_family,
        selected_version,
        ra_first,
    )?;
    let core = crate::core_choices::resolve(&family, menu_root, selected_version, ra_first)?;
    crate::core_variants::apply(&text, game, &core).map(Some)
}

/// Work out how to start one entry.
///
/// Arcade is the exception that needs no MGL: an `.mra` already names its
/// core and its ROM set, and MiSTer loads one directly. The same is true of
/// an `.mgl` someone has already written and of a bare `.rbf`.
pub fn plan(system: &SystemConfig, game: &Path, mgl_path: &Path) -> Result<LaunchPlan> {
    if let Some(extension) = game.extension().and_then(|e| e.to_str()) {
        // A favourite that carries a title rather than a path: start
        // AmigaVision the way choosing the title in its own folder would,
        // rather than handing MiSTer an MGL that only opens its menu.
        if extension.eq_ignore_ascii_case("mgl") {
            if let Some((install, title)) = amiga_marker(game) {
                return plan_amiga_vision(system, &install, &title, mgl_path);
            }
        }
        if matches!(
            extension.to_ascii_lowercase().as_str(),
            "mra" | "mgl" | "rbf"
        ) {
            let path = game.to_str().ok_or_else(|| {
                DegaussError::unsupported(
                    "game path",
                    format!("{} is not valid UTF-8", game.display()),
                )
            })?;
            return Ok(LaunchPlan {
                mgl: String::new(),
                mgl_path: PathBuf::new(),
                command: format!("load_core {path}\n"),
                boot_file: None,
            });
        }
    }

    let rule = rule_for(system, game)?;

    let absolute = game.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "game path",
            format!("{} is not valid UTF-8", game.display()),
        )
    })?;
    let mut items = Vec::new();
    // The companion is mounted first, so the disk the game boots from is
    // the last thing handed over.
    if let Some(companion) = companion_for(rule, game)? {
        let path = companion.to_str().ok_or_else(|| {
            DegaussError::unsupported(
                "companion path",
                format!("{} is not valid UTF-8", companion.display()),
            )
        })?;
        let mut extra = MglItem::new(rule, path)?;
        extra.index = rule.companion_index.unwrap_or(rule.index);
        extra.reset = None;
        items.push(extra);
    }
    items.push(MglItem::new(rule, absolute)?);
    let mgl = build_mgl(
        rule.rbf.as_deref().unwrap_or(&system.rbf),
        system.setname.as_deref(),
        &items,
    )?;

    let mgl_path_str = mgl_path.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "mgl path",
            format!("{} is not valid UTF-8", mgl_path.display()),
        )
    })?;

    Ok(LaunchPlan {
        mgl,
        mgl_path: mgl_path.to_path_buf(),
        // One command, one newline: Main reads the FIFO once per wakeup and
        // treats the whole buffer as a single command.
        command: format!("load_core {mgl_path_str}\n"),
        boot_file: None,
    })
}

/// Write the MGL and hand it to MiSTer. After this returns, Main reloads the
/// FPGA and re-executes itself; Degauss is on its way out. Device path
/// only, hence the off-target allowance.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn execute(plan: &LaunchPlan, fifo: &Path) -> Result<()> {
    // Whatever the game needs in place before the core comes up.
    if let Some((path, contents)) = &plan.boot_file {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| DegaussError::io("creating boot folder", parent, e))?;
        }
        std::fs::write(path, contents)
            .map_err(|e| DegaussError::io("writing boot file", path, e))?;
    }

    // A self-describing file needs no MGL written for it.
    if !plan.mgl.is_empty() {
        std::fs::write(&plan.mgl_path, plan.mgl.as_bytes())
            .map_err(|e| DegaussError::io("writing mgl", &plan.mgl_path, e))?;
    }

    // Opened write-only: Main holds the read end open permanently, so this
    // does not block, and a missing FIFO is a real error worth seeing.
    std::fs::write(fifo, plan.command.as_bytes())
        .map_err(|e| DegaussError::io("writing to MiSTer command fifo", fifo, e))?;
    Ok(())
}

/// Publish a successfully handed-off game, or clear a previous game's signal
/// when launching a core without a selected game. Zaparoo reads the entire
/// file as the absolute path, with no newline or other framing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn publish_active_game(game: Option<&Path>, target: &Path) -> Result<()> {
    let text = match game {
        Some(path) => path.to_str().ok_or_else(|| {
            DegaussError::unsupported(
                "active game tracking",
                format!("{} is not a UTF-8 path", path.display()),
            )
        })?,
        None => "",
    };
    std::fs::write(target, text).map_err(|e| DegaussError::io("updating active game", target, e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LaunchRule;

    const AMIGAVISION_2026_04_GAMES: &[u8] =
        include_bytes!("../tests/fixtures/amigavision-2026-04-games.txt");

    #[test]
    fn active_game_signal_is_exact_path_and_clears_on_core_only_launch() {
        let marker = std::env::temp_dir().join(format!(
            "degauss-activegame-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let first = Path::new("/media/fat/games/NES/First Game.nes");
        let second = Path::new("/media/usb0/games/NES/Second Game.nes");
        publish_active_game(Some(first), &marker).unwrap();
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            first.as_os_str().as_encoded_bytes()
        );
        publish_active_game(Some(second), &marker).unwrap();
        assert_eq!(
            std::fs::read(&marker).unwrap(),
            second.as_os_str().as_encoded_bytes()
        );
        publish_active_game(None, &marker).unwrap();
        assert!(std::fs::read(&marker).unwrap().is_empty());
        std::fs::remove_file(&marker).unwrap();

        let error = publish_active_game(Some(first), &std::env::temp_dir()).unwrap_err();
        assert!(matches!(error, DegaussError::Io { .. }));
    }

    fn rule(exts: &[&str], kind: &str, index: u8, delay: u8) -> LaunchRule {
        LaunchRule {
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            rbf: None,
            kind: kind.to_string(),
            index,
            delay,
            reset_delay: None,
            reset_hold: None,
            companion_extensions: Vec::new(),
            companion_index: None,
        }
    }

    #[test]
    fn archive_companions_stay_in_the_selected_virtual_directory() {
        let root = std::env::temp_dir().join(format!("degauss-companion-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("library.zip");
        std::fs::write(
            &archive,
            crate::zip::tests_archive(&["Game/game.vhd", "Game/disc.chd", "Other/disc.chd"], false),
        )
        .unwrap();
        let mut rule = c64().launch.remove(0);
        rule.companion_extensions = vec!["chd".into()];
        assert_eq!(
            companion_for(&rule, &archive.join("Game/game.vhd")).unwrap(),
            Some(archive.join("Game/disc.chd"))
        );
        std::fs::write(&archive, b"broken archive").unwrap();
        assert!(companion_for(&rule, &archive.join("Game/game.vhd")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn c64() -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "C64".into(),
            path: "/media/fat/games/C64".into(),
            extensions: vec!["d64".into(), "prg".into(), "crt".into()],
            rbf: "_Computer/C64".into(),
            launch: vec![
                rule(&["d64", "g64", "t64", "d81"], "s", 0, 1),
                rule(&["prg", "crt", "reu", "tap"], "f", 1, 1),
            ],
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    fn shipped_system(id: &str, paths: Vec<PathBuf>) -> SystemConfig {
        let def = crate::systems::load_table(Path::new("assets/systems.toml"))
            .unwrap()
            .into_iter()
            .find(|system| system.id == id)
            .unwrap_or_else(|| panic!("missing shipped system {id}"));
        crate::systems::FoundSystem {
            def,
            paths,
            logo_dir: None,
            menu_folder: None,
        }
        .to_config()
    }

    #[test]
    fn current_and_legacy_ngpc_cores_launch_the_same_existing_libraries() {
        let root = std::env::temp_dir().join(format!("degauss-ngpc-{}", std::process::id()));
        let ngp_folder = root.join("games/NGP");
        let ngpc_folder = root.join("games/NGPC");
        std::fs::create_dir_all(&ngp_folder).unwrap();
        std::fs::create_dir_all(&ngpc_folder).unwrap();
        std::fs::create_dir_all(root.join("_Console")).unwrap();
        std::fs::create_dir_all(root.join("_Arcade")).unwrap();
        let monochrome_legacy_folder = ngp_folder.join("Mono.ngp");
        let monochrome_shared_folder = ngpc_folder.join("Mono Shared.ngp");
        let colour_ngc = ngpc_folder.join("Colour.ngc");
        let colour_npc = ngpc_folder.join("Colour Alias.npc");
        for game in [
            &monochrome_legacy_folder,
            &monochrome_shared_folder,
            &colour_ngc,
            &colour_npc,
        ] {
            std::fs::write(game, b"cartridge").unwrap();
        }
        let pocket = shipped_system(
            "NeoGeoPocket",
            vec![ngp_folder.clone(), ngpc_folder.clone()],
        );
        let colour = shipped_system("NeoGeoPocketColor", vec![ngpc_folder.clone()]);
        assert!(pocket.accepts(&monochrome_legacy_folder));
        assert!(pocket.accepts(&monochrome_shared_folder));
        assert!(colour.accepts(&colour_ngc));
        assert!(colour.accepts(&colour_npc));
        for (system, game) in [
            (&pocket, &monochrome_legacy_folder),
            (&pocket, &monochrome_shared_folder),
            (&colour, &colour_ngc),
            (&colour, &colour_npc),
        ] {
            let rule = system.rule_for(game).expect("cartridge launch rule");
            assert_eq!(rule.kind, "f");
            assert_eq!(rule.index, 1);
        }

        std::fs::write(root.join("_Console/NGPC_20260916.rbf"), b"core").unwrap();
        for (system, game) in [
            (&pocket, &monochrome_legacy_folder),
            (&pocket, &monochrome_shared_folder),
            (&colour, &colour_ngc),
            (&colour, &colour_npc),
        ] {
            let planned =
                plan_with_preference(system, game, &root.join("current.mgl"), &root, false)
                    .unwrap();
            assert!(planned.mgl.contains("<rbf>_Console/NGPC</rbf>"));
            assert!(!planned.mgl.contains("<setname>"));
            assert!(planned.mgl.contains("type=\"f\" index=\"1\""));
        }
        let current_favorite =
            favorite_mgl_with_preference(&pocket, &monochrome_legacy_folder, &root, false)
                .unwrap()
                .unwrap();
        assert!(current_favorite.contains("<rbf>_Console/NGPC</rbf>"));
        assert!(!current_favorite.contains("<setname>"));

        std::fs::remove_file(root.join("_Console/NGPC_20260916.rbf")).unwrap();
        std::fs::write(root.join("_Arcade/JTNGP.rbf"), b"core").unwrap();
        std::fs::write(root.join("_Arcade/JTNGPC.rbf"), b"core").unwrap();
        let legacy_ngp = plan_with_preference(
            &pocket,
            &monochrome_legacy_folder,
            &root.join("legacy-ngp.mgl"),
            &root,
            false,
        )
        .unwrap();
        assert!(legacy_ngp.mgl.contains("<rbf>_Arcade/JTNGP</rbf>"));
        assert!(legacy_ngp.mgl.contains("<setname>NeoGeoPocket</setname>"));
        let legacy_colour = plan_with_preference(
            &colour,
            &colour_ngc,
            &root.join("legacy-ngpc.mgl"),
            &root,
            false,
        )
        .unwrap();
        assert!(legacy_colour.mgl.contains("<rbf>_Arcade/JTNGPC</rbf>"));
        assert!(legacy_colour.mgl.contains("<setname>JTNGPC</setname>"));
        let legacy_favorite = root.join("Legacy Pocket.mgl");
        std::fs::write(&legacy_favorite, &legacy_ngp.mgl).unwrap();
        assert!(crate::core_variants::recognized_favorite(&legacy_favorite, &pocket).unwrap());

        std::fs::write(root.join("_Console/NGPC_20260916.rbf"), b"core").unwrap();
        let preferred = plan_with_preference(
            &pocket,
            &monochrome_legacy_folder,
            &root.join("preferred.mgl"),
            &root,
            false,
        )
        .unwrap();
        assert!(preferred.mgl.contains("<rbf>_Console/NGPC</rbf>"));
        assert!(!preferred.mgl.contains("<setname>"));

        let pinned_legacy = plan_with_selections(
            &pocket,
            &monochrome_shared_folder,
            &root.join("pinned-legacy.mgl"),
            &root,
            false,
            Some("standard"),
            Some("jtngp"),
        )
        .unwrap();
        assert!(pinned_legacy.mgl.contains("<rbf>_Arcade/JTNGP</rbf>"));
        assert!(pinned_legacy
            .mgl
            .contains("<setname>NeoGeoPocket</setname>"));

        let current_favorite_path = root.join("Current Pocket.mgl");
        std::fs::write(&current_favorite_path, &current_favorite).unwrap();
        let converted = plan_with_selections(
            &pocket,
            &current_favorite_path,
            &root.join("converted-favorite.mgl"),
            &root,
            false,
            Some("standard"),
            Some("jtngp"),
        )
        .unwrap();
        assert!(converted.mgl.contains("<rbf>_Arcade/JTNGP</rbf>"));
        assert!(converted.mgl.contains("<setname>NeoGeoPocket</setname>"));

        let pinned_favorite = favorite_mgl_with_selections(
            &pocket,
            &monochrome_legacy_folder,
            &root,
            false,
            Some("standard"),
            Some("jtngp"),
        )
        .unwrap()
        .unwrap();
        assert!(pinned_favorite.contains("<rbf>_Arcade/JTNGP</rbf>"));

        std::fs::remove_file(root.join("_Arcade/JTNGP.rbf")).unwrap();
        let unavailable = plan_with_selections(
            &pocket,
            &monochrome_legacy_folder,
            &root.join("missing-pinned.mgl"),
            &root,
            false,
            Some("standard"),
            Some("jtngp"),
        )
        .unwrap_err();
        assert!(unavailable
            .to_string()
            .contains("JTNGP (Legacy) is not installed"));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn game_and_watch_uses_the_core_required_by_each_file_format() {
        let root =
            std::env::temp_dir().join(format!("degauss-game-and-watch-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        std::fs::create_dir_all(root.join("_Console")).unwrap();
        std::fs::write(root.join("_Console/GnW.rbf"), b"core").unwrap();
        std::fs::write(root.join("_Console/GameAndWatch.rbf"), b"core").unwrap();
        std::fs::write(root.join("_Console/Custom.rbf"), b"core").unwrap();
        let system = shipped_system("GameNWatch", vec![root.join("games/GameNWatch")]);
        let legacy = root.join("games/GameNWatch/Legacy.bin");
        let current = root.join("games/GameNWatch/Current.gnw");

        let legacy_plan = plan(&system, &legacy, &root.join("legacy.mgl")).unwrap();
        assert!(legacy_plan.mgl.contains("<rbf>_Console/GnW</rbf>"));
        let current_plan = plan_with_selections(
            &system,
            &current,
            &root.join("current.mgl"),
            &root,
            true,
            Some("ra"),
            Some("not-installed"),
        )
        .unwrap();
        assert!(current_plan
            .mgl
            .contains("<rbf>_Console/GameAndWatch</rbf>"));
        assert!(!current_plan.mgl.contains("<rbf>_Console/GnW</rbf>"));

        let current_favorite = favorite_mgl_with_selections(
            &system,
            &current,
            &root,
            true,
            Some("ra"),
            Some("not-installed"),
        )
        .unwrap()
        .unwrap();
        assert!(current_favorite.contains("<rbf>_Console/GameAndWatch</rbf>"));

        let archive = root.join("watches.zip");
        let zipped_legacy = archive.join("Legacy.bin");
        let zipped_current = archive.join("Current.gnw");
        assert!(
            plan(&system, &zipped_legacy, &root.join("zipped-legacy.mgl"))
                .unwrap()
                .mgl
                .contains("<rbf>_Console/GnW</rbf>")
        );
        assert!(
            plan(&system, &zipped_current, &root.join("zipped-current.mgl"))
                .unwrap()
                .mgl
                .contains("<rbf>_Console/GameAndWatch</rbf>")
        );

        let existing = root.join("Existing Current.mgl");
        let old = build_mgl_with(
            "_Console/GnW",
            None,
            &[MglItem {
                delay: 1,
                kind: 'f',
                index: 1,
                path: current.to_string_lossy().into_owned(),
                reset: None,
            }],
            "",
        )
        .unwrap();
        std::fs::write(&existing, &old).unwrap();
        let converted = plan_with_selections(
            &system,
            &existing,
            &root.join("converted.mgl"),
            &root,
            true,
            Some("ra"),
            Some("not-installed"),
        )
        .unwrap();
        assert!(converted.mgl.contains("<rbf>_Console/GameAndWatch</rbf>"));

        let custom = root.join("Custom.mgl");
        let custom_text = "<mistergamedescription><rbf>_Console/Custom</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"Current.gnw\"/></mistergamedescription>";
        std::fs::write(&custom, custom_text).unwrap();
        let custom_plan = plan_with_selections(
            &system,
            &custom,
            &root.join("custom-output.mgl"),
            &root,
            false,
            None,
            None,
        )
        .unwrap();
        assert!(custom_plan.mgl.is_empty());
        assert_eq!(std::fs::read_to_string(&custom).unwrap(), custom_text);

        let mismatched = root.join("Mismatched.mgl");
        let mismatched_text = "<mistergamedescription><rbf>_Console/GameAndWatch</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"Legacy.bin\"/></mistergamedescription>";
        std::fs::write(&mismatched, mismatched_text).unwrap();
        assert!(!crate::core_variants::recognized_favorite(&mismatched, &system).unwrap());
        let mismatched_plan = plan_with_selections(
            &system,
            &mismatched,
            &root.join("mismatched-output.mgl"),
            &root,
            false,
            None,
            None,
        )
        .unwrap();
        assert!(mismatched_plan.mgl.is_empty());
        assert_eq!(
            std::fs::read_to_string(&mismatched).unwrap(),
            mismatched_text
        );

        std::fs::remove_file(root.join("_Console/GameAndWatch.rbf")).unwrap();
        for game in [&current, &existing] {
            let error = plan_with_selections(
                &system,
                game,
                &root.join("missing-core.mgl"),
                &root,
                false,
                None,
                None,
            )
            .unwrap_err();
            assert!(error.to_string().contains("GameAndWatch"), "{error}");
            assert!(error.to_string().contains("not installed"), "{error}");
        }

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn cd32_chds_use_matching_amigavision_launchers_across_storage() {
        let root = std::env::temp_dir().join(format!("degauss-cd32-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let sd = root.join("fat");
        let usb = root.join("usb0");
        let sd_games = sd.join("games/AmigaCD32");
        let usb_games = usb.join("games/AmigaCD32");
        for storage in [&sd, &usb] {
            std::fs::create_dir_all(storage.join("_Console/_Amiga CD32 Games")).unwrap();
        }
        std::fs::create_dir_all(sd.join("_Computer")).unwrap();
        std::fs::create_dir_all(sd.join("_Other")).unwrap();
        std::fs::write(sd.join("_Other/Custom.rbf"), b"core").unwrap();
        std::fs::create_dir_all(&sd_games).unwrap();
        std::fs::create_dir_all(&usb_games).unwrap();
        std::fs::write(sd.join("_Computer/Minimig.rbf"), b"core").unwrap();
        let system = shipped_system("AmigaCD32", vec![sd_games.clone(), usb_games.clone()]);

        for (storage, games, title) in [
            (&sd, &sd_games, "Chaos Engine"),
            (&usb, &usb_games, "Alien Breed"),
        ] {
            let game = games.join(format!("{title}.chd"));
            let supplied = storage
                .join("_Console/_Amiga CD32 Games")
                .join(format!("{title}.mgl"));
            std::fs::write(&game, b"chd").unwrap();
            std::fs::write(
                &supplied,
                format!(
                    "<mistergamedescription><setname>{title}</setname></mistergamedescription>"
                ),
            )
            .unwrap();

            let plan =
                plan_with_preference(&system, &game, &root.join("temporary.mgl"), &sd, false)
                    .unwrap();
            assert!(plan.mgl.is_empty(), "the supplied MGL is not rewritten");
            assert_eq!(plan.command, format!("load_core {}\n", supplied.display()));
        }

        let usb_game = usb_games.join("Alien Breed.chd");
        let favourite = sd.join("_@Favorites/Alien Breed.mgl");
        std::fs::create_dir_all(favourite.parent().unwrap()).unwrap();
        let favourite_text = favorite_mgl_with_preference(&system, &usb_game, &sd, false)
            .unwrap()
            .unwrap();
        std::fs::write(&favourite, favourite_text).unwrap();
        let favorite_plan = plan_with_preference(
            &system,
            &favourite,
            &root.join("favorite-output.mgl"),
            &sd,
            false,
        )
        .unwrap();
        assert!(favorite_plan.mgl.is_empty());
        assert_eq!(
            favorite_plan.command,
            format!(
                "load_core {}\n",
                usb.join("_Console/_Amiga CD32 Games/Alien Breed.mgl")
                    .display()
            )
        );

        let same_storage = usb.join("_Console/_Amiga CD32 Games/Alien Breed.mgl");
        let menu_root_launcher = sd.join("_Console/_Amiga CD32 Games/Alien Breed.mgl");
        std::fs::remove_file(&same_storage).unwrap();
        std::fs::write(&menu_root_launcher, b"menu-root supplied").unwrap();
        let cross_storage =
            plan_with_preference(&system, &usb_game, &root.join("cross.mgl"), &sd, false).unwrap();
        assert!(
            cross_storage.mgl.is_empty(),
            "the supplied MGL is not rewritten"
        );
        assert_eq!(
            cross_storage.command,
            format!("load_core {}\n", menu_root_launcher.display())
        );
        let favorite_cross_storage = plan_with_preference(
            &system,
            &favourite,
            &root.join("favorite-cross.mgl"),
            &sd,
            false,
        )
        .unwrap();
        assert_eq!(
            favorite_cross_storage.command,
            format!("load_core {}\n", menu_root_launcher.display())
        );

        std::fs::write(&same_storage, b"same-storage supplied").unwrap();
        let preferred =
            plan_with_preference(&system, &usb_game, &root.join("preferred.mgl"), &sd, false)
                .unwrap();
        assert_eq!(
            preferred.command,
            format!("load_core {}\n", same_storage.display()),
            "the CHD storage keeps precedence when both launchers exist"
        );

        std::fs::remove_file(&same_storage).unwrap();
        std::fs::remove_file(&menu_root_launcher).unwrap();
        let fallback =
            plan_with_preference(&system, &usb_game, &root.join("fallback.mgl"), &sd, false)
                .unwrap();
        assert!(fallback.mgl.contains("<rbf>_Computer/Minimig</rbf>"));
        assert!(fallback.mgl.contains(usb_game.to_string_lossy().as_ref()));

        std::fs::write(&same_storage, b"supplied").unwrap();
        std::fs::write(&menu_root_launcher, b"menu supplied").unwrap();
        let cue = usb_games.join("Alien Breed.cue");
        std::fs::write(&cue, b"cue").unwrap();
        let cue_plan =
            plan_with_preference(&system, &cue, &root.join("cue.mgl"), &sd, false).unwrap();
        assert!(cue_plan.mgl.contains(cue.to_string_lossy().as_ref()));
        assert_ne!(
            cue_plan.command,
            format!("load_core {}\n", same_storage.display())
        );

        let direct_mgl = usb_games.join("Custom.mgl");
        std::fs::write(
            &direct_mgl,
            "<mistergamedescription><rbf>_Other/Custom</rbf></mistergamedescription>",
        )
        .unwrap();
        let direct_plan = plan_with_preference(
            &system,
            &direct_mgl,
            &root.join("direct-output.mgl"),
            &sd,
            false,
        )
        .unwrap();
        assert!(direct_plan.mgl.is_empty());
        assert_eq!(
            direct_plan.command,
            format!("load_core {}\n", direct_mgl.display())
        );

        let other_games = usb.join("games/Other");
        std::fs::create_dir_all(&other_games).unwrap();
        let other_game = other_games.join("Alien Breed.chd");
        std::fs::write(&other_game, b"chd").unwrap();
        let mut other = system.clone();
        other.name = "Other CD system".into();
        other.path = other_games.to_string_lossy().into_owned();
        other.extra_paths.clear();
        let other_plan =
            plan_with_preference(&other, &other_game, &root.join("other.mgl"), &sd, false).unwrap();
        assert!(other_plan
            .mgl
            .contains(other_game.to_string_lossy().as_ref()));
        assert_eq!(
            other_plan.command,
            format!("load_core {}\n", root.join("other.mgl").display())
        );

        std::fs::remove_dir_all(root).ok();
    }

    fn amiga_vision_system(install: &Path) -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: install.to_string_lossy().into_owned(),
            extensions: vec![],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: Some("Amiga".into()),
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    fn amiga_vision_install(name: &str, games: &[u8]) -> PathBuf {
        let install =
            std::env::temp_dir().join(format!("degauss-amigavision-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&install).ok();
        std::fs::create_dir_all(install.join("listings")).unwrap();
        std::fs::write(install.join("listings/games.txt"), games).unwrap();
        install
    }

    #[test]
    fn amigavision_titles_and_new_favourites_apply_the_selected_family() {
        let install = amiga_vision_install("game-core", b"Zool 2\n");
        let root = install
            .parent()
            .unwrap()
            .join(format!("degauss-amigavision-cores-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("_Computer")).unwrap();
        std::fs::write(root.join("_Computer/Minimig.rbf"), b"primary").unwrap();
        std::fs::write(root.join("_Computer/MinimigLegacy.rbf"), b"legacy").unwrap();
        let mut system = amiga_vision_system(&install);
        system.compatible_cores.push(crate::config::CoreProfile {
            id: "legacy".into(),
            label: "Legacy Minimig".into(),
            rbf: "_Computer/MinimigLegacy".into(),
            setname: Some("Amiga".into()),
        });

        let plan = plan_amiga_vision_with_selections(
            &system,
            &install,
            "Zool 2",
            &root.join("degauss.mgl"),
            &root,
            false,
            None,
            Some("legacy"),
        )
        .unwrap();
        assert!(plan.mgl.contains("<rbf>_Computer/MinimigLegacy</rbf>"));
        assert_eq!(plan.boot_file.unwrap().1, b"Zool 2\n");

        let favorite = favorite_mgl_amiga_with_selections(
            &system,
            &install,
            "Zool 2",
            &root,
            false,
            None,
            Some("legacy"),
        )
        .unwrap();
        assert!(favorite.contains("<rbf>_Computer/MinimigLegacy</rbf>"));
        assert!(favorite.contains("kind=\"amigavision\""));

        std::fs::remove_dir_all(root).unwrap();
        std::fs::remove_dir_all(install).unwrap();
    }

    #[test]
    fn a_prg_produces_the_documented_c64_mgl() {
        let plan = plan(
            &c64(),
            Path::new("/media/fat/games/C64/Boulder.prg"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");

        assert_eq!(
            plan.mgl,
            "<mistergamedescription>\n\
             \t<rbf>_Computer/C64</rbf>\n\
             \t<file delay=\"1\" type=\"f\" index=\"1\" path=\"../../../../../media/fat/games/C64/Boulder.prg\"/>\n\
             </mistergamedescription>\n"
        );
        assert_eq!(plan.command, "load_core /tmp/degauss.mgl\n");
    }

    #[test]
    fn a_disk_image_mounts_into_the_drive_slot_instead_of_loading() {
        let plan = plan(
            &c64(),
            Path::new("/media/fat/games/C64/Game.d64"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");
        assert!(
            plan.mgl.contains(r#"type="s" index="0""#),
            "got: {}",
            plan.mgl
        );
    }

    #[test]
    fn an_accented_amiga_title_is_written_back_in_the_encoding_it_came_from() {
        // The listings are ISO-8859-1, and Degauss reads them by mapping
        // each byte to the code point of the same value. Writing the chosen
        // title back as UTF-8 turns every byte above 0x7f into two, and
        // AmigaVision then cannot match the name it wrote itself: the core
        // starts and the game does not.
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        };
        let plan = plan_amiga_vision(
            &system,
            Path::new("/media/fat/games/Amiga"),
            "B\u{e9}zier",
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");

        let (_, contents) = plan.boot_file.as_ref().expect("a boot file");
        assert_eq!(
            contents.as_slice(),
            b"B\xe9zier\n",
            "one byte for the accent, not two"
        );
    }

    #[test]
    fn april_2026_amigavision_friendly_names_use_their_exact_launch_names() {
        let install = amiga_vision_install("friendly", AMIGAVISION_2026_04_GAMES);
        let system = amiga_vision_system(&install);

        for (friendly, canonical) in [
            ("1000 Miglia", b"1000 Miglia (OCS)[en]\n".as_slice()),
            ("1869", b"1869 (AGA)[en]\n".as_slice()),
            (
                "Ast\u{e9}rix Operation Getafix",
                b"Ast\xe9rix Operation Getafix (OCS)[en]\n".as_slice(),
            ),
        ] {
            let plan = plan_amiga_vision(&system, &install, friendly, Path::new("/tmp/a.mgl"))
                .expect("planned");
            assert_eq!(plan.boot_file.expect("boot file").1, canonical);
        }

        std::fs::remove_dir_all(install).ok();
    }

    #[test]
    fn april_2026_amigavision_demos_keep_their_existing_launch_names() {
        let install = amiga_vision_install("demo", AMIGAVISION_2026_04_GAMES);
        let system = amiga_vision_system(&install);
        let title = "1001 Stolen Ideas (Airwalk)(AGA)";
        let plan =
            plan_amiga_vision(&system, &install, title, Path::new("/tmp/a.mgl")).expect("planned");

        assert_eq!(
            plan.boot_file.expect("boot file").1,
            b"1001 Stolen Ideas (Airwalk)(AGA)\n"
        );
        std::fs::remove_dir_all(install).ok();
    }

    #[test]
    fn other_amigavision_listings_are_not_remapped() {
        let mut changed = AMIGAVISION_2026_04_GAMES.to_vec();
        changed[0] = b'X';
        let install = amiga_vision_install("changed", &changed);
        let system = amiga_vision_system(&install);
        let plan = plan_amiga_vision(&system, &install, "1000 Miglia", Path::new("/tmp/a.mgl"))
            .expect("planned");

        assert_eq!(plan.boot_file.expect("boot file").1, b"1000 Miglia\n");
        std::fs::remove_dir_all(install).ok();
    }

    #[test]
    fn april_2026_known_issue_titles_report_that_they_cannot_launch_directly() {
        let install = amiga_vision_install("known-issue", AMIGAVISION_2026_04_GAMES);
        let system = amiga_vision_system(&install);
        let error = plan_amiga_vision(
            &system,
            &install,
            "Dynamite D\u{fc}x",
            Path::new("/tmp/a.mgl"),
        )
        .expect_err("known issue titles have no direct launcher");

        assert!(error.to_string().contains("known issues"), "got: {error}");
        std::fs::remove_dir_all(install).ok();
    }

    #[test]
    fn an_existing_short_name_favourite_uses_the_april_2026_launch_name() {
        let install = amiga_vision_install("favourite", AMIGAVISION_2026_04_GAMES);
        let system = amiga_vision_system(&install);
        let favourite_text = favorite_mgl_amiga(&system, &install, "1000 Miglia").unwrap();
        let favourite = install.join("1000 Miglia.mgl");
        std::fs::write(&favourite, favourite_text).unwrap();

        let plan = plan(&system, &favourite, Path::new("/tmp/a.mgl")).expect("planned");
        assert_eq!(
            plan.boot_file.expect("boot file").1,
            b"1000 Miglia (OCS)[en]\n"
        );
        std::fs::remove_dir_all(install).ok();
    }

    #[test]
    fn april_2026_amigavision_launch_map_stays_aligned_with_its_listing() {
        let games: Vec<_> = amiga_vision_lines(AMIGAVISION_2026_04_GAMES).collect();
        let launches: Vec<_> = amiga_vision_lines(AMIGAVISION_2026_04_LAUNCHES).collect();

        assert_eq!(games.len(), 2_670);
        assert_eq!(launches.len(), games.len());
        assert!(launches.iter().all(|entry| {
            matches!(entry.first(), Some(b'G' | b'I'))
                && entry.get(1) == Some(&b'\t')
                && entry.len() > 2
        }));
        let digest: [u8; 20] = Sha1::digest(AMIGAVISION_2026_04_GAMES).into();
        assert_eq!(digest, AMIGAVISION_2026_04_GAMES_SHA1);
    }

    #[test]
    fn a_title_that_is_not_latin1_is_refused_rather_than_mangled() {
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        };
        let err = plan_amiga_vision(
            &system,
            Path::new("/media/fat/games/Amiga"),
            "\u{4e2d}",
            Path::new("/tmp/degauss.mgl"),
        )
        .expect_err("must refuse");
        assert!(err.to_string().contains("ISO-8859-1"), "got: {err}");
    }

    #[test]
    fn an_amiga_vision_title_is_named_in_the_boot_file_not_pointed_at() {
        // The library lives inside one disk image, so there is no file to
        // hand over: the game is chosen by name on the way up.
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        };
        let plan = plan_amiga_vision(
            &system,
            Path::new("/media/fat/games/Amiga"),
            "Turrican II (AGA)[en]",
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");

        assert!(plan.mgl.contains("<setname>Amiga</setname>"));
        assert!(
            !plan.mgl.contains("<file"),
            "there is no file to load: {}",
            plan.mgl
        );
        let (boot, contents) = plan.boot_file.as_ref().expect("a boot file is required");
        assert_eq!(boot, Path::new("/media/fat/games/Amiga/shared/ags_boot"));
        assert_eq!(contents.as_slice(), b"Turrican II (AGA)[en]\n");
    }

    #[test]
    fn executing_writes_the_boot_file_before_the_command() {
        let dir = std::env::temp_dir().join(format!("degauss-ags-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: dir.to_string_lossy().into_owned(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        };
        let plan = plan_amiga_vision(&system, &dir, "Lotus II", &dir.join("degauss.mgl")).unwrap();
        execute(&plan, &dir.join("cmd")).expect("executed");

        assert_eq!(
            std::fs::read_to_string(dir.join("shared/ags_boot")).unwrap(),
            "Lotus II\n"
        );
        assert!(std::fs::read_to_string(dir.join("cmd"))
            .unwrap()
            .contains("load_core"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_arcade_mra_is_launched_directly_with_no_mgl() {
        // An .mra already names its core and ROM set. Wrapping one in an
        // MGL would be inventing a step MiSTer does not need.
        let plan = plan(
            &c64(),
            Path::new("/media/fat/_Arcade/Pac-Man.mra"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");
        assert!(plan.mgl.is_empty(), "no MGL should be written");
        assert_eq!(plan.command, "load_core /media/fat/_Arcade/Pac-Man.mra\n");
    }

    #[test]
    fn an_existing_mgl_shortcut_is_passed_straight_through() {
        let plan = plan(
            &c64(),
            Path::new("/media/fat/_Console/Thing.mgl"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");
        assert!(plan.mgl.is_empty());
        assert!(plan.command.contains("Thing.mgl"));
    }

    /// A card with the NES core, its games folder, and an alias folder
    /// the same system also owns, for favourites that need relocating.
    fn nes_card(name: &str) -> (PathBuf, SystemConfig) {
        let root =
            std::env::temp_dir().join(format!("degauss-launch-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        for folder in [
            "_Console",
            "games/NES",
            "games/Famicom",
            "games/FDS",
            "_@Favorites",
        ] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(root.join("_Console/NES.rbf"), b"core fixture").unwrap();
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "NES".into(),
            path: root.join("games/NES").to_string_lossy().into_owned(),
            extra_paths: vec![root.join("games/Famicom").to_string_lossy().into_owned()],
            extensions: vec!["nes".into(), "fds".into()],
            rbf: "_Console/NES".into(),
            launch: vec![rule(&["nes", "fds"], "f", 1, 1)],
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
        };
        (root, system)
    }

    /// An arcade descriptor names a core outside this system's own, so it
    /// is not a favourite of this system's to rewrite: Main gets the file
    /// itself and reads its bare components under `games/<setname>`.
    /// Rewriting it would be inventing paths Main already knows.
    #[test]
    fn an_arcade_core_mgl_is_handed_to_main_unrewritten() {
        let (root, system) = nes_card("arcade-direct");
        std::fs::create_dir_all(root.join("_Arcade/cores")).unwrap();
        std::fs::write(root.join("_Arcade/cores/Battletoads.rbf"), b"core").unwrap();
        let mgl = root.join("_@Favorites/Battletoads.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription>\n\t<rbf>_Arcade/cores/Battletoads</rbf>\n\t<setname>Battletoads</setname>\n\t\
             <file delay=\"1\" type=\"f\" index=\"0\" path=\"btc0-p0.bin\"/>\n\t\
             <file delay=\"1\" type=\"f\" index=\"1\" path=\"btc0-p1.bin\"/>\n\t\
             <file delay=\"1\" type=\"f\" index=\"2\" path=\"btc0-s.bin\"/>\n\
             </mistergamedescription>\n",
        )
        .unwrap();
        let planned = plan_with_choice(
            &system,
            &mgl,
            &root.join("temp.mgl"),
            &root,
            false,
            Some("standard"),
        )
        .unwrap();
        assert!(planned.mgl.is_empty(), "no MGL is written for it");
        assert_eq!(planned.command, format!("load_core {}\n", mgl.display()));
        std::fs::remove_dir_all(root).ok();
    }

    /// A favourite of this system's core with `../x` paths is placed as
    /// Main places it, in the system's own folders, never beside the
    /// favourite: the temporary copy has to name the file Main would load.
    #[test]
    fn relocation_of_parent_relative_components_uses_the_home_dir() {
        let (root, system) = nes_card("relocate-parent");
        std::fs::write(root.join("games/NES/Game.nes"), b"rom").unwrap();
        std::fs::create_dir_all(root.join("_@Favorites/NES")).unwrap();
        std::fs::write(
            root.join("_@Favorites/NES/Game.nes"),
            b"decoy beside the folder",
        )
        .unwrap();
        let mgl = root.join("_@Favorites/Folder/Game.mgl");
        std::fs::create_dir_all(mgl.parent().unwrap()).unwrap();
        std::fs::write(
            &mgl,
            "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"../NES/Game.nes\"/></mistergamedescription>",
        )
        .unwrap();
        let planned = plan_with_choice(
            &system,
            &mgl,
            &root.join("temp.mgl"),
            &root,
            false,
            Some("standard"),
        )
        .unwrap();
        assert!(
            planned.mgl.contains(&format!(
                "path=\"{}\"",
                root.join("games/NES/Game.nes").display()
            )),
            "{}",
            planned.mgl
        );
        assert!(!planned.mgl.contains("_@Favorites/NES/Game.nes"));
        assert_eq!(
            std::fs::read_to_string(&mgl)
                .unwrap()
                .matches("path=")
                .count(),
            1
        );
        std::fs::remove_dir_all(root).ok();
    }

    /// The same name in two of the system's folders is two files. Main
    /// would take one by its own order; here the launch says so instead
    /// of starting a game the user may not have meant.
    #[test]
    fn relocation_reports_an_ambiguous_component_instead_of_choosing() {
        let (root, system) = nes_card("relocate-ambiguous");
        std::fs::write(root.join("games/NES/Game.nes"), b"one").unwrap();
        std::fs::write(root.join("games/Famicom/Game.nes"), b"another").unwrap();
        let mgl = root.join("_@Favorites/Game.mgl");
        std::fs::write(
            &mgl,
            "<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"Game.nes\"/></mistergamedescription>",
        )
        .unwrap();
        let error = plan_with_choice(
            &system,
            &mgl,
            &root.join("temp.mgl"),
            &root,
            false,
            Some("standard"),
        )
        .unwrap_err();
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
        assert!(text.contains(&root.join("games/NES/Game.nes").display().to_string()));
        assert!(text.contains(&root.join("games/Famicom/Game.nes").display().to_string()));
        std::fs::remove_file(root.join("games/Famicom/Game.nes")).unwrap();
        let planned = plan_with_choice(
            &system,
            &mgl,
            &root.join("temp.mgl"),
            &root,
            false,
            Some("standard"),
        )
        .unwrap();
        assert!(planned
            .mgl
            .contains(&root.join("games/NES/Game.nes").display().to_string()));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_dos_disk_brings_its_cd_with_it() {
        // A DOS game boots from the hard disk image and then asks for its
        // CD. Mounting only what was selected starts a game that cannot
        // find itself.
        let dir = std::env::temp_dir().join(format!("degauss-dos-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let vhd = dir.join("7th Guest.vhd");
        std::fs::write(&vhd, b"disk").unwrap();
        std::fs::write(dir.join("7th Guest-1.chd"), b"disc").unwrap();

        let mut system = c64();
        system.rbf = "_Computer/ao486".to_string();
        system.extensions = vec!["vhd".to_string()];
        system.launch = vec![LaunchRule {
            extensions: vec!["vhd".to_string()],
            rbf: None,
            kind: "s".to_string(),
            index: 2,
            delay: 0,
            reset_delay: Some(1),
            reset_hold: None,
            companion_extensions: vec!["iso".to_string(), "chd".to_string()],
            companion_index: Some(4),
        }];

        let plan = plan(&system, &vhd, Path::new("/tmp/degauss.mgl")).expect("plans");
        assert!(
            plan.mgl.contains("index=\"4\""),
            "no CD mounted: {}",
            plan.mgl
        );
        assert!(plan.mgl.contains("7th Guest-1.chd"), "{}", plan.mgl);
        assert!(
            plan.mgl.contains("index=\"2\""),
            "no disk mounted: {}",
            plan.mgl
        );
        assert!(plan.mgl.contains("<reset delay=\"1\"/>"), "{}", plan.mgl);
        // One reset, after both files, not one per file.
        assert_eq!(plan.mgl.matches("<reset").count(), 1, "{}", plan.mgl);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_disk_with_no_cd_beside_it_mounts_alone() {
        let dir = std::env::temp_dir().join(format!("degauss-dos-solo-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let vhd = dir.join("Solo.vhd");
        std::fs::write(&vhd, b"disk").unwrap();

        let mut system = c64();
        system.rbf = "_Computer/ao486".to_string();
        system.extensions = vec!["vhd".to_string()];
        system.launch = vec![LaunchRule {
            extensions: vec!["vhd".to_string()],
            rbf: None,
            kind: "s".to_string(),
            index: 2,
            delay: 0,
            reset_delay: Some(1),
            reset_hold: None,
            companion_extensions: vec!["iso".to_string(), "chd".to_string()],
            companion_index: Some(4),
        }];

        let plan = plan(&system, &vhd, Path::new("/tmp/degauss.mgl")).expect("plans");
        assert_eq!(plan.mgl.matches("<file").count(), 1, "{}", plan.mgl);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_core_shared_by_several_systems_is_told_which_one_to_be() {
        // The Atari 7800 core also runs 2600 games. Without the set name it
        // starts as a 7800 and the game does not run. Every launcher that
        // works carries the
        // same value for the same reason.
        let mut system = c64();
        system.rbf = "_Console/Atari7800".to_string();
        system.setname = Some("Atari2600".to_string());
        system.extensions = vec!["a26".to_string()];
        system.launch = vec![rule(&["a26"], "f", 1, 1)];

        let plan = plan(
            &system,
            Path::new("/media/fat/games/Atari2600/Pitfall.a26"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("plans");
        assert!(
            plan.mgl.contains("<setname>Atari2600</setname>"),
            "got: {}",
            plan.mgl
        );
        // Order matters: the core comes first, then which machine it is.
        let rbf_at = plan.mgl.find("<rbf>").expect("rbf");
        let set_at = plan.mgl.find("<setname>").expect("setname");
        let file_at = plan.mgl.find("<file ").expect("file");
        assert!(rbf_at < set_at && set_at < file_at);
    }

    #[test]
    fn a_system_with_no_set_name_does_not_get_the_tag() {
        let plan = plan(
            &c64(),
            Path::new("/media/fat/games/C64/Uridium.prg"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("plans");
        assert!(!plan.mgl.contains("setname"), "got: {}", plan.mgl);
    }

    #[test]
    fn an_extension_with_no_rule_refuses_to_launch_rather_than_guessing() {
        // Guessing an index would silently boot the core with nothing loaded.
        let err = plan(
            &c64(),
            Path::new("/media/fat/games/C64/Game.zip"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect_err("must refuse");
        assert!(
            err.to_string().contains("no [[systems.launch]] rule"),
            "got: {err}"
        );
    }

    #[test]
    fn an_invalid_type_is_rejected_because_mister_would_discard_the_item() {
        let bad = rule(&["prg"], "x", 1, 1);
        let err = MglItem::new(&bad, "/games/x.prg").expect_err("must reject");
        assert!(err.to_string().contains("discard"), "got: {err}");
    }

    #[test]
    fn a_relative_game_path_is_rejected() {
        let err = MglItem::new(&rule(&["prg"], "f", 1, 1), "games/x.prg").expect_err("must reject");
        assert!(err.to_string().contains("not absolute"), "got: {err}");
    }

    #[test]
    fn a_self_describing_file_does_not_need_the_systems_core() {
        // A favourite or a core file names its own core, so a missing
        // system core must not block it: only a game that would be wrapped
        // in an MGL naming system.rbf depends on that core being there.
        assert!(!needs_system_core(Path::new("/fav/Game.mra")));
        assert!(!needs_system_core(Path::new("/fav/Game.MRA")));
        assert!(!needs_system_core(Path::new("/fav/Game.mgl")));
        assert!(!needs_system_core(Path::new("/fav/Game.MGL")));
        // An AmigaVision favourite is an .mgl in name only: it carries a
        // title marker and plan() rewrites it into a fresh MGL naming
        // system.rbf, so it depends on that core like any plain game.
        let dir = std::env::temp_dir().join(format!("degauss-marker-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let marker = dir.join("Zool 2.mgl");
        std::fs::write(
            &marker,
            "<mistergamedescription>\n\t<rbf>_Computer/Minimig</rbf>\n\t\
             <degauss kind=\"amigavision\" install=\"/games/Amiga/AV.hdf\" title=\"Zool 2\"/>\n\
             </mistergamedescription>\n",
        )
        .unwrap();
        assert!(needs_system_core(&marker));
        std::fs::remove_dir_all(&dir).ok();
        assert!(!needs_system_core(Path::new("/fav/core.rbf")));
        assert!(!needs_system_core(Path::new("/fav/core.RBF")));
        assert!(needs_system_core(Path::new("/games/Game.neo")));
        assert!(needs_system_core(Path::new("/games/Game.bin")));
        assert!(needs_system_core(Path::new("/games/Game")));

        let system = c64();
        assert!(game_core_selectable(
            &system,
            &crate::browse::Launch::File(PathBuf::from("/games/C64/Game.prg"))
        ));
        for path in ["/fav/Game.mra", "/fav/Game.mgl", "/fav/core.rbf"] {
            assert!(!game_core_selectable(
                &system,
                &crate::browse::Launch::File(PathBuf::from(path))
            ));
        }
        let mut fixed = system.clone();
        fixed
            .launch
            .iter_mut()
            .find(|rule| rule.extensions.iter().any(|extension| extension == "prg"))
            .unwrap()
            .rbf = Some("_Computer/Fixed".into());
        assert!(!game_core_selectable(
            &fixed,
            &crate::browse::Launch::File(PathBuf::from("/games/C64/Game.prg"))
        ));
        assert!(game_core_selectable(
            &system,
            &crate::browse::Launch::AmigaVision {
                install: PathBuf::from("/games/AmigaVision"),
                title: "Zool 2".into(),
            }
        ));
    }

    #[test]
    fn a_core_that_needs_a_reset_gets_one_after_the_file() {
        // Some cores sit there having taken the file until they are reset.
        let mut system = c64();
        system.launch = vec![LaunchRule {
            reset_delay: Some(1),
            reset_hold: Some(2),
            ..rule(&["prg"], "f", 1, 1)
        }];
        let plan = plan(
            &system,
            Path::new("/games/C64/Game.prg"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");

        assert!(
            plan.mgl.contains(r#"<reset delay="1" hold="2"/>"#),
            "got: {}",
            plan.mgl
        );
        let file_at = plan.mgl.find("<file").expect("file element");
        let reset_at = plan.mgl.find("<reset").expect("reset element");
        assert!(reset_at > file_at, "the reset must follow the file");
    }

    #[test]
    fn a_reset_without_a_hold_leaves_it_to_mister() {
        let mut system = c64();
        system.launch = vec![LaunchRule {
            reset_delay: Some(3),
            reset_hold: None,
            companion_extensions: Vec::new(),
            companion_index: None,
            ..rule(&["prg"], "f", 1, 1)
        }];
        let plan = plan(
            &system,
            Path::new("/games/C64/Game.prg"),
            Path::new("/tmp/degauss.mgl"),
        )
        .unwrap();
        assert!(
            plan.mgl.contains(r#"<reset delay="3"/>"#),
            "got: {}",
            plan.mgl
        );
    }

    #[test]
    fn xml_special_characters_in_a_filename_are_escaped() {
        // Real libraries contain ampersands; an unescaped one makes the whole
        // MGL unparseable and the launch silently does nothing.
        let plan = plan(
            &c64(),
            Path::new("/media/fat/games/C64/Rock & Roll.prg"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect("planned");
        assert!(
            plan.mgl.contains("Rock &amp; Roll.prg"),
            "got: {}",
            plan.mgl
        );
        assert!(
            !plan.mgl.contains("Rock & Roll"),
            "raw ampersand must not survive"
        );
    }

    #[test]
    fn more_than_six_actions_is_an_error_not_a_silent_truncation() {
        let item = MglItem::new(&rule(&["prg"], "f", 1, 1), "/games/x.prg").unwrap();
        let items: Vec<MglItem> = std::iter::repeat_n(item, 7).collect();
        let err = build_mgl("_Computer/C64", None, &items).expect_err("must refuse");
        assert!(err.to_string().contains("only the first 6"), "got: {err}");
    }

    #[test]
    fn executing_a_plan_writes_the_mgl_and_the_command() {
        let dir = std::env::temp_dir().join(format!("degauss-launch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mgl_path = dir.join("degauss.mgl");
        let fake_fifo = dir.join("cmd");

        let plan = plan(&c64(), Path::new("/games/C64/A.prg"), &mgl_path).unwrap();
        execute(&plan, &fake_fifo).expect("executed");

        assert_eq!(std::fs::read_to_string(&mgl_path).unwrap(), plan.mgl);
        assert_eq!(
            std::fs::read_to_string(&fake_fifo).unwrap(),
            "load_core ".to_owned() + mgl_path.to_str().unwrap() + "\n"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_missing_command_fifo_reports_the_device_it_could_not_reach() {
        let plan = plan(
            &c64(),
            Path::new("/games/C64/A.prg"),
            &std::env::temp_dir().join("degauss-nofifo.mgl"),
        )
        .unwrap();
        let err = execute(&plan, Path::new("/nonexistent/dir/MiSTer_cmd")).expect_err("must fail");
        assert!(err.to_string().contains("MiSTer_cmd"), "got: {err}");
        std::fs::remove_file(std::env::temp_dir().join("degauss-nofifo.mgl")).ok();
    }

    #[test]
    fn hdmi_scanline_commands_switch_on_and_off_without_changing_launch_commands() {
        let path =
            std::env::temp_dir().join(format!("degauss-scanlines-cmd-{}", std::process::id()));
        set_hdmi_scanlines(true, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fb_scanlines 1\n");
        set_hdmi_scanlines(false, &path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"fb_scanlines 0\n");
        std::fs::remove_file(&path).ok();
        let error = set_hdmi_scanlines(true, Path::new("/nonexistent/dir/MiSTer_cmd"))
            .expect_err("missing Main command FIFO must not appear successful");
        assert!(error.to_string().contains("MiSTer_cmd"));
    }

    #[test]
    fn an_amigavision_favourite_is_a_valid_mgl_that_also_carries_the_title() {
        // Main's MGL parser walks the tags it knows and ignores the rest,
        // so the extra element costs nothing there: the stock menu starts
        // AmigaVision at its own menu and Degauss starts it at the title.
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec![],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            setname: Some("Amiga".into()),
            compatible_cores: Vec::new(),
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        };
        let mgl = favorite_mgl_amiga(
            &system,
            Path::new("/media/fat/games/Amiga"),
            "Zool 2 (AGA)[en]",
        )
        .unwrap();
        assert!(mgl.contains("<rbf>_Computer/Minimig</rbf>"));
        assert!(mgl.contains("<setname>Amiga</setname>"));
        assert!(mgl.starts_with("<mistergamedescription>"));
        assert!(mgl.trim_end().ends_with("</mistergamedescription>"));

        let dir = std::env::temp_dir().join(format!("degauss-amiga-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("fav.mgl");
        std::fs::write(&file, &mgl).unwrap();
        let (install, title) = amiga_marker(&file).expect("the title comes back");
        assert_eq!(install, Path::new("/media/fat/games/Amiga"));
        assert_eq!(title, "Zool 2 (AGA)[en]");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_title_with_characters_xml_cares_about_survives_the_round_trip() {
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec![],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            setname: Some("Amiga".into()),
            compatible_cores: Vec::new(),
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        };
        let awkward = "Ghosts 'n Goblins & \"Friends\" <OCS>";
        let mgl =
            favorite_mgl_amiga(&system, Path::new("/media/fat/games/Amiga"), awkward).unwrap();
        let dir = std::env::temp_dir().join(format!("degauss-amiga2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("fav.mgl");
        std::fs::write(&file, &mgl).unwrap();
        let (_, title) = amiga_marker(&file).expect("the title comes back");
        assert_eq!(
            title, awkward,
            "escaped on the way out, put back on the way in"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn launching_an_amigavision_favourite_writes_the_boot_file() {
        // The whole point of the extra element: choosing the favourite has
        // to do what choosing the title in its own folder does, which is
        // write the name where AmigaVision reads it before the core comes
        // up. Without this the favourite would start AmigaVision's menu.
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["mgl".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            setname: Some("Amiga".into()),
            compatible_cores: Vec::new(),
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        };
        let install = Path::new("/media/fat/games/Amiga");
        let mgl = favorite_mgl_amiga(&system, install, "Zool 2 (AGA)[en]").unwrap();

        let dir = std::env::temp_dir().join(format!("degauss-favlaunch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let favourite = dir.join("Zool 2 (AGA)[en].mgl");
        std::fs::write(&favourite, &mgl).unwrap();

        let plan = plan(&system, &favourite, Path::new("/tmp/degauss.mgl")).unwrap();
        let (path, bytes) = plan.boot_file.expect("the title has to be written down");
        assert_eq!(path, install.join("shared").join("ags_boot"));
        assert_eq!(bytes, b"Zool 2 (AGA)[en]\n".to_vec());
        // And the core it starts is AmigaVision, not the favourite file.
        assert!(plan.mgl.contains("<setname>Amiga</setname>"));
        assert!(!plan.mgl.contains("<file "), "there is no file to mount");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn neogeo() -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Neo Geo".into(),
            path: "/media/fat/games/NEOGEO".into(),
            extensions: vec!["neo".into(), "mgl".into()],
            rbf: "_Console/NeoGeo".into(),
            launch: vec![rule(&["neo", "mgl"], "f", 1, 1)],
            skip_folders: Vec::new(),
            setname: None,
            compatible_cores: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    #[test]
    fn a_neo_geo_set_launches_through_the_neo_slot_with_its_complete_path() {
        // Main routes any <file> for the Neo Geo core into its ROM-set
        // loader, which takes a .neo, a set ZIP or a set folder by path.
        // So a set is started exactly as a .neo is, with the whole ZIP or
        // folder named: nothing is unpacked, and Main validates the
        // components itself once it has the path.
        for set in ["mslug.zip", "kof98"] {
            let game = Path::new("/media/fat/games/NEOGEO").join(set);
            let plan = plan(&neogeo(), &game, Path::new("/tmp/degauss.mgl")).expect("planned");
            assert_eq!(
                plan.mgl,
                format!(
                    "<mistergamedescription>\n\
                     \t<rbf>_Console/NeoGeo</rbf>\n\
                     \t<file delay=\"1\" type=\"f\" index=\"1\" path=\"../../../../../media/fat/games/NEOGEO/{set}\"/>\n\
                     </mistergamedescription>\n"
                )
            );
            assert_eq!(plan.command, "load_core /tmp/degauss.mgl\n");
        }
        // The same ZIP in a system whose core is not the Neo Geo one is
        // still refused rather than guessed at.
        let err = plan(
            &c64(),
            Path::new("/media/fat/games/C64/mslug.zip"),
            Path::new("/tmp/degauss.mgl"),
        )
        .expect_err("must refuse");
        assert!(
            err.to_string().contains("no [[systems.launch]] rule"),
            "got: {err}"
        );
        // The name is not looked at, exactly as Main routes whatever fills
        // the Neo Geo file slot to its ROM-set loader: a set folder with a
        // dot in its name is a set, and so is anything else no rule covers.
        let system = neogeo();
        for set in ["mslug", "v1.2", "notes.txt"] {
            let rule = rule_for(&system, &Path::new("/media/fat/games/NEOGEO").join(set))
                .unwrap_or_else(|error| panic!("{set}: {error}"));
            assert_eq!((rule.index, rule.kind.as_str()), (1, "f"), "{set}");
        }
        // A rule the system declares is never displaced by the borrowed
        // one: a user's table may route another extension to a second
        // slot, and that file must still go where the table says.
        let mut two_slots = neogeo();
        two_slots.launch.push(rule(&["bin"], "f", 2, 1));
        let declared = rule_for(&two_slots, Path::new("/media/fat/games/NEOGEO/x.bin")).unwrap();
        assert_eq!(declared.index, 2);
        assert_eq!(
            rule_for(&two_slots, Path::new("/media/fat/games/NEOGEO/mslug.zip"))
                .unwrap()
                .index,
            1
        );
    }

    #[test]
    fn a_set_favourite_is_an_ordinary_mgl_with_the_absolute_set_path() {
        // What MiSTer's own favourites script would write for a set: an
        // MGL naming the core and the complete path. Read back, it points
        // at the set, so the heart shows on the right row after a restart
        // and choosing the favourite starts the set.
        let dir = std::env::temp_dir().join(format!("degauss-neogeo-fav-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("games/NEOGEO/kof98")).unwrap();
        std::fs::create_dir_all(dir.join("_Console")).unwrap();
        std::fs::write(dir.join("_Console/NeoGeo_20250101.rbf"), b"core").unwrap();
        std::fs::write(dir.join("games/NEOGEO/mslug.zip"), b"zip").unwrap();
        let mut system = neogeo();
        system.path = dir.join("games/NEOGEO").to_string_lossy().into_owned();
        let def = crate::systems::parse_table(
            include_str!("../assets/systems.toml"),
            Path::new("systems.toml"),
        )
        .unwrap()
        .into_iter()
        .find(|candidate| candidate.id == "NeoGeo")
        .expect("the shipped table has Neo Geo");
        let found = crate::systems::FoundSystem {
            def,
            paths: vec![dir.join("games/NEOGEO")],
            logo_dir: None,
            menu_folder: None,
        };
        let homes = crate::mgl::Homes::new(&[], &[found]);

        for set in ["mslug.zip", "kof98"] {
            let game = dir.join("games/NEOGEO").join(set);
            let mgl = favorite_mgl(&system, &game)
                .expect("built")
                .expect("a set is described, not linked");
            assert_eq!(
                mgl,
                format!(
                    "<mistergamedescription>\n\
                     \t<rbf>_Console/NeoGeo</rbf>\n\
                     \t<file delay=\"1\" type=\"f\" index=\"1\" path=\"{}\"/>\n\
                     </mistergamedescription>\n",
                    game.display()
                )
            );
            let favourite = dir.join(format!("{set}.mgl"));
            std::fs::write(&favourite, &mgl).unwrap();
            assert_eq!(
                crate::favorites::reference_of(&favourite, &homes)
                    .map(|reference| reference.cache_target),
                Some(game.clone())
            );
            let favorites = crate::favorites::Favorites::read_with(&dir, &homes);
            assert!(favorites.holds(&game), "{set} is held after a fresh read");
            // Chosen from the shelf, the favourite is passed through as
            // the ordinary MGL it is.
            let plan = plan_with_preference(
                &system,
                &favourite,
                Path::new("/tmp/degauss.mgl"),
                &dir,
                false,
            )
            .expect("planned");
            assert!(plan.mgl.is_empty());
            assert_eq!(plan.command, format!("load_core {}\n", favourite.display()));
            std::fs::remove_file(&favourite).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_explicit_cached_core_is_revalidated_at_the_launch_boundary() {
        let root = std::env::temp_dir().join(format!("degauss-direct-core-{}", std::process::id()));
        let menu = root.join("menu");
        std::fs::create_dir_all(menu.join("_Console")).unwrap();
        let standard = menu.join("_Console/NES_20260916.rbf");
        let ra = menu.join("_Console/RA_NES.mgl");
        let arcade = menu.join("_Arcade/Example.mra");
        let text = menu.join("_Console/readme.txt");
        std::fs::create_dir_all(arcade.parent().unwrap()).unwrap();
        std::fs::write(&standard, b"core").unwrap();
        std::fs::write(&ra, b"<mistergamedescription/>").unwrap();
        std::fs::write(&arcade, b"<misterromdescription/>").unwrap();
        std::fs::write(&text, b"not a core").unwrap();
        std::fs::write(root.join("outside.rbf"), b"outside").unwrap();

        assert_eq!(
            plan_core(&standard, &menu).unwrap().command,
            format!("load_core {}\n", standard.canonicalize().unwrap().display())
        );
        assert_eq!(
            plan_core(&ra, &menu).unwrap().command,
            format!("load_core {}\n", ra.canonicalize().unwrap().display())
        );
        assert_eq!(
            plan_misterzine(&arcade, &menu).unwrap().command,
            format!("load_core {}\n", arcade.canonicalize().unwrap().display())
        );
        assert_eq!(
            plan_misterzine(&ra, &menu).unwrap().command,
            format!("load_core {}\n", ra.canonicalize().unwrap().display())
        );
        assert!(plan_core(&arcade, &menu)
            .unwrap_err()
            .to_string()
            .contains("not an RBF or MGL"));
        assert!(plan_core(&text, &menu)
            .unwrap_err()
            .to_string()
            .contains("not an RBF or MGL"));
        assert!(plan_core(&root.join("outside.rbf"), &menu)
            .unwrap_err()
            .to_string()
            .contains("outside the configured MiSTer menu"));
        assert!(plan_core(&menu.join("../outside.rbf"), &menu)
            .unwrap_err()
            .to_string()
            .contains("outside the configured MiSTer menu"));
        #[cfg(unix)]
        {
            let linked = menu.join("_Console/Linked.rbf");
            std::os::unix::fs::symlink(root.join("outside.rbf"), &linked).unwrap();
            assert!(plan_misterzine(&linked, &menu)
                .unwrap_err()
                .to_string()
                .contains("outside the configured MiSTer menu"));
        }
        std::fs::remove_file(&standard).unwrap();
        assert!(plan_core(&standard, &menu)
            .unwrap_err()
            .to_string()
            .contains("no longer installed"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn self_describing_launches_check_their_own_versioned_core_before_handoff() {
        let root = std::env::temp_dir().join(format!(
            "degauss-descriptor-core-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let menu = root.join("menu");
        let arcade = menu.join("_Arcade/Test.mra");
        let favorite = menu.join("_@Favorites/Other.mgl");
        let core_launcher = menu.join("_Console/Other.mgl");
        for path in [&arcade, &favorite, &core_launcher] {
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        }
        std::fs::write(
            &arcade,
            "<misterromdescription><rbf>Test</rbf></misterromdescription>",
        )
        .unwrap();
        for path in [&favorite, &core_launcher] {
            std::fs::write(
                path,
                "<mistergamedescription><rbf>_Console/Other</rbf></mistergamedescription>",
            )
            .unwrap();
        }
        let temporary_mgl = root.join("degauss.mgl");
        let missing_arcade =
            plan_with_preference(&c64(), &arcade, &temporary_mgl, &menu, false).unwrap_err();
        assert!(missing_arcade.to_string().contains("requires Test"));
        let missing_favorite =
            plan_with_preference(&c64(), &favorite, &temporary_mgl, &menu, false).unwrap_err();
        assert!(missing_favorite.to_string().contains("_Console/Other"));
        let missing_core = plan_core(&core_launcher, &menu).unwrap_err();
        assert!(missing_core.to_string().contains("_Console/Other"));

        std::fs::create_dir_all(menu.join("_Arcade/cores")).unwrap();
        std::fs::write(menu.join("_Arcade/cores/Arcade-Test_20260924.rbf"), b"core").unwrap();
        std::fs::write(menu.join("_Console/Other_20260924.rbf"), b"core").unwrap();
        for path in [&arcade, &favorite] {
            let planned = plan_with_preference(&c64(), path, &temporary_mgl, &menu, false).unwrap();
            assert_eq!(planned.command, format!("load_core {}\n", path.display()));
        }
        std::fs::remove_file(menu.join("_Arcade/cores/Arcade-Test_20260924.rbf")).unwrap();
        std::fs::write(menu.join("_Arcade/cores/Test_20260924.rbf"), b"core").unwrap();
        assert!(plan_with_preference(&c64(), &arcade, &temporary_mgl, &menu, false).is_ok());

        let usb_arcade = root.join("usb/_Arcade/Test.mra");
        std::fs::create_dir_all(usb_arcade.parent().unwrap()).unwrap();
        std::fs::copy(&arcade, &usb_arcade).unwrap();
        assert!(plan_with_preference(&c64(), &usb_arcade, &temporary_mgl, &menu, false).is_err());
        std::fs::create_dir_all(root.join("usb/_Arcade/cores")).unwrap();
        std::fs::write(root.join("usb/_Arcade/cores/Test_20260924.rbf"), b"core").unwrap();
        assert!(plan_with_preference(&c64(), &usb_arcade, &temporary_mgl, &menu, false).is_ok());
        assert_eq!(
            plan_core(&core_launcher, &menu).unwrap().command,
            format!(
                "load_core {}\n",
                core_launcher.canonicalize().unwrap().display()
            )
        );
        let ordinary = plan(
            &c64(),
            Path::new("/media/fat/games/C64/Game.prg"),
            &temporary_mgl,
        )
        .unwrap();
        assert!(ordinary.mgl.contains("Game.prg"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
