//! Per-system selection among explicitly compatible core families.
//!
//! A family chooses the core identity and optional set name. The existing
//! Core Version setting then chooses Standard, RetroAchievements or an
//! Unstable build within that family.

use std::path::Path;

use crate::config::SystemConfig;
use crate::error::{DegaussError, Result};

/// Stable settings identity for the system-level, preferred core family.
/// Additional family IDs come from `systems.toml`.
pub const PRIMARY_PROFILE_ID: &str = "primary";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    /// Empty means Automatic and therefore removes the saved override.
    pub id: String,
    pub label: String,
    pub available: bool,
}

#[derive(Debug, Clone)]
struct Family {
    id: String,
    label: String,
    config: SystemConfig,
}

fn primary_label(system: &SystemConfig) -> String {
    Path::new(&system.rbf)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or(&system.name)
        .to_string()
}

fn families(system: &SystemConfig) -> Vec<Family> {
    let mut configurations = system.core_family_configs().into_iter();
    let primary = configurations
        .next()
        .expect("every system has its preferred core family");
    let mut result = Vec::with_capacity(1 + system.compatible_cores.len());
    result.push(Family {
        id: PRIMARY_PROFILE_ID.to_string(),
        label: primary_label(system),
        config: primary,
    });
    result.extend(
        system
            .compatible_cores
            .iter()
            .zip(configurations)
            .map(|(profile, config)| Family {
                id: profile.id.clone(),
                label: profile.label.clone(),
                config,
            }),
    );
    result
}

/// True when this system has a useful Launch Core choice to show.
pub fn has_alternatives(system: &SystemConfig) -> bool {
    !system.compatible_cores.is_empty()
}

/// A malformed or incomplete RA installation still proves that this family
/// is installed. Core Version reports why that particular version cannot be
/// used. I/O failures remain real failures and must not be hidden.
fn family_present(system: &SystemConfig, root: &Path) -> Result<bool> {
    if crate::core_variants::core_present(root, &system.rbf)? {
        return Ok(true);
    }
    match crate::core_variants::installed_ra(system, root) {
        Ok(Some(_)) => return Ok(true),
        Ok(None) => {}
        Err(error @ DegaussError::Io { .. }) => return Err(error),
        Err(_) => return Ok(true),
    }
    crate::core_choices::has_unstable_version(system, root)
}

fn family_for_saved_version(
    system: &SystemConfig,
    root: &Path,
    selected_version: &str,
    ra_first: bool,
) -> Result<Family> {
    let named_variant = matches!(selected_version, "standard" | "ra");
    let mut missing = None;
    for family in families(system) {
        if !named_variant
            && !crate::core_choices::is_unstable_reference(&family.config, selected_version)
        {
            continue;
        }
        match crate::core_choices::resolve(&family.config, root, Some(selected_version), ra_first) {
            Ok(_) => return Ok(family),
            Err(error @ DegaussError::Unsupported { .. }) if named_variant => missing = Some(error),
            Err(error) => return Err(error),
        }
    }
    if let Some(error) = missing {
        return Err(error);
    }
    match crate::core_choices::resolve(system, root, Some(selected_version), ra_first) {
        Ok(_) => Err(DegaussError::unsupported(
            "selected core version",
            format!(
                "saved choice {selected_version:?} does not identify a declared compatible core for {}",
                system.name
            ),
        )),
        Err(error) => Err(error),
    }
}

fn automatic_family(
    system: &SystemConfig,
    root: &Path,
    selected_version: Option<&str>,
    ra_first: bool,
) -> Result<Family> {
    if let Some(selected_version) = selected_version {
        return family_for_saved_version(system, root, selected_version, ra_first);
    }
    for family in families(system) {
        if family_present(&family.config, root)? {
            return Ok(family);
        }
    }
    Err(DegaussError::unsupported(
        "launch core",
        format!(
            "no declared compatible core is installed for {}",
            system.name
        ),
    ))
}

/// Resolve a saved profile, or the first installed declared family when the
/// system remains Automatic. Explicit missing and removed profiles fail.
#[cfg(test)]
pub fn resolve(system: &SystemConfig, root: &Path, selected: Option<&str>) -> Result<SystemConfig> {
    resolve_for_version(system, root, selected, None, false)
}

/// Resolve a Launch Core while retaining an existing Core Version choice.
/// An explicit Launch Core remains authoritative. Automatic instead chooses
/// the first declared family that can still satisfy the saved version, which
/// preserves the behaviour from before compatible families were selectable.
pub fn resolve_for_version(
    system: &SystemConfig,
    root: &Path,
    selected: Option<&str>,
    selected_version: Option<&str>,
    ra_first: bool,
) -> Result<SystemConfig> {
    let Some(selected) = selected else {
        return automatic_family(system, root, selected_version, ra_first)
            .map(|family| family.config);
    };
    let family = families(system)
        .into_iter()
        .find(|family| family.id == selected)
        .ok_or_else(|| {
            DegaussError::unsupported(
                "selected launch core",
                format!(
                    "saved profile {selected:?} is no longer declared for {}",
                    system.name
                ),
            )
        })?;
    if !family_present(&family.config, root)? {
        return Err(DegaussError::unsupported(
            "selected launch core",
            format!("{} is not installed for {}", family.label, system.name),
        ));
    }
    Ok(family.config)
}

/// Rows for the dedicated chooser. Automatic is always available so a bad or
/// removed saved profile can always be cleared without successful discovery.
#[cfg(test)]
pub fn choices(system: &SystemConfig, root: &Path, selected: Option<&str>) -> Vec<Choice> {
    choices_for_version(system, root, selected, None, false)
}

/// Rows for the chooser with Automatic labelled from the complete effective
/// selection, including a saved Core Version that belongs to an alternate
/// compatible family.
pub fn choices_for_version(
    system: &SystemConfig,
    root: &Path,
    selected: Option<&str>,
    selected_version: Option<&str>,
    ra_first: bool,
) -> Vec<Choice> {
    let automatic = match automatic_family(system, root, selected_version, ra_first) {
        Ok(family) => format!("Automatic ({})", family.label),
        Err(error) => {
            crate::note(&format!(
                "launch core  automatic resolution failed: {error}"
            ));
            "Automatic (Unavailable)".to_string()
        }
    };
    let mut result = vec![Choice {
        id: String::new(),
        label: automatic,
        available: true,
    }];
    let mut selected_declared = selected.is_none();
    for family in families(system) {
        let saved = selected == Some(family.id.as_str());
        selected_declared |= saved;
        match family_present(&family.config, root) {
            Ok(true) => result.push(Choice {
                id: family.id,
                label: family.label,
                available: true,
            }),
            Ok(false) if saved => result.push(Choice {
                id: family.id,
                label: format!("{} (Unavailable)", family.label),
                available: false,
            }),
            Ok(false) => {}
            Err(error) => {
                crate::note(&format!(
                    "launch core  {} could not be checked: {error}",
                    family.label
                ));
                if saved {
                    result.push(Choice {
                        id: family.id,
                        label: format!("{} (Unavailable)", family.label),
                        available: false,
                    });
                }
            }
        }
    }
    if let Some(selected) = selected.filter(|_| !selected_declared) {
        result.push(Choice {
            id: selected.to_string(),
            label: format!("{selected} (Unavailable)"),
            available: false,
        });
    }
    result
}

/// Context-row value without touching the filesystem on every redraw.
pub fn saved_label(system: &SystemConfig, selected: Option<&str>) -> String {
    match selected {
        None => "Automatic".to_string(),
        Some(PRIMARY_PROFILE_ID) => primary_label(system),
        Some(selected) => system
            .compatible_cores
            .iter()
            .find(|profile| profile.id == selected)
            .map(|profile| profile.label.clone())
            .unwrap_or_else(|| format!("{selected} (Unavailable)")),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::config::CoreProfile;

    use super::*;

    fn directory(tag: &str) -> PathBuf {
        let root =
            std::env::temp_dir().join(format!("degauss-launch-cores-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    fn system() -> SystemConfig {
        SystemConfig {
            preserve_rbf_stem: false,
            name: "Neo Geo Pocket Color".into(),
            path: "/games/NGPC".into(),
            extensions: vec!["ngc".into(), "npc".into()],
            rbf: "_Console/NGPC".into(),
            launch: Vec::new(),
            setname: None,
            compatible_cores: vec![CoreProfile {
                id: "jtngpc".into(),
                label: "JTNGPC (Legacy)".into(),
                rbf: "_Arcade/JTNGPC".into(),
                setname: Some("JTNGPC".into()),
            }],
            skip_folders: Vec::new(),
            extra_paths: Vec::new(),
        }
    }

    fn write(root: &Path, path: &str) {
        let path = root.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"fixture").unwrap();
    }

    #[test]
    fn automatic_uses_declared_order_and_explicit_profiles_pin_the_family() {
        let root = directory("ordering");
        let system = system();
        write(&root, "_Arcade/JTNGPC.rbf");
        assert_eq!(resolve(&system, &root, None).unwrap().rbf, "_Arcade/JTNGPC");
        write(&root, "_Console/NGPC.rbf");
        assert_eq!(resolve(&system, &root, None).unwrap().rbf, "_Console/NGPC");
        assert_eq!(
            resolve(&system, &root, Some("jtngpc")).unwrap().rbf,
            "_Arcade/JTNGPC"
        );
        assert_eq!(
            resolve(&system, &root, Some(PRIMARY_PROFILE_ID))
                .unwrap()
                .rbf,
            "_Console/NGPC"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn chooser_hides_missing_profiles_but_keeps_a_saved_one_resettable() {
        let root = directory("missing");
        let system = system();
        write(&root, "_Console/NGPC.rbf");
        let available = choices(&system, &root, None);
        assert_eq!(
            available
                .iter()
                .map(|choice| choice.label.as_str())
                .collect::<Vec<_>>(),
            ["Automatic (NGPC)", "NGPC"]
        );
        let saved = choices(&system, &root, Some("jtngpc"));
        assert!(saved[0].id.is_empty(), "Automatic always remains first");
        assert!(saved
            .iter()
            .any(|choice| choice.id == "jtngpc" && !choice.available));
        assert!(resolve(&system, &root, Some("jtngpc")).is_err());
        let removed = choices(&system, &root, Some("removed-profile"));
        assert!(removed.iter().any(|choice| {
            choice.id == "removed-profile"
                && choice.label == "removed-profile (Unavailable)"
                && !choice.available
        }));
        assert!(resolve(&system, &root, Some("removed-profile")).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_unstable_build_counts_as_an_installed_family() {
        let root = directory("unstable");
        let system = system();
        write(&root, "_Unstable/NGPC_unstable_20260916_a1.rbf");
        assert_eq!(resolve(&system, &root, None).unwrap().rbf, "_Console/NGPC");
        assert!(choices(&system, &root, None)
            .iter()
            .any(|choice| choice.id == PRIMARY_PROFILE_ID));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn core_version_is_resolved_inside_the_selected_family() {
        let root = directory("versions");
        let system = system();
        write(&root, "_Console/NGPC.rbf");
        write(&root, "_Arcade/JTNGPC.rbf");
        write(&root, "_RA_Cores/Cores/JTNGPC.rbf");
        let launcher = root.join("_RA_Cores/JTNGPC.mgl");
        std::fs::write(
            &launcher,
            b"<mistergamedescription><rbf>_RA_Cores/Cores/JTNGPC</rbf><setname same_dir=\"1\">RA_JTNGPC</setname></mistergamedescription>",
        )
        .unwrap();
        let nightly = "_Unstable/JTNGPC_unstable_20260916_a1.rbf";
        write(&root, nightly);

        let family = resolve(&system, &root, Some("jtngpc")).unwrap();
        assert_eq!(
            crate::core_choices::resolve(&family, &root, Some("standard"), false)
                .unwrap()
                .rbf,
            "_Arcade/JTNGPC"
        );
        assert_eq!(
            crate::core_choices::resolve(&family, &root, Some("ra"), false)
                .unwrap()
                .rbf,
            "_RA_Cores/Cores/JTNGPC"
        );
        assert_eq!(
            crate::core_choices::resolve(&family, &root, Some(nightly), false)
                .unwrap()
                .rbf,
            nightly.trim_end_matches(".rbf")
        );
        assert_eq!(
            crate::core_choices::available(&family, &root)
                .unwrap()
                .into_iter()
                .map(|choice| choice.key)
                .collect::<Vec<_>>(),
            ["standard", "ra", nightly]
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_keeps_saved_versions_on_the_family_that_provides_them() {
        let root = directory("saved-version-family");
        let system = system();
        write(&root, "_Console/NGPC.rbf");
        write(&root, "_RA_Cores/Cores/JTNGPC.rbf");
        let launcher = root.join("_RA_Cores/JTNGPC.mgl");
        std::fs::write(
            &launcher,
            b"<mistergamedescription><rbf>_RA_Cores/Cores/JTNGPC</rbf><setname same_dir=\"1\">RA_JTNGPC</setname></mistergamedescription>",
        )
        .unwrap();

        let ra = resolve_for_version(&system, &root, None, Some("ra"), false).unwrap();
        assert_eq!(ra.rbf, "_Arcade/JTNGPC");
        assert_eq!(
            choices_for_version(&system, &root, None, Some("ra"), false)[0].label,
            "Automatic (JTNGPC (Legacy))"
        );

        let nightly = "_Unstable/JTNGPC_unstable_20260916_a1.rbf";
        write(&root, nightly);
        let unstable = resolve_for_version(&system, &root, None, Some(nightly), false).unwrap();
        assert_eq!(unstable.rbf, "_Arcade/JTNGPC");

        let explicit_primary = resolve_for_version(
            &system,
            &root,
            Some(PRIMARY_PROFILE_ID),
            Some(nightly),
            false,
        )
        .unwrap();
        assert_eq!(explicit_primary.rbf, "_Console/NGPC");

        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn automatic_rejects_a_saved_version_that_is_not_a_safe_family_reference() {
        let root = directory("unsafe-saved-version");
        let system = system();
        let filename = "NGPC_unstable_20991231_deadbeef.rbf";
        let outside = root.parent().unwrap().join(filename);
        let _ = std::fs::remove_file(&outside);
        std::fs::write(&outside, b"fixture").unwrap();

        let error =
            resolve_for_version(&system, &root, None, Some(&format!("../{filename}")), false)
                .unwrap_err();
        assert!(error.to_string().contains("selected core version"));

        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}
