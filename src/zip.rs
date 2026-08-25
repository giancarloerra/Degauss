//! Listing what is inside a zip, without unpacking any of it.
//!
//! Plenty of MiSTer libraries keep games zipped, one game per archive or
//! several. Indexing the contents rather than the archive is what stops a
//! folder of zips looking empty to anything that only sees the `.zip`.
//! MiSTer itself launches a file inside an archive perfectly well: the path
//! `something.zip/inner.rom` is understood by its own loader.
//!
//! So Degauss needs the names inside an archive and nothing else. That is
//! the central directory, a plain table at the end of the file, and reading
//! it needs no decompression and therefore no compression library: a listing
//! is a few seeks and some little-endian integers. Unpacking is MiSTer's job
//! at launch time, not ours at scan time.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

use crate::error::{DegaussError, Result};

/// End of central directory record.
const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
/// Central directory file header.
const CENTRAL_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x01, 0x02];

/// A zip comment can be 64 KiB, and the record we want sits just before it,
/// so that is how far back it is worth looking.
const MAX_TRAILER: usize = 66_000;

/// Refuse absurd archives rather than allocating whatever a corrupt header
/// claims. No real game archive holds a hundred thousand files.
const MAX_ENTRIES: usize = 100_000;

fn u16_at(bytes: &[u8], offset: usize) -> Option<usize> {
    let slice = bytes.get(offset..offset + 2)?;
    Some(u16::from_le_bytes([slice[0], slice[1]]) as usize)
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u64> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]) as u64)
}

/// The names of the files inside an archive, in the order the archive lists
/// them. Directory entries are left out: only files can be launched.
///
/// A damaged archive yields an error rather than an empty list, so a corrupt
/// file is never mistaken for an empty one.
pub fn list(path: &Path) -> Result<Vec<String>> {
    let mut file =
        std::fs::File::open(path).map_err(|e| DegaussError::io("opening archive", path, e))?;
    let size = file
        .metadata()
        .map_err(|e| DegaussError::io("reading archive size", path, e))?
        .len();

    // Read the tail and find the end-of-directory record inside it.
    let tail_len = MAX_TRAILER.min(size as usize);
    let tail_start = size - tail_len as u64;
    file.seek(SeekFrom::Start(tail_start))
        .map_err(|e| DegaussError::io("seeking archive", path, e))?;
    let mut tail = vec![0u8; tail_len];
    file.read_exact(&mut tail)
        .map_err(|e| DegaussError::io("reading archive", path, e))?;

    let eocd = tail
        .windows(4)
        .rposition(|w| w == EOCD_SIGNATURE)
        .ok_or_else(|| {
            DegaussError::malformed("zip archive", path, "no end-of-directory record")
        })?;

    let entry_count = u16_at(&tail, eocd + 10).ok_or_else(|| {
        DegaussError::malformed("zip archive", path, "truncated directory record")
    })?;
    let directory_offset = u32_at(&tail, eocd + 16).ok_or_else(|| {
        DegaussError::malformed("zip archive", path, "truncated directory record")
    })?;

    if entry_count > MAX_ENTRIES {
        return Err(DegaussError::unsupported(
            "zip archive",
            format!("{} claims {entry_count} entries", path.display()),
        ));
    }

    // Both of these are numbers taken straight out of the file, so both are
    // checked before they are used to index or to allocate. A corrupt one is
    // a malformed archive, never a panic: this process is the whole menu, and
    // the release profile aborts rather than unwinding.
    if directory_offset >= size {
        return Err(DegaussError::malformed(
            "zip archive",
            path,
            "directory offset past the end of the file",
        ));
    }
    let directory_size = u32_at(&tail, eocd + 12).ok_or_else(|| {
        DegaussError::malformed("zip archive", path, "truncated directory record")
    })?;
    let directory_size = directory_size.min(size - directory_offset);

    // The directory may already be inside the tail we hold; if not, read it.
    // Reading only as far as the record says stops a wrong offset pulling a
    // whole archive into memory.
    let directory = if directory_offset >= tail_start {
        let start = (directory_offset - tail_start) as usize;
        tail.get(start..)
            .ok_or_else(|| {
                DegaussError::malformed("zip archive", path, "directory offset outside the file")
            })?
            .to_vec()
    } else {
        file.seek(SeekFrom::Start(directory_offset))
            .map_err(|e| DegaussError::io("seeking archive directory", path, e))?;
        let mut buffer = Vec::new();
        file.take(directory_size)
            .read_to_end(&mut buffer)
            .map_err(|e| DegaussError::io("reading archive directory", path, e))?;
        buffer
    };

    let mut names = Vec::with_capacity(entry_count.min(1024));
    let mut cursor = 0usize;
    // Counted separately from `names`, which leaves directory entries out: the
    // record says how many headers there are, and every one of them must be
    // found or the directory is damaged.
    let mut seen = 0usize;
    while seen < entry_count {
        if directory.get(cursor..cursor + 4) != Some(&CENTRAL_SIGNATURE) {
            return Err(DegaussError::malformed(
                "zip archive",
                path,
                "directory ends before the entries it claims",
            ));
        }
        seen += 1;
        let name_len = u16_at(&directory, cursor + 28).ok_or_else(|| {
            DegaussError::malformed("zip archive", path, "truncated directory entry")
        })?;
        let extra_len = u16_at(&directory, cursor + 30).ok_or_else(|| {
            DegaussError::malformed("zip archive", path, "truncated directory entry")
        })?;
        let comment_len = u16_at(&directory, cursor + 32).ok_or_else(|| {
            DegaussError::malformed("zip archive", path, "truncated directory entry")
        })?;

        let name_start = cursor + 46;
        let name = directory
            .get(name_start..name_start + name_len)
            .ok_or_else(|| {
                DegaussError::malformed("zip archive", path, "directory entry runs past the end")
            })?;
        let name = String::from_utf8_lossy(name).into_owned();

        // A trailing slash marks a directory, which is not launchable.
        if !name.ends_with('/') && !name.is_empty() {
            names.push(name);
        }

        cursor = name_start + name_len + extra_len + comment_len;
    }

    Ok(names)
}

/// The test archive, shared with the catalog's tests so both exercise the
/// same real bytes rather than two hand-made approximations.
#[cfg(test)]
pub fn tests_fixture() -> &'static [u8] {
    tests::ZIP_FIXTURE
}

#[cfg(test)]
mod tests {
    use super::*;

    // Three files, one of them in a subfolder, built with a real zip writer.
    pub(super) const ZIP_FIXTURE: &[u8] = &[
        0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x6a, 0xaa, 0x17, 0x5d, 0xd2,
        0x8f, 0x34, 0xb5, 0x07, 0x00, 0x00, 0x00, 0x2c, 0x01, 0x00, 0x00, 0x0e, 0x00, 0x00, 0x00,
        0x4d, 0x65, 0x74, 0x61, 0x6c, 0x20, 0x53, 0x6c, 0x75, 0x67, 0x2e, 0x6e, 0x65, 0x6f, 0x63,
        0x60, 0x18, 0x05, 0xc4, 0x02, 0x00, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x6a, 0xaa, 0x17, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x07, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x72, 0x65, 0x61, 0x64, 0x6d, 0x65, 0x2e, 0x74, 0x78,
        0x74, 0xcb, 0x48, 0xcd, 0xc9, 0xc9, 0x07, 0x00, 0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00,
        0x00, 0x08, 0x00, 0x6a, 0xaa, 0x17, 0x5d, 0x15, 0x96, 0x2c, 0x6a, 0x06, 0x00, 0x00, 0x00,
        0x78, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, 0x73, 0x75, 0x62, 0x2f, 0x41, 0x6e, 0x6f,
        0x74, 0x68, 0x65, 0x72, 0x20, 0x47, 0x61, 0x6d, 0x65, 0x2e, 0x6e, 0x65, 0x6f, 0x63, 0x64,
        0x1c, 0x18, 0x00, 0x00, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x6a, 0xaa, 0x17, 0x5d, 0xd2, 0x8f, 0x34, 0xb5, 0x07, 0x00, 0x00, 0x00, 0x2c, 0x01,
        0x00, 0x00, 0x0e, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
        0x01, 0x00, 0x00, 0x00, 0x00, 0x4d, 0x65, 0x74, 0x61, 0x6c, 0x20, 0x53, 0x6c, 0x75, 0x67,
        0x2e, 0x6e, 0x65, 0x6f, 0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x08,
        0x00, 0x6a, 0xaa, 0x17, 0x5d, 0x86, 0xa6, 0x10, 0x36, 0x07, 0x00, 0x00, 0x00, 0x05, 0x00,
        0x00, 0x00, 0x0a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80,
        0x01, 0x33, 0x00, 0x00, 0x00, 0x72, 0x65, 0x61, 0x64, 0x6d, 0x65, 0x2e, 0x74, 0x78, 0x74,
        0x50, 0x4b, 0x01, 0x02, 0x14, 0x03, 0x14, 0x00, 0x00, 0x00, 0x08, 0x00, 0x6a, 0xaa, 0x17,
        0x5d, 0x15, 0x96, 0x2c, 0x6a, 0x06, 0x00, 0x00, 0x00, 0x78, 0x00, 0x00, 0x00, 0x14, 0x00,
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x80, 0x01, 0x62, 0x00, 0x00,
        0x00, 0x73, 0x75, 0x62, 0x2f, 0x41, 0x6e, 0x6f, 0x74, 0x68, 0x65, 0x72, 0x20, 0x47, 0x61,
        0x6d, 0x65, 0x2e, 0x6e, 0x65, 0x6f, 0x50, 0x4b, 0x05, 0x06, 0x00, 0x00, 0x00, 0x00, 0x03,
        0x00, 0x03, 0x00, 0xb6, 0x00, 0x00, 0x00, 0x9a, 0x00, 0x00, 0x00, 0x00, 0x00,
    ];

    fn write_fixture(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("degauss-zip-{tag}-{}.zip", std::process::id()));
        std::fs::write(&path, bytes).expect("fixture written");
        path
    }

    #[test]
    fn every_file_in_an_archive_is_listed_including_ones_in_subfolders() {
        let path = write_fixture("list", ZIP_FIXTURE);
        let names = list(&path).expect("archive lists");

        assert_eq!(
            names,
            vec![
                "Metal Slug.neo".to_string(),
                "readme.txt".to_string(),
                "sub/Another Game.neo".to_string()
            ]
        );
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_file_that_is_not_an_archive_is_an_error_not_an_empty_list() {
        // An empty list would look like an archive with nothing in it, and
        // the games inside would silently disappear from the library.
        let path = write_fixture("garbage", b"this is not a zip file at all");
        let err = list(&path).expect_err("must not accept garbage");
        assert!(err.to_string().contains("end-of-directory"), "got: {err}");
        std::fs::remove_file(&path).ok();
    }

    /// The offset of the central directory is a number taken straight out of
    /// the file. A corrupt one used to be sliced with, which panics, and a
    /// panic here is not an error the caller can handle: the release profile
    /// aborts, and this process is the whole menu. A file on the card must
    /// never be able to take the screen away.
    #[test]
    fn a_directory_offset_past_the_end_is_an_error_not_a_crash() {
        let mut bytes = ZIP_FIXTURE.to_vec();
        let eocd = bytes
            .windows(4)
            .rposition(|w| w == EOCD_SIGNATURE)
            .expect("fixture has an end-of-directory record");
        // Point the directory a byte past the end of the file.
        let past = (bytes.len() as u32) + 1;
        bytes[eocd + 16..eocd + 20].copy_from_slice(&past.to_le_bytes());
        let path = write_fixture("bad-offset", &bytes);
        let outcome = list(&path);
        let _ = std::fs::remove_file(&path);
        assert!(
            outcome.is_err(),
            "a directory offset past the end must be reported, got {outcome:?}"
        );
    }

    /// The same number, at its maximum, on a small file.
    #[test]
    fn a_wildly_wrong_directory_offset_is_an_error_not_a_crash() {
        let mut bytes = ZIP_FIXTURE.to_vec();
        let eocd = bytes
            .windows(4)
            .rposition(|w| w == EOCD_SIGNATURE)
            .expect("fixture has an end-of-directory record");
        bytes[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        let path = write_fixture("huge-offset", &bytes);
        let outcome = list(&path);
        let _ = std::fs::remove_file(&path);
        assert!(outcome.is_err(), "got {outcome:?}");
    }

    /// The module promises a damaged archive is an error, never an empty
    /// list: a folder of games must not quietly become a folder of nothing.
    #[test]
    fn a_damaged_directory_is_an_error_not_an_empty_list() {
        let mut bytes = ZIP_FIXTURE.to_vec();
        let first = bytes
            .windows(4)
            .position(|w| w == CENTRAL_SIGNATURE)
            .expect("fixture has a central directory");
        bytes[first] = 0x00; // break the first header's signature
        let path = write_fixture("damaged-directory", &bytes);
        let outcome = list(&path);
        let _ = std::fs::remove_file(&path);
        assert!(
            outcome.is_err(),
            "a damaged directory must be reported, got {outcome:?}"
        );
    }

    #[test]
    fn a_truncated_archive_is_reported() {
        let path = write_fixture("truncated", &ZIP_FIXTURE[..120]);
        assert!(list(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn a_missing_archive_names_the_file_it_could_not_open() {
        let err = list(Path::new("/definitely/not/here.zip")).expect_err("must fail");
        assert!(err.to_string().contains("here.zip"), "got: {err}");
    }
}
