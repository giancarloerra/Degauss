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

use std::path::{Path, PathBuf};

use crate::config::{LaunchRule, SystemConfig};
use crate::error::{DegaussError, Result};

/// Where Main listens for commands.
pub const CMD_FIFO: &str = "/dev/MiSTer_cmd";

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
            latin1(&format!("{title}\n"))?,
        )),
    })
}

/// A second file to mount beside this one, if the rule asks for one and the
/// folder holds it.
///
/// The first match in the folder wins, which is what MiSTer's own tooling
/// does. A game with two CDs needs its own shortcut either way.
fn companion_for(rule: &LaunchRule, game: &Path) -> Option<PathBuf> {
    if rule.companion_extensions.is_empty() {
        return None;
    }
    let dir = game.parent()?;
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|item| item.path())
        .filter(|path| path != game)
        .filter(|path| {
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.to_ascii_lowercase())
                .is_some_and(|ext| rule.companion_extensions.contains(&ext))
        })
        .collect();
    // Sorted so the same folder always produces the same MGL, rather than
    // whatever order the filesystem happened to return.
    found.sort();
    found.into_iter().next()
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

/// The AmigaVision title an MGL carries, where it carries one.
pub fn amiga_marker(mgl: &Path) -> Option<(PathBuf, String)> {
    let text = std::fs::read_to_string(mgl).ok()?;
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
    let rule = system.rule_for(game).ok_or_else(|| {
        DegaussError::unsupported(
            "launch rule",
            format!(
                "no [[systems.launch]] rule covers {:?} in system {}",
                game.extension().and_then(|e| e.to_str()).unwrap_or(""),
                system.name
            ),
        )
    })?;
    let absolute = game.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "game path",
            format!("{} is not valid UTF-8", game.display()),
        )
    })?;
    let mut items = Vec::new();
    if let Some(companion) = companion_for(rule, game) {
        if let Some(path) = companion.to_str() {
            let mut extra = MglItem::new(rule, path)?;
            extra.index = rule.companion_index.unwrap_or(rule.index);
            extra.reset = None;
            items.push(extra);
        }
    }
    items.push(MglItem::new(rule, absolute)?);
    build_mgl_with(&system.rbf, system.setname.as_deref(), &items, "").map(Some)
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

    let rule = system.rule_for(game).ok_or_else(|| {
        DegaussError::unsupported(
            "launch rule",
            format!(
                "no [[systems.launch]] rule covers {:?} in system {}",
                game.extension().and_then(|e| e.to_str()).unwrap_or(""),
                system.name
            ),
        )
    })?;

    let absolute = game.to_str().ok_or_else(|| {
        DegaussError::unsupported(
            "game path",
            format!("{} is not valid UTF-8", game.display()),
        )
    })?;
    let mut items = Vec::new();
    // The companion is mounted first, so the disk the game boots from is
    // the last thing handed over.
    if let Some(companion) = companion_for(rule, game) {
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
    let mgl = build_mgl(&system.rbf, system.setname.as_deref(), &items)?;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::LaunchRule;

    fn rule(exts: &[&str], kind: &str, index: u8, delay: u8) -> LaunchRule {
        LaunchRule {
            extensions: exts.iter().map(|s| s.to_string()).collect(),
            kind: kind.to_string(),
            index,
            delay,
            reset_delay: None,
            reset_hold: None,
            companion_extensions: Vec::new(),
            companion_index: None,
        }
    }

    fn c64() -> SystemConfig {
        SystemConfig {
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
            extra_paths: Vec::new(),
        }
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
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
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
    fn a_title_that_is_not_latin1_is_refused_rather_than_mangled() {
        let system = SystemConfig {
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
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
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
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
            name: "Amiga".into(),
            path: dir.to_string_lossy().into_owned(),
            extensions: vec!["adf".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            skip_folders: Vec::new(),
            setname: None,
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
    fn an_amigavision_favourite_is_a_valid_mgl_that_also_carries_the_title() {
        // Main's MGL parser walks the tags it knows and ignores the rest,
        // so the extra element costs nothing there: the stock menu starts
        // AmigaVision at its own menu and Degauss starts it at the title.
        let system = SystemConfig {
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec![],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            setname: Some("Amiga".into()),
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
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec![],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            setname: Some("Amiga".into()),
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
            name: "Amiga".into(),
            path: "/media/fat/games/Amiga".into(),
            extensions: vec!["mgl".into()],
            rbf: "_Computer/Minimig".into(),
            launch: Vec::new(),
            setname: Some("Amiga".into()),
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
}
