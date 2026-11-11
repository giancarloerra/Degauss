use sha2::{Digest, Sha256};
use std::path::Path;

use super::{gamelist_edit, targets, Error, ImagePolicy, Metadata, MetadataPolicy, Result};

pub const MAX_UPLOAD_BYTES: usize = 16 * 1024 * 1024;

pub fn validate_uploaded_image(bytes: &[u8]) -> Result<&'static str> {
    if bytes.is_empty() || bytes.len() > MAX_UPLOAD_BYTES {
        return Err(Error::local(
            "Choose a PNG or JPEG image no larger than 16 MiB.",
        ));
    }
    let extension = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        "png"
    } else if bytes.starts_with(&[0xff, 0xd8]) {
        "jpg"
    } else {
        return Err(Error::local("Only PNG and JPEG images can be uploaded."));
    };
    crate::covers::decode_bounded(bytes, Path::new("Uploaded artwork"), [0, 0, 0], 4096)
        .map_err(|error| Error::local(error.to_string()))?;
    Ok(extension)
}

/// Install only artwork and merge its reference with the production gamelist
/// writer. Existing metadata, ZIP ownership and automatic backups stay intact.
pub fn install_uploaded_image(
    systems: &[crate::systems::FoundSystem],
    system: &str,
    launch: &crate::browse::Launch,
    title: &str,
    body: &[u8],
    excluded: &std::collections::HashSet<String>,
) -> Result<Vec<String>> {
    let extension = validate_uploaded_image(body)?;
    let target = targets::local_image_target(systems, system, launch, title, excluded)?;
    let checksum: String = Sha256::digest(body)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let relative = format!("./media/degauss/{checksum}.{extension}");
    let path = target.folder.join(&relative);
    std::fs::create_dir_all(path.parent().expect("image parent"))
        .map_err(|error| Error::local(format!("Creating artwork folder failed: {error}")))?;
    if path.exists() {
        let previous = std::fs::read(&path)
            .map_err(|error| Error::local(format!("Reading existing artwork failed: {error}")))?;
        if previous != body {
            return Err(Error::local(
                "A content-named artwork file has different bytes.",
            ));
        }
    } else {
        crate::cache::write(&path, body).map_err(|error| Error::local(error.to_string()))?;
    }
    let update = gamelist_edit::Update {
        relative_game_path: target.relative_path.clone(),
        metadata: Metadata::default(),
        image_path: Some(relative),
        image_created: true,
        image_policy: ImagePolicy::ReplaceExisting,
        metadata_policy: MetadataPolicy::Off,
    };
    let outcomes = gamelist_edit::apply_many_with_fallback(
        &target.gamelist_path(),
        &target.folder,
        &[update],
        &[target.metadata_fallback],
        &mut gamelist_edit::Backups::new(),
    )?;
    for outcome in outcomes {
        outcome?;
    }
    Ok(target.affected_system_ids)
}
