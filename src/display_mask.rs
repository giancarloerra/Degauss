//! MiSTer shadow-mask files offered while the Degauss framebuffer is open.

use std::path::{Path, PathBuf};

use crate::error::{DegaussError, Result};

pub const LEGACY_SCANLINES: &str = "Scanlines";

pub fn names(dir: &Path) -> Result<Vec<String>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(DegaussError::io("reading display masks", dir, error)),
    };
    let mut names = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| DegaussError::io("reading display masks", dir, error))?;
        let path = entry.path();
        if !path.is_file() || !path.extension().is_some_and(|ext| ext == "txt") {
            continue;
        }
        let Some(name) = path.file_stem().and_then(|stem| stem.to_str()) else {
            continue;
        };
        if valid_name(name) {
            names.push(name.to_string());
        }
    }
    names.sort_by_key(|name| name.to_lowercase());
    Ok(names)
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.eq_ignore_ascii_case("off")
        && !name.contains("..")
        && !name
            .chars()
            .any(|ch| ch.is_control() || ch == '/' || ch == '\\')
}

/// Check the selected file before telling Main to replace the active mask.
/// MiSTer's native text reader ignores blank lines and lines starting with
/// `#` or `;`; each resolution block is a v1/v2 1..16 by 1..16 LUT.
pub fn validate(dir: &Path, name: &str) -> Result<PathBuf> {
    if !valid_name(name) {
        return Err(DegaussError::unsupported(
            "display mask name",
            "use a plain file name of at most 128 bytes".to_string(),
        ));
    }
    let path = dir.join(format!("{name}.txt"));
    let content = std::fs::read_to_string(&path)
        .map_err(|error| DegaussError::io("reading display mask", &path, error))?;
    let lines: Vec<&str> = content
        .lines()
        .map(|line| line.trim_start_matches([' ', '\t']))
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with(';'))
        .collect();
    let mut at = 0;
    let mut blocks = 0;
    while at < lines.len() {
        if lines[at].to_ascii_lowercase().starts_with("resolution=") {
            let threshold = lines[at][11..].trim_ascii().parse::<u32>().ok();
            if threshold.is_none_or(|threshold| threshold == 0) {
                return Err(DegaussError::malformed(
                    "display mask",
                    &path,
                    "invalid resolution",
                ));
            }
            at += 1;
        }
        let v2 = lines
            .get(at)
            .is_some_and(|line| line.eq_ignore_ascii_case("v2"));
        if v2 {
            at += 1;
        }
        let Some(dimensions) = lines.get(at) else {
            return Err(DegaussError::malformed(
                "display mask",
                &path,
                "missing dimensions",
            ));
        };
        let Some((width, height)) = dimensions.split_once(',') else {
            return Err(DegaussError::malformed(
                "display mask",
                &path,
                "invalid dimensions",
            ));
        };
        // MiSTer's comma-delimited reader does not accept whitespace before
        // a comma, even though it accepts whitespace after one.
        if width.trim_ascii_end() != width {
            return Err(DegaussError::malformed(
                "display mask",
                &path,
                "invalid dimensions",
            ));
        }
        let (Ok(width), Ok(height)) = (
            width.trim_ascii().parse::<usize>(),
            height.trim_ascii().parse::<usize>(),
        ) else {
            return Err(DegaussError::malformed(
                "display mask",
                &path,
                "invalid dimensions",
            ));
        };
        if !(1..=16).contains(&width) || !(1..=16).contains(&height) {
            return Err(DegaussError::malformed(
                "display mask",
                &path,
                "dimensions must be 1..16",
            ));
        }
        at += 1;
        for _ in 0..height {
            let Some(row) = lines.get(at) else {
                return Err(DegaussError::malformed(
                    "display mask",
                    &path,
                    "missing mask row",
                ));
            };
            let values: Vec<_> = row.split(',').collect();
            if values.len() != width
                || values[..values.len().saturating_sub(1)]
                    .iter()
                    .any(|value| value.trim_ascii_end() != *value)
                || values.iter().any(|value| {
                    u16::from_str_radix(value.trim_ascii(), 16).is_err()
                        || u16::from_str_radix(value.trim_ascii(), 16)
                            .is_ok_and(|value| value > if v2 { 0x7ff } else { 7 })
                })
            {
                return Err(DegaussError::malformed(
                    "display mask",
                    &path,
                    "invalid mask row",
                ));
            }
            at += 1;
        }
        blocks += 1;
    }
    if blocks == 0 {
        return Err(DegaussError::malformed("display mask", &path, "empty file"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_masks_follow_the_native_format() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets/masks");
        let names = names(&dir).unwrap();
        assert!(names.iter().any(|name| name == LEGACY_SCANLINES));
        assert!(names.len() >= 3);
        for name in names {
            validate(&dir, &name).unwrap();
        }
    }

    #[test]
    fn invalid_custom_file_cannot_be_applied() {
        let dir = std::env::temp_dir().join(format!("degauss-mask-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Broken.txt");
        std::fs::write(&file, "v2\n2,2\n70f,008\n").unwrap();
        assert!(validate(&dir, "Broken").is_err());
        assert!(validate(&dir, "../Broken").is_err());
        std::fs::remove_file(file).unwrap();
        std::fs::remove_dir(dir).unwrap();
    }

    #[test]
    fn discovery_only_offers_files_main_can_open() {
        let dir = std::env::temp_dir().join(format!("degauss-mask-case-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lower.txt"), "v2\n1,1\n70f\n").unwrap();
        std::fs::write(dir.join("upper.TXT"), "v2\n1,1\n70f\n").unwrap();
        assert_eq!(names(&dir).unwrap(), vec!["lower"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn validation_matches_main_comma_parsing() {
        let dir = std::env::temp_dir().join(format!("degauss-mask-fields-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Custom.txt");
        std::fs::write(&file, "v2\n2, 1\n70f, 70f\n").unwrap();
        validate(&dir, "Custom").unwrap();
        std::fs::write(&file, "v2\n2 ,1\n70f,70f\n").unwrap();
        assert!(validate(&dir, "Custom").is_err());
        std::fs::write(&file, "v2\n2,1\n70f ,70f\n").unwrap();
        assert!(validate(&dir, "Custom").is_err());
        std::fs::write(&file, "v2\n2,\u{a0}1\n70f,70f\n").unwrap();
        assert!(validate(&dir, "Custom").is_err());
        std::fs::write(&file, "v2\n2,1\n70f,\u{a0}70f\n").unwrap();
        assert!(validate(&dir, "Custom").is_err());
        std::fs::write(&file, "v2 \n2,1\n70f,70f\n").unwrap();
        assert!(validate(&dir, "Custom").is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
