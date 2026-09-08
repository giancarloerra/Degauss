//! Runtime core selection. Installed launchers supply RA's set name and attributes;
//! no variant information is persisted in systems tables or catalog caches.
use crate::{
    config::SystemConfig,
    error::{DegaussError, Result},
};
use quick_xml::{events::Event, Reader};
use std::ops::Range;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct EffectiveCore {
    pub rbf: String,
    /// Complete XML element, including the installer's same_dir attribute.
    pub setname_xml: Option<String>,
}

#[derive(Debug)]
struct Element {
    range: Range<usize>,
    inner: Range<usize>,
    value: String,
}

fn elements(text: &str, path: &Path) -> Result<(Element, Option<Element>)> {
    let mut reader = Reader::from_str(text);
    let mut depth = 0usize;
    let mut root_seen = false;
    let mut active: Option<(bool, usize, usize)> = None;
    let mut rbf = None;
    let mut setname = None;
    let bad = |message: &str| DegaussError::malformed("core launcher", path, message.to_string());
    loop {
        let start = reader.buffer_position() as usize;
        match reader.read_event().map_err(|e| bad(&e.to_string()))? {
            Event::Start(e) => {
                if depth == 0 {
                    if root_seen
                        || !e
                            .name()
                            .as_ref()
                            .eq_ignore_ascii_case("mistergamedescription")
                    {
                        return Err(bad("expected a single mistergamedescription root"));
                    }
                    root_seen = true;
                }
                if active.is_some() {
                    return Err(bad("nested core or setname element"));
                }
                if e.name().as_ref().eq_ignore_ascii_case("rbf")
                    || e.name().as_ref().eq_ignore_ascii_case("setname")
                {
                    if depth != 1 {
                        return Err(bad("core and setname must be root children"));
                    }
                    for a in e.attributes() {
                        a.map_err(|e| bad(&e.to_string()))?;
                    }
                    active = Some((
                        e.name().as_ref().eq_ignore_ascii_case("rbf"),
                        start,
                        reader.buffer_position() as usize,
                    ));
                }
                depth += 1;
            }
            Event::End(_) => {
                depth = depth
                    .checked_sub(1)
                    .ok_or_else(|| bad("unexpected closing element"))?;
                if let Some((is_rbf, begin, inner)) = active.take() {
                    let value = quick_xml::escape::unescape(text[inner..start].trim())
                        .map_err(|e| bad(&e.to_string()))?
                        .into_owned();
                    if value.is_empty() {
                        return Err(bad("empty core or setname"));
                    }
                    let slot = if is_rbf { &mut rbf } else { &mut setname };
                    if slot.is_some() {
                        return Err(bad("duplicate core or setname"));
                    }
                    *slot = Some(Element {
                        range: begin..reader.buffer_position() as usize,
                        inner: inner..start,
                        value,
                    });
                }
            }
            Event::Empty(e)
                if e.name().as_ref().eq_ignore_ascii_case("rbf")
                    || e.name().as_ref().eq_ignore_ascii_case("setname") =>
            {
                return Err(bad("empty core or setname"))
            }
            Event::Empty(_) if active.is_some() => {
                return Err(bad("nested core or setname element"))
            }
            // Main accepts and ignores a DOCTYPE declaration. quick_xml does
            // not fetch external declarations; retaining it preserves native
            // custom MGLs without adding entity expansion or network access.
            Event::DocType(_) => {}
            Event::Eof => break,
            _ => {}
        }
    }
    if depth != 0 {
        return Err(bad("unclosed XML element"));
    }
    Ok((rbf.ok_or_else(|| bad("missing rbf element"))?, setname))
}

fn read_launcher(path: &Path) -> Result<String> {
    crate::favorites::read_mgl_text(path, "core launcher")
}

fn undated_name(stem: &str) -> &str {
    match stem.rsplit_once('_') {
        Some((before, date))
            if date.len() >= 8 && date.as_bytes()[..8].iter().all(u8::is_ascii_digit) =>
        {
            before
        }
        _ => stem,
    }
}

/// Preserve the released core-presence comparison, including dated references
/// and punctuation aliases. The original configured reference is still emitted.
pub(crate) fn same_core_identity(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        undated_name(value)
            .chars()
            .filter(|c| c.is_ascii_alphanumeric())
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

pub(crate) fn core_present(root: &Path, rbf: &str) -> Result<bool> {
    let reference = Path::new(rbf);
    if reference
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
    {
        return Err(DegaussError::unsupported("MGL core reference", "MiSTer Main requires an extensionless RBF reference; remove the trailing .rbf from the configured core"));
    }
    let folder = root.join(reference.parent().unwrap_or(Path::new("")));
    let wanted = reference.file_name().and_then(|s| s.to_str()).unwrap_or("");
    let listing = match std::fs::read_dir(&folder) {
        Ok(listing) => listing,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(DegaussError::io("core directory", &folder, e)),
    };
    for entry in listing {
        let entry = entry.map_err(|e| DegaussError::io("core directory", &folder, e))?;
        let path = entry.path();
        if !path
            .extension()
            .is_some_and(|s| s.eq_ignore_ascii_case("rbf"))
        {
            continue;
        }
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        if same_core_identity(stem, wanted) {
            let metadata =
                std::fs::metadata(&path).map_err(|e| DegaussError::io("core file", &path, e))?;
            if metadata.is_file() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

pub(crate) fn standard(system: &SystemConfig) -> EffectiveCore {
    EffectiveCore {
        rbf: system.rbf.clone(),
        setname_xml: system
            .setname
            .as_ref()
            .filter(|s| !s.is_empty())
            .map(|s| format!("<setname>{}</setname>", quick_xml::escape::escape(s))),
    }
}

/// Resolve the installer launcher for the configured RBF. Shared systems keep
/// their existing file index and extension rules, which select the hardware
/// mode. RA's attributed setname selects the Main profile and retains the
/// original core's game directory; it must not be synthesized from a system ID.
pub fn resolve(system: &SystemConfig, root: &Path, ra_first: bool) -> Result<EffectiveCore> {
    if !ra_first && core_present(root, &system.rbf)? {
        return Ok(standard(system));
    }
    if let Some(core) = installed_ra(system, root)? {
        return Ok(core);
    }
    if ra_first && core_present(root, &system.rbf)? {
        return Ok(standard(system));
    }
    Err(DegaussError::unsupported(
        "core",
        format!(
            "neither standard nor RetroAchievements core is installed for {}",
            system.name
        ),
    ))
}

pub(crate) fn installed_ra(system: &SystemConfig, root: &Path) -> Result<Option<EffectiveCore>> {
    let basename = Path::new(&system.rbf)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let basename = undated_name(basename);
    let identity = basename;
    let folder = root.join("_RA_Cores");
    let mut matches = Vec::new();
    match std::fs::read_dir(&folder) {
        Ok(listing) => {
            for entry in listing {
                let entry =
                    entry.map_err(|e| DegaussError::io("RA launcher directory", &folder, e))?;
                let path = entry.path();
                if path
                    .extension()
                    .is_some_and(|s| s.eq_ignore_ascii_case("mgl"))
                    && path.file_stem().and_then(|s| s.to_str()).is_some_and(|s| {
                        s.eq_ignore_ascii_case(identity)
                            || s.eq_ignore_ascii_case(&format!("RA_{identity}"))
                    })
                {
                    matches.push(path);
                }
            }
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(DegaussError::io("RA launcher directory", &folder, e)),
    }
    if matches.len() > 1 {
        return Err(DegaussError::unsupported(
            "RA core",
            format!("multiple launchers match {identity}"),
        ));
    }
    if let Some(path) = matches.first() {
        let text = read_launcher(path)?;
        let (rbf, setname) = elements(&text, path)?;
        let expected_rbf = format!("_RA_Cores/Cores/{basename}");
        let set = setname
            .ok_or_else(|| DegaussError::malformed("RA launcher", path, "missing setname"))?;
        if !rbf.value.eq_ignore_ascii_case(&expected_rbf)
            || !set.value.eq_ignore_ascii_case(&format!("RA_{identity}"))
        {
            return Err(DegaussError::malformed(
                "RA launcher",
                path,
                format!("expected core {expected_rbf} and setname RA_{identity}"),
            ));
        }
        if !core_present(root, &rbf.value)? {
            return Err(DegaussError::unsupported(
                "RA core",
                format!(
                    "launcher {} names missing core {}",
                    path.display(),
                    rbf.value
                ),
            ));
        }
        return Ok(Some(EffectiveCore {
            rbf: rbf.value,
            setname_xml: Some(text[set.range].to_owned()),
        }));
    }
    if core_present(root, &format!("_RA_Cores/Cores/{basename}"))? {
        return Err(DegaussError::unsupported(
            "RA launcher",
            format!(
                "RA core {basename} is installed but its launcher is missing from {}",
                folder.display()
            ),
        ));
    }
    Ok(None)
}

fn preserve_setname_attributes(old: &str, new: &str, path: &Path) -> Result<String> {
    let bad = |e: String| DegaussError::malformed("core launcher", path, e);
    let mut new_reader = Reader::from_str(new);
    let Event::Start(new_tag) = new_reader.read_event().map_err(|e| bad(e.to_string()))? else {
        return Err(bad("invalid replacement setname".into()));
    };
    let keys: Vec<_> = new_tag
        .attributes()
        .map(|a| a.map(|a| a.key.as_ref().to_ascii_lowercase()))
        .collect::<std::result::Result<_, _>>()
        .map_err(|e| bad(e.to_string()))?;
    let mut old_reader = Reader::from_str(old);
    let Event::Start(old_tag) = old_reader.read_event().map_err(|e| bad(e.to_string()))? else {
        return Err(bad("invalid original setname".into()));
    };
    let mut additional = String::new();
    for attribute in old_tag.attributes() {
        let attribute = attribute.map_err(|e| bad(e.to_string()))?;
        let key = attribute.key.as_ref();
        if key.eq_ignore_ascii_case("same_dir") || keys.iter().any(|k| k.eq_ignore_ascii_case(key))
        {
            continue;
        }
        let start = key.as_ptr() as usize - old.as_ptr() as usize;
        let end =
            attribute.value.as_ptr() as usize - old.as_ptr() as usize + attribute.value.len() + 1;
        additional.push(' ');
        additional.push_str(&old[start..end]);
    }
    let mut result = new.to_owned();
    result.insert_str(1 + new_tag.name().as_ref().len(), &additional);
    Ok(result)
}

/// Replace only core/setname elements; all unrelated XML remains byte-for-byte.
pub fn apply(text: &str, path: &Path, core: &EffectiveCore) -> Result<String> {
    let (rbf, setname) = elements(text, path)?;
    let mut patches = vec![(
        rbf.inner.clone(),
        quick_xml::escape::escape(&core.rbf).into_owned(),
    )];
    match (setname, &core.setname_xml) {
        (Some(old), replacement) => {
            let replacement = match replacement {
                Some(new) => preserve_setname_attributes(&text[old.range.clone()], new, path)?,
                None => String::new(),
            };
            patches.push((old.range, replacement));
        }
        (None, Some(new)) => patches.push((rbf.range.end..rbf.range.end, format!("\n\t{new}"))),
        _ => {}
    }
    patches.sort_by_key(|(range, _)| std::cmp::Reverse(range.start));
    let mut result = text.to_owned();
    for (range, replacement) in patches {
        result.replace_range(range, &replacement);
    }
    Ok(result)
}

/// Only known standard/RA references are eligible for conversion. Custom MGLs
/// remain direct launches, including launchers for unrelated cores or sets.
pub fn recognized_favorite(path: &Path, system: &SystemConfig) -> Result<bool> {
    let text = read_launcher(path)?;
    let (rbf, setname) = elements(&text, path)?;
    let base = Path::new(&system.rbf)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let base = undated_name(base);
    let identity = base;
    let standard_match = (rbf.value.eq_ignore_ascii_case(&system.rbf)
        || crate::core_choices::is_unstable_reference(system, &rbf.value))
        && match (
            &setname,
            system.setname.as_deref().filter(|s| !s.is_empty()),
        ) {
            (None, None) => true,
            (Some(actual), Some(expected)) => actual.value.eq_ignore_ascii_case(expected),
            _ => false,
        };
    let ra_match = rbf
        .value
        .eq_ignore_ascii_case(&format!("_RA_Cores/Cores/{base}"))
        && setname.is_some_and(|s| s.value.eq_ignore_ascii_case(&format!("RA_{identity}")));
    Ok(standard_match || ra_match)
}

/// Avoid generating a temporary shortcut when its effective core is unchanged.
pub fn needs_conversion(path: &Path, core: &EffectiveCore) -> Result<bool> {
    let text = read_launcher(path)?;
    let (rbf, setname) = elements(&text, path)?;
    if !rbf.value.eq_ignore_ascii_case(&core.rbf) {
        return Ok(true);
    }
    Ok(match (setname, &core.setname_xml) {
        (None, None) => false,
        (Some(old), Some(new)) => text[old.range] != *new,
        _ => true,
    })
}

pub fn convert_favorite(path: &Path, core: &EffectiveCore) -> Result<String> {
    apply(&read_launcher(path)?, path, core)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    fn setup(name: &str) -> (PathBuf, SystemConfig) {
        let root = std::env::temp_dir().join(format!("degauss-ra-{name}-{}", std::process::id()));
        if root.exists() {
            std::fs::remove_dir_all(&root).unwrap();
        }
        std::fs::create_dir_all(root.join("_Console")).unwrap();
        std::fs::create_dir_all(root.join("_RA_Cores/Cores")).unwrap();
        std::fs::create_dir_all(root.join("games/NES")).unwrap();
        let system = SystemConfig {
            preserve_rbf_stem: false,
            name: "NES".into(),
            path: root.join("games/NES").to_string_lossy().into_owned(),
            extra_paths: vec![],
            extensions: vec!["nes".into()],
            rbf: "_Console/NES".into(),
            setname: None,
            skip_folders: vec![],
            launch: vec![crate::config::LaunchRule {
                extensions: vec!["nes".into()],
                delay: 1,
                kind: "f".into(),
                index: 0,
                companion_extensions: vec![],
                companion_index: None,
                reset_delay: None,
                reset_hold: None,
            }],
        };
        (root, system)
    }
    fn install_ra(root: &Path) {
        std::fs::write(root.join("_RA_Cores/Cores/NES.rbf"), b"test discovery only").unwrap();
        std::fs::write(root.join("_RA_Cores/NES.mgl"), "<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir=\"1\">RA_NES</setname></mistergamedescription>").unwrap();
    }
    #[test]
    fn preference_and_missing_variant_matrix() {
        let (root, system) = setup("matrix");
        for ra_first in [false, true] {
            assert!(resolve(&system, &root, ra_first).is_err());
        }
        std::fs::write(
            root.join("_Console/NES_20260907.rbf"),
            b"test discovery only",
        )
        .unwrap();
        for ra_first in [false, true] {
            assert_eq!(resolve(&system, &root, ra_first).unwrap().rbf, system.rbf);
        }
        install_ra(&root);
        let favorite = crate::launch::favorite_mgl_with_preference(
            &system,
            &root.join("games/NES/Game.nes"),
            &root,
            true,
        )
        .unwrap()
        .unwrap();
        assert!(favorite.contains("<setname same_dir=\"1\">RA_NES</setname>"));
        assert_eq!(resolve(&system, &root, false).unwrap().rbf, system.rbf);
        assert_eq!(
            resolve(&system, &root, true)
                .unwrap()
                .setname_xml
                .as_deref(),
            Some("<setname same_dir=\"1\">RA_NES</setname>")
        );
        std::fs::remove_file(root.join("_Console/NES_20260907.rbf")).unwrap();
        for ra_first in [false, true] {
            assert_eq!(
                resolve(&system, &root, ra_first).unwrap().rbf,
                "_RA_Cores/Cores/NES"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn malformed_preferred_launcher_is_not_silently_replaced_with_standard() {
        let (root, system) = setup("malformed");
        install_ra(&root);
        std::fs::write(root.join("_Console/NES.rbf"), b"test discovery only").unwrap();
        std::fs::write(
            root.join("_RA_Cores/NES.mgl"),
            "<mistergamedescription><rbf>wrong</rbf></mistergamedescription>",
        )
        .unwrap();
        assert!(resolve(&system, &root, true).is_err());
        assert_eq!(resolve(&system, &root, false).unwrap().rbf, system.rbf);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn temporary_conversion_preserves_actions_and_original_favorite() {
        let (root, system) = setup("favorite");
        install_ra(&root);
        std::fs::write(root.join("games/NES/Game.nes"), b"test path only").unwrap();
        let original = "<mistergamedescription custom='yes'><rbf>_Console/NES</rbf><!-- keep --><file delay='2' type='f' index='1' path='Game.nes' custom='unchanged'/><reset delay='3' hold='4'/><unknown x='y'/></mistergamedescription>";
        let path = root.join("Favorite.mgl");
        std::fs::write(&path, original).unwrap();
        let plan = crate::launch::plan_with_preference(
            &system,
            &path,
            &root.join("temp.mgl"),
            &root,
            true,
        )
        .unwrap();
        assert!(plan
            .mgl
            .contains("<setname same_dir=\"1\">RA_NES</setname>"));
        assert!(plan.mgl.contains("<!-- keep -->"));
        assert!(plan
            .mgl
            .contains("custom='unchanged'/><reset delay='3' hold='4'/><unknown x='y'/>"));
        assert!(plan.mgl.contains(&format!(
            "path='{}'",
            root.join("games/NES/Game.nes").display()
        )));
        assert_eq!(std::fs::read_to_string(path).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn shared_core_uses_installer_identity_and_preserves_system_file_rules() {
        let (root, mut system) = setup("shared");
        install_ra(&root);
        system.setname = Some("FDS".into());
        assert_eq!(
            resolve(&system, &root, true)
                .unwrap()
                .setname_xml
                .as_deref(),
            Some("<setname same_dir=\"1\">RA_NES</setname>")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn a_nightly_favorite_can_switch_back_without_rewriting_the_saved_file() {
        let (root, system) = setup("nightly-favorite");
        std::fs::write(root.join("_Console/NES.rbf"), b"discovery fixture").unwrap();
        std::fs::create_dir_all(root.join("_Unstable")).unwrap();
        let chosen = "_Unstable/NES_unstable_20260810_137d52.rbf";
        std::fs::write(root.join(chosen), b"discovery fixture").unwrap();
        let game = root.join("games/NES/Game.nes");
        std::fs::write(&game, b"path fixture").unwrap();
        let original =
            crate::launch::favorite_mgl_with_choice(&system, &game, &root, false, Some(chosen))
                .unwrap()
                .unwrap();
        let expected_rbf = "<rbf>_Unstable/NES_unstable_20260810_137d52</rbf>";
        assert!(original.contains(expected_rbf));
        assert!(!original.contains("137d52.rbf</rbf>"));
        let ordinary = crate::launch::plan_with_choice(
            &system,
            &game,
            &root.join("ordinary.mgl"),
            &root,
            false,
            Some(chosen),
        )
        .unwrap();
        assert!(ordinary.mgl.contains(expected_rbf));
        let definitions = crate::systems::parse_table(
            r#"
[[systems]]
name = "NES"
id = "NES"
folders = ["NES"]
rbf = "_Console/NES"
extensions = ["nes", "mgl"]
"#,
            Path::new("nightly Favorite fixture"),
        )
        .unwrap();
        let owners: Vec<_> = definitions
            .into_iter()
            .map(|def| crate::systems::FoundSystem {
                def,
                paths: vec![root.join("games/NES")],
                logo_dir: None,
                menu_folder: None,
            })
            .collect();
        let favorite = root.join("Nightly.mgl");
        std::fs::write(&favorite, &original).unwrap();
        let owner = || {
            let reference =
                crate::favorites::reference_of_with_systems(&favorite, &owners).unwrap();
            crate::app::owner_of_favorite(&owners, &reference)
        };
        assert!(recognized_favorite(&favorite, &system).unwrap());
        assert_eq!(owner().as_deref(), Some("NES"));
        std::fs::remove_file(root.join(chosen)).unwrap();
        assert!(recognized_favorite(&favorite, &system).unwrap());
        assert_eq!(owner().as_deref(), Some("NES"));
        let plan = crate::launch::plan_with_choice(
            &system,
            &favorite,
            &root.join("temp.mgl"),
            &root,
            false,
            Some("standard"),
        )
        .unwrap();
        assert!(plan.mgl.contains("<rbf>_Console/NES</rbf>"));
        assert_eq!(std::fs::read_to_string(favorite).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn configured_dated_stems_and_released_punctuation_aliases_remain_present() {
        let (root, mut system) = setup("dated-config");
        std::fs::write(root.join("_Console/NES_20260823.rbf"), b"discovery fixture").unwrap();
        system.rbf = "_Console/NES_20260823".into();
        assert!(core_present(&root, &system.rbf).unwrap());
        assert_eq!(resolve(&system, &root, false).unwrap().rbf, system.rbf);
        install_ra(&root);
        assert_eq!(
            resolve(&system, &root, true).unwrap().rbf,
            "_RA_Cores/Cores/NES"
        );
        std::fs::write(
            root.join("_Console/NeoGeoPocket-Color.rbf"),
            b"discovery fixture",
        )
        .unwrap();
        assert!(core_present(&root, "_Console/NeoGeoPocketColor").unwrap());
        let invalid = core_present(&root, "_Console/NES_20260823.rbf")
            .unwrap_err()
            .to_string();
        assert!(invalid.contains("extensionless RBF reference"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_doctype_mgl_keeps_direct_launch_and_declaration_during_conversion() {
        let (root, system) = setup("doctype");
        std::fs::write(root.join("_Console/NES.rbf"), b"discovery fixture").unwrap();
        let game = root.join("games/NES/Game.nes");
        std::fs::write(&game, b"path fixture").unwrap();
        let original = format!("<!DOCTYPE mistergamedescription>\n<mistergamedescription><rbf>_Console/NES</rbf><file delay=\"1\" type=\"f\" index=\"1\" path=\"{}\"/></mistergamedescription>", game.display());
        let favorite = root.join("Custom.mgl");
        std::fs::write(&favorite, &original).unwrap();
        let output = root.join("temp.mgl");
        let direct = crate::launch::plan_with_choice(
            &system,
            &favorite,
            &output,
            &root,
            false,
            Some("standard"),
        )
        .unwrap();
        assert!(direct.mgl.is_empty());
        assert_eq!(
            direct.command,
            format!("load_core {}\n", favorite.display())
        );
        install_ra(&root);
        let converted =
            crate::launch::plan_with_choice(&system, &favorite, &output, &root, false, Some("ra"))
                .unwrap();
        assert!(converted
            .mgl
            .starts_with("<!DOCTYPE mistergamedescription>\n"));
        assert!(converted.mgl.contains("<rbf>_RA_Cores/Cores/NES</rbf>"));
        assert_eq!(std::fs::read_to_string(favorite).unwrap(), original);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn duplicate_or_unclosed_descriptors_are_rejected() {
        let path = Path::new("test.mgl");
        for text in [
            "<mistergamedescription><rbf>x</rbf><rbf>x</rbf></mistergamedescription>",
            "<mistergamedescription><rbf>x</rbf>",
        ] {
            assert!(elements(text, path).is_err());
        }
    }
}
