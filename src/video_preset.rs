//! General MiSTer video presets offered for Degauss's framebuffer.

use std::fs;
#[cfg(any(target_os = "linux", all(test, unix)))]
use std::io::Write;
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::error::{DegaussError, Result};

// MiSTer's preset collection also includes core-specific and display-specific
// game configurations. Only these general framebuffer effects belong in the
// Degauss menu. A preset is offered only while its own files are installed.
const GENERAL_PRESETS: [&str; 5] = [
    "Display Specific/Sony PVM.ini",
    "Interpolation Only.ini",
    "Scanlines - Medium.ini",
    "Scanlines - Sharp.ini",
    "Scanlines - Soft.ini",
];
const CUSTOM_PRESETS_DIR: &str = "Degauss";

static REQUEST_SEQUENCE: AtomicU32 = AtomicU32::new(0);

fn next_request_token() -> u32 {
    const FIRST: u32 = 100_000_000;
    const RANGE: u32 = 900_000_000;
    let sequence = REQUEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    FIRST
        + std::process::id()
            .wrapping_mul(1_000_003)
            .wrapping_add(sequence)
            % RANGE
}

#[cfg(target_os = "linux")]
fn remove_if_present(path: &Path) -> std::io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(any(target_os = "linux", all(test, unix)))]
fn send_request(fifo: &Path, request: &[u8]) -> std::io::Result<()> {
    let mut options = fs::OpenOptions::new();
    options.write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    options.open(fifo)?.write_all(request)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub relative_path: String,
}

pub fn discover(root: &Path) -> Result<Vec<Preset>> {
    let mut presets = Vec::new();
    let Some(menu_root) = root.parent() else {
        return Ok(presets);
    };
    for relative in GENERAL_PRESETS {
        let path = root.join(relative);
        let contents = match fs::read_to_string(&path) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(DegaussError::io("reading MiSTer preset", &path, error)),
        };
        if preset_components_present(menu_root, &contents) {
            presets.push(Preset {
                name: relative[..relative.len() - 4].to_string(),
                relative_path: relative.to_string(),
            });
        }
    }
    let custom_dir = root.join(CUSTOM_PRESETS_DIR);
    let entries = match fs::read_dir(&custom_dir) {
        Ok(entries) => Some(entries),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(DegaussError::io(
                "reading custom video presets",
                &custom_dir,
                error,
            ))
        }
    };
    if let Some(entries) = entries {
        for entry in entries {
            let entry = entry.map_err(|error| {
                DegaussError::io("reading custom video preset entry", &custom_dir, error)
            })?;
            let path = entry.path();
            if !path.is_file()
                || !path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("ini"))
            {
                continue;
            }
            let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let relative = format!("{CUSTOM_PRESETS_DIR}/{file_name}");
            if !valid_relative_path(&relative) {
                continue;
            }
            let contents = fs::read_to_string(&path)
                .map_err(|error| DegaussError::io("reading custom video preset", &path, error))?;
            if preset_components_present(menu_root, &contents) {
                presets.push(Preset {
                    name: format!("Custom/{}", &file_name[..file_name.len() - 4]),
                    relative_path: relative,
                });
            }
        }
    }
    presets.sort_by_key(|item| item.name.to_lowercase());
    Ok(presets)
}

fn preset_components_present(menu_root: &Path, contents: &str) -> bool {
    let mut has_effect = false;
    for line in contents.lines() {
        let Some((key, value)) = line.trim().split_once('=') else {
            continue;
        };
        let folder = match key.trim().to_ascii_lowercase().as_str() {
            "hfilter" | "vfilter" | "sfilter" | "ifilter" => "filters",
            "gamma" => "gamma",
            "mask" => "shadow_masks",
            "maskmode" => {
                has_effect = true;
                continue;
            }
            _ => continue,
        };
        has_effect = true;
        let value = value.trim();
        if value.eq_ignore_ascii_case("off")
            || value.eq_ignore_ascii_case("same")
            || value.eq_ignore_ascii_case("none")
        {
            continue;
        }
        if value.is_empty()
            || value.starts_with('/')
            || value.contains("..")
            || value.contains('\\')
            || !menu_root.join(folder).join(value).is_file()
        {
            return false;
        }
    }
    has_effect
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
    use std::time::Duration;

    if relative_path.is_some_and(|path| !valid_relative_path(path)) {
        return Err(DegaussError::unsupported(
            "MiSTer video preset",
            "invalid preset path",
        ));
    }
    let token = next_request_token();
    let status = PathBuf::from(format!("/tmp/degauss-preset-{token}.status"));
    remove_if_present(&status).map_err(|error| {
        DegaussError::io("cleaning stale MiSTer video preset status", &status, error)
    })?;
    let name = relative_path.unwrap_or("off");
    send_request(fifo, format!("fb_preset {token} {name}\n").as_bytes())
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
    let _ = remove_if_present(&status);
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
    fn offers_only_general_presets_with_installed_components() {
        let root = std::env::temp_dir().join(format!("degauss-presets-{}", std::process::id()));
        let presets_root = root.join("Presets");
        fs::create_dir_all(presets_root.join("Display Specific")).unwrap();
        fs::create_dir_all(presets_root.join("Core Specific")).unwrap();
        fs::create_dir_all(root.join("gamma")).unwrap();
        fs::write(
            presets_root.join("Display Specific/Sony PVM.ini"),
            "gamma=Pure_Gamma/gamma_110.txt\n",
        )
        .unwrap();
        fs::write(presets_root.join("Core Specific/NES.ini"), "gamma=off\n").unwrap();
        fs::write(presets_root.join("Display Specific/JVC.ini"), "gamma=off\n").unwrap();
        fs::write(
            presets_root.join("Scanlines - Soft.ini"),
            "mask=missing.txt\n",
        )
        .unwrap();
        assert!(discover(&presets_root).unwrap().is_empty());
        fs::create_dir_all(root.join("gamma/Pure_Gamma")).unwrap();
        fs::write(root.join("gamma/Pure_Gamma/gamma_110.txt"), "curve").unwrap();
        let presets = discover(&presets_root).unwrap();
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "Display Specific/Sony PVM");
        assert_eq!(presets[0].relative_path, "Display Specific/Sony PVM.ini");
        fs::create_dir_all(root.join("shadow_masks")).unwrap();
        fs::write(root.join("shadow_masks/missing.txt"), "mask").unwrap();
        assert_eq!(discover(&presets_root).unwrap().len(), 2);
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

    #[test]
    fn request_tokens_are_distinct_and_in_mains_supported_range() {
        let first = next_request_token();
        let second = next_request_token();
        assert_ne!(first, second);
        assert!((100_000_000..1_000_000_000).contains(&first));
        assert!((100_000_000..1_000_000_000).contains(&second));
    }

    #[cfg(unix)]
    #[test]
    fn request_does_not_block_when_fifo_has_no_reader() {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        use std::time::{Duration, SystemTime, UNIX_EPOCH};

        let path = std::env::temp_dir().join(format!(
            "degauss-preset-no-reader-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let native = CString::new(path.as_os_str().as_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(native.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        let error = send_request(&path, b"fb_preset 123456789 off\n").unwrap_err();
        fs::remove_file(&path).unwrap();

        assert_eq!(error.raw_os_error(), Some(libc::ENXIO));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn offers_user_presets_only_from_dedicated_folder_with_installed_components() {
        let root =
            std::env::temp_dir().join(format!("degauss-custom-presets-{}", std::process::id()));
        let presets_root = root.join("Presets");
        fs::create_dir_all(presets_root.join("Degauss")).unwrap();
        fs::create_dir_all(presets_root.join("Core Specific")).unwrap();
        fs::create_dir_all(root.join("gamma")).unwrap();
        fs::write(
            presets_root.join("Degauss/My Tube.ini"),
            "gamma=gamma_110.txt\n",
        )
        .unwrap();
        fs::write(
            presets_root.join("Degauss/Incomplete.ini"),
            "mask=missing.txt\n",
        )
        .unwrap();
        fs::write(presets_root.join("Core Specific/NES.ini"), "gamma=off\n").unwrap();
        assert!(discover(&presets_root).unwrap().is_empty());
        fs::write(root.join("gamma/gamma_110.txt"), "curve").unwrap();
        assert_eq!(
            discover(&presets_root).unwrap(),
            vec![Preset {
                name: "Custom/My Tube".to_string(),
                relative_path: "Degauss/My Tube.ini".to_string(),
            }]
        );
        fs::remove_dir_all(root).unwrap();
    }
}
