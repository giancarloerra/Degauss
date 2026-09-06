use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use md5::{Digest, Md5};
use sha1::Sha1;

use super::{Error, ErrorKind, Result};

const BUFFER_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hashes {
    pub size: u64,
    pub crc32: String,
    pub md5: String,
    pub sha1: String,
}

pub fn file(path: &Path, maximum: u64, cancelled: &AtomicBool) -> Result<Option<Hashes>> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        Error::local(format!(
            "could not open {} for matching: {error}",
            display_name(path)
        ))
    })?;
    let size = file
        .metadata()
        .map_err(|error| {
            Error::local(format!(
                "could not read the size of {}: {error}",
                display_name(path)
            ))
        })?
        .len();
    if size > maximum {
        return Ok(None);
    }

    let mut crc = crc32fast::Hasher::new();
    let mut md5 = Md5::new();
    let mut sha1 = Sha1::new();
    let mut buffer = vec![0u8; BUFFER_BYTES];
    let mut read = 0u64;
    loop {
        if cancelled.load(Ordering::Relaxed) {
            return Err(Error::new(ErrorKind::Cancelled, "stopped while hashing"));
        }
        let count = file.read(&mut buffer).map_err(|error| {
            Error::local(format!(
                "could not read {} for matching: {error}",
                display_name(path)
            ))
        })?;
        if count == 0 {
            break;
        }
        read = read.saturating_add(count as u64);
        if read > maximum || read > size {
            return Err(Error::local(format!(
                "{} changed while it was being hashed",
                display_name(path)
            )));
        }
        crc.update(&buffer[..count]);
        md5.update(&buffer[..count]);
        sha1.update(&buffer[..count]);
    }
    if read != size {
        return Err(Error::local(format!(
            "{} changed while it was being hashed",
            display_name(path)
        )));
    }
    Ok(Some(Hashes {
        size,
        crc32: format!("{:08X}", crc.finalize()),
        md5: upper_hex(&md5.finalize()),
        sha1: upper_hex(&sha1.finalize()),
    }))
}

fn upper_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(output, "{byte:02X}");
    }
    output
}

fn display_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "game file".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "degauss-scraper-hash-{name}-{}",
            std::process::id()
        ));
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn all_three_hashes_and_size_are_streamed_exactly() {
        let path = temp("known", b"abc");
        let hashes = file(&path, 1024, &AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(hashes.size, 3);
        assert_eq!(hashes.crc32, "352441C2");
        assert_eq!(hashes.md5, "900150983CD24FB0D6963F7D28E17F72");
        assert_eq!(hashes.sha1, "A9993E364706816ABA3E25717850C26C9CD0D89D");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_file_over_the_ceiling_is_not_read() {
        let path = temp("large", b"four");
        assert_eq!(file(&path, 3, &AtomicBool::new(false)).unwrap(), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn cancellation_is_not_reported_as_a_missing_match() {
        let path = temp("cancel", b"abc");
        let error = file(&path, 1024, &AtomicBool::new(true)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Cancelled);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_read_failure_is_reported_instead_of_becoming_a_name_fallback() {
        let path = std::env::temp_dir().join(format!(
            "degauss-scraper-hash-directory-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir(&path).unwrap();
        let error = file(&path, u64::MAX, &AtomicBool::new(false)).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Local);
        let _ = std::fs::remove_dir(path);
    }
}
