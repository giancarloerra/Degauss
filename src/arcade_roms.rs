//! Archive presence only, not ROM contents or checksums. Work is done during
//! Arcade indexing, never while paging through a list.
use std::collections::{BTreeSet, HashMap};
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::browse::{Kind, Launch, Place, Row};
use crate::error::{DegaussError, Result};

pub const FILE: &str = "arcade-missing-roms.bin";
pub type Missing = BTreeSet<PathBuf>;

pub fn read(dir: &Path) -> Result<Option<Missing>> {
    let path = dir.join(FILE);
    match std::fs::read(&path) {
        Ok(bytes) => postcard::from_bytes(&bytes)
            .map(Some)
            .map_err(|e| DegaussError::malformed("Arcade ROM check", &path, e.to_string())),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
        Err(e) => Err(DegaussError::io("reading the Arcade ROM check", &path, e)),
    }
}

pub fn filter(
    dir: &Path,
    place: &Place,
    config: &crate::config::SystemConfig,
    rows: &mut Vec<Row>,
) -> Result<()> {
    let Place::Dir(path) = place else {
        return Ok(());
    };
    if !path
        .file_name()
        .is_some_and(|n| n.eq_ignore_ascii_case("_Arcade"))
        || !std::iter::once(&config.path)
            .chain(&config.extra_paths)
            .any(|root| Path::new(root) == path)
    {
        return Ok(());
    }
    let missing = read(dir)?.ok_or_else(|| {
        DegaussError::unsupported(
            "Arcade ROM check",
            "Rebuild the Arcade system list to check its ROM ZIPs.",
        )
    })?;
    rows.retain(
        |row| !matches!(&row.kind, Kind::Play(Launch::File(path)) if missing.contains(path)),
    );
    Ok(())
}

// This is an absence check, not a launchability test. If several possible
// Main storage roots contain a mame directory, a ZIP on any of them is not
// missing locally. Do not guess which root the currently running Main owns.
pub struct Lookup {
    roots: std::io::Result<Vec<PathBuf>>,
    probes: HashMap<PathBuf, bool>,
}

impl Lookup {
    pub fn new(card: &Path) -> Self {
        let media = card.parent().unwrap_or(card);
        let stores = (0..6)
            .map(|n| media.join(format!("usb{n}")))
            .chain([media.join("network"), card.join("cifs"), card.to_owned()])
            .chain((0..4).map(|n| media.join(format!("usb{n}/cifs"))));
        let roots = (|| {
            let mut roots = Vec::new();
            for store in stores {
                for base in [store.clone(), store.join("games")] {
                    match std::fs::metadata(base.join("mame")) {
                        Ok(m) if m.is_dir() => {
                            // With "mame" directly on the native card,
                            // Main can construct "/mame" (no parent).
                            if base == card {
                                roots.push(PathBuf::from("/"));
                            }
                            roots.push(base);
                        }
                        Ok(_) => {}
                        Err(e)
                            if e.kind() == ErrorKind::NotFound
                                || e.kind() == ErrorKind::NotADirectory => {}
                        Err(e) => return Err(e),
                    }
                }
            }
            Ok(roots)
        })();
        if let Err(e) = &roots {
            crate::note(&format!(
                "Arcade ROM check: cannot determine MiSTer ROM storage: {e}"
            ));
        }
        Self {
            roots,
            probes: HashMap::new(),
        }
    }

    fn absent(&mut self, arcade: &Path, name: &str) -> bool {
        // Keep unusual references visible rather than guessing. Main prepends
        // its games base to a leading slash, and "mame/" to a relative name.
        let relative = Path::new(name.strip_prefix('/').unwrap_or(name));
        if !relative
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("zip"))
            || !relative
                .components()
                .all(|c| matches!(c, Component::Normal(_)))
        {
            return false;
        }
        let Ok(roots) = &self.roots else {
            return false;
        };
        // set_arcade_root also uses the first /_... directory in the MRA
        // path when no MAME directory is selected, including nested menus.
        let Some(arcade_root) = arcade
            .ancestors()
            .filter(|p| {
                p.file_name()
                    .is_some_and(|n| n.as_encoded_bytes().starts_with(b"_"))
            })
            .last()
        else {
            return false;
        };
        roots
            .iter()
            .map(PathBuf::as_path)
            .chain([arcade_root])
            .all(|root| {
                let path = if name.starts_with('/') {
                    root.join(relative)
                } else {
                    root.join("mame").join(relative)
                };
                *self
                    .probes
                    .entry(path)
                    .or_insert_with_key(|path| match std::fs::metadata(path) {
                        Ok(_) => false,
                        Err(e)
                            if e.kind() == ErrorKind::NotFound
                                || e.kind() == ErrorKind::NotADirectory =>
                        {
                            true
                        }
                        Err(e) => {
                            crate::note(&format!("Arcade ROM check: {}: {e}", path.display()));
                            false
                        }
                    })
            })
    }

    pub fn missing(&mut self, path: &Path, text: &str) -> bool {
        let Ok(doc) = roxmltree::Document::parse(text) else {
            return false;
        };
        let root = doc.root_element();
        if !named(root, "misterromdescription") {
            return false;
        }
        let mut saw_game_rom = false;
        for rom in root.children().filter(|n| named(*n, "rom")) {
            match attr(rom, "index").unwrap_or("0").parse::<u32>() {
                Ok(0) => {}
                Ok(_) => continue, // auxiliary data does not prove the game ROM is missing
                Err(_) => return false,
            }
            saw_game_rom = true;
            let default_zip = attr(rom, "zip").unwrap_or("");
            let missing_part = rom.descendants().filter(|n| named(*n, "part")).any(|part| {
                if attr(part, "name").is_none_or(str::is_empty) {
                    return false;
                } // inline bytes
                let archives = attr(part, "zip")
                    .filter(|s| !s.is_empty())
                    .unwrap_or(default_zip);
                archives.split('|').all(|name| self.absent(path, name))
            });
            if !missing_part {
                return false;
            } // another index-zero layout can supply the ROM
        }
        saw_game_rom
    }
}

fn named(node: roxmltree::Node<'_, '_>, name: &str) -> bool {
    node.is_element()
        && node.tag_name().namespace().is_none()
        && node.tag_name().name().eq_ignore_ascii_case(name)
}

fn attr<'a>(node: roxmltree::Node<'a, '_>, name: &str) -> Option<&'a str> {
    node.attributes()
        .find(|a| a.namespace().is_none() && a.name().eq_ignore_ascii_case(name))
        .map(|a| a.value())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_proven_missing_game_archives_are_hidden() {
        let base = std::env::temp_dir().join(format!("degauss-arcade-roms-{}", std::process::id()));
        let card = base.join("fat");
        let mra = card.join("_Arcade/Example.mra");
        let disk = base.join("usb1/games/mame");
        std::fs::create_dir_all(&disk).unwrap();
        std::fs::write(disk.join("parent.zip"), b"presence, not contents").unwrap();
        let hb = base.join("usb1/games/hbmame");
        std::fs::create_dir_all(&hb).unwrap();
        std::fs::write(hb.join("present.zip"), b"").unwrap();
        // Archives may be split between card, USB and the Arcade fallback.
        for zip in [
            card.join("games/mame/second.zip"),
            card.join("_Arcade/mame/fallback.zip"),
        ] {
            std::fs::create_dir_all(zip.parent().unwrap()).unwrap();
            std::fs::write(zip, b"").unwrap();
        }
        let mut lookup = Lookup::new(&card);
        let missing = |lookup: &mut Lookup, roms: &str| {
            lookup.missing(
                &mra,
                &format!("<misterromdescription>{roms}</misterromdescription>"),
            )
        };
        let absent = r#"<rom index="0" zip="4dwarrio.zip"><part name="4d.116"/></rom>"#;
        assert!(missing(&mut lookup, absent));
        for rom in [
            r#"<rom index="0" zip="child.zip|parent.zip"><interleave><part name="game.bin"/></interleave></rom>"#,
            r#"<ROM ZIP="absent.zip"><PART ZIP="/hbmame/present.zip" NAME="rom.bin"/></ROM>"#,
            r#"<rom index="0" zip="absent.zip"><part>aa bb</part></rom>"#,
            r#"<rom index="1" zip="absent.zip"><part name="aux.bin"/></rom>"#,
            r#"<rom zip="../unknown.zip"><part name="rom.bin"/></rom>"#,
            r#"<rom zip="second.zip"><part name="rom.bin"/></rom>"#,
            r#"<rom zip="fallback.zip"><part name="rom.bin"/></rom>"#,
            r#"<rom xmlns:meta="urn:metadata" meta:zip="absent.zip"><part name="rom.bin"/></rom>"#,
            r#"<rom index="0" zip="absent.zip"><part name="rom.bin"></rom>"#,
            "",
        ] {
            assert!(!missing(&mut lookup, rom), "{rom}");
        }
        assert!(!missing(
            &mut lookup,
            &format!(r#"{absent}<rom index="0" zip="parent.zip"><part name="game.bin"/></rom>"#)
        ));
        assert!(missing(
            &mut lookup,
            r#"<rom zip="parent.zip"><part name="a"/><part zip="missing.zip" name="b"/></rom>"#
        ));
        assert!(!missing(&mut lookup, &format!("{absent}<broken>")));
        assert!(!lookup.missing(&mra, &format!("<unknown>{absent}</unknown>")));
        #[cfg(unix)]
        {
            // A storage error is not proof that a ZIP is missing.
            let loop_zip = disk.join("unknown.zip");
            std::os::unix::fs::symlink(&loop_zip, &loop_zip).unwrap();
            assert!(!missing(
                &mut lookup,
                r#"<rom zip="unknown.zip"><part name="rom.bin"/></rom>"#
            ));
            let loop_mount = base.join("usb0");
            std::os::unix::fs::symlink(&loop_mount, &loop_mount).unwrap();
            assert!(!missing(&mut Lookup::new(&card), absent));
        }
        std::fs::remove_dir_all(&base).unwrap();
    }
}
