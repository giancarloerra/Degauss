//! Sparse per-game compatible-core selections.
//!
//! The outer settings key is the owning system ID. The inner key is the
//! logical launch target, so the same choice follows a game through normal
//! browsing, Last Played and Degauss Favourites without indexing the library.

use std::path::{Component, Path};

use crate::browse::Launch;
use crate::settings::Settings;

/// Stable logical identity for one launch within its owning system.
pub fn key(launch: &Launch) -> String {
    match launch {
        Launch::File(path) => format!("f:{}", normalized_path(path)),
        Launch::AmigaVision { install, title } => {
            format!("a:{}|{title}", normalized_path(install))
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
    if let Some(profile) = profile {
        settings
            .game_launch_cores
            .entry(system_id.to_string())
            .or_default()
            .insert(launch_key.to_string(), profile.to_string());
        return;
    }
    let remove_system = settings
        .game_launch_cores
        .get_mut(system_id)
        .is_some_and(|games| {
            games.remove(launch_key);
            games.is_empty()
        });
    if remove_system {
        settings.game_launch_cores.remove(system_id);
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
            "a:/media/fat/games/AmigaVision|The Settlers"
        );
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
