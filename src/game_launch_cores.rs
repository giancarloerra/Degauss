//! Sparse per-game compatible-core selections.
//!
//! The outer settings key is the owning system ID. The inner key is the
//! logical launch target, so the same choice follows a game through normal
//! browsing, Last Played and Degauss Favourites without indexing the library.

use std::collections::BTreeMap;
use std::path::{Component, Path};

use crate::browse::Launch;
use crate::settings::Settings;

/// Stable logical identity for one launch within its owning system.
pub fn key(launch: &Launch) -> String {
    match launch {
        Launch::File(path) => format!("f:{}", normalized_path(path)),
        Launch::AmigaVision { install, title } => {
            let install = normalized_path(install);
            format!("a:{}:{install}{title}", install.len())
        }
    }
}

fn normalized_path(path: &Path) -> String {
    let mut result = String::new();
    for component in path.components() {
        match component {
            Component::RootDir => {
                if result.is_empty() {
                    result.push('/');
                }
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.is_empty() && !result.ends_with('/') {
                    result.push('/');
                }
                result.push_str("..");
            }
            Component::Prefix(prefix) => {
                if !result.is_empty() && !result.ends_with('/') {
                    result.push('/');
                }
                result.push_str(&prefix.as_os_str().to_string_lossy());
            }
            Component::Normal(part) => {
                if !result.is_empty() && !result.ends_with('/') {
                    result.push('/');
                }
                result.push_str(&part.to_string_lossy());
            }
        }
    }
    if result.is_empty() {
        ".".to_string()
    } else {
        result
    }
}

/// A game choice wins over the system choice. Absence from both is Automatic.
pub fn effective<'a>(settings: &'a Settings, system_id: &str, launch: &Launch) -> Option<&'a str> {
    settings
        .game_launch_cores
        .get(system_id)
        .and_then(|games| games.get(&key(launch)))
        .or_else(|| settings.launch_cores.get(system_id))
        .map(String::as_str)
}

/// Save or clear one game without leaving empty system tables behind.
pub fn set(settings: &mut Settings, system_id: &str, launch_key: &str, profile: Option<&str>) {
    set_choice(
        &mut settings.game_launch_cores,
        system_id,
        launch_key,
        profile,
    );
}

pub fn set_version(
    settings: &mut Settings,
    system_id: &str,
    launch_key: &str,
    version: Option<&str>,
) {
    set_choice(
        &mut settings.game_core_versions,
        system_id,
        launch_key,
        version,
    );
}

pub fn saved_version<'a>(
    settings: &'a Settings,
    system_id: &str,
    launch_key: &str,
) -> Option<&'a str> {
    settings
        .game_core_versions
        .get(system_id)?
        .get(launch_key)
        .map(String::as_str)
}

pub fn effective_version<'a>(
    settings: &'a Settings,
    system_id: &str,
    launch: &Launch,
    system: &crate::config::SystemConfig,
) -> Option<&'a str> {
    settings
        .game_core_versions
        .get(system_id)
        .and_then(|games| games.get(&key(launch)))
        .filter(|_| crate::launch::game_core_selectable(system, launch))
        .or_else(|| settings.core_choices.get(system_id))
        .map(String::as_str)
}

/// Enumerate only on opening the chooser. Clearing a broken selection must
/// not depend on the old core still being installed.
pub fn version_choices(
    settings: &Settings,
    system_id: &str,
    launch_key: &str,
    system: &crate::config::SystemConfig,
    root: &Path,
) -> (Vec<crate::launch_cores::Choice>, Option<String>) {
    use crate::launch_cores::Choice;
    let saved = saved_version(settings, system_id, launch_key);
    let version = saved.or_else(|| settings.core_choices.get(system_id).map(String::as_str));
    let family = settings
        .game_launch_cores
        .get(system_id)
        .and_then(|m| m.get(launch_key))
        .or_else(|| settings.launch_cores.get(system_id))
        .map(String::as_str);
    let ra_first = settings.core_preference.unwrap_or_default()
        == crate::settings::CorePreference::RetroAchievementsFirst;
    let mut choices = vec![Choice {
        id: String::new(),
        label: "Use System Setting".into(),
        available: true,
    }];
    let mut notice = None;
    let available =
        match crate::launch_cores::resolve_for_version(system, root, family, version, ra_first) {
            Ok(family) => crate::core_choices::available_for_family(&family, root),
            Err(error @ crate::error::DegaussError::Unsupported { .. })
                if family.is_none() && version.is_some() =>
            {
                // The requested version is unavailable. Show the error and offer
                // replacements without silently changing the saved launch choice.
                notice = Some(error.to_string());
                crate::core_choices::available(system, root)
            }
            Err(error) => Err(error),
        };
    match available {
        Ok(rows) => choices.extend(rows.into_iter().map(|c| Choice {
            id: c.key,
            label: c.label,
            available: c.available,
        })),
        Err(error) => notice = Some(error.to_string()),
    }
    if let Some(id) = saved.filter(|id| !choices.iter().any(|c| c.id == *id)) {
        choices.push(Choice {
            id: id.into(),
            label: format!("{} (Unavailable)", version_label(id)),
            available: false,
        });
    }
    (choices, notice)
}

pub fn version_label(key: &str) -> &str {
    match key {
        "standard" => "Standard",
        "ra" => "RetroAchievements",
        other => other,
    }
}

fn set_choice(
    choices: &mut BTreeMap<String, BTreeMap<String, String>>,
    system_id: &str,
    launch_key: &str,
    value: Option<&str>,
) {
    if let Some(value) = value {
        choices
            .entry(system_id.to_string())
            .or_default()
            .insert(launch_key.to_string(), value.to_string());
        return;
    }
    let remove_system = choices.get_mut(system_id).is_some_and(|games| {
        games.remove(launch_key);
        games.is_empty()
    });
    if remove_system {
        choices.remove(system_id);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn identities_cover_files_zip_members_and_title_launches() {
        assert_eq!(
            key(&Launch::File(PathBuf::from(
                "/media/fat/games/NES/./Collection.zip/Folder/Game.nes"
            ))),
            "f:/media/fat/games/NES/Collection.zip/Folder/Game.nes"
        );
        assert_eq!(
            key(&Launch::AmigaVision {
                install: PathBuf::from("/media/fat/games/AmigaVision"),
                title: "The Settlers".into(),
            }),
            "a:28:/media/fat/games/AmigaVisionThe Settlers"
        );

        let title_delimiter = Launch::AmigaVision {
            install: PathBuf::from("/games/A"),
            title: "B|C".into(),
        };
        let path_delimiter = Launch::AmigaVision {
            install: PathBuf::from("/games/A|B"),
            title: "C".into(),
        };
        assert_ne!(key(&title_delimiter), key(&path_delimiter));
    }

    #[test]
    fn game_choice_wins_and_clearing_last_choice_removes_the_system_map() {
        let launch = Launch::File(PathBuf::from("/games/N64/Game.z64"));
        let mut settings = Settings::default();
        settings.launch_cores.insert("N64".into(), "primary".into());
        assert_eq!(effective(&settings, "N64", &launch), Some("primary"));

        let launch_key = key(&launch);
        set(&mut settings, "N64", &launch_key, Some("n64-80mhz"));
        assert_eq!(effective(&settings, "N64", &launch), Some("n64-80mhz"));
        assert_eq!(
            effective(
                &settings,
                "N64",
                &Launch::File(PathBuf::from("/games/N64/Renamed.z64"))
            ),
            Some("primary"),
            "a renamed or moved game no longer matches the stale override"
        );

        set(&mut settings, "N64", &launch_key, None);
        assert!(!settings.game_launch_cores.contains_key("N64"));
        assert_eq!(effective(&settings, "N64", &launch), Some("primary"));
    }
}
