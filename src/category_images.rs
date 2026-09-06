//! User-selected artwork for category and system rows.
//!
//! The files in `logos` remain source material. A selection is copied into
//! a managed subfolder so clearing it never deletes or overwrites a shipped
//! logo or a file the user added by hand.

use std::path::{Path, PathBuf};

use crate::covers::{self, CoverStats};
use crate::error::{DegaussError, Result};

const CATEGORY_OVERRIDE_DIR: &str = ".category-images";
const SYSTEM_OVERRIDE_DIR: &str = ".system-images";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub label: String,
    pub path: PathBuf,
}

/// Every PNG or JPEG directly inside the ordinary logos folder.
pub fn choices(logo_dir: &Path) -> Result<Vec<Choice>> {
    let entries = std::fs::read_dir(logo_dir)
        .map_err(|error| DegaussError::io("reading the logos folder", logo_dir, error))?;
    let mut choices = Vec::new();
    for entry in entries {
        let entry =
            entry.map_err(|error| DegaussError::io("reading the logos folder", logo_dir, error))?;
        let path = entry.path();
        if !path.is_file() || !supported_extension(&path) {
            continue;
        }
        choices.push(Choice {
            label: entry.file_name().to_string_lossy().into_owned(),
            path,
        });
    }
    choices.sort_by(|left, right| {
        left.label
            .to_lowercase()
            .cmp(&right.label.to_lowercase())
            .then_with(|| left.label.cmp(&right.label))
    });
    Ok(choices)
}

/// The effective fixed category image. A UI selection wins; removing it
/// reveals the long-standing `Category.png`/`.jpg` convention unchanged.
pub fn fixed_image(logo_dir: &Path, category: &str) -> Option<PathBuf> {
    let selected = category_override_path(logo_dir, category);
    if selected.is_file() {
        return Some(selected);
    }
    ["png", "jpg"].iter().find_map(|extension| {
        let path = logo_dir.join(format!("{category}.{extension}"));
        path.is_file().then_some(path)
    })
}

pub fn has_override(logo_dir: &Path, category: &str) -> bool {
    category_override_path(logo_dir, category).is_file()
}

/// Validate and copy one source without touching it. The destination name is
/// derived only from encoded category bytes, never trusted path characters.
pub fn install(logo_dir: &Path, category: &str, source: &Path) -> Result<PathBuf> {
    install_override(
        logo_dir,
        category_override_path(logo_dir, category),
        source,
        "category image",
    )
}

/// A UI-selected image for a system. The traditional `<system id>.png` or
/// `.jpg` file remains the fallback owned by `FoundSystem::logo`; keeping
/// the managed selection separate makes clearing non-destructive.
pub fn system_image(logo_dir: &Path, system_id: &str) -> Option<PathBuf> {
    let selected = system_override_path(logo_dir, system_id);
    selected.is_file().then_some(selected)
}

pub fn has_system_override(logo_dir: &Path, system_id: &str) -> bool {
    system_override_path(logo_dir, system_id).is_file()
}

pub fn install_system(logo_dir: &Path, system_id: &str, source: &Path) -> Result<PathBuf> {
    install_override(
        logo_dir,
        system_override_path(logo_dir, system_id),
        source,
        "system image",
    )
}

fn install_override(
    logo_dir: &Path,
    destination: PathBuf,
    source: &Path,
    description: &'static str,
) -> Result<PathBuf> {
    if source.parent() != Some(logo_dir) || !supported_extension(source) {
        return Err(DegaussError::unsupported(
            description,
            "the selected file is not a PNG or JPEG in the logos folder",
        ));
    }

    // Refuse a corrupt or excessively large file before replacing a working
    // selection. One-pixel scaling keeps only the validation result in RAM.
    let mut stats = CoverStats::default();
    covers::load_scaled(source, 1, [0, 0, 0], &mut stats)?;

    let parent = destination
        .parent()
        .expect("a managed image override always has a parent");
    std::fs::create_dir_all(parent)
        .map_err(|error| DegaussError::io("creating custom image storage", parent, error))?;

    // Copy beside the destination and atomically replace the live file with
    // the complete copy. If the final move fails, the previous selection is
    // retained and a partial override is never left under the live name.
    let temporary = destination.with_extension("part");
    let _ = std::fs::remove_file(&temporary);
    if let Err(error) = std::fs::copy(source, &temporary) {
        let _ = std::fs::remove_file(&temporary);
        return Err(DegaussError::io(
            "copying the custom image",
            temporary,
            error,
        ));
    }
    if let Err(error) = std::fs::OpenOptions::new()
        .write(true)
        .open(&temporary)
        .and_then(|file| file.sync_all())
    {
        let _ = std::fs::remove_file(&temporary);
        return Err(DegaussError::io(
            "syncing the custom image",
            temporary,
            error,
        ));
    }
    std::fs::rename(&temporary, &destination).map_err(|error| {
        let _ = std::fs::remove_file(&temporary);
        DegaussError::io("saving the custom image", &destination, error)
    })?;
    Ok(destination)
}

/// Remove only Degauss's managed copy. The source image and any traditional
/// `Category.png`/`.jpg` file remain untouched.
pub fn clear(logo_dir: &Path, category: &str) -> Result<bool> {
    clear_override(category_override_path(logo_dir, category))
}

/// Remove only the picker-managed image for a system. Any directly named
/// system logo remains in place and becomes visible again.
pub fn clear_system(logo_dir: &Path, system_id: &str) -> Result<bool> {
    clear_override(system_override_path(logo_dir, system_id))
}

fn clear_override(path: PathBuf) -> Result<bool> {
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(DegaussError::io("clearing the custom image", path, error)),
    }
}

fn supported_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            extension.eq_ignore_ascii_case("png")
                || extension.eq_ignore_ascii_case("jpg")
                || extension.eq_ignore_ascii_case("jpeg")
        })
}

fn encoded_path(logo_dir: &Path, directory: &str, key: &str) -> PathBuf {
    let mut encoded = String::with_capacity(key.len() * 2);
    for byte in key.as_bytes() {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    logo_dir.join(directory).join(encoded)
}

fn category_override_path(logo_dir: &Path, category: &str) -> PathBuf {
    encoded_path(logo_dir, CATEGORY_OVERRIDE_DIR, category)
}

fn system_override_path(logo_dir: &Path, system_id: &str) -> PathBuf {
    encoded_path(logo_dir, SYSTEM_OVERRIDE_DIR, system_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn directory(tag: &str) -> PathBuf {
        let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "degauss-category-images-{tag}-{}-{sequence}",
            std::process::id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn png() -> &'static [u8] {
        include_bytes!("../assets/logos/Arcade.png")
    }

    fn other_png() -> &'static [u8] {
        include_bytes!("../assets/logos/Arduboy.png")
    }

    #[test]
    fn picker_lists_shipped_and_user_images_but_not_other_files_or_subfolders() {
        let dir = directory("choices");
        std::fs::write(dir.join("z-user.JPG"), png()).unwrap();
        std::fs::write(dir.join("m-user.JPEG"), png()).unwrap();
        std::fs::write(dir.join("A-shipped.png"), png()).unwrap();
        std::fs::write(dir.join("notes.txt"), b"not an image").unwrap();
        std::fs::create_dir(dir.join("nested.jpg")).unwrap();

        let labels: Vec<_> = choices(&dir)
            .unwrap()
            .into_iter()
            .map(|choice| choice.label)
            .collect();
        assert_eq!(labels, ["A-shipped.png", "m-user.JPEG", "z-user.JPG"]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn long_jpeg_extension_can_be_selected_and_installed() {
        let dir = directory("jpeg-extension");
        let source = dir.join("user-art.jpeg");
        std::fs::write(&source, png()).unwrap();

        let selected = install(&dir, "Arcade", &source).unwrap();
        assert_eq!(std::fs::read(&selected).unwrap(), png());
        assert_eq!(std::fs::read(&source).unwrap(), png());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn selection_is_a_copy_and_clear_restores_the_existing_category_file() {
        let dir = directory("roundtrip");
        let source = dir.join("C64.png");
        let replacement = dir.join("Arduboy.png");
        let original = dir.join("Arcade.png");
        std::fs::write(&source, png()).unwrap();
        std::fs::write(&replacement, other_png()).unwrap();
        std::fs::write(&original, b"original category art").unwrap();

        let selected = install(&dir, "Arcade", &source).unwrap();
        assert_eq!(fixed_image(&dir, "Arcade"), Some(selected.clone()));
        assert_eq!(std::fs::read(&source).unwrap(), png());
        assert_eq!(std::fs::read(&selected).unwrap(), png());

        assert_eq!(install(&dir, "Arcade", &replacement).unwrap(), selected);
        assert_eq!(std::fs::read(&selected).unwrap(), other_png());
        assert_eq!(std::fs::read(&replacement).unwrap(), other_png());

        assert!(clear(&dir, "Arcade").unwrap());
        assert_eq!(fixed_image(&dir, "Arcade"), Some(original));
        assert!(source.is_file(), "clearing must not delete the source");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn invalid_replacement_does_not_discard_the_working_selection() {
        let dir = directory("invalid");
        let source = dir.join("valid.png");
        let invalid = dir.join("invalid.jpg");
        std::fs::write(&source, png()).unwrap();
        std::fs::write(&invalid, b"not a picture").unwrap();
        let selected = install(&dir, "Console", &source).unwrap();
        let before = std::fs::read(&selected).unwrap();

        assert!(install(&dir, "Console", &invalid).is_err());
        assert_eq!(std::fs::read(selected).unwrap(), before);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn category_text_cannot_escape_the_managed_folder() {
        let dir = Path::new("/logos");
        let path = category_override_path(dir, "../../Other/\u{2603}");
        assert_eq!(
            path.parent(),
            Some(dir.join(CATEGORY_OVERRIDE_DIR).as_path())
        );
        assert!(!path.to_string_lossy().contains(".."));
    }

    #[test]
    fn system_selection_is_isolated_and_clear_restores_the_named_logo() {
        let dir = directory("system-roundtrip");
        let source = dir.join("Arduboy.png");
        let original = dir.join("Amiga.png");
        std::fs::write(&source, other_png()).unwrap();
        std::fs::write(&original, png()).unwrap();

        let selected = install_system(&dir, "Amiga", &source).unwrap();
        assert_eq!(system_image(&dir, "Amiga"), Some(selected.clone()));
        assert_eq!(std::fs::read(&selected).unwrap(), other_png());
        assert_eq!(std::fs::read(&source).unwrap(), other_png());
        assert!(!has_override(&dir, "Amiga"));
        assert_eq!(fixed_image(&dir, "Amiga"), Some(original.clone()));

        assert!(clear_system(&dir, "Amiga").unwrap());
        assert_eq!(system_image(&dir, "Amiga"), None);
        assert!(original.is_file());
        assert!(source.is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn system_id_cannot_escape_the_managed_folder() {
        let dir = Path::new("/logos");
        let path = system_override_path(dir, "../../Games/\u{2603}");
        assert_eq!(path.parent(), Some(dir.join(SYSTEM_OVERRIDE_DIR).as_path()));
        assert!(!path.to_string_lossy().contains(".."));
    }
}
