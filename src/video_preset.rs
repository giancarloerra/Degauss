//! Installed MiSTer video presets offered for Degauss's framebuffer.

use std::fs;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;

use crate::error::{DegaussError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub relative_path: String,
}

pub fn discover(root: &Path) -> Result<Vec<Preset>> {
    let mut presets = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        let entries = match fs::read_dir(&folder) {
            Ok(entries) => entries,
            Err(error) if folder == root && error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(presets);
            }
            Err(error) => return Err(DegaussError::io("reading MiSTer presets", &folder, error)),
        };
        for entry in entries {
            let entry = entry
                .map_err(|error| DegaussError::io("reading MiSTer presets", &folder, error))?;
            let kind = entry.file_type().map_err(|error| {
                DegaussError::io("inspecting MiSTer preset", entry.path(), error)
            })?;
            if kind.is_dir() {
                folders.push(entry.path());
            } else if kind.is_file()
                && entry
                    .path()
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
            {
                let path = entry.path();
                let Ok(relative) = path.strip_prefix(root) else {
                    continue;
                };
                let relative = relative.to_string_lossy().replace('\\', "/");
                if !valid_relative_path(&relative) {
                    continue;
                }
                let name = relative[..relative.len() - 4].to_string();
                presets.push(Preset {
                    name,
                    relative_path: relative,
                });
            }
        }
    }
    presets.sort_by_key(|item| item.name.to_lowercase());
    Ok(presets)
}

pub fn valid_relative_path(path: &str) -> bool {
    !path.is_empty()
        && path.len() < 900
        && !path.starts_with('/')
        && !path.contains("..")
        && !path.chars().any(|ch| ch.is_control() || ch == '\\')
        && path.to_ascii_lowercase().ends_with(".ini")
}

#[cfg(target_os = "linux")]
pub fn apply(relative_path: Option<&str>, fifo: &Path) -> Result<()> {
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    if relative_path.is_some_and(|path| !valid_relative_path(path)) {
        return Err(DegaussError::unsupported(
            "MiSTer video preset",
            "invalid preset path",
        ));
    }
    let token = ((SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| DegaussError::unsupported("MiSTer video preset", error.to_string()))?
        .as_nanos()
        % 900_000_000) as u32)
        + 100_000_000;
    let status = PathBuf::from(format!("/tmp/degauss-preset-{token}.status"));
    if status.exists() {
        return Err(DegaussError::unsupported(
            "MiSTer video preset",
            "request status path already exists",
        ));
    }
    let name = relative_path.unwrap_or("off");
    fs::write(fifo, format!("fb_preset {token} {name}\n"))
        .map_err(|error| DegaussError::io("requesting MiSTer video preset", fifo, error))?;
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while std::time::Instant::now() < deadline {
        match fs::read_to_string(&status) {
            Ok(response) => {
                fs::remove_file(&status).map_err(|error| {
                    DegaussError::io("cleaning MiSTer video preset status", &status, error)
                })?;
                if response == "ok\n" {
                    return Ok(());
                }
                return Err(DegaussError::unsupported(
                    "MiSTer video preset",
                    response
                        .trim()
                        .strip_prefix("error ")
                        .unwrap_or("invalid Main response"),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(error) => {
                return Err(DegaussError::io(
                    "reading MiSTer video preset status",
                    &status,
                    error,
                ));
            }
        }
    }
    Err(DegaussError::unsupported(
        "MiSTer video preset",
        "Main did not confirm the preset. Check that the installed Degauss Main and Menu core support video presets",
    ))
}

#[cfg(not(target_os = "linux"))]
pub fn apply(_relative_path: Option<&str>, _fifo: &Path) -> Result<()> {
    Err(DegaussError::unsupported(
        "MiSTer video preset",
        "only available on MiSTer",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_nested_native_presets_without_following_links() {
        let root = std::env::temp_dir().join(format!("degauss-presets-{}", std::process::id()));
        fs::create_dir_all(root.join("Display Specific")).unwrap();
        fs::write(root.join("Display Specific/Sony PVM.ini"), "gamma=off\n").unwrap();
        fs::write(root.join("Other.txt"), "").unwrap();
        let presets = discover(&root).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Display Specific/Sony PVM");
        assert_eq!(presets[0].relative_path, "Display Specific/Sony PVM.ini");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_paths_that_could_escape_presets_or_break_the_command() {
        for invalid in [
            "../other.ini",
            "/absolute.ini",
            "Name\nOff.ini",
            "not-a-preset.txt",
        ] {
            assert!(!valid_relative_path(invalid));
        }
    }
}
