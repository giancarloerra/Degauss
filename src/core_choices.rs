//! Explicit per-system core versions. Choices are resolved from current files
//! at launch time; catalog rows and systems tables retain their old identity.
use std::path::{Component, Path, PathBuf};

use crate::config::SystemConfig;
use crate::core_variants::{self, EffectiveCore};
use crate::error::{DegaussError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CoreChoice {
    pub key: String,
    pub label: String,
}

fn core_stem(reference: &str) -> &str {
    let name = reference.rsplit('/').next().unwrap_or(reference);
    if name
        .get(name.len().saturating_sub(4)..)
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".rbf"))
    {
        &name[..name.len() - 4]
    } else {
        name
    }
}

fn nightly_matches(stem: &str, reference: &str) -> bool {
    let wanted = crate::systems::core_name(core_stem(reference));
    if wanted.is_empty() {
        return false;
    }
    if crate::systems::core_name(stem) == wanted {
        return true;
    }
    // The official nightly suffix is bounded, not a loose core-name prefix.
    let lower = stem.to_ascii_lowercase();
    let Some(at) = lower.rfind("_unstable_") else {
        return false;
    };
    let suffix = &stem[at + "_unstable_".len()..];
    let Some((date, hash)) = suffix.split_once('_') else {
        return false;
    };
    date.len() == 8
        && date.bytes().all(|byte| byte.is_ascii_digit())
        && !hash.is_empty()
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
        && crate::systems::core_name(&stem[..at]) == wanted
}

/// Recognize a nightly reference even after removal, so a saved favourite
/// can retain its owning system and report its missing selected version.
pub fn is_unstable_reference(system: &SystemConfig, reference: &str) -> bool {
    let path = Path::new(reference);
    path.components()
        .all(|component| matches!(component, Component::Normal(_)))
        && path.components().next() == Some(Component::Normal("_Unstable".as_ref()))
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| nightly_matches(core_stem(name), &system.rbf))
}

fn nightlies(system: &SystemConfig, root: &Path) -> Result<Vec<CoreChoice>> {
    let folder = root.join("_Unstable");
    match std::fs::metadata(&folder) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DegaussError::io("Unstable directory", &folder, error)),
        Ok(_) => {}
    }
    let mut result = Vec::new();
    walk_nightlies(system, root, &folder, &mut Vec::new(), &mut result)?;
    result.sort_by(|a, b| {
        a.key
            .to_ascii_lowercase()
            .as_bytes()
            .cmp(b.key.to_ascii_lowercase().as_bytes())
            .then_with(|| a.key.as_bytes().cmp(b.key.as_bytes()))
    });
    Ok(result)
}

fn walk_nightlies(
    system: &SystemConfig,
    root: &Path,
    folder: &Path,
    ancestors: &mut Vec<PathBuf>,
    result: &mut Vec<CoreChoice>,
) -> Result<()> {
    if ancestors.len() > crate::browse::MAX_DEPTH {
        return Err(DegaussError::unsupported(
            "Unstable directory",
            "maximum folder depth exceeded",
        ));
    }
    let identity = std::fs::canonicalize(folder)
        .map_err(|error| DegaussError::io("Unstable directory", folder, error))?;
    if ancestors.contains(&identity) {
        return Err(DegaussError::unsupported(
            "Unstable directory",
            format!("directory cycle at {}", folder.display()),
        ));
    }
    ancestors.push(identity);
    let listing = std::fs::read_dir(folder)
        .map_err(|error| DegaussError::io("Unstable directory", folder, error))?;
    for entry in listing {
        let entry =
            entry.map_err(|error| DegaussError::io("Unstable directory entry", folder, error))?;
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        let metadata = std::fs::metadata(&path)
            .map_err(|error| DegaussError::io("Unstable core entry", &path, error))?;
        if metadata.is_dir() {
            walk_nightlies(system, root, &path, ancestors, result)?;
        } else if metadata.is_file()
            && path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("rbf"))
        {
            let stem = path
                .file_stem()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    DegaussError::unsupported("Unstable core", "filename is not valid UTF-8")
                })?;
            if nightly_matches(stem, &system.rbf) {
                let relative = path.strip_prefix(root).expect("scan remains beneath root");
                let key = relative.to_str().ok_or_else(|| {
                    DegaussError::unsupported("Unstable core", "path is not valid UTF-8")
                })?;
                result.push(CoreChoice {
                    key: key.into(),
                    label: key.into(),
                });
            }
        }
    }
    ancestors.pop();
    Ok(())
}

/// Main's MGL loader chooses the greatest original filename among
/// case-insensitive stem-prefix matches followed by '.' or '_'. Validate
/// that its native lookup will select the exact saved choice.
fn validate_native_choice(actual: &Path) -> Result<()> {
    let directory = actual.parent().expect("Unstable file has a parent");
    let selected = actual
        .file_name()
        .expect("Unstable file has a name")
        .as_encoded_bytes();
    let prefix = &selected[..selected.len() - 4];
    let mut winner: Option<Vec<u8>> = None;
    let entries = std::fs::read_dir(directory)
        .map_err(|error| DegaussError::io("selected core directory", directory, error))?;
    for entry in entries {
        let entry = entry
            .map_err(|error| DegaussError::io("selected core directory entry", directory, error))?;
        let name = entry.file_name();
        let bytes = name.as_encoded_bytes();
        if bytes.len() <= prefix.len()
            || bytes.len() <= 4
            || !bytes[bytes.len() - 4..].eq_ignore_ascii_case(b".rbf")
            || !bytes[..prefix.len()].eq_ignore_ascii_case(prefix)
            || !matches!(bytes[prefix.len()], b'.' | b'_')
        {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|error| DegaussError::io("selected core entry", entry.path(), error))?;
        if kind.is_dir() {
            continue;
        }
        if winner
            .as_ref()
            .is_none_or(|current| current.as_slice() < bytes)
        {
            winner = Some(bytes.to_vec());
        }
    }
    if winner.as_deref() != Some(selected) {
        return Err(DegaussError::unsupported("selected core version", format!(
            "MiSTer's filename lookup would not select {}; another matching filename shadows this build",
            actual.display())));
    }
    Ok(())
}

/// Installed versions plus a labelled RA error when that installation is
/// invalid. The UI adds Default; no default is persisted.
pub fn available(system: &SystemConfig, root: &Path) -> Result<Vec<CoreChoice>> {
    if system.rbf.is_empty() {
        return Ok(Vec::new());
    }
    let mut choices = Vec::new();
    if core_variants::core_present(root, &system.rbf)? {
        choices.push(CoreChoice {
            key: "standard".into(),
            label: "Standard".into(),
        });
    }
    match core_variants::installed_ra(system, root) {
        Ok(Some(_)) => choices.push(CoreChoice {
            key: "ra".into(),
            label: "RetroAchievements".into(),
        }),
        Ok(None) => {}
        Err(error) => choices.push(CoreChoice {
            key: "ra".into(),
            label: format!("RetroAchievements unavailable: {error}"),
        }),
    }
    choices.extend(nightlies(system, root)?);
    Ok(choices)
}

/// Explicit choices fail if missing. Default preserves the Standard/RA rule
/// and never silently selects an Unstable core, even if only one exists.
pub fn resolve(
    system: &SystemConfig,
    root: &Path,
    selected: Option<&str>,
    ra_first: bool,
) -> Result<EffectiveCore> {
    let Some(selected) = selected else {
        return core_variants::resolve(system, root, ra_first);
    };
    let missing = || {
        DegaussError::unsupported(
            "selected core version",
            format!("{selected} is not installed for {}", system.name),
        )
    };
    match selected {
        "standard" => {
            if !core_variants::core_present(root, &system.rbf)? {
                return Err(missing());
            }
            Ok(core_variants::standard(system))
        }
        "ra" => core_variants::installed_ra(system, root)?.ok_or_else(missing),
        _ => {
            let path = Path::new(selected);
            if !path
                .extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("rbf"))
                || !is_unstable_reference(system, selected)
            {
                return Err(DegaussError::unsupported(
                    "selected core version",
                    format!("invalid Unstable choice {selected}"),
                ));
            }
            let actual = root.join(path);
            match std::fs::metadata(&actual) {
                Ok(metadata) if metadata.is_file() => {}
                Ok(_) => return Err(missing()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(missing()),
                Err(error) => {
                    return Err(DegaussError::io("selected Unstable core", &actual, error))
                }
            }
            validate_native_choice(&actual)?;
            let mut core = core_variants::standard(system);
            // Main appends/matches the extension itself. Keep the complete
            // dated/hash stem so the native lookup retains this exact build.
            core.rbf = selected[..selected.len() - 4].into();
            Ok(core)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn directory(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("degauss-core-choices-{tag}-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn system() -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "NES".into(),
            path: "/games/NES".into(),
            extensions: vec!["nes".into()],
            rbf: "_Console/NES".into(),
            launch: Vec::new(),
            setname: Some("NES".into()),
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    fn write(root: &Path, path: &str, bytes: &[u8]) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn choices_list_installed_standard_ra_and_distinct_matching_nightlies() {
        let root = directory("all");
        write(&root, "_Console/NES_20260901.rbf", b"fixture");
        write(&root, "_RA_Cores/Cores/NES.rbf", b"fixture");
        write(&root, "_RA_Cores/NES.mgl", br#"<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir="1">RA_NES</setname></mistergamedescription>"#);
        for filename in [
            "NES_unstable_20260901_a1.rbf",
            "NES_unstable_20260902_b2.RBF",
            "NES_20260903.rbf",
            "NES_RF_20260901.rbf",
            "SNES_unstable_20260901_a1.rbf",
        ] {
            write(&root, &format!("_Unstable/{filename}"), b"fixture");
        }
        let choices = available(&system(), &root).unwrap();
        assert_eq!(choices.len(), 5);
        assert_eq!(choices[0].key, "standard");
        assert_eq!(choices[1].key, "ra");
        assert!(choices
            .iter()
            .all(|choice| !choice.key.contains("NES_RF") && !choice.key.contains("SNES")));
        let ra = resolve(&system(), &root, Some("ra"), false).unwrap();
        assert_eq!(ra.rbf, "_RA_Cores/Cores/NES");
        assert_eq!(
            ra.setname_xml.as_deref(),
            Some("<setname same_dir=\"1\">RA_NES</setname>")
        );
        for key in [
            "_Unstable/NES_unstable_20260901_a1.rbf",
            "_Unstable/NES_unstable_20260902_b2.RBF",
        ] {
            let chosen = resolve(&system(), &root, Some(key), false).unwrap();
            assert_eq!(chosen.rbf, &key[..key.len() - 4]);
            assert_eq!(
                chosen.setname_xml.as_deref(),
                Some("<setname>NES</setname>")
            );
        }
        assert_eq!(
            resolve(&system(), &root, None, false).unwrap().rbf,
            "_Console/NES"
        );
        assert_eq!(
            resolve(&system(), &root, None, true).unwrap().rbf,
            "_RA_Cores/Cores/NES"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nightly_only_system_requires_an_explicit_exact_choice() {
        let root = directory("nightly-only");
        let key = "_Unstable/nested/NES_unstable_20260901_ab12.rbf";
        write(&root, key, b"fixture");
        let choices = available(&system(), &root).unwrap();
        assert_eq!(choices.len(), 1);
        assert_eq!(choices[0].key, key);
        assert!(resolve(&system(), &root, None, false).is_err());
        assert_eq!(
            resolve(&system(), &root, Some(key), false).unwrap().rbf,
            &key[..key.len() - 4]
        );
        assert!(resolve(&system(), &root, Some("standard"), false).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn removed_explicit_nightly_is_an_error_even_with_standard_installed() {
        let root = directory("missing-selected");
        let key = "_Unstable/NES_unstable_20260901_aa.rbf";
        write(&root, "_Console/NES.rbf", b"fixture");
        write(&root, key, b"fixture");
        assert!(resolve(&system(), &root, Some(key), false).is_ok());
        std::fs::remove_file(root.join(key)).unwrap();
        let error = resolve(&system(), &root, Some(key), false).unwrap_err();
        assert!(error.to_string().contains(key));
        assert!(
            is_unstable_reference(&system(), key),
            "removed favourite retains system identity"
        );
        assert!(resolve(&system(), &root, Some("ra"), false).is_err());
        assert_eq!(
            resolve(&system(), &root, Some("standard"), true)
                .unwrap()
                .rbf,
            "_Console/NES"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn arbitrary_prefixes_and_escaping_paths_are_not_core_versions() {
        let system = system();
        for reference in [
            "_Unstable/NES_RF.rbf",
            "_Unstable/NES_unstable_20260901_NOTHASH.rbf",
            "_Unstable/NES_unstable_2026_abc.rbf",
            "_Unstable/../NES.rbf",
            "/_Unstable/NES.rbf",
            "_Console/NES.rbf",
            "_Unstable/NES.mgl",
        ] {
            assert!(!is_unstable_reference(&system, reference), "{reference}");
        }
        assert!(is_unstable_reference(&system, "_Unstable/NES.rbf"));
        assert!(is_unstable_reference(&system, "_Unstable/NES"));
        assert!(is_unstable_reference(
            &system,
            "_Unstable/NES_unstable_20260901_ab12"
        ));
        assert!(is_unstable_reference(&system, "_Unstable/NES_20260901.rbf"));
        assert!(!nightly_matches("日本語", "_Console/NES"));
        assert_eq!(core_stem("日本語"), "日本語");
    }

    #[test]
    fn malformed_ra_choice_does_not_remove_a_valid_standard_alternative() {
        let root = directory("malformed-ra");
        write(&root, "_Console/NES.rbf", b"fixture");
        write(&root, "_RA_Cores/NES.mgl", b"malformed XML");
        let choices = available(&system(), &root).unwrap();
        assert_eq!(choices[0].key, "standard");
        assert_eq!(choices[1].key, "ra");
        assert!(choices[1].label.contains("unavailable"));
        assert!(resolve(&system(), &root, Some("standard"), true).is_ok());
        assert!(resolve(&system(), &root, Some("ra"), false).is_err());
        assert!(resolve(&system(), &root, None, false).is_ok());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ra_only_system_remains_available_without_a_standard_binary() {
        let root = directory("ra-only");
        write(&root, "_RA_Cores/Cores/NES.rbf", b"fixture");
        write(&root, "_RA_Cores/NES.mgl", br#"<mistergamedescription><rbf>_RA_Cores/Cores/NES</rbf><setname same_dir="1">RA_NES</setname></mistergamedescription>"#);
        assert_eq!(available(&system(), &root).unwrap()[0].key, "ra");
        assert_eq!(
            resolve(&system(), &root, None, false).unwrap().rbf,
            "_RA_Cores/Cores/NES"
        );
        assert!(resolve(&system(), &root, Some("standard"), false).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn native_main_prefix_shadowing_is_reported_instead_of_loading_another_build() {
        let root = directory("native-shadow");
        let key = "_Unstable/NES_unstable_20260901_aa.rbf";
        write(&root, key, b"fixture");
        write(
            &root,
            "_Unstable/NES_unstable_20260901_aa_extra.rbf",
            b"fixture",
        );
        let error = resolve(&system(), &root, Some(key), false).unwrap_err();
        assert!(error.to_string().contains("shadows this build"));
        std::fs::remove_file(root.join("_Unstable/NES_unstable_20260901_aa_extra.rbf")).unwrap();
        assert_eq!(
            resolve(&system(), &root, Some(key), false).unwrap().rbf,
            "_Unstable/NES_unstable_20260901_aa"
        );
        assert!(
            resolve(
                &system(),
                &root,
                Some("_Unstable/NES_unstable_20260901_aa"),
                false
            )
            .is_err(),
            "saved choices require their exact .rbf filename"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
